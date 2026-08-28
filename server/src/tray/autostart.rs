//! Launch-at-login registration for the compiled-in desktop platform.
//!
//! The registration entries always pass the config path explicitly
//! (`--config <absolute path>`) because a login-started process runs with a
//! working directory like `C:\Windows\System32` or `/`, where the default
//! relative `config.toml` would not be found.

use std::path::Path;

#[cfg(target_os = "windows")]
#[path = "autostart_windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "autostart_macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "autostart_linux.rs"]
mod platform;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[path = "autostart_fallback.rs"]
mod platform;

pub(crate) use platform::AutostartManager as PlatformAutostart;

/// Launch-at-login management for the current platform.
pub(super) trait Autostart {
    fn is_enabled(&self) -> bool;
    fn enable(&self) -> anyhow::Result<()>;
    fn disable(&self) -> anyhow::Result<()>;
}

// ── Shared entry text builders ───────────────────────────────────────────────
// Platform-independent so their unit tests run everywhere; each is consumed
// by exactly one platform module.

/// Windows: value data stored under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. The value itself is
/// the full command line, so login startup needs no working directory.
#[allow(dead_code)] // only used by the Windows backend
pub(crate) fn run_command_line(exe: &Path, config: &Path) -> String {
    format!(
        "{} --config {}",
        win_cmd_quote(&exe.to_string_lossy()),
        win_cmd_quote(&config.to_string_lossy())
    )
}

/// macOS: LaunchAgent property list registered under `~/Library/LaunchAgents`.
#[allow(dead_code)] // only used by the macOS backend
pub(crate) fn launch_agent_plist(label: &str, exe: &Path, config: &Path) -> String {
    let workdir = exe
        .parent()
        .map(|p| xml_escape(&p.to_string_lossy()))
        .unwrap_or_else(|| "/".to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--config</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>WorkingDirectory</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(label),
        xml_escape(&exe.to_string_lossy()),
        xml_escape(&config.to_string_lossy()),
        workdir
    )
}

/// Linux: XDG autostart entry under `~/.config/autostart`, honored by both
/// GNOME and KDE.
#[allow(dead_code)] // only used by the Linux backend
pub(crate) fn desktop_entry(exe: &Path, config: &Path) -> String {
    let workdir = exe
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Nanofile\n\
         Comment=Nanofile sync server\n\
         Exec={} --config {}\n\
         Path={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_arg(&exe.to_string_lossy()),
        exec_arg(&config.to_string_lossy()),
        workdir
    )
}

/// Quotes one argument for a Desktop Entry `Exec` value: double quotes with
/// backslash escaping, per the Desktop Entry Specification.
fn exec_arg(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Quotes a path for a Windows command line (Run-key value data): only double
/// quotes need escaping — backslashes are literal except before a quote, and
/// doubling them would corrupt plain paths.
fn win_cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn run_command_line_quotes_paths() {
        let line = run_command_line(
            &PathBuf::from(r"C:\Apps\Na nofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Na nofile\config.toml"),
        );
        assert_eq!(
            line,
            r#""C:\Apps\Na nofile\nanofile.exe" --config "C:\Apps\Na nofile\config.toml""#
        );
    }

    #[test]
    fn run_command_line_escapes_inner_quotes() {
        let line = run_command_line(
            &PathBuf::from(r#"C:\we"ird\nanofile.exe"#),
            &PathBuf::from(r"C:\cfg.toml"),
        );
        assert!(line.contains(r#""C:\we\"ird\nanofile.exe""#));
    }

    #[test]
    fn desktop_entry_has_exec_and_path() {
        let entry = desktop_entry(
            &PathBuf::from("/opt/nanofile/nanofile"),
            &PathBuf::from("/srv/nanofile/config.toml"),
        );
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Type=Application\n"));
        assert!(
            entry.contains(
                "Exec=\"/opt/nanofile/nanofile\" --config \"/srv/nanofile/config.toml\"\n"
            )
        );
        assert!(entry.contains("Path=/opt/nanofile\n"));
        assert!(entry.contains("X-GNOME-Autostart-enabled=true\n"));
    }

    #[test]
    fn desktop_entry_escapes_quoted_paths() {
        let entry = desktop_entry(
            &PathBuf::from(r#"/opt/na "file/nanofile"#),
            &PathBuf::from("/srv/cfg.toml"),
        );
        assert!(entry.contains("Exec=\"/opt/na \\\"file/nanofile\""));
    }

    #[test]
    fn plist_escapes_xml_and_sets_run_at_load() {
        let plist = launch_agent_plist(
            "com.nanofile.nanofile",
            &PathBuf::from("/opt/nanofile/nanofile"),
            &PathBuf::from("/opt/nanofile/config&v<1>.toml"),
        );
        assert!(plist.contains("<key>Label</key>\n  <string>com.nanofile.nanofile</string>"));
        assert!(plist.contains("<string>/opt/nanofile/config&amp;v&lt;1&gt;.toml</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>WorkingDirectory</key>\n  <string>/opt/nanofile</string>"));
    }
}
