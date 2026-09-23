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

    /// Whether the recorded entry points at a location that no longer exists.
    ///
    /// An entry records absolute paths, so moving or renaming the installation
    /// leaves it launching something that is not there — which fails silently at
    /// login. Reporting that here lets the tray repoint the entry at the copy the
    /// user is actually running. The default is "unknown, leave it alone": a
    /// backend that cannot read its entry must never rewrite it.
    fn is_stale(&self) -> bool {
        false
    }
}

// ── Reading back what we wrote ───────────────────────────────────────────────
// The entries are text this crate generates, so the readers below only have to
// undo that generation. Platform-independent, so their tests run everywhere.

/// Split one of our command lines back into its arguments — the inverse of
/// [`win_cmd_quote`] / [`exec_arg`]: double quotes group, `\"` and `\\` escape,
/// bare whitespace separates.
#[allow(dead_code)] // only read by the platform backends
pub(crate) fn split_quoted_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Only `\"` and `\\` occur in what we write; anything else keeps
                // its backslash, which is what a Windows path needs.
                match chars.next() {
                    Some(next @ ('"' | '\\')) => current.push(next),
                    Some(other) => {
                        current.push('\\');
                        current.push(other);
                    }
                    None => current.push('\\'),
                }
                started = true;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// The executable and config file a recorded argument list names.
///
/// `None` when the list does not carry both — a hand-edited or foreign entry is
/// then left alone rather than "repaired" into something the user did not ask
/// for.
#[allow(dead_code)] // only read by the platform backends
pub(crate) fn paths_from_args(args: &[String]) -> Option<(String, String)> {
    let mut exe = None;
    let mut config = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            config = iter.next().cloned();
        } else if exe.is_none() {
            exe = Some(arg.clone());
        }
    }
    Some((exe?, config?))
}

/// Whether a recorded entry has to be repointed at `current_exe`.
///
/// The current config file does not enter into it: a process is only running if
/// its own config exists (a named one that is missing is now refused at start),
/// so repointing at the running pair is always the improvement.
#[allow(dead_code)] // only read by the platform backends
pub(crate) fn stale_between(recorded_exe: &str, recorded_config: &str, current_exe: &Path) -> bool {
    // The copy the entry launches is gone: the folder was moved, renamed or
    // deleted. The entry would fail invisibly at login.
    if !Path::new(recorded_exe).exists() {
        return true;
    }
    // The same copy, but the config file it was told to use is gone: that start
    // would come up against a different setup rather than this one.
    same_path(recorded_exe, current_exe) && !Path::new(recorded_config).exists()
}

/// Path identity the way the platform sees it: Windows ignores case and treats
/// both separators alike.
fn same_path(a: &str, b: &Path) -> bool {
    normalize_path(a) == normalize_path(&b.to_string_lossy())
}

#[cfg(windows)]
fn normalize_path(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

#[cfg(not(windows))]
fn normalize_path(path: &str) -> String {
    path.trim().to_string()
}

/// The `Exec=` value of an XDG desktop entry.
#[allow(dead_code)] // only read by the Linux backend
pub(crate) fn desktop_exec(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("Exec=").map(str::to_string))
}

/// Every `<string>` value of a LaunchAgent property list, in document order.
#[allow(dead_code)] // only read by the macOS backend
pub(crate) fn plist_strings(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<string>") {
        let after = &rest[start + "<string>".len()..];
        let Some(end) = after.find("</string>") else {
            break;
        };
        values.push(xml_unescape(&after[..end]));
        rest = &after[end + "</string>".len()..];
    }
    values
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
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
         Comment[zh_CN]=Nanofile 同步服务器\n\
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

    // ── Reading back what we wrote ───────────────────────────────────────

    #[test]
    fn a_quoted_command_line_round_trips() {
        let exe = PathBuf::from(r"C:\Apps\Na nofile\nanofile.exe");
        let config = PathBuf::from(r"C:\Apps\Na nofile\config.toml");

        // The Run-key form.
        let line = run_command_line(&exe, &config);
        let args = split_quoted_args(&line);
        assert_eq!(
            args,
            vec![
                r"C:\Apps\Na nofile\nanofile.exe".to_string(),
                "--config".to_string(),
                r"C:\Apps\Na nofile\config.toml".to_string(),
            ]
        );
        let expected = paths_from_args(&args).expect("both paths are present");

        // The XDG form differs only by its `Exec=` prefix.
        let entry = desktop_entry(&exe, &config);
        let exec = desktop_exec(&entry).expect("the entry has an Exec line");
        assert_eq!(
            paths_from_args(&split_quoted_args(&exec)),
            Some(expected.clone())
        );

        // The plist spreads the same arguments over separate <string> elements,
        // with the label first.
        let plist = launch_agent_plist("com.nanofile.nanofile", &exe, &config);
        let values = plist_strings(&plist);
        assert_eq!(
            values.first().map(String::as_str),
            Some("com.nanofile.nanofile")
        );
        let (_, args) = values.split_first().unwrap();
        assert_eq!(paths_from_args(args), Some(expected));
    }

    #[test]
    fn an_argument_may_contain_quotes_and_spaces() {
        let exe = PathBuf::from(r#"C:\we "ird\na nofile.exe"#);
        let config = PathBuf::from(r"C:\cfg files\config.toml");
        let args = split_quoted_args(&run_command_line(&exe, &config));
        assert_eq!(args[0], r#"C:\we "ird\na nofile.exe"#);
        assert_eq!(args[2], r"C:\cfg files\config.toml");
    }

    #[test]
    fn an_entry_without_a_config_is_left_alone() {
        // Something a person wrote by hand, or an older format: it names no
        // config file, so "repairing" it could only guess.
        assert_eq!(
            paths_from_args(&split_quoted_args(r#""C:\nanofile.exe""#)),
            None
        );
        assert_eq!(
            paths_from_args(&split_quoted_args(r#""C:\nanofile.exe" --config"#)),
            None
        );
        assert_eq!(paths_from_args(&[]), None);
    }

    #[test]
    fn staleness_is_about_a_target_that_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/nanofile");
        let config = dir.path().join("config.toml");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"binary").unwrap();
        std::fs::write(&config, b"").unwrap();
        let recorded = exe.to_string_lossy().to_string();
        let recorded_config = config.to_string_lossy().to_string();

        // Healthy: both sides exist and agree.
        assert!(!stale_between(&recorded, &recorded_config, &exe));

        // Moved folder: the recorded executable is gone.
        let gone = dir
            .path()
            .join("elsewhere/nanofile")
            .to_string_lossy()
            .to_string();
        assert!(stale_between(&gone, &recorded_config, &exe));

        // Another copy that still exists: not ours to repoint.
        let other = dir.path().join("other/nanofile");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"binary").unwrap();
        assert!(!stale_between(
            &other.to_string_lossy(),
            &recorded_config,
            &exe
        ));

        // The same copy, but the config it was told to use is gone.
        let moved_config = dir
            .path()
            .join("moved-away.toml")
            .to_string_lossy()
            .to_string();
        assert!(stale_between(&recorded, &moved_config, &exe));
    }

    #[test]
    fn a_foreign_entry_is_not_parsed_into_paths() {
        assert_eq!(paths_from_args(&split_quoted_args("just some words")), None);
        // `Exec=` is only read at the start of a line.
        assert_eq!(
            desktop_exec("Name=X\nExec=/bin/x --config /c\nPath=/x"),
            Some("/bin/x --config /c".to_string())
        );
        assert_eq!(desktop_exec("Name=Exec=/bin/x"), None);
        assert!(plist_strings("<plist><dict></dict></plist>").is_empty());
        assert_eq!(
            plist_strings("<string>a&amp;b</string><string>c</string>"),
            vec!["a&b".to_string(), "c".to_string()]
        );
    }
}
