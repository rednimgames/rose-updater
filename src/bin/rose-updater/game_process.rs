//! Refuse file work while a game client is using the installation.

use std::path::Path;

use anyhow::Context;
use rose_update::error::ErrorCode;

pub fn ensure_stopped(executable: &Path) -> anyhow::Result<()> {
    let names = process_names().context(ErrorCode::CheckGameProcess.to_string())?;
    if names.iter().any(|name| is_game_process(name, executable)) {
        anyhow::bail!(ErrorCode::GameRunning.to_string());
    }
    Ok(())
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
        match std::fs::read_to_string(entry.path().join("comm")) {
            Ok(name) => names.push(name.trim_end_matches('\n').to_owned()),
            // Processes can exit while we enumerate them.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        // comm is limited to 15 bytes. Also check the executable for longer
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
            let executable = directory.path().join(if cfg!(windows) {
                "rose-guard.exe"
            } else {
                "rose-guard"
            });
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
