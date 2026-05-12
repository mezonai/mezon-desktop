# Migration Plan: Electron → Rust/GPUI

This is a 15-stage roadmap for replacing the existing Electron desktop app with a native
Rust app using GPUI. Each stage is independently shippable.

---

## Current Status

> **As of 2026-05-01:** Stage 0 and Stage 1 are **complete**.
> Stage 2 (App Shell + Navigation) is next.

| Stage | Description | Status |
|-------|-------------|--------|
| 0 | Foundation — window, tray, deep links, OS APIs | ✅ Complete |
| 1 | Auth pages — OTP login, password login, session restore | ✅ Complete |
| 2 | App shell + sidebars (clan list, channels, user bar) | 🔜 Next |
| 3 | Settings modal | — |
| 4 | Image viewer window | — |
| 5 | Direct messages + core MessageList | — |
| 6 | Text channels + rich text rendering | — |
| 7 | Rich text message editor (mentions, emoji, markdown) | — |
| 8 | Threads | — |
| 9 | Members sidebar + user profile popover | — |
| 10 | Notifications panel | — |
| 11 | App directory | — |
| 12 | Voice channel (LiveKit audio) | — |
| 13 | Video meeting + screen share | — |
| 14 | AI generation + remaining pages | — |
| 15 | Remove Electron, update CI/CD | — |

**~11 months** for complete migration. **~5 months** (Stages 0–7) for a shippable app
covering all core use cases.

---

## Stage 0 — Foundation (Complete ✅)

**Goal:** A shippable macOS binary. No chat UI yet.

What was built:
- Frameless window (1280×720, min 950×500)
- Custom title bar (macOS hidden traffic lights + drag region)
- System tray (icon + Show / Check for Updates / Quit)
- Single instance lock (Unix socket on macOS/Linux, named pipe on Windows)
- Persistent settings: `~/.config/mezon/settings.json`
- Auto-start on login
- Badge count on dock icon (macOS) / taskbar (Windows)
- Deep link scheme: `mezonapp://` registered on all platforms
- Screen lock/unlock detection
- OAuth2 flow: open system browser → receive callback deep link
- Desktop notifications (macOS, Windows, Linux)
- GitHub Actions CI: macOS arm64/x64, Windows, Ubuntu

---

## Stage 1 — Auth Pages (Complete ✅)

**Goal:** User can sign in. Token persisted across restarts.

What was built:
- `POST /v2/account/authenticate/emailotp` — OTP step 1
- `POST /v2/account/authenticate/confirmotp` — OTP step 2
- `POST /v2/account/authenticate/email` — password login
- `POST /v2/account/session/refresh` — token refresh
- `Session` struct with JWT claim decoding
- OS keychain: store / load / clear session
- Background 60s refresh task (refreshes when <5 min remaining)
- `LoginView`: OTP mode (default) with 60s resend countdown
- `LoginView`: Password mode toggle
- Error labels, loading spinner
- Startup session restore (silent refresh if expired)

---

## Stage 2 — App Shell + Navigation (Next 🔜)

**Goal:** Full sidebar renders with real data. Real-time unread badges.

### API calls needed
- `GET /v2/clans` — user's clan list
- `GET /v2/channels?clan_id=X` — channel tree
- `GET /v2/direct` — DM channel list
- WebSocket: TCP/TLS: abridged protocol over TLS to {tcp_url} port 7349. Protobuf Envelope framing with CID-routed request/response.
- Real-time events: `channel_message`, `channel_presence`, `status_presence`, `notification`

### New state models needed
- `ClansModel` — clan list, active clan
- `ChannelsModel` — channel tree per clan, unread counts
- `DirectModel` — DM channel list
- `PresenceModel` — online/away/offline/dnd per user

### UI to build

```
MainLayout
├── TitleBar (already exists)
│
├── ClanSidebar (72px wide)
│   ├── Direct Messages icon
│   ├── ClanIcon × N (Avatar + unread Badge + tooltip)
│   └── Add Clan / Discover (bottom)
│
├── ChannelSidebar (240px wide)
│   ├── Clan name header + settings gear
│   ├── CategorySection × N (SectionHeader — collapsible)
│   │   └── ChannelRow × N (# text, 🔊 voice, unread bold)
│   ├── DM list (Avatar + Label + Badge)
│   └── UserInfoBar (Avatar + Label + StatusDot + IconButton)
│
└── ContentArea
    └── EmptyState ("Select a channel to start chatting")
```

---

## Stage 3 — Settings Modal

**Goal:** Settings modal opens. Theme toggle + other settings persist.

This is also when `Settings::save()` will finally be called — wiring up persistence
that's already implemented but unused.

```
SettingsModal (overlay, Cmd+,)
├── My Account tab
├── Notifications tab
├── Appearance tab (theme: Dark/Light/Auto, zoom slider)
└── Advanced tab (hardware acceleration, auto-start)
```

---

## Stage 4 — Image Viewer Window

A secondary GPUI window (transparent background) for viewing images/attachments:
- GPU-decoded images (image crate → GPUI texture)
- Zoom, pan, rotate
- Thumbnail strip (virtualized)
- Context menu: Copy Link, Copy Image, Save Image, Open in Browser

---

## Stage 5 — Direct Messages

The core `MessageList` and `MessageInputBar` components — reused in all later stages.

- Paginated message history (cursor-based)
- Real-time receive via WebSocket
- Send messages (optimistic UI with Snowflake temp ID)
- Edit / delete
- Typing indicators
- Virtual scroll (only renders visible messages)

---

## Stage 6 — Text Channels + Rich Text

Extends Stage 5 with:

| Markdown | Renders as |
|----------|-----------|
| `**bold**` | Bold text |
| `` `code` `` | Inline code |
| ` ```lang\ncode\n``` ` | Code block with syntax highlighting |
| `@username` | Mention chip |
| `#channel` | Clickable channel link |
| `:emoji:` | Custom emoji image |
| `https://...` | URL + OGP embed card |

Also adds: pinned messages, message context menu, member list sidebar.

---

## Stage 7 — Rich Text Editor

Upgrades the `MessageInputBar`:
- `@mention` autocomplete dropdown
- `#channel` autocomplete
- `:emoji:` picker popup
- Markdown shortcuts
- Slash commands (`/giphy`, `/me`)
- Paste image → upload → inline preview
- Draft persistence per channel

---

## Stages 8–15 (Summary)

| Stage | Description |
|-------|-------------|
| 8 | Threads (reuses MessageList + MessageInputBar) |
| 9 | Member list sidebar + user profile popover |
| 10 | Notifications panel |
| 11 | App directory |
| 12 | Voice channels (LiveKit Rust SDK + cpal audio) |
| 13 | Video meetings + screen share (LiveKit video + wgpu textures) |
| 14 | AI generation page + remaining minor pages |
| 15 | Delete Electron app, update CI/CD |

---

## IPC Migration: Electron → Rust

Every Electron IPC channel has a direct Rust equivalent:

| Electron IPC | Rust equivalent |
|---|---|
| `APP::SET_BADGE_COUNT` | `mezon_native::badge::set_badge_count(n)` |
| `APP::SHOW_NOTIFICATION` | `mezon_native::notifications::show()` |
| `APP::AUTO_START_APP` | `auto_launch::AutoLaunch::enable/disable()` |
| `APP::SYNC_REDUX_STATE` | `mezon_store::Settings::save/load()` |
| `APP::QUIT_APP` | `gpui::App::quit()` |
| `APP::OPEN_NEW_WINDOW` | `gpui::App::open_window()` |
| `APP::DOWNLOAD_FILE` | `rfd::AsyncFileDialog::save_file()` + `tokio::fs::write()` |
| `APP::TITLE_BAR_ACTION` | `gpui::WindowContext::minimize/maximize/close()` |

---

## Packaging & Distribution

| Platform | Format | Tool |
|----------|--------|------|
| macOS | `.app` bundle | `cargo-bundle` |
| macOS | DMG (universal arm64 + x64) | `create-dmg` |
| macOS | App Store (`id6756601798`) | Transporter |
| Windows | NSIS installer | NSIS scripts |
| Windows | APPX (Microsoft Store) | `makeappx` |
| Linux | `.deb` | `cargo-deb` |
| Linux | AppImage | `appimagetool` |
