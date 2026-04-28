use serde::{Deserialize, Serialize};

/// Authenticated session returned after login.
/// Mirrors the mezon-js Session object.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    /// Bearer token for API requests
    pub token: String,
    /// Refresh token for obtaining a new token
    pub refresh_token: String,
    /// Unix timestamp (seconds) when the token expires
    pub expires_at: u64,
    /// The WebSocket endpoint URL returned by the server after auth
    pub ws_url: Option<String>,
    /// The REST API endpoint URL returned by the server after auth
    pub api_url: Option<String>,
    /// User ID
    pub user_id: String,
    /// Username
    pub username: String,
}

impl Session {
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.expires_at > 0 && now >= self.expires_at
    }
}
