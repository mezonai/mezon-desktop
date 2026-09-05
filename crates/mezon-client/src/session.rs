use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::RealtimeEndpoint;

#[cfg(debug_assertions)]
const DEBUG_FAILOVER_SIMULATION_ENV: &str = "MEZON_DEBUG_FAILOVER_SIMULATION";
#[cfg(debug_assertions)]
const DEBUG_FAILOVER_UNREACHABLE_PRIMARY: &str = "unreachable-primary";
#[cfg(debug_assertions)]
static DEBUG_FAILOVER_POISON_SPENT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HealthyEndpointReason {
    Unreachable = 1,
    HighLatency = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct HealthyEndpointSession {
    #[serde(alias = "userId")]
    pub user_id: String,
    #[serde(alias = "sessionId")]
    pub session_id: String,
    #[serde(alias = "apiUrl")]
    pub api_url: Option<String>,
    #[serde(alias = "wsUrl")]
    pub ws_url: Option<String>,
    #[serde(alias = "tcpUrl")]
    pub tcp_url: Option<String>,
    #[serde(alias = "endpointId", deserialize_with = "deserialize_endpoint_id")]
    pub endpoint_id: i32,
}

impl HealthyEndpointSession {
    pub fn realtime_endpoint(&self, default_port: Option<u16>) -> Option<RealtimeEndpoint> {
        let (tcp_host, tcp_port, _) = parse_endpoint(self.tcp_url.as_deref());
        let (ws_host, ws_port, _) = parse_endpoint(self.ws_url.as_deref());
        let host = named_host(tcp_host).or_else(|| named_host(ws_host))?;
        Some(RealtimeEndpoint {
            id: self.endpoint_id,
            host,
            port: tcp_port.or(ws_port).or(default_port).unwrap_or(443),
        })
    }
}

fn named_host(host: Option<String>) -> Option<String> {
    host.filter(|host| !host.is_empty())
}

pub(crate) fn deserialize_endpoint_id<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum WireEndpointId {
        Number(i64),
        Text(String),
        Unset,
    }

    Ok(match WireEndpointId::deserialize(deserializer)? {
        WireEndpointId::Number(id) => i32::try_from(id).unwrap_or_default(),
        WireEndpointId::Text(id) => id.trim().parse().unwrap_or_default(),
        WireEndpointId::Unset => 0,
    })
}

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
    #[serde(default)]
    pub endpoint_id: i32,
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
        endpoint: &HealthyEndpointSession,
        default_port: Option<u16>,
    ) -> bool {
        if endpoint.realtime_endpoint(default_port).is_none() {
            return false;
        }
        if !endpoint.user_id.is_empty() && endpoint.user_id != self.user_id {
            return false;
        }
        let node_before = (self.named_realtime_host(), self.realtime_port(default_port));

        let api_url = endpoint
            .api_url
            .clone()
            .filter(|url| !url.is_empty())
            .or_else(|| self.api_url.clone());
        let ws_url = endpoint
            .ws_url
            .clone()
            .filter(|url| !url.is_empty())
            .or_else(|| self.ws_url.clone());
        let tcp_url = endpoint
            .tcp_url
            .clone()
            .filter(|url| !url.is_empty())
            .or_else(|| self.tcp_url.clone());
        let (api_host, api_port, api_secure) = parse_endpoint(api_url.as_deref());
        let (ws_host, ws_port, ws_secure) = parse_endpoint(ws_url.as_deref());
        let (tcp_host, tcp_port, _) = parse_endpoint(tcp_url.as_deref());

        self.api_url = api_url;
        self.api_host = api_host;
        self.api_port = api_port;
        self.api_secure = api_secure;
        self.ws_url = ws_url;
        self.ws_host = ws_host;
        self.ws_port = ws_port;
        self.ws_secure = ws_secure;
        self.tcp_url = tcp_url;
        self.tcp_host = tcp_host;
        self.tcp_port = tcp_port;

        let node_after = (self.named_realtime_host(), self.realtime_port(default_port));
        let stays_on_the_same_node = node_before.0.is_some() && node_before == node_after;

        if !endpoint.session_id.is_empty()
            && (!stays_on_the_same_node || self.session_id.is_empty())
        {
            self.session_id = endpoint.session_id.clone();
        }
        self.endpoint_id = if endpoint.endpoint_id > 0 {
            endpoint.endpoint_id
        } else if stays_on_the_same_node {
            self.endpoint_id
        } else {
            0
        };
        true
    }

    pub fn realtime_endpoint(
        &self,
        default_host: &str,
        default_port: Option<u16>,
    ) -> Option<RealtimeEndpoint> {
        #[cfg(debug_assertions)]
        if let Some(poisoned) = debug_failover_poisoned_endpoint() {
            return Some(poisoned);
        }
        let host = self
            .named_realtime_host()
            .unwrap_or_else(|| default_host.to_string());
        if host.is_empty() {
            return None;
        }
        Some(RealtimeEndpoint {
            id: self.endpoint_id,
            host,
            port: self.realtime_port(default_port),
        })
    }

    fn named_realtime_host(&self) -> Option<String> {
        named_host(self.tcp_host.clone()).or_else(|| named_host(self.ws_host.clone()))
    }

    fn realtime_port(&self, default_port: Option<u16>) -> u16 {
        self.tcp_port
            .or(self.ws_port)
            .or(default_port)
            .unwrap_or(443)
    }
}

#[cfg(debug_assertions)]
fn debug_failover_poisoned_endpoint() -> Option<RealtimeEndpoint> {
    if std::env::var(DEBUG_FAILOVER_SIMULATION_ENV).as_deref()
        != Ok(DEBUG_FAILOVER_UNREACHABLE_PRIMARY)
        || DEBUG_FAILOVER_POISON_SPENT.swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return None;
    }
    Some(RealtimeEndpoint {
        id: 0,
        host: "127.0.0.1".to_string(),
        port: 1,
    })
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
    fn a_session_yields_the_one_node_the_gateway_named() {
        let session = Session {
            endpoint_id: 2,
            api_url: Some("https://api2.example.com".into()),
            tcp_host: Some("sock2.example.com".into()),
            tcp_port: Some(4433),
            ..Default::default()
        };

        let endpoint = session
            .realtime_endpoint("default.example.com", Some(7349))
            .expect("a node");
        assert_eq!(endpoint.id, 2);
        assert_eq!(endpoint.host, "sock2.example.com");
        assert_eq!(endpoint.port, 4433);
    }

    #[test]
    fn a_session_without_a_gateway_id_falls_back_to_the_configured_host() {
        let session = Session::default();

        let endpoint = session
            .realtime_endpoint("default.example.com", Some(7349))
            .expect("a node");
        assert_eq!(endpoint.host, "default.example.com");
        assert_eq!(endpoint.port, 7349);
        assert_eq!(endpoint.id, 0);
    }

    #[test]
    fn an_empty_realtime_url_falls_back_instead_of_producing_a_nameless_node() {
        let session = Session {
            tcp_url: Some(String::new()),
            tcp_host: Some(String::new()),
            ws_host: Some("sock.example.com".into()),
            ws_port: Some(4433),
            ..Default::default()
        };

        let endpoint = session
            .realtime_endpoint("default.example.com", Some(7349))
            .expect("an empty tcp host must not shadow the ws host");
        assert_eq!(endpoint.host, "sock.example.com");
    }

    #[test]
    fn a_gateway_answer_that_leaves_the_session_where_it_is_is_not_a_move() {
        let mut session = Session {
            user_id: "7".into(),
            session_id: "live-sid".into(),
            tcp_url: Some("sock.example.com:4433".into()),
            tcp_host: Some("sock.example.com".into()),
            tcp_port: Some(4433),
            ..Default::default()
        };
        let confirmed = HealthyEndpointSession {
            user_id: "7".into(),
            session_id: "minted-sid".into(),
            ws_url: Some("wss://sock.example.com".into()),
            tcp_url: Some(String::new()),
            ..Default::default()
        };

        assert!(session.apply_healthy_endpoint(&confirmed, Some(4433)));
        assert_eq!(session.tcp_host.as_deref(), Some("sock.example.com"));
        assert_eq!(session.tcp_port, Some(4433));
        assert_eq!(
            session.session_id, "live-sid",
            "a 60-second credential must not replace the one the live socket uses"
        );
    }

    #[test]
    fn moving_to_another_node_adopts_the_credential_minted_for_it() {
        let mut session = Session {
            user_id: "7".into(),
            session_id: "old-sid".into(),
            tcp_url: Some("sock.example.com:4433".into()),
            tcp_host: Some("sock.example.com".into()),
            tcp_port: Some(4433),
            ..Default::default()
        };
        let moved = HealthyEndpointSession {
            user_id: "7".into(),
            session_id: "minted-sid".into(),
            tcp_url: Some("sock2.example.com:4433".into()),
            ..Default::default()
        };

        assert!(session.apply_healthy_endpoint(&moved, Some(4433)));
        assert_eq!(session.session_id, "minted-sid");
        assert_eq!(session.tcp_host.as_deref(), Some("sock2.example.com"));
    }

    #[test]
    fn an_endpoint_id_the_gateway_sends_as_a_string_still_decodes() {
        let numeric: HealthyEndpointSession =
            serde_json::from_str(r#"{"tcp_url":"sock:4433","endpoint_id":11}"#).expect("numeric");
        let text: HealthyEndpointSession =
            serde_json::from_str(r#"{"tcp_url":"sock:4433","endpoint_id":"11"}"#).expect("string");
        let absent: HealthyEndpointSession =
            serde_json::from_str(r#"{"tcp_url":"sock:4433"}"#).expect("absent");

        assert_eq!(numeric.endpoint_id, 11);
        assert_eq!(text.endpoint_id, 11);
        assert_eq!(absent.endpoint_id, 0);
    }

    #[test]
    fn healthy_endpoint_replaces_urls_and_socket_credential() {
        let mut session = Session {
            user_id: "7".into(),
            session_id: "old-sid".into(),
            api_url: Some("https://old-api.example.com".into()),
            tcp_url: Some("old-sock.example.com:4433".into()),
            tcp_host: Some("old-sock.example.com".into()),
            tcp_port: Some(4433),
            ..Default::default()
        };
        let response = HealthyEndpointSession {
            user_id: "7".into(),
            session_id: "new-sid".into(),
            api_url: Some("https://new-api.example.com".into()),
            ws_url: Some("wss://new-sock.example.com".into()),
            tcp_url: Some("new-sock.example.com:4433".into()),
            endpoint_id: 2,
        };

        assert!(session.apply_healthy_endpoint(&response, Some(4433)));
        assert_eq!(session.session_id, "new-sid");
        assert_eq!(session.tcp_host.as_deref(), Some("new-sock.example.com"));
        assert_eq!(session.endpoint_id, 2);
        assert_eq!(
            session
                .realtime_endpoint("default.example.com", Some(4433))
                .map(|endpoint| endpoint.host),
            Some("new-sock.example.com".into())
        );
    }

    #[test]
    fn healthy_endpoint_rejects_another_users_session() {
        let mut session = Session {
            user_id: "7".into(),
            session_id: "old-sid".into(),
            ..Default::default()
        };
        let response = HealthyEndpointSession {
            user_id: "8".into(),
            session_id: "new-sid".into(),
            tcp_url: Some("new-sock.example.com:4433".into()),
            endpoint_id: 2,
            ..Default::default()
        };

        assert!(!session.apply_healthy_endpoint(&response, Some(4433)));
        assert_eq!(session.session_id, "old-sid");
    }

    #[test]
    fn the_same_node_coming_back_keeps_the_id_the_gateway_omitted() {
        let mut session = Session {
            user_id: "7".into(),
            endpoint_id: 3,
            tcp_url: Some("sock.example.com:4433".into()),
            tcp_host: Some("sock.example.com".into()),
            tcp_port: Some(4433),
            ..Default::default()
        };
        let confirmed = HealthyEndpointSession {
            user_id: "7".into(),
            tcp_url: Some("sock.example.com:4433".into()),
            ..Default::default()
        };

        assert!(session.apply_healthy_endpoint(&confirmed, Some(4433)));
        assert_eq!(session.endpoint_id, 3);

        let moved = HealthyEndpointSession {
            user_id: "7".into(),
            tcp_url: Some("sock2.example.com:4433".into()),
            ..Default::default()
        };
        assert!(session.apply_healthy_endpoint(&moved, Some(4433)));
        assert_eq!(session.endpoint_id, 0);
    }
}
