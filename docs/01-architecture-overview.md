# Architecture Overview

## What is this project?

Mezon Desktop is a native desktop client for the Mezon chat platform — think Discord-like.
It is being rewritten from Electron (web-based) to native Rust using **GPUI**, the same
GPU-accelerated UI framework that powers the [Zed](https://zed.dev) code editor.

**Current status:** Stage 1 complete (auth pages + TCP transport for app APIs). Stage 2 (app shell) is next.
**Platform priority:** macOS first, then Windows and Linux.

---

## Tech Stack

| Layer | Technology | Web equivalent |
|-------|-----------|----------------|
| UI framework | GPUI (vendored from Zed) | React |
| Rendering | Metal (macOS), wgpu (Win/Linux) | Browser GPU compositor |
| Async runtime | tokio + smol | Node.js event loop |
| HTTP client | reqwest (Zed fork) | fetch / axios |
| TCP/TLS transport | tokio-rustls | — |
| Protobuf | prost | protobufjs |
| Envelope protocol | MezonTransport | — |
| Serialization | serde + serde_json | JSON.parse / JSON.stringify |
| OS keychain | keyring crate | httpOnly cookies (OS-level) |
| Settings | JSON file on disk | localStorage (but a file) |
| Logging | tracing + tracing-subscriber | winston / pino |

---

## Crate Dependency Map

A "crate" in Rust is the equivalent of an npm package. This workspace has several:

```
mezon-app            ← Binary entry point (like index.js / server.js)
  │                    Handles: app bootstrap, window, tray, deep links
  │
  ├── mezon-ui       ← All UI views (one file per screen)
  │     │              Like your React pages/components
  │     ├── mezon-store   ← App state models (AuthState, Settings, etc.)
  │     │                   Like a Redux store
  │     ├── mezon-client  ← REST API client + session management
  │     │                   Like mezon-js / axios service layer
  │     └── mezon-native  ← OS-specific APIs
  │                         Like Electron's main process APIs
  │
  ├── mezon-client   ← Same as above (also used directly by mezon-app)
  │     ├── mezon-proto   ← Protobuf type definitions (active, used by transport)
  │     ├── AppApi        ← UI-safe API wrapper over shared TCP transport
  │     ├── TransportClient  ← Transport with dedicated tokio runtime
  │     ├── MezonTransport   ← Protobuf Envelope API client
  │     └── AbridgedTcpAdapter  ← TCP/TLS wire protocol + framing
  │
  ├── mezon-native   ← Tray, badge, notifications, deep link, power, single-instance
  │
  └── mezon-updater  ← Auto-update checker (stub — always returns "no update")
```

Additionally, `crates/vendor/` contains a **vendored copy of GPUI** — the full UI framework
copied from the Zed repository. You should not modify these crates.

---

## Directory Structure

```
mezon-desktop/
├── crates/
│   ├── mezon-app/         ← Entry point binary
│   │   └── src/main.rs
│   ├── mezon-ui/          ← Views and components
│   │   └── src/
│   │       ├── login_view.rs
│   │       ├── root.rs
│   │       ├── title_bar.rs
│   │       ├── theme.rs
│   │       └── components/
│   │           ├── primitives/   ← Avatar, Button, Icon, Label, Spinner, TextInput...
│   │           └── compositions/ ← FormField, IconButton, SectionHeader...
│   ├── mezon-client/      ← REST auth + TCP transport + protobuf API + keychain + AppApi
│   ├── mezon-store/       ← State structs (AuthState, Settings)
│   ├── mezon-native/      ← OS APIs (tray, badge, deep link, etc.)
│   ├── mezon-updater/     ← Auto-update stub
│   ├── mezon-proto/       ← Protobuf schema + build.rs (api.proto, realtime.proto)
│   └── vendor/            ← Vendored GPUI framework (do not modify)
├── assets/
│   ├── fonts/             ← IBM Plex Sans, Lilex
│   └── icons/
├── Cargo.toml             ← Workspace manifest (like package.json for the whole repo)
├── justfile               ← Task runner (like npm scripts)
├── CLAUDE.md              ← AI assistant context
├── MIGRATION_PLAN.md      ← Detailed 15-stage migration roadmap
└── docs/                  ← You are here
```

---

## Key Constants

| Constant | Value |
|----------|-------|
| Default API host | `dev-mezon.nccsoft.vn:8088` |
| Server key (Basic auth) | `defaultkey` |
| Deep link scheme | `mezonapp://` |
| Settings file (macOS/Linux) | `~/.config/mezon/settings.json` |
| Settings file (Windows) | `%APPDATA%\mezon\settings.json` |
| Keychain service name | `mezon-desktop` |
| Rust toolchain | stable 1.94.1 (pinned in `rust-toolchain.toml`) |

---

## Platform Support

| Feature | macOS | Windows | Linux |
|---------|-------|---------|-------|
| System tray | ✓ | ✓ | ✓ |
| Dock/taskbar badge | ✓ (NSDockTile) | ✓ (ITaskbarList3) | — |
| Notifications | ✓ (UNUserNotificationCenter) | ✓ (ToastNotification) | ✓ (notify-rust) |
| Auto-start | ✓ (Login Items) | ✓ (Registry) | ✓ (.desktop) |
| Deep link scheme | ✓ (Info.plist) | ✓ (Registry) | ✓ (xdg-mime) |
| Screen lock detection | ✓ (CFNotificationCenter) | ✓ (WTSRegisterSessionNotification) | stub |
| Single instance lock | ✓ (Unix socket) | ✓ (Named pipe) | ✓ (Unix socket) |

---

## Why GPUI instead of a web-based approach?

| Property | Detail |
|----------|--------|
| Rendering | GPU-accelerated via Metal (macOS) and wgpu (Linux/Windows) |
| Layout | Tailwind-style utility methods (`flex()`, `p_4()`, `text_color()`) |
| Reactivity | Fine-grained — only subscribed views re-render on state change |
| Async | First-class — `cx.spawn()`, `cx.background_executor()` |
| Proof | Powers Zed editor in production — handles rich text, virtual lists, GPU textures |

---

## Backend Transport (Rust vs the old Electron/JS app)

| Layer | Old (Electron/JS) | New (Rust) |
|-------|------------------|------------|
| HTTP REST (login/OTP/refresh) | mezon-js Client | reqwest async client |
| TCP/TLS + Protobuf (app APIs) | WebSocketAdapterPb | AbridgedTcpAdapter + MezonTransport |
| Auth tokens | localStorage | OS keychain (keyring crate) |
| OAuth2 | Browser window | System browser + mezonapp://callback deep link |
| Session refresh | client.onRefreshSession | Background tokio task |
