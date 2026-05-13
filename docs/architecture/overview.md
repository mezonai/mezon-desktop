# Architecture Overview

Mezon Desktop is a native desktop client organized as a Rust Cargo workspace. The architecture separates GPUI rendering, domain state, network access, persistence, native OS integration, update logic, and protocol-facing types.

## Crates

```text
crates/
├── mezon-app/      Binary entry point, GPUI bootstrap, runtime wiring
├── mezon-ui/       GPUI views, components, theme, UI composition
├── mezon-client/   REST client, auth, sessions, keychain adapters
├── mezon-store/    Persistent settings and domain state models
├── mezon-native/   Tray, deep links, notifications, single instance, OS APIs
├── mezon-updater/  Update checks and update metadata
└── mezon-proto/    Protobuf-facing types and conversions
```

## Runtime Shape

```text
OS event loop
    ↓
mezon-app
    ├── initializes tracing
    ├── initializes Tokio runtime or task executor bridge
    ├── opens GPUI windows
    ├── wires stores, client, native services
    └── owns top-level app lifecycle

GPUI views
    ↓ actions/events
domain stores
    ↓ commands
client/native/updater services
    ↓ results/events
domain stores
    ↓ subscriptions
GPUI views
```

The UI should observe state and emit actions. It should not directly own network sessions, platform handles, or persistence backends.

## Core Rules

- `mezon-app` composes crates; it should contain minimal business logic.
- `mezon-ui` owns presentation and GPUI composition only.
- `mezon-store` owns domain state and state transitions.
- `mezon-client` owns transport, auth/session mechanics, and DTO conversion.
- `mezon-native` owns platform-specific APIs behind stable Rust traits or services.
- `mezon-updater` owns update-checking policy and metadata.
- `mezon-proto` is a boundary crate, not a global model crate.

## Preferred Flow

```rust
pub enum WorkspaceAction {
    SelectChannel(ChannelId),
    SendMessage { channel_id: ChannelId, body: MessageBody },
}

pub enum WorkspaceEvent {
    ChannelSelected(ChannelId),
    MessageQueued(MessageId),
    MessageSendFailed { message_id: MessageId, error: SendMessageError },
}
```

Actions represent user or system intent. Events represent facts that already happened. Stores apply events and notify views.

## Anti-Patterns

Avoid:

```rust
// Bad: global mutable state becomes the architecture.
type SharedAppState = Arc<Mutex<AppState>>;
```

Prefer:

```rust
// Good: state is owned by a domain store with explicit updates.
pub struct WorkspaceStore {
    selected_channel: Option<ChannelId>,
    channels: BTreeMap<ChannelId, Channel>,
}
```

Avoid passing API DTOs directly into GPUI components:

```rust
// Bad: UI is coupled to wire format.
pub fn render_user(user: mezon_proto::UserResponse) -> impl IntoElement { ... }
```

Prefer UI-facing models:

```rust
pub struct UserViewModel {
    pub id: UserId,
    pub display_name: SharedString,
    pub avatar: Option<AvatarUrl>,
}
```

## Design Bias

Mezon Desktop should favor simple, explicit Rust over broad framework patterns. Introduce abstractions only when they preserve crate boundaries, remove real duplication, or make ownership easier to reason about.
