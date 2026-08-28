//! Stub tray backend for platforms without tray support. Unreachable in
//! practice (`run_mode` returns `Headless` there); exists to keep the crate
//! compiling on unusual targets.

use std::sync::mpsc::Sender;

use super::TrayContext;

pub(super) fn run(_ctx: &TrayContext, _quit_tx: Sender<crate::TrayCommand>) -> ! {
    unreachable!("tray is not supported on this platform");
}
