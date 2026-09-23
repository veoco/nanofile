//! Stub tray backend for platforms without tray support. Unreachable in
//! practice (`run_mode` returns `Headless` there); exists to keep the crate
//! compiling on unusual targets.

use tokio::sync::mpsc::UnboundedSender;

use super::TrayContext;

pub(super) fn run(
    _ctx: &TrayContext,
    _quit_tx: UnboundedSender<crate::TrayCommand>,
    _client_mode: bool,
) -> ! {
    unreachable!("tray is not supported on this platform");
}
