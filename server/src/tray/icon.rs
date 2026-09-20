//! Runtime tray icon, rasterized from `static/img/favicon.svg` at compile time
//! (see `build.rs`) — the repository ships no binary icon assets.
//!
//! One raster per desktop theme: the mark's tile is near-black in the light
//! theme and near-white in the dark one, so a single image cannot stay visible
//! on both panels. macOS is the exception — its menu bar inverts a template
//! image itself, so it gets the bare glyph.

use tray_icon::Icon;

use super::icon_gen::TRAY_ICON_SIZE;

/// The icon for the desktop this process is running on. Re-reads the OS theme
/// on every call, so callers can also use it to refresh after a theme change.
pub(super) fn tray_icon() -> Icon {
    #[cfg(target_os = "macos")]
    let rgba: &[u8] = TEMPLATE;
    #[cfg(not(target_os = "macos"))]
    let rgba: &[u8] = if super::theme::is_dark() {
        ON_DARK
    } else {
        ON_LIGHT
    };

    Icon::from_rgba(rgba.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .expect("tray icon raster does not match TRAY_ICON_SIZE — rebuild")
}

#[cfg(target_os = "macos")]
const TEMPLATE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tray_icon_template.rgba"));

#[cfg(not(target_os = "macos"))]
const ON_LIGHT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tray_icon_on_light.rgba"));
#[cfg(not(target_os = "macos"))]
const ON_DARK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tray_icon_on_dark.rgba"));
