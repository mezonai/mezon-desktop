# GPUI: The UI Framework

GPUI is the GPU-accelerated UI framework that powers the [Zed](https://zed.dev) editor.
It is vendored into this project under `crates/vendor/gpui/`.

If you're coming from React, most concepts map directly — the terminology is just different.

---

## Core Concept Map: GPUI vs React

| React | GPUI | Notes |
|-------|------|-------|
| `useState<T>` | `Entity<T>` | Reactive state container |
| `setState(newValue)` | `entity.update(cx, \|state, cx\| { *state = new; cx.notify(); })` | Must call `cx.notify()` to trigger re-render |
| Component | A struct implementing the `Render` trait | |
| `render()` return value | `impl IntoElement` | What the view renders |
| `useEffect` + async | `cx.spawn(async move \|cx\| { ... })` | Spawn background async task |
| `ReactDOM.render()` | `application().run(\|cx\| { ... })` | Start the app |
| `createContext` / `useContext` | `cx` (the AppContext) | Passed everywhere |
| `React.createRef` | `FocusHandle` | For keyboard focus management |
| Re-render | `cx.notify()` | Called inside an entity update |
| Props | Method arguments on the struct constructor | |
| Component tree | Element tree (div, flex, text) → Taffy layout → GPU draw | |

---

## The Reactive Model

### State: `Entity<T>`

`Entity<T>` is a handle to a reactive state object — like a Zustand store or Redux slice.

```rust
// Create state (inside a GPUI context)
let auth_state: Entity<AuthState> = cx.new(|_cx| AuthState::NotAuthenticated);

// Read state
let current = auth_state.read(cx);  // → &AuthState

// Update state (triggers re-render of all subscribed views)
auth_state.update(cx, |state, cx| {
    *state = AuthState::Authenticated(session);
    cx.notify();  // ← This is the "setState" — schedules a re-render
});
```

Multiple views can hold a clone of the same `Entity<T>` handle and all will re-render
when the state changes. The `Entity` is cheap to clone (it's reference-counted internally).

### Views: implementing `Render`

A view is a Rust struct that implements the `Render` trait:

```rust
pub struct RootView {
    auth_state: Entity<AuthState>,
    title_bar: Entity<TitleBar>,
    client: Arc<MezonClient>,
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut ViewContext<Self>) -> impl IntoElement {
        // Read current state
        let auth = self.auth_state.read(cx);

        match auth {
            AuthState::NotAuthenticated => {
                // Render login screen
                div().child(self.login_view.clone())
            }
            AuthState::Authenticated(_) => {
                self.account_test_view.clone().into_any_element()
            }
        }
    }
}
```

### Layout: Tailwind-like methods

GPUI uses a Flexbox layout engine (Taffy). Layout is expressed as method chains
on elements — very similar to Tailwind CSS classes:

| CSS / Tailwind | GPUI |
|----------------|------|
| `display: flex` | `.flex()` |
| `flex-direction: column` | `.flex_col()` |
| `align-items: center` | `.items_center()` |
| `justify-content: space-between` | `.justify_between()` |
| `padding: 16px` | `.p_4()` |
| `padding-top: 8px` | `.pt_2()` |
| `width: 100%` | `.w_full()` |
| `color: #fff` | `.text_color(white)` |
| `background: #313338` | `.bg(theme.background)` |
| `border-radius: 8px` | `.rounded_lg()` |
| `gap: 8px` | `.gap_2()` |

```rust
// Example: a centered column with padding
div()
    .flex()
    .flex_col()
    .items_center()
    .p_6()
    .gap_4()
    .child(Label::new("Hello").size(LabelSize::Xl2))
    .child(Button::new("Click me").on_click(|_, cx| { /* ... */ }))
```

---

## The Dual Runtime

The app runs two async engines simultaneously:

```
┌─────────────────────────────────┐   ┌─────────────────────────────────┐
│          GPUI / smol             │   │             tokio               │
│                                  │   │                                 │
│  Main UI thread                  │   │  Background HTTP calls          │
│  cx.spawn() tasks                │   │  (reqwest uses tokio internally)│
│  Animation, timers               │   │  Startup blocking work          │
│                                  │   │  (Settings::load, session       │
│  Everything that touches the UI  │   │   refresh before window opens)  │
└─────────────────────────────────┘   └─────────────────────────────────┘
```

**Rule of thumb:**
- Use `cx.spawn()` for tasks that need to update UI state
- HTTP/network code runs on tokio transparently (reqwest manages this internally)
- Never block the GPUI main thread — use `.await` or `cx.background_executor()`

---

## Data Flow: TCP/TLS → UI

This is the full path a real-time message takes:

```
TCP/TLS → AbridgedTcpAdapter → MezonTransport → protobuf Envelope → cid-routed response → entity update
```

The realtime push path (cid=0 → on_message callback) is wired but not yet used by UI.

---

## Async in GPUI

Spawning a background task from within GPUI:

```rust
// Inside a view method or app closure:
cx.spawn(async move |cx: &mut AsyncApp| {
    // Do async work
    let result = some_api_call().await;

    // Update UI state from the async context
    cx.update(|cx| {
        my_entity.update(cx, |state, cx| {
            state.data = result;
            cx.notify();
        });
    });
})
.detach();   // Fire and forget (like an unawaited promise)
```

Key points:
- `cx.spawn()` runs the task on GPUI's executor
- To update state from inside the async task, use `cx.update(|cx| { ... })`
- `.detach()` means the task runs independently — like calling a promise without `await`
- For timers: `cx.background_executor().timer(Duration::from_secs(60)).await`

---

## Current Views in This Project

| File | View | Role |
|------|------|------|
| `src/root.rs` | `RootView` | Root component — switches between login / app shell based on `AuthState` |
| `src/login_view.rs` | `LoginView` | Login form — OTP or password mode |
| `src/title_bar.rs` | `TitleBar` | Custom frameless title bar — drag region + window controls |

### `AuthState` variants and what renders

```
AuthState::NotAuthenticated    → LoginView (login form)
AuthState::OtpRequested { .. } → LoginView (OTP code entry step)
AuthState::AwaitingCallback    → Placeholder ("Waiting for login...")
AuthState::Authenticated(..)   → `AccountTestView` (shows `get_account`/`list_clan_descs`/`list_channel_descs` results)
```

---

## UI Components

### Primitives (`mezon-ui/src/components/primitives/`)

Ready-to-use leaf components — equivalent to a basic design system:

| Component | Key props / features |
|-----------|---------------------|
| `Avatar` | Size (Xs–Xl2), presence status dot, URL image or initials fallback |
| `Badge` | Count pill, caps at "99+", auto-hides at 0 |
| `Button` | Variant (Primary/Secondary/Ghost/Danger), size, disabled/loading, `on_click` |
| `Divider` | Horizontal rule, optional centered label |
| `Icon` | 32 named icons (`IconName` enum), size + color |
| `Label` | Size (Xs–Xl2), weight (Normal/Medium/SemiBold/Bold) |
| `Spinner` | Animated SVG rotation (700ms) |
| `TextInput` | Stateful, placeholder, label, error, password mode, caret rendering |

### Compositions (`mezon-ui/src/components/compositions/`)

Higher-level components built from primitives:

| Component | Description |
|-----------|-------------|
| `EmptyState` | Icon + title + subtitle + optional action button |
| `FormField` | Wraps `TextInput` with a label header |
| `IconButton` | Square button with an icon |
| `SectionHeader` | Collapsible sidebar category header with chevron |
| `StatusDot` | Presence indicator dot (derives color from `PresenceStatus`) |
| `UserChip` | Avatar + username label inline |

### Theme (`mezon-ui/src/theme.rs`)

```rust
// Two theme constructors
let theme = Theme::dark();
let theme = Theme::light();

// 22 color tokens available, e.g.:
theme.background          // main window background
theme.surface             // card / panel background
theme.text_primary        // primary text color
theme.text_secondary      // muted text color
theme.brand               // accent / brand color
theme.status_online       // green presence dot
theme.status_dnd          // red DND dot
```

> **Note:** `RootView` currently hard-codes `Theme::dark()` regardless of `Settings.theme`.
> This will be fixed in Stage 3 (Settings UI).

---

## AppApi

`AppApi` in `mezon-client/src/app_api.rs` wraps `TransportClient` providing a UI-safe API surface. Authenticated views receive `Arc<AppApi>` from `RootView`.
