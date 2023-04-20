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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LocalManifestFileEntry {
    pub path: String,
    pub hash: Vec<u8>,
    pub size: usize,
}
