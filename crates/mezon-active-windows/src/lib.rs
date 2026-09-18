mod catalog;
mod description;
mod info;
mod platform;

pub use catalog::{
    ACTIVITY_TYPE_CODING, ACTIVITY_TYPE_LIVE, ACTIVITY_TYPE_PLAY, ACTIVITY_TYPE_WORK, ActivityKind,
    classify_process_name, is_coding_app, match_linux_process, match_process_name,
    pick_highest_priority_match,
};
pub use description::{TrackedActivity, tracked_activity_from_window};
pub use info::{ActiveWindowInfo, normalize_process_name, parse_wm_class};

pub fn get_active_window() -> anyhow::Result<ActiveWindowInfo> {
    platform::get_active_window()
}
