use serde::{Deserialize, Serialize};

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

impl From<&RemoteManifest> for LocalManifest {
    fn from(remote_manifest: &RemoteManifest) -> Self {
        LocalManifest {
            version: remote_manifest.version,
            updater: (&remote_manifest.updater).into(),
            files: remote_manifest
                .files
                .iter()
                .map(|remote_entry| remote_entry.into())
                .collect(),
        }
    }
}
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LocalManifestFileEntry {
    pub path: String,
    pub hash: Vec<u8>,
    pub size: usize,
}

impl From<&RemoteManifestFileEntry> for LocalManifestFileEntry {
    fn from(remote_entry: &RemoteManifestFileEntry) -> Self {
        LocalManifestFileEntry {
            path: remote_entry.source_path.clone(),
            hash: remote_entry.source_hash.clone(),
            size: remote_entry.source_size,
        }
    }
}
