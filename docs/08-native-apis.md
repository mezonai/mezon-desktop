# Native Platform APIs

This document covers `mezon-native` — the crate that wraps platform-specific OS features. Each module has real implementations for macOS, Windows, and Linux (where supported).

## Overview

```
mezon-native/src/
├── lib.rs            # Crate root: re-exports open_url()
├── tray.rs           # System tray icon with context menu
├── badge.rs          # Dock/taskbar notification badge
├── notifications.rs  # Desktop toast notifications
├── autostart.rs      # Launch app on system login
├── deep_link.rs      # Register mezonapp:// URL scheme
├── power.rs          # System sleep/wake event listener
└── instance.rs       # Single-instance lock (prevent duplicate processes)
```

All modules are designed to be fire-and-forget at the call site. Errors are logged via `tracing::warn!` rather than propagated — a failing notification should never crash the app.

---

## 1. System Tray (`tray.rs`)

Creates a system tray icon with a right-click context menu.

| Platform | Implementation |
|----------|---------------|
| macOS | `tray_icon` crate via `NSStatusBar` |
| Windows | `tray_icon` crate via `Shell_NotifyIcon` |
| Linux | `tray_icon` crate via `XEmbed` or `StatusNotifierItem` |

**Key functionality:**
- Shows the Mezon icon in the system tray
- Context menu with: "Open Mezon", "Settings", separator, "Quit"
- Left-click on tray icon focuses the main window
- Tray icon is created in `main.rs` after `application().run()` is entered

---

## 2. Notification Badge (`badge.rs`)

Shows an unread-count badge on the app icon in the dock (macOS) or taskbar (Windows).

| Platform | API |
|----------|-----|
| macOS | `NSDockTile` — `setBadgeLabel:` via raw `objc` |
| Windows | `ITaskbarList3` — `SetOverlayIcon` with a custom-drawn bitmap |
| Linux | Not implemented (most DEs don't support it via a stable API) |

Usage in `main.rs`:
```rust
mezon_native::badge::set_badge(count); // count = 0 clears the badge
```

---

## 3. Desktop Notifications (`notifications.rs`)

Shows OS-native toast notifications.

### API

```rust
use mezon_native::notifications::Notification;

mezon_native::notifications::show(&Notification {
    title: "Alice".to_string(),
    body: "Hey, are you free?".to_string(),
    channel_id: Some("channel-123".to_string()), // groups notifications by channel
});
```

`show()` is fire-and-forget — it spawns a thread internally and returns immediately.

### Platform Implementations

**macOS (`UNUserNotificationCenter`):**
- Uses raw `objc` runtime calls (no `UserNotifications` framework header dependency)
- Requests authorization (badge + sound + alert) on first call; the OS caches the user's decision
- Each notification has a UUID identifier so they stack independently
- `channel_id` maps to `threadIdentifier` which groups notifications visually per-channel

**Windows (`ToastNotification`):**
- Uses the Windows Runtime (WinRT) `windows` crate
- Builds a simple XML toast template: `<toast><visual><binding template="ToastGeneric">...`
- Registered under AUMID `ai.mezon.Mezon` — must match the app's installer entry
- XML-escapes title and body to prevent injection

**Linux (`notify-rust`):**
- Wraps `libnotify` / D-Bus `org.freedesktop.Notifications`
- Works on GNOME, KDE, and most standard DEs
- No custom icons yet; uses `dialog-information`

---

## 4. Auto-Start (`autostart.rs`)

Registers or deregisters the app from system startup.

| Platform | Mechanism |
|----------|-----------|
| macOS | Login Item via `LaunchServices` |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` registry key |
| Linux | `~/.config/autostart/mezon.desktop` file |

Implementation uses the `auto-launch` crate:

```rust
pub fn sync_auto_start(enabled: bool) {
    set_auto_start(enabled); // logs warning on failure, does not propagate
}

pub fn set_auto_start(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let auto = AutoLaunchBuilder::new()
        .set_app_name("Mezon")
        .set_app_path(exe.to_str().unwrap_or("mezon"))
        .build()?;
    if enabled { auto.enable()? } else { auto.disable()? }
    Ok(())
}
```

`sync_auto_start()` is called once at startup in `main.rs` after settings are loaded. It compares `settings.auto_start` against the current OS state and adjusts accordingly. The check `if auto.is_enabled()?` before disabling avoids spurious errors on platforms where disabling a non-existent entry is an error.

---

## 5. Deep Link Registration (`deep_link.rs`)

Registers the `mezonapp://` URL scheme so the OS routes URLs to this app.

| Platform | Mechanism |
|----------|-----------|
| macOS | `Info.plist` `CFBundleURLTypes` entry (set at build time in the bundle) |
| Windows | `HKCU\Software\Classes\mezonapp` registry entry |
| Linux | `xdg-mime` with a custom `.desktop` file |

After registration, when any process opens a `mezonapp://` URL:
1. The OS launches the app (or focuses an existing instance) and passes the URL as a command-line argument
2. `main.rs` checks `argv` for a `mezonapp://` URL before the window opens
3. If a second instance is already running, `SingleInstance::try_acquire_or_forward(url)` sends the URL to the first instance via the IPC channel and then exits

---

## 6. Power Events (`power.rs`)

Listens for system sleep and wake events. Used to pause background tasks (e.g., the session refresh loop) when the system sleeps.

| Platform | API |
|----------|-----|
| macOS | `CFNotificationCenter` — `NSWorkspaceWillSleepNotification` / `NSWorkspaceDidWakeNotification` |
| Windows | `WTSRegisterSessionNotification` — `WM_WTSSESSION_CHANGE` messages |
| Linux | Stub — no implementation yet |

Usage in `main.rs`:
```rust
mezon_native::power::subscribe(|event| match event {
    PowerEvent::Sleep => pause_background_refresh(),
    PowerEvent::Wake  => resume_background_refresh(),
});
```

---

## 7. Single Instance Lock (`instance.rs`)

Ensures only one copy of the app runs at a time. When a second instance is launched (e.g., by clicking the app icon while already running), it forwards any deep-link URL to the running instance and exits.

### How It Works

The mechanism differs by platform:

**macOS / Linux (Unix domain socket):**
1. First instance: creates `$XDG_RUNTIME_DIR/mezon.sock` and binds a `UnixListener`
2. Second instance: tries `UnixStream::connect(socket_path)`. Succeeds → writes URL bytes → exits
3. First instance: the listener thread reads the URL and calls the callback

**Windows (Named pipe):**
1. First instance: creates `\\.\pipe\mezon-single-instance` with `FILE_FLAG_FIRST_PIPE_INSTANCE` — this flag makes it fail if a server already exists (mutex semantics)
2. Second instance: opens the pipe → writes URL → exits
3. First instance: the pipe listener thread reads the URL

### Stale Socket Handling

On Unix, if the app crashed without cleanup, the socket file remains. `try_acquire_unix()` handles this:
```rust
if socket_path.exists() {
    match UnixStream::connect(&socket_path) {
        Ok(_)  => return Ok(None),              // another instance is running
        Err(_) => fs::remove_file(&socket_path), // stale — clean up and continue
    }
}
```

### URL Forwarding

After `SingleInstance::try_acquire()`, the main instance calls `instance.listen_for_urls(callback)` to start accepting incoming URLs from future secondary instances. This callback is what wires the IPC channel to the GPUI event loop in `main.rs`.

### Cleanup

On Unix, `SingleInstance` implements `Drop` to remove the socket file. On Windows, the pipe handle lives until the process exits (no explicit cleanup needed).

---

## 8. `open_url` Helper

A small convenience function in `lib.rs`:

```rust
mezon_native::open_url("https://mezon.ai/forgot-password")?;
```

Uses the `open` crate which delegates to:
- macOS: `open`
- Windows: `ShellExecute`
- Linux: `xdg-open`

This is used in `LoginView` for the "Forgot password?" link.

---

## Platform Support Summary

| Feature | macOS | Windows | Linux |
|---------|-------|---------|-------|
| System tray | ✓ | ✓ | ✓ (partial) |
| Dock/taskbar badge | ✓ | ✓ | — |
| Desktop notifications | ✓ | ✓ | ✓ |
| Auto-start | ✓ | ✓ | ✓ |
| Deep link scheme | ✓ (plist) | ✓ (registry) | ✓ (xdg) |
| Power events | ✓ | ✓ | stub |
| Single instance | ✓ (socket) | ✓ (pipe) | ✓ (socket) |
| Open URL | ✓ | ✓ | ✓ |

> **Note:** On macOS, the `windows` crate is excluded from compilation — it is only pulled in under `[target.'cfg(target_os = "windows")'.dependencies]` in `crates/mezon-native/Cargo.toml`.
