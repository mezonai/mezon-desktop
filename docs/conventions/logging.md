# Logging And Tracing

Use `tracing` for structured, production-ready observability. Logs should help diagnose user issues without exposing secrets.

## Spans

Instrument async and IO boundaries:

```rust
#[tracing::instrument(skip(client), fields(channel_id = %channel_id))]
pub async fn load_channel(
    client: &ApiClient,
    channel_id: ChannelId,
) -> Result<Channel, ClientError> {
    client.fetch_channel(channel_id).await
}
```

## Events

Use structured fields:

```rust
tracing::info!(
    workspace_id = %workspace_id,
    channel_count = channels.len(),
    "workspace loaded"
);
```

Avoid string interpolation:

```rust
tracing::info!("workspace {} loaded with {}", workspace_id, channels.len());
```

## Levels

- `error`: operation failed and needs attention.
- `warn`: degraded behavior or recoverable abnormal condition.
- `info`: lifecycle events and user-visible state changes.
- `debug`: development diagnostics.
- `trace`: high-volume details disabled by default.

## Secrets

Never log:

- auth tokens
- refresh tokens
- cookies
- keychain contents
- full private message bodies
- raw deep-link auth callbacks

Good:

```rust
tracing::warn!(error = %error, "session refresh failed");
```

Bad:

```rust
tracing::debug!(token = %token, "loaded token");
```

## Startup

`mezon-app` should initialize subscribers once. Honor environment filters where possible:

```rust
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```
