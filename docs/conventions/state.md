# State Conventions

State should be explicit, domain-oriented, and easy to test without GPUI.

## Rules

- Prefer enums over boolean state.
- Prefer newtypes for ids, tokens, and domain-specific strings.
- Prefer immutable snapshots for UI read models.
- Prefer events over direct cross-crate mutation.
- Keep persistence models separate from UI models.

## State Machines

Good:

```rust
pub enum LoginState {
    Idle,
    Submitting,
    RequiresMfa(MfaChallenge),
    Authenticated(Session),
    Failed(LoginError),
}
```

Bad:

```rust
pub struct LoginState {
    loading: bool,
    mfa_required: bool,
    authenticated: bool,
    error: Option<String>,
}
```

## Derived State

Derived values should be computed from authoritative state or cached with invalidation rules.

```rust
pub fn unread_count(&self, channel_id: ChannelId) -> UnreadCount {
    self.messages
        .get(&channel_id)
        .map(|messages| messages.iter().filter(|message| !message.read).count())
        .unwrap_or_default()
        .into()
}
```

## Store API

Expose behavior, not fields:

```rust
impl SettingsStore {
    pub fn set_theme(&mut self, theme: ThemePreference) -> SettingsChanged {
        self.settings.theme = theme;
        SettingsChanged::Theme(theme)
    }
}
```

Avoid public mutable fields:

```rust
pub struct SettingsStore {
    pub settings: Settings,
}
```

## Persistence

Persist only durable user or application state. Do not persist transient UI details unless product behavior requires it.

Durable:

- auth session metadata
- selected workspace
- theme preference
- notification preference

Transient:

- hover state
- in-flight request flags
- temporary error banners
