use anyhow::{Context, Result};
use dirs::config_dir;
use mezon_client::Session;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

// ─── Clan domain model ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Clan {
    pub id: String,
    pub name: String,
    pub initials: String,
    pub avatar_url: Option<String>,
    pub unread_count: u32,
}

#[derive(Debug, Clone)]
pub struct ClansModel {
    pub clans: Vec<Clan>,
    pub active_clan_id: Option<String>,
}

impl ClansModel {
    pub fn with_dummy_data() -> Self {
        Self {
            clans: vec![
                Clan {
                    id: "1".into(),
                    name: "Gaming".into(),
                    initials: "GM".into(),
                    avatar_url: None,
                    unread_count: 3,
                },
                Clan {
                    id: "2".into(),
                    name: "Work".into(),
                    initials: "WK".into(),
                    avatar_url: None,
                    unread_count: 0,
                },
                Clan {
                    id: "3".into(),
                    name: "Friends".into(),
                    initials: "FR".into(),
                    avatar_url: None,
                    unread_count: 12,
                },
                Clan {
                    id: "4".into(),
                    name: "Dev".into(),
                    initials: "DV".into(),
                    avatar_url: None,
                    unread_count: 0,
                },
                Clan {
                    id: "5".into(),
                    name: "Music".into(),
                    initials: "MU".into(),
                    avatar_url: None,
                    unread_count: 1,
                },
            ],
            active_clan_id: Some("1".into()),
        }
    }

    pub fn select_clan(&mut self, id: &str) {
        self.active_clan_id = Some(id.to_string());
    }
}

// ─── Channel domain model ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Text,
    Voice,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub unread: bool,
    pub private: bool,
}

#[derive(Debug, Clone)]
pub struct Category {
    pub clan_id: String,
    pub name: String,
    pub collapsed: bool,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone)]
pub struct ChannelsModel {
    pub categories: Vec<Category>,
    pub active_channel_id: Option<String>,
}

impl ChannelsModel {
    pub fn with_dummy_data() -> Self {
        fn channel(
            id: &str,
            name: &str,
            ch_type: ChannelType,
            unread: bool,
            private: bool,
        ) -> Channel {
            Channel {
                id: id.into(),
                name: name.into(),
                channel_type: ch_type,
                unread,
                private,
            }
        }

        fn category(
            clan_id: &str,
            name: &str,
            collapsed: bool,
            channels: Vec<Channel>,
        ) -> Category {
            Category {
                clan_id: clan_id.into(),
                name: name.into(),
                collapsed,
                channels,
            }
        }

        Self {
            categories: vec![
                category(
                    "1",
                    "GENERAL",
                    false,
                    vec![
                        channel("c1", "announcements", ChannelType::Text, false, false),
                        channel("c2", "general", ChannelType::Text, true, false),
                        channel("c3", "voice-chat", ChannelType::Voice, false, false),
                    ],
                ),
                category(
                    "1",
                    "GAMING",
                    true,
                    vec![
                        channel("c4", "lol", ChannelType::Text, false, false),
                        channel("c5", "csgo", ChannelType::Voice, false, true),
                    ],
                ),
                category(
                    "2",
                    "WORK",
                    false,
                    vec![
                        channel("c6", "meetings", ChannelType::Text, false, false),
                        channel("c7", "standup", ChannelType::Text, true, false),
                        channel("c8", "conf-room", ChannelType::Voice, false, false),
                    ],
                ),
                category(
                    "2",
                    "PROJECTS",
                    false,
                    vec![
                        channel("c9", "backend", ChannelType::Text, false, false),
                        channel("c10", "frontend", ChannelType::Text, true, false),
                    ],
                ),
            ],
            active_channel_id: None,
        }
    }

    pub fn categories_for_clan(&self, clan_id: &str) -> Vec<&Category> {
        self.categories
            .iter()
            .filter(|c| c.clan_id == clan_id)
            .collect()
    }

    pub fn select_channel(&mut self, id: &str) {
        self.active_channel_id = Some(id.to_string());
        if let Some(ch) = self
            .categories
            .iter_mut()
            .flat_map(|c| &mut c.channels)
            .find(|ch| ch.id == id)
        {
            ch.unread = false;
        }
    }

    pub fn toggle_category(&mut self, name: &str) {
        if let Some(cat) = self.categories.iter_mut().find(|c| c.name == name) {
            cat.collapsed = !cat.collapsed;
        }
    }
}

/// Persistent application settings — written to ~/.config/mezon/settings.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Launch app on system login
    pub auto_start: bool,
    /// Enable GPU hardware acceleration
    pub hardware_acceleration: bool,
    /// UI zoom/scale factor (0.8 – 1.5)
    pub zoom_factor: f32,
    /// Last window bounds [x, y, width, height]
    pub window_bounds: Option<[i32; 4]>,
    /// UI theme: "dark" | "light" | "system"
    pub theme: String,
    /// Enable desktop notifications
    pub notifications_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_start: false,
            hardware_acceleration: true,
            zoom_factor: 1.0,
            window_bounds: None,
            theme: "dark".to_string(),
            notifications_enabled: true,
        }
    }
}

impl Settings {
    /// Returns the path to the settings file: ~/.config/mezon/settings.json
    pub fn path() -> PathBuf {
        config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mezon")
            .join("settings.json")
    }

    /// Load settings from disk. Returns defaults if the file does not exist.
    pub async fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            tracing::debug!(
                "Settings file not found, using defaults: {}",
                path.display()
            );
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read settings from {}", path.display()))?;
        let settings: Self =
            serde_json::from_str(&data).with_context(|| "Failed to parse settings.json")?;
        tracing::debug!("Loaded settings from {}", path.display());
        Ok(settings)
    }

    /// Save settings to disk, creating the directory if needed.
    pub async fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(self).context("Failed to serialize settings")?;
        fs::write(&path, data)
            .await
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;
        tracing::debug!("Saved settings to {}", path.display());
        Ok(())
    }
}

/// Which login method is currently shown in the `LoginView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginMethod {
    /// Email OTP — two-step flow (default).
    #[default]
    Otp,
    /// Email + password — single-step form.
    Password,
}

/// A user-visible login error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// Wrong credentials / bad OTP.
    InvalidCredentials,
    /// The server returned an unexpected error.
    ServerError(String),
    /// Could not reach the server.
    NetworkError(String),
    /// The OTP has expired; user must request a new one.
    OtpExpired,
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCredentials => write!(f, "Invalid credentials. Please try again."),
            Self::ServerError(msg) => write!(f, "Server error: {msg}"),
            Self::NetworkError(msg) => write!(f, "Network error: {msg}"),
            Self::OtpExpired => write!(f, "OTP has expired. Please request a new one."),
        }
    }
}

/// Authentication state of the application.
///
/// Drives which view is shown in the content area of `RootView`.
#[derive(Debug, Clone, Default)]
pub enum AuthState {
    /// No session — show login form.
    #[default]
    NotAuthenticated,
    /// OTP email was sent; waiting for the user to enter the code.
    OtpRequested {
        /// Server-issued request ID — required for the confirm-OTP call.
        req_id: String,
        /// The email address the OTP was sent to.
        email: String,
    },
    /// OAuth2 browser was opened; waiting for the `mezonapp://callback` deep link.
    /// Kept for future OAuth integration.
    AwaitingCallback,
    /// Token received and session is valid.
    Authenticated(Session),
}
