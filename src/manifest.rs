use std::{fmt, path::Path, str::FromStr};

use anyhow::Context;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::info;

use crate::{dns, error::ErrorCode};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RemoteManifest {
    pub version: usize,
    pub updater: RemoteManifestFileEntry,
    pub files: Vec<RemoteManifestFileEntry>,
    /// Windows x64 prerequisite. Omitted by legacy and non-Windows manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vc_runtime_min_version: Option<RuntimeVersion>,
}

/// A Windows file version, compared numerically (missing build/revision = zero).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct RuntimeVersion(pub [u16; 4]);

impl FromStr for RuntimeVersion {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.split('.').collect();
        let invalid =
            || "Runtime version must have 2 to 4 numeric components (e.g. 14.44)".to_string();
        if !(2..=4).contains(&parts.len()) {
            return Err(invalid());
        }
        let mut version = [0; 4];
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            version[index] = part.parse().map_err(|_| invalid())?;
        }
        Ok(Self(version))
    }
}

impl TryFrom<String> for RuntimeVersion {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for RuntimeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.0[0], self.0[1])?;
        if self.0[2] != 0 || self.0[3] != 0 {
            write!(f, ".{}", self.0[2])?;
        }
        if self.0[3] != 0 {
            write!(f, ".{}", self.0[3])?;
        }
        Ok(())
    }
}

impl From<RuntimeVersion> for String {
    fn from(value: RuntimeVersion) -> Self {
        value.to_string()
    }
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

        let manifest_file = fs::File::open(&path)
            .await
            .context(ErrorCode::ReadLocalData.to_string())?;
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
        fs::create_dir_all(manifest_parent_dir)
            .await
            .context(ErrorCode::SaveProgress.to_string())?;
    }

    let manifest_file = fs::File::create(manifest_path)
        .await
        .context(ErrorCode::SaveProgress.to_string())?;
    serde_json::to_writer(manifest_file.into_std().await, &manfiest)
        .context(ErrorCode::SaveProgress.to_string())?;

    Ok(())
}

pub async fn download_remote_manifest(
    remote_url: &Url,
    manifest_name: &str,
) -> anyhow::Result<RemoteManifest> {
    let remote_manifest_url = remote_url
        .join(manifest_name)
        .context(ErrorCode::InvalidServerAddress.to_string())?;

    info!(url=% remote_manifest_url.as_str(), "Downloading remote manifest");

    let response = dns::client_builder()
        .build()
        .context(ErrorCode::SetupConnection.to_string())?
        .get(remote_manifest_url)
        .send()
        .await
        .context(ErrorCode::DownloadManifest.to_string())?;

    let manifest = response
        .json::<RemoteManifest>()
        .await
        .context(ErrorCode::InvalidServerData.to_string())?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_manifest_has_no_runtime_requirement() {
        let mut json = serde_json::to_value(RemoteManifest::default()).unwrap();
        assert!(json.get("vc_runtime_min_version").is_none());
        let legacy: RemoteManifest = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(legacy.vc_runtime_min_version, None);

        json["vc_runtime_min_version"] = "14.50.12345.6".into();
        let manifest: RemoteManifest = serde_json::from_value(json).unwrap();
        assert_eq!(
            manifest.vc_runtime_min_version,
            Some(RuntimeVersion([14, 50, 12345, 6]))
        );
        assert_eq!(
            serde_json::to_value(manifest).unwrap()["vc_runtime_min_version"],
            "14.50.12345.6"
        );
    }

    #[test]
    fn runtime_versions_compare_numeric_components() {
        let version = |v: &str| v.parse::<RuntimeVersion>().unwrap();
        assert!(version("14.9") < version("14.44"));
        assert!(version("14.44.123") < version("14.44.123.1"));
        assert_eq!(version("14.44"), version("14.44.0.0"));
        assert!(version("15.0") > version("14.99.65535.65535"));
    }

    #[test]
    fn malformed_requirements_fail_instead_of_disabling_the_check() {
        for value in [
            "",
            "14",
            "14.",
            "14.44.0.0.1",
            "14.-1",
            "14.+44",
            "14.65536",
            "14.44 ",
            "latest",
        ] {
            assert!(
                serde_json::from_value::<RuntimeVersion>(value.into()).is_err(),
                "{value}"
            );
        }
    }
}
