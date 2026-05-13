# Rust Style

Follow idiomatic Rust, Rust API Guidelines, and Clippy. Prefer clear ownership and small public APIs over clever abstractions.

## Baseline

- Edition: Rust 2024.
- Format with `cargo fmt --all`.
- Lint with `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
- Avoid `unsafe` unless isolated, documented, and reviewed.
- Avoid `unwrap`, `expect`, and `panic` in production paths.

## Ownership

Good:

```rust
pub fn set_session(&mut self, session: Session) {
    self.session = SessionState::Authenticated(session);
}
```

Bad:

```rust
pub fn set_session(&self, session: Session) {
    self.inner.lock().unwrap().session = Some(session);
}
```

Use interior mutability only when the type's semantics require it.

## API Design

Good:

```rust
pub struct MessageBody(String);

impl MessageBody {
    pub fn new(value: impl Into<String>) -> Result<Self, MessageBodyError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(MessageBodyError::Empty);
        }
        Ok(Self(value))
    }
}
```

Bad:

```rust
pub fn send_message(body: String) {
    // validation somewhere later
}
```

Validate at boundaries and carry valid types inward.

## Imports

Prefer explicit imports. Avoid large glob imports except in tests or module prelude files.

```rust
use std::time::Duration;

use tracing::{debug, instrument};
```

## Comments

Comment why something exists, not what obvious code does.

Good:

```rust
// macOS can deliver the same activation URL twice after a cold start.
// Deduplicate before emitting a deep-link event.
```

Bad:

```rust
// Set seen to true.
seen = true;
```
