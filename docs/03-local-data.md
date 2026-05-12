# Local Data Management

The app has a deliberately minimal footprint: **two persistence mechanisms only**.
No database, no cache, no temp files.

---

## Summary Table

| Mechanism | Data stored | Location | Format |
|-----------|------------|----------|--------|
| JSON file | `Settings` struct | `~/.config/mezon/settings.json` | Pretty JSON |
| OS keychain | `Session` (auth tokens) | macOS Keychain / Win Credential Manager / Linux Secret Service | JSON string |
| Unix socket (transient) | Deep link URLs between app instances | `$XDG_RUNTIME_DIR/mezon.sock` | Raw UTF-8 |
| Windows named pipe (transient) | Deep link URLs between app instances | `\\.\pipe\mezon-single-instance` | Raw UTF-8 |
| Linux `.desktop` file | Deep link scheme registration | `~/.local/share/applications/mezon.desktop` | XDG Desktop format |
| Windows Registry | Deep link scheme + auto-start | `HKCU\Software\Classes\mezonapp` | REG_SZ |

The Unix socket and named pipe are **transient IPC** (inter-process communication),
not persistent storage — they disappear when the app exits.

---

## 1. Settings File

**Crate:** `mezon-store` (`crates/mezon-store/src/lib.rs`)

### What's stored

```rust
pub struct Settings {
    pub auto_start: bool,              // Launch at login
    pub hardware_acceleration: bool,   // GPU acceleration
    pub zoom_factor: f32,              // UI zoom (0.8–1.5)
    pub window_bounds: Option<[i32; 4]>, // [x, y, width, height]
    pub theme: String,                 // "dark" | "light" | "system"
    pub notifications_enabled: bool,
}
```

### File location

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/mezon/settings.json` |
| Linux | `~/.config/mezon/settings.json` |
| Windows | `%APPDATA%\mezon\settings.json` |

### How it works

**Loading** (called once at startup in `main.rs`):
```rust
let settings = rt.block_on(Settings::load()).unwrap_or_default();
```

- Reads the JSON file with `tokio::fs::read_to_string`
- Deserializes with `serde_json::from_str`
- If file is **missing**: silently returns `Default::default()` (no error)
- If file is **malformed**: returns error, but `unwrap_or_default()` in `main.rs` falls back to defaults

**Saving:**
```rust
// In mezon-store/src/lib.rs
pub async fn save(&self) -> Result<()> {
    let path = settings_path();
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    let json = serde_json::to_string_pretty(self)?;
    tokio::fs::write(&path, json).await?;
    Ok(())
}
```

> **Known gap:** `Settings::save()` is implemented but **never called anywhere** in the
> codebase. This means settings changes (theme, window position) are not persisted yet.
> This will be wired up in Stage 3 (Settings UI).

---

## 2. Session Tokens (OS Keychain)

**Crate:** `mezon-client` (`crates/mezon-client/src/keychain.rs`)

### Why the OS keychain?

The keychain is the secure, OS-managed secret store — like an encrypted vault the OS manages on your behalf. On macOS it's Keychain Services, on Windows it's Credential Manager, on Linux it's the Secret Service (GNOME Keyring / KWallet). Much more secure than a plain file.

In web terms: it's like `httpOnly` + `Secure` cookies, but at the OS level — other apps cannot read it, and it persists across app restarts.

### What's stored

The entire `Session` struct is JSON-serialized into a single keychain entry:

```rust
pub struct Session {
    pub token: String,          // JWT access token
    pub refresh_token: String,  // Long-lived refresh token
    pub expires_at: u64,        // Unix timestamp (seconds)
    pub ws_url: Option<String>, // WebSocket host (returned by server after auth)
    pub api_url: Option<String>,// REST API host (returned by server after auth)
    pub user_id: String,
    pub username: String,
    pub api_host: Option<String>,  // REST host parsed from url endpoints
    pub api_port: Option<u16>,     // REST port parsed from url endpoints
    pub api_secure: Option<bool>,  // REST TLS parsed from url endpoints
    pub ws_host: Option<String>,   // WebSocket host parsed from url endpoints
    pub ws_port: Option<u16>,      // WebSocket port parsed from url endpoints
    pub ws_secure: Option<bool>,   // WebSocket TLS parsed from url endpoints
    pub tcp_url: Option<String>,   // TCP transport URL parsed from url endpoints
    pub tcp_host: Option<String>,  // TCP transport host parsed from url endpoints
    pub tcp_port: Option<u16>,     // TCP transport port parsed from url endpoints
}
```

**Keychain entry details:**
- Service name: `"mezon-desktop"`
- Account name: `"session"`
- Value: `serde_json::to_string(&session)` — the whole Session as a JSON string

### The three operations

```rust
// Save after successful login
keychain::save_session(&session)?;

// Load on startup
let session: Option<Session> = keychain::load_session();
// Returns None if nothing stored OR if data is corrupt (logs a warning)

// Clear on logout or failed refresh
keychain::clear_session()?;
```

### Lifecycle

```
App starts
  └─ keychain::load_session()
       ├─ None → show login screen
       ├─ Valid session → AuthState::Authenticated (skip login)
       └─ Expired session → call /v2/account/session/refresh
            ├─ OK → keychain::save_session(new) → Authenticated
            └─ Error → keychain::clear_session() → NotAuthenticated

User logs in
  └─ API call succeeds → keychain::save_session(&session)

Background task (every 60s)
  └─ Token expires in <5min → call /v2/account/session/refresh
       ├─ OK → keychain::save_session(new)
       └─ Error → keychain::clear_session() → show login

The `spawn_transport_task` in main.rs reads `tcp_host`/`tcp_port` from the restored session to auto-connect shared TCP transport.
```

---

## 3. What is NOT persisted

| Data | Where it lives | Why not persisted |
|------|---------------|-------------------|
| `AuthState` | In-memory `Entity<AuthState>` | Reconstructed from keychain on startup |
| OTP step state | In-memory | Transient login flow |
| Deep link URLs | Channel / Unix socket | Transient IPC, not needed after handling |
| Window position | `settings.window_bounds` is read but never written | `Settings::save()` not yet called |

---

## 4. Platform-Specific Registration Files

These are written by `mezon-native` at startup — they are configuration/registration,
not application data:

### Linux: `.desktop` file

Written to `~/.local/share/applications/mezon.desktop` so the OS knows to open the
app when a `mezonapp://` URL is clicked. Written fresh on every launch (idempotent).

### Windows: Registry keys

Written to `HKCU\Software\Classes\mezonapp` for deep link scheme registration, and
to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for auto-start.

### macOS

No runtime file writing needed — the deep link scheme is declared in `Info.plist`
(compiled into the app bundle), and auto-start uses macOS Login Items APIs.
