//! Windows login entry: the per-user `Run` registry key; no admin rights are needed.
//!
//! Two keys are involved. `…\CurrentVersion\Run` holds the registration, and
//! `…\Explorer\StartupApproved\Run` holds the startup switch Task Manager and
//! Settings show, which turns the entry off without deleting it. Both are read, so
//! the tray can report what a login would really do; both are written, so turning
//! the item on here is not silently overruled by a switch left off elsewhere.

use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};

use super::LoginEntry;
use super::entry::{paths_from_args, run_value, split_win_args};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where Task Manager and Settings record that a startup entry is switched off.
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

const VALUE_NAME: &str = "Nanofile";

/// Whether a `StartupApproved` value leaves the entry enabled.
///
/// Task Manager and Settings write `02`/`06` for on and `03`/`07` for off; the rest
/// of the twelve-byte value is the time it changed. A missing value means nothing
/// has switched the entry off, and an unfamiliar byte is not evidence that it was.
fn approved(bytes: &[u8]) -> bool {
    !matches!(bytes.first(), Some(0x03 | 0x07))
}

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

    /// The raw `StartupApproved` value recorded for us, if the key and value exist.
    fn switch(&self) -> Option<Vec<u8>> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(APPROVED_KEY, KEY_QUERY_VALUE)
            .ok()?
            .get_raw_value(VALUE_NAME)
            .ok()
            .map(|value| value.bytes.into_owned())
    }

    /// Write the registration, leaving the launcher's switch alone: repointing a
    /// broken entry must not turn back on what a user switched off in Task Manager.
    fn write_value(&self) -> anyhow::Result<()> {
        // The entry records `--config`, and a start that names a missing file is
        // refused: registering one would make every login report a broken
        // configuration instead of starting.
        if !self.config.is_file() {
            anyhow::bail!(
                "the configuration file {} does not exist; a login start would refuse it, \
                 so start-at-login was not registered",
                self.config.display()
            );
        }
        let line = run_value(&self.exe, &self.config).map_err(anyhow::Error::msg)?;
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
            .map_err(|e| anyhow::anyhow!("opening HKCU Run key failed: {e}"))?;
        key.set_value(VALUE_NAME, &line)
            .map_err(|e| anyhow::anyhow!("writing Run value failed: {e}"))?;
        Ok(())
    }

    /// Drop the startup switch so the registration is not skipped: enabling from
    /// the menu is the user asking for it, whatever Task Manager was set to.
    fn clear_switch(&self) -> anyhow::Result<()> {
        let key = match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(APPROVED_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
        {
            Ok(key) => key,
            // No switch key at all: nothing has ever switched an entry off here.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "opening the StartupApproved key failed: {e}"
                ));
            }
        };
        match key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            // Already on (or never switched): nothing to clear.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(anyhow::anyhow!(
                "clearing the StartupApproved value failed: {e}"
            )),
        }
    }
}

impl LoginEntry for LoginManager {
    fn exe(&self) -> &Path {
        &self.exe
    }

    fn recorded(&self) -> Option<(String, String)> {
        paths_from_args(&split_win_args(&self.recorded_value()?))
    }

    fn is_enabled(&self) -> bool {
        self.recorded_value().is_some()
    }

    fn is_approved(&self) -> bool {
        self.switch().as_deref().is_none_or(approved)
    }

    fn enable(&self) -> anyhow::Result<()> {
        self.write_value()?;
        self.clear_switch()
    }

    fn repoint(&self) -> anyhow::Result<()> {
        self.write_value()
    }

    fn disable(&self) -> anyhow::Result<()> {
        match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE | KEY_SET_VALUE)
        {
            Ok(key) => match key.delete_value(VALUE_NAME) {
                Ok(()) => {}
                // Already disabled — treat as success.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(anyhow::anyhow!("deleting Run value failed: {e}")),
            },
            // No Run key means no registration, which is what "disabled" says.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("opening HKCU Run key failed: {e}")),
        }
        // The switch is keyed by the value name, so a leftover would silently
        // suppress a later registration. Best effort: the entry is already gone.
        let _ = self.clear_switch();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::approved;

    #[test]
    fn the_startup_switch_reads_the_state_byte() {
        // Task Manager's on/off bytes; the remaining bytes are a timestamp.
        assert!(approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(approved(&[0x06, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]));
        assert!(!approved(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!approved(&[0x07, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]));
        // No value, or a byte we do not know: the launcher is not expected to skip it.
        assert!(approved(&[]));
        assert!(approved(&[0x00]));
    }
}
