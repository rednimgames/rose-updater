#![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use anyhow::Context;
use bitar::{ChunkIndex, CloneOutput};
use clap::Parser;
use directories::ProjectDirs;
use fltk::frame::Frame;
use fltk::image::PngImage;
use fltk::{enums::*, prelude::*, *};
use fltk_webview::FromFltkWindow;
use reqwest::Url;
use rose_update::error::ErrorCode;
use rose_update::progress::{ProgressStage, ProgressState};
use tokio::fs;
use tracing::{error, info, Level};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use rose_update::clone::{
    build_local_chunk_index, clone_remote_file, estimate_local_chunk_count,
    init_local_clone_output, init_remote_archive_reader, RemoteArchiveReader,
};
use rose_update::manifest::{
    download_remote_manifest, get_or_create_local_manifest, save_local_manifest, LocalManifest,
    LocalManifestFileEntry, RemoteManifest,
};

pub mod launch_button;
pub mod progress_bar;

const LOCAL_MANIFEST_VERSION: usize = 1;

const TEXT_FILE_EXTENSIONS: &[&str; 1] = &["xml"];

const fn default_url() -> &'static str {
    if cfg!(target_os = "macos") {
        "https://updates2.roseonlinegame.com/macos"
    } else if cfg!(target_os = "linux") {
        "https://updates2.roseonlinegame.com/linux-x64"
    } else {
        "https://updates2.roseonlinegame.com"
    }
}

const fn default_exe() -> &'static str {
    if cfg!(target_os = "windows") {
        "trose.exe"
    } else {
        "trose"
    }
}

#[derive(Clone, Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    /// Remote archive URL
    #[clap(long, default_value = default_url())]
    url: String,

    /// Output directory
    #[clap(long, default_value = ".")]
    output: PathBuf,

    /// Name of manifest file
    #[clap(long, default_value = "manifest.json")]
    manifest_name: String,

    /// Skip checking for updater update and only update data files
    #[clap(long)]
    skip_updater: bool,

    /// Ignore the local manifest in the cache and force all files to be checked
    #[clap(long)]
    force_recheck: bool,

    /// Ignore the local manifest in the cache and force the updater to be rechecked for updates
    #[clap(long)]
    force_recheck_updater: bool,

    /// Enable/Disable debug logs
    #[clap(long)]
    debug: bool,

    /// Verify all local files
    #[clap(long)]
    verify: bool,

    /// Executable to run after updating
    #[clap(long, default_value = default_exe())]
    exe: PathBuf,

    /// Working directory to run the executable
    #[clap(long, default_value = ".")]
    exe_dir: PathBuf,

    /// Arguments for the executable
    /// NOTE: This must be the last option in the command line to properly handle
    #[clap(
        default_value = "--init --server connect.roseonlinegame.com",
        value_delimiter = ' '
    )]
    exe_args: Vec<String>,
}

enum UpdateProcessResult {
    ApplicationUpdated,
    UpdaterUpdated,
}

async fn update_updater(
    local_updater_path: &Path,
    updater_output_path: &Path,
    remote_url: &Url,
    progress_state: ProgressState,
) -> anyhow::Result<()> {
    info!("Updating updater");

    let old_updater_temp_path = local_updater_path.with_extension("old");
    let new_updater_temp_path = local_updater_path.with_extension("new");

    let mut archive_reader = init_remote_archive_reader(remote_url.clone()).await?;
    let mut clone_output = init_local_clone_output(
        &archive_reader,
        &new_updater_temp_path,
        ChunkIndex::new_empty(archive_reader.chunk_hash_length()),
    )
    .await?;

    clone_remote_file(&mut archive_reader, &mut clone_output, progress_state).await?;

    // We cannot delete or modify a currently executing binary so we rename
    // the currently executing updater to allow us to download the new one
    // with the same name.

    if old_updater_temp_path.exists() {
        fs::remove_file(&old_updater_temp_path)
            .await
            .with_context(|| {
                format!(
                    "{} ({})",
                    ErrorCode::RemoveOldLauncher,
                    old_updater_temp_path.display()
                )
            })?;
    }

    if local_updater_path.exists() {
        fs::rename(&local_updater_path, &old_updater_temp_path)
            .await
            .context(ErrorCode::ReplaceLauncherFile.to_string())?;
    }

    // Rename the new updater from its temp path to the real path
    if new_updater_temp_path.exists() {
        fs::rename(&new_updater_temp_path, &local_updater_path)
            .await
            .context(ErrorCode::ReplaceLauncherFile.to_string())?;
    }

    // Set execute permission on Unix platforms
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&local_updater_path, perms)
            .context(ErrorCode::ReplaceLauncherFile.to_string())?;
    }

    info!(
        "Cloned {} to {}",
        &remote_url,
        updater_output_path.display()
    );

    Ok(())
}

#[derive(Debug)]
struct FileToDownload {
    /// Path to file in local directory
    local_path: String,
    /// Path to file at remote URL
    remote_path: String,
}

type VerifyCancelState = Arc<tokio::sync::Mutex<Option<tokio::sync::watch::Sender<bool>>>>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RemoteFilePass {
    scan_stage: ProgressStage,
    transfer_stage: ProgressStage,
    delete_text_files: bool,
}

impl RemoteFilePass {
    const fn update() -> Self {
        Self {
            scan_stage: ProgressStage::CheckingFiles,
            transfer_stage: ProgressStage::DownloadingUpdates,
            delete_text_files: true,
        }
    }

    const fn verify() -> Self {
        Self {
            scan_stage: ProgressStage::VerifyingFiles,
            transfer_stage: ProgressStage::VerifyingFiles,
            delete_text_files: false,
        }
    }
}

fn is_text_file_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };

    TEXT_FILE_EXTENSIONS.contains(&ext)
}

fn should_remove_text_file(pass: RemoteFilePass, path: &Path) -> bool {
    pass.delete_text_files && is_text_file_path(path)
}

fn local_manifest_path(output_dir: &Path, remote_url: &Url) -> PathBuf {
    output_dir
        .join("updater")
        .join(remote_url.host_str().unwrap_or("default"))
        .join("local_manifest.json")
}

fn updater_manifest_entry(remote_manifest: &RemoteManifest) -> LocalManifestFileEntry {
    LocalManifestFileEntry {
        path: remote_manifest.updater.source_path.clone(),
        hash: remote_manifest.updater.source_hash.clone(),
        size: remote_manifest.updater.source_size,
    }
}

fn build_local_manifest(
    remote_manifest: &RemoteManifest,
    updater: LocalManifestFileEntry,
) -> LocalManifest {
    let mut local_manifest = LocalManifest {
        version: LOCAL_MANIFEST_VERSION,
        updater,
        ..Default::default()
    };

    for file in &remote_manifest.files {
        local_manifest.files.push(LocalManifestFileEntry {
            path: file.source_path.clone(),
            hash: file.source_hash.clone(),
            size: file.source_size,
        });
    }

    local_manifest
}

fn parse_remote_url(url: &str) -> anyhow::Result<Url> {
    let mut url_str = url.to_owned();
    if !url_str.ends_with('/') {
        url_str.push('/');
    }

    Url::parse(&url_str).context(format!("{} ({})", ErrorCode::InvalidServerAddress, url))
}

async fn run_verify_process(
    verify_url: &str,
    manifest_name: &str,
    output_dir: &Path,
    progress_state: ProgressState,
) -> anyhow::Result<usize> {
    let remote_url = parse_remote_url(verify_url)?;

    verify_and_repair(&remote_url, manifest_name, output_dir, progress_state).await
}

/// Check which files need to be updated by comparing our local manifest to the remote manifest
async fn get_files_to_update(
    output_dir: &Path,
    remote_manifest: &RemoteManifest,
    local_manifest: &LocalManifest,
) -> Vec<FileToDownload> {
    // Build a lookup table to make it easier to check local manifest values
    let mut local_manifest_data = HashMap::new();
    for entry in &local_manifest.files {
        local_manifest_data.insert(&entry.path, entry.clone());
    }

    // Only return paths in the remote manifest that don't match the local manifest by hash
    remote_manifest
        .files
        .iter()
        .filter_map(|entry| {
            let local_path = output_dir.join(&entry.source_path);

            // Skip updates when the file exists and the hash in the local
            // manifest matches the remote manifest
            if let Some(local_entry) = local_manifest_data.get(&entry.source_path) {
                if local_entry.hash == entry.source_hash && local_path.exists() {
                    return None;
                }
            }

            return Some(FileToDownload {
                local_path: entry.source_path.clone(),
                remote_path: entry.path.clone(),
            });
        })
        .collect()
}

/// Downloads/repairs files from remote. Returns the number of files that had chunks downloaded.
async fn get_remote_files(
    base_url: &Url,
    files_to_update: &[FileToDownload],
    output_dir: &Path,
    progress_state: ProgressState,
    pass: RemoteFilePass,
) -> anyhow::Result<usize> {
    info!(count = files_to_update.len(), "Starting clone process");

    let mut archive_readers = {
        let mut archive_reader_tasks = Vec::new();
        for file_data in files_to_update {
            let file_url = base_url
                .join(&file_data.remote_path)
                .context(ErrorCode::InvalidServerAddress.to_string())?;
            let archive_reader_task = init_remote_archive_reader(file_url);
            archive_reader_tasks.push(archive_reader_task);
        }

        let archive_readers: anyhow::Result<Vec<RemoteArchiveReader>> =
            futures::future::join_all(archive_reader_tasks)
                .await
                .into_iter()
                .collect();
        archive_readers?
    };

    info!(count = archive_readers.len(), "Remote Archives Initialized");

    let local_file_paths: Vec<_> = files_to_update
        .iter()
        .map(|file_data| output_dir.join(&file_data.local_path))
        .collect();

    // Bitar doesn't handle text files well so when one of the text files
    // has changed, we delete it first so bitar will just redownload the
    // whole file.
    for path in &local_file_paths {
        if !path.exists() || !should_remove_text_file(pass, path) {
            continue;
        }

        if let Err(e) = std::fs::remove_file(&path) {
            error!(
                path =? path.display(),
                error =? e,
                "Failed to delete text file"
            )
        }
    }

    let mut total_local_chunk_count = 0;
    for (archive_reader, local_file_path) in archive_readers.iter().zip(&local_file_paths) {
        let chunk_count = estimate_local_chunk_count(archive_reader, &local_file_path).await?;
        total_local_chunk_count += chunk_count;
    }

    info!(
        chunk_count = total_local_chunk_count,
        "Building local chunk indexes"
    );

    progress_state.set_stage(pass.scan_stage);
    progress_state.set_current_progress(0);
    progress_state.set_max_progress(total_local_chunk_count);

    let chunk_indexes = {
        let chunk_index_tasks: Vec<_> = archive_readers
            .iter()
            .zip(&local_file_paths)
            .map(|(archive_reader, local_file_path)| {
                build_local_chunk_index(archive_reader, &local_file_path, progress_state.clone())
            })
            .collect();

        let chunk_indexes: anyhow::Result<Vec<ChunkIndex>> =
            futures::future::join_all(chunk_index_tasks)
                .await
                .into_iter()
                .collect();

        chunk_indexes?
    };

    info!(
        clone_output_count = archive_readers.len(),
        "Initializing clone outputs"
    );

    let mut clone_outputs = {
        let clone_output_tasks = archive_readers
            .iter()
            .zip(&local_file_paths)
            .zip(chunk_indexes)
            .map(|((archive_reader, local_file_path), local_chunk_index)| {
                init_local_clone_output(archive_reader, local_file_path, local_chunk_index)
            });

        let clone_outputs: anyhow::Result<Vec<CloneOutput<tokio::fs::File>>> =
            futures::future::join_all(clone_output_tasks)
                .await
                .into_iter()
                .collect();

        clone_outputs?
    };

    let mut total_download_chunk_count = 0;
    let mut total_download_chunk_size = 0;
    let mut files_repaired = 0;

    for clone_output in &clone_outputs {
        let mut file_needs_repair = false;
        for (_hashsum, chunk_location) in clone_output.chunks().iter_chunks() {
            total_download_chunk_count += 1;
            total_download_chunk_size += chunk_location.size();
            file_needs_repair = true;
        }
        if file_needs_repair {
            files_repaired += 1;
        }
    }

    info!(
        chunk_count = total_download_chunk_count,
        chunks_total_size = total_download_chunk_size,
        "Downloading missing chunks"
    );

    progress_state.set_stage(pass.transfer_stage);
    progress_state.set_current_progress(0);
    progress_state.set_max_progress(total_download_chunk_size as u64);

    {
        let clone_tasks = archive_readers
            .iter_mut()
            .zip(clone_outputs.iter_mut())
            .map(|(archive_reader, clone_output)| {
                clone_remote_file(archive_reader, clone_output, progress_state.clone())
            });

        let clone_results: anyhow::Result<Vec<()>> = futures::future::join_all(clone_tasks)
            .await
            .into_iter()
            .collect();

        clone_results?;
    }

    // bitar's CloneOutput writes chunks by offset but never shrinks the output
    // file. If a local file is larger than the remote source (e.g. left over
    // from an older version with a bigger file), the stale tail bytes remain.
    // With fixed-size chunking those extra bytes shift the final chunk
    // boundary, so the remote's last (partial) chunk is never found locally and
    // gets re-downloaded on every verify, never converging. Truncate each file
    // down to the remote source size so it matches and subsequent passes are
    // clean.
    for (archive_reader, local_file_path) in archive_readers.iter().zip(&local_file_paths) {
        let source_size = archive_reader.total_source_size();
        match fs::metadata(local_file_path).await {
            Ok(metadata) if metadata.len() > source_size => {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(local_file_path)
                    .await
                    .with_context(|| {
                        format!(
                            "{} ({})",
                            ErrorCode::OpenFileForWriting,
                            local_file_path.display()
                        )
                    })?;
                file.set_len(source_size).await.with_context(|| {
                    format!(
                        "{} ({})",
                        ErrorCode::WriteUpdateToDisk,
                        local_file_path.display()
                    )
                })?;
            }
            _ => {}
        }
    }

    Ok(files_repaired)
}

/// Build the full file list from a remote manifest (all files, not just changed ones)
fn build_full_file_list(remote_manifest: &RemoteManifest) -> Vec<FileToDownload> {
    remote_manifest
        .files
        .iter()
        .map(|entry| FileToDownload {
            local_path: entry.source_path.clone(),
            remote_path: entry.path.clone(),
        })
        .collect()
}

/// Verify all files against the remote manifest and repair any that are corrupted.
/// Returns the number of files that were repaired.
async fn verify_and_repair(
    remote_url: &Url,
    manifest_name: &str,
    output_dir: &Path,
    progress_state: ProgressState,
) -> anyhow::Result<usize> {
    progress_state.set_stage(ProgressStage::FetchingMetadata);
    progress_state.set_current_progress(0);
    progress_state.set_max_progress(0);

    let remote_manifest = download_remote_manifest(remote_url, manifest_name).await?;
    let all_files = build_full_file_list(&remote_manifest);

    info!(count = all_files.len(), "Starting verification");

    let files_repaired = get_remote_files(
        remote_url,
        &all_files,
        output_dir,
        progress_state,
        RemoteFilePass::verify(),
    )
    .await?;

    // Update local manifest to match remote after verification
    let local_manifest_path = local_manifest_path(output_dir, remote_url);
    let local_manifest = get_or_create_local_manifest(&local_manifest_path).await?;
    let new_local_manifest = build_local_manifest(&remote_manifest, local_manifest.updater);
    save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

    info!(files_repaired, "Verification complete");

    Ok(files_repaired)
}

async fn update_process(
    args: &Args,
    progress_state: ProgressState,
) -> anyhow::Result<UpdateProcessResult> {
    progress_state.set_stage(ProgressStage::FetchingMetadata);

    fs::create_dir_all(&args.output)
        .await
        .context(ErrorCode::CreateGameFolder.to_string())?;

    // Get the base URL for our update remote
    let remote_url = parse_remote_url(&args.url)?;

    // The updater can use different "profiles" to use the same updater for different clients
    let local_manifest_path = local_manifest_path(&args.output, &remote_url);

    info!(%remote_url, local_manifest_path=%local_manifest_path.display(), output_dir=%args.output.display(), "Starting update process");

    // Download the remote manifest
    let remote_manifest = download_remote_manifest(&remote_url, &args.manifest_name)
        .await
        .context(ErrorCode::CheckForUpdates.to_string())?;

    // Load the local manifest (if it exists)
    let local_manifest = get_or_create_local_manifest(&local_manifest_path)
        .await
        .context(ErrorCode::ReadLocalData.to_string())?;

    // First, we check if the updater itself needs an update. If it does then we
    // will only update the updater then start the process again to update the
    // rest of the files.
    let updater_output_path = args.output.join(&remote_manifest.updater.source_path);
    let updater_needs_update = remote_manifest.updater.source_hash != local_manifest.updater.hash;

    if !args.skip_updater && (args.force_recheck_updater || updater_needs_update) {
        let local_updater_path = args.output.join(&remote_manifest.updater.source_path);

        let remote = remote_url
            .join(&remote_manifest.updater.path)
            .context(ErrorCode::InvalidServerAddress.to_string())?;

        progress_state.set_stage(ProgressStage::UpdatingUpdater);
        progress_state.set_current_progress(0);
        progress_state.set_max_progress(remote_manifest.updater.source_size as u64);

        update_updater(
            &local_updater_path,
            &updater_output_path,
            &remote,
            progress_state,
        )
        .await?;

        // We update the local manifest with only the data for the updater, the
        // rest of the data should be updated the next time we run the updater.
        let new_local_manifest = LocalManifest {
            version: LOCAL_MANIFEST_VERSION,
            updater: updater_manifest_entry(&remote_manifest),
            ..local_manifest
        };

        save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

        info!("Restarting updater");
        let current_exe =
            env::current_exe().context(ErrorCode::FindLauncherLocation.to_string())?;
        process::Command::new(current_exe)
            .args(
                env::args()
                    .skip(1)
                    // Prevent infinite loop of update rechecks by removing the forced updater check
                    .filter(|arg| !arg.contains("force-recheck-updater")),
            )
            // Don't share handles so child process can exit cleanly
            .stdin(process::Stdio::null())
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .spawn()
            .context(ErrorCode::RestartLauncher.to_string())?;

        return Ok(UpdateProcessResult::UpdaterUpdated);
    }

    let files_to_update = if args.force_recheck {
        build_full_file_list(&remote_manifest)
    } else {
        get_files_to_update(&args.output, &remote_manifest, &local_manifest).await
    };

    get_remote_files(
        &remote_url,
        &files_to_update,
        &args.output,
        progress_state.clone(),
        RemoteFilePass::update(),
    )
    .await
    .context(ErrorCode::CheckForUpdates.to_string())?;

    // Set execute permission on the game executable for Unix platforms
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let exe_path = args.output.join(&args.exe);
        if exe_path.exists() {
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&exe_path, perms)
                .context("Failed to set execute permission on game executable")?;
        }
    }

    // Verify all files after update
    let all_files = build_full_file_list(&remote_manifest);
    let files_repaired = get_remote_files(
        &remote_url,
        &all_files,
        &args.output,
        progress_state,
        RemoteFilePass::verify(),
    )
    .await
    .context(ErrorCode::CheckForUpdates.to_string())?;

    if files_repaired > 0 {
        info!(
            files_repaired,
            "Repaired files during post-update verification"
        );
    }

    let new_local_manifest = build_local_manifest(&remote_manifest, local_manifest.updater);

    save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

    Ok(UpdateProcessResult::ApplicationUpdated)
}

#[derive(Debug)]
enum Message {
    Launch,
    Shutdown,
    Error { message: String, details: String },
    VerifyStarted,
    VerifyComplete { repaired: usize },
    VerifyCancelled,
    VerifyFailed(String),
}

fn restore_ready_state(
    launch_button: &mut launch_button::LaunchButton,
    verify_button: &mut button::Button,
    progress_state: &ProgressState,
) {
    launch_button.activate();
    launch_button.change_state(launch_button::LaunchButtonState::Play);
    launch_button.redraw();

    verify_button.set_label("Verify");
    verify_button.activate();
    verify_button.show();
    verify_button.redraw();

    progress_state.set_stage(ProgressStage::Play);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Setup tracing for logging
    let _log_guard = setup_logging(Level::INFO).inspect_err(|e| {
        eprintln!("Error setting up logging, error: {e}");

        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Initialization Error")
            .set_description(e.to_string())
            .show();
    })?;

    // Load application resources
    let icon_bytes = include_bytes!("../../../res/client.png");
    let background_bytes = include_bytes!("../../../res/Launcher_Alpha_Background.png");

    let mut background_image = PngImage::from_data(background_bytes).unwrap();

    let app = app::App::default().with_scheme(app::AppScheme::Gtk);

    let mut win = window::DoubleWindow::default()
        .with_size(780, 630)
        .center_screen()
        .with_label("ROSE Online Updater");

    let mut background_frame = Frame::new(0, 0, 780, 630, "");
    background_frame.draw(move |_| {
        background_image.draw(0, 0, 780, 630);
    });

    let mut main_progress_bar = progress_bar::ProgressBar::new(12, 547);

    let mut launch_button = launch_button::LaunchButton::new(572, 547);
    launch_button.deactivate();

    // Verify button: small text button under the Play button
    let mut verify_button = button::Button::new(572, 607, 196, 20, "Verify");
    verify_button.set_label_size(11);
    verify_button.set_color(Color::from_rgb(33, 26, 39));
    verify_button.set_label_color(Color::White);
    verify_button.set_frame(FrameType::FlatBox);
    verify_button.deactivate();
    verify_button.hide();

    let mut webview_win = window::Window::default().with_size(780, 530).with_pos(0, 0);
    webview_win.set_border(false);
    webview_win.set_frame(FrameType::NoBox);
    webview_win.make_resizable(false);

    let icon = image::PngImage::from_data(icon_bytes)?;
    win.set_icon(Some(icon));

    win.end();
    win.show();

    // Script used in the webview to force links to be opened in the native
    // browser rather than in the webview.
    let script = "
    window.onload = function() {
        const links = document.getElementsByTagName('a');
        for (const link of links) {
            link.onclick = function() {
                open_url(link.href);
                return false; // prevent default
            }
        }
    };
    ";

    // Create the webview
    let webview = fltk_webview::Webview::create(false, &mut webview_win);
    if let Err(e) = webview.bind("open_url", |_, content| {
        let parsed: serde_json::Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to parse webview callback data: {}", e);
                return;
            }
        };

        // Open the url in the native browser
        let url = parsed.get(0).and_then(|url_param| url_param.as_str());
        if let Some(url) = url {
            info!("Opening url in native browser: {}", url);
            if let Err(e) = open::that(url) {
                error!("Failed to open URL in browser: {}", e);
            }
        }
    }) {
        error!("Failed to bind webview callback: {}", e);
    }
    if let Err(e) = webview.init(script) {
        error!("Failed to initialize webview: {}", e);
    }
    if let Err(e) = webview.navigate("https://roseonlinegame.com/launcher.html") {
        error!("Failed to navigate webview: {}", e);
    }

    // general channel
    let (app_message_sender, app_message_receiver) = app::channel::<Message>();

    // shutdown channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // verify cancellation channel
    let verify_cancel_tx: VerifyCancelState = Arc::new(tokio::sync::Mutex::new(None));

    // Create our updaters
    let progress_state = ProgressState::default();

    // Clone some args before moving args into download task
    let exe = args.exe.clone();
    let exe_dir = args.exe_dir.clone();
    let exe_args = args.exe_args.clone();

    // Store args needed for verify button
    let verify_url = args.url.clone();
    let verify_manifest_name = args.manifest_name.clone();
    let verify_output = args.output.clone();

    // When the launch button is clicked we start the application
    launch_button.set_callback(move |_| {
        info!(
            "Executing Command: {}/{} {}",
            exe_dir.display(),
            exe.display(),
            exe_args.join(" ")
        );

        let exe = exe_dir.join(&exe);

        match process::Command::new(&exe)
            .current_dir(&exe_dir)
            .args(&exe_args)
            // Don't share handles so child process can exit cleanly
            .stdin(process::Stdio::null())
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                app.quit();
            }
            Err(e) => {
                error!("Failed to launch game: {}", e);
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("Launch Error")
                    .set_description(format!(
                        "{}\n\nMake sure '{}' exists in the game folder.\n\nDetails: {}",
                        ErrorCode::LaunchGame,
                        exe.display(),
                        e
                    ))
                    .show();
            }
        }
    });

    // Verify button callback
    {
        let app_message_sender = app_message_sender.clone();
        let progress_state = progress_state.clone();
        let verify_cancel_tx = verify_cancel_tx.clone();
        let verify_url = verify_url.clone();
        let verify_manifest_name = verify_manifest_name.clone();
        let verify_output = verify_output.clone();

        verify_button.set_callback(move |btn| {
            let cancel_tx = verify_cancel_tx.clone();
            let is_verifying = btn.label() == "Stop";

            if is_verifying {
                // Cancel the running verification
                let cancel_tx = cancel_tx.clone();
                tokio::spawn(async move {
                    let guard = cancel_tx.lock().await;
                    if let Some(tx) = guard.as_ref() {
                        let _ = tx.send(true);
                    }
                });
                return;
            }

            // Start verification
            btn.set_label("Stop");
            btn.redraw();
            app_message_sender.send(Message::VerifyStarted);

            let app_message_sender = app_message_sender.clone();
            let progress_state = progress_state.clone();
            let verify_url = verify_url.clone();
            let verify_manifest_name = verify_manifest_name.clone();
            let verify_output = verify_output.clone();

            let (cancel_send, mut cancel_recv) = tokio::sync::watch::channel(false);
            let cancel_tx_clone = cancel_tx.clone();
            tokio::spawn(async move {
                // Store the cancel sender so the Stop button can use it
                {
                    let mut guard = cancel_tx_clone.lock().await;
                    *guard = Some(cancel_send);
                }

                let result = tokio::select! {
                    res = run_verify_process(
                        &verify_url,
                        &verify_manifest_name,
                        &verify_output,
                        progress_state,
                    ) => res,
                    _ = cancel_recv.changed() => {
                        info!("Verification cancelled by user");
                        app_message_sender.send(Message::VerifyCancelled);
                        // Clear the cancel sender
                        let mut guard = cancel_tx_clone.lock().await;
                        *guard = None;
                        return;
                    }
                };

                // Clear the cancel sender
                {
                    let mut guard = cancel_tx_clone.lock().await;
                    *guard = None;
                }

                match result {
                    Ok(repaired) => {
                        app_message_sender.send(Message::VerifyComplete { repaired });
                    }
                    Err(e) => {
                        error!("Verification failed: {}", e);
                        app_message_sender
                            .send(Message::VerifyFailed(format!("Verification failed: {}", e)));
                    }
                }
            });
        });
    }

    // Spawn a task to download our updates (or verify only if --verify flag is set)
    let _ = {
        let progress_state = progress_state.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        let verify_only = args.verify;
        let verify_url_for_task = args.url.clone();
        let verify_manifest_for_task = args.manifest_name.clone();
        let verify_output_for_task = args.output.clone();

        tokio::spawn(async move {
            if verify_only {
                // --verify flag: skip update, go straight to verify+repair
                let result = tokio::select! {
                    res = run_verify_process(
                        &verify_url_for_task,
                        &verify_manifest_for_task,
                        &verify_output_for_task,
                        progress_state,
                    ) => res,
                    _ = shutdown_rx.changed() => Err(anyhow::anyhow!("Verification cancelled"))
                };

                match result {
                    Ok(repaired) => {
                        app_message_sender.send(Message::VerifyComplete { repaired });
                    }
                    Err(e) => {
                        error!("Verification failed: {}", e);
                        app_message_sender.send(Message::Error {
                            message: e.to_string(),
                            details: format!("{:#}", e),
                        });
                    }
                }
                return;
            }

            let result = tokio::select! {
                res = update_process(&args, progress_state) => res,
                _ = shutdown_rx.changed() => Err(anyhow::anyhow!("Download cancelled"))
            };

            if let Ok(download_result) = result {
                info!("Download task completed");

                match download_result {
                    UpdateProcessResult::ApplicationUpdated => {
                        info!("Application updated");
                        app_message_sender.send(Message::Launch);
                    }
                    UpdateProcessResult::UpdaterUpdated => {
                        // The updater itself was updated, we should exit because a new
                        // process was started with the new updater to update the
                        // application.
                        info!("Updater updated");
                        app_message_sender.send(Message::Shutdown);
                        app::awake();
                    }
                }
            } else {
                let err = result.err().unwrap();
                let message = err.to_string();
                let details = format!("{:#}", err);
                error!("Download task failed or cancelled: {}", &details);
                app_message_sender.send(Message::Error { message, details });
            }
        })
    };

    while app.wait() {
        if let Some(e) = app_message_receiver.recv() {
            match e {
                Message::Launch => {
                    info!("Ready to launch");
                    restore_ready_state(&mut launch_button, &mut verify_button, &progress_state);
                }
                Message::Shutdown => {
                    info!("Shutting down");
                    break;
                }
                Message::Error { message, details } => {
                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Error)
                        .set_title("Update Error")
                        .set_description(format!(
                            "{}\n\nIf this keeps happening, check your internet connection or try again later.\n\nDetails: {}",
                            message,
                            details
                        ))
                        .show();
                    break;
                }
                Message::VerifyStarted => {
                    launch_button.deactivate();
                    launch_button.redraw();
                }
                Message::VerifyComplete { repaired } => {
                    info!(repaired, "Verification complete");
                    restore_ready_state(&mut launch_button, &mut verify_button, &progress_state);

                    let message = if repaired == 0 {
                        "All files verified successfully".to_string()
                    } else {
                        format!("Repaired {} file(s)", repaired)
                    };

                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Info)
                        .set_title("Verification Complete")
                        .set_description(&message)
                        .show();
                }
                Message::VerifyCancelled => {
                    info!("Verification cancelled");
                    restore_ready_state(&mut launch_button, &mut verify_button, &progress_state);
                }
                Message::VerifyFailed(e) => {
                    error!("Manual verification failed: {}", e);
                    restore_ready_state(&mut launch_button, &mut verify_button, &progress_state);
                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Error)
                        .set_title("Verification Error")
                        .set_description(&e)
                        .show();
                }
            }
        }

        let max_progress = progress_state.max_progress() as usize;
        if main_progress_bar.maximum() != max_progress {
            main_progress_bar.set_maximum(max_progress);
            main_progress_bar.set_value(0);

            // To reset the progress bar area we need to redraw the background
            // and then draw the progress bar on top of it
            background_frame.redraw();
            main_progress_bar.redraw();

            // We need to redraw the buttons ontop of the background frame after
            // we've redrawn it
            launch_button.redraw();
            verify_button.redraw();
        }

        let current_progress = progress_state.current_progress() as usize;
        if main_progress_bar.value() != current_progress {
            main_progress_bar.set_value(current_progress);
            main_progress_bar.redraw();
        }

        let current_stage = progress_state.current_stage();
        if main_progress_bar.current_stage() != current_stage {
            main_progress_bar.set_stage(current_stage);
            main_progress_bar.redraw();
        }
    }

    info!("Sending shutdown signal");
    shutdown_tx.send(true)?;
    info!("Terminating application");
    Ok(())
}

fn setup_logging(
    level: tracing::Level,
) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let Some(project_dirs) = ProjectDirs::from("com", "Rednim Games", "ROSE Online") else {
        anyhow::bail!("{}", ErrorCode::InitLogging);
    };

    let log_file_path = project_dirs.data_local_dir().join("rose-updater.log");
    if let Some(log_file_dir) = log_file_path.parent() {
        std::fs::create_dir_all(log_file_dir).context(ErrorCode::InitLogging.to_string())?;
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_file_path)
        .context(ErrorCode::InitLogging.to_string())?;

    let env_filter = format!("{},hyper=info,mio=info", level);
    let (non_blocking_file_appender, log_guard) = tracing_appender::non_blocking(log_file);

    let stdout_layer = tracing_subscriber::fmt::layer()
        .event_format(
            tracing_subscriber::fmt::format()
                .with_file(true)
                .with_line_number(true),
        )
        .pretty()
        .with_filter(tracing_subscriber::EnvFilter::new(&env_filter));

    let file_layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_ansi(false)
        .with_line_number(false)
        .with_file(false)
        .with_target(false)
        .with_writer(move || non_blocking_file_appender.clone())
        .with_filter(tracing_subscriber::EnvFilter::new(&env_filter));

    let subscriber = tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer);

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| anyhow::anyhow!("{}: {}", ErrorCode::InitLogging, e))?;

    Ok(log_guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_pass_uses_update_stages_and_text_workaround() {
        let pass = RemoteFilePass::update();

        assert_eq!(pass.scan_stage, ProgressStage::CheckingFiles);
        assert_eq!(pass.transfer_stage, ProgressStage::DownloadingUpdates);
        assert!(pass.delete_text_files);
    }

    #[test]
    fn verify_pass_keeps_verify_stage_without_text_workaround() {
        let pass = RemoteFilePass::verify();

        assert_eq!(pass.scan_stage, ProgressStage::VerifyingFiles);
        assert_eq!(pass.transfer_stage, ProgressStage::VerifyingFiles);
        assert!(!pass.delete_text_files);
    }

    #[test]
    fn only_update_pass_removes_xml_files() {
        let xml_path = Path::new("data.xml");
        let bin_path = Path::new("data.stb");

        assert!(should_remove_text_file(RemoteFilePass::update(), xml_path));
        assert!(!should_remove_text_file(RemoteFilePass::verify(), xml_path));
        assert!(!should_remove_text_file(RemoteFilePass::update(), bin_path));
    }
}
