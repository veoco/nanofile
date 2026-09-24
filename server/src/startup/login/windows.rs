//! Windows login entry: the per-user `Run` registry key
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`). Per-user, so no
//! administrator rights are ever required for the registration itself.

use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};

use super::LoginEntry;
use super::entry::{paths_from_args, run_command_line, split_quoted_args};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Nanofile";

#[derive(Clone)]
pub(crate) struct LoginManager {
    exe: PathBuf,
    config: PathBuf,
}

impl LoginManager {
    pub(crate) fn new(exe: PathBuf, config: PathBuf) -> Self {
        Self { exe, config }
    }

    /// The command line currently registered under our value name, if any.
    fn recorded_value(&self) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .ok()?
            .get_value::<String, _>(VALUE_NAME)
            .ok()
    }
}

impl LoginEntry for LoginManager {
    fn exe(&self) -> &Path {
        &self.exe
    }

    fn recorded(&self) -> Option<(String, String)> {
        paths_from_args(&split_quoted_args(&self.recorded_value()?))
    }

    fn is_enabled(&self) -> bool {
        self.recorded_value().is_some()
    }

    fn enable(&self) -> anyhow::Result<()> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
            .map_err(|e| anyhow::anyhow!("opening HKCU Run key failed: {e}"))?;
        key.set_value(VALUE_NAME, &run_command_line(&self.exe, &self.config))
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
