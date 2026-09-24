use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWindowInfo {
    pub os: String,
    #[serde(rename = "windowClass")]
    pub window_class: String,
    #[serde(rename = "windowName")]
    pub window_name: String,
    #[serde(rename = "windowDesktop")]
    pub window_desktop: String,
    #[serde(rename = "windowType")]
    pub window_type: String,
    #[serde(rename = "windowPid")]
    pub window_pid: String,
    #[serde(rename = "idleTime")]
    pub idle_time: String,
}

impl ActiveWindowInfo {
    pub fn app_name(&self) -> String {
        normalize_process_name(&self.window_class)
    }
}

pub fn normalize_process_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut name = trimmed;
    for suffix in [".exe", ".EXE", ".app", ".AppImage", ".appimage"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped;
        }
    }
    name.to_string()
}

pub fn parse_wm_class(raw: &str) -> (String, String) {
    let parts: Vec<&str> = raw.split('\0').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        [instance, class] => (instance.to_string(), class.to_string()),
        [single] => (single.to_string(), single.to_string()),
        _ => (String::new(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_process_name_strips_exe_and_app_suffixes() {
        assert_eq!(normalize_process_name("Code.exe"), "Code");
        assert_eq!(normalize_process_name("Spotify.app"), "Spotify");
        assert_eq!(normalize_process_name("Cursor"), "Cursor");
        assert_eq!(normalize_process_name("Cursor.AppImage"), "Cursor");
    }

    #[test]
    fn parse_wm_class_splits_instance_and_class() {
        let (instance, class) = parse_wm_class("code\0Code");
        assert_eq!(instance, "code");
        assert_eq!(class, "Code");
    }
}
