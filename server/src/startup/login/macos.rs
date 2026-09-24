//! macOS login entry: a per-user LaunchAgent
//! (`~/Library/LaunchAgents/com.nanofile.nanofile.plist`). User-level, no
//! admin rights involved.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::LoginEntry;
use super::entry::{launch_agent_plist, paths_from_args, plist_strings};

const LABEL: &str = "com.nanofile.nanofile";

#[derive(Clone)]
pub(crate) struct LoginManager {
    exe: PathBuf,
    config: PathBuf,
}

impl LoginManager {
    pub(crate) fn new(exe: PathBuf, config: PathBuf) -> Self {
        Self { exe, config }
    }
}

impl LoginEntry for LoginManager {
    fn exe(&self) -> &Path {
        &self.exe
    }

    fn recorded(&self) -> Option<(String, String)> {
        let text = std::fs::read_to_string(plist_path()).ok()?;
        let values = plist_strings(&text);
        // [Label, exe, "--config", config]
        let (_, args) = values.split_first()?;
        paths_from_args(args)
    }

    fn is_enabled(&self) -> bool {
        plist_path().is_file()
    }

    fn enable(&self) -> anyhow::Result<()> {
        let path = plist_path();
        std::fs::create_dir_all(path.parent().expect("plist path has a parent"))?;
        std::fs::write(&path, launch_agent_plist(LABEL, &self.exe, &self.config))?;

        // Load immediately so it also takes effect without re-login. The
        // modern `bootstrap` API is preferred; `load -w` covers older macOS.
        let uid = current_uid();
        let loaded = Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !loaded {
            Command::new("launchctl")
                .args(["load", "-w", &path.to_string_lossy()])
                .status()?;
        }
        Ok(())
    }

    fn disable(&self) -> anyhow::Result<()> {
        let path = plist_path();
        let uid = current_uid();
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{LABEL}")])
            .status();
        let _ = Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()])
            .status();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn plist_path() -> PathBuf {
    let home = std::env::home_dir().expect("cannot determine home directory");
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn current_uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".to_string())
}
