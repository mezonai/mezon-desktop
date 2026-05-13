# Repository Guidelines

## Project Structure & Module Organization

Mezon Desktop is a Rust 2024 Cargo workspace for a native GPUI desktop app. Source lives under `crates/`: `mezon-app` is the binary/bootstrap crate, `mezon-ui` owns GPUI views and components, `mezon-client` owns REST/auth transport, `mezon-store` owns persistent/domain state, `mezon-native` owns OS integration, `mezon-updater` owns update checks, and `mezon-proto` owns protobuf-facing types. Vendored dependencies live in `crates/vendor/`. Assets are in `assets/` and `crates/assets/`. Engineering docs live in `docs/`; start with `docs/README.md`.

## Build, Test, and Development Commands

Use `just` recipes when possible:

```sh
just run       # build and run the app
just check     # cargo clippy --workspace -- -D warnings
just test      # cargo nextest run --workspace --all-targets
just lint      # strict Clippy plus rustfmt check
just safety    # cargo-deny security/license checks
```

For focused work, prefer crate-scoped commands such as `cargo clippy -p mezon-store -- -D warnings` or `cargo nextest run -p mezon-client`.

## Coding Style & Naming Conventions

Run `cargo fmt --all` before review. Prefer explicit ownership, typed errors, domain newtypes such as `ChannelId`, and enums over boolean state flags. Avoid `unwrap` in production code, blocking work on the UI thread, locks held across `.await`, and platform-specific code in `mezon-ui`. Keep domain events near their owning store, for example `crates/mezon-store/src/workspace/event.rs`.

## Testing Guidelines

Use unit tests near domain logic and integration tests under each crate’s `tests/` directory when needed. Test behavior: store transitions, DTO conversion, persistence migrations, cancellation/retry paths, deep-link parsing, and view-model derivation. Name tests descriptively, e.g. `expired_session_transitions_to_unauthenticated`.

## Commit & Pull Request Guidelines

Recent history uses short imperative subjects with occasional Conventional Commit prefixes (`fix:`, `docs:`). Keep commits focused and describe the user-visible or architectural effect. PRs should include a concise summary, crates touched, linked issues, screenshots for UI changes, tests run, and any security, migration, or dependency notes.

## Agent-Specific Instructions

Before editing, read the relevant docs in `docs/architecture/` and `docs/conventions/`. For reviews, follow `.agents/commands/ai-review-code.md`. Preserve crate boundaries and do not revert unrelated user changes.
