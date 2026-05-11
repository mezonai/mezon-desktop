# Contributing Guide

This document covers the practical how-to for working on this codebase: the day-to-day workflow, how to add new components, screens, and native modules, and the rules each change must satisfy before merging.

---

## 1. Daily Workflow

All commands go through the `just` task runner. Run `just --list` to see all available tasks.

```bash
just watch    # hot-reload — recompiles and restarts on every save
just run      # one-shot debug build + run
just check    # fast cargo check + clippy (no linking) — use this frequently
just lint     # strict check before committing: clippy -D warnings + fmt --check
just fix      # auto-fix: cargo fmt + cargo clippy --fix
just test     # run all workspace tests via cargo-nextest
just cov      # generate HTML coverage report and open it
just release  # production build (opt-level 3, thin LTO, stripped)
```

**Before every commit:**
```bash
just fix   # auto-format and fix clippy suggestions
just lint  # must pass with zero warnings
just test  # must pass
```

---

## 2. Crate Boundaries — What Goes Where

Before adding code, decide which crate it belongs in:

| Crate | Put here |
|-------|----------|
| `mezon-store` | App-wide state types: enums, config structs, error types. No OS calls, no HTTP, no UI. |
| `mezon-client` | REST auth + TCP/TLS transport adapter + protobuf API client + UI-safe API wrapper + keychain + session model. No UI, no GPUI. |
| `mezon-native` | OS-specific APIs: tray, badge, notifications, autostart, deep links. No GPUI, no HTTP. |
| `mezon-ui` | All GPUI views and components. Depends on `mezon-store` and `mezon-client` but not on `mezon-native` directly (call native through `mezon-app`). |
| `mezon-proto` | Protobuf/wire types only. No business logic. |
| `mezon-updater` | Update check + download logic only. |
| `mezon-app` | Binary entry point only. Wires the other crates together. No reusable logic should live here. |

**Golden rule:** If something is in `mezon-app/src/main.rs`, it should be short. Anything that grows beyond 50 lines belongs in a dedicated module or crate.

---

## 3. Adding a New Primitive Component

Primitives live in `crates/mezon-ui/src/components/primitives/`. They are stateless — no GPUI entity, no `Context`, just a builder that terminates in `.render(&theme) -> AnyElement`.

### Step-by-step

1. **Create the file:** `crates/mezon-ui/src/components/primitives/my_widget.rs`

2. **Write the struct and builder:**

```rust
use gpui::{AnyElement, div, prelude::*};
use crate::theme::Theme;

pub struct MyWidget {
    label: String,
    // ... other config fields
}

impl MyWidget {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into() }
    }

    // Builder methods — each returns `Self` for chaining
    pub fn variant(mut self, v: MyVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn render(self, theme: &Theme) -> AnyElement {
        div()
            .text_color(theme.text_primary)
            .child(self.label)
            .into_any_element()
    }
}
```

3. **Export from `primitives/mod.rs`:**

```rust
pub mod my_widget;
pub use my_widget::MyWidget;
```

4. **Use in a view:**

```rust
card.child(MyWidget::new("Hello").render(&theme))
```

5. **Write tests** (in the same file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn renders_without_panicking() {
        let theme = Theme::dark();
        let _el = MyWidget::new("test").render(&theme);
        // If render() returns without panicking, the test passes.
    }
}
```

---

## 4. Adding a New Composition Component

Compositions have entity state and implement `Render`. They live in `crates/mezon-ui/src/components/compositions/`.

### Step-by-step

1. **Create the file:** `crates/mezon-ui/src/components/compositions/my_composition.rs`

2. **Write the struct, constructor, and `Render` impl:**

```rust
use gpui::{div, prelude::*, Context, Window};
use crate::theme::Theme;

pub struct MyComposition {
    title: String,
    value: String,
}

impl MyComposition {
    pub fn new(cx: &mut Context<Self>, title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            value: String::new(),
        }
    }

    pub fn set_value(&mut self, value: String, cx: &mut Context<Self>) {
        self.value = value;
        cx.notify();
    }
}

impl Render for MyComposition {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        div()
            .flex()
            .flex_col()
            .child(div().text_xs().text_color(theme.text_secondary).child(self.title.clone()))
            .child(div().text_sm().text_color(theme.text_primary).child(self.value.clone()))
    }
}
```

3. **Create it as a GPUI entity in a parent view's constructor:**

```rust
let my_comp = cx.new(|cx| MyComposition::new(cx, "Label"));
```

4. **Export from `compositions/mod.rs`:**

```rust
pub mod my_composition;
pub use my_composition::MyComposition;
```

---

## 5. Adding a New Screen / View

A "screen" is a full-window GPUI view (like `LoginView`). In Stage 2 most new screens will be panels inside `MainLayout`, but the process is the same.

### Step-by-step

1. **Create the file:** `crates/mezon-ui/src/my_screen.rs`

2. **Write the view struct:**

```rust
use std::sync::Arc;
use gpui::{div, prelude::*, Context, Entity, Window};
use mezon_client::MezonClient;
use mezon_store::AuthState;
use crate::theme::Theme;

pub struct MyScreen {
    auth_state: Entity<AuthState>,
    client: Arc<MezonClient>,
    // local UI state
    loading: bool,
}

impl MyScreen {
    pub fn new(
        auth_state: Entity<AuthState>,
        client: Arc<MezonClient>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { auth_state, client, loading: false }
    }
}

impl Render for MyScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .bg(theme.bg_primary)
            .child("My Screen")
    }
}
```

3. **Export from `mezon-ui/src/lib.rs`:**

```rust
pub mod my_screen;
pub use my_screen::MyScreen;
```

4. **Wire into `RootView`** by adding a new `AuthState` variant or a routing enum (Stage 2 will introduce proper routing).

5. **Construct in `main.rs`** (or in `RootView::new`) where needed.

---

## 6. Adding a New Native Module

Native modules live in `crates/mezon-native/src/`. Each module provides a cross-platform API with `#[cfg(target_os = "...")]` implementations.

### Step-by-step

1. **Create the file:** `crates/mezon-native/src/my_module.rs`

2. **Define a platform-agnostic public API with per-platform implementations:**

```rust
/// Brief description of what this module does.
pub fn do_thing(arg: &str) {
    tracing::debug!("do_thing: {arg}");

    #[cfg(target_os = "macos")]
    do_thing_macos(arg);

    #[cfg(target_os = "windows")]
    do_thing_windows(arg);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    do_thing_linux(arg);
}

#[cfg(target_os = "macos")]
fn do_thing_macos(arg: &str) {
    // macOS implementation
    // All unsafe blocks MUST have // SAFETY: comments
}

#[cfg(target_os = "windows")]
fn do_thing_windows(arg: &str) {
    // Windows implementation
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn do_thing_linux(arg: &str) {
    tracing::warn!("do_thing not implemented on Linux");
}
```

3. **Add `pub mod my_module;` to `mezon-native/src/lib.rs`.**

4. **Call from `main.rs`** in the appropriate startup phase (before or after `application().run()`).

### Rules for native modules

- Every `unsafe` block **must** have a `// SAFETY:` comment. See [Known Issues C-1](./11-known-issues.md#c-1-unsafe-blocks-have-no--safety-documentation).
- Never panic — use `tracing::warn!` on failure and return gracefully.
- Linux implementations that are genuinely not feasible should be explicit stubs with a `// TODO:` comment explaining what library or D-Bus interface would implement them.
- OS calls that must happen on the main thread (macOS: anything `NSApplication`-related) must use `dispatch::Queue::main().exec_async(...)`, not `std::thread::spawn`.

---

## 7. Code Quality Checklist

Before submitting a PR, every changed file must pass:

- [ ] `just lint` — zero clippy warnings, formatted correctly
- [ ] `just test` — all tests pass
- [ ] Functions are ≤50 lines
- [ ] No new `unwrap()` / `expect()` in non-test production paths — use `?` with context
- [ ] Every new `unsafe` block has a `// SAFETY:` comment
- [ ] New public items have `///` doc comments
- [ ] No `dbg!()`, `todo!()`, `println!()` — clippy denies these workspace-wide
- [ ] New state types go in `mezon-store`, not `mezon-ui` or `mezon-app`
- [ ] New tests written for any new pure-function logic

---

## 8. Error Handling Rules

| Location | Use |
|----------|-----|
| Library crates (`mezon-store`, `mezon-client`, `mezon-proto`) | `thiserror` typed errors |
| Application crate (`mezon-app`) | `anyhow::Result` with `.context(...)` |
| Native modules (`mezon-native`) | `anyhow::Result` internally; log + swallow at the public API boundary |
| UI code (`mezon-ui`) | Store errors as `Option<String>` in view state; render them as user-facing labels |

Never use `.unwrap()` in production paths. Use `.expect("reason")` only where the invariant is truly compile-time provable and add a comment explaining why it can never fail.

---

## 9. Async Rules

- All GPUI async work uses `cx.spawn(async move |cx: &mut AsyncApp| { ... }).detach()`
- Re-enter the app context with `cx.update(|cx| { ... })` — check the `Result` it returns
- Never `std::thread::sleep` — use `cx.background_executor().timer(Duration).await`
- Never call `tokio::runtime::Runtime::block_on` from inside a `cx.spawn()` task
- Pre-UI async work (before `application().run()`) uses the tokio runtime built in `run_app()`

---

## 10. Dependency Rules

- Adding a new crate dependency requires a workspace-level entry in the root `Cargo.toml`
- Run `cargo deny check` after adding dependencies to verify license and security compliance
- Prefer crates already in the workspace over adding new ones
- Do not add dependencies to `mezon-store` that would pull in GPUI, tokio, or reqwest — it must remain a lightweight state-only crate

---

## 11. Commit Message Format

```
<type>: <short description>

<optional body — why, not what>
```

Types: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`

Examples:
```
feat: add StatusDot composition component
fix: call set_api_url after successful authentication
docs: add known issues doc from rust-review
test: add unit tests for decode_jwt_claims and Session::is_expired
chore: run cargo fmt across all non-vendor crates
```

---

## 12. Vendored Code Policy

`crates/vendor/` contains GPUI and its supporting crates vendored from [zed-industries/zed](https://github.com/zed-industries/zed). **Do not modify vendor crates** unless:

1. The change is a Mezon-specific bug fix that cannot be upstreamed
2. The change is documented with a `// MEZON:` comment explaining the deviation
3. The change is reviewed separately from application code

If a GPUI bug needs fixing, prefer working around it in application code first.
