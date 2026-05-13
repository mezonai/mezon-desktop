# Workspace Layout

The workspace root owns shared dependency versions, lint policy, and developer commands. Individual crates own their public APIs and tests.

## Root Layout

```text
.
├── Cargo.toml
├── Cargo.lock
├── justfile
├── deny.toml
├── rust-toolchain.toml
├── assets/
├── crates/
└── docs/
```

The root `Cargo.toml` should contain:

- workspace members
- shared package metadata
- workspace dependency versions
- workspace lint configuration

Avoid declaring unrelated crate-specific dependencies at the root unless they are intentionally shared.

## Crate Layout

Use predictable Rust crate structure:

```text
crates/mezon-store/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── settings.rs
│   ├── session.rs
│   └── workspace.rs
└── tests/
    └── settings_persistence.rs
```

For larger modules:

```text
src/
├── lib.rs
├── workspace/
│   ├── mod.rs
│   ├── event.rs
│   ├── model.rs
│   └── store.rs
└── settings/
    ├── mod.rs
    ├── error.rs
    └── repository.rs
```

Keep `mod.rs` files small. They should usually re-export the module API and describe the module boundary.

## Public API Shape

Good:

```rust
mod error;
mod repository;
mod store;

pub use error::SettingsError;
pub use repository::SettingsRepository;
pub use store::{SettingsStore, SettingsUpdate};
```

Bad:

```rust
pub mod error;
pub mod repository;
pub mod store;
```

Public modules expose implementation structure. Prefer re-exporting stable types and keeping internals private.

## Binary Crate

`mezon-app` should remain a thin composition crate:

```text
crates/mezon-app/src/
├── main.rs
├── bootstrap.rs
├── runtime.rs
└── window.rs
```

Do not place domain state transitions, HTTP request construction, or platform-specific implementations in `mezon-app`.

## UI Crate

`mezon-ui` should separate primitives, components, views, and view models:

```text
crates/mezon-ui/src/
├── lib.rs
├── theme/
├── primitives/
├── components/
├── views/
├── actions.rs
└── view_model/
```

- `primitives`: low-level reusable GPUI building blocks.
- `components`: reusable domain-aware UI pieces.
- `views`: screens, panels, and composed surfaces.
- `view_model`: UI-facing data derived from stores.

## Tests

Prefer unit tests beside logic and integration tests under `tests/`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_notifications_do_not_emit_native_request() {
        // ...
    }
}
```

Use `cargo-nextest` for workspace test execution:

```sh
just test
cargo nextest run -p mezon-store
```
