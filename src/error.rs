use std::fmt;

/// Error codes for user-facing error messages.
///
/// Categories:
/// - ROSE-1xx: Network & server errors
/// - ROSE-2xx: File system errors
/// - ROSE-3xx: Data integrity errors
/// - ROSE-4xx: Launcher & process errors
/// - ROSE-5xx: Initialization errors
#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    // Network (1xx)
    /// Top-level network failure
    CheckForUpdates = 100,
    /// Manifest HTTP request failed
    DownloadManifest = 101,
    /// URL parsing/join failed
    InvalidServerAddress = 102,
    /// HTTP client build failed
    SetupConnection = 103,
    /// Archive init failed
    DownloadArchive = 104,
    /// Chunk stream error
    DownloadChunk = 105,

    // File system (2xx)
    /// Output dir creation failed
    CreateGameFolder = 200,
    /// Local manifest read failed
    ReadLocalData = 201,
    /// Manifest save failed
    SaveProgress = 202,
    /// File open (read) failed
    OpenFileForReading = 203,
    /// File open (write) failed
    OpenFileForWriting = 204,
    /// Directory creation failed
    CreateFolder = 205,
    /// Chunk write failed
    WriteUpdateToDisk = 206,
    /// File metadata read failed
    ReadFileMetadata = 207,

    // Data integrity (3xx)
    /// JSON/format parse error
    InvalidServerData = 300,
    /// Decompression failed
    CorruptDownload = 301,
    /// Hash verification failed
    IntegrityCheckFailed = 302,
    /// Chunk reorder failed
    PrepareFileForUpdate = 303,
    /// Local chunk verify failed
    VerifyLocalFile = 304,
    /// Clone task failed
    ProcessDownloadedData = 305,

    // Launcher & process (4xx)
    /// Old updater deletion failed
    RemoveOldLauncher = 400,
    /// Updater rename failed
    ReplaceLauncherFile = 401,
    /// current_exe() failed
    FindLauncherLocation = 402,
    /// Process spawn failed (updater restart)
    RestartLauncher = 403,
    /// Game exe spawn failed
    LaunchGame = 404,

    // Initialization (5xx)
    /// Logging/tracing setup failed
    InitLogging = 500,
}

impl ErrorCode {
    pub fn code(self) -> u16 {
        self as u16
    }

    fn message(self) -> &'static str {
        match self {
            Self::CheckForUpdates => "Failed to check for updates",
            Self::DownloadManifest => "Could not download update information",
            Self::InvalidServerAddress => "Invalid update server address",
            Self::SetupConnection => "Could not set up the download connection",
            Self::DownloadArchive => "Could not download game data",
            Self::DownloadChunk => "Failed to download a game file chunk",

            Self::CreateGameFolder => "Could not create the game folder",
            Self::ReadLocalData => "Could not read local update data",
            Self::SaveProgress => "Could not save update progress",
            Self::OpenFileForReading => "Could not open game file for reading",
            Self::OpenFileForWriting => "Could not open game file for updating",
            Self::CreateFolder => "Could not create folder",
            Self::WriteUpdateToDisk => "Could not write update to disk",
            Self::ReadFileMetadata => "Could not check game file",

            Self::InvalidServerData => "Received invalid data from the update server",
            Self::CorruptDownload => "Downloaded data appears corrupted",
            Self::IntegrityCheckFailed => "Downloaded data failed integrity check",
            Self::PrepareFileForUpdate => "Could not prepare game file for updating",
            Self::VerifyLocalFile => "Could not verify a local game file",
            Self::ProcessDownloadedData => "Could not process downloaded data",

            Self::RemoveOldLauncher => "Could not remove the old launcher file",
            Self::ReplaceLauncherFile => "Could not replace the launcher file",
            Self::FindLauncherLocation => "Could not determine the launcher location",
            Self::RestartLauncher => "Could not restart the launcher after updating",
            Self::LaunchGame => "Could not start the game",

            Self::InitLogging => "Failed to initialize logging",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[ROSE-{}] {}", self.code(), self.message())
    }
}
