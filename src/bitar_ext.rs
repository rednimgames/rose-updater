use std::path::{Path, PathBuf};

use anyhow::Context;
use async_stream::try_stream;
use bitar::archive_reader::HttpReader;
use bitar::{Archive, ChunkIndex, CloneOutput};
use futures::Stream;
use futures_util::{StreamExt, TryStreamExt};
use reqwest::Url;
use tokio::fs;
use tracing::instrument;

use async_trait::async_trait;

pub struct RemoteFileDownloader {
    remote_archive: bitar::Archive<bitar::archive_reader::HttpReader>,
    output_path: PathBuf,
    output_file: fs::File,
    output_original_size: usize,
    output_chunk_index: bitar::ChunkIndex,
    remote_chunk_index: bitar::ChunkIndex,
}

impl RemoteFileDownloader {
    /// Create a new file downloader
    pub async fn new(url: &Url, output_path: &Path) -> anyhow::Result<Self> {
        let http_reader = bitar::archive_reader::HttpReader::from_url(url.clone()).retries(4);

        // The remote archive is the bita archive (.cba). If the file already
        // exists locally only new chunnks will be downloaded from the archive,
        // otherwise the whole file will be downloaded.
        let remote_archive = Archive::try_init(http_reader)
            .await
            .context(format!("Failed to read archive at {}", &url))?;

        // Create parent directories for output file if they don't exist
        if let Some(output_parent) = output_path.parent() {
            fs::create_dir_all(output_parent).await?;
        }

        // Create or open the file that will be updated
        let output_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&output_path)
            .await
            .context(format!(
                "Failed to open the output file at {}",
                output_path.display()
            ))?;

        let output_original_size = output_file.metadata().await?.len() as usize;

        // Create an empty index for all the chunks of the final output file.
        // This index will be populated by using chunks from the local existing
        // file in addition to chunks from the remote archive.
        let output_chunk_index = ChunkIndex::new_empty(remote_archive.chunk_hash_length());
        let remote_chunk_index = remote_archive.build_source_index();

        Ok(RemoteFileDownloader {
            remote_archive,
            output_path: output_path.into(),
            output_file,
            output_original_size,
            output_chunk_index,
            remote_chunk_index,
        })
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// Size in bytes of output file before downloading any new chunks
    pub fn output_original_size(&self) -> usize {
        self.output_original_size
    }

    /// Number of chunks that will be downloaded from the remote file
    pub fn chunk_download_count(&self) -> usize {
        let mut diff_chunk_index = self.remote_chunk_index.clone();

        self.output_chunk_index
            .strip_chunks_already_in_place(&mut diff_chunk_index);

        diff_chunk_index.len()
    }

    /// Load the chunks of the output file into the file downloader
    ///
    /// The output file will be broken into chunks using the same configuration
    /// as the chunker used to create the remote file. This chunk index is used
    /// to seed data for the download process. If the file contains chunks that
    /// can be re-used then the won't be re-downloaded.
    ///
    /// # Note
    /// This function should be called before the download function.
    ///
    /// # Return
    /// A stream with each item being the size of the chunk processed
    pub async fn load_output_chunks(&mut self) -> impl Stream<Item = anyhow::Result<usize>> + '_ {
        try_stream! {
            // Create a chunker on the output file using the same chunker config
            // that was used to chunk the remote archive file.
            let chunker = self.remote_archive.chunker_config().new_chunker(&mut self.output_file);

            // Promote each chunk to a VerifiedChunk which includes the hashsum for the chunk
            let mut verified_chunk_stream = chunker.map_ok(|(offset, chunk)| (offset, chunk.verify()));

            // Add each chunk to our output chunk index
            while let Some(r) = verified_chunk_stream.next().await {
                let (offset, verified) = r.context("Failed to load more chunks from output file")?;
                let (hash, chunk) = verified.into_parts();
                self.output_chunk_index.add_chunk(hash, chunk.len(), &[offset]);
                yield chunk.len();
            }
        }
    }

    /// clone remote chunks to the output file
    ///
    /// Returns a stream with each item being the size of the chunk processed
    pub async fn clone_remote_chunks(mut self) -> impl Stream<Item = anyhow::Result<usize>> {
        try_stream! {
            // Create output to contain the clone of the archive's source
            let mut output = CloneOutput::new(self.output_file, self.remote_archive.build_source_index());

            // Reorder chunks in the output
            let _size = output.reorder_in_place(self.output_chunk_index).await?;

            // Fetch the rest of the chunks from the archive
            let mut chunk_stream = self.remote_archive.chunk_stream(output.chunks());
            while let Some(result) = chunk_stream.next().await {
                let compressed = result?;
                let unverified = compressed.decompress()?;
                let verified = unverified.verify()?;
                let size = output.feed(&verified).await?;
                yield size;
            }
        }
    }
}
#[async_trait]
pub trait Updater {
    async fn set_max_progress(&self, total: usize);
    async fn increment_progress(&self, amount: usize);
}

#[instrument(skip(updater))]
pub async fn clone_remote<T: Updater>(
    url: &Url,
    output_path: &Path,
    updater: T,
) -> anyhow::Result<()> {
    let http_reader = HttpReader::from_url(url.clone()).retries(4);

    let mut archive = Archive::try_init(http_reader)
        .await
        .context(format!("Failed to read archive at {}", &url))?;

    // Create parent directory
    if let Some(output_parent) = output_path.parent() {
        fs::create_dir_all(output_parent).await?;
    }

    // Create a file for clone output
    let mut output_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(&output_path)
        .await
        .context(format!(
            "Failed to open the output file at {}",
            output_path.display()
        ))?;

    // Scan the output file for chunks and build a chunk index
    let mut output_index = ChunkIndex::new_empty(archive.chunk_hash_length());
    {
        let chunker = archive.chunker_config().new_chunker(&mut output_file);
        let mut chunk_stream = chunker.map_ok(|(offset, chunk)| (offset, chunk.verify()));
        while let Some(r) = chunk_stream.next().await {
            let (offset, verified) = r?;
            let (hash, chunk) = verified.into_parts();
            output_index.add_chunk(hash, chunk.len(), &[offset]);
            updater.increment_progress(chunk.len()).await;
        }
    }

    // Create output to contain the clone of the archive's source
    let mut output = CloneOutput::new(output_file, archive.build_source_index());

    // Reorder chunks in the output
    let _size = output.reorder_in_place(output_index).await?;

    // Fetch the rest of the chunks from the archive
    let mut chunk_stream = archive.chunk_stream(output.chunks());
    while let Some(result) = chunk_stream.next().await {
        let compressed = result?;
        let unverified = compressed.decompress()?;
        let verified = unverified.verify()?;
        let size = output.feed(&verified).await?;
        updater.increment_progress(size).await;
    }

    Ok(())
}
