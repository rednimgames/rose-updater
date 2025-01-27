use std::path::Path;

use anyhow::Context;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::info;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RemoteManifest {
    pub version: usize,
    pub updater: RemoteManifestFileEntry,
    pub files: Vec<RemoteManifestFileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RemoteManifestFileEntry {
    pub path: String,
    pub source_path: String,
    pub source_hash: Vec<u8>,
    pub source_size: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LocalManifest {
    pub version: usize,
    pub updater: LocalManifestFileEntry,
    pub files: Vec<LocalManifestFileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LocalManifestFileEntry {
    pub path: String,
    pub hash: Vec<u8>,
    pub size: usize,
}

pub async fn get_or_create_local_manifest(path: &Path) -> anyhow::Result<LocalManifest> {
    info!("Getting local manifest");

    // Read the manifest file if we can. Otherwise we default to an empty local
    // manifest which we save as a new manifest later.
    let local_manifest = if path
        .try_exists()
        .context("Failed to get the local manifest")?
    {
        info!(local_manifest_path=%path.display(), "Using existing manifest file");

        let manifest_file = fs::File::open(&path).await?;
        match serde_json::from_reader(manifest_file.into_std().await) {
            Ok(manifest) => manifest,
            Err(_) => {
                info!("Failed to parse local manifest");
                LocalManifest::default()
            }
        }
    } else {
        info!("Creating new manifest");
        LocalManifest::default()
    };

    Ok(local_manifest)
}

pub async fn save_local_manifest(
    manifest_path: &Path,
    manfiest: &LocalManifest,
) -> anyhow::Result<()> {
    info!(
        manifest_path =% manifest_path.display(),
        "Saving local manifest"
    );

    if let Some(manifest_parent_dir) = manifest_path.parent() {
        fs::create_dir_all(manifest_parent_dir).await?;
    }

    let manifest_file = fs::File::create(manifest_path).await?;
    serde_json::to_writer(manifest_file.into_std().await, &manfiest)?;

    Ok(())
}

pub async fn download_remote_manifest(
    remote_url: &Url,
    manifest_name: &str,
) -> anyhow::Result<RemoteManifest> {
    let remote_manifest_url = remote_url.join(manifest_name)?;

    info!(url=% remote_manifest_url.as_str(), "Downloading remote manifest");

    Ok(reqwest::get(remote_manifest_url)
        .await?
        .json::<RemoteManifest>()
        .await?)
}
