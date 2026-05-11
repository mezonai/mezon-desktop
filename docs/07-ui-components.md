# UI Components

This document covers the UI layer: the theme system, primitive components, composition components, and how `RootView` assembles them.

## Overview

All UI code lives in the `mezon-ui` crate:

```
mezon-ui/src/
├── lib.rs                     # Crate root, re-exports
├── theme.rs                   # Theme struct with color tokens
├── root.rs                    # RootView — top-level window view
├── login_view.rs              # LoginView — Stage 1 auth screen
├── title_bar.rs               # Custom window title bar
├── account_test_view.rs       # AccountTestView — post-login test view
└── components/
    ├── primitives/            # Low-level building blocks
    │   ├── avatar.rs          # User avatar with presence indicator
    │   ├── badge.rs           # Small numeric/dot badge
    │   ├── button.rs          # Button with variants and sizes
    │   ├── divider.rs         # Horizontal or vertical rule
    │   ├── icon.rs            # SVG icon renderer
    │   ├── label.rs           # Styled text label
    │   ├── spinner.rs         # Loading spinner animation
    │   └── text_input.rs      # Single-line text input with masking
    └── compositions/          # Higher-level combinations
        ├── empty_state.rs     # "Nothing here" placeholder
        ├── form_field.rs      # Label + TextInput pair
        ├── icon_button.rs     # Clickable icon
        ├── section_header.rs  # Section title with optional action
        ├── status_dot.rs      # Colored presence dot
        └── user_chip.rs       # Avatar + name inline pill
```

---

## 1. Theme System (`theme.rs`)

### Design Philosophy

The theme mirrors Discord/Mezon's visual language. All colors are defined once as named tokens; UI code references `theme.bg_primary` rather than hardcoding `rgba(49, 51, 56, 1.0)`. This makes future theme changes a single-file edit.

### Theme Struct

```rust
pub struct Theme {
    // Backgrounds — four levels of depth
    pub bg_primary: Rgba,    // #313338 — main canvas
    pub bg_secondary: Rgba,  // #2b2d31 — sidebar panels
    pub bg_tertiary: Rgba,   // #1e1f22 — clan sidebar (deepest)
    pub bg_floating: Rgba,   // #111214 — modals, tooltips
    pub bg_hover: Rgba,      // rgba(255,255,255,0.06)

    // Text hierarchy
    pub text_primary: Rgba,   // #f2f3f5 — headings, active items
    pub text_secondary: Rgba, // #b5bac1 — body text
    pub text_muted: Rgba,     // #80848e — timestamps, hints
    pub text_link: Rgba,      // #00aff4 — clickable links

    // Interactive elements
    pub interactive_normal: Rgba,  // default icon/text
    pub interactive_hover: Rgba,   // on hover
    pub interactive_active: Rgba,  // on press/selected

    // Brand
    pub brand: Rgba,       // #5865f2 — primary buttons, accents
    pub brand_hover: Rgba, // #4752c4

    // User presence status
    pub status_online: Rgba,  // #23a55a green
    pub status_idle: Rgba,    // #f0b232 yellow
    pub status_dnd: Rgba,     // #f23f43 red
    pub status_offline: Rgba, // #80848e grey

    // Notification indicators
    pub unread_dot: Rgba,    // white dot on channel row
    pub mention_badge: Rgba, // red badge count

    pub border: Rgba,        // rgba(255,255,255,0.08) — subtle dividers
    pub title_bar_bg: Rgba,  // #1e1f22
}
```

### Using the Theme

Currently, every view calls `Theme::dark()` locally:

```rust
fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = Theme::dark();
    div().bg(theme.bg_primary)...
}
```

`Theme::light()` exists but is not yet wired up — `RootView` ignores `Settings.theme` for now. Connecting them is a Stage 2 task.

### Color Format

Colors are stored as GPUI's `Rgba { r, g, b, a }` with all channels as `f32` in the range `0.0–1.0`. The private `rgba(r, g, b, a)` helper converts `u8` RGB + `f32` alpha to this format.

---

## 2. Primitive Components

Primitives are pure visual building blocks. They do not own entities — they render to `AnyElement` given a `&Theme`.

### Button (`primitives/button.rs`)

The most complex primitive. Uses a builder pattern:

```rust
Button::new("Send OTP")
    .variant(ButtonVariant::Primary)   // Primary | Secondary | Ghost | Danger
    .size(ButtonSize::Md)              // Xs | Sm | Md | Lg
    .icon(IconName::Send)              // optional leading icon
    .loading(self.loading)             // shows Spinner instead of label
    .disabled(self.loading)            // 50% opacity, no cursor
    .full_width()                      // w-full
    .on_click(|window, cx| { ... })
    .render(&theme)                    // → AnyElement
```

**Variant colors:**
- `Primary` — brand purple fill, white text
- `Secondary` — transparent fill, border, primary text
- `Ghost` — transparent fill, no border, secondary text
- `Danger` — red fill, white text

**Interactive states:** When neither disabled nor loading, the button adds `cursor_pointer()` and a hover background. When disabled it renders at 50% opacity. When loading it shows a `Spinner` in place of the label and applies 80% opacity.

**Element ID:** Uses `#[track_caller]` so each `Button::new()` call site gets a unique `ElementId` automatically. Override with `.id(my_id)` when you need a stable ID (e.g., in a list).

### TextInput (`primitives/text_input.rs`)

A stateful GPUI entity wrapping a single-line text input:

```rust
let input = cx.new(|cx| TextInput::new(cx, "email-field").placeholder("Email address"));
input.update(cx, |t, _| t.masked = true); // for password fields
```

Key fields: `value` (current text), `masked` (renders dots), `error` (red border), `placeholder`, `on_change` callback.

### Label (`primitives/label.rs`)

```rust
Label::new("Status")
    .size(LabelSize::Sm)          // Xs | Sm | Md | Lg
    .weight(LabelWeight::Bold)    // Normal | Medium | Semibold | Bold
    .color(theme.text_secondary)
    .render(&theme)
```

### Avatar (`primitives/avatar.rs`)

```rust
Avatar::new()
    .url("https://cdn.mezon.ai/avatars/user123.png")  // or None for initials
    .size(AvatarSize::Md)                              // Xs | Sm | Md | Lg
    .presence(PresenceStatus::Online)                  // adds status dot overlay
    .render(&theme)
```

`PresenceStatus` maps to the theme's status colors: `Online` → `status_online`, `Idle` → `status_idle`, `DoNotDisturb` → `status_dnd`, `Offline` → `status_offline`.

### Icon (`primitives/icon.rs`)

Renders SVG icons from the embedded icon set:

```rust
Icon::new(IconName::Search)
    .size(16.0)
    .color(theme.interactive_normal)
    .render(&theme)
```

`IconName` is an enum of all available icons (e.g., `Search`, `Settings`, `Bell`, `Add`, `Close`, `Chevron`, etc.).

### Badge (`primitives/badge.rs`)

Small number or dot overlaid on another element (e.g., unread count):

```rust
Badge::new().count(3).render(&theme)   // shows "3"
Badge::new().dot().render(&theme)       // shows a small dot (unread indicator)
```

### Spinner (`primitives/spinner.rs`)

An animated loading indicator:

```rust
Spinner::new()
    .size(14)                   // pixel size
    .color(theme.text_primary)
    .render()                   // → AnyElement
```

### Divider (`primitives/divider.rs`)

```rust
Divider::horizontal().render(&theme)  // thin horizontal rule
Divider::vertical().render(&theme)    // thin vertical rule
```

---

## 3. Composition Components

Compositions combine primitives into reusable patterns that carry their own GPUI entity state.

### FormField (`compositions/form_field.rs`)

The most used composition in the login screen. Wraps a `TextInput` with a label above it:

```rust
// Created as a GPUI entity
let field = cx.new(|cx| FormField::new(cx, "Email"));

// Configure after creation
field.update(cx, |f, cx| f.set_masked(cx));     // password masking
field.read(cx).value(cx)                         // read current text
field.update(cx, |f, cx| f.set_error(Some("Invalid".into()), cx));
```

Renders as:
```
EMAIL                    ← uppercase label in text_secondary
┌─────────────────────┐
│ email@example.com   │  ← TextInput with border
└─────────────────────┘
```

### StatusDot (`compositions/status_dot.rs`)

Standalone colored dot for presence indicators. Used separately from `Avatar` when you need just the dot:

```rust
StatusDot::new(PresenceStatus::Online).render(&theme)
```

### UserChip (`compositions/user_chip.rs`)

Inline pill with avatar + username. Used in mention lists, DM headers, etc.:

```rust
UserChip::new("alice", Some(avatar_url)).render(&theme)
```

### IconButton (`compositions/icon_button.rs`)

A clickable icon-only button (no text label):

```rust
IconButton::new(IconName::Settings)
    .tooltip("Settings")
    .on_click(|window, cx| { ... })
    .render(&theme)
```

### SectionHeader (`compositions/section_header.rs`)

Section title row with optional trailing action button:

```rust
SectionHeader::new("DIRECT MESSAGES")
    .action(IconName::Add, |window, cx| { ... })
    .render(&theme)
```

### EmptyState (`compositions/empty_state.rs`)

Centered placeholder for empty lists or loading states:

```rust
EmptyState::new()
    .icon(IconName::Chat)
    .title("No messages yet")
    .description("Start a conversation")
    .render(&theme)
```

---

## 4. View Layer

### RootView (`root.rs`)

The top-level view owns the window layout. It always renders:
1. `TitleBar` at the top
2. Content area below, driven by `AuthState`

```
┌──────────────────────────────────────┐
│ TitleBar (draggable, traffic lights) │
├──────────────────────────────────────┤
│                                      │
│        [content area]                │
│                                      │
│  NotAuthenticated → LoginView        │
│  OtpRequested    → LoginView         │
│  AwaitingCallback → "connecting..."  │
│  Authenticated   → AccountTestView  │
│                  │ (account info,     │
│                  │  clan name, list   │
│                  │  of channels from  │
│                  │  shared TCP        │
│                  │  transport)        │
└──────────────────────────────────────┘
```

`RootView` subscribes to `auth_state` implicitly — reading it inside `render()` means GPUI re-renders `RootView` whenever `auth_state` changes.

### TitleBar (`title_bar.rs`)

A custom window decoration that:
- Sets `draggable(true)` on the entire bar so users can move the window
- On macOS renders a blank area to the right of the traffic-light buttons
- Shows the app name "Mezon" centered
- Will gain navigation controls and user avatar in Stage 2

### LoginView (`login_view.rs`)

See the [Authentication Flow](./06-auth-flow.md) document for full details. From a component perspective, `LoginView` renders a centered card that adapts based on `method` and `otp_step`:

```
OTP step 0:          OTP step 1:          Password:
┌──────────┐         ┌──────────┐         ┌──────────┐
│  Mezon   │         │  Mezon   │         │  Mezon   │
│──────────│         │──────────│         │──────────│
│ Email    │         │ Enter    │         │ Email    │
│ [input]  │         │ code     │         │ [input]  │
│          │         │ [6 boxes]│         │ Password │
│[Send OTP]│         │[Verify]  │         │ [input]  │
│    or    │         │60s timer │         │[Sign In] │
│[Password]│         │← Back    │         │    or    │
└──────────┘         └──────────┘         │  [OTP]   │
                                          └──────────┘
```

---

## 5. Component Patterns

### Builder Pattern for Non-Entity Components

Primitives use a builder pattern terminated by `.render(&theme)` which returns `AnyElement`. This allows method chaining without requiring GPUI entity overhead:

```rust
// ✓ Correct: chain + render
Button::new("Save").variant(ButtonVariant::Primary).on_click(...).render(&theme)

// ✗ Wrong: Button is not a GPUI view, can't use cx.new()
```

### Entity Components for Stateful Inputs

Components that need to track mutable state (like `TextInput`, `FormField`) must be GPUI entities:

```rust
// Create once in the parent's constructor
let email_field = cx.new(|cx| FormField::new(cx, "Email"));

// Read in render — returns a clone of the entity handle (cheap)
container.child(self.email_field.clone())

// Mutate from an event handler
self.email_field.update(cx, |field, cx| {
    field.set_error(Some("Invalid email".into()), cx);
});
```

### No Global Theme Context (Yet)

Because GPUI doesn't have React Context, every `render()` creates a local `Theme::dark()`. This is cheap (it's just struct construction on the stack). In Stage 2 this will likely move to a GPUI global (`cx.global::<Theme>()`).

---

## 6. AccountTestView

`AccountTestView` in `account_test_view.rs` is a post-login view. On first render, waits 3 seconds, then calls `AppApi::get_account()`, `AppApi::list_clan_descs()`, and `AppApi::list_channel_descs(&clan.clan_id)`. Displays user ID, username, email, display name, clan name, and channel list with labels/IDs/types. Uses shared `Arc<AppApi>` — no per-view transport connection boilerplate.
