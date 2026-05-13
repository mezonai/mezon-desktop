# Async Conventions

Async code must be cancellation-aware, UI-safe, and explicit about ownership.

## Rules

- Do not block the UI thread.
- Do not hold locks across `.await`.
- Do not spawn detached tasks without a cancellation path.
- Use bounded channels by default.
- Use `tracing::instrument` on async service boundaries.
- Keep async work out of GPUI render methods.

## Good Spawn Boundary

```rust
#[tracing::instrument(skip(client, events, cancel))]
pub fn spawn_session_refresh(
    client: ApiClient,
    events: mpsc::Sender<SessionEvent>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let event = match client.refresh_session().await {
            Ok(session) => SessionEvent::RefreshSucceeded(session),
            Err(error) => SessionEvent::RefreshFailed(error),
        };

        if !cancel.is_cancelled() {
            let _ = events.send(event).await;
        }
    })
}
```

## Bad Spawn Boundary

```rust
tokio::spawn(async move {
    store.lock().await.refresh_from_network().await;
});
```

This mixes locking, store mutation, and network IO in one task.

## Timeouts

External IO should have timeouts.

```rust
let response = tokio::time::timeout(Duration::from_secs(10), client.fetch_user(id))
    .await
    .map_err(|_| ClientError::Timeout)??;
```

## UI Handoff

Async tasks should emit events. The UI should render store-derived state.

```rust
client_events.send(ClientEvent::UserLoaded(user)).await?;
```

Do not update GPUI elements directly from arbitrary Tokio tasks unless the GPUI runtime API explicitly provides a safe handoff.
