# Testing

Tests should verify behavior at crate boundaries and protect architecture from regression.

## Commands

```sh
just test
cargo nextest run -p mezon-store
cargo test -p mezon-client auth
```

Before merging:

```sh
just lint
just safety
```

## What To Test

- store transitions
- DTO to domain conversions
- settings persistence and migration
- retry and cancellation behavior
- security-sensitive parsing
- native boundary fallbacks with mocked adapters
- view model derivation

## Store Test Example

```rust
#[test]
fn selecting_channel_updates_current_channel() {
    let mut store = WorkspaceStore::default();
    let channel_id = ChannelId::new("general");

    store.apply(WorkspaceEvent::ChannelSelected(channel_id.clone()));

    assert_eq!(store.selected_channel(), Some(&channel_id));
}
```

## Async Test Example

```rust
#[tokio::test]
async fn cancelled_retry_returns_cancelled() {
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = fetch_with_retry(&client, cancel).await;

    assert!(matches!(result, Err(ClientError::Cancelled)));
}
```

## UI Tests

Prefer testing view models and interaction reducers before snapshotting GPUI output. GPUI rendering tests are useful for regressions but should not be the only validation.

## Anti-Patterns

Avoid tests that mirror implementation:

```rust
assert_eq!(store.channels.len(), 1);
assert!(store.channels.contains_key(&id));
```

Prefer observable behavior:

```rust
assert_eq!(store.channel_title(&id), Some("general"));
```

Avoid sleeps in async tests. Use channels, time control, or explicit synchronization.
