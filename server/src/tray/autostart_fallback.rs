//! Stub auto-start backend for platforms without a supported desktop
//! integration. Unreachable in practice: `run_mode` already returns `Headless`
//! there; it exists only to keep the crate compiling on unusual targets.

use std::path::PathBuf;

use super::Autostart;

pub(super) struct AutostartManager;

impl AutostartManager {
    pub(crate) fn new(_exe: PathBuf, _config: PathBuf) -> Self {
        Self
    }
}

impl Autostart for AutostartManager {
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
