# Dependency Rules

Crate dependencies define architecture. Review dependency changes as design changes.

## Allowed Direction

```text
mezon-app
 ├── mezon-ui
 ├── mezon-store
 ├── mezon-client
 ├── mezon-native
 └── mezon-updater

mezon-ui
 ├── mezon-store      view models and domain ids only
 └── gpui

mezon-store
 └── mezon-proto      only at conversion boundaries, if needed

mezon-client
 └── mezon-proto

mezon-native
 └── platform crates

mezon-updater
 └── mezon-client or HTTP dependencies, if needed
```

`mezon-app` is the composition root. Lower-level crates must not depend on it.

## Forbidden Direction

```text
mezon-store  ─X→ mezon-ui
mezon-client ─X→ mezon-ui
mezon-native ─X→ mezon-ui
mezon-proto  ─X→ any Mezon crate
```

`mezon-ui` must not become the place where domain behavior lives. `mezon-proto` must not import application types.

## Dependency Review Checklist

Before adding a dependency:

- Is it needed in this crate, or only at the composition root?
- Does it pull async runtime, platform, or UI concerns into a domain crate?
- Is the dependency already available in `[workspace.dependencies]`?
- Does it duplicate functionality from the standard library or an existing crate?
- Does `cargo deny check` allow it?

## Good Example

```toml
[dependencies]
mezon-store.workspace = true
gpui.workspace = true
tracing.workspace = true
```

The UI depends on store types and GPUI.

## Bad Example

```toml
[dependencies]
mezon-ui.workspace = true
windows.workspace = true
```

This is bad in `mezon-store`: a domain crate should not depend on presentation or platform APIs.

## DTO Boundary

Convert wire types near the transport boundary.

```rust
impl TryFrom<proto::User> for User {
    type Error = UserDecodeError;

    fn try_from(value: proto::User) -> Result<Self, Self::Error> {
        Ok(Self {
            id: UserId::parse(value.id)?,
            display_name: DisplayName::new(value.display_name)?,
        })
    }
}
```

Do not let generated or protobuf-facing structures leak across the workspace.

## Feature Flags

Use feature flags for optional integration points, not for arbitrary architecture switches.

Good:

```toml
[features]
default = []
native-notifications = []
```

Bad:

```toml
[features]
new-architecture = []
```

Architecture should be explicit in code and docs, not hidden behind vague feature names.
