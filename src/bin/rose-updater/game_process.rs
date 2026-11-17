//! Refuse file work while a game client is using the installation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use rose_update::error::ErrorCode;
use tokio::sync::oneshot;

/// One update or Repair operation. Consent is never saved for later operations.
pub struct Check {
    executable: PathBuf,
    confirmed: AtomicBool,
    confirm: Option<Box<dyn Fn(oneshot::Sender<bool>) + Send + Sync>>,
}

#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Update cancelled")
    }
}

impl std::error::Error for Cancelled {}

impl Check {
    pub fn new(executable: &Path) -> Self {
        Self {
            executable: executable.to_owned(),
            confirmed: AtomicBool::new(false),
            confirm: None,
        }
    }

    pub fn with_confirmation(
        mut self,
        confirm: impl Fn(oneshot::Sender<bool>) + Send + Sync + 'static,
    ) -> Self {
        self.confirm = Some(Box::new(confirm));
        self
    }

    pub async fn ensure_stopped(&self) -> anyhow::Result<()> {
        self.check_snapshot(process_names()).await
    }

    async fn check_snapshot(&self, snapshot: anyhow::Result<Vec<String>>) -> anyhow::Result<()> {
        match snapshot {
            Ok(names) => {
                if names
                    .iter()
                    .any(|name| is_game_process(name, &self.executable))
                {
                    anyhow::bail!(ErrorCode::GameRunning.to_string());
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "Could not inspect running game processes");
                if self.confirmed.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let Some(confirm) = &self.confirm else {
                    // Headless callers cannot answer a dialog.
                    return Err(error).context(ErrorCode::CheckGameProcess.to_string());
                };
                let (reply, response) = oneshot::channel();
                confirm(reply);
                if !response.await.unwrap_or(false) {
                    return Err(Cancelled.into());
                }
                self.confirmed.store(true, Ordering::Relaxed);
                tracing::warn!("Continuing after the user confirmed the game is stopped");
            }
        }
        Ok(())
    }
}

fn is_game_process(name: &str, executable: &Path) -> bool {
    // Windows paths can also occur under Wine. Match whole basenames, never
    // command-line substrings (e.g. a launcher mentioning trose in its args).
    let basename = |name: &str| name.rsplit(['/', '\\']).next().unwrap_or(name).to_owned();
    let name = basename(name);
    let configured = basename(&executable.to_string_lossy());
    name.eq_ignore_ascii_case("trose")
        || name.eq_ignore_ascii_case("trose.exe")
        || (!configured.is_empty() && name.eq_ignore_ascii_case(&configured))
}

#[cfg(windows)]
fn process_names() -> anyhow::Result<Vec<String>> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    struct Snapshot(HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: this owns the handle returned by CreateToolhelp32Snapshot.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    // A process snapshot exposes names without opening each process, including
    // elevated clients that the unelevated launcher cannot otherwise inspect.
    let snapshot = Snapshot(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }?);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut names = Vec::new();
    // SAFETY: snapshot is live and entry has the required size and layout.
    let mut result = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    loop {
        if let Err(error) = result {
            if error.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break;
            }
            return Err(error.into());
        }
        let len = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        names.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
        // SAFETY: the same snapshot and initialized entry remain live.
        result = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    Ok(names)
}

#[cfg(target_os = "linux")]
fn process_names() -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let status = match std::fs::read_to_string(entry.path().join("status")) {
            Ok(status) => status,
            // Processes can exit while we enumerate them.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if let Some(name) = linux_process_name(&status) {
            names.push(name.to_owned());
        } else {
            continue;
        }
        // Name is limited to 15 bytes. Also check the executable for longer
        // custom --exe names; kernel threads/other users may not expose it.
        if let Ok(exe) = std::fs::read_link(entry.path().join("exe")) {
            names.push(
                exe.to_string_lossy()
                    .trim_end_matches(" (deleted)")
                    .to_owned(),
            );
        }
    }
    Ok(names)
}

#[cfg(target_os = "linux")]
fn linux_process_name(status: &str) -> Option<&str> {
    let state = status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))?;
    // Zombies/dead processes no longer hold game files, even if their parent
    // has not reaped them. They must not block updates indefinitely.
    if matches!(state.split_whitespace().next(), Some("Z" | "X")) {
        return None;
    }
    status.lines().find_map(|line| line.strip_prefix("Name:\t"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_names() -> anyhow::Result<Vec<String>> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "comm="])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "ps failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(|line| line.trim().to_owned())
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn unavailable() -> anyhow::Result<Vec<String>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "process access denied",
        )
        .into())
    }

    #[tokio::test]
    async fn unavailable_process_list_without_confirmation_reports_406() {
        let check = Check::new(Path::new("trose"));
        let error = check.check_snapshot(unavailable()).await.unwrap_err();
        assert!(error.to_string().contains("[ROSE-406]"));
        assert!(format!("{error:#}").contains("process access denied"));
    }

    #[tokio::test]
    async fn ok_waits_for_consent_and_only_bypasses_listing_errors_for_this_operation() {
        let (prompts, mut received) = tokio::sync::mpsc::unbounded_channel();
        let check = std::sync::Arc::new(Check::new(Path::new("trose")).with_confirmation(
            move |reply| {
                prompts.send(reply).unwrap();
            },
        ));
        let pending = {
            let check = check.clone();
            tokio::spawn(async move { check.check_snapshot(unavailable()).await })
        };
        let reply = received.recv().await.unwrap();
        assert!(!pending.is_finished(), "file work must wait for OK");
        reply.send(true).unwrap();
        pending.await.unwrap().unwrap();

        check.check_snapshot(unavailable()).await.unwrap();
        assert!(
            received.try_recv().is_err(),
            "do not ask again during the same operation"
        );
        check.check_snapshot(Ok(vec![])).await.unwrap();
        let error = check
            .check_snapshot(Ok(vec!["TROSE.EXE".into()]))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("[ROSE-405]"),
            "consent never bypasses a detected client"
        );

        let new_operation = Check::new(Path::new("trose"));
        assert!(
            new_operation.check_snapshot(unavailable()).await.is_err(),
            "consent must not persist"
        );
    }

    #[tokio::test]
    async fn cancel_or_dismiss_stops_the_operation() {
        for dismiss in [false, true] {
            let check = Check::new(Path::new("trose")).with_confirmation(move |reply| {
                if !dismiss {
                    reply.send(false).unwrap();
                }
            });
            let error = check.check_snapshot(unavailable()).await.unwrap_err();
            assert!(error.is::<Cancelled>());
            assert!(!check.confirmed.load(Ordering::Relaxed));
        }
    }

    #[tokio::test]
    async fn known_process_status_never_asks_for_a_bypass() {
        let check = Check::new(Path::new("trose")).with_confirmation(|_| {
            panic!("only a failed process listing can offer confirmation");
        });
        check
            .check_snapshot(Ok(vec!["other-client".into()]))
            .await
            .unwrap();
        let error = check
            .check_snapshot(Ok(vec!["trose".into()]))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("[ROSE-405]"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exited_linux_processes_do_not_block_updates() {
        assert_eq!(
            linux_process_name("Name:\ttrose\nState:\tS (sleeping)\n"),
            Some("trose")
        );
        assert_eq!(
            linux_process_name("Name:\ttrose\nState:\tR (running)\n"),
            Some("trose")
        );
        assert_eq!(
            linux_process_name("Name:\ttrose\nState:\tZ (zombie)\n"),
            None
        );
        assert_eq!(linux_process_name("Name:\ttrose\nState:\tX (dead)\n"), None);
    }

    #[test]
    fn matches_game_names_and_configured_executable_only() {
        let configured = Path::new("client/custom-client.exe");
        for name in [
            "trose",
            "TROSE.EXE",
            "/Games/ROSE Online/trose",
            r"C:\ROSE\TRose.exe",
            "custom-client.exe",
        ] {
            assert!(is_game_process(name, configured), "{name}");
        }
        for name in [
            "rose-updater",
            "trose.exe.bak",
            "not-trose",
            "trose-helper",
            "launcher --exe trose.exe",
        ] {
            assert!(!is_game_process(name, configured), "{name}");
        }
    }

    // Run a copy of this test binary as a harmless stand-in for a game client.
    // A distinct name keeps parallel update tests from seeing a real trose.
    pub struct Client {
        child: std::process::Child,
        stdout: std::io::BufReader<std::process::ChildStdout>,
        pub executable: std::path::PathBuf,
        _directory: tempfile::TempDir,
    }

    impl Client {
        pub fn start() -> Self {
            use std::io::BufRead;
            let directory = tempfile::tempdir().unwrap();
            // Each test owns a distinct process name, so another concurrent
            // fixture cannot look like a client that this test already closed.
            let executable = directory.path().join(format!(
                "rg-{}{}",
                directory.path().file_name().unwrap().to_string_lossy(),
                if cfg!(windows) { ".exe" } else { "" }
            ));
            std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
            let mut child = std::process::Command::new(&executable)
                .args([
                    "--ignored",
                    "--exact",
                    "game_process::tests::client_process",
                    "--nocapture",
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let mut client = Self {
                child,
                stdout: std::io::BufReader::new(stdout),
                executable,
                _directory: directory,
            };
            assert!((&mut client.stdout)
                .lines()
                .any(|line| line.unwrap().contains("client ready")));
            client
        }

        pub fn stop(&mut self) {
            self.child.stdin.take();
            assert!(self.child.wait().unwrap().success());
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    #[ignore = "helper process started by the process guard regression test"]
    fn client_process() {
        use std::io::{Read, Write};
        println!("client ready");
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read(&mut [0]);
    }
}
