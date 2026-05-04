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

use crate::session::Session;

// ─── Default connection constants ────────────────────────────────────────────

/// Default REST API host (desktop .env: NX_CHAT_APP_API_HOST)
pub const DEFAULT_API_HOST: &str = "dev-mezon.nccsoft.vn";
/// Default REST API port (desktop .env: NX_CHAT_APP_API_PORT)
pub const DEFAULT_API_PORT: u16 = 8088;
/// Default TLS (desktop .env: NX_CHAT_APP_API_SECURE)
pub const DEFAULT_API_SECURE: bool = true;
/// Server key used for Basic auth (desktop .env: NX_CHAT_APP_API_KEY)
pub const DEFAULT_SERVER_KEY: &str = "defaultkey";

// ─── Wire types ──────────────────────────────────────────────────────────────

/// Raw session response from the server (before normalisation into `Session`).
#[derive(Debug, Deserialize)]
struct ApiSession {
    token: String,
    refresh_token: String,
    #[allow(dead_code)]
    #[serde(default)]
    created: bool,
    #[allow(dead_code)]
    #[serde(default)]
    is_remember: bool,
    /// The REST API endpoint this client should use for subsequent calls.
    api_url: Option<String>,
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
        // ReqwestClient::new() calls reqwest::Client::builder().build().into()
        // The From<reqwest::Client> impl calls Handle::try_current(); if no tokio runtime
        // is current it falls back to the static OnceLock<Runtime>.
        // All subsequent .send() calls are spawned onto that handle — safe from any executor.
        Self {
            http: Arc::new(ReqwestClient::new()),
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

        let mut resp_bytes: Vec<u8> = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut resp_bytes)
            .await
            .with_context(|| format!("Failed to read response body from POST {url}"))?;

        if !status.is_success() {
            bail!(
                "HTTP {} on POST {url}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&resp_bytes).trim()
            );
        }

        serde_json::from_slice(&resp_bytes)
            .with_context(|| format!("Failed to parse JSON response from POST {url}"))
    }

    /// Convert an `ApiSession` wire response into our internal `Session`.
    fn parse_session(&self, api: ApiSession) -> Session {
        let (user_id, username, expires_at) = decode_jwt_claims(&api.token);
        Session {
            token: api.token,
            refresh_token: api.refresh_token,
            expires_at,
            api_url: api.api_url,
            ws_url: None,
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
}

// ─── JWT helpers ─────────────────────────────────────────────────────────────

/// Decode the payload segment of a JWT and extract `exp`, `uid`, `usn` claims.
/// Returns `(user_id, username, expires_at)`. Falls back to empty strings / 0 on error.
fn decode_jwt_claims(token: &str) -> (String, String, u64) {
    let payload = token.split('.').nth(1).unwrap_or("");
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap_or_default();
    let json: serde_json::Value = serde_json::from_slice(&decoded).unwrap_or_default();

    let user_id = json
        .get("uid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let username = json
        .get("usn")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let expires_at = json.get("exp").and_then(|v| v.as_u64()).unwrap_or(0);

    (user_id, username, expires_at)
}
