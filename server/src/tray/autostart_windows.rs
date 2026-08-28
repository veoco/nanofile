//! Windows auto-start via the per-user `Run` registry key
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`). Per-user, so no
//! administrator rights are ever required for the registration itself.

use std::path::PathBuf;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};

use super::Autostart;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Nanofile";

pub(crate) struct AutostartManager {
    exe: PathBuf,
    config: PathBuf,
}

impl AutostartManager {
    pub(crate) fn new(exe: PathBuf, config: PathBuf) -> Self {
        Self { exe, config }
    }
}

impl Autostart for AutostartManager {
    fn is_enabled(&self) -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
            .is_ok()
    }

    fn enable(&self) -> anyhow::Result<()> {
        // An elevated process writes to the elevated account's HKCU hive; make
        // that consequence explicit instead of silently registering auto-start
        // for (potentially) another account. Login startup itself always runs
        // unelevated (the exe manifest is asInvoker), so no UAC prompt.
        if is_elevated() && !confirm_elevated_registration() {
            anyhow::bail!("launch-at-login registration cancelled");
        }
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
            .map_err(|e| anyhow::anyhow!("opening HKCU Run key failed: {e}"))?;
        key.set_value(
            VALUE_NAME,
            &super::run_command_line(&self.exe, &self.config),
        )
        .map_err(|e| anyhow::anyhow!("writing Run value failed: {e}"))?;
        Ok(())
    }

    fn disable(&self) -> anyhow::Result<()> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
            .map_err(|e| anyhow::anyhow!("opening HKCU Run key failed: {e}"))?;
        match key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            // Already disabled — treat as success.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::anyhow!("deleting Run value failed: {e}")),
        }
    }
}

/// True when the current process token is elevated ("Run as administrator").
fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

fn confirm_elevated_registration() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};

    let user = std::env::var("USERNAME").unwrap_or_else(|_| "the current user".into());
    let text = format!(
        "nanofile is running with administrator privileges.\n\n\
         Launch at login will be registered for account '{user}' and will\n\
         start at login without administrator rights.\n\nContinue?"
    );
    let text = wide(&text);
    let caption = wide("nanofile");
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONWARNING | MB_OKCANCEL,
        )
    };
    result == 1 // IDOK
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
