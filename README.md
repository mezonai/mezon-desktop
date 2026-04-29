# Mezon Desktop

Mezon Desktop is the native Rust desktop client for Mezon, a live communication
platform for communities, gaming, work, and collaboration.

This repository contains the Rust/GPUI application. It is organized as a Cargo
workspace with a small app shell, GPUI views, API/client state, native OS
integration, and vendored GPUI/Zed support crates.

## Status

The app is under active migration to a native GPUI client.

- Auth screens are implemented with email OTP and email/password login.
- Sessions are stored in the OS keychain and refreshed in the background.
- Native integration includes single-instance handling, deep links, tray support,
  notifications, auto-start, and screen lock/unlock hooks.
- The main authenticated app shell is still in progress.

## Requirements

- Rust via `rustup`
- `just`
- Platform build tools for Rust desktop applications

The Rust toolchain is pinned in `rust-toolchain.toml`. When using `rustup`, Cargo
will automatically install and use the configured toolchain and components.

## Quick Start

```bash
# Install just if needed
cargo install just

# Clone the repository
git clone https://github.com/mezonai/mezon-desktop
cd mezon-desktop

# Install development tools used by the just recipes
just install

# Run the app in debug mode
just run
```

Run `just` to list the available recipes.

## Common Commands

```bash
just run          # Build and run the app
just watch        # Re-run on changes with cargo-watch
just check        # Run clippy checks for the workspace
just lint         # Strict lint and format checks
just fix          # Apply rustfmt and clippy fixes
just test         # Run tests with cargo-nextest
just cov-summary  # Show coverage summary
just cov          # Generate and open HTML coverage
just safety       # Run cargo-deny checks
just release      # Build a release binary
```

Most recipes are thin wrappers around Cargo commands, so direct Cargo usage also
works:

```bash
cargo run
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Workspace Layout

```text
crates/
  mezon-app/      Binary entry point, GPUI bootstrap, window setup, app runtime
  mezon-ui/       GPUI views, theme, primitives, and composition components
  mezon-client/   REST API client, session handling, keychain integration
  mezon-store/    Persistent settings and application state models
  mezon-native/   Tray, deep links, notifications, auto-start, single instance
  mezon-updater/  Update-checking logic
  mezon-proto/    Protobuf-facing types
  vendor/         Vendored GPUI/Zed support crates

assets/
  fonts/          Bundled fonts used by the UI
```

The default Cargo workspace member is `crates/mezon-app`, whose binary is named
`mezon`.

## Runtime Notes

The default API client configuration currently points at the development Mezon
backend:

- REST host: `dev-mezon.nccsoft.vn`
- REST port: `8088`
- TLS: enabled
- WebSocket host: `sock.mezon.ai`

Persistent settings are stored at:

```text
~/.config/mezon/settings.json
```

Auth sessions are stored through the platform keychain using the service name
`mezon-desktop`.

Logs use `tracing_subscriber` and can be filtered with `RUST_LOG`:

```bash
RUST_LOG=mezon=debug,info just run
```

## Development Workflow

Before opening a pull request or handing off changes, run:

```bash
just lint
just test
```

For dependency and license checks, run:

```bash
just safety
```

To inspect or update dependencies:

```bash
just outdated
just update
```
