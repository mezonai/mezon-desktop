# Development Workflow

Work in small, behavior-focused changes. Architecture changes should be explicit and documented.

## Standard Loop

```sh
cargo fmt --all
just check
cargo nextest run -p <crate>
```

Before opening a PR:

```sh
just lint
just test
just safety
```

## Branch Scope

A good branch usually changes one of:

- one domain behavior
- one UI surface
- one native integration
- one persistence migration
- one client endpoint group
- one architecture decision

Avoid mixing broad formatting, dependency upgrades, and feature work in the same PR.

## Adding A Feature

1. Identify the owning crate.
2. Add or update domain types.
3. Add action/event/store transitions.
4. Add client/native/updater behavior if needed.
5. Add UI view models and GPUI composition.
6. Add tests near the most stable behavior boundary.
7. Run focused checks, then workspace checks.

## PR Expectations

A PR should include:

- concise description of behavior
- crate boundaries touched
- screenshots or recordings for UI changes when possible
- tests added or reason tests were not practical
- commands run
- security and migration notes when relevant

## Review Expectations

Reviewers should prioritize:

- dependency direction
- invalid state prevention
- async cancellation and responsiveness
- error handling
- state ownership
- security-sensitive data flow
- test coverage of behavior

Style comments should defer to `rustfmt`, Clippy, and existing project conventions.

## Clippy Policy

Clippy warnings are treated as design feedback. Prefer fixing the underlying issue. Use `#[allow]` only when:

- the lint is incorrect for this context
- the allow is local
- the reason is documented

```rust
#[allow(clippy::too_many_arguments)]
fn build_platform_menu(...) {
    // Platform menu construction mirrors the OS API shape.
}
```

Avoid crate-wide allows for convenience.
