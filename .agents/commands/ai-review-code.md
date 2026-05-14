# AI Code Review

Use this command when reviewing Mezon Desktop code changes for correctness, maintainability, and architectural fit.

You are reviewing a native Rust desktop application built with GPUI, Tokio, `tracing`, and a Cargo workspace. Treat the repository documentation as policy, not background reading.

## Read First

Before reviewing code, read the relevant specs:

- `docs/README.md`
- `docs/architecture/overview.md`
- `docs/architecture/workspace-layout.md`
- `docs/architecture/dependency-rules.md`
- `docs/architecture/state-management.md`
- `docs/architecture/async-architecture.md`
- `docs/architecture/event-flow.md`
- `docs/architecture/native-integration.md`
- `docs/conventions/rust-style.md`
- `docs/conventions/error-handling.md`
- `docs/conventions/async.md`
- `docs/conventions/logging.md`
- `docs/conventions/testing.md`
- `docs/conventions/ui.md`
- `docs/conventions/state.md`
- `docs/conventions/security.md`
- `docs/conventions/ai-contributing.md`

If the change touches only one area, still read the overview, dependency rules, and the specific convention document for that area.

## Review Stance

Prioritize bugs, regressions, security issues, architecture violations, missing tests, and async/UI responsiveness risks.

Do not spend review budget on cosmetic preferences already covered by `rustfmt`, Clippy, or existing local style unless the style issue hides a real maintainability problem.

## Output Format

Return findings first, ordered by severity.

Use this structure:

```md
## Findings

- [severity] `path:line` Short issue title
  Explain the concrete problem, why it matters, and what should change.

## Open Questions

- Question or assumption that affects correctness.

## Verification

- Commands reviewed or recommended.
- Tests missing or not run.

## Summary

Brief change summary only after findings.
```

If there are no findings, say so directly and still mention residual risk or missing verification.

## Severity Guide

- `critical`: data loss, credential leak, remote code execution, broken startup, unsafe update path.
- `high`: architecture boundary violation, UI freeze, uncancellable task leak, auth/session bug, persistent state corruption.
- `medium`: incorrect state transition, missing error handling, missing test for meaningful behavior, poor retry/backoff behavior.
- `low`: maintainability issue, unclear naming, small observability gap, narrow documentation mismatch.

## Architecture Checks

Check dependency direction:

```text
mezon-app     composes crates
mezon-ui      GPUI views, components, view models
mezon-store   domain state and persistence models
mezon-client  REST/auth/session/transport
mezon-native  tray, deep links, notifications, OS APIs
mezon-updater update metadata and checks
mezon-proto   protobuf-facing boundary types
```

Flag these patterns:

- `mezon-store` depends on `mezon-ui`
- `mezon-client` depends on `mezon-ui`
- `mezon-native` depends on `mezon-ui`
- platform APIs inside `mezon-ui`
- REST calls directly from GPUI views
- protobuf DTOs passed into UI components
- circular crate dependencies

## State And Events

Prefer domain-local stores and events.

Good:

```rust
pub enum WorkspaceEvent {
    ChannelSelected(ChannelId),
    MessageReceived {
        channel_id: ChannelId,
        message: Message,
    },
}
```

Expected placement:

```text
crates/mezon-store/src/workspace/event.rs
crates/mezon-store/src/session/event.rs
crates/mezon-store/src/settings/event.rs
```

Flag:

- global `Arc<Mutex<AppState>>` as the main architecture
- boolean state machines such as `is_loading`, `is_authenticated`, `has_error`
- public mutable store fields
- untyped string or JSON event buses
- UI-owned domain state

Prefer:

- enums over boolean state
- newtypes for IDs and tokens
- typed actions for intent
- typed events for facts
- store methods for state transitions

## Async Checks

Flag:

- blocking IO or CPU work in GPUI render paths
- locks held across `.await`
- detached `tokio::spawn` loops without cancellation
- unbounded channels without justification
- retry loops without backoff, jitter, limits, or cancellation
- websocket loops that mutate UI state directly

Good:

```rust
tokio::select! {
    _ = cancel.cancelled() => return Ok(()),
    result = connect_once(&mut events) => result?,
}
```

Bad:

```rust
let mut state = state.lock().await;
let user = client.fetch_user(id).await?;
state.users.insert(id, user);
```

## GPUI Checks

Views may render state, own local interaction state, and emit typed actions.

Views must not:

- call HTTP APIs directly
- parse protobuf messages
- access keychain, registry, notification APIs, or platform windows directly
- perform disk or network IO during render
- own global application state

Flag slow or nondeterministic render code.

## Error Handling

Flag:

- `unwrap`, `expect`, or `panic` in production paths
- `Result<T, String>` for domain or service APIs
- erased errors without context
- user-facing raw transport/platform errors

Prefer typed errors and boundary context.

## Logging And Security

Flag logs containing:

- auth tokens
- refresh tokens
- cookies
- full private message bodies
- raw deep-link auth callbacks
- keychain contents

Prefer structured `tracing` fields:

```rust
tracing::warn!(error = %error, "session refresh failed");
```

## Testing Expectations

Expect tests for:

- store transitions
- DTO-to-domain conversion
- settings persistence and migration
- retry/cancellation behavior
- auth/session behavior
- deep-link parsing
- security-sensitive parsing
- view model derivation

Recommended commands:

```sh
cargo fmt --all -- --check
cargo clippy -p <crate> -- -D warnings
cargo nextest run -p <crate>
cargo deny check
```

If workspace Clippy fails because of vendored crates, report that separately from app-owned findings.

## Forbidden Review Advice

Do not recommend broad rewrites without identifying the smallest safe change.

Do not suggest introducing:

- global service locators
- generic untyped event buses
- UI-to-client direct calls
- platform-specific UI modules
- broad `#[allow]` attributes to silence real app-owned issues
- new dependencies when existing workspace dependencies cover the need

## Final Review Rule

Every finding must be actionable and grounded in a file and line number. If a concern is speculative, put it under `Open Questions` instead of `Findings`.
