use super::*;
use rose_update::manifest::RemoteManifestFileEntry;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONTENTS: &[u8] = b"AAAABBBBCCCCAAAA";

/// Real bitar archives behind a local HTTP Range server exercise the same scan,
/// reorder, download, and truncate paths used by updates and Repair.
struct UpdateServer {
    url: String,
    manifest: Arc<std::sync::Mutex<RemoteManifest>>,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl UpdateServer {
    async fn start() -> Self {
        let mut files = Vec::new();
        let mut archives = HashMap::new();
        for (name, contents) in [
            ("config.xml", CONTENTS),
            ("unchanged.dat", &b"keep this file"[..]),
        ] {
            let mut archive = Vec::new();
            let info = bitar::api::compress::create_archive(
                contents,
                &mut archive,
                &bitar::api::compress::CreateArchiveOptions {
                    chunker_config: bitar::chunker::Config::FixedSize(4),
                    compression: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let archive_path = format!("{name}.bita");
            archives.insert(format!("/{archive_path}"), archive);
            files.push(RemoteManifestFileEntry {
                path: archive_path,
                source_path: name.into(),
                source_hash: info.source_hash,
                source_size: info.source_length,
            });
        }
        let manifest = Arc::new(std::sync::Mutex::new(RemoteManifest {
            files,
            ..Default::default()
        }));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn({
            let manifest = manifest.clone();
            let requests = requests.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert_ne!(count, 0);
                        request.extend_from_slice(&buffer[..count]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap();
                    requests.lock().unwrap().push(path.into());
                    let body = if path == "/manifest.json" {
                        serde_json::to_vec(&*manifest.lock().unwrap()).unwrap()
                    } else {
                        archives
                            .get(path)
                            .expect("unexpected archive request")
                            .clone()
                    };
                    let range = request.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::to_owned)
                    });
                    let (status, range_header, bytes) = if let Some(range) = range {
                        let (start, end) = range.split_once('-').unwrap();
                        let start: usize = start.parse().unwrap();
                        let end = if end.is_empty() {
                            body.len() - 1
                        } else {
                            end.parse::<usize>().unwrap().min(body.len() - 1)
                        };
                        (
                            "206 Partial Content",
                            format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len()),
                            &body[start..=end],
                        )
                    } else {
                        ("200 OK", String::new(), body.as_slice())
                    };
                    let header = format!("HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n{range_header}Connection: close\r\n\r\n", bytes.len());
                    // A cancelled update may close a request before the body finishes.
                    if socket.write_all(header.as_bytes()).await.is_ok() {
                        let _ = socket.write_all(bytes).await;
                    }
                }
            }
        });
        Self {
            url,
            manifest,
            requests,
            task,
        }
    }

    async fn install(&self, output: &Path, executable: &Path) -> Args {
        let args = Args::try_parse_from([
            "updater",
            "--headless",
            "--skip-updater=false",
            "--url",
            &self.url,
            "--output",
            output.to_str().unwrap(),
            "--exe",
            executable.to_str().unwrap(),
        ])
        .unwrap();
        fs::write(output.join("config.xml"), CONTENTS)
            .await
            .unwrap();
        fs::write(output.join("unchanged.dat"), b"keep this file")
            .await
            .unwrap();
        self.save_manifest(&args).await;
        args
    }

    async fn save_manifest(&self, args: &Args) {
        let manifest = self.manifest.lock().unwrap().clone();
        let cached = build_local_manifest(&manifest, updater_manifest_entry(&manifest));
        save_local_manifest(
            &local_manifest_path(&args.output, &parse_remote_url(&self.url).unwrap()),
            &cached,
        )
        .await
        .unwrap();
    }
}

impl Drop for UpdateServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn current_installation_allows_another_client_and_read_only_verification() {
    let client = game_process::tests::Client::start();
    let output = tempfile::tempdir().unwrap();
    let server = UpdateServer::start().await;
    let mut args = server.install(output.path(), &client.executable).await;
    let file = output.path().join("config.xml");
    let metadata = fs::metadata(&file).await.unwrap();
    let mut read_only = metadata.permissions();
    read_only.set_readonly(true);
    fs::set_permissions(&file, read_only).await.unwrap();

    for (verify, force) in [(false, false), (false, true), (true, false)] {
        args.verify = verify;
        args.force_recheck = force;
        server.requests.lock().unwrap().clear();
        let result = run_file_process(&args, ProgressState::default(), None)
            .await
            .unwrap();
        let UpdateProcessResult::ApplicationUpdated(files) = result else {
            panic!("unexpected self-update")
        };
        assert_eq!(files.repaired, if verify { Some(0) } else { None });
        assert_eq!(fs::read(&file).await.unwrap(), CONTENTS);
        assert_eq!(
            fs::metadata(&file).await.unwrap().modified().unwrap(),
            metadata.modified().unwrap()
        );
        if !verify && !force {
            assert_eq!(
                *server.requests.lock().unwrap(),
                ["/manifest.json"],
                "a current normal launch only needs the manifest"
            );
        }
    }

    // A stale cache alone does not imply that the game files need writing.
    let manifest = local_manifest_path(output.path(), &parse_remote_url(&server.url).unwrap());
    let mut cached = get_or_create_local_manifest(&manifest).await.unwrap();
    cached.files[0].hash = vec![0];
    save_local_manifest(&manifest, &cached).await.unwrap();
    args.verify = false;
    args.force_recheck = false;
    run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap();
    assert_eq!(
        fs::metadata(&file).await.unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );

    let repaired = run_verify_process(
        &args.url,
        &args.manifest_name,
        &args.output,
        &args.exe,
        ProgressState::default(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(repaired.repaired, Some(0));
    fs::set_permissions(&file, metadata.permissions())
        .await
        .unwrap();
}

#[tokio::test]
async fn changes_still_block_until_the_client_exits_without_touching_files_or_manifest() {
    let mut client = game_process::tests::Client::start();
    let output = tempfile::tempdir().unwrap();
    let server = UpdateServer::start().await;
    let mut args = server.install(output.path(), &client.executable).await;
    let manifest = local_manifest_path(output.path(), &parse_remote_url(&server.url).unwrap());
    let file = output.path().join("config.xml");

    // Mark the cached content stale so ordinary updates also scan this file.
    let mut cached = get_or_create_local_manifest(&manifest).await.unwrap();
    cached.files[0].hash = vec![0];
    save_local_manifest(&manifest, &cached).await.unwrap();
    let saved_manifest = fs::read(&manifest).await.unwrap();

    for contents in [
        &b"AAAABBBBXXXXAAAA"[..],      // new chunks
        &b"BBBBAAAACCCCAAAA"[..],      // same chunks at different offsets
        &b"AAAABBBBCCCCAAAAEXTRA"[..], // stale tail
        &b"AAAABBBB"[..],              // truncated file
    ] {
        fs::write(&file, contents).await.unwrap();
        for (verify, force) in [(false, false), (false, true), (true, false)] {
            args.verify = verify;
            args.force_recheck = force;
            let error = run_file_process(&args, ProgressState::default(), None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("[ROSE-405]"), "{error:#}");
            assert_eq!(
                fs::read(&file).await.unwrap(),
                contents,
                "must not delete XML or reorder/truncate it before the guard"
            );
            assert_eq!(fs::read(&manifest).await.unwrap(), saved_manifest);
        }
    }

    fs::remove_file(&file).await.unwrap();
    let error = run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("[ROSE-405]"));
    assert!(
        !file.exists(),
        "must not create a missing file before the guard"
    );
    assert_eq!(fs::read(&manifest).await.unwrap(), saved_manifest);

    // Self-updates and explicitly forced updater rewrites remain guarded.
    args.verify = false;
    args.force_recheck_updater = true;
    let error = run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("[ROSE-405]"));
    args.force_recheck_updater = false;
    server.manifest.lock().unwrap().updater.source_hash = vec![1];
    let error = run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("[ROSE-405]"));
    assert_eq!(fs::read(&manifest).await.unwrap(), saved_manifest);

    // A fresh check after closing the client allows an actual repair, while
    // the unchanged file is never opened for writing.
    client.stop();
    args.skip_updater = true;
    args.verify = true;
    let unchanged = output.path().join("unchanged.dat");
    let permissions = fs::metadata(&unchanged).await.unwrap().permissions();
    let mut read_only = permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&unchanged, read_only).await.unwrap();
    let result = run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap();
    let UpdateProcessResult::ApplicationUpdated(files) = result else {
        panic!("unexpected self-update")
    };
    assert_eq!(files.repaired, Some(1));
    assert_eq!(fs::read(&file).await.unwrap(), CONTENTS);
    assert_eq!(fs::read(&unchanged).await.unwrap(), b"keep this file");

    // Ordinary updates still replace changed XML files completely. The scan's
    // chunk index must be discarded once that file has been deleted.
    fs::write(&file, b"AAAABBBBXXXXAAAA").await.unwrap();
    let mut cached = get_or_create_local_manifest(&manifest).await.unwrap();
    cached.files[0].hash = vec![0];
    save_local_manifest(&manifest, &cached).await.unwrap();
    args.verify = false;
    run_file_process(&args, ProgressState::default(), None)
        .await
        .unwrap();
    assert_eq!(fs::read(&file).await.unwrap(), CONTENTS);

    fs::set_permissions(&unchanged, permissions).await.unwrap();
}
