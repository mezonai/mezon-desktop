# Native Module Deep Dive

This document goes deeper than [Native Platform APIs](./08-native-apis.md), explaining the implementation details of each native module: how icons are loaded, how OS APIs are called, and where unsafe Rust is required and why.

---

## 1. Tray (`mezon-native/src/tray.rs`)

### Ownership Model

`MezonTray` wraps a single `TrayIcon` handle:

```rust
pub struct MezonTray {
    _icon: TrayIcon,  // leading underscore signals "keep-alive, not read"
}
```

`TrayIcon` from the `tray_icon` crate removes the icon from the system tray when dropped. Keeping `MezonTray` alive in `main.rs` for the entire process lifetime keeps the icon visible. If you store it in a local variable, it disappears immediately when that scope exits.

### Menu Items

Three menu items are assigned string IDs at construction time:

```
SHOW_ID   = "show"    → "Show Mezon"         → calls on_show()
UPDATE_ID = "update"  → "Check for Updates"  → async update check via tokio
QUIT_ID   = "quit"    → "Quit Mezon"         → calls on_quit()
```

A `PredefinedMenuItem::separator()` sits between the update item and quit.

### Event Thread

`MenuEvent::receiver()` returns a channel receiver. A dedicated background thread is spawned at tray creation that loops on `receiver.recv()`, matching against the three IDs:

```
show   → on_show()
update → rt_handle.spawn(async { check_for_updates(...) })
quit   → on_quit()
```

The `on_show` and `on_quit` callbacks are wrapped in `Arc` before moving into the thread (they are already `Send + Sync + 'static`). The tokio handle is also cloned per-event so the async update check doesn't need to wait for the event thread.

### Icon Loading: Search Path Cascade

The tray icon is loaded via a four-path cascade:

```
1. <exe_dir>/assets/icons/trayicon.png
2. <exe_dir>/../../../assets/icons/trayicon.png          (macOS .app bundle depth)
3. <exe_dir>/../../../../assets/icons/trayicon.png       (one level deeper)
4. assets/icons/trayicon.png                             (relative to CWD, for dev)
```

Paths 2 and 3 exist because a macOS `.app` bundle places the executable at `MyApp.app/Contents/MacOS/myapp`, three directories deep from the bundle root. Assets are typically at `MyApp.app/Contents/Resources/`. The `../../../` prefix climbs out of `MacOS/` → `Contents/` → `MyApp.app/` to land at the bundle root. Path 4 catches `cargo run` from the workspace root.

The cascade tries each path with `path.exists()` and `load_icon_from_path()`. The first successful load returns. On failure, it logs a warning and continues to the next path.

### Fallback Icon

If all search paths fail, `build_fallback_icon()` generates a 22×22 solid-colour icon at runtime:

```rust
const SIZE: u32 = 22;
// Each pixel: R=0x58, G=0x65, B=0xF2, A=0xFF → brand blue #5865F2
rgba.extend_from_slice(&[0x58, 0x65, 0xF2, 0xFF]);
```

484 pixels × 4 bytes = 1,936 bytes allocated on the heap. This is cheap and ensures the tray always appears even in a stripped production binary that lost its asset path.

---

## 2. Badge (`mezon-native/src/badge.rs`)

### macOS: `NSDockTile.setBadgeLabel`

macOS badge counts are set via the ObjC runtime. The call chain:

```
NSApplication.sharedApplication → .dockTile → .setBadgeLabel(NSString?)
```

Passing `nil` clears the badge (count == 0). Passing an `NSString` containing the count sets the red badge. The label is constructed from a Rust `String` via:

```
NSString.alloc → initWithBytes:length:encoding:  (encoding 4 = NSUTF8StringEncoding)
```

The `// Safety:` comment in the source states this must be called from the main thread. Since GPUI runs all UI callbacks on the main thread and `set_badge_count` is called from those callbacks, this is satisfied — but it's not enforced by the type system (see [Known Issues H-1](./11-known-issues.md#h-1-macos-notifications-dispatched-on-the-wrong-thread) for the similar notifications bug).

### Windows: `ITaskbarList3::SetOverlayIcon`

Windows does not have a dock badge. Instead, `ITaskbarList3::SetOverlayIcon` draws a small icon overlay in the bottom-right corner of the taskbar button. The overlay is an `HICON`, not a text label — so we must generate a tiny bitmap with the count drawn on it.

**Instantiation:**
```rust
let taskbar: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
taskbar.HrInit()?;
```

**Clearing:** Pass `None` as the icon and `HWND(0)` (uses foreground window of calling process).

**Setting:** Call `build_count_icon(count)` to get an `HICON`, then `SetOverlayIcon(HWND(0), hicon, &description)`, then `DestroyIcon(hicon)`.

> Note: `HWND(0)` is a known limitation documented in the source code. Stage 2 will store the real `WindowHandle` and pass it here. The current code works because Windows uses the calling process's foreground window when `HWND(0)` is passed.

### Windows: `build_count_icon` — GDI Bitmap Generation

The badge icon is drawn with the Windows GDI (Graphics Device Interface):

```
1. GetDC(None)                           → get screen device context
2. CreateCompatibleDC(hdc_screen)        → create off-screen DC
3. CreateCompatibleBitmap(hdc_screen, 16, 16)   → color bitmap
4. CreateCompatibleBitmap(hdc_screen, 16, 16)   → mask bitmap (required for ICONINFO)
5. ReleaseDC(None, hdc_screen)           → release screen DC
6. SelectObject(hdc, hbmp_color)         → activate color bitmap in DC
7. CreateSolidBrush(COLORREF(BG_COLOR))  → brand blue brush
8. FillRect(hdc, &full_rect, brush)      → fill 16×16 background
9. DeleteObject(brush)
10. SetBkMode(hdc, TRANSPARENT)          → transparent text background
11. SetTextColor(hdc, COLORREF(TEXT_COLOR))  → white
12. TextOutW(hdc, 1, 2, text_u16)        → draw count string at (1,2)
13. DeleteDC(hdc)
14. CreateIconIndirect(&ICONINFO { fIcon, hbmColor, hbmMask })  → HICON
15. DeleteObject(hbmp_color); DeleteObject(hbmp_mask)
```

`COLORREF` uses BGR byte order (the Windows convention), so `#5865F2` (RGB) becomes `0x00F26558` (BGR). Count above 99 is capped at `"99+"` to fit the 16-pixel width.

The `ICONINFO.fIcon = true` marks this as an icon rather than a cursor.

---

## 3. Deep Links (`mezon-native/src/deep_link.rs`)

### What "Deep Link" Means Here

A deep link lets the OS route a URL like `mezonapp://callback?token=abc` to the Mezon app when clicked in a browser. The app receives the URL as a command-line argument (`argv[1]`) or via IPC from a second instance (see `instance.rs`). `deep_link.rs` handles only the *registration* step — telling the OS which executable owns the `mezonapp://` scheme.

### macOS: No Runtime Code

macOS reads `CFBundleURLTypes` from `Info.plist` at app launch. The registration is completely static:

```xml
<!-- In mezon-app/assets/Info.plist (not shown in this file, handled by build system) -->
<key>CFBundleURLTypes</key>
<array>
  <dict>
    <key>CFBundleURLSchemes</key>
    <array>
      <string>mezonapp</string>
    </array>
  </dict>
</array>
```

`register_deep_link_scheme()` on macOS is a no-op with a debug log.

### Windows: Registry Layout

Windows requires the scheme to be registered in `HKCU\Software\Classes`. The registration writes three registry entries:

```
HKCU\Software\Classes\mezonapp
  (Default) = "URL:mezonapp Protocol"    ← human-readable description

HKCU\Software\Classes\mezonapp
  URL Protocol = ""                       ← empty value signals this is a URL scheme

HKCU\Software\Classes\mezonapp\shell\open\command
  (Default) = "C:\path\to\mezon.exe" "%1" ← command to run; %1 = the URL
```

Values are written as `REG_SZ` (wide-string, UTF-16). `RegCreateKeyExW` is used (not `RegOpenKeyExW`) so it creates the key if it doesn't exist and opens it if it does — making the operation idempotent.

The loop iterates a slice of `(subkey, value_name, data)` tuples and encodes each as UTF-16 + null terminator before calling the Win32 API.

### Linux: `.desktop` File + xdg-mime

Linux has no single registry. The XDG specification uses `.desktop` files:

1. Write `~/.local/share/applications/mezon.desktop`:

```ini
[Desktop Entry]
Name=Mezon
Comment=Mezon desktop client
Exec=/path/to/mezon %u
Icon=mezon
Type=Application
Categories=Network;InstantMessaging;
MimeType=x-scheme-handler/mezonapp;
StartupNotify=true
```

The `%u` placeholder is replaced with the URL by the desktop environment. The `MimeType` key declares this app handles `mezonapp://` URLs.

2. Set permissions to `0o755` (executable required by some DEs).

3. Register with `xdg-mime default mezon.desktop x-scheme-handler/mezonapp` — this updates `~/.config/mimeapps.list`.

4. Call `update-desktop-database ~/.local/share/applications` — refreshes the MIME cache so the change takes effect without a logout.

Both `xdg-mime` and `update-desktop-database` are called with `tracing::warn!` on failure rather than returning an error, since these tools may not be installed on minimal systems.

---

## 4. Power Events (`mezon-native/src/power.rs`)

### Public API

```rust
pub enum PowerEvent { ScreenLocked, ScreenUnlocked }
pub type PowerEventCallback = Box<dyn Fn(PowerEvent) + Send + 'static>;

pub fn subscribe(callback: PowerEventCallback) { ... }
```

The callback is called from a background thread. It is `Send + 'static` so it can be moved into a thread. Current usage in `main.rs` logs the event with `tracing::info!` — Stage 2 will use it to pause/resume the WebSocket connection.

### macOS: CFNotificationCenter Trampoline Pattern

macOS system events (screen lock, wake from sleep, etc.) are delivered via the `CFNotificationCenter` distributed notification center. This is a C API that requires a callback with a specific C function signature — a Rust closure cannot be passed directly.

**The trampoline pattern** solves this in three steps:

**Step 1: Leak the closure into a stable heap address.**

```rust
let raw: usize = Box::into_raw(Box::new(callback)) as usize;
```

`Box::new(callback)` moves the closure onto the heap. `Box::into_raw` converts the `Box` into a raw pointer, preventing Rust from ever dropping it (the pointer is "leaked"). Casting to `usize` makes it trivially `Send` so it can cross the thread boundary.

**Step 2: Register an `extern "C"` trampoline function.**

```rust
extern "C" fn trampoline(
    _center: *mut c_void,
    observer: *mut c_void,   // ← the raw pointer we passed as "observer"
    name: *const c_void,     // ← CFStringRef of the notification name
    _object: *const c_void,
    _user_info: *const c_void,
) {
    let cb = unsafe { &*(observer as *const PowerEventCallback) };
    // ... convert CFString name, invoke cb(event)
}
```

`trampoline` is a regular Rust function with `extern "C"` ABI. Its address can be passed to C APIs. The `observer` parameter is the raw pointer we pass as the "observer object" to `CFNotificationCenterAddObserver` — the OS hands it back unchanged on every notification.

Inside the trampoline, `observer` is cast back to `*const PowerEventCallback` and dereferenced. This is safe because the pointer was created by `Box::into_raw` in this call's lifetime, is never freed, is non-null, properly aligned, and no mutable reference exists.

**Step 3: Spawn a thread and run the Core Foundation run loop.**

```rust
std::thread::spawn(move || unsafe {
    cf_register(trampoline, raw as *mut c_void);
    cf_run_loop_run();  // blocks forever, delivering notifications to trampoline
});
```

`CFRunLoopRun()` blocks the thread indefinitely, dispatching CF notifications as they arrive. This thread's sole purpose is to keep the run loop alive.

**`cf_register`** wraps two C functions declared with `unsafe extern "C"`:
- `CFNotificationCenterGetDistributedCenter()` — the distributed notification center
- `CFNotificationCenterAddObserver(center, observer, callback, name, object, suspension_behavior)` — registers for a named notification
- `CFStringCreateWithCString(alloc, c_str, encoding)` — creates a CFString from a null-terminated C string

Both `com.apple.screenIsLocked` and `com.apple.screenIsUnlocked` are registered in a loop.

**`cf_string_to_str`** converts the CFStringRef notification name back to a Rust `String` using `CFStringGetCStringPtr`. Returns `None` if the string is null or uses a non-ASCII encoding.

### Windows: Message-Only Window + WTSRegisterSessionNotification

Windows session events (lock/unlock) are delivered as Win32 messages to a window procedure. To receive them without creating a visible window, a "message-only window" is used.

**Storage for the callback:**

```rust
static WIN_POWER_CB: OnceLock<Mutex<Option<PowerEventCallback>>> = OnceLock::new();
```

A module-level static is required because `wnd_proc` is an `extern "system"` fn — it cannot capture any Rust closures. The callback is stored in a `Mutex<Option<...>>` accessed via the static.

**Initialization:**

1. Store the callback in `WIN_POWER_CB`.
2. Spawn the message loop thread.

**Message loop thread (`windows_message_loop`):**

1. Register a window class `"MezonPowerWnd"` with `wnd_proc` as the procedure.
2. Create a message-only window: `CreateWindowExW(..., HWND_MESSAGE, ...)`. `HWND_MESSAGE` makes the window invisible and prevents it from appearing in `GetDesktopWindow`'s child list.
3. Register for session notifications: `WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)`. This subscribes the window to `WM_WTSSESSION_CHANGE` messages.
4. Run a standard Win32 message loop: `GetMessageW` / `TranslateMessage` / `DispatchMessageW`.

**Window procedure (`wnd_proc`):**

```rust
if msg == WM_WTSSESSION_CHANGE {
    match wparam.0 {
        WTS_SESSION_LOCK   (7) → cb(PowerEvent::ScreenLocked)
        WTS_SESSION_UNLOCK (8) → cb(PowerEvent::ScreenUnlocked)
    }
} else if msg == WM_DESTROY {
    PostQuitMessage(0);
}
DefWindowProcW(hwnd, msg, wparam, lparam)
```

The callback is retrieved by locking `WIN_POWER_CB`. `DefWindowProcW` handles all other messages with default processing.

### Linux: Stub

```rust
pub fn subscribe(_callback: PowerEventCallback) {
    tracing::debug!("Power event subscription not yet implemented on Linux");
}
```

The correct Linux implementation would subscribe to `org.freedesktop.login1` on the system D-Bus using the `zbus` crate, or alternatively subscribe to `UPower` for sleep/wake events. This is deferred until Stage 2 adds the WebSocket connection that needs to be paused on sleep.

---

## 5. Known Unsafe Patterns Summary

All four modules use `unsafe` code. The [Known Issues C-1](./11-known-issues.md#c-1-unsafe-blocks-have-no--safety-documentation) issue documents that the mandatory `// SAFETY:` comments are missing. Here is what those comments should document for each unsafe block:

| Module | Unsafe operation | Required invariants |
|--------|-----------------|---------------------|
| `badge.rs` macOS | ObjC `msg_send!` on `NSApplication` | Called from main thread (GPUI guarantee) |
| `badge.rs` Windows | GDI calls + `CreateIconIndirect` | All Win32 handles are valid and correctly sized |
| `deep_link.rs` Windows | `RegCreateKeyExW` / `RegSetValueExW` | Wide strings are null-terminated; buffer lengths are correct |
| `power.rs` macOS | Dereference `observer: *mut c_void` as `*const PowerEventCallback` | Pointer came from `Box::into_raw`, is never freed, non-null, aligned, no aliasing |
| `power.rs` macOS | `CFNotificationCenterAddObserver` C FFI | C API contract; center is non-null (returned by CF itself) |
| `power.rs` Windows | `CreateWindowExW` / message loop | Follows Win32 message loop contract; HWND is valid before registration |

See the [Contributing Guide](./13-contributing.md#rules-for-native-modules) for the requirement that every `unsafe` block in a native module must have a `// SAFETY:` comment.
