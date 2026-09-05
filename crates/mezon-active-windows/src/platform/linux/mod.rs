mod proc_scan;
mod x11;

use crate::info::ActiveWindowInfo;

pub fn get_active_window() -> anyhow::Result<ActiveWindowInfo> {
    x11::get_active_window_x11().or_else(|x11_error| {
        proc_scan::scan_tracked_process().ok_or_else(|| {
            tracing::debug!(
                "X11 active window unavailable ({x11_error}); proc scan also found no tracked process"
            );
            x11_error
        })
    })
}
