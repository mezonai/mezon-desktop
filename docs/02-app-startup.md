# App Startup: main.rs

**File:** `crates/mezon-app/src/main.rs`

This is the entry point — the equivalent of `index.js` in a Node app. It's ~340 lines
and is split into several well-named functions. Here's a complete walkthrough.

---

## The Full Startup Flow

```
main()
  │
  ├─ 1. Set up logging
  ├─ 2. Parse argv for deep link URL
  ├─ 3. Single-instance check ──→ already running? forward URL + exit
  │
  └─ run_app()
       │
       ├─ 4. Create tokio runtime (async engine)
       ├─ 5. Load settings from JSON file
       ├─ 6. Resolve auth state (keychain → maybe refresh API call)
       ├─ 7. OS-level setup (autostart, deep link scheme, power events)
       │
       └─ application().run()   ← GPUI takes over (like ReactDOM.render)
            │
             ├─ 8. Open main window (mounts RootView)
             ├─ 9. Spawn URL poll loop (every 100ms)
             ├─ 10. Spawn session refresh loop (every 60s)
             ├─ 11. Spawn shared TCP transport task (500ms poll loop, auto-connects when AuthState changes to Authenticated)
             └─ 12. Set up system tray
```

Everything before `application().run()` is synchronous setup on the main thread.
Everything inside `.run()` is GPUI's reactive world — like React's component tree.

---

## Step 1 — Logging Setup (lines 12–17)

```rust
fmt()
    .with_env_filter(
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("mezon=debug,info")),
    )
    .init();
```

Configures structured logging — like setting up `winston` or `pino` in Node.

- Default filter: `mezon=debug,info` — DEBUG level for our crates, INFO for everything else.
- Override at runtime: `RUST_LOG=mezon=trace cargo run` (like `LOG_LEVEL=trace node app.js`)

---

## Step 2 — Parse Deep Link from argv (lines 22–24)

```rust
let deep_link_url: Option<String> = std::env::args()
    .nth(1)
    .filter(|a| a.starts_with("mezonapp://"));
```

When the OS opens the app from a `mezonapp://` URL (e.g., user clicks a link in a browser),
it passes the URL as a command-line argument — like `process.argv[1]` in Node.

`Option<String>` means the result is either `Some("mezonapp://...")` or `None` — Rust's
version of nullable. There is no `null` or `undefined` in Rust; optionality is explicit.

---

## Step 3 — Single Instance Guard (lines 27–38)

```rust
let lock_result = match deep_link_url.as_deref() {
    Some(url) => SingleInstance::try_acquire_or_forward(url)?,
    None => SingleInstance::try_acquire()?,
};

match lock_result {
    None => { return Ok(()); }   // Another instance is already running — just exit
    Some(lock) => run_app(lock, deep_link_url),
}
```

Ensures only one copy of the app runs at a time — like preventing duplicate browser tabs.

**Mechanism:** A Unix domain socket file at `$XDG_RUNTIME_DIR/mezon.sock` (or a Windows
named pipe). If a socket already exists and responds, another instance is running.
In that case, the new instance forwards the deep link URL to the existing one and exits.

`match` is Rust's pattern matching — like a `switch` statement but exhaustive (you must
handle every case). The `?` at the end of `try_acquire()?` means "if this returns an
error, propagate it up to the caller immediately" — like `throw` but without exceptions.

---

## Step 4 — Create the Tokio Runtime (lines 45–49)

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("Failed to build tokio runtime");
```

In Rust, there is no built-in event loop — you create one explicitly. This creates a
multi-threaded async runtime (like starting the Node.js event loop, but manually).

**Why two runtimes?** GPUI uses its own async executor (`smol`) for UI tasks. The `tokio`
runtime here is used for blocking startup work (settings load, session refresh) that
must complete *before* the window opens. After that, HTTP calls also use tokio internally.

---

## Step 5 — Load Settings (line 51)

```rust
let settings = rt.block_on(Settings::load()).unwrap_or_default();
```

- `rt.block_on(...)` — runs an async function synchronously, blocking the thread until done.
  Like `await` at the top level of an async Node script.
- `Settings::load()` reads `~/.config/mezon/settings.json`.
- `.unwrap_or_default()` — if loading fails for any reason, use default values. The app
  never crashes just because settings are missing or malformed.

---

## Step 6 — Resolve Auth State (lines 61–62, 145–179)

```rust
let client = Arc::new(MezonClient::default());
let initial_auth_state = resolve_initial_auth_state(&rt, &client);
```

`Arc` is Rust's reference-counted smart pointer — it lets multiple parts of the code
share ownership of the same data safely across threads. Think of it as a shared object
in JS that multiple modules hold a reference to.

`resolve_initial_auth_state` checks the OS keychain for a stored session token. Three outcomes:

| Keychain state | Outcome |
|----------------|---------|
| Nothing stored | `AuthState::NotAuthenticated` → show login page |
| Token exists, not expired | `AuthState::Authenticated(session)` → skip login |
| Token exists, expired | Call refresh API → `Authenticated` or `NotAuthenticated` |

```rust
fn resolve_initial_auth_state(rt: &Runtime, client: &MezonClient) -> AuthState {
    match keychain::load_session() {
        None => AuthState::NotAuthenticated,
        Some(session) if !Session::is_expired(&session) => AuthState::Authenticated(session),
        Some(session) => {
            // Try silent refresh — if it fails, go to login
            match rt.block_on(client.refresh_session(&session.refresh_token, false)) {
                Ok(new_session) => AuthState::Authenticated(new_session),
                Err(_) => AuthState::NotAuthenticated,
            }
        }
    }
}
```

---

## Step 7 — OS-Level Setup (lines 65–78)

```rust
mezon_native::autostart::sync_auto_start(settings.auto_start);
mezon_native::deep_link::register_deep_link_scheme();
mezon_native::power::subscribe(Box::new(|event| { ... }));
```

Three one-time registrations before the window opens:

1. **Autostart** — sync the "launch at login" setting with the OS.
2. **Deep link scheme** — register `mezonapp://` with the OS (idempotent — safe to call every launch).
3. **Power events** — subscribe to screen lock/unlock events. Currently just logs them; future use for pausing WebSocket connections, etc.

---

## Step 8 — Open the Main Window (lines 100, 244–294)

```rust
let auth_state_handle = open_main_window(cx, &settings, client.clone(), initial_auth_state);
```

Inside `open_main_window`:

```rust
cx.open_window(options, move |_window, cx| {
    let auth_state = cx.new(|_cx| initial_auth);   // Create shared state entity
    let title_bar = cx.new(|_cx| TitleBar::new("Mezon"));
    cx.new(|cx| RootView::new(title_bar, auth_state, client, api: Arc<AppApi>, cx))  // Mount root view (Arc<AppApi> created from shared TransportClient before window open)
})
```

- `cx.new(|_cx| ...)` creates a reactive state entity — like calling `useState()` in React.
- `RootView` is the root component. It reads `AuthState` and renders either the login screen
  or the app shell (once Stage 2 is built).
- Window options include: size (1280×720, min 950×500), transparent title bar, hidden
  traffic lights (macOS), window bounds restored from settings.

---

## Step 9 — Deep Link URL Poll Loop (lines 103–128)

```rust
cx.spawn(async move |cx: &mut AsyncApp| {
    loop {
        match url_rx.try_recv() {
            Ok(url) if url.starts_with("mezonapp://callback") => {
                // Update auth state → triggers re-render
                auth_state.update(cx, |state, cx| {
                    *state = AuthState::AwaitingCallback;
                    cx.notify();
                });
            }
            Err(Disconnected) => break,
            _ => {}
        }
        exec.timer(Duration::from_millis(100)).await;
    }
}).detach();
```

A background task that polls a channel every 100ms for incoming deep link URLs.
When a `mezonapp://callback` URL arrives (from an OAuth browser flow), it updates
`AuthState` — which triggers a re-render of `RootView`.

`.detach()` means "fire and forget" — like calling a promise without `await`.

---

## Step 10 — Session Refresh Loop (lines 131, 182–241)

```rust
spawn_refresh_task(cx, auth_state_handle.clone(), client.clone());
```

A background task that wakes every 60 seconds:

1. Read the current `AuthState` — if not `Authenticated`, do nothing.
2. Check if the token expires within the next 5 minutes.
3. If yes, call the refresh API endpoint (`POST /v2/account/session/refresh`).
4. On success: update keychain + update `AuthState` → triggers re-render.
5. On failure: clear keychain + set `AuthState::NotAuthenticated` → re-render shows login.

---

## Step 11 — Shared TCP Transport Task

`spawn_transport_task` creates a background task that reads `session.tcp_host` from session, uses hardcoded dev port 7349, creates `TransportClient`, calls `connect()`. On connection success, calls `AppApi::get_account()` to verify API connectivity.

A dedicated tokio runtime named `mezon-transport` with 2 worker threads is initialized lazily via `OnceLock` in `TransportClient`. This is separate from the main app tokio runtime.

## Step 12 — System Tray (lines 134, 297–337)

```rust
let _tray = setup_tray(cx, rt_handle.clone());
```

Creates the icon in the OS menu bar/system tray. The tray has three menu items:
- **Show Mezon** — currently just logs (Stage 2 will bring the window to front)
- **Check for Updates** — calls `mezon_updater::check_for_updates()` (always returns "no update" for now)
- **Quit** — sets an `AtomicBool` flag to `true`

A separate background task polls that flag every 200ms and calls `cx.quit()` when set.
The flag + poll approach is needed because `cx.quit()` must run on the GPUI main thread.

---

## Key Rust Concepts in This File

| Rust concept | What it does | Web analogy |
|---|---|---|
| `Option<T>` | Value that may or may not exist | `T \| null` |
| `Result<T, E>` | Success or error | `Promise<T>` / try-catch |
| `?` operator | Propagate error up | `throw` (but explicit) |
| `match` | Pattern matching | `switch` (but exhaustive) |
| `Arc<T>` | Shared ownership across threads | Shared object reference |
| `let x = ...` | Immutable binding (default) | `const x = ...` |
| `let mut x = ...` | Mutable binding | `let x = ...` |
| `.detach()` | Fire-and-forget async task | Unawaited promise |
| `cx.notify()` | Trigger re-render | `setState()` |
| `block_on(...)` | Run async code synchronously | Top-level `await` |
