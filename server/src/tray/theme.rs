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

/// Linux: the panel an appindicator is drawn on follows the GTK theme, already
/// initialized when the tray is created (see `backend_linux`). The prefer-dark
/// flag is the direct signal; a theme name saying "dark" is the fallback.
#[cfg(target_os = "linux")]
pub(super) fn is_dark() -> bool {
    use gtk::prelude::GtkSettingsExt;

    gtk::Settings::default()
        .map(|settings| {
            settings.is_gtk_application_prefer_dark_theme()
                || settings
                    .gtk_theme_name()
                    .is_some_and(|name| name.to_lowercase().contains("dark"))
        })
        .unwrap_or(false)
}

/// Re-applies the tray icon when the GTK theme changes, so switching to dark
/// mode does not leave the wrong raster on the panel until a restart. The notify
/// handlers live as long as the tray, and the clone keeps the refcounted icon alive.
#[cfg(target_os = "linux")]
pub(super) fn watch(tray: &tray_icon::TrayIcon) {
    use gtk::prelude::ObjectExt;

    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    for property in ["gtk-theme-name", "gtk-application-prefer-dark-theme"] {
        let tray = tray.clone();
        settings.connect_notify_local(Some(property), move |_, _| {
            if let Err(e) = tray.set_icon(Some(super::icon::tray_icon())) {
                tracing::warn!("Failed to refresh the tray icon: {e}");
            }
        });
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(super) fn is_dark() -> bool {
    false
}
