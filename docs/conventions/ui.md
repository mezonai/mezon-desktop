# GPUI UI Conventions

`mezon-ui` owns presentation. GPUI views should be composed from small primitives and components, with domain behavior delegated to stores and services.

## Layers

```text
views/        screens and panels
components/  reusable domain UI
primitives/  buttons, inputs, lists, popovers
theme/       colors, typography, spacing
view_model/  UI-facing projections
```

## View Boundaries

Views may:

- render state
- emit typed actions
- own local interaction state such as focus or hover
- subscribe to store-derived view models

Views must not:

- call REST APIs directly
- parse protobuf DTOs
- access platform APIs
- perform blocking work
- own global domain state

## Good View Shape

```rust
pub struct ChannelListView {
    workspace: Entity<WorkspaceViewModel>,
}

impl Render for ChannelListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render from view model and emit actions.
    }
}
```

## Bad View Shape

```rust
pub struct ChannelListView {
    client: ApiClient,
    channels: Arc<Mutex<Vec<proto::Channel>>>,
}
```

This couples rendering to transport, protobuf, and shared mutable state.

## Action Flow

UI actions should be typed:

```rust
pub enum UiAction {
    Workspace(WorkspaceAction),
    Session(SessionAction),
    Native(NativeAction),
}
```

Avoid stringly typed callbacks.

## Responsive UI

Render methods must be fast and deterministic. Expensive work belongs in stores or background tasks.

Bad:

```rust
fn render(...) -> impl IntoElement {
    let image = std::fs::read(path).unwrap();
    // ...
}
```

Good:

```rust
fn render(...) -> impl IntoElement {
    match self.avatar_cache.get(user_id) {
        Some(image) => avatar(image),
        None => avatar_placeholder(),
    }
}
```

## Theme

Use theme tokens rather than ad hoc colors and spacing. Components should accept semantic variants instead of raw style bags where possible.

```rust
Button::new("send")
    .variant(ButtonVariant::Primary)
```

## Platform UI

Do not place platform-specific UI behavior in components. Route platform capability differences through view models or native services.
