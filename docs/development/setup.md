# Development Setup

This project uses a Cargo workspace, Rust 2024, GPUI, Tokio, `just`, `cargo-nextest`, `cargo-deny`, and Clippy.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`
- platform build tools for native desktop development
- `just`
- development tools installed through the project recipe

```sh
just install
```

If `just install` is unavailable, install the tools directly:

```sh
cargo install cargo-binstall
cargo binstall -y cargo-watch cargo-nextest cargo-deny cargo-outdated cargo-llvm-cov
```

## Verify Environment

```sh
cargo --version
rustc --version
just --list
cargo metadata --no-deps
```

## First Run

```sh
just run
```

For hot reload during UI development:

```sh
just watch
```

## Editor Setup

Recommended Rust analyzer settings:

- enable Clippy on save
- use all workspace features when practical
- format with rustfmt
- show lifetime elision and type hints when helpful

Do not rely on editor diagnostics alone. Run the workspace commands before submitting a PR.

## Common Checks

```sh
just check      # fast Clippy check
just test       # cargo-nextest workspace tests
just lint       # strict pre-commit linting
just safety     # cargo-deny checks
```

## Platform Notes

Native integrations vary by OS. Keep platform setup notes in this file as they become concrete, but keep platform-specific code in `mezon-native`.

Linux desktop features may depend on portals and notification services. Windows and macOS features may require signing, entitlements, registry access, or app metadata for full behavior.
