//! Manifest-driven Windows Visual C++ runtime prerequisite.

use rose_update::manifest::RuntimeVersion;
use rose_update::progress::ProgressState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Installed,
    RestartRequired,
}

impl Outcome {
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::UpToDate => None,
            Self::Installed => Some("Installed the Microsoft Visual C++ Redistributable."),
            Self::RestartRequired => Some("Installed the Microsoft Visual C++ Redistributable. Restart your computer before playing."),
        }
    }

    pub fn ready_to_play(self) -> bool {
        self != Self::RestartRequired
    }
}

/// Must be awaited outside the cancellable file-download/verification phase.
pub async fn ensure_installed(
    required: Option<RuntimeVersion>,
    progress: ProgressState,
) -> anyhow::Result<Outcome> {
    #[cfg(windows)]
    {
        use anyhow::Context;
        windows_impl::ensure_installed(required, progress)
            .await
            .context("Couldn't install the required Microsoft Visual C++ Redistributable")
    }
    #[cfg(not(windows))]
    {
        let _ = (required, progress);
        Ok(Outcome::UpToDate)
    }
}

// Keep the decision and post-install verification testable without elevation.
#[cfg(any(windows, test))]
async fn ensure_with<F, I, R>(
    required: Option<RuntimeVersion>,
    mut installed_version: F,
    install: I,
) -> anyhow::Result<Outcome>
where
    F: FnMut() -> Option<RuntimeVersion>,
    I: FnOnce() -> R,
    R: std::future::Future<Output = anyhow::Result<u32>>,
{
    let Some(required) = required else {
        return Ok(Outcome::UpToDate);
    };
    if installed_version().is_some_and(|version| version >= required) {
        return Ok(Outcome::UpToDate);
    }

    let exit_code = install().await?;
    tracing::info!(exit_code, "Visual C++ redistributable installer finished");
    match exit_code {
        // A replacement DLL may only take effect on reboot.
        1641 | 3010 => Ok(Outcome::RestartRequired),
        // Burn can also return HRESULT_FROM_WIN32(ERROR_PRODUCT_VERSION).
        0 | 1638 | 0x80070666 => {
            let actual = installed_version();
            if !actual.is_some_and(|version| version >= required) {
                anyhow::bail!(
                    "The installer exited with code {}, but the runtime is still missing or older than {}. Repair the Microsoft Visual C++ Redistributable in Windows Installed apps, then try again.",
                    exit_code, required
                );
            }
            Ok(if exit_code == 0 {
                Outcome::Installed
            } else {
                Outcome::UpToDate
            })
        }
        code => anyhow::bail!("The runtime installer exited with code {}", code),
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use tracing::info;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    use super::{ensure_with, Outcome};
    use rose_update::manifest::RuntimeVersion;
    use rose_update::progress::{ProgressStage, ProgressState};

    const REDIST_URL: &str = "https://aka.ms/vc14/vc_redist.x64.exe";

    pub async fn ensure_installed(
        required: Option<RuntimeVersion>,
        progress: ProgressState,
    ) -> anyhow::Result<Outcome> {
        ensure_with(required, installed_version, || install(progress)).await
    }

    async fn install(progress: ProgressState) -> anyhow::Result<u32> {
        progress.set_stage(ProgressStage::InstallingRuntime);
        progress.set_current_progress(0);
        progress.set_max_progress(0);
        info!("Installing the Visual C++ runtime");

        let bytes = rose_update::dns::client_builder()
            .build()
            .context("Setting up the runtime download failed")?
            .get(REDIST_URL)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .context("Download failed")?
            .bytes()
            .await
            .context("Download failed")?;
        // Separate concurrent launchers, and clean up on both failure and success.
        // The blocking task owns the directory until the installer has exited.
        let directory = tempfile::Builder::new()
            .prefix("rose-vc-runtime-")
            .tempdir()?;
        let installer = directory.path().join("vc_redist.x64.exe");
        tokio::fs::write(&installer, &bytes)
            .await
            .context("Saving the runtime installer failed")?;
        tokio::task::spawn_blocking(move || {
            let _directory = directory;
            run_elevated(&installer)
        })
        .await?
    }

    /// Version of the 64-bit msvcp140.dll in System32, the copy the game loads.
    fn installed_version() -> Option<RuntimeVersion> {
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let dll = PathBuf::from(system_root)
            .join("System32")
            .join("msvcp140.dll");
        let dll_wide = to_wide(dll.as_os_str());
        let dll_name = PCWSTR(dll_wide.as_ptr());

        unsafe {
            let size = GetFileVersionInfoSizeW(dll_name, None);
            if size == 0 {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            GetFileVersionInfoW(dll_name, None, size, data.as_mut_ptr() as *mut c_void).ok()?;

            let mut info: *mut c_void = std::ptr::null_mut();
            let mut length = 0u32;
            if !VerQueryValueW(
                data.as_ptr() as *const c_void,
                w!("\\"),
                &mut info,
                &mut length,
            )
            .as_bool()
                || info.is_null()
                || length < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
            {
                return None;
            }
            let info = std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO);
            Some(RuntimeVersion([
                (info.dwFileVersionMS >> 16) as u16,
                info.dwFileVersionMS as u16,
                (info.dwFileVersionLS >> 16) as u16,
                info.dwFileVersionLS as u16,
            ]))
        }
    }

    /// Runs the redistributable quietly behind a UAC prompt and waits for its exit code.
    fn run_elevated(installer: &Path) -> anyhow::Result<u32> {
        let file = to_wide(installer.as_os_str());
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = w!("runas");
        info.lpFile = PCWSTR(file.as_ptr());
        info.lpParameters = w!("/install /quiet /norestart");
        info.nShow = 0; // SW_HIDE

        unsafe {
            ShellExecuteExW(&mut info).map_err(|error| anyhow::anyhow!("{}", error))?;
            if info.hProcess.is_invalid() {
                anyhow::bail!("the installer did not start");
            }
            if WaitForSingleObject(info.hProcess, INFINITE) != WAIT_OBJECT_0 {
                let error = windows::core::Error::from_thread();
                let _ = CloseHandle(info.hProcess);
                return Err(error.into());
            }
            let mut exit_code = 0u32;
            let result = GetExitCodeProcess(info.hProcess, &mut exit_code);
            let _ = CloseHandle(info.hProcess);
            result?;
            Ok(exit_code)
        }
    }

    fn to_wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn version(value: &str) -> RuntimeVersion {
        value.parse().unwrap()
    }

    #[tokio::test]
    async fn no_requirement_or_current_runtime_never_runs_an_installer() {
        for required in [None, Some(version("14.44")), Some(version("14.50"))] {
            let result = ensure_with(
                required,
                || Some(version("14.50")),
                || async { panic!("must not download or elevate when already current") },
            )
            .await
            .unwrap();
            assert_eq!(result, Outcome::UpToDate);
        }
    }

    #[tokio::test]
    async fn manifest_bump_and_missing_runtime_trigger_installation() {
        for before in [None, Some(version("14.44"))] {
            let current = Cell::new(before);
            let result = ensure_with(
                Some(version("14.50")),
                || current.get(),
                || async {
                    current.set(Some(version("14.50")));
                    Ok(0)
                },
            )
            .await
            .unwrap();
            assert_eq!(result, Outcome::Installed);
        }
    }

    #[tokio::test]
    async fn success_or_registered_newer_version_must_satisfy_the_dll_check() {
        for code in [0, 1638, 0x80070666] {
            for actual in [None, Some(version("14.43"))] {
                let result =
                    ensure_with(Some(version("14.44")), || actual, || async { Ok(code) }).await;
                assert!(result.is_err(), "code {code}, DLL {actual:?}");
            }
        }
        for code in [1638, 0x80070666] {
            let current = Cell::new(Some(version("14.43")));
            let result = ensure_with(
                Some(version("14.44")),
                || current.get(),
                || async {
                    current.set(Some(version("14.50")));
                    Ok(code)
                },
            )
            .await
            .unwrap();
            assert_eq!(result, Outcome::UpToDate);
        }
    }

    #[tokio::test]
    async fn installer_failures_and_reboots_are_not_ready_to_play() {
        for code in [3010, 1641] {
            let result = ensure_with(Some(version("14.44")), || None, || async { Ok(code) })
                .await
                .unwrap();
            assert_eq!(result, Outcome::RestartRequired);
            assert!(!result.ready_to_play());
            assert!(result.note().unwrap().contains("Restart"));
        }
        assert!(
            ensure_with(Some(version("14.44")), || None, || async { Ok(1603) })
                .await
                .is_err()
        );
        let error = ensure_with(
            Some(version("14.44")),
            || None,
            || async { anyhow::bail!("UAC declined") },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("UAC declined"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn non_windows_ignores_windows_prerequisites() {
        assert_eq!(
            ensure_installed(Some(version("99.0")), ProgressState::default())
                .await
                .unwrap(),
            Outcome::UpToDate
        );
    }
}
