//! Whether this process has a desktop session to put something in front of.
//!
//! One question, asked from two places that must not answer it differently: the
//! tray (`tray::run_mode`, which decides between an icon and a headless server)
//! and the first-run setup (`setup::run`, which decides whether opening a browser
//! is worth attempting). A build that put an icon in no tray but opened a browser
//! the same way, or the reverse, would be two bugs with one cause.

/// Whether a desktop session is present.
///
/// Linux is the only platform where the answer is not implied by the target: a
/// server reached over SSH has no `DISPLAY` and no `WAYLAND_DISPLAY`, so there is
/// no window to open and no tray to show. Windows and macOS processes reach the
/// session they were started from; on macOS over SSH, `open` lands on the console
/// session, which is where the person is looking.
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
        // Nothing to assert about the environment here: the point is that the
        // non-Linux answer does not depend on one, and that the function is
        // callable everywhere.
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
