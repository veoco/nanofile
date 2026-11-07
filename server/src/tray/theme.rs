//! Desktop theme detection, for choosing between the two tray rasters.
//!
//! The brand mark is achromatic — near-black tile on light, near-white on dark,
//! the web UI's accent pair — so one image cannot stay visible on both panels;
//! macOS is not compiled here, since it inverts a template image itself.

/// Whether the desktop behind the tray is dark. A wrong guess costs contrast,
/// not correctness, so every probe falls back to the plain (light) mark.
#[cfg(target_os = "windows")]
pub(super) fn is_dark() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    // `SystemUsesLightTheme` is the taskbar setting the tray icon sits on
    // (`AppsUseLightTheme` is for app windows); missing means light is the default.
    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(PERSONALIZE)
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

/// Linux: the desktop's colour preference, asked of the freedesktop portal's
/// Settings interface. `color-scheme` is the direct signal (0 = no preference,
/// 1 = prefer dark, 2 = prefer light); a GTK theme name saying "dark" is the
/// fallback for desktops whose portal reports no preference, which is the pair of
/// signals GtkSettings used to provide. No GTK linkage: this is one D-Bus round
/// trip, and a missing portal costs the dark raster, not the tray.
#[cfg(target_os = "linux")]
pub(super) fn is_dark() -> bool {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let scheme = read_u32(&connection, "org.freedesktop.appearance", "color-scheme");
    let theme = read_string(&connection, "org.gnome.desktop.interface", "gtk-theme");
    prefers_dark(scheme, theme.as_deref())
}

/// The decision, without a bus: the portal's answer wins, and only "no
/// preference" (or no answer at all) falls back to the theme name.
#[cfg(target_os = "linux")]
fn prefers_dark(scheme: Option<u32>, gtk_theme: Option<&str>) -> bool {
    match scheme {
        Some(1) => true,
        Some(2) => false,
        _ => gtk_theme.is_some_and(|name| name.to_ascii_lowercase().contains("dark")),
    }
}

/// Reads one portal setting. The answer is a variant, and some portal
/// implementations wrap it one variant deep; unwrapping that level is what
/// `ashpd` does for the same call.
#[cfg(target_os = "linux")]
fn read_value(
    connection: &zbus::blocking::Connection,
    namespace: &str,
    key: &str,
) -> Option<zbus::zvariant::OwnedValue> {
    use zbus::zvariant::OwnedValue;

    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &(namespace, key),
        )
        .ok()?;
    let value: OwnedValue = reply.body().deserialize().ok()?;
    unwrap_variant(value)
}

/// Strips the variant wrapper the portal's `ReadOne` answers in, including the
/// extra level some implementations add.
#[cfg(target_os = "linux")]
fn unwrap_variant(value: zbus::zvariant::OwnedValue) -> Option<zbus::zvariant::OwnedValue> {
    use zbus::zvariant::Value;

    value.downcast_ref::<Value>().ok()?.try_to_owned().ok()
}

#[cfg(target_os = "linux")]
fn read_u32(connection: &zbus::blocking::Connection, namespace: &str, key: &str) -> Option<u32> {
    u32::try_from(read_value(connection, namespace, key)?).ok()
}

#[cfg(target_os = "linux")]
fn read_string(
    connection: &zbus::blocking::Connection,
    namespace: &str,
    key: &str,
) -> Option<String> {
    String::try_from(read_value(connection, namespace, key)?).ok()
}

/// Re-applies the tray icon when the desktop switches between light and dark.
/// The portal's `SettingChanged` wakes the tray thread, which re-reads the
/// preference and re-renders (see `backend_linux`), so there is no GTK signal to
/// connect to any more. The portal is reconnected if it goes away: an upgrade
/// restarting it must not leave the wrong raster on the panel until the next
/// launch.
#[cfg(target_os = "linux")]
pub(super) fn watch() {
    let started = std::thread::Builder::new()
        .name("tray-theme".into())
        .spawn(|| {
            loop {
                if let Err(e) = watch_portal() {
                    tracing::debug!("theme watcher stopped ({e:#}); retrying in 30 s");
                }
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        });
    if let Err(e) = started {
        tracing::warn!("Failed to start the theme watcher: {e}");
    }
}

#[cfg(target_os = "linux")]
fn watch_portal() -> anyhow::Result<()> {
    use zbus::MatchRule;
    use zbus::blocking::{Connection, MessageIterator};

    let connection = Connection::session()?;
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Settings")?
        .member("SettingChanged")?
        .build();
    // Every namespace the portal knows arrives here; the tray re-reads the two
    // keys it cares about and skips the rewrite when the answer did not change, so
    // filtering the match rule by namespace would only add a second rule.
    let messages = MessageIterator::for_match_rule(rule, &connection, Some(8))?;
    for message in messages {
        message?;
        super::backend::theme_changed();
    }
    anyhow::bail!("the portal's setting stream ended")
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(super) fn is_dark() -> bool {
    false
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use zbus::zvariant::{OwnedValue, Value};

    use super::{prefers_dark, unwrap_variant};

    #[test]
    fn the_portal_answer_wins_over_the_theme_name() {
        assert!(prefers_dark(Some(1), None));
        assert!(!prefers_dark(Some(2), Some("Adwaita-dark")));
    }

    #[test]
    fn no_preference_falls_back_to_the_theme_name() {
        assert!(prefers_dark(Some(0), Some("Adwaita-dark")));
        assert!(prefers_dark(None, Some("Breeze-Dark")));
        assert!(!prefers_dark(None, Some("Breeze")));
        assert!(!prefers_dark(None, None));
    }

    #[test]
    fn a_nested_variant_is_unwrapped_once() {
        let outer = Value::Value(Box::new(Value::U32(1)));
        let value: OwnedValue = outer.try_to_owned().unwrap();

        assert_eq!(u32::try_from(unwrap_variant(value).unwrap()).unwrap(), 1);
    }

    #[test]
    fn a_plain_value_survives_the_unwrap() {
        let value: OwnedValue = Value::from("Breeze-Dark").try_to_owned().unwrap();
        let value = unwrap_variant(value).unwrap();

        assert_eq!(String::try_from(value).unwrap(), "Breeze-Dark");
    }
}
