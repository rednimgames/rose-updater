#![windows_subsystem = "windows"]
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use async_trait::async_trait;
use clap::Parser;
use fltk::frame::Frame;
use fltk::image::PngImage;
use fltk::{enums::*, prelude::*, *};
use reqwest::Url;
use tokio::fs;
use tokio::fs::File;
use tracing::{debug, error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[cfg(feature = "console")]
use console_subscriber;

use rose_update::{
    clone_remote, launch_button, progress_bar, LocalManifest, LocalManifestFileEntry,
    RemoteManifest, RemoteManifestFileEntry, Updater,
};

const LOCAL_MANIFEST_VERSION: usize = 1;
const UPDATER_OLD_EXT: &str = "old";

#[derive(Clone, Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    /// Remote archive URL
    #[clap(long, default_value = "https://updates.roseonlinegame.com")]
    url: String,

    /// Output directory
    #[clap(long, default_value = ".")]
    output: PathBuf,

    /// Name of manifest file
    #[clap(long, default_value = "manifest.json")]
    manifest_name: String,

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
    #[clap(default_value = "trose.exe")]
    exe: PathBuf,

    /// Arguments for the executable
    /// NOTE: This must be the last option in the command line to properly handle
    #[clap(
        multiple_values = true,
        default_value = "--init --server connect.roseonlinegame.com",
        value_delimiter = ' '
    )]
    exe_args: Vec<String>,

    /// Working directory to run the executable
    #[clap(long, default_value = ".")]
    exe_dir: PathBuf,
}

async fn save_local_manifest(manifest_path: &Path, manfiest: &LocalManifest) -> anyhow::Result<()> {
    if let Some(manifest_parent_dir) = manifest_path.parent() {
        std::fs::create_dir_all(manifest_parent_dir)?;
    }

    let manifest_file = std::fs::File::create(manifest_path)?;
    serde_json::to_writer(manifest_file, &manfiest)?;

    info!("Saved local manifest to {}", manifest_path.display());

    Ok(())
}

enum DownloadResult {
    ApplicationUpdated,
    UpdaterUpdated,
}

async fn get_remote_manifest(
    remote_url: &Url,
    manifest_name: &str,
) -> anyhow::Result<RemoteManifest> {
    info!("Downloading remote manifest");
    // Download our remote manifest file
    let remote_manifest_url = remote_url.join(manifest_name)?;
    Ok(reqwest::get(remote_manifest_url)
        .await?
        .json::<RemoteManifest>()
        .await?)
}

async fn update_updater(
    local_updater_path: &Path,
    updater_output_path: &Path,
    remote_url: &Url,
    main_updater: MainProgressUpdater,
) -> anyhow::Result<()> {
    // When the updater needs to be updated we change the exe name before
    // restarting the process. This step ensures that we delete the old,
    // outdated updater exe.
    let local_updater_path_old = local_updater_path.with_extension(UPDATER_OLD_EXT);
    if local_updater_path_old.exists() {
        fs::remove_file(&local_updater_path_old)
            .await
            .context(format!(
                "Failed to delete the old updater file: {}",
                local_updater_path_old.display()
            ))?;
    }

    info!("Updating updater");

    // We cannot delete or modify a currently executing binary so we rename
    // the currently executing updater to allow us to download the new one
    // with the same name.
    if local_updater_path.exists() {
        fs::rename(&local_updater_path, &local_updater_path_old)
            .await
            .context(format!(
                "Failed to rename the updater from {} to {}",
                local_updater_path.display(),
                local_updater_path_old.display(),
            ))?;
    }

    clone_remote(remote_url, updater_output_path, main_updater)
        .await
        .context(format!("Failed to clone {}", &remote_url))?;

    info!(
        "Cloned {} to {}",
        &remote_url,
        updater_output_path.display()
    );

    Ok(())
}

async fn get_local_manifest(folder: &PathBuf) -> anyhow::Result<LocalManifest> {
    info!("Getting local manifest");

    // Read the manifest file if we can. Otherwise we default to an empty local
    // manifest which we save as a new manifest later.
    let local_manifest = if folder
        .try_exists()
        .context("Failed to get the local manifest")?
    {
        info!("Using existing manifest file: {}", folder.display());

        let file = File::open(&folder).await?;
        match serde_json::from_reader(file.into_std().await) {
            Ok(manifest) => manifest,
            Err(_) => {
                info!("Failed to parse local manifest");
                LocalManifest::default()
            }
        }
    } else {
        LocalManifest::default()
    };

    Ok(local_manifest)
}

fn verify_local_files(
    output: &Path,
    remote_url: &Url,
    remote_manifest: RemoteManifest,
    local_filedata: &HashMap<PathBuf, LocalManifestFileEntry>,
    force_verify: bool,
) -> anyhow::Result<(Vec<(Url, RemoteManifestFileEntry)>, usize, usize)> {
    info!("Checking local files");

    let mut files_to_update = Vec::new();
    let mut total_size = 0;
    let mut already_downloaded_size = 0;
    for remote_entry in remote_manifest.files {
        let output_path = output.join(&remote_entry.source_path);
        let needs_update = || {
            if !output_path.exists() {
                return true;
            }

            if let Some(local_entry) = local_filedata.get(&PathBuf::from(&remote_entry.source_path))
            {
                if local_entry.hash == remote_entry.source_hash {
                    return false;
                }
            }

            true
        };

        total_size += remote_entry.source_size;

        if !force_verify && !needs_update() {
            debug!(
                "Skipping file {} as it is already present",
                output_path.display()
            );
            already_downloaded_size += remote_entry.source_size;
            continue;
        }

        let clone_url = remote_url.join(&remote_entry.path)?;
        files_to_update.push((clone_url, remote_entry));
    }

    Ok((files_to_update, total_size, already_downloaded_size))
}

fn get_remote_files(
    output: &Path,
    files_to_update: Vec<(Url, RemoteManifestFileEntry)>,
    main_updater: MainProgressUpdater,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    tx: tokio::sync::mpsc::Sender<LocalManifestFileEntry>,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let mut clone_tasks = Vec::new();

    for entry in files_to_update {
        let (clone_url, remote_entry) = entry;
        let main_updater = main_updater.clone();
        let output_path = output.join(&remote_entry.source_path);
        let mut cloned_shutdown = shutdown_rx.clone();
        let cloned_tx = tx.clone();

        clone_tasks.push(tokio::spawn(async move {
            info!("Downloading {}", &clone_url);
            tokio::select! {
                res = clone_remote(
                    &clone_url,
                    &output_path,
                    main_updater) => if res.is_ok() {
                        info!("Cloned {} to {}", &clone_url, output_path.display());
                        cloned_tx.send(LocalManifestFileEntry {
                            path: remote_entry.source_path.clone(),
                            hash: remote_entry.source_hash.clone(),
                            size: remote_entry.source_size,
                        }).await.expect("Failed to send clone message");
                    } else {
                        error!("Failed to clone {}", &clone_url);
                    },
                _ = cloned_shutdown.changed() => {
                    info!("Stopped cloning {}", &clone_url);
                }
            }
        }));
    }

    Ok(clone_tasks)
}

async fn process(
    args: &Args,
    main_updater: MainProgressUpdater,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<DownloadResult> {
    let remote_url =
        Url::parse(&args.url).context(format!("Failed to parse the url {}", args.url))?;

    let remote_manifest = tokio::select! {
        res = get_remote_manifest(&remote_url, &args.manifest_name) => res?,
        _ = shutdown_rx.changed() => bail!("Download cancelled")
    };

    // The updater can use different "profiles" to use the same updater for different clients
    let local_manifest_path = args
        .output
        .join("updater")
        .join(remote_url.host_str().unwrap_or("default"))
        .join("local_manifest.json");

    let local_manifest = tokio::select! {
        res = get_local_manifest(&local_manifest_path) => res?,
        _ = shutdown_rx.changed() => bail!("Download cancelled")
    };

    // First, we check if the updater itself needs an update. If it does then we
    // will only update the updater then start the process again to update the
    // rest of the files.
    let updater_output_path = args.output.join(&remote_manifest.updater.source_path);
    let updater_needs_update = remote_manifest.updater.source_hash != local_manifest.updater.hash;

    if args.force_recheck_updater || updater_needs_update {
        let local_updater_path = args.output.join(&remote_manifest.updater.source_path);

        main_updater
            .set_max_progress(remote_manifest.updater.source_size)
            .await;

        let remote = remote_url.join(&remote_manifest.updater.path)?;

        tokio::select! {
            res = update_updater(&local_updater_path, &updater_output_path, &remote, main_updater) => res?,
            _ = shutdown_rx.changed() => bail!("Download cancelled")
        }

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
        Command::new(env::current_exe()?)
            .args(
                env::args()
                    .skip(1)
                    // Prevent infinite loop of update rechecks by removing the forced updater check
                    .filter(|arg| !arg.contains("force-recheck-updater")),
            )
            .spawn()?;

        return Ok(DownloadResult::UpdaterUpdated);
    }

    // Create a lookup table for our local cache data so we can compare to remote manifest
    let mut current_local_filedata: HashMap<PathBuf, LocalManifestFileEntry> = HashMap::new();
    for entry in &local_manifest.files {
        current_local_filedata.insert(PathBuf::from(&entry.path), entry.clone());
    }

    let (files_to_download, total_size, already_downloaded_size) = verify_local_files(
        &args.output,
        &remote_url,
        remote_manifest,
        &current_local_filedata,
        args.verify,
    )?;

    main_updater.set_max_progress(total_size).await;
    main_updater
        .increment_progress(already_downloaded_size)
        .await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<LocalManifestFileEntry>(64);

    let work = tokio::spawn(async move {
        let mut hash_new_local_manifest = HashSet::new();
        let mut new_local_manifest = LocalManifest {
            version: LOCAL_MANIFEST_VERSION,
            updater: local_manifest.updater,
            ..Default::default()
        };

        while let Some(manifest) = rx.recv().await {
            hash_new_local_manifest.insert(PathBuf::from(&manifest.path));
            new_local_manifest.files.push(manifest);
        }

        (hash_new_local_manifest, new_local_manifest)
    });

    let clone_tasks = get_remote_files(
        &args.output,
        files_to_download,
        main_updater,
        shutdown_rx,
        tx,
    )?;

    futures::future::join_all(clone_tasks).await;
    let (hash_new_local_manifest, mut new_local_manifest) = work.await?;

    for (path, local_entry) in current_local_filedata {
        if !hash_new_local_manifest.contains(&path) {
            new_local_manifest.files.push(local_entry);
        }
    }

    save_local_manifest(&local_manifest_path, &new_local_manifest).await?;

    Ok(DownloadResult::ApplicationUpdated)
}

#[derive(Debug)]
enum MainProgressUpdaterEvent {
    SetMaxProgress(usize),
    IncrementProgress(usize),
}

#[derive(Debug)]
enum Message {
    MainProgressUpdate(MainProgressUpdaterEvent),
    Launch,
    Shutdown,
    Error,
}

#[derive(Clone)]
struct MainProgressUpdater {
    sender: app::Sender<Message>,
}

#[async_trait]
impl Updater for MainProgressUpdater {
    async fn set_max_progress(&self, total: usize) {
        self.sender.send(Message::MainProgressUpdate(
            MainProgressUpdaterEvent::SetMaxProgress(total),
        ));
    }

    async fn increment_progress(&self, amount: usize) {
        self.sender.send(Message::MainProgressUpdate(
            MainProgressUpdaterEvent::IncrementProgress(amount),
        ));
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Setup tracing for loggin

    if cfg!(feature = "console") {
        #[cfg(feature = "console")]
        console_subscriber::init();
    } else {
        let subscriber = FmtSubscriber::builder()
            .with_max_level(Level::INFO)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("Critical failure: Failed to set default tracing subscriber");
    }

    // Load application resources
    let icon_bytes = include_bytes!("../../res/client.png");
    let background_bytes = include_bytes!("../../res/Launcher_Alpha_Background.png");

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
    let (tx, rx) = app::channel::<Message>();

    // shutdown channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Create our updaters
    let main_updater = MainProgressUpdater { sender: tx.clone() };

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

        Command::new(&exe)
            .current_dir(&exe_dir)
            .args(&exe_args)
            .spawn()
            .unwrap();

        app.quit();
    });

    let rt = tokio::runtime::Runtime::new().unwrap();

    // Spawn a task to download our updates
    let process_future = rt.spawn(async move {
        let result = process(&args, main_updater, shutdown_rx).await;
        if let Ok(download_result) = result {
            info!("Download task completed");

            match download_result {
                DownloadResult::ApplicationUpdated => {
                    info!("Application updated");
                    tx.send(Message::Launch);
                }
                DownloadResult::UpdaterUpdated => {
                    // The updater itself was updated, we should exit because a new
                    // process was started with the new updater to update the
                    // application.
                    info!("Updater updated");
                    tx.send(Message::Shutdown);
                }
            }
        } else {
            error!(
                "Download task failed or cancelled, error {}",
                result.err().unwrap()
            );
            tx.send(Message::Error);
        }
    });

    while app.wait() {
        if let Some(e) = rx.recv() {
            match e {
                Message::MainProgressUpdate(e) => match e {
                    MainProgressUpdaterEvent::SetMaxProgress(amount) => {
                        main_progress_bar.set_minimum(0);
                        main_progress_bar.set_maximum(amount);
                        main_progress_bar.set_value(0);
                        background_frame.redraw();
                        main_progress_bar.redraw();
                        launch_button.redraw();
                    }
                    MainProgressUpdaterEvent::IncrementProgress(amount) => {
                        main_progress_bar.set_value(main_progress_bar.value() + amount);
                        main_progress_bar.redraw();
                    }
                },
                Message::Launch => {
                    info!("Ready to launch");
                    launch_button.activate();
                    launch_button.change_state(launch_button::LaunchButtonState::Play);
                    launch_button.redraw();
                }
                Message::Shutdown => {
                    info!("Shutting down");
                    break;
                }
                Message::Error => {
                    dialog::alert(
                        (app::screen_size().0 / 2.0) as i32,
                        (app::screen_size().0 / 2.0) as i32,
                        "An error was detected, please restart the launcher",
                    );
                    break;
                }
            }
        }
    }

    rt.block_on(async move {
        let result = shutdown_tx.send(true);
        if result.is_err() {
            info!("Failed to send shutdown message");
        }
    });

    let result = rt.block_on(process_future);
    if result.is_err() {
        error!("Error while closing down download process");
    }

    Ok(())
}
