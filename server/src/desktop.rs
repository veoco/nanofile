//! Whether this process has a desktop session to put something in front of.
//!
//! One question, asked from two places that must not answer it differently:
//! the tray (`tray::run_mode`, icon or headless server) and first-run setup
//! (`setup::run`, whether opening a browser is worth attempting).

/// Whether a desktop session is present.
///
/// Linux is the only platform where the target does not imply the answer: an
/// SSH session has neither `DISPLAY` nor `WAYLAND_DISPLAY`. macOS `open` over
/// SSH lands on the console session, which is where the person is looking.
pub fn has_desktop_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(any(target_os = "windows", target_os = "macos"))
    }
}

#[cfg(test)]
mod tests {
    use super::has_desktop_session;

    #[test]
    fn the_answer_is_a_platform_property_off_linux() {
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(
                has_desktop_session(),
                cfg!(any(target_os = "windows", target_os = "macos"))
            );
        }
        #[cfg(target_os = "linux")]
        {
            let expected = std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some();
            assert_eq!(has_desktop_session(), expected);
        }
    }
}
