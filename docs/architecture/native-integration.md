# Native Integration

Native OS integration belongs in `mezon-native`. UI and store crates should consume stable Rust APIs, not platform APIs directly.

## Scope

`mezon-native` owns:

- tray integration
- deep links
- notifications
- auto-start
- single-instance behavior
- OS-specific keychain adapters when not owned by `mezon-client`
- platform permissions and capability checks

## Boundary Pattern

Expose platform-neutral types:

```rust
pub enum NativeEvent {
    DeepLinkReceived(DeepLink),
    TrayMenuSelected(TrayCommand),
    NotificationActivated(NotificationId),
}

pub trait NativeRuntime: Send + Sync {
    fn set_tray_menu(&self, menu: TrayMenu) -> Result<(), NativeError>;
    fn show_notification(&self, notification: Notification) -> Result<(), NativeError>;
}
```

Keep `cfg` blocks inside `mezon-native`:

```rust
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;
```

Bad:

```rust
// In mezon-ui
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::*;
```

## Deep Links

Deep links should be parsed into typed commands before they reach stores.

```rust
pub enum DeepLink {
    OpenChannel(ChannelId),
    JoinWorkspace(InviteToken),
    Authenticate(AuthCallback),
}
```

Reject unknown, malformed, or unsafe deep links early. Never pass raw deep-link URLs into UI components for interpretation.

## Notifications

Notification payloads must avoid secrets and excessive user data.

Good:

```rust
Notification {
    title: "New message".into(),
    body: "Open Mezon to view it".into(),
}
```

Risky:

```rust
Notification {
    title: workspace_name,
    body: full_private_message,
}
```

Use privacy settings to decide what can appear in OS-level notifications.

## Single Instance

Single-instance behavior should forward activation data to the running instance:

```text
second launch
  ↓
native instance lock detects existing process
  ↓
forwards deep link or activation request
  ↓
running app raises window and emits NativeEvent
```

Do not let two app instances write to the same settings database without explicit locking and recovery.

## Platform Failures

Platform APIs fail for normal reasons: missing portals, disabled notifications, denied permissions, unavailable keychain, registry restrictions, sandboxing.

Represent those cases explicitly:

```rust
pub enum NotificationError {
    PermissionDenied,
    PlatformUnavailable,
    InvalidPayload,
    Backend(NativeBackendError),
}
```

Do not collapse platform failures into `String`.
