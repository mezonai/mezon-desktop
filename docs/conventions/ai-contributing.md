# AI Contributing

AI-generated changes must follow the same architecture and review standards as human-written code. The goal is not merely compiling code; it is code that fits Mezon Desktop.

## Required Behavior

AI agents must:

- inspect existing code before editing
- preserve crate boundaries
- prefer small, reviewable patches
- use existing patterns and dependencies
- run focused checks when possible
- explain unresolved risks
- avoid reverting unrelated user changes

## Forbidden Patterns

Do not introduce:

```rust
Arc<Mutex<AppState>>
```

as the default state architecture.

Do not put platform code in `mezon-ui`.

Do not pass `mezon-proto` DTOs into GPUI components.

Do not use `unwrap` in production paths.

Do not spawn detached infinite tasks.

Do not add broad dependencies without checking existing workspace dependencies.

## Expected Patch Shape

Good:

- one crate boundary adjusted intentionally
- typed action/event added
- store transition tested
- UI consumes a view model
- async task has cancellation

Bad:

- generated generic framework
- large cross-crate rewrite without tests
- new global service locator
- untyped JSON event bus
- UI calls HTTP client directly

## Review Checklist For AI Code

Reviewers should ask:

- Does this preserve dependency direction?
- Are invalid states represented with enums or types?
- Are errors typed and propagated with context?
- Are async tasks cancellable?
- Are locks held across `.await`?
- Are secrets protected from logs and UI?
- Did tests cover behavior instead of implementation details?
- Did the agent run `cargo fmt`, Clippy, or focused tests?

## Instructions For AI Tools

Claude Code, Cursor, Copilot, and ChatGPT should use these docs as repository policy.

When changing code:

1. Read the crate `Cargo.toml` and nearby modules.
2. Identify the owning crate for the behavior.
3. Keep the UI, store, client, native, updater, and proto boundaries intact.
4. Implement the smallest complete behavior.
5. Add or update tests where the behavior can regress.
6. Run the narrowest useful verification command.
7. Report commands run and any skipped checks.

## Prompting Guidance

Good task prompt:

```text
Add typed cancellation to the websocket reconnect task in mezon-client.
Do not change GPUI views. Add tests for cancellation and retry exhaustion.
```

Risky task prompt:

```text
Refactor app state and make it cleaner.
```

Ask for explicit crate scope, behavior, and verification expectations.
