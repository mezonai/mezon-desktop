use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

/// Authenticated session returned after login.
/// Mirrors the mezon-js Session object.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Session {
    /// Bearer token for API requests
    pub token: String,
    /// Refresh token for obtaining a new token
    pub refresh_token: String,
    /// OpenID id_token (JWT) used for MMN zk-proof generation
    #[serde(default)]
    pub id_token: String,
    /// Socket credential (`AAA…`) — preferred for WebSocket connect (mezon-js uses this over JWT).
    #[serde(default)]
    pub session_id: String,
    /// Unix timestamp (seconds) when the token expires
    pub expires_at: u64,
    pub is_remember: bool,
    /// The WebSocket endpoint URL returned by the server after auth
    pub ws_url: Option<String>,
    /// Parsed WebSocket host returned by the server after auth
    pub ws_host: Option<String>,
    /// Parsed WebSocket port returned by the server after auth
    pub ws_port: Option<u16>,
    /// Whether WebSocket endpoint uses TLS
    pub ws_secure: Option<bool>,
    /// The REST API endpoint URL returned by the server after auth
    pub api_url: Option<String>,
    /// Parsed REST API host returned by the server after auth
    pub api_host: Option<String>,
    /// Parsed REST API port returned by the server after auth
    pub api_port: Option<u16>,
    /// Whether REST API endpoint uses TLS
    pub api_secure: Option<bool>,
    /// The TCP endpoint URL returned by the server after auth
    pub tcp_url: Option<String>,
    /// Parsed TCP host returned by the server after auth
    pub tcp_host: Option<String>,
    /// Parsed TCP port returned by the server after auth
    pub tcp_port: Option<u16>,
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
        self.expires_at == 0 || now >= self.expires_at
    }

    /// Credential for `wss://…?token=…` — matches mezon-js (`session_id` first, else JWT).
    pub fn ws_credential(&self) -> &str {
        if !self.session_id.is_empty() {
            &self.session_id
        } else {
            &self.token
        }
    }

    /// Apply a server-pushed `refresh_session_event` (mezon-js `onrefreshsession`):
    /// adopt the new token / session_id and recompute expiry from the new JWT.
    pub fn apply_refresh(
        &mut self,
        token: &str,
        refresh_token: &str,
        session_id: &str,
        id_token: &str,
    ) {
        if !token.is_empty() {
            let (user_id, username, expires_at) = decode_jwt_claims(token);
            self.token = token.to_string();
            if let Some(exp) = expires_at {
                self.expires_at = exp;
            }
            if !user_id.is_empty() {
                self.user_id = user_id;
            }
            if !username.is_empty() {
                self.username = username;
            }
        }
        if !refresh_token.is_empty() {
            self.refresh_token = refresh_token.to_string();
        }
        if !session_id.is_empty() {
            self.session_id = session_id.to_string();
        }
        if !id_token.is_empty() {
            self.id_token = id_token.to_string();
        }
    }

    pub fn apply_healthy_endpoint(
        &mut self,
        session_id: &str,
        api_url: Option<&str>,
        ws_url: Option<&str>,
        tcp_url: Option<&str>,
    ) {
        if !session_id.is_empty() {
            self.session_id = session_id.to_string();
        }
        if let Some(url) = api_url.filter(|u| !u.is_empty()) {
            let (host, port, secure) = parse_endpoint(Some(url));
            if let Some(host) = host.filter(|h| !h.is_empty()) {
                self.api_url = Some(url.to_string());
                self.api_host = Some(host);
                self.api_port = port;
                self.api_secure = secure;
            }
        }
        if let Some(url) = ws_url.filter(|u| !u.is_empty()) {
            let (host, port, secure) = parse_endpoint(Some(url));
            if let Some(host) = host.filter(|h| !h.is_empty()) {
                self.ws_url = Some(url.to_string());
                self.ws_host = Some(host);
                self.ws_port = port;
                self.ws_secure = secure;
            }
        }
        if let Some(url) = tcp_url.filter(|u| !u.is_empty()) {
            let (host, port, _) = parse_endpoint(Some(url));
            if let Some(host) = host.filter(|h| !h.is_empty()) {
                self.tcp_url = Some(url.to_string());
                self.tcp_host = Some(host);
                self.tcp_port = port;
            }
        }
    }
}

pub(crate) fn parse_endpoint(
    endpoint: Option<&str>,
) -> (Option<String>, Option<u16>, Option<bool>) {
    let Some(endpoint) = endpoint else {
        return (None, None, None);
    };

    let endpoint = if endpoint.contains("://") {
        endpoint.to_owned()
    } else if endpoint.contains(':') {
        format!("tcp://{endpoint}")
    } else {
        return (Some(endpoint.to_owned()), None, Some(true));
    };

    let Ok(parsed) = url::Url::parse(&endpoint) else {
        return (None, None, None);
    };

    let secure = match parsed.scheme() {
        "https" | "wss" => Some(true),
        "http" | "ws" | "tcp" => Some(false),
        _ => None,
    };

    (
        parsed.host_str().map(str::to_owned),
        parsed.port_or_known_default(),
        secure,
    )
}

pub fn jwt_expires_at(token: &str) -> Option<u64> {
    decode_jwt_claims(token).2
}

pub(crate) fn decode_jwt_claims(token: &str) -> (String, String, Option<u64>) {
    let payload = token.split('.').nth(1).unwrap_or("");
    let decoded = URL_SAFE_NO_PAD.decode(payload).unwrap_or_default();
    let json: serde_json::Value = serde_json::from_slice(&decoded).unwrap_or_default();

    let user_id = json
        .get("uid")
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    let username = json
        .get("usn")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let expires_at = json
        .get("exp")
        .and_then(|v| v.as_u64())
        .filter(|&exp| exp > 0);

    (user_id, username, expires_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_is_expired() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let session = Session {
            expires_at: 0,
            ..Default::default()
        };
        assert!(
            session.is_expired(),
            "missing expiry (0) must be treated as expired"
        );

        let session = Session {
            expires_at: now + 1000,
            ..Default::default()
        };
        assert!(!session.is_expired());

        let session = Session {
            expires_at: now - 10,
            ..Default::default()
        };
        assert!(session.is_expired());
    }

    fn fake_jwt(user_id: &str, username: &str, exp: u64) -> String {
        let claims = format!(r#"{{"uid":"{user_id}","usn":"{username}","exp":{exp}}}"#);
        format!("header.{}.signature", URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn apply_refresh_adopts_the_new_token_and_its_expiry() {
        let mut session = Session {
            token: fake_jwt("7", "ngoc", 1000),
            refresh_token: "old-refresh".into(),
            session_id: "old-sid".into(),
            expires_at: 1000,
            user_id: "7".into(),
            username: "ngoc".into(),
            ..Default::default()
        };

        let renewed = fake_jwt("7", "ngoc", 9000);
        session.apply_refresh(&renewed, "new-refresh", "new-sid", "new-id-token");

        assert_eq!(session.token, renewed);
        assert_eq!(session.refresh_token, "new-refresh");
        assert_eq!(session.session_id, "new-sid");
        assert_eq!(session.expires_at, 9000);
        assert_eq!(session.id_token, "new-id-token");
    }

    #[test]
    fn apply_refresh_keeps_the_previous_id_token_when_the_server_sends_none() {
        let mut session = Session {
            id_token: "login-id-token".into(),
            ..Default::default()
        };

        session.apply_refresh("", "", "new-sid", "");

        assert_eq!(
            session.id_token, "login-id-token",
            "a refresh without an id_token must not strip the one zk proofs are minted from"
        );
    }

    #[test]
    fn apply_refresh_keeps_fields_the_server_left_empty() {
        let original = fake_jwt("7", "ngoc", 1000);
        let mut session = Session {
            token: original.clone(),
            refresh_token: "old-refresh".into(),
            session_id: "old-sid".into(),
            expires_at: 1000,
            ..Default::default()
        };

        session.apply_refresh("", "", "new-sid", "");

        assert_eq!(
            session.token, original,
            "an sid-only push must not clear the token"
        );
        assert_eq!(session.refresh_token, "old-refresh");
        assert_eq!(session.session_id, "new-sid");
        assert_eq!(session.expires_at, 1000);
    }

    #[test]
    fn apply_healthy_endpoint_adopts_sid_and_urls() {
        let mut session = Session {
            session_id: "old".into(),
            ..Default::default()
        };
        session.apply_healthy_endpoint(
            "new-sid",
            Some("https://api.mezon.ai:443"),
            Some("wss://ws.mezon.ai:443"),
            Some("tcp://tcp.mezon.ai:7349"),
        );
        assert_eq!(session.session_id, "new-sid");
        assert_eq!(session.api_url.as_deref(), Some("https://api.mezon.ai:443"));
        assert_eq!(session.api_host.as_deref(), Some("api.mezon.ai"));
        assert_eq!(session.tcp_host.as_deref(), Some("tcp.mezon.ai"));
        assert_eq!(session.tcp_port, Some(7349));
    }

    #[test]
    fn apply_healthy_endpoint_keeps_hosts_when_url_does_not_parse() {
        let mut session = Session {
            session_id: "old".into(),
            api_url: Some("https://api.mezon.ai:443".into()),
            api_host: Some("api.mezon.ai".into()),
            api_port: Some(443),
            ws_url: Some("wss://ws.mezon.ai:443".into()),
            ws_host: Some("ws.mezon.ai".into()),
            ws_port: Some(443),
            tcp_url: Some("tcp://tcp.mezon.ai:7349".into()),
            tcp_host: Some("tcp.mezon.ai".into()),
            tcp_port: Some(7349),
            ..Default::default()
        };
        session.apply_healthy_endpoint(
            "new-sid",
            Some("https://["),
            Some("://bad"),
            Some("tcp://["),
        );
        assert_eq!(session.session_id, "new-sid");
        assert_eq!(session.api_host.as_deref(), Some("api.mezon.ai"));
        assert_eq!(session.api_url.as_deref(), Some("https://api.mezon.ai:443"));
        assert_eq!(session.ws_host.as_deref(), Some("ws.mezon.ai"));
        assert_eq!(session.ws_url.as_deref(), Some("wss://ws.mezon.ai:443"));
        assert_eq!(session.tcp_host.as_deref(), Some("tcp.mezon.ai"));
        assert_eq!(session.tcp_url.as_deref(), Some("tcp://tcp.mezon.ai:7349"));
        assert_eq!(session.tcp_port, Some(7349));
    }
}
