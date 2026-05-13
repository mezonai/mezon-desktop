# Async Architecture

Mezon Desktop uses Tokio for network and background work. GPUI rendering must remain responsive; never block the UI thread with network, disk, crypto, decompression, or long CPU work.

## Async Boundaries

Spawn async work at ownership boundaries:

- `mezon-app`: starts top-level services.
- `mezon-client`: owns request futures and websocket tasks.
- `mezon-store`: may consume event streams, but should not hide unbounded task trees.
- `mezon-ui`: may trigger commands, but should not own long-running service loops.

## Spawn Pattern

Good:

```rust
pub struct TaskHandle {
    cancel: tokio_util::sync::CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

impl TaskHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
```

Bad:

```rust
tokio::spawn(async move {
    loop {
        sync().await.unwrap();
    }
});
```

Detached infinite tasks are shutdown bugs.

## Cancellation

Every long-running task needs a cancellation path.

```rust
pub async fn run_ws_loop(
    cancel: CancellationToken,
    mut events: mpsc::Sender<ClientEvent>,
) -> Result<(), ClientError> {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            result = connect_once(&mut events) => {
                result?;
            }
        }
    }
}
```

Cancellation should be cooperative and fast. Do not wait for a long retry sleep without selecting on cancellation.

## Do Not Hold Locks Across Await

Bad:

```rust
let mut state = state.lock().await;
let user = client.fetch_user(id).await?;
state.users.insert(id, user);
```

Good:

```rust
let user = client.fetch_user(id).await?;

let mut state = state.lock().await;
state.users.insert(id, user);
```

Prefer store-owned mutation methods over exposing locks.

## Channel Usage

Use bounded channels by default:

```rust
let (tx, rx) = tokio::sync::mpsc::channel::<ClientEvent>(256);
```

Use channel types intentionally:

- `mpsc`: command or event queue with one consumer.
- `watch`: latest state snapshot.
- `broadcast`: fan-out events where lag can be handled.
- `oneshot`: request/response completion.

Avoid unbounded channels unless the producer is naturally bounded and documented.

## Retry Strategy

Network retries must include:

- maximum attempts or bounded retry window
- exponential backoff with jitter
- cancellation
- classification of retryable errors
- tracing fields for attempt and delay

```rust
#[tracing::instrument(skip(client, cancel), fields(attempt))]
pub async fn fetch_with_retry(
    client: &ApiClient,
    cancel: CancellationToken,
) -> Result<Session, ClientError> {
    let mut delay = Duration::from_millis(250);

    for attempt in 1..=5 {
        tracing::Span::current().record("attempt", attempt);

        match client.fetch_session().await {
            Ok(session) => return Ok(session),
            Err(error) if error.is_retryable() => {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(ClientError::Cancelled),
                    _ = tokio::time::sleep(delay) => {}
                }
                delay = (delay * 2).min(Duration::from_secs(5));
            }
            Err(error) => return Err(error),
        }
    }

    Err(ClientError::RetryExhausted)
}
```

## Websocket Reconnects

Websocket loops should separate:

- connection establishment
- authentication
- inbound message decoding
- outbound command sending
- reconnect policy
- shutdown

Do not mix UI state mutation into the websocket loop. Emit typed client events and let stores apply them.

## Blocking Work

Use `spawn_blocking` only for unavoidable blocking work:

```rust
let parsed = tokio::task::spawn_blocking(move || parse_large_file(bytes))
    .await
    .map_err(ClientError::Join)??;
```

Do not use `spawn_blocking` as a way to hide slow design. Measure first when possible.
