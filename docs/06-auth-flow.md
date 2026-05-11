# Authentication Flow

This document covers the complete authentication system: from HTTP client configuration through to session storage and UI state management.

## Overview

Authentication touches four crates:

| Crate | Role |
|-------|------|
| `mezon-client` | HTTP client, JWT decoding, session model, keychain module (`crates/mezon-client/src/keychain.rs`) |
| `mezon-store` | `AuthState` enum — drives which screen is shown |
| `mezon-ui` | `LoginView` renders the login UI and calls `mezon-client` |
| `mezon-native` | OS integration modules (tray, badge, notifications, etc.) |

The flow is:
```
LoginView (UI) → MezonClient (HTTP) → parse JWT → save keychain → update AuthState → re-render
```

---

## 1. The Session Model (`mezon-client/src/session.rs`)

```rust
pub struct Session {
    pub token: String,          // Bearer token for API requests
    pub refresh_token: String,  // Used to get a new token when expired
    pub expires_at: u64,        // Unix timestamp (seconds)
    pub user_id: String,        // Extracted from JWT claim "uid"
    pub username: String,       // Extracted from JWT claim "usn"
    pub api_url: Option<String>,  // REST host returned by the server post-auth
    pub ws_url: Option<String>,   // WebSocket host (populated in Stage 2)
    pub api_host: Option<String>,  // Parsed from url endpoints
    pub api_port: Option<u16>,     // Parsed from url endpoints
    pub api_secure: Option<bool>,  // Parsed from url endpoints
    pub ws_host: Option<String>,   // Parsed from url endpoints
    pub ws_port: Option<u16>,      // Parsed from url endpoints
    pub ws_secure: Option<bool>,   // Parsed from url endpoints
    pub tcp_url: Option<String>,   // Parsed from url endpoints
    pub tcp_host: Option<String>,  // Parsed from url endpoints
    pub tcp_port: Option<u16>,     // Parsed from url endpoints
}
```

`Session::is_expired()` compares `expires_at` against `SystemTime::now()`. The background refresh task (spawned in `main.rs`) calls this every 60 seconds and refreshes when within 5 minutes of expiry.

---

## 2. The HTTP Client (`mezon-client/src/auth.rs`)

### Why `ReqwestClient` Instead of a Plain `reqwest`?

GPUI runs its own `smol`-based async executor on the main thread. The standard `reqwest` client requires a tokio context. The vendored `reqwest_client` crate wraps reqwest with a `static OnceLock<Runtime>` — it spins up a dedicated tokio runtime on first use and parks HTTP futures on that runtime regardless of which executor calls `.await`. This means you can safely `cx.spawn()` an auth task in GPUI without needing a tokio runtime at the call site.

### Creating a Client

```rust
// Default (dev server)
let client = MezonClient::default();

// Explicit
let client = MezonClient::new("api.mezon.ai", 443, true, "mykey");
```

`MezonClient` is `Clone` (via `Arc<ReqwestClient>` inside) and should be wrapped in `Arc<MezonClient>` and shared across the app.

### HTTP Basics

All requests use **HTTP Basic auth**: `Authorization: Basic base64("serverkey:")`. Note the trailing colon — the password is always empty; only the server key is used.

Every API call goes through the private `post_json()` helper which:
1. Serialises the body to JSON bytes
2. Attaches the `Authorization` and `Content-Type` headers
3. Sends with `HttpClient::send()`
4. Reads the response body via `AsyncReadExt::read_to_end()`
5. Returns an error for non-2xx status codes
6. Deserialises the JSON response

### Auth Endpoints

| Method | Endpoint | What it does |
|--------|----------|--------------|
| `authenticate_email` | `POST /v2/account/authenticate/email` | Password login — single step |
| `request_otp` | `POST /v2/account/authenticate/emailotp` | Send OTP email, returns `req_id` |
| `confirm_otp` | `POST /v2/account/authenticate/confirmotp` | Verify OTP, returns session |
| `refresh_session` | `POST /v2/account/session/refresh` | Exchange refresh token for new session |

### JWT Decoding

After a successful API call, `parse_session()` calls `decode_jwt_claims()` to extract fields from the JWT token — without any third-party JWT library:

```
token = "header.payload.signature"
payload = base64url_decode(token.split('.')[1])
json = parse_json(payload)

user_id   = json["uid"]
username  = json["usn"]
expires_at = json["exp"]
```

The function handles decode/parse errors gracefully by returning empty strings and `0` for the timestamp.

### Dynamic API URL

After login, the server returns an `api_url` field. Calling `client.set_api_url(api_url)` updates the client's `host`, `port`, and `secure` fields so subsequent calls hit the correct production endpoint. This is important because the default `dev-mezon.nccsoft.vn:8088` is only for development.

---

## 3. Auth State Machine (`mezon-store/src/lib.rs`)

`AuthState` is a Rust enum that represents which authentication stage the app is in:

```
NotAuthenticated
    │ user enters email + clicks "Send OTP"
    ▼
OtpRequested { req_id, email }
    │ user enters 6-digit code + clicks "Verify"
    ▼
Authenticated(Session)
```

Password login skips the middle state:

```
NotAuthenticated
    │ user enters email + password + clicks "Sign In"
    ▼
Authenticated(Session)
```

`AwaitingCallback` is reserved for future OAuth2 flows (browser-based redirect). It's currently a dead branch in `RootView`.

This enum lives in `mezon-store` (not `mezon-ui`) so that it can be shared between the UI layer and `main.rs` without creating a circular dependency.

---

## 4. The Login View (`mezon-ui/src/login_view.rs`)

`LoginView` is a GPUI `Entity<LoginView>` that owns the UI state for the login screen.

### State Fields

```rust
pub struct LoginView {
    client: Arc<MezonClient>,       // injected — used for API calls
    auth_state: Entity<AuthState>,  // shared handle — updated on success

    method: LoginMethod,    // Otp | Password
    otp_step: u8,           // 0 = email entry, 1 = code entry

    email_field: Entity<FormField>,
    password_field: Entity<FormField>,
    otp_fields: Vec<Entity<FormField>>, // 6 individual digit boxes

    loading: bool,          // true while an HTTP call is in-flight
    error: Option<String>,  // shown below the form
    countdown: u32,         // seconds until OTP resend is available
}
```

### OTP Flow Step by Step

**Step 0 — Email entry:**
1. User types email and clicks "Send OTP"
2. `handle_send_otp()` validates the email is non-empty
3. Sets `loading = true`, calls `cx.notify()` to show spinner
4. `cx.spawn()` creates a GPUI async task that calls `client.request_otp(email).await`
5. On success: stores `req_id` and `email`, sets `otp_step = 1`, updates `AuthState` to `OtpRequested`, starts the 60-second countdown timer
6. On failure: sets `error` string and re-renders

**Step 1 — Code entry:**
1. User types 6 digits (one per box)
2. User clicks "Verify Code"
3. `handle_confirm_otp()` assembles the 6 digits into a single string
4. Calls `client.confirm_otp(req_id, otp_code).await`
5. On success: calls `on_auth_success()` which saves to keychain and transitions `AuthState` to `Authenticated`
6. On failure: clears OTP boxes and shows error

**Countdown timer:**
`start_countdown()` spawns a GPUI task that loops, sleeping 1 second at a time via `cx.background_executor().timer()`. Each tick decrements `countdown` and calls `cx.notify()`. When it hits 0 the loop breaks. The "Resend code" link only renders when `countdown == 0`.

### Password Flow

`handle_sign_in()` calls `client.authenticate_email(email, password).await` in a spawned task. On success, calls the same `on_auth_success()` helper.

### Post-Auth Success (shared)

```rust
fn on_auth_success(session: Session, auth_state: &Entity<AuthState>, cx: &mut App) {
    keychain::save_session(&session);       // persist to OS keychain
    tracing::info!(
        "Auth success — ws_url={:?}, api_url={:?}, tcp_url={:?}",
        session.ws_url, session.api_url, session.tcp_url
    );
    *auth_state = AuthState::Authenticated(session);
    cx.notify();                             // triggers RootView re-render
    // Transport connection and API verification are handled by the background
    // `spawn_transport_task`, not by the login view.
}
```

---

## 5. How `RootView` Uses `AuthState`

`RootView` reads `auth_state` on every render and switches the content area:

```rust
match auth_state {
    NotAuthenticated | OtpRequested { .. } => show LoginView,
    AwaitingCallback                        => show "connecting" message,
    Authenticated(_)                        => show placeholder (Stage 2 will render the app shell),
}
```

Since `auth_state` is an `Entity<AuthState>`, GPUI automatically re-renders `RootView` whenever `cx.notify()` is called on it.

---

## 6. Session Persistence (`mezon-client/src/keychain.rs`)

Sessions are stored in the OS keychain under the service name `mezon-desktop`. The `keyring` crate handles:

- **macOS**: Keychain Services
- **Windows**: Windows Credential Manager
- **Linux**: libsecret / kwallet

Key functions (see `keychain.rs`):
- `save_session(session)` — serialises to JSON, stores in keychain
- `load_session()` — loads and deserialises; returns `None` if absent
- `delete_session()` — called on logout (Stage 2)

The session is loaded at startup in `main.rs` before the UI opens, so the window can open directly to the authenticated state if a valid session exists.

`tcp_host` and `tcp_port` from loaded session are used by `spawn_transport_task` in main.rs to auto-connect the shared TCP transport after startup.

---

## Data Flow Diagram

```
[main.rs startup]
    │ keychain::load_session()
    │ client.refresh_session() if near-expiry
    ▼
[AuthState::Authenticated] ──────────────────── go directly to app
    or
[AuthState::NotAuthenticated] ────────────────── show LoginView

[LoginView] ──user action──▶ [MezonClient HTTP]
                                     │ ApiSession JSON
                                     ▼
                              [decode_jwt_claims()]
                                     │ Session { token, refresh_token, expires_at, user_id, ... }
                                     ▼
                              [keychain::save_session()]
                                     │
                                     ▼
                          [AuthState::Authenticated]
                                     │ cx.notify()
                                     ▼
                             [RootView re-renders]
```
