# ADR: <Title>

- Status: Proposed
- Date: YYYY-MM-DD
- Owners: <names or team>
- Related: <issues, PRs, docs>

## Context

Describe the problem and constraints. Include relevant crate boundaries, platform concerns, async behavior, security implications, and performance constraints.

## Decision

State the decision directly.

Example:

```text
`mezon-store` will own workspace state transitions. `mezon-ui` will consume view models and emit typed actions. Network commands will be executed by `mezon-client` and returned as typed events.
```

## Consequences

List expected outcomes and tradeoffs.

- Positive:
- Negative:
- Neutral:

## Alternatives Considered

Document serious alternatives and why they were not chosen.

## Implementation Notes

Include crate-level changes, dependency changes, migration steps, and testing requirements.

## Review Checklist

- Does this preserve dependency direction?
- Does this avoid global mutable state?
- Does this keep GPUI responsive?
- Are async tasks cancellable?
- Are errors typed?
- Are secrets protected?
- Are tests and migration steps defined?
