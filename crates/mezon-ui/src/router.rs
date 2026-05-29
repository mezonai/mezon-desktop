#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Chat,
    Direct,
    DirectMessage {
        direct_id: String,
        message_type: String,
    },
    Channel {
        clan_id: String,
        channel_id: String,
    },
    SettingsAccount,
    SettingsProfile,
    SettingsDevices,
    SettingsAppearance,
    SettingsActivity,
    SettingsNotifications,
    SettingsLanguage,
    SettingsVoice,
    SettingsAdvanced,
    NotFound {
        path: String,
    },
}

#[derive(Debug, Clone)]
pub struct Router {
    history: Vec<String>,
    current: usize,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    pub const DEFAULT_PATH: &'static str = "/chat";

    pub fn new() -> Self {
        Self {
            history: vec![Self::DEFAULT_PATH.to_string()],
            current: 0,
        }
    }

    pub fn current_path(&self) -> &str {
        &self.history[self.current]
    }

    pub fn navigate(&mut self, path: impl Into<String>) {
        let path = normalize_path(path.into());
        self.history.truncate(self.current + 1);
        self.history.push(path);
        self.current = self.history.len() - 1;
    }

    pub fn replace(&mut self, path: impl Into<String>) {
        self.history[self.current] = normalize_path(path.into());
    }

    pub fn go_back(&mut self) {
        if self.current > 0 {
            self.current -= 1;
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.current > 0
    }

    pub fn route(&self) -> Route {
        match_path(&self.history[self.current])
    }
}

pub fn match_path(path: &str) -> Route {
    let normalized = normalize_path(path);
    let segments = normalized
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    match segments.as_slice() {
        ["chat"] => Route::Chat,
        ["chat", "direct"] => Route::Direct,
        ["chat", "direct", "message", direct_id, message_type] => Route::DirectMessage {
            direct_id: (*direct_id).to_string(),
            message_type: (*message_type).to_string(),
        },
        ["chat", "clans", clan_id, "channels", channel_id] => Route::Channel {
            clan_id: (*clan_id).to_string(),
            channel_id: (*channel_id).to_string(),
        },
        ["settings"] | ["settings", "account"] => Route::SettingsAccount,
        ["settings", "profile"] => Route::SettingsProfile,
        ["settings", "devices"] => Route::SettingsDevices,
        ["settings", "appearance"] => Route::SettingsAppearance,
        ["settings", "activity"] => Route::SettingsActivity,
        ["settings", "notifications"] => Route::SettingsNotifications,
        ["settings", "language"] => Route::SettingsLanguage,
        ["settings", "voice"] => Route::SettingsVoice,
        ["settings", "advanced"] => Route::SettingsAdvanced,
        _ => Route::NotFound { path: normalized },
    }
}

fn normalize_path(path: impl AsRef<str>) -> String {
    let path = path.as_ref();
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Router::DEFAULT_PATH.to_string();
    }

    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.starts_with('/') {
        without_trailing.to_string()
    } else {
        format!("/{without_trailing}")
    }
}
