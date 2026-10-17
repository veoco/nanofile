//! Runtime tray icon, rasterized from `static/img/favicon.svg` at compile time
//! (see `build.rs`) — one raster per desktop theme, since the tile is near-black
//! on a light panel and near-white on a dark one. macOS gets the bare glyph,
//! because its menu bar inverts a template image itself.

use tray_icon::Icon;

use super::icon_gen::TRAY_ICON_SIZE;

/// The icon for this desktop; re-reads the OS theme, so callers can refresh with it.
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
