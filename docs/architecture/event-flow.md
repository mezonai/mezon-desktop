# Event Flow

Mezon Desktop uses message passing and domain events to keep UI, state, networking, and native integration decoupled.

## Terms

- Action: intent requested by a user, view, or service.
- Command: work that may involve IO or async execution.
- Event: fact that has happened.
- Store update: deterministic application of an event to domain state.
- View model: read-only UI-facing projection of state.

## Standard Flow

```text
GPUI view
  emits Action
    ↓
action handler
  validates and starts Command
    ↓
client/native/updater
  returns Result or emits Event
    ↓
domain store
  applies Event
    ↓
view model
  notifies GPUI view
```

## Example

```rust
pub enum MessageAction {
    Send {
        channel_id: ChannelId,
        body: MessageBody,
    },
}

pub enum MessageEvent {
    Queued {
        local_id: LocalMessageId,
    },
    Accepted {
        local_id: LocalMessageId,
        remote_id: MessageId,
    },
    Failed {
        local_id: LocalMessageId,
        error: SendMessageError,
    },
}
```

This supports optimistic UI without letting the UI own transport state.

## Error Events

Errors that affect state should be events:

```rust
pub enum SessionEvent {
    RefreshStarted,
    RefreshSucceeded(Session),
    RefreshFailed(SessionError),
}
```

Errors that only affect a single command may remain a `Result`.

## Ordering

Stores should define ordering guarantees. For example:

- message events are applied per channel in server sequence order
- local optimistic messages are ordered by local monotonic counter
- stale session refresh responses are ignored if a newer refresh completed

Document ordering rules in the module that owns the store.

## Anti-Patterns

Avoid callback chains:

```rust
client.send_message(body, |result| {
    ui.update_message_status(result);
});
```

Prefer explicit events:

```rust
let event = command.send_message(channel_id, body).await?;
message_events.send(event)?;
```

Avoid stringly typed events:

```rust
Event::new("message_failed", serde_json::json!({ "id": id }))
```

Prefer typed enums:

```rust
MessageEvent::Failed { local_id, error }
```

## Backpressure

Events from services into stores should use bounded channels. When events can be dropped, document the drop behavior.

Good candidates for dropping:

- transient typing indicators
- progress updates superseded by newer progress
- presence refresh snapshots

Never silently drop:

- persisted settings changes
- auth state transitions
- message send completion
- security-sensitive events
