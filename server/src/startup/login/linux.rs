//! Linux login entry: an XDG autostart entry
//! (`~/.config/autostart/nanofile.desktop`), honored by GNOME and KDE.
//! User-level, no admin rights involved.

use std::path::{Path, PathBuf};

use super::LoginEntry;
use super::entry::{desktop_entry, desktop_exec, paths_from_args, split_quoted_args};

const FILE_NAME: &str = "nanofile.desktop";

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

    /// `Exec=` holds absolute paths, so a moved installation is visible here.
    fn recorded(&self) -> Option<(String, String)> {
        let text = std::fs::read_to_string(desktop_path()).ok()?;
        let exec = desktop_exec(&text)?;
        paths_from_args(&split_quoted_args(&exec))
    }

    fn is_enabled(&self) -> bool {
        desktop_path().is_file()
    }

    fn enable(&self) -> anyhow::Result<()> {
        let path = desktop_path();
        std::fs::create_dir_all(path.parent().expect("desktop path has a parent"))?;
        std::fs::write(&path, desktop_entry(&self.exe, &self.config))?;
        Ok(())
    }

    fn disable(&self) -> anyhow::Result<()> {
        match std::fs::remove_file(desktop_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn desktop_path() -> PathBuf {
    let home = std::env::home_dir().expect("cannot determine home directory");
    home.join(".config/autostart").join(FILE_NAME)
}
