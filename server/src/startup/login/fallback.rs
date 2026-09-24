//! Stub login entry for platforms without a supported desktop integration.
//! Unreachable in practice: `run_mode` already returns `Headless` there; it
//! exists only to keep the crate compiling on unusual targets.

use std::path::{Path, PathBuf};

use super::LoginEntry;

#[derive(Clone)]
pub(crate) struct LoginManager {
    exe: PathBuf,
}

impl LoginManager {
    pub(crate) fn new(exe: PathBuf, _config: PathBuf) -> Self {
        Self { exe }
    }
}

impl LoginEntry for LoginManager {
    fn exe(&self) -> &Path {
        &self.exe
    }

    fn recorded(&self) -> Option<(String, String)> {
        None
    }

    fn is_enabled(&self) -> bool {
        false
    }

    fn enable(&self) -> anyhow::Result<()> {
        anyhow::bail!("launch-at-login is not supported on this platform")
    }

    fn disable(&self) -> anyhow::Result<()> {
        anyhow::bail!("launch-at-login is not supported on this platform")
    }
}
