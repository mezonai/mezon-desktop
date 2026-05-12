# Known Issues and Technical Debt

This document captures every known bug, safety gap, and quality violation in the Stage 1 codebase as of the last Rust code review. Read this before touching any of the affected files.

Issues are grouped by severity. Fix CRITICAL items before shipping any production build. HIGH items should be resolved before Stage 2 merge. MEDIUM items can be addressed incrementally.

---

## CRITICAL

### C-1: `unsafe` blocks have no `// SAFETY:` documentation

**Affects:** All of `mezon-native` — `notifications.rs`, `power.rs`, `instance.rs`, `badge.rs`, `deep_link.rs`

**Problem:** Every `unsafe` block in the codebase is missing the mandatory `// SAFETY:` comment that documents the invariants being upheld. Rust's safety rules require the programmer to prove, in writing, that a block of unsafe code is sound. Without this, future refactors can silently break the safety proof without anyone noticing.

The worst cases:

- `power.rs` — raw `*const PowerEventCallback` reconstructed from a `usize` across thread boundaries with no documented lifetime, alignment, or aliasing guarantees
- `instance.rs` — Windows `HANDLE` smuggled as a `usize` into a spawned thread:
  ```rust
  let raw = handle.0 as usize; // Send-safe: we own it exclusively
  // inside thread:
  let h = windows::Win32::Foundation::HANDLE(raw as isize);
  ```
  This has an inline comment but not a `// SAFETY:` block — there is no documentation that the handle stays valid across the thread boundary, that the thread is the sole owner, or that no double-close occurs.
- `notifications.rs` — entire `show_macos` body is one large `unsafe` block with 15+ ObjC message sends and no documentation of object lifetimes, retain/release counts, or thread affinity.

**Fix:** Before every `unsafe` block, add a `// SAFETY:` comment that names all invariants. Example:
```rust
// SAFETY: `raw` was created by `Box::into_raw` in `subscribe()` and is never freed
// (it is intentionally leaked for the process lifetime). The pointer is non-null,
// properly aligned for `PowerEventCallback`, and no mutable aliasing exists because
// the `Box` was the sole owner before this conversion.
let cb = unsafe { &*(raw as *const PowerEventCallback) };
```

---

### C-2: `Mutex::lock().unwrap()` can cause a double-panic

**Affects:** `crates/mezon-app/src/main.rs:279, 291`

**Problem:** The `open_main_window` function uses `Arc<Mutex<Option<Entity<AuthState>>>>` to extract the `auth_state` handle from the `open_window` closure. If the closure panics mid-execution (e.g. a GPUI initialisation failure), the `Mutex` is poisoned. The subsequent `.unwrap()` on the poisoned mutex then panics again, producing an unhelpful `PoisonError` instead of the original error message.

```rust
// line 279 — inside open_window closure
*auth_out_clone.lock().unwrap() = Some(auth_state.clone());

// line 291 — outside, after open_window returns  
auth_out.lock().unwrap().clone().expect("auth_state not initialised after open_window")
```

**Fix:**
```rust
// Tolerate a poisoned mutex — extract the inner value regardless
auth_out.lock().unwrap_or_else(|p| p.into_inner()).clone()
    .expect("auth_state not initialised after open_window")
```
Or restructure to avoid the pattern entirely (see `open_main_window` in `09-gpui-internals.md`).

---

## HIGH

### H-1: macOS notifications dispatched on the wrong thread

**Affects:** `crates/mezon-native/src/notifications.rs:52`

**Problem:** `UNUserNotificationCenter` on macOS must be called from the main thread. The code's own comment says so — but then immediately spawns a background thread:

```rust
// UNUserNotificationCenter operations must happen on the main thread in
// a macOS app that has an NSApplication.  We dispatch to it asynchronously.
// GPUI's main thread is always the UI thread, so this is safe.
std::thread::spawn(move || unsafe {
    let center: *mut Object = msg_send![center_cls, currentNotificationCenter];
    // ...
```

The comment says "GPUI's main thread is always the UI thread, so this is safe" but the code does the opposite of dispatching to the main thread — it dispatches to a *new* thread. The result is silent failure or a crash at runtime depending on the macOS version.

**Fix:** Use Grand Central Dispatch (GCD) to run on the main queue:
```rust
// dispatch to main queue
dispatch::Queue::main().exec_async(move || unsafe {
    // ... UNUserNotificationCenter calls here
});
```
Or, since GPUI runs on the main thread, call `mezon_native::notifications::show()` directly from a GPUI context rather than spawning a thread.

---

### H-2: `set_api_url` is never called after authentication

**Affects:** `crates/mezon-client/src/auth.rs`, `crates/mezon-ui/src/login_view.rs`

**Problem:** The `MezonClient::set_api_url` method exists specifically to redirect subsequent API calls to the server-specified REST endpoint (returned as `api_url` in the login response). The module documentation explicitly calls this out:

```rust
//! After a successful login the server returns an `api_url` field; subsequent API calls
//! should be directed to that host (call `set_api_url` after auth).
```

However, `set_api_url` is never called anywhere in the codebase. After login, the client continues to hit `dev-mezon.nccsoft.vn:8088` regardless of what the server returns. The `Session.api_url` field is saved to the keychain but never read back to configure the client. Session refresh calls (every 60 seconds in `main.rs`) will silently fail against a production server that uses a different host.

**Fix:** In `LoginView::on_auth_success` and in `main.rs::resolve_initial_auth_state`, after obtaining a session, update the client:
```rust
if let Some(url) = &session.api_url {
    client.set_api_url(url);
}
```
Note: `MezonClient` takes `&mut self` for `set_api_url`, so the `Arc<MezonClient>` wrapping needs to change to `Arc<Mutex<MezonClient>>` or `Arc<RwLock<MezonClient>>`, or the method signature needs to use interior mutability.

---

### H-3: `cx.update()` return values silently discarded

**Affects:** `crates/mezon-app/src/main.rs:111, 220, 230, 309` and `crates/mezon-ui/src/login_view.rs:116, 177, 231`

**Problem:** `AsyncApp::update` returns `Result<T, Dropped>`. If the window (or the app itself) is closed between the time an async HTTP call is fired and when it completes, `cx.update` returns `Err(Dropped)` — and the state mutation never runs. In the current code, all call sites ignore this result:

```rust
cx.update(|cx| {         // ← Result<(), Dropped> discarded
    entity_clone.update(cx, |this, cx| {
        this.loading = false;
        match result {
            Ok(session) => Self::on_auth_success(session, &auth_state, cx),
            Err(e)      => this.error = Some(format!("{e}")),
        }
        cx.notify();
    });
});
```

A user who closes the login window while an OTP confirmation is in flight would lose the authentication success silently.

**Fix:** Log the dropped case at minimum:
```rust
if let Err(_dropped) = cx.update(|cx| { ... }) {
    tracing::debug!("Async state update skipped — context was dropped (window closed)");
}
```

---

### H-4: `LoginView::render` is 277 lines (should be ≤50)

**Affects:** `crates/mezon-ui/src/login_view.rs:297–573`

**Problem:** The entire `Render::render` implementation is a single 277-line function. This violates the 50-line limit and makes the UI structure difficult to follow.

**Fix:** Extract helper methods:
```rust
impl LoginView {
    fn render_otp_step0(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement { ... }
    fn render_otp_step1(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement { ... }
    fn render_password_form(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement { ... }
    fn render_header(theme: &Theme) -> impl IntoElement { ... }
    fn render_divider(theme: &Theme) -> impl IntoElement { ... }
    fn render_method_toggle(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement { ... }
}
```
The `render` function itself then becomes a short coordinator under 20 lines.

---

### H-5: Zero test coverage

**Affects:** All non-vendor crates

**Problem:** There are zero `#[test]` functions in the entire codebase. The project rule requires 80% minimum coverage. See [Testing Guide](./12-testing-guide.md) for how to add tests and what to test first.

**Easiest tests to add immediately** (pure functions with no GPUI dependency):
- `mezon-client::auth::decode_jwt_claims` — pure function, fully unit-testable
- `mezon-client::session::Session::is_expired` — pure function
- `mezon-store::Settings` — serde round-trip test
- `mezon-store::LoginError` — `Display` impl test

---

### H-6: `TitleBar` allocates a new `String` on every render frame

**Affects:** `crates/mezon-ui/src/title_bar.rs:20`

**Problem:** `TitleBar` stores `title: String` and clones it in `render()` every frame:
```rust
let title = self.title.clone(); // heap allocation on every frame
```

GPUI re-renders every frame. On a 60Hz display this is 60 allocations per second for a string that never changes.

**Fix:** Change the field to `SharedString` (GPUI's interned, ref-counted string):
```rust
pub struct TitleBar {
    title: SharedString,  // clone is a ref-count bump, not a heap copy
}
```

---

## MEDIUM

### M-1: Blocking syscall in async function

**Affects:** `crates/mezon-store/src/lib.rs:51`

`path.exists()` is synchronous blocking I/O inside an `async fn`:
```rust
pub async fn load() -> Result<Self> {
    if !path.exists() { ... }  // blocks the executor thread
```
**Fix:** Use `tokio::fs::try_exists(&path).await?` or handle `ErrorKind::NotFound` from the async read.

---

### M-2: Malformed JWT silently makes session appear immortal

**Affects:** `crates/mezon-client/src/auth.rs:291–311`

If JWT decoding fails, `expires_at` falls back to `0`. In `Session::is_expired`, the check `self.expires_at > 0 && ...` treats `0` as "never expires". A corrupted stored session would prevent the refresh loop from ever triggering.

**Fix:** Add a `tracing::warn!` when base64 decoding or JSON parsing fails, and consider returning `Result<(String, String, u64)>` from `decode_jwt_claims` so the caller can decide how to handle parse failures.

---

### M-3: `#[allow(dead_code)]` without explanatory comments

**Affects:** `crates/mezon-client/src/auth.rs:41, 44`

```rust
#[allow(dead_code)]
created: bool,
#[allow(dead_code)]
is_remember: bool,
```

The suppression is correct — these fields exist only to absorb JSON from the server — but without a comment a future reader may delete them thinking they're genuinely unused.

**Fix:** Add a comment: `// Present in server JSON response; not used by the client.`

---

### M-4: Library crate uses `anyhow::Result` for public API

**Affects:** `crates/mezon-store/src/lib.rs`

`Settings::load` and `Settings::save` return `anyhow::Result`. `mezon-store` is a library crate; library crates should expose typed errors (via `thiserror`) so callers can match on specific error variants without depending on `anyhow`'s opaque error chain.

---

### M-5: `autostart.rs` silently uses `"mezon"` as the exe path on non-UTF-8 systems

**Affects:** `crates/mezon-native/src/autostart.rs:24`

```rust
.set_app_path(exe.to_str().unwrap_or("mezon"))
```
On a system where the executable path contains non-UTF-8 characters, the auto-launch entry is registered with the literal string `"mezon"` — an invalid path. The app would not actually auto-launch.

**Fix:**
```rust
let exe_str = exe.to_str().context("Executable path contains non-UTF-8 characters")?;
```

---

### M-7: OTP digit count is a magic number duplicated in two places

**Affects:** `crates/mezon-ui/src/login_view.rs:71, 161`

```rust
let otp_fields = (0..6).map(...).collect();
// ...
if otp_code.len() != 6 { return; }
```

**Fix:**
```rust
const OTP_DIGIT_COUNT: usize = 6;
let otp_fields = (0..OTP_DIGIT_COUNT).map(...).collect();
// ...
if otp_code.len() != OTP_DIGIT_COUNT { return; }
```

---

## Formatting

`cargo fmt --check` currently fails across 15 non-vendor files. Run `just fix` (which runs `cargo fmt` + `cargo clippy --fix`) to auto-fix all formatting issues before any commit. CI will reject unformatted code.

```bash
just fix    # auto-fix formatting and clippy suggestions
just lint   # verify: must pass before commit
```

---

## Issue Tracker Summary

| ID | Severity | File | One-line description |
|----|----------|------|----------------------|
| C-1 | CRITICAL | `mezon-native/*` | All `unsafe` blocks missing `// SAFETY:` comments |
| C-2 | CRITICAL | `main.rs:291` | `Mutex::lock().unwrap()` can double-panic |
| H-1 | HIGH | `notifications.rs:52` | macOS UNUserNotificationCenter called on wrong thread |
| H-2 | HIGH | `auth.rs`, `login_view.rs` | `set_api_url` never called post-auth |
| H-3 | HIGH | `main.rs`, `login_view.rs` | `cx.update()` results silently discarded |
| H-4 | HIGH | `login_view.rs:297` | `render()` is 277 lines |
| H-5 | HIGH | all crates | Zero test coverage (0% vs 80% required) |
| H-6 | HIGH | `title_bar.rs:20` | `String::clone()` per frame — use `SharedString` |
| M-1 | MEDIUM | `mezon-store/lib.rs:51` | Blocking `path.exists()` in async fn |
| M-2 | MEDIUM | `auth.rs:291` | Malformed JWT silently sets `expires_at = 0` |
| M-3 | MEDIUM | `auth.rs:41,44` | `#[allow(dead_code)]` without comment |
| M-4 | MEDIUM | `mezon-store/lib.rs` | Library crate exposes `anyhow::Result` |
| M-5 | MEDIUM | `autostart.rs:24` | Silent `"mezon"` fallback for non-UTF-8 exe path |
| M-7 | MEDIUM | `login_view.rs:71,161` | Magic number `6` for OTP digit count |
| FMT | BLOCKER | 15 files | `cargo fmt --check` fails — run `just fix` |
