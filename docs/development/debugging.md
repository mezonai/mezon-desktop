# Debugging

Debugging should preserve observability and avoid adding temporary global state.

## Logging

Run with `RUST_LOG` filters:

```sh
RUST_LOG=mezon_app=debug,mezon_client=trace,mezon_store=debug just run
```

Use structured fields:

```rust
tracing::debug!(
    channel_id = %channel_id,
    pending = pending_messages,
    "rendering channel"
);
```

Remove noisy temporary logs before merging or lower them to `trace`.

## Async Issues

For hangs:

- check for locks held across `.await`
- check for unbounded channel growth
- check for tasks without cancellation
- check whether `select!` branches can be starved
- check whether UI work is blocked on IO

Bad:

```rust
let guard = store.lock().await;
client.sync().await?;
drop(guard);
```

Good:

```rust
let snapshot = store.snapshot();
let result = client.sync(snapshot).await?;
store.apply(result);
```

## UI Issues

For GPUI rendering bugs:

- reduce the issue to a view model state
- check local view state versus domain state
- verify actions are emitted once
- verify expensive work is not inside render
- check theme tokens and layout constraints

## Native Issues

Debug native integration in `mezon-native` first. Do not patch around platform failures in UI code.

Represent platform capability failures as typed errors:

```rust
NativeError::NotificationPermissionDenied
```

## Dependency Issues

Use:

```sh
cargo tree -p mezon-app
cargo tree -i <crate>
cargo deny check
```

Dependency graph problems are architecture problems. Fix direction rather than adding more glue.
