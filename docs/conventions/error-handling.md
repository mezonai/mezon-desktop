# Error Handling

Use `Result` and typed errors. Error types are part of crate APIs and should be designed deliberately.

## Typed Errors

Good:

```rust
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session token is missing")]
    MissingToken,
    #[error("session token expired")]
    Expired,
    #[error("transport failed")]
    Transport(#[from] ClientError),
}
```

Bad:

```rust
fn refresh_session() -> Result<Session, String> { ... }
```

Use strings for display, not control flow.

## Context

Add context at boundaries:

```rust
let settings = repository
    .load()
    .await
    .map_err(SettingsStartupError::Load)?;
```

Avoid losing the source:

```rust
return Err(AppError::Other("failed".into()));
```

## Unwrap Policy

Allowed:

- tests
- examples where failure would make the example unreadable
- impossible invariants after a documented proof

Not allowed:

- network responses
- disk IO
- environment variables
- user input
- platform APIs
- channel sends in long-running tasks

Bad:

```rust
let token = keychain.load().await.unwrap();
```

Good:

```rust
let token = keychain.load().await.map_err(SessionError::Keychain)?;
```

## UI Errors

Views should receive user-safe error models, not raw transport errors.

```rust
pub enum LoginErrorView {
    InvalidCredentials,
    NetworkUnavailable,
    ServiceUnavailable,
}
```

Log detailed errors with `tracing`; display concise, non-sensitive messages.
