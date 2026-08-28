//! Runtime tray icon, rasterized from `static/img/favicon.svg` at compile
//! time (see `build.rs`) — the repository ships no binary icon assets.

use tray_icon::Icon;

use super::icon_gen::TRAY_ICON_SIZE;

pub(super) fn tray_icon() -> Icon {
    const RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tray_icon.rgba"));
    Icon::from_rgba(RGBA.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .expect("tray_icon.rgba does not match TRAY_ICON_SIZE — rebuild")
}
