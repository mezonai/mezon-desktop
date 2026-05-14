# Mezon Desktop Engineering Docs

This documentation is the working contract for Mezon Desktop: a native Rust desktop application built on GPUI, Tokio, `tracing`, and a Cargo workspace.

The docs are optimized for maintainers, new contributors, and AI coding agents. Prefer these documents over local intuition when making architectural changes.

## Start Here

- [Architecture Overview](architecture/overview.md): system shape and crate responsibilities.
- [Workspace Layout](architecture/workspace-layout.md): crate map and source organization.
- [Dependency Rules](architecture/dependency-rules.md): allowed dependency directions.
- [State Management](architecture/state-management.md): domain stores, events, and ownership.
- [Async Architecture](architecture/async-architecture.md): Tokio, cancellation, channels, and task ownership.
- [UI Conventions](conventions/ui.md): GPUI view composition and boundaries.
- [AI Contributing](conventions/ai-contributing.md): rules for AI-generated code.

## Documentation Map

```text
docs/
├── architecture/      System design and crate relationships
├── conventions/       Coding, review, security, UI, and testing rules
├── development/       Local setup, workflow, debugging, releases
└── decisions/         Architecture Decision Record template
```

## Project Principles

- Keep crate boundaries explicit.
- Prefer strong domain types over primitive strings and booleans.
- Prefer event-driven state updates over shared mutable global state.
- Keep GPUI views responsive and free of blocking work.
- Keep platform integration isolated in `mezon-native`.
- Use `Result` and typed errors in production paths.
- Use `tracing` spans and structured fields for observability.
- Treat tests, Clippy, and `cargo-deny` as part of the design process.

## Baseline Commands

```sh
just check
just test
just lint
just safety
```

Use narrower commands while iterating:

```sh
cargo clippy -p mezon-ui --all-targets -- -D warnings
cargo nextest run -p mezon-store
cargo test -p mezon-client session
```

## Existing Reference Docs

The repository also contains earlier numbered documents under `docs/`. Keep them when useful for migration or historical context, but use this structured documentation as the current contributor-facing contract.
