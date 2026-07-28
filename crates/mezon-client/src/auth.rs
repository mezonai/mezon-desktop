//! REST authentication client — mirrors the mezon-js `Client.authenticate*` methods.
//!
//! Uses `ReqwestClient` (from the vendored `reqwest_client` crate) which manages its own
//! dedicated tokio runtime via a `static OnceLock<Runtime>`. This means HTTP calls work
//! correctly when `.await`-ed from GPUI's smol-based executor — no tokio context needed at
//! the call site.
//!
//! All requests use HTTP Basic auth: `Authorization: Basic base64("{server_key}:")`.
//! After a successful login the server returns an `api_url` field; subsequent API calls
//! should be directed to that host (call `set_api_url` after auth).

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, http};
use reqwest_client::ReqwestClient;
use serde::{Deserialize, Serialize};

use crate::session::{Session, decode_jwt_claims};

// ─── Default connection constants ────────────────────────────────────────────
// Defaults for `MezonClient::default()`. Per-deployment config lives in
// [`mezon_store::AppConfig`] (`NX_*` baked at build time via `option_env!`).

/// Default REST API host (`NX_CHAT_APP_API_HOST`)
pub const DEFAULT_API_HOST: &str = "dev-mezon.nccsoft.vn";
/// Default REST API port (`NX_CHAT_APP_API_PORT`)
pub const DEFAULT_API_PORT: u16 = 8088;
/// Default TLS (`NX_CHAT_APP_API_SECURE`)
pub const DEFAULT_API_SECURE: bool = true;
/// Server key for Basic auth (`NX_CHAT_APP_API_KEY`)
pub const DEFAULT_SERVER_KEY: &str = "defaultkey";

// ─── Wire types ──────────────────────────────────────────────────────────────

/// Raw session response from the server (before normalisation into `Session`).
#[derive(Debug, Deserialize)]
struct ApiSession {
    token: String,
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    created: bool,
    #[allow(dead_code)]
    #[serde(default)]
    is_remember: bool,
    /// Socket credential returned after auth (used for WebSocket `token=` query param).
    #[serde(default)]
    session_id: Option<String>,
    /// The REST API endpoint this client should use for subsequent calls.
    api_url: Option<String>,
    /// The WebSocket endpoint URL returned by the server after auth.
    ws_url: Option<String>,
    /// The TCP endpoint URL returned by the server after auth.
    tcp_url: Option<String>,
}

/// Response from the OTP request endpoint.
#[derive(Debug, Deserialize)]
struct OtpRequestResponse {
    req_id: String,
}

// ─── Request bodies ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct EmailAuthBody<'a> {
    account: EmailAccount<'a>,
    username: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct EmailAccount<'a> {
    email: &'a str,
    password: &'a str,
}

#[derive(Debug, Serialize)]
struct OtpRequestBody<'a> {
    account: OtpAccount<'a>,
    username: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct OtpAccount<'a> {
    email: &'a str,
}

#[derive(Debug, Serialize)]
struct ConfirmOtpBody<'a> {
    otp_code: &'a str,
    req_id: &'a str,
}

#[derive(Debug, Serialize)]
struct RefreshBody<'a> {
    token: &'a str,
    is_remember: bool,
}

#[derive(Debug, Serialize)]
struct MezonAuthBody<'a> {
    account: MezonAccount<'a>,
    create: bool,
}

#[derive(Debug, Serialize)]
struct MezonAccount<'a> {
    token: &'a str,
}

#[derive(Debug, Serialize)]
struct SmsOtpRequestBody<'a> {
    account: SmsAccount<'a>,
}

#[derive(Debug, Serialize)]
struct SmsAccount<'a> {
    phoneno: &'a str,
}

#[derive(Debug, Deserialize)]
struct OtpConfirmResponse {
    req_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QrLoginId {
    pub login_id: String,
}

#[derive(Debug, Serialize)]
struct QrConfirmBody<'a> {
    login_id: &'a str,
    is_remember: bool,
}

// ─── Client ──────────────────────────────────────────────────────────────────

/// HTTP client for the Mezon REST API.
///
/// Backed by `ReqwestClient` which maintains a dedicated tokio runtime internally
/// (via `static OnceLock<Runtime>`), so HTTP calls work correctly when awaited from
/// GPUI's smol-based executor.
///
/// Create one instance at startup via [`MezonClient::new`] (or the [`Default`] impl
/// which uses dev defaults) and share it via `Arc<MezonClient>`.
#[derive(Clone)]
pub struct MezonClient {
    http: Arc<ReqwestClient>,
    host: String,
    port: u16,
    secure: bool,
    server_key: String,
}

impl Default for MezonClient {
    fn default() -> Self {
        Self::new(
            DEFAULT_API_HOST,
            DEFAULT_API_PORT,
            DEFAULT_API_SECURE,
            DEFAULT_SERVER_KEY,
        )
    }
}

impl MezonClient {
    /// Construct a new client with explicit connection parameters.
    pub fn new(host: &str, port: u16, secure: bool, server_key: &str) -> Self {
        // Built on the shared transport runtime (`new_http_client` enters it) so this REST client
        // reuses that runtime's handle instead of spinning up reqwest's own — one tokio for all.
        // `.send()` is then spawned onto that handle, safe to await from any executor.
        Self {
            http: Arc::new(crate::transport_runtime::new_http_client()),
            host: host.to_owned(),
            port,
            secure,
            server_key: server_key.to_owned(),
        }
    }

    /// Update the API host/port/scheme after a successful login using `api_url` from session.
    pub fn set_api_url(&mut self, api_url: &str) {
        if let Ok(parsed) = url::Url::parse(api_url) {
            if let Some(h) = parsed.host_str() {
                self.host = h.to_owned();
            }
            if let Some(p) = parsed.port() {
                self.port = p;
            }
            self.secure = parsed.scheme() == "https";
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn base_url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }

    fn basic_auth_header(&self) -> String {
        let raw = format!("{}:", self.server_key);
        format!("Basic {}", B64.encode(raw.as_bytes()))
    }

    /// Build, send and decode a JSON POST request.
    ///
    /// Uses `HttpClient::send()` directly so every detail of the request (headers,
    /// URL, body) is fully explicit. The response body is read via `AsyncReadExt`.
    async fn post_json<B: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R> {
        let url = format!("{}{}", self.base_url(), path);
        tracing::debug!("POST {url}");

        let body_bytes: Vec<u8> =
            serde_json::to_vec(body).context("Failed to serialise request body")?;

        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri(&url)
            .header("Authorization", self.basic_auth_header())
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body_bytes))
            .with_context(|| format!("Failed to build request for POST {url}"))?;

        let mut response = self
            .http
            .send(request)
            .await
            .with_context(|| format!("Network error on POST {url}"))?;

        let status = response.status();

        const MAX_AUTH_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
        let mut resp_bytes: Vec<u8> = Vec::new();
        response
            .body_mut()
            .take(MAX_AUTH_RESPONSE_BYTES + 1)
            .read_to_end(&mut resp_bytes)
            .await
            .with_context(|| format!("Failed to read response body from POST {url}"))?;
        if resp_bytes.len() as u64 > MAX_AUTH_RESPONSE_BYTES {
            bail!("Auth response from POST {url} exceeds {MAX_AUTH_RESPONSE_BYTES} bytes");
        }

        if !status.is_success() {
            let preview: String = String::from_utf8_lossy(&resp_bytes)
                .trim()
                .chars()
                .take(300)
                .collect();
            bail!("HTTP {} on POST {url}: {}", status.as_u16(), preview);
        }

        serde_json::from_slice(&resp_bytes)
            .with_context(|| format!("Failed to parse JSON response from POST {url}"))
    }

    /// Convert an `ApiSession` wire response into our internal `Session`.
    fn parse_session(&self, api: ApiSession) -> Session {
        let (user_id, username, expires_at) = decode_jwt_claims(&api.token);
        let (api_host, api_port, api_secure) = parse_endpoint(api.api_url.as_deref());
        let (ws_host, ws_port, ws_secure) = parse_endpoint(api.ws_url.as_deref());
        let (tcp_host, tcp_port, _) = parse_endpoint(api.tcp_url.as_deref());
        Session {
            token: api.token,
            refresh_token: api.refresh_token,
            id_token: api.id_token,
            session_id: api.session_id.unwrap_or_default(),
            expires_at: expires_at.unwrap_or(0),
            api_url: api.api_url,
            api_host,
            api_port,
            api_secure,
            tcp_url: api.tcp_url,
            tcp_host,
            tcp_port,
            ws_url: api.ws_url,
            ws_host,
            ws_port,
            ws_secure,
            user_id,
            username,
        }
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Email + password login.
    ///
    /// `POST /v2/account/authenticate/email`
    pub async fn authenticate_email(&self, email: &str, password: &str) -> Result<Session> {
        let body = EmailAuthBody {
            account: EmailAccount { email, password },
            username: None,
        };
        let api: ApiSession = self
            .post_json("/v2/account/authenticate/email", &body)
            .await?;
        Ok(self.parse_session(api))
    }

    /// OTP login — step 1: request an OTP email.
    ///
    /// `POST /v2/account/authenticate/emailotp`
    ///
    /// Returns the `req_id` to pass to [`confirm_otp`].
    pub async fn request_otp(&self, email: &str) -> Result<String> {
        let body = OtpRequestBody {
            account: OtpAccount { email },
            username: None,
        };
        let resp: OtpRequestResponse = self
            .post_json("/v2/account/authenticate/emailotp", &body)
            .await?;
        Ok(resp.req_id)
    }

    /// OTP login — step 2: verify the OTP code and obtain a session.
    ///
    /// `POST /v2/account/authenticate/confirmotp`
    pub async fn confirm_otp(&self, req_id: &str, otp_code: &str) -> Result<Session> {
        let body = ConfirmOtpBody { otp_code, req_id };
        let api: ApiSession = self
            .post_json("/v2/account/authenticate/confirmotp", &body)
            .await?;
        Ok(self.parse_session(api))
    }

    /// Refresh an existing session using its refresh token.
    ///
    /// `POST /v2/account/session/refresh`
    pub async fn refresh_session(&self, refresh_token: &str, is_remember: bool) -> Result<Session> {
        let body = RefreshBody {
            token: refresh_token,
            is_remember,
        };
        let api: ApiSession = self.post_json("/v2/account/session/refresh", &body).await?;
        Ok(self.parse_session(api))
    }

    pub async fn authenticate_mezon(&self, token: &str) -> Result<Session> {
        let body = MezonAuthBody {
            account: MezonAccount { token },
            create: false,
        };
        let api: ApiSession = self
            .post_json("/v2/account/authenticate/mezon", &body)
            .await?;
        Ok(self.parse_session(api))
    }

    pub async fn request_sms_otp(&self, phone: &str) -> Result<String> {
        let body = SmsOtpRequestBody {
            account: SmsAccount { phoneno: phone },
        };
        let resp: OtpConfirmResponse = self
            .post_json("/v2/account/authenticate/smsotp", &body)
            .await?;
        resp.req_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("SMS OTP request returned no req_id"))
    }

    pub async fn confirm_otp_sms(&self, req_id: &str, otp_code: &str) -> Result<Session> {
        let body = ConfirmOtpBody { otp_code, req_id };
        let api: ApiSession = self
            .post_json("/v2/account/authenticate/confirmotp", &body)
            .await?;
        Ok(self.parse_session(api))
    }

    pub async fn create_qr_login(&self) -> Result<QrLoginId> {
        let body = serde_json::json!({});
        let resp: QrLoginId = self
            .post_json("/v2/account/authenticate/createqrlogin", &body)
            .await?;
        Ok(resp)
    }

    pub async fn confirm_qr_login(&self, login_id: &str, is_remember: bool) -> Result<Session> {
        let body = QrConfirmBody {
            login_id,
            is_remember,
        };
        let api: ApiSession = self
            .post_json("/v2/account/authenticate/checklogin", &body)
            .await?;
        Ok(self.parse_session(api))
    }
}

fn parse_endpoint(endpoint: Option<&str>) -> (Option<String>, Option<u16>, Option<bool>) {
    let Some(endpoint) = endpoint else {
        return (None, None, None);
    };

    let endpoint = if endpoint.contains("://") {
        endpoint.to_owned()
    } else if endpoint.contains(':') {
        format!("tcp://{endpoint}")
    } else {
        // Bare hostname (e.g. `sock.mezon.ai`) — host-only endpoint; port comes from
        // session/config or TLS implicit default at connect time, not URL parsing.
        return (Some(endpoint.to_owned()), None, Some(true));
    };

    let parsed = url::Url::parse(&endpoint);
    let Ok(parsed) = parsed else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_endpoint() {
        assert_eq!(parse_endpoint(None), (None, None, None));

        let (host, port, secure) = parse_endpoint(Some("https://example.com:8443"));
        assert_eq!(host, Some("example.com".to_string()));
        assert_eq!(port, Some(8443));
        assert_eq!(secure, Some(true));

        let (host, port, secure) = parse_endpoint(Some("https://example.com"));
        assert_eq!(host, Some("example.com".to_string()));
        assert_eq!(port, Some(443));
        assert_eq!(secure, Some(true));

        let (host, port, secure) = parse_endpoint(Some("http://127.0.0.1:8080"));
        assert_eq!(host, Some("127.0.0.1".to_string()));
        assert_eq!(port, Some(8080));
        assert_eq!(secure, Some(false));

        let (host, port, secure) = parse_endpoint(Some("example.com:9000"));
        assert_eq!(host, Some("example.com".to_string()));
        assert_eq!(port, Some(9000));
        assert_eq!(secure, Some(false));

        let (host, port, secure) = parse_endpoint(Some("sock.mezon.ai"));
        assert_eq!(host, Some("sock.mezon.ai".to_string()));
        assert_eq!(port, None);
        assert_eq!(secure, Some(true));
    }

    #[test]
    fn test_decode_jwt_claims() {
        let token =
            "header.eyJ1aWQiOiJ1c2VyMTIzIiwidXNuIjoidGVzdHVzZXIiLCJleHAiOjE3MDAwMDAwMDB9.signature";
        let (user_id, username, expires_at) = decode_jwt_claims(token);
        assert_eq!(user_id, "user123");
        assert_eq!(username, "testuser");
        assert_eq!(expires_at, Some(1700000000));

        let (user_id, username, expires_at) = decode_jwt_claims("invalid_token");
        assert_eq!(user_id, "");
        assert_eq!(username, "");
        assert_eq!(expires_at, None);
    }
}
