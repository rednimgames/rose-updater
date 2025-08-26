#![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::Context;
use bitar::{ChunkIndex, CloneOutput};
use clap::Parser;
use directories::ProjectDirs;
use fltk::frame::Frame;
use fltk::image::PngImage;
use fltk::{enums::*, prelude::*, *};
use fltk_webview::FromFltkWindow;
use reqwest::Url;
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

#[derive(Clone, Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    /// Remote archive URL
    #[clap(long, default_value = "https://updates2.roseonlinegame.com")]
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
    #[clap(long, default_value = "trose.exe")]
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
                    "Failed to delete old updater at {}",
                    old_updater_temp_path.display()
                )
            })?;
    }

    if local_updater_path.exists() {
        fs::rename(&local_updater_path, &old_updater_temp_path)
            .await
            .context(format!(
                "Failed to rename the updater from {} to {}",
                local_updater_path.display(),
                old_updater_temp_path.display(),
            ))?;
    }

    // Rename the new updater from its temp path to the real path
    if new_updater_temp_path.exists() {
        fs::rename(&new_updater_temp_path, &local_updater_path)
            .await
            .context(format!(
                "Failed to rename the updater from {} to {}",
                new_updater_temp_path.display(),
                local_updater_path.display(),
            ))?;
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

async fn get_remote_files(
    base_url: &Url,
    files_to_update: &[FileToDownload],
    output_dir: &Path,
    progress_state: ProgressState,
) -> anyhow::Result<()> {
    info!(count = files_to_update.len(), "Starting clone process");

    let mut archive_readers = {
        let mut archive_reader_tasks = Vec::new();
        for file_data in files_to_update {
            let file_url = base_url.join(&file_data.remote_path)?;
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
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };

        if !TEXT_FILE_EXTENSIONS.contains(&ext) {
            continue;
        }

        if !path.exists() {
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

    progress_state.set_stage(ProgressStage::CheckingFiles);
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

    for clone_output in &clone_outputs {
        for (_hashsum, chunk_location) in clone_output.chunks().iter_chunks() {
            total_download_chunk_count += 1;
            total_download_chunk_size += chunk_location.size();
        }
    }

    info!(
        chunk_count = total_download_chunk_count,
        chunks_total_size = total_download_chunk_size,
        "Downloading missing chunks"
    );

    progress_state.set_stage(ProgressStage::DownloadingUpdates);
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

    // TODO: Verify files??

    Ok(())
}

async fn update_process(
    args: &Args,
    progress_state: ProgressState,
) -> anyhow::Result<UpdateProcessResult> {
    progress_state.set_stage(ProgressStage::FetchingMetadata);

    fs::create_dir_all(&args.output)
        .await
        .context("Failed to create output directory")?;

    // Get the base URL for our update remote
    let remote_url =
        Url::parse(&args.url).context(format!("Failed to parse the url {}", args.url))?;

    // The updater can use different "profiles" to use the same updater for different clients
    let local_manifest_path = args
        .output
        .join("updater")
        .join(remote_url.host_str().unwrap_or("default"))
        .join("local_manifest.json");

    info!(%remote_url, local_manifest_path=%local_manifest_path.display(), output_dir=%args.output.display(), "Starting update process");

    // Download the remote manifest
    let remote_manifest = download_remote_manifest(&remote_url, &args.manifest_name).await?;

    // Load the local manifest (if it exists)
    let local_manifest = get_or_create_local_manifest(&local_manifest_path).await?;

    // First, we check if the updater itself needs an update. If it does then we
    // will only update the updater then start the process again to update the
    // rest of the files.
    let updater_output_path = args.output.join(&remote_manifest.updater.source_path);
    let updater_needs_update = remote_manifest.updater.source_hash != local_manifest.updater.hash;

    if !args.skip_updater && (args.force_recheck_updater || updater_needs_update) {
        let local_updater_path = args.output.join(&remote_manifest.updater.source_path);

        let remote = remote_url.join(&remote_manifest.updater.path)?;

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
            updater: LocalManifestFileEntry {
                path: remote_manifest.updater.source_path.clone(),
                hash: remote_manifest.updater.source_hash.clone(),
                size: remote_manifest.updater.source_size,
            },
            ..local_manifest
        };

        save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

        info!("Restarting updater");
        process::Command::new(env::current_exe()?)
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
            .spawn()?;

        return Ok(UpdateProcessResult::UpdaterUpdated);
    }

    let files_to_update =
        get_files_to_update(&args.output, &remote_manifest, &local_manifest).await;

    get_remote_files(&remote_url, &files_to_update, &args.output, progress_state).await?;

    let mut new_local_manifest = LocalManifest {
        version: LOCAL_MANIFEST_VERSION,
        updater: local_manifest.updater,
        ..Default::default()
    };

    for file in &remote_manifest.files {
        new_local_manifest.files.push(LocalManifestFileEntry {
            path: file.source_path.clone(),
            hash: file.source_hash.clone(),
            size: file.source_size,
        });
    }

    save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

    Ok(UpdateProcessResult::ApplicationUpdated)
}

#[derive(Debug)]
enum Message {
    Launch,
    Shutdown,
    Error(String),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Setup tracing for logging
    let _log_guard = setup_logging(Level::INFO)?;

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
    webview.bind("open_url", |_, content| {
        let parsed: serde_json::Value = serde_json::from_str(content).unwrap();

        // Open the url in the native browser
        let url = parsed.get(0).and_then(|url_param| url_param.as_str());
        if let Some(url) = url {
            info!("Opening url in native browser: {}", url);
            open::that(url).unwrap();
        }
    });
    webview.init(script);
    webview.navigate("https://roseonlinegame.com/launcher.html");

    // general channel
    let (app_message_sender, app_message_receiver) = app::channel::<Message>();

    // shutdown channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Create our updaters
    let progress_state = ProgressState::default();

    // Clone some args before moving args into download task
    let exe = args.exe.clone();
    let exe_dir = args.exe_dir.clone();
    let exe_args = args.exe_args.clone();

    // When the launch button is clicked we start the application
    launch_button.set_callback(move |_| {
        info!(
            "Executing Command: {}/{} {}",
            exe_dir.display(),
            exe.display(),
            exe_args.join(" ")
        );

        let exe = exe_dir.join(&exe);

        process::Command::new(&exe)
            .current_dir(&exe_dir)
            .args(&exe_args)
            // Don't share handles so child process can exit cleanly
            .stdin(process::Stdio::null())
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null())
            .spawn()
            .unwrap();

        app.quit();
    });

    // Spawn a task to download our updates
    let _ = {
        let progress_state = progress_state.clone();
        let mut shutdown_rx = shutdown_rx.clone();

        tokio::spawn(async move {
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
                let error_string = result.err().unwrap().to_string();
                error!("Download task failed or cancelled, error {}", &error_string);
                app_message_sender.send(Message::Error(error_string));
            }
        })
    };

    while app.wait() {
        if let Some(e) = app_message_receiver.recv() {
            match e {
                Message::Launch => {
                    info!("Ready to launch");
                    launch_button.activate();
                    launch_button.change_state(launch_button::LaunchButtonState::Play);
                    launch_button.redraw();
                    progress_state.set_stage(ProgressStage::Play);
                }
                Message::Shutdown => {
                    info!("Shutting down");
                    break;
                }
                Message::Error(e) => {
                    dialog::alert(
                        (app::screen_size().0 / 2.0) as i32,
                        (app::screen_size().0 / 2.0) as i32,
                        &format!(
                            "An error was detected, please restart the launcher:\nError: {}",
                            e
                        ),
                    );
                    break;
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

            // We need to redraw the button ontop of the background frame after
            // we've redrawn it
            launch_button.redraw();
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
        anyhow::bail!("Failed to get project dirs");
    };

    let log_file_path = project_dirs.data_local_dir().join("rose-updater.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_file_path)?;

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

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set default subscriber");

    Ok(log_guard)
}
