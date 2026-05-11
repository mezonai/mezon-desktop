# GPUI Internals

This document goes deeper into GPUI concepts that show up repeatedly in the Mezon codebase: the ownership model, reactivity primitives, the text input pipeline, and the async executor patterns used in `main.rs`.

---

## 1. The Ownership Model

GPUI uses a single-owner model: the `App` object owns **all** entity state. You never hold a direct `&mut Entity`; you hold a lightweight handle (`Entity<T>`) that is just a ref-counted ID.

```
App {
    entity_map: HashMap<EntityId, Box<dyn Any>> {
        1 → Counter { count: 5 }
        2 → AuthState::NotAuthenticated
        3 → LoginView { ... }
    }
}

Entity<Counter>   // handle — just an ID + ref-count
Entity<AuthState> // handle — just an ID + ref-count
```

This design means:
- **No dangling pointers** — the app keeps state alive as long as any handle exists
- **No aliasing** — you can't hold two `&mut T` at once; the app mediates access
- **Cheap cloning** — `entity.clone()` is just an atomic ref-count increment

### Reading State

```rust
let count = counter.read(cx).count;   // immutable borrow for the duration of the call
```

### Mutating State

```rust
counter.update(cx, |counter, cx| {
    counter.count += 1;
    cx.notify();  // schedule re-renders of any view that read this entity
});
```

The `cx` inside the callback is a `Context<Counter>` — a narrowed view of `App` bound to this specific entity. It gives access to entity-specific services like `cx.notify()` and `cx.observe()`.

---

## 2. Reactivity: `notify` vs `emit`

GPUI has two reactivity primitives. Knowing which to use is important.

### `cx.notify()` + `cx.observe()`

Use when: "my state has changed, please re-check me."

- `cx.notify()` schedules a re-render of all views that **read** this entity
- `cx.observe(&other, callback)` runs `callback` whenever `other` calls `cx.notify()`

```rust
// In a view's render() — GPUI tracks that this view "depends on" auth_state
let state = self.auth_state.read(cx).clone();  // reading registers the dependency

// Elsewhere, when auth state changes:
auth_state.update(cx, |state, cx| {
    *state = AuthState::Authenticated(session);
    cx.notify();  // all views that called .read(cx) get re-rendered
});
```

This is how `RootView` switches content automatically — it reads `auth_state` in `render()`, so it re-renders whenever `auth_state` notifies.

### `cx.emit(event)` + `cx.subscribe()`

Use when: "a specific thing happened, here are the details."

```rust
// Entity declares what events it can emit
impl EventEmitter<LoginEvent> for LoginView {}

// Emitting
cx.emit(LoginEvent::Success { user_id: "...".into() });

// Subscribing (from another entity's constructor)
cx.subscribe(&login_view, |this, _entity, event, cx| {
    match event {
        LoginEvent::Success { user_id } => this.on_login(user_id, cx),
    }
})
.detach(); // keep subscription alive for the entity's lifetime
```

The returned `Subscription` must be either stored (to cancel later) or `.detach()`ed (to keep alive until one of the participants is dropped).

### Key Difference

| | `notify` / `observe` | `emit` / `subscribe` |
|---|---|---|
| Data | None — just a "changed" signal | Typed event payload |
| Use case | State change triggers re-render | Discrete event with details |
| React analogy | State update → re-render | Event listener |

---

## 3. The `cx.spawn()` Pattern

Long-running work happens in tasks spawned off the main thread:

```rust
cx.spawn(async move |cx: &mut AsyncApp| {
    let result = do_something_async().await;
    cx.update(|cx| {
        // Back on the main thread — safe to update entities
        entity.update(cx, |this, cx| {
            this.result = result;
            cx.notify();
        });
    });
})
.detach();
```

Key rules:
- The async closure runs on GPUI's smol executor (not tokio)
- Re-entering the app context requires `cx.update(|cx| { ... })`
- `.detach()` means "fire and forget" — the task runs until completion even if the `cx` is dropped
- To cancel a task, store the returned `Task<T>` handle and drop it

### Background Executor vs Main Executor

```rust
// Sleep/timer — never block the main thread
let exec = cx.background_executor().clone();
exec.timer(Duration::from_secs(60)).await;

// Spawn CPU work off the main thread
cx.background_executor().spawn(async { heavy_computation() }).await;
```

`background_executor()` runs work on a thread pool. The main GPUI executor handles UI work and is single-threaded.

---

## 4. The `AsyncApp` Context

Inside a `cx.spawn()` block, the context is `&mut AsyncApp` rather than `&mut App`. This is intentional — you cannot call most GPUI APIs directly from an async context; you must re-enter via `cx.update()`:

```rust
cx.spawn(async move |cx: &mut AsyncApp| {
    // ✓ OK — background work
    let data = fetch_data().await;

    // ✓ OK — re-enter with update
    cx.update(|cx| {
        entity.update(cx, |this, cx| { this.data = data; cx.notify(); });
    });

    // ✗ WRONG — can't call cx.notify() directly here
    // cx.notify();
})
```

The distinction exists because async tasks may be interleaved with other tasks; `cx.update()` provides a synchronised re-entry point where GPUI can safely process the update atomically.

---

## 5. The `TextInput` Pipeline

`TextInput` is the most complex component in the codebase because it needs to integrate with the OS text input system (IME, dead keys, copy/paste). This requires a non-obvious two-part design.

### The Problem

GPUI does not directly route keyboard events for printable characters to `on_key_down`. Instead, it follows the OS text input protocol:

1. The OS calls methods on a `NSTextInputClient` (macOS) / `ITextStoreACP` (Windows)
2. GPUI exposes this as the `EntityInputHandler` trait
3. The primary entry point is `replace_text_in_range` — called for every committed character, including IME compositions

### The Two-Part Design

**Part 1 — `EntityInputHandler` impl on `TextInput`:**

Handles the OS text protocol. The critical method is `replace_text_in_range`:

```rust
fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, ...) {
    self.replace_utf16_range(range, text); // UTF-16 range → UTF-8 byte range conversion
    self.fire_change(window, cx);           // call the on_change callback
    cx.notify();                            // re-render
}
```

The range is in **UTF-16 code units** (because that's what the OS uses internally), but Rust strings are UTF-8. `byte_range_for_utf16_range()` converts between them character by character.

**Part 2 — `TextInputElement` wrapper:**

`window.handle_input()` can only be called during the **paint phase**. To achieve this, `TextInput::render()` does not return a plain `div()`. Instead it returns a `TextInputElement` — a custom `Element` implementation that wraps the visual `AnyElement` and calls `handle_input` in its `paint()` method:

```rust
fn paint(&mut self, ...) {
    self.child.paint(window, cx);   // paint the visible UI first

    // Register the OS input handler — only legal here in paint
    window.handle_input(
        &self.focus_handle,
        ElementInputHandler::new(bounds, self.entity.clone()),
        cx,
    );
}
```

`ElementInputHandler::new(bounds, entity)` creates the bridge: when the OS delivers a `replace_text_in_range` call, GPUI routes it to `entity` (the `TextInput`) at the registered `bounds`.

### Focus Management

`TextInput` owns a `FocusHandle` created via `cx.focus_handle()`. The input only accepts text when focused:

```rust
// Click to focus
.on_mouse_down(MouseButton::Left, |_, window, cx| {
    window.focus(&focus_handle, cx);
})

// Visual cue — blue cursor bar when focused
if is_focused && !self.disabled {
    input_box = input_box.child(/* cursor bar div */);
}
```

`Focusable` is implemented on `TextInput` so the framework can manage tab-stop navigation.

### Backspace Handling

Backspace is handled separately in `on_key_down` because it is not a printable character and doesn't go through the OS text input protocol:

```rust
.on_key_down(|event, window, cx| {
    match event.keystroke.key.as_str() {
        "backspace" => {
            // Remove the last UTF-8 character
            let new_len = value.char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
            value.truncate(new_len);
        }
        _ => {} // printable chars handled via replace_text_in_range
    }
})
```

### Masked Mode

When `masked = true`, the display value replaces every character with `●`. The underlying `self.value` still holds the real text — masking is purely visual:

```rust
let display_value = if self.masked {
    "●".repeat(self.value.len())
} else {
    self.value.clone()
};
```

---

## 6. Entity Smuggling from `open_window`

`main.rs` has a pattern worth understanding: extracting an `Entity<AuthState>` from inside the `open_window` closure to use it outside:

```rust
// The closure passed to cx.open_window() must return the root view.
// But we also need the auth_state entity *outside* the closure.
// GPUI doesn't provide a return value from open_window, so we smuggle it out.

let auth_out = Arc::new(Mutex::new(None::<Entity<AuthState>>));
let auth_out_clone = auth_out.clone();

cx.open_window(options, move |_window, cx| {
    let auth_state = cx.new(|_cx| initial_auth);

    // Stash the handle in the Arc before returning the view
    *auth_out_clone.lock().unwrap() = Some(auth_state.clone());

    cx.new(|cx| RootView::new(title_bar, auth_state, client, cx))
});

// After open_window returns, extract the handle
let auth_state_handle = auth_out.lock().unwrap().clone().unwrap();
```

This works because `open_window` calls the closure synchronously before returning, so the `Arc<Mutex<Option>>` is populated by the time we read it on the next line.

---

## 7. The Deep-Link IPC Channel

`main.rs` uses an `std::sync::mpsc` channel to bridge between the OS thread (where `SingleInstance::listen_for_urls` fires) and the GPUI main thread (where `AuthState` lives):

```
[OS / secondary instance thread]
    SingleInstance::listen_for_urls callback
        └─→ url_tx.send(url)

[GPUI background task, polling every 100ms]
    url_rx.try_recv()
        └─→ cx.update(|cx| auth_state.update(...))
```

`try_recv()` (non-blocking) is used inside the GPUI task so it doesn't stall the executor waiting for a URL. The 100ms poll interval is a reasonable balance between responsiveness and CPU usage.

---

## 8. The Tray Quit-Flag Pattern

The system tray "Quit" menu item cannot call `cx.quit()` directly — it fires on a different thread with no GPUI context. The workaround:

```
[Tray thread]
    quit_flag.store(true, Relaxed)

[GPUI background task, polling every 200ms]
    if quit_flag.load(Relaxed) {
        cx.update(|cx| cx.quit())
    }
```

An `AtomicBool` is the thread-safe primitive here. `Ordering::Relaxed` is sufficient because there's no other shared memory that needs to be synchronised with this flag — we only need the bool itself to be visible eventually, not to establish a happens-before relationship.

---

## 9. The Dual Runtime

The app runs two async runtimes simultaneously:

| Runtime | Lives on | Used for |
|---------|----------|----------|
| `tokio::runtime::Runtime` | Background threads | Pre-UI work in `main()`: `Settings::load()`, silent session refresh. Also the fallback for `ReqwestClient` HTTP calls. |
| GPUI smol executor | Main thread | All `cx.spawn()` tasks, timers, UI work |

**Key rule:** Never `.block_on()` tokio from inside the GPUI executor. The pre-UI tokio work in `run_app()` happens **before** `application().run()` is called, so there's no conflict. After `run()` starts, all async work goes through `cx.spawn()`.

The `rt_handle` (an `Arc<tokio::runtime::Handle>`) is passed into `setup_tray()` in case the tray needs to dispatch tokio futures in Stage 2.
