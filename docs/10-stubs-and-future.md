# Stub Crates and Stage 2 Entry Points

This document covers the crates and constants that are currently stubs — minimal placeholders that will expand significantly in Stage 2 (app shell) and beyond. Understanding what's a stub prevents confusion when reading the codebase.

---

## 1. `mezon-proto` — Protobuf Types

**Location:** `crates/mezon-proto/`

**Status:** Active `prost-build` generated protobuf crate.

### What It Is

`mezon-proto` contains generated wire types for Mezon API and realtime protocol messages.

### Current State

Generated modules:

```rust
mezon_proto::api
mezon_proto::realtime
```

Source files:

```text
crates/mezon-proto/src/api.proto
crates/mezon-proto/src/realtime.proto
crates/mezon-proto/build.rs
```

### Future Work

1. Add generated-type tests for critical API messages.
2. Keep `api.proto` and `realtime.proto` in sync with backend protocol updates.
3. Add conversion helpers only where UI/store models intentionally differ from wire types.

---

## 2. `mezon-updater` — Auto-Update (Stub)

**Location:** `crates/mezon-updater/src/lib.rs`

**Status:** Single function, always returns `Ok(None)`.

### What It Is

The updater will poll `https://cdn.mezon.ai/release/` for a manifest describing the latest version, compare it against `CARGO_PKG_VERSION`, download the update binary if newer, verify its signature, and trigger installation.

### Current State

```rust
pub const UPDATE_URL: &str = "https://cdn.mezon.ai/release/";

pub async fn check_for_updates(current_version: &str) -> anyhow::Result<Option<String>> {
    tracing::info!("Checking for updates (current: {})", current_version);
    // TODO: implement update check against UPDATE_URL
    Ok(None)  // always "no update available"
}
```

`check_for_updates` is now called from tray menu "Check for Updates" via `mezon_native::tray::MezonTray::new()`.

### What's Planned

- Fetch a `release.json` manifest from `UPDATE_URL`
- Parse `{ "version": "x.y.z", "url": "...", "sha256": "..." }`
- Compare semantic versions
- Download + SHA-256 verify
- On macOS: replace the app bundle and relaunch
- On Windows: run an NSIS installer silently
- Surface update prompts through `AuthState` or a new `AppEvent` type

---

## 3. WebSocket Constants (Declared, Unused)

**Location:** `crates/mezon-client/src/lib.rs`

These constants are declared for Stage 2 but not yet used:

```rust
pub const DEFAULT_WS_HOST: &str = "sock.mezon.ai";
pub const DEFAULT_WS_PORT: u16 = 443;
pub const DEFAULT_WS_SECURE: bool = true;
```

TCP/TLS transport is active instead (`AbridgedTcpAdapter` + `MezonTransport`). WebSocket constants remain declared but not wired.

---

## 4. `AuthState::AwaitingCallback` (Reserved)

In `mezon-store/src/lib.rs`:

```rust
/// OAuth2 browser was opened; waiting for the `mezonapp://callback` deep link.
/// Kept for future OAuth integration.
AwaitingCallback,
```

This variant exists and is handled in `RootView` (shows a "connecting" placeholder), but the OAuth flow itself is not implemented. It's triggered in `main.rs` when a `mezonapp://callback` deep link arrives:

```rust
if url.starts_with("mezonapp://callback") {
    *state = AuthState::AwaitingCallback;
}
```

When OAuth is implemented in a future stage, this is the entry point.

---

## 5. `Settings.theme` — Wired but Not Applied

`Settings` has a `theme: String` field ("dark" / "light" / "system"), and it is loaded correctly from disk. However, in `RootView` and all component `render()` methods, the theme is hard-coded:

```rust
let theme = Theme::dark();  // ignores settings.theme
```

The connection between `Settings.theme` and the active `Theme` instance is Stage 2 work. The plan is to move the active theme into a GPUI global (`cx.set_global(theme)`) so any view can access it via `cx.global::<Theme>()` without passing it manually.

---

## 6. `Settings.window_bounds` — Loaded but Never Saved

`Settings` persists the last window position/size as `[x, y, width, height]`. It is read at startup to restore the window:

```rust
// In open_main_window()
let window_bounds = if let Some([x, y, w, h]) = settings.window_bounds {
    WindowBounds::Windowed(Bounds { ... })
} else {
    WindowBounds::Windowed(Bounds::centered(...))
};
```

But `Settings::save()` is never called — so if you move the window and restart, it won't remember its position. Stage 2 will add a window-bounds observer that calls `Settings::save()` when the window moves or resizes.

---

## 7. `mezon-native/power.rs` Linux Stub

The Linux implementation of power events is a stub:

```rust
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn subscribe(_callback: Box<dyn Fn(PowerEvent) + Send>) {
    // TODO: implement via systemd-logind D-Bus or upower
}
```

Sleep/wake handling on Linux does not affect Stage 1 (no background WebSocket to pause). It will be implemented when Stage 2 adds the real-time connection.

---

## 8. `TitleBar` — Minimal, Will Expand

`TitleBar` currently renders: a small brand square + "Mezon" label + window controls (Windows/Linux only). It is a stub for Stage 2's full title bar which will include:

- User avatar (top-right)
- Status indicator dot
- Search bar
- Navigation back/forward buttons
- Server/DM breadcrumb

The current `title_bar.rs` is intentionally minimal to unblock Stage 1 rendering without depending on any user/auth data.

---

## 9. `RootView` Authenticated Placeholder → Replaced by `AccountTestView`

`render_authenticated_placeholder` no longer exists. `AccountTestView` renders post-login content (account info, clans, channels via shared TCP transport).

---

## Stage 2 Integration Map

| Stub | What replaces/extends it | Where |
|------|--------------------------|-------|
| `render_authenticated_placeholder` | `AccountTestView` (`MainLayout` as future Stage 2 work) | `mezon-ui/src/account_test_view.rs` |
| `DEFAULT_WS_*` constants | `AbridgedTcpAdapter` TCP transport (WebSocket stubbed) | `mezon-client/src/transport/` |
| New protobuf APIs | generated types | `mezon-proto/src/*.proto` + `build.rs` |
| `mezon-updater` stub | Full update check + download | `mezon-updater/src/lib.rs` |
| `Theme::dark()` hard-coded | `cx.global::<Theme>()` | `mezon-ui/src/theme.rs` + `root.rs` |
| `Settings::save()` never called | Window-move observer | `mezon-app/src/main.rs` |
| `AuthState::AwaitingCallback` | OAuth browser redirect handler | `mezon-client/src/oauth.rs` (new) |
| `TitleBar` minimal | Full title bar with user info | `mezon-ui/src/title_bar.rs` |
