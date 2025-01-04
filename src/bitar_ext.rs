use std::path::Path;

use anyhow::Context;
use bitar::archive_reader::HttpReader;
use bitar::{Archive, ChunkIndex, CloneOutput};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::Url;
use tokio::fs;
use tracing::instrument;

use async_trait::async_trait;

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
        .truncate(true)
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
