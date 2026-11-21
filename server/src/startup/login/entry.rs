//! The text of a login entry: how it is written, and how it is read back.
//!
//! Platform-independent so the tests for all three formats run on every platform,
//! including Linux CI.

use std::path::Path;

/// Split a Windows command line back into its arguments — the inverse of the
/// quoting in `crate::startup::cmdline`. A backslash is literal except immediately
/// before a quote, so a UNC path (`\\server\share`) keeps both leading backslashes.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn split_win_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.as_str().starts_with('"') => {
                current.push('"');
                chars.next();
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

/// Split an XDG `Exec=` value back into its arguments — the inverse of `exec_arg`,
/// where a backslash escapes both a quote and another backslash.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn split_desktop_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Only `\"` and `\\` occur in what we write; anything else keeps its backslash.
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

/// The executable and config file a recorded argument list names, or `None` when
/// it does not carry both: a hand-edited or foreign entry is then left alone
/// rather than "repaired" into something the user did not ask for.
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

/// Whether a recorded entry has to be repointed at `current_exe`. The current
/// config file does not enter into it: a process is only running if its own
/// config exists, so repointing at the running pair is always the improvement.
pub(crate) fn stale_between(recorded_exe: &str, recorded_config: &str, current_exe: &Path) -> bool {
    // The copy the entry launches is gone: the folder was moved or deleted, and
    // the entry would fail invisibly at login.
    if !Path::new(recorded_exe).exists() {
        return true;
    }
    // The same copy, but its config file is gone: that start would use a different setup.
    same_path(recorded_exe, current_exe) && !Path::new(recorded_config).exists()
}

/// Path identity the way the platform sees it: Windows ignores case and treats
/// both separators alike.
pub(crate) fn same_path(a: &str, b: &Path) -> bool {
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
#[cfg(any(target_os = "linux", test))]
pub(crate) fn desktop_exec(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("Exec=").map(str::to_string))
}

/// Every `<string>` value of a LaunchAgent property list, in document order.
#[cfg(any(target_os = "macos", test))]
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

#[cfg(any(target_os = "macos", test))]
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

// Each builder is consumed by exactly one platform module; the tests cover all of them.

/// Windows: value data under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`,
/// the full command line, so login startup needs no working directory.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn run_command_line(exe: &Path, config: &Path) -> String {
    use crate::startup::cmdline::win_cmd_quote;

    format!(
        "{} --config {}",
        win_cmd_quote(&exe.to_string_lossy()),
        win_cmd_quote(&config.to_string_lossy())
    )
}

/// Windows documents that a `Run` value's data — the command line — is no longer
/// than 260 characters, and a longer value is not guaranteed to start.
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN_RUN_VALUE_LIMIT: usize = 260;

/// [`run_command_line`], or the reason it cannot be registered: an entry Windows
/// would truncate or skip is refused here, where the menu can report it.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn run_value(exe: &Path, config: &Path) -> Result<String, String> {
    let line = run_command_line(exe, config);
    let length = line.encode_utf16().count();
    if length > WIN_RUN_VALUE_LIMIT {
        return Err(format!(
            "the start-at-login command line is {length} characters long, over the \
             {WIN_RUN_VALUE_LIMIT}-character limit Windows documents for a Run value; \
             install the server in a shorter path"
        ));
    }
    Ok(line)
}

/// macOS: LaunchAgent property list registered under `~/Library/LaunchAgents`.
#[cfg(any(target_os = "macos", test))]
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

/// Linux: XDG autostart entry under `~/.config/autostart`, honored by GNOME and KDE.
#[cfg(any(target_os = "linux", test))]
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

/// Quotes one argument for a Desktop Entry `Exec` value, per the Desktop Entry Specification.
#[cfg(any(target_os = "linux", test))]
fn exec_arg(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(any(target_os = "macos", test))]
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

    #[test]
    fn a_quoted_command_line_round_trips() {
        let exe = PathBuf::from(r"C:\Apps\Na nofile\nanofile.exe");
        let config = PathBuf::from(r"C:\Apps\Na nofile\config.toml");

        let line = run_command_line(&exe, &config);
        let args = split_win_args(&line);
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
            paths_from_args(&split_desktop_args(&exec)),
            Some(expected.clone())
        );

        // The plist spreads the same arguments over separate <string> elements, label first.
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
        let args = split_win_args(&run_command_line(&exe, &config));
        assert_eq!(args[0], r#"C:\we "ird\na nofile.exe"#);
        assert_eq!(args[2], r"C:\cfg files\config.toml");
    }

    #[test]
    fn a_unc_command_line_keeps_its_leading_backslashes() {
        let exe = PathBuf::from(r"\\server\share\nanofile.exe");
        let config = PathBuf::from(r"\\server\share\config.toml");
        let args = split_win_args(&run_command_line(&exe, &config));
        assert_eq!(args[0], r"\\server\share\nanofile.exe", "{args:?}");
        assert_eq!(
            paths_from_args(&args),
            Some((
                exe.to_string_lossy().to_string(),
                config.to_string_lossy().to_string()
            ))
        );
    }

    #[test]
    fn a_desktop_exec_unescapes_only_its_own_quoting() {
        // `exec_arg` doubles a backslash and escapes a quote; the reader is its inverse.
        let exe = PathBuf::from(r#"/opt/na "file/nanofile"#);
        let config = PathBuf::from(r"/opt/back\slash/config.toml");
        let entry = desktop_entry(&exe, &config);
        let exec = desktop_exec(&entry).expect("the entry has an Exec line");
        assert_eq!(
            paths_from_args(&split_desktop_args(&exec)),
            Some((
                exe.to_string_lossy().to_string(),
                config.to_string_lossy().to_string()
            ))
        );
    }

    #[test]
    fn the_run_value_stops_at_the_documented_limit() {
        let exe = PathBuf::from(r"C:\apps\nanofile.exe");
        let short = run_value(&exe, &PathBuf::from(r"C:\apps\config.toml"))
            .expect("a normal installation fits");
        let padding = WIN_RUN_VALUE_LIMIT - short.encode_utf16().count();
        let file = "config.toml".len();

        let at_limit = PathBuf::from(format!(r"C:\apps\{}", "c".repeat(padding + file)));
        let line = run_value(&exe, &at_limit).expect("exactly at the limit is allowed");
        assert_eq!(line.encode_utf16().count(), WIN_RUN_VALUE_LIMIT);

        let over = PathBuf::from(format!(r"C:\apps\{}", "c".repeat(padding + file + 1)));
        let reason = run_value(&exe, &over).expect_err("one character over is refused");
        assert!(reason.contains("260"), "{reason}");
    }

    #[test]
    fn an_entry_without_a_config_is_left_alone() {
        // A hand-written or older entry that names no config file: repairing it could only guess.
        assert_eq!(
            paths_from_args(&split_win_args(r#""C:\nanofile.exe""#)),
            None
        );
        assert_eq!(
            paths_from_args(&split_win_args(r#""C:\nanofile.exe" --config"#)),
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
        assert_eq!(paths_from_args(&split_win_args("just some words")), None);
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
