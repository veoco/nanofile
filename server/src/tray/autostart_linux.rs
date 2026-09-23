//! Linux auto-start via an XDG autostart entry
//! (`~/.config/autostart/nanofile.desktop`), honored by GNOME and KDE.
//! User-level, no admin rights involved.

use std::path::PathBuf;

use super::Autostart;

const FILE_NAME: &str = "nanofile.desktop";

#[derive(Clone)]
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
        desktop_path().is_file()
    }

    /// `Exec=` holds absolute paths, so a moved installation leaves the entry
    /// starting something that is not there.
    fn is_stale(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(desktop_path()) else {
            return false;
        };
        let Some(exec) = super::desktop_exec(&text) else {
            return false;
        };
        let args = super::split_quoted_args(&exec);
        let Some((exe, config)) = super::paths_from_args(&args) else {
            return false;
        };
        super::stale_between(&exe, &config, &self.exe)
    }

    fn enable(&self) -> anyhow::Result<()> {
        let path = desktop_path();
        std::fs::create_dir_all(path.parent().expect("desktop path has a parent"))?;
        std::fs::write(&path, super::desktop_entry(&self.exe, &self.config))?;
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
