//! Desktop theme detection, for choosing between the two tray rasters.
//!
//! The brand mark is achromatic: its tile is near-black in the light theme and
//! near-white in the dark one (the same accent pair the web UI uses), so a
//! single image cannot stay visible on both a light and a dark panel. macOS is
//! not compiled here — it inverts a template image itself.
//!
//! This module is only reached from the tray, which `run_mode` never enables on
//! a platform without a desktop session.

/// Whether the desktop the tray icon is drawn on is dark, i.e. whether the
/// light-on-dark raster is the visible one. A wrong guess costs contrast, not
/// correctness, so every probe falls back to the plain brand mark (light).
#[cfg(target_os = "windows")]
pub(super) fn is_dark() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    // `SystemUsesLightTheme` is the taskbar / notification-area setting — the
    // surface the tray icon actually sits on (`AppsUseLightTheme` is for app
    // windows). A missing value means an older or stripped-down system, where
    // light is the classic default.
    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(PERSONALIZE)
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

/// Linux: the panel an appindicator is drawn on follows the GTK theme, and GTK
/// is already initialized when the tray is created (see `backend_linux`). The
/// prefer-dark flag is the direct signal; some themes only say "dark" in the
/// theme name, so that is the fallback.
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
/// mode does not leave the wrong raster on the panel until a restart. The
/// notify handlers live for the process lifetime, which is as long as the tray
/// does; `TrayIcon` is reference-counted, so the clone keeps it alive.
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
