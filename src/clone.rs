//! # ROSE Updater Clone
//!
//! This module provides utilities for working with bitar archives and cloning
//! data from remote sources to local files.
//!
//! ## Overview
//!
//! The cloning process involves the following steps to ensure a local file
//! matches the remote archive:
//!
//! ### 1. Create a Remote Archive Reader
//!
//! A `RemoteArchiveReader` is responsible for communicating with the remote archive.
//! It wraps an HTTP client to make requests to the remote file and retrieves
//! header information, which contains essential configuration details such as
//! chunk size, number of chunks. Additionally the archive reader is used to
//! stream chunks from the remote archive.
//!
//! ### 2. Scan the Local File for Existing Chunks
//!
//! Using the chunking configuration from the remote archive, the local file is
//! divided into chunks to create an index. This index identifies which chunks
//! already exist locally and which chunks need to be downloaded from the remote
//! archive.
//!
//! ### 3. Initialize the Local Clone Output
//!
//! After determining the state of the local file, the next step is to prepare
//! it for receiving new chunks. This involves reordering existing chunks,
//! resizing the file, etc. A `CloneOutput` object is initialized to manage the
//! cloning process.
//!
//! ### 4. Clone Missing Chunks
//!
//! The final step is downloading the missing chunks from the remote archive and
//! applying them to the local file. This ensures the local file matches the
//! remote archive chunk by chunk, completing the cloning process.
//!
use std::path::Path;

use anyhow::Context;
use bitar::CloneOutput;
use futures::StreamExt;

use tokio::fs;

use crate::{dns::CloudflareResolver, progress::ProgressState};

pub type RemoteArchiveReader = bitar::Archive<bitar::archive_reader::HttpReader>;

const LOCAL_CHUNK_BUFFER_SIZE: usize = 64;
const REMOTE_CHUNK_BUFFER_SIZE: usize = 64;

/// Initiates a bitar archive reader for reading a remote archive over HTTP
pub async fn init_remote_archive_reader(url: reqwest::Url) -> anyhow::Result<RemoteArchiveReader> {
    let client = reqwest::ClientBuilder::new()
        .brotli(true)
        .dns_resolver2(CloudflareResolver::new())
        .build()
        .context("Failed to build request client")?
        .get(url.clone());

    let http_reader = bitar::archive_reader::HttpReader::from_request(client).retries(4);
    let archive = bitar::Archive::try_init(http_reader)
        .await
        .with_context(|| format!("Failed to read remote archive at {}", &url))?;

    Ok(archive)
}

/// Estimate how many chunks will be needed for the local file using the chunk
/// configuration from the remote archive
pub async fn estimate_local_chunk_count(
    archive_reader: &RemoteArchiveReader,
    local_file_path: &Path,
) -> anyhow::Result<u64> {
    if !local_file_path.exists() {
        return Ok(0);
    }

    let chunker_config = archive_reader.chunker_config();
    let chunk_size = match chunker_config {
        bitar::chunker::Config::FixedSize(size) => *size as u64,
        // We don't use RollSum/BuzHash but in the weird case that we hit an
        // archive that does we'll just show 0->100% in one step.
        _ => 1,
    };

    let local_file_metadata = tokio::fs::metadata(local_file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to read file metadata for {}",
                local_file_path.display()
            )
        })?;

    let local_file_size = local_file_metadata.len();
    let local_chunk_count = (local_file_size + chunk_size - 1) / chunk_size;

    Ok(local_chunk_count)
}

/// Build a chunk index for the local file using the chunk configuration from the remote archive
pub async fn build_local_chunk_index(
    archive_reader: &RemoteArchiveReader,
    local_file_path: &Path,
    progress_state: ProgressState,
) -> anyhow::Result<bitar::ChunkIndex> {
    if !local_file_path.exists() {
        return Ok(bitar::ChunkIndex::new_empty(
            archive_reader.chunk_hash_length(),
        ));
    }

    let mut local_file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(local_file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to open the local file for reading at {}",
                local_file_path.display()
            )
        })?;

    let chunker_config = archive_reader.chunker_config();

    // We only use incremental progress for FixedSize because we can estimate the max size beforehand
    let use_incremental_progress = matches!(chunker_config, bitar::chunker::Config::FixedSize(_));

    let mut chunk_stream = chunker_config
        .new_chunker(&mut local_file)
        .map(|stream_chunk| {
            tokio::task::spawn_blocking(|| {
                stream_chunk.map(|(offset, chunk)| (offset, chunk.verify()))
            })
        })
        .buffered(LOCAL_CHUNK_BUFFER_SIZE);

    let mut chunk_index = bitar::ChunkIndex::new_empty(archive_reader.chunk_hash_length());
    while let Some(r) = chunk_stream.next().await {
        let (chunk_offset, verified) = r??;
        let (hash, chunk) = verified.into_parts();
        chunk_index.add_chunk(hash, chunk.len(), &[chunk_offset]);

        if use_incremental_progress {
            progress_state.increment_progress(1);
        }
    }

    if !use_incremental_progress {
        progress_state.increment_progress(1);
    }

    Ok(chunk_index)
}

/// Initialize the local file for cloning by reordering existing chunks if necessary
pub async fn init_local_clone_output(
    archive_reader: &RemoteArchiveReader,
    local_file_path: &Path,
    local_chunk_index: bitar::ChunkIndex,
) -> anyhow::Result<CloneOutput<tokio::fs::File>> {
    if let Some(parent) = local_file_path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "Failed to create directory to clone into: {}",
                parent.display()
            )
        })?;
    }
    let local_file = tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(local_file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to open the local file for cloning at {}",
                local_file_path.display()
            )
        })?;

    let mut clone_output = CloneOutput::new(local_file, archive_reader.build_source_index());
    let _size = clone_output.reorder_in_place(local_chunk_index).await?;
    Ok(clone_output)
}

/// Clone the remote archive to the local file
pub async fn clone_remote_file(
    archive_reader: &mut RemoteArchiveReader,
    clone_output: &mut bitar::CloneOutput<tokio::fs::File>,
    progress_state: ProgressState,
) -> anyhow::Result<()> {
    // We only use incremental progress for FixedSize because we can estimate the max size beforehand
    let use_incremental_progress = matches!(
        archive_reader.chunker_config(),
        bitar::chunker::Config::FixedSize(_)
    );

    let mut chunk_stream = archive_reader
        .chunk_stream(clone_output.chunks())
        .map(|archive_chunk| {
            tokio::task::spawn_blocking(move || -> anyhow::Result<bitar::VerifiedChunk> {
                let compressed = archive_chunk?;
                let verified = compressed.decompress()?.verify()?;
                Ok(verified)
            })
        })
        .buffered(REMOTE_CHUNK_BUFFER_SIZE);

    while let Some(r) = chunk_stream.next().await {
        let verified = r??;
        let bytes_written = clone_output.feed(&verified).await?;

        // When "feeding" verified chunks to the clone output, some chunks may
        // already exist in the target location and the result will be 0 bytes
        // written. In such cases, progress reporting is skipped since no actual
        // data transfer occurred.

        if use_incremental_progress && bytes_written > 0 {
            progress_state.increment_progress(verified.len() as u64);
        }
    }

    if !use_incremental_progress {
        progress_state.increment_progress(1);
    }

    Ok(())
}
