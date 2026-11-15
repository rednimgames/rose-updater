//! Microsoft Visual C++ runtime step of the Repair button.
//!
//! The game refuses to start when msvcp140.dll is older than the toolset it was
//! built with, so Repair installs the current x64 redistributable when the
//! system copy is missing or older than that.

/// Oldest runtime the game accepts: the VS 2022 (v143) 14.44 toolset the client
/// is built with. Keep in step with `vc_runtime_is_current` in the client.
#[cfg_attr(not(windows), allow(dead_code))]
const REQUIRED_VERSION: (u16, u16) = (14, 44);

#[cfg_attr(not(windows), allow(dead_code))]
fn is_current(version: (u16, u16)) -> bool {
    version >= REQUIRED_VERSION
}

/// Installs the runtime if needed and returns a line for the Repair dialog,
/// or `None` when nothing had to change.
#[cfg(windows)]
pub async fn repair() -> Option<String> {
    match windows_impl::ensure_installed().await {
        Ok(windows_impl::Outcome::UpToDate) => None,
        Ok(windows_impl::Outcome::Installed) => {
            Some("Installed the Microsoft Visual C++ Redistributable.".to_string())
        }
        Ok(windows_impl::Outcome::RestartRequired) => Some(
            "Installed the Microsoft Visual C++ Redistributable. Restart your computer before playing."
                .to_string(),
        ),
        Err(error) => {
            tracing::error!("Visual C++ runtime install failed: {:#}", error);
            Some(format!(
                "Couldn't install the Microsoft Visual C++ Redistributable: {}",
                error
            ))
        }
    }
}

#[cfg(not(windows))]
pub async fn repair() -> Option<String> {
    None
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use tracing::{info, warn};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    use super::is_current;

    const REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

    #[derive(Debug)]
    pub enum Outcome {
        UpToDate,
        Installed,
        RestartRequired,
    }

    pub async fn ensure_installed() -> anyhow::Result<Outcome> {
        let version = installed_version();
        if let Some(version) = version.filter(|v| is_current(*v)) {
            info!(?version, "Visual C++ runtime is up to date");
            return Ok(Outcome::UpToDate);
        }
        info!(?version, "Installing the Visual C++ runtime");

        let bytes = reqwest::get(REDIST_URL)
            .await
            .and_then(|response| response.error_for_status())
            .context("Download failed")?
            .bytes()
            .await
            .context("Download failed")?;
        let installer = std::env::temp_dir().join("rose-vc_redist.x64.exe");
        tokio::fs::write(&installer, &bytes).await?;

        let run_path = installer.clone();
        let exit_code = tokio::task::spawn_blocking(move || run_elevated(&run_path)).await??;
        let _ = tokio::fs::remove_file(&installer).await;

        info!(exit_code, "Visual C++ redistributable installer finished");
        match exit_code {
            0 => Ok(Outcome::Installed),
            1641 | 3010 => Ok(Outcome::RestartRequired),
            // A newer runtime is already registered; nothing more the installer can do.
            1638 => {
                warn!("Visual C++ redistributable reports a newer version is already installed");
                Ok(Outcome::UpToDate)
            }
            code => anyhow::bail!("the installer exited with code {}", code),
        }
    }

    /// Version of the 64-bit msvcp140.dll in System32, the copy the game loads.
    fn installed_version() -> Option<(u16, u16)> {
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
            {
                return None;
            }
            let info = &*(info as *const VS_FIXEDFILEINFO);
            Some((
                (info.dwFileVersionMS >> 16) as u16,
                info.dwFileVersionMS as u16,
            ))
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
            WaitForSingleObject(info.hProcess, INFINITE);
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

    #[test]
    fn accepts_the_required_runtime_and_newer() {
        assert!(is_current((14, 44)));
        assert!(is_current((14, 50)));
        assert!(is_current((15, 0)));
    }

    #[test]
    fn rejects_older_runtimes() {
        assert!(!is_current((14, 43)));
        assert!(!is_current((14, 0)));
        assert!(!is_current((12, 99)));
    }
}
