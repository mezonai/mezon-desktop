# State Management

State is organized by domain, not by UI tree shape. The application should avoid a single global `AppState` guarded by `Arc<Mutex<_>>`.

## Store Responsibilities

A store owns:

- domain state
- validation and transitions
- persistence coordination
- event emission
- derived read models

A store should not own:

- GPUI rendering
- raw API DTOs
- OS handles
- long-running tasks without cancellation

## Domain Store Example

```rust
pub struct WorkspaceStore {
    selected_channel: Option<ChannelId>,
    channels: BTreeMap<ChannelId, Channel>,
    messages: BTreeMap<ChannelId, Vec<Message>>,
}

impl WorkspaceStore {
    pub fn apply(&mut self, event: WorkspaceEvent) {
        match event {
            WorkspaceEvent::ChannelSelected(id) => {
                self.selected_channel = Some(id);
            }
            WorkspaceEvent::MessageReceived { channel_id, message } => {
                self.messages.entry(channel_id).or_default().push(message);
            }
        }
    }
}
```

## Prefer Enums Over Booleans

Bad:

```rust
pub struct SessionState {
    pub is_loading: bool,
    pub is_authenticated: bool,
    pub has_error: bool,
}
```

Good:

```rust
pub enum SessionState {
    Unknown,
    Loading,
    Authenticated(Session),
    Unauthenticated,
    Failed(SessionError),
}
```

Enums make invalid states unrepresentable.

## Prefer Newtypes

Bad:

```rust
pub fn open_channel(id: String) { ... }
pub fn open_user(id: String) { ... }
```

Good:

```rust
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChannelId(String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct UserId(String);
```

Strong types prevent accidental cross-domain calls.

## Event-Driven Updates

Use events when multiple parts of the app react to a fact.

```rust
pub enum AppEvent {
    SessionChanged(SessionState),
    WorkspaceUpdated(WorkspaceEvent),
    NativeDeepLinkReceived(DeepLink),
}
```

Avoid direct mutation from views:

```rust
// Bad: UI reaches through multiple layers and mutates internals.
app_state.lock().unwrap().workspace.selected_channel = Some(id);
```

Prefer actions:

```rust
workspace_actions.send(WorkspaceAction::SelectChannel(id))?;
```

## Shared State

Shared ownership is allowed when it represents actual shared ownership. It is not a substitute for architecture.

Acceptable:

```rust
pub type SharedClient = Arc<ApiClient>;
```

Risky:

```rust
pub type SharedStores = Arc<Mutex<HashMap<String, Box<dyn Any>>>>;
```

## Persistence

Persistent models should be versioned and separate from UI models.

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct SettingsFileV1 {
    pub theme: ThemePreference,
    pub start_on_login: bool,
}
```

When changing persisted formats, write migrations and tests. Do not silently discard unknown user settings unless the migration explicitly documents why.
