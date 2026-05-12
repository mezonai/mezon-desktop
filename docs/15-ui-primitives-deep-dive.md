# UI Primitives Deep Dive

This document covers the implementation details of the `Spinner` and `Icon` primitives — the two components that use GPUI features not seen elsewhere in the codebase: GPU-side animation transforms and inline SVG rendering.

For the full primitive catalogue and theme system, see [UI Components](./07-ui-components.md).

---

## 1. Spinner (`mezon-ui/src/components/primitives/spinner.rs`)

### What It Does

`Spinner` renders an animated loading ring. It uses GPUI's built-in `Animation` system to continuously rotate an SVG arc, producing a smooth spinning effect entirely in the GPU render pipeline.

### Builder API

```rust
Spinner::new()
    .size(24)                    // pixels, default 20
    .color(theme.text_secondary) // Rgba, default white (1.0, 1.0, 1.0, 1.0)
    .render()                    // → impl IntoElement
```

Note: `Spinner::render` takes no `&Theme` argument (unlike other primitives). It accepts an explicit `Rgba` directly, or falls back to solid white. This is because the spinner is typically used inside a button or overlay that already knows its own desired color.

### The SVG Arc

The spinner shape is a single SVG path — a 270° arc:

```
<path d="M12 4a8 8 0 0 1 7.938 7H21.95A10 10 0 1 0 12 22v-2a8 8 0 0 1 0-16Z"/>
```

This draws a ring with a 30° gap (the arc goes from approximately 12-o'clock clockwise, leaving a small visible break). The gap is what makes the rotation visually meaningful — if it were a full circle, you couldn't tell it was spinning.

The SVG uses `fill="currentColor"`, which means GPUI's `text_color()` method controls the icon color. GPUI renders SVGs by interpreting `currentColor` as the element's text color.

### GPUI Animation

The animation is applied with `AnimationExt::with_animation`:

```rust
svg()
    .path(SPINNER_SVG)
    .size(sz)
    .text_color(color)
    .with_animation(
        "spinner-rotation",       // stable identifier — used to resume on re-render
        Animation::new(Duration::from_millis(700)).repeat(),
        |el, progress| {          // closure called every frame with progress ∈ [0.0, 1.0)
            el.with_transformation(Transformation::rotate(radians(
                progress * std::f32::consts::TAU,  // TAU = 2π = full rotation
            )))
        },
    )
```

**Key details:**

- `Animation::new(Duration::from_millis(700))` — one complete rotation every 700ms (~86 RPM). No easing is applied; the default is linear, which is correct for a spinner.
- `.repeat()` — loops indefinitely rather than stopping at the end of one cycle.
- The string ID `"spinner-rotation"` is a stable key. GPUI uses it to resume the animation at the correct progress value across re-renders. If you use the same ID for two different spinners in the same view, they will share animation state.
- `progress` is a `f32` in the half-open range `[0.0, 1.0)`. Multiplying by `TAU` (2π radians) maps the full cycle to a full rotation.
- `Transformation::rotate(radians(...))` applies a 2D rotation transform. This happens on the GPU compositor — the CPU does not re-rasterize the SVG each frame.

### Default vs. Explicit Color

```rust
let color = self.color.unwrap_or(Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
```

White is the default because spinners typically appear inside colored buttons or dark overlays where white is the correct contrast choice. Call `.color(theme.brand_color)` to override.

### React Analogy

```tsx
// React equivalent (simplified)
function Spinner({ size = 20, color = "white" }) {
  return (
    <svg style={{ animation: "spin 700ms linear infinite", width: size, height: size }}>
      <path d="..." fill={color} />
    </svg>
  );
}
```

The key difference: GPUI's `Animation` runs the transform through the GPU render pipeline, not CSS. There is no CSS animation, no `requestAnimationFrame`, no `setInterval`. The progress value is computed by the GPUI runtime each frame and passed directly to the transformation closure.

---

## 2. Icon (`mezon-ui/src/components/primitives/icon.rs`)

### Design Decision: Inline SVG Strings

The icon system makes a deliberate choice: **every icon is an inline `&'static str` constant** embedded in the binary at compile time. There are no icon files to load, no font files, no image atlases.

The tradeoff:
- **Pro**: Zero I/O at startup. Icons are always available — even in `build_fallback_icon()` scenarios.
- **Pro**: No asset path resolution needed.
- **Con**: Binary size grows with each icon (~200–800 bytes per SVG string). With 30 icons, this is roughly 15–25 KB.
- **Con**: Updating an icon requires a recompile.

The SVGs were ported from `mezon-js/libs/ui/src/lib/Icons/icons.tsx` (the Electron app's React icon library), preserving the same visual language across platforms.

### `IconName` Enum

30 variants organized by semantic category:

```
Channel types    : Hashtag, HashtagLocked, Speaker, Thread
Navigation       : ArrowRight, ArrowDown, ArrowLeft, ChevronDown
Common actions   : Close, Search, Add, Check, PenEdit, Delete, Download, Copy, Link, Pin, Reply
Users / members  : UserIcon, MemberList
Media / voice    : Attachment, Emoji, Mic, Deafen, Video
App chrome       : Settings, Bell
Misc             : React
```

`IconName` is `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` — it's a `Copy` type because it's just an enum discriminant. Passing an `IconName` by value is free.

### SVG Format Requirements

All SVG strings follow the same format:
1. `<svg viewBox="0 0 N N" fill="none" xmlns="...">` — explicit viewBox, no hardcoded width/height
2. Exactly one or two `<path>` elements with `fill="currentColor"`
3. No `<defs>`, no `<g>`, no gradients, no external references

GPUI's `svg()` element renders SVG paths using its own path rasterizer. Only simple path shapes are supported — `currentColor` is resolved to the element's `text_color`, and the viewBox is mapped to the element's pixel size.

### `Icon` Builder

```rust
Icon::new(IconName::Settings)
    .size(16.0)                   // pixels, default 20.0
    .color(theme.text_primary)    // explicit Rgba override
    .render(&theme)               // → impl IntoElement
```

Convenience shorthand:
```rust
Icon::new(IconName::Bell)
    .muted(&theme)         // shorthand for .color(theme.text_muted)
    .render(&theme)
```

`render` falls back to `theme.text_secondary` if no explicit color was set:

```rust
let color = self.color.unwrap_or(theme.text_secondary);
svg().path(self.name.svg_str()).size(sz).text_color(color)
```

### `svg_str` Dispatch

`IconName::svg_str(self)` is a match over all 30 variants returning a `&'static str`. This is a zero-cost dispatch — the compiler generates a jump table or direct lookup for the enum variant.

```rust
impl IconName {
    pub fn svg_str(self) -> &'static str {
        match self {
            IconName::Hashtag => SVG_HASHTAG,
            IconName::Close   => SVG_CLOSE,
            // ... 28 more
        }
    }
}
```

### Example Usage Patterns

**Icon in a button:**
```rust
div()
    .flex()
    .items_center()
    .gap_2()
    .child(Icon::new(IconName::Settings).size(16.0).render(&theme))
    .child(Label::new("Settings").render(&theme))
```

**Muted secondary icon:**
```rust
Icon::new(IconName::Hashtag).muted(&theme).render(&theme)
```

**Dynamic icon based on state:**
```rust
let icon = if is_muted { IconName::Deafen } else { IconName::Mic };
Icon::new(icon).render(&theme)
```

### Adding a New Icon

1. Export the SVG from Figma or copy from `mezon-js/libs/ui/src/lib/Icons/icons.tsx`.
2. Minimize the SVG: keep only `viewBox`, remove `width`/`height`, replace `fill="..."` with `fill="currentColor"`.
3. Add a variant to `IconName`.
4. Add `IconName::NewIcon => SVG_NEW_ICON` to the `svg_str` match.
5. Define `const SVG_NEW_ICON: &str = r#"<svg ...>"#;` in the constants section.
6. Export is automatic — `icon.rs` is already re-exported from `primitives/mod.rs`.

---

## 3. How GPUI Renders SVGs

For both `Spinner` and `Icon`, the rendering path is:

1. `svg().path(svg_string)` — GPUI parses the SVG path data at render time.
2. `.size(px)` — the SVG is scaled to fit the given pixel size, preserving the aspect ratio of the `viewBox`.
3. `.text_color(color)` — `currentColor` in the SVG is resolved to this `Rgba` value.
4. The GPU rasterizes the paths into a texture and composites it into the scene.

For animated SVGs (the `Spinner`), GPUI applies the `Transformation::rotate` to the texture's transform matrix on the GPU each frame. The SVG itself is not re-parsed or re-rasterized every frame — only the transform changes.

This means both components are efficient: icons render in a single GPU draw call, and the spinner's rotation has zero CPU cost at 60Hz.
