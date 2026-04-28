# Mezon Desktop — Rust/GPUI Migration Plan

**Status:** Stage 0 — Complete ✓ | Stage 1 (Auth Pages) — Complete ✓ | Stage 2 (App Shell) — Next
**Target:** Full native desktop app using [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (Zed's GPU-accelerated UI framework) — vendored locally under `crates/vendor/`
**Strategy:** Page-by-page migration — each screen is replaced one at a time with a native GPUI view
**Platform priority:** macOS first, then Windows and Linux
**Parallel development:** New app lives in `apps/desktop-rs/` alongside the existing Electron app in `apps/desktop/`

---

## Why GPUI

| Property   | Detail                                                                           |
| ---------- | -------------------------------------------------------------------------------- |
| Rendering  | GPU-accelerated via Metal (macOS) and wgpu (Linux/Windows)                       |
| Layout     | Tailwind-style utility methods (`flex()`, `p_4()`, `text_color()`)               |
| Reactivity | Fine-grained — `Model<T>` + `cx.notify()` — only subscribed views re-render      |
| Async      | First-class — `cx.spawn()`, `cx.background_executor()`                           |
| Proof      | Powers Zed editor — handles rich text, virtual lists, GPU textures in production |
| Text       | Full Unicode, BiDi, emoji, font fallback, custom inline elements                 |

---

## Architecture

```
apps/desktop-rs/
├── Cargo.toml                    ← workspace root
├── Cargo.lock
├── rust-toolchain.toml           ← pins stable 1.94.1 (vendored GPUI, nightly not required)
├── .cargo/config.toml            ← target-specific linker flags
├── crates/
│   ├── mezon-app/                ← binary entry point, GPUI bootstrap, window management
│   │   ├── src/main.rs           ← app bootstrap, tray, deep link polling, power events
│   │   └── Info.plist            ← macOS bundle metadata + CFBundleURLTypes (mezonapp://)
│   ├── mezon-ui/                 ← all GPUI views (one module per page)
│   ├── mezon-client/             ← Rust equivalent of mezon-js (REST + WebSocket + Protobuf)
│   ├── mezon-store/              ← app state models (Model<T> per domain)
│   ├── mezon-native/             ← OS APIs: tray, badge, notifications, screen capture, activity
│   │   ├── src/autostart.rs      ← login item (auto-launch crate)
│   │   ├── src/badge.rs          ← dock badge (macOS objc) + taskbar overlay (Windows ITaskbarList3)
│   │   ├── src/deep_link.rs      ← mezonapp:// scheme registration (Info.plist / registry / .desktop)
│   │   ├── src/instance.rs       ← single-instance lock (Unix socket / Windows named pipe)
│   │   ├── src/notifications.rs  ← UNUserNotificationCenter / WinRT Toast / notify-rust
│   │   ├── src/power.rs          ← screen lock/unlock (CFNotificationCenter / WTSRegisterSessionNotification)
│   │   └── src/tray.rs           ← system tray icon + menu (tray-icon crate)
│   ├── mezon-updater/            ← auto-update (polls cdn.mezon.ai/release/)
│   ├── mezon-proto/              ← generated Protobuf types via prost-build
│   └── vendor/                   ← vendored Zed/GPUI crates (cloned from zed-industries/zed)
│       ├── gpui/                 ← main GPUI framework (Metal + wgpu renderer, layout, views)
│       ├── gpui_macros/          ← derive macros: Action, IntoElement, Render, etc.
│       ├── gpui_shared_string/   ← SharedString (Arc<str> wrapper)
│       ├── gpui_util/            ← ArcCow + utility helpers
│       ├── gpui_platform/        ← platform abstraction, exports application()
│       ├── gpui_macos/           ← macOS Metal renderer backend
│       ├── collections/          ← VecMap<K,V>
│       ├── refineable/           ← Refineable trait for cascading style structs
│       ├── derive_refineable/    ← derive macro for Refineable
│       ├── scheduler/            ← async task scheduler with test clock
│       ├── sum_tree/             ← B-tree with cursor (used in text system)
│       ├── util/                 ← fs, paths, markdown, archive, shell helpers
│       ├── util_macros/          ← derive macros for util types
│       ├── http_client/          ← HTTP client abstraction (wraps reqwest)
│       ├── http_client_tls/      ← TLS config for http_client
│       ├── reqwest_client/       ← reqwest-backed http_client impl
│       ├── media/                ← C FFI bindings for audio/video
│       ├── ztracing/             ← tracing integration
│       ├── ztracing_macro/       ← tracing derive macros
│       ├── zlog/                 ← logging sink with env config
│       └── perf/                 ← performance benchmarking utilities
└── assets/
    ├── fonts/                    ← IBM Plex Sans, Lilex
    ├── icons/                    ← app.icns, app.ico, trayicon-linux.png  [TODO]
    └── sounds/                   ← notification sounds  [TODO]
```

### GPUI Data Flow

```
WebSocket (mezon API — Protobuf frames)
        │
        ▼
  tokio task (background executor)
        │
   cx.update() ──► MessagesModel.update()
                          │
                    cx.notify() ──► MessageList view re-renders
                                         │
                                   Element Tree (div/flex/text)
                                         │
                                   Taffy layout pass (Flexbox)
                                         │
                                   GPU draw calls
                                         │
                                  Metal (macOS) / wgpu (Linux/Windows)
                                         │
                                      Display
```

### Backend Transport (Rust equivalent of mezon-js)

| Layer                | Electron/JS               | Rust                                             |
| -------------------- | ------------------------- | ------------------------------------------------ |
| HTTP REST            | `mezon-js` Client         | `reqwest` async client (Zed fork)                |
| WebSocket + Protobuf | `WebSocketAdapterPb`      | `tokio-tungstenite` + `prost`                    |
| Auth tokens          | `localStorage`            | `keyring` crate (OS keychain)                    |
| OAuth2               | Browser window            | System browser + `mezonapp://callback` deep link |
| Session refresh      | `client.onRefreshSession` | Background `tokio` task                          |

API host is dynamically returned in the Session object after auth. Default: `sock.mezon.ai`.

### Crate Dependency Map

```
mezon-app
  ├── mezon-ui        (GPUI views)
  │     └── mezon-store   (Model<T> state)
  ├── mezon-client    (REST + WebSocket)
  │     └── mezon-proto   (Protobuf types)
  ├── mezon-native    (OS APIs)
  └── mezon-updater   (auto-update)
```

---

## Current Component Inventory

> Implemented as of Stage 0. All components live in `crates/mezon-ui/src/`.

### Theme (`src/theme.rs`)

-   `Theme::dark()` and `Theme::light()` — 22 color tokens covering backgrounds, text, brand, status, unread/mention, border, title bar.

### Views

| View       | Location           | Status                                                                                                                                   |
| ---------- | ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `TitleBar` | `src/title_bar.rs` | Frameless, drag region, macOS traffic lights hidden off-screen, platform-conditional window controls (minimize/zoom/close) for non-macOS |
| `RootView` | `src/root.rs`      | Top-level view; switches content area on `AuthState` (NotAuthenticated / AwaitingCallback / Authenticated)                               |

### Primitive Components (`src/components/primitives/`)

| Component   | Key Features                                                                                                                      |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `Avatar`    | `AvatarSize` (Xs–Xl2), `PresenceStatus` (Online/Idle/Dnd/Offline), URL image or initials fallback, presence dot                   |
| `Badge`     | Count pill, mention variant, caps at "99+", auto-hides at 0                                                                       |
| `Button`    | `ButtonVariant` (Primary/Secondary/Ghost/Danger), `ButtonSize` (Xs–Lg), disabled/loading states, leading icon, `on_click` handler |
| `Divider`   | Horizontal rule, optional centered label                                                                                          |
| `Icon`      | 32 named icons (`IconName` enum), inline SVG via `gpui::svg()`, size + color control                                              |
| `Label`     | `LabelSize` (Xs–Xl2), `LabelWeight` (Normal/Medium/SemiBold/Bold), secondary/muted helpers                                        |
| `Spinner`   | Animated SVG rotation via `gpui::Animation` (700ms, repeat)                                                                       |
| `TextInput` | Stateful view, `FocusHandle`, placeholder, label, error, masked/password mode, disabled, caret rendering                          |

### Composition Components (`src/components/compositions/`)

| Component       | Key Features                                                                                         |
| --------------- | ---------------------------------------------------------------------------------------------------- |
| `EmptyState`    | Icon + title + subtitle + optional action `Button`                                                   |
| `FormField`     | Owns `Entity<TextInput>`, uppercase label header, `set_masked()` / `set_on_change()` / `set_error()` |
| `IconButton`    | Square button with `Icon`, variant + size, tooltip field (display pending)                           |
| `SectionHeader` | Collapsible sidebar category header, chevron toggle, `on_toggle` callback, trailing action button    |
| `StatusDot`     | Presence indicator dot, derives color from `PresenceStatus`                                          |
| `UserChip`      | Avatar + username label inline                                                                       |

---

## Key Dependencies

```toml
# UI framework — vendored locally from zed-industries/zed
gpui               = { path = "crates/vendor/gpui", default-features = false }
gpui_platform      = { path = "crates/vendor/gpui_platform", default-features = false }
gpui_macos         = { path = "crates/vendor/gpui_macos", default-features = false }
# (+ gpui_macros, gpui_shared_string, gpui_util, collections, refineable, scheduler, sum_tree, util, ...)

# Async runtime
tokio = { version = "1", features = ["full"] }
futures = "0.3"
smol = "2.0"

# Networking — Zed forks
reqwest = { git = "https://github.com/zed-industries/reqwest.git", rev = "c15662463bda39148ba154100dd44d3fba5873a4", package = "zed-reqwest", version = "0.12.15-zed" }
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
http = "1.0"

# Protobuf
prost = "0.13"
# prost-build = "0.13"  ← added in mezon-proto/build.rs when .proto files are vendored

# Serialization
serde = { version = "1", features = ["derive", "rc"] }
serde_json = { version = "1", features = ["preserve_order", "raw_value"] }

# Native OS
tray-icon = "0.19"       # system tray (active)
rfd = "0.15"             # native file dialogs  [Stage 4]
arboard = "3"            # clipboard  [Stage 4]
scap = { git = "https://github.com/zed-industries/scap", package = "zed-scap" }  # screen capture  [Stage 13]
open = "5"               # open URLs in system browser
auto-launch = "0.5"      # login item / startup registration (active)
keyring = "3"            # OS keychain (auth tokens)  [Stage 1]

# Image & media
image = "0.25.1"         # PNG/WebP/JPEG decoding

# Graphics
wgpu = { version = "24", features = ["wgsl"] }
metal = "0.33"           # macOS Metal bindings

# Voice/video  [Stage 12+]
# livekit = { git = "https://github.com/livekit/rust-sdks" }
# cpal = "0.15"

# Syntax highlighting  [Stage 6]
# syntect = "5"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# macOS-specific
[target.'cfg(target_os = "macos")'.dependencies]
objc = "0.2"
objc2-app-kit = { version = "0.3", default-features = false }
cocoa = "=0.26.0"
core-foundation = "=0.10.0"
metal = "0.33"

# Windows-specific
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    "Win32_UI_Shell", "Win32_UI_WindowsAndMessaging",
    "Win32_Foundation", "Win32_System_Threading",
    "Win32_System_RemoteDesktop",     # screen lock/unlock
    "Win32_Storage_FileSystem",       # named pipe single-instance
    "Win32_System_Pipes",
    "Win32_Graphics_Gdi",             # badge HICON generation
    "Win32_System_Com",               # ITaskbarList3
    "Win32_System_Registry",          # deep link scheme registration
    "Data_Xml_Dom",                   # toast notification XML
    "UI_Notifications",               # ToastNotification
] }

# Linux-specific
[target.'cfg(not(any(target_os = "macos", target_os = "windows")))'.dependencies]
notify-rust = "4"   # desktop notifications via D-Bus
```

---

## Migration Stages

### Stage 0 — Foundation (Weeks 1–3)

**Goal:** Shippable macOS binary replacing Electron. No chat UI yet.

**Tasks:**

-   [x] Scaffold `apps/desktop-rs/` Cargo workspace with all crates
-   [x] Vendor GPUI from Zed repo into `crates/vendor/` (stable toolchain, no nightly required)
-   [x] Open frameless window (1280×720, min 950×500, background `#313338`)
-   [x] Custom title bar: macOS hidden traffic lights + drag region + app menu bar
-   [x] System tray: icon + "Show Mezon" / "Check for Updates" / "Quit"
-   [x] Single instance lock (Unix socket on macOS/Linux)
-   [x] Persistent settings: `~/.config/mezon/settings.json` (autoStart, hardwareAcceleration, windowBounds, zoomFactor)
-   [x] Auto-start on login (`auto-launch` crate, wired to settings)
-   [x] Badge count on dock icon (`objc` on macOS)
-   [x] Deep link: register `mezonapp://` scheme (macOS `Info.plist` + `CFBundleURLTypes`, Windows registry self-registration via `HKCU\Software\Classes\mezonapp`, Linux `.desktop` + `xdg-mime`)
-   [x] Single instance lock (Windows named pipe `\\.\pipe\mezon-single-instance` with `FILE_FLAG_FIRST_PIPE_INSTANCE`)
-   [x] Badge count on taskbar icon (Windows `ITaskbarList3::SetOverlayIcon` with dynamically generated `HICON`)
-   [x] Screen lock/unlock detection (macOS `CFNotificationCenter` + `CFRunLoopRun` thread; Windows `WTSRegisterSessionNotification` + message-only `HWND`)
-   [x] OAuth2 flow: wire sign-in button → open system browser → receive `mezonapp://callback` deep link → set `AwaitingCallback` (token parsing deferred to Stage 1)
-   [x] Desktop notifications delivery (macOS `UNUserNotificationCenter` via `objc` runtime; Windows WinRT `ToastNotification`; Linux `notify-rust` / D-Bus)
-   [x] Tray "Check for Updates" → calls `mezon_updater::check_for_updates()` via tokio spawn; opens download URL on update available
-   [x] GitHub Actions build matrix: `macos-latest` (arm64), `macos-13` (x64), `windows-latest`, `ubuntu-latest` — stable toolchain, clippy, fmt, correct system deps

**Deliverable:** DMG installer. Opens native frameless window. OAuth login works. System tray works. ✓ **Stage 0 complete.**

**Validation spikes (must pass before proceeding):**

1. Frameless window + custom title bar renders correctly on macOS 13+ and Windows 11
2. Virtual scroll list of 10,000 items renders at 60fps
3. PNG/WebP image decoded and rendered as GPUI texture
4. System tray + context menu works on all 3 platforms
5. `mezonapp://` deep link is received and parsed correctly

---

### Stage 1 — Auth Pages (Weeks 4–5)

**Pages:** `/desktop/login`, `/login/callback`, `/logout/callback`

**`mezon-client` work:**

-   [x] `POST /v2/account/authenticate/email` — email/password login (`MezonClient::authenticate_email`)
-   [x] `POST /v2/account/authenticate/emailotp` — OTP step 1 (`MezonClient::request_otp`)
-   [x] `POST /v2/account/authenticate/confirmotp` — OTP step 2 (`MezonClient::confirm_otp`)
-   [x] `POST /v2/account/session/refresh` — token refresh (`MezonClient::refresh_session`)
-   [x] Session struct: `token`, `refresh_token`, `expires_at`, `ws_url`, `api_url` — JWT claims decoded (`uid`, `usn`, `exp`)
-   [x] Keychain: store/load/clear session via `keyring` crate (`mezon-client/src/keychain.rs`)
-   [x] Background refresh task: re-authenticates before expiry (60s polling, refreshes when <5min remaining)

**GPUI views:**

-   [x] `LoginView` — OTP mode (default): email → OTP code entry (6 digits, auto-submit), 60s countdown + resend
-   [x] `LoginView` — Password mode: email + password form, "Forgot password" link
-   [x] Toggle link: "Login by Password" ↔ "Login by OTP"
-   [x] Error label, loading Spinner on in-flight requests
-   [x] Wired into `RootView` as `Entity<LoginView>`

**Startup session restore:**

-   [x] On launch: load session from keychain → valid → `Authenticated`, expired → try silent refresh → else `NotAuthenticated`

**Deliverable:** User can sign in via OTP or email+password. Token persisted to keychain. App remembers login across restarts. ✓ **Stage 1 complete.**

---

### Stage 2 — App Shell + Navigation (Weeks 6–8)

**Pages:** `/chat/` shell — `MainLayout` + sidebars

The permanent frame that wraps all authenticated pages.

**`mezon-client` work:**

-   `GET /v2/clans` — fetch user's clan list with icons and unread counts
-   `GET /v2/channels?clan_id=X` — fetch channel tree (categories + channels)
-   `GET /v2/direct` — fetch DM channel list
-   WebSocket connect: `wss://{ws_url}/ws?token=...` with Protobuf framing
-   Subscribe real-time: `channel_message`, `channel_presence`, `status_presence`, `notification`

**`mezon-store` models:**

-   `ClansModel` — clan list, currently active clan
-   `ChannelsModel` — channel tree per clan, unread counts
-   `DirectModel` — DM channels list
-   `PresenceModel` — online/away/offline/dnd per user

**GPUI views:**

```
MainLayout
├── TitleBar (from Stage 0)
│
├── ClanSidebar (72px wide)
│   ├── Direct Messages icon (top)
│   ├── ClanIcon × N (virtual list — Avatar component, unread Badge, tooltip on hover)
│   ├── Separator (Divider component)
│   └── Add Clan / Discover (bottom)
│
├── ChannelSidebar (240px wide)
│   ├── Clan name header + settings gear icon (IconButton component)
│   ├── CategorySection × N (SectionHeader component — collapsible)
│   │   └── ChannelRow × N (# text, 🔊 voice, unread bold, lock icon)
│   ├── DM list (when in DM mode — Avatar, Label, Badge components)
│   └── UserInfoBar (Avatar, Label, StatusDot, IconButton components)
│
└── ContentArea (flex-1)
    └── Placeholder: "Select a channel to start chatting" (EmptyState component)
```

**Note:** Tray "Show Mezon" stub (see `mezon-app/src/main.rs:187`) is completed here by storing the `WindowHandle` and calling `window.activate()`.

**Deliverable:** Full sidebar renders with real data. Switching clans updates channel list. Unread badges update in real-time via WebSocket. Content area still shows placeholder.

---

### Stage 3 — Settings (Weeks 9–10)

**Route:** `/chat/clans/:clanId/channel-setting` + user settings modal

Self-contained, no real-time data — ideal first native page.

**GPUI views:**

```
SettingsModal (overlay — Cmd+,  or settings gear)
├── TabBar
│   ├── My Account
│   ├── Privacy & Safety
│   ├── Notifications
│   ├── Appearance
│   ├── Voice & Video
│   └── Advanced
│
├── My Account tab
│   ├── Avatar + username + email (Avatar, Label components)
│   └── Edit profile button (Button component) → API call
│
├── Notifications tab
│   ├── Enable/disable desktop notifications toggle
│   ├── Notification sound toggle
│   └── Per-channel overrides list
│
├── Appearance tab
│   ├── Theme: Dark / Light / Auto (follows system)
│   └── Zoom: slider 80%–150% → GPUI global scale factor
│
└── Advanced tab
    ├── Hardware acceleration toggle (requires restart prompt)
    └── Auto-start on login toggle
```

**Deliverable:** Settings modal opens/closes (`Cmd+,`). Theme toggle applies instantly. Advanced settings persist.

---

### Stage 4 — Image Viewer Window (Weeks 11–12)

**Replaces:** `apps/desktop/src/assets/image-window/window_image.ts` (1500-line inline HTML popup)

A **separate secondary GPUI Window** — transparent background, popup style.

**GPUI views:**

```
ImageViewerWindow (secondary Window — transparent bg)
├── TitleBar (minimize, toggle-maximize, close)
│
├── MainImageArea
│   ├── ImageElement — GPU-decoded (image crate → GPUI texture)
│   │   ├── Mouse wheel → zoom in/out
│   │   ├── Click + drag → pan
│   │   └── Double-click → fit to window
│   ├── VideoElement (for .mp4/.webm attachments)
│   ├── Zoom controls (IconButton components)
│   └── Rotate controls (IconButton components)
│
├── ThumbnailStrip (bottom, horizontally scrollable, virtualized)
│   ├── ThumbnailItem × N (only visible range rendered — binary search)
│   ├── Selected item: highlighted border
│   └── Load-more trigger at 5 items from edge → fetch next batch
│
└── ContextMenu (right-click)
    ├── Copy Link
    ├── Copy Image → arboard clipboard
    ├── Save Image → rfd save dialog
    └── Open in Browser → open crate
```

**Keyboard shortcuts:** `Esc` close, `←/→` prev/next image, `+/-` zoom, `R` rotate

**imgproxy URL:** `https://imgproxy.mezon.ai/{key}/rs:{type}:{w}:{h}:1/mb:2097152/plain/{url}@webp`

**Deliverable:** Clicking any image in the app opens the native GPUI image viewer. Zoom, rotate, thumbnail navigation all work natively.

---

### Stage 5 — Direct Messages (Weeks 13–16)

**Route:** `/chat/direct/message/:directId/:type`

Core messaging component — establishes the reusable `MessageList` and `MessageInputBar` used by all subsequent stages.

**`mezon-client` work:**

-   `GET /v2/channels/:id/messages?limit=50&cursor=X` — paginated history (cursor-based)
-   WebSocket event: `channel_message` → append to `MessagesModel`
-   `POST /v2/channels/:id/messages` — send message (with temp Snowflake ID for optimistic UI)
-   `PUT /v2/channels/:id/messages/:msgId` — edit
-   `DELETE /v2/channels/:id/messages/:msgId` — delete
-   `PUT /v2/channels/:id/seen` — mark as read
-   WebSocket send: `channel_typing` → update `TypingModel`
-   WebSocket receive: `channel_typing` → show typing indicator

**Message data model (from Redux store analysis):**

```rust
pub struct Message {
    pub id: String,                    // Snowflake ID
    pub channel_id: String,
    pub sender_id: String,
    pub username: String,
    pub content: MessageContent,       // { t: String } + rich content
    pub attachments: Vec<Attachment>,
    pub mentions: Vec<Mention>,
    pub reactions: Vec<Reaction>,
    pub references: Vec<MessageRef>,   // replies
    pub create_time_seconds: i64,
    pub update_time_seconds: Option<i64>,
    pub code: MessageCode,             // FIRST_MESSAGE, etc.
    // UI computed fields:
    pub is_group_start: bool,          // first in consecutive group from same sender
    pub is_day_start: bool,            // first message of the day
}
```

**GPUI views:**

```
DirectMessageView
├── DMHeader
│   ├── Avatar + Label (recipient username)
│   └── "This is the beginning of your direct message history with @{user}"
│
├── MessageList (virtual scroll — core reusable component)
│   ├── Only renders messages in viewport + 20-item buffer above/below
│   ├── Variable row heights — measured on first render, cached
│   ├── Scroll-to-bottom on new message (if already at bottom)
│   ├── "New Messages" separator line (last-seen boundary)
│   ├── "Jump to Present" Button (when scrolled far up)
│   │
│   ├── DateSeparator ("Today" / "Yesterday" / "April 21, 2025") — Divider + Label
│   │
│   └── MessageGroup (consecutive messages, same sender, <5 min apart)
│       ├── Avatar (left, shown once per group)
│       ├── Label (sender name + timestamp, shown once per group)
│       └── MessageRow × N
│           ├── TextContent (plain text — markdown in Stage 6)
│           ├── AttachmentGrid (images → click opens ImageViewer)
│           ├── ReactionBar (emoji + Badge count + click to react)
│           ├── HoverActions (visible on mouse hover — IconButton components)
│           │   ├── Reply
│           │   ├── React (emoji picker)
│           │   ├── Edit (own messages)
│           │   └── Delete (own messages)
│           └── Label "(edited)"
│
├── TypingIndicator
│   └── "{user} is typing..." / "{user1} and {user2} are typing..."
│
└── MessageInputBar
    ├── TextInput (multi-line, Shift+Enter = newline, Enter = send)
    ├── Attachment IconButton → rfd file picker → upload → show inline preview
    ├── Emoji IconButton → EmojiPicker panel (basic grid)
    └── Send Button
```

**Deliverable:** Full DM conversations work. Send/receive real-time. Images open ImageViewer. Basic text. Typing indicators.

---

### Stage 6 — Text Channel (Weeks 17–22)

**Route:** `/chat/clans/:clanId/channels/:channelId`

Extends the message components from Stage 5 with channel-specific features and rich text rendering.

**Additional `mezon-client` work:**

-   Reaction add/remove: `PUT /v2/messages/:id/reactions`
-   Pin messages: `GET /v2/channels/:id/pinned`, `PUT /v2/messages/:id/pin`
-   Thread creation: `POST /v2/threads`
-   Member list: `GET /v2/channels/:id/members`

**Rich text rendering (extends Stage 5 plain text):**

| Syntax                 | Render                                            |
| ---------------------- | ------------------------------------------------- |
| `**bold**`             | Bold weight                                       |
| `*italic*`             | Italic style                                      |
| `~~strikethrough~~`    | Strikethrough                                     |
| `` `code` ``           | Inline code (monospace, bg highlight)             |
| ` ```lang\ncode\n``` ` | Code block with syntax highlighting (`syntect`)   |
| `> quote`              | Blockquote with left border                       |
| `@username`            | Mention chip (UserChip component, highlighted bg) |
| `#channel-name`        | Channel link (clickable → navigate)               |
| `:emoji_name:`         | Custom emoji image inline                         |
| `https://...`          | URL underlined + OGP embed card below             |

**Dependencies added this stage:**

```toml
syntect = "5"   # syntax highlighting for code blocks
```

**Additional GPUI views:**

-   `PinnedMessagesPanel` — side panel listing pinned messages
-   `MessageContextMenu` — right-click: Reply, Edit, Delete, Pin, Copy ID, Report
-   `SystemMessageRow` — "Alice joined the clan", "Channel created", etc.
-   `UnreadSeparator` — "NEW MESSAGES ──────────────────"
-   `MemberListSidebar` — right panel, grouped by role, virtual list (toggle with `Cmd+Shift+M`)

**Deliverable:** Full channel messaging. Rich text renders. Reactions, pins, context menu functional. Unread badges accurate.

---

### Stage 7 — Rich Text Message Editor (Weeks 23–26)

**Upgrades:** `MessageInputBar` from Stages 5 & 6

The most complex single component. Starting point: adapt Zed's `editor` crate.

**Features:**

-   [ ] Multi-line editor with Shift+Enter newline, Enter send
-   [ ] `@mention` autocomplete dropdown (fuzzy-search clan members)
-   [ ] `#channel` autocomplete dropdown
-   [ ] `:emoji:` autocomplete with emoji picker popup
-   [ ] Markdown shortcuts: type `**` → bold mode, `` ` `` → code mode, etc.
-   [ ] Slash commands: `/giphy`, `/me`, `/shrug`
-   [ ] Paste image → upload to CDN → inline preview in editor
-   [ ] Drag-and-drop file → attachment preview
-   [ ] Press `↑` to edit last sent message
-   [ ] Draft persistence per channel (saved to `mezon-store`)
-   [ ] Character limit indicator (2000 chars) — Label component
-   [ ] Send loading state (optimistic UI with Snowflake temp ID)

**Deliverable:** Full-featured message editor matching current React app behavior.

---

### Stage 8 — Threads (Weeks 27–28)

**Route:** `/chat/clans/:clanId/channels/:channelId/threads/:threadId`

Thread panel slides in as a right overlay alongside the main message list. Reuses `MessageList` and `MessageInputBar` from Stage 6/7.

**Additional state:**

-   `ThreadsModel` — thread metadata, parent message reference
-   Thread header: parent message preview + "Thread" title
-   Thread list in channel: inline thread summary below parent message

---

### Stage 9 — Members + User Profiles (Weeks 29–30)

**Route:** `/chat/clans/:clanId/member-safety`

**GPUI views:**

```
MemberSidebar (right panel, toggled)
├── TextInput (search members)
├── SectionHeader ("ONLINE — 12", "OFFLINE — 48")
└── MemberRow × N (virtual list)
    ├── Avatar + StatusDot + Label (username)
    └── Click → UserProfilePopover

UserProfilePopover (floating panel)
├── Banner + Avatar
├── Label (username + discriminator)
├── Label ("Playing {game}" / "Listening to {song}")
├── Badge × N (role badges)
├── TextInput (note field, editable)
└── Button × 2 (Send Message, Call)
```

---

### Stage 10 — Notifications Panel (Week 31)

**GPUI views:**

```
NotificationPanel (dropdown from bell IconButton in TitleBar)
├── Button "Mark all as read"
├── Tab filter: All / Mentions / DMs
└── NotificationRow × N
    ├── Avatar + Label (channel context)
    ├── Label (message preview, truncated)
    ├── Label (timestamp)
    └── Click → navigate to message (triggers jump-scroll in MessageList)
```

---

### Stage 11 — App Directory (Week 32)

**Route:** `/apps`

```
AppDirectoryView
├── TextInput (search)
├── Tab (category filter)
└── AppCard grid × N
    ├── Avatar (app icon) + Label (name + description)
    ├── Button (Install / Uninstall)
    └── Click → AppDetailModal
```

---

### Stage 12 — Voice Channel (Weeks 33–36)

**Partial voice — no video yet**

**Dependencies added this stage:**

```toml
livekit = { git = "https://github.com/livekit/rust-sdks" }
cpal = "0.15"    # cross-platform audio I/O
```

**`mezon-client` work:**

-   LiveKit Rust SDK: `livekit` crate
-   Join voice channel → `Room::connect(url, token)`
-   Audio I/O: `cpal` crate — enumerate devices, capture mic, play remote audio
-   Publish local audio track → LiveKit room
-   Subscribe to remote audio tracks → play via `cpal`

**GPUI views:**

```
VoiceChannelBar (bottom overlay, shown when in a voice channel)
├── Label (channel name + clan name)
├── Avatar × N (participant avatars, speaking ring animation)
├── IconButton (Mute/Unmute)
├── IconButton (Deafen/Undeafen)
└── Button (Disconnect)

VoiceChannelView (in ContentArea when viewing a voice channel)
├── Participant tiles × N
│   ├── Avatar + speaking indicator (animated ring)
│   └── Label (username) + StatusDot (muted indicator)
└── Button "Join Voice" (if not yet connected)
```

**Deliverable:** Voice calls work. Audio in/out. Mute/deafen. Speaking indicators animated.

---

### Stage 13 — Video Meeting + Screen Share (Weeks 37–42)

**Routes:** `/meet`, `/meeting/:code`

**Technical challenge:** LiveKit delivers raw YUV/RGB video frames. These must be uploaded as `wgpu::Texture` each frame and rendered via a custom GPUI element.

**`mezon-client` work:**

-   Camera capture: platform APIs via `objc` (macOS) / `windows-rs` (Windows)
-   Screen capture: `scap` crate (Zed fork) → raw frames → encode with `openh264` → publish as LiveKit track
-   Video subscribe: receive remote video tracks → YUV frames → `wgpu::Texture` → GPUI `VideoElement`

**GPUI views:**

```
MeetingView
├── VideoGrid (responsive — 1, 2×2, 3×3 based on participant count)
│   └── VideoTile × N
│       ├── VideoElement (custom GPUI element wrapping wgpu texture)
│       ├── Label (participant name overlay)
│       └── speaking indicator border
│
├── Spotlight mode (pin a participant to full-screen)
│
└── ControlBar (bottom)
    ├── IconButton (Mute/Unmute)
    ├── IconButton (Camera on/off)
    ├── Button (Share Screen → scap source picker)
    ├── Button (Participants → side panel)
    └── Button (End Call / Leave)

ScreenPickerModal
├── SectionHeader "Entire Screen"
└── SectionHeader "Application Window"
    └── SourceCard × N (thumbnail + Label)
```

---

### Stage 14 — AI Generation + Remaining Pages (Weeks 43–44)

**Routes:** `/aigeneration`, `/integrations`, `/organize`, `/customize`

-   **AI Generation:** Streaming text output display (SSE or WebSocket chunks → append to GPUI text view)
-   **Integrations:** Webhook management forms (`FormField`, `Button` components)
-   **Organize:** Clan management forms
-   **Customize:** Theme/branding upload forms
-   **Public pages** (`/about`, `/privacy-policy`, `/terms-of-service`, `/brand-center`): Static text + images — low effort

---

### Stage 15 — Remove Electron (Week 45)

-   [ ] Delete `apps/desktop/` directory
-   [ ] Remove Electron dependencies from root `package.json`
-   [ ] Update Nx workspace config (`nx.json`, `workspace.json`) to remove desktop target
-   [ ] Update CI/CD GitHub Actions workflows to build only `apps/desktop-rs/`
-   [ ] Update App Store connect listing: new binary, same App ID `E9Y2J54ZH3.app.mezon.ai`
-   [ ] Update Windows Store listing: `MEZON.Mezon_vdgv9gtrfadw6!MEZON.Mezon`
-   [ ] Update auto-update manifest on `cdn.mezon.ai/release/` to point to new binary

---

## Packaging & Distribution

| Platform | Format                      | Tool                                           |
| -------- | --------------------------- | ---------------------------------------------- |
| macOS    | `.app` bundle               | `cargo-bundle`                                 |
| macOS    | DMG (x64 + arm64 universal) | `create-dmg`                                   |
| macOS    | Notarization                | `xcrun notarytool` (Apple team `E9Y2J54ZH3`)   |
| macOS    | App Store (`id6756601798`)  | Transporter                                    |
| Windows  | NSIS installer              | NSIS scripts                                   |
| Windows  | Portable EXE                | —                                              |
| Windows  | APPX (Microsoft Store)      | `makeappx` (Store ID: `MEZON.Mezon`)           |
| Linux    | `.deb`                      | `cargo-deb`                                    |
| Linux    | AppImage                    | `appimagetool`                                 |
| Linux    | MIME handler                | `x-scheme-handler/mezonapp` in `.desktop` file |

---

## Timeline Summary

> **Current progress (as of 2026-04-24):** Stage 0 and Stage 1 are **complete**. All auth tasks are implemented and compile cleanly:
> OTP login (two-step: email OTP request + confirm), email+password login, JWT claim decoding (uid/usn/exp),
> OS keychain session persistence (`keyring` crate), startup silent session restore + token refresh,
> background 60-second refresh task, `LoginView` with OTP/password toggle, error display, loading spinner,
> 60-second OTP resend countdown, `RootView` refactored to use `Entity<LoginView>`.
> **Stage 2 (App Shell + Navigation) is next.**

| Stage | Description                                              | Duration | Cumulative |
| ----- | -------------------------------------------------------- | -------- | ---------- |
| 0     | Foundation — app shell, tray, deep links, auth flow      | 3 weeks  | Week 3     |
| 1     | Auth pages — login, OAuth callback                       | 2 weeks  | Week 5     |
| 2     | App shell + sidebars (clan list, channel list, user bar) | 3 weeks  | Week 8     |
| 3     | Settings modal                                           | 2 weeks  | Week 10    |
| 4     | Image viewer window                                      | 2 weeks  | Week 12    |
| 5     | Direct messages + core MessageList + MessageInputBar     | 4 weeks  | Week 16    |
| 6     | Text channel + rich text rendering                       | 6 weeks  | Week 22    |
| 7     | Rich text message editor (mentions, emoji, markdown)     | 4 weeks  | Week 26    |
| 8     | Threads                                                  | 2 weeks  | Week 28    |
| 9     | Members sidebar + user profile popover                   | 2 weeks  | Week 30    |
| 10    | Notifications panel                                      | 1 week   | Week 31    |
| 11    | App directory                                            | 1 week   | Week 32    |
| 12    | Voice channel (LiveKit audio)                            | 4 weeks  | Week 36    |
| 13    | Video meeting + screen share                             | 6 weeks  | Week 42    |
| 14    | AI generation + remaining pages                          | 2 weeks  | Week 44    |
| 15    | Remove Electron, update CI/CD                            | 1 week   | Week 45    |

**~11 months** for complete migration with a dedicated team.
**~5 months** (Stages 0–7) for a fully shippable app covering all core use cases.

---

## IPC Surface Migration (Electron → Native Rust)

Every `ipcMain.handle` / `ipcRenderer.invoke` in the Electron app is replaced by direct Rust function calls.

| Electron IPC Channel                        | Rust equivalent                                                       |
| ------------------------------------------- | --------------------------------------------------------------------- |
| `APP::GET_DEVICE_ID`                        | `mezon_native::device::get_device_id()`                               |
| `APP::SET_BADGE_COUNT`                      | `mezon_native::badge::set_badge_count(n)`                             |
| `APP::DOWNLOAD_FILE`                        | `rfd::AsyncFileDialog::save_file()` + `tokio::fs::write()`            |
| `APP::OPEN_NEW_WINDOW`                      | `gpui::App::open_window()` → `ImageViewerWindow`                      |
| `APP::TITLE_BAR_ACTION`                     | `gpui::WindowContext::minimize/maximize/close()`                      |
| `APP::MAC_WINDOWS_ACTION`                   | `gpui::WindowContext::set_bounds()`                                   |
| `APP::REQUEST_PERMISSION_SCREEN`            | `scap::get_sources()` batched                                         |
| `APP::ACTION_SHOW_IMAGE`                    | `arboard::Clipboard::set_image()` / `rfd::save()`                     |
| `APP::SHOW_NOTIFICATION`                    | `mezon_native::notifications::show()`                                 |
| `APP::LOCK_SCREEN` / `UNLOCK_SCREEN`        | `mezon_native::power::subscribe_lock_events()`                        |
| `APP::UPDATE_ACTIVITY_TRACKING`             | `mezon_native::activity::start_polling()`                             |
| `APP::AUTO_START_APP`                       | `auto_launch::AutoLaunch::enable/disable()`                           |
| `APP::TOGGLE_HARDWARE_ACCELERATION`         | Restart with `--disable-gpu` flag equivalent                          |
| `APP::SYNC_REDUX_STATE` / `GET_REDUX_STATE` | `mezon_store::Settings::save/load()`                                  |
| `APP::QUIT_APP`                             | `gpui::App::quit()`                                                   |
| `PUSH_RECEIVER:::*`                         | `mezon_native::fcm::start_receiver()` — HTTP v1 FCM API via `reqwest` |

---

## Risks & Mitigations

| Risk                                          | Impact | Mitigation                                                                                                |
| --------------------------------------------- | ------ | --------------------------------------------------------------------------------------------------------- |
| GPUI API breaks on Zed update                 | High   | GPUI is vendored locally — upgrade deliberately by re-vendoring from upstream during low-activity periods |
| Windows GPUI support not fully mature         | High   | Run Spike 1 on Windows in Stage 0 before committing; macOS is primary target                              |
| Rich text editor complexity                   | High   | Start with plain text (Stage 5); add features incrementally each stage; adapt Zed's `editor` crate        |
| Video frame rendering (Stage 13)              | High   | Defer to last stage; voice-only (Stage 12) is independently shippable                                     |
| LiveKit Rust SDK maturity                     | Medium | Evaluate SDK in Stage 12 spike; keep fallback plan of `wry` WebView for video-only                        |
| `mezon-proto` Protobuf types                  | Medium | Generate with `prost-build` from the same `.proto` files the server uses                                  |
| `mezon-client` feature parity with `mezon-js` | Medium | Map API calls one-by-one from the Redux slice thunks in `libs/store/`                                     |
| macOS notarization / code signing             | Low    | Set up early in Stage 0 CI; use same Apple Team ID `E9Y2J54ZH3`                                           |

---

## References

-   GPUI source (vendored): `crates/vendor/gpui/` — upstream: https://github.com/zed-industries/zed/tree/main/crates/gpui
-   GPUI examples: https://github.com/zed-industries/zed/tree/main/crates/gpui/examples
-   Existing Electron app: `apps/desktop/src/`
-   Existing React app pages: `apps/chat/src/app/pages/`
-   Existing Redux store: `libs/store/src/lib/`
-   Existing transport layer: `libs/transport/src/lib/`
-   LiveKit Rust SDK: https://github.com/livekit/rust-sdks
