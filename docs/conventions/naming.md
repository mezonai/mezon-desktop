# Naming

Names should make ownership, domain, and side effects clear.

## Types

Use domain names:

```rust
ChannelId
WorkspaceStore
SessionToken
NotificationPermission
```

Avoid vague names:

```rust
Data
Manager
Helper
Info
```

`Manager` is allowed only when the type genuinely coordinates multiple resources with lifecycle ownership.

## Enums

Name enum variants as states or facts:

```rust
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting { attempt: u32 },
}
```

Avoid boolean pairs:

```rust
is_connected: bool,
is_reconnecting: bool,
```

## Functions

Use verbs that expose cost and behavior:

- `load_settings`: may do IO.
- `parse_deep_link`: CPU-only conversion.
- `spawn_sync_task`: starts background work.
- `apply_event`: deterministic state transition.
- `view_model`: read-only projection.

Avoid hiding IO behind getters:

```rust
// Bad
fn session(&self) -> Result<Session, Error>;

// Good
async fn load_session(&self) -> Result<Session, Error>;
```

## Modules

Module names should be singular unless they are collections of peers:

```text
session/
workspace/
notification/
components/
views/
```

## Tests

Test names should describe behavior:

```rust
#[test]
fn expired_session_transitions_to_unauthenticated() {}
```

Avoid:

```rust
#[test]
fn test_session() {}
```
