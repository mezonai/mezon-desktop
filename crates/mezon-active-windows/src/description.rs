use crate::catalog::{ActivityKind, match_process_name};
use crate::info::ActiveWindowInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedActivity {
    pub app_name: String,
    pub description: String,
    pub kind: ActivityKind,
}

pub fn tracked_activity_from_window(info: &ActiveWindowInfo) -> Option<TrackedActivity> {
    let (app_name, kind) = match_process_name(&info.app_name())?;
    Some(TrackedActivity {
        app_name,
        description: String::new(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::ActiveWindowInfo;

    fn info(class: &str) -> ActiveWindowInfo {
        ActiveWindowInfo {
            os: "linux".into(),
            window_class: class.into(),
            window_name: "main.rs - secret-project - Visual Studio Code".into(),
            window_desktop: "0".into(),
            window_type: "0".into(),
            window_pid: "0".into(),
            idle_time: "0".into(),
        }
    }

    #[test]
    fn tracked_activity_reports_process_only() {
        let tracked = tracked_activity_from_window(&info("Code")).expect("code activity");
        assert_eq!(tracked.app_name, "Code");
        assert!(tracked.description.is_empty());
        assert_eq!(tracked.kind, ActivityKind::Coding);
    }

    #[test]
    fn rejects_untracked_apps() {
        assert!(tracked_activity_from_window(&info("Google Chrome")).is_none());
    }
}
