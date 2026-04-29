/// TextInput — single-line controlled text input as a stateful GPUI View.
///
/// Text input works via two mechanisms:
///   1. `EntityInputHandler` + `ElementInputHandler` — registered during the **paint** phase
///      via the `TextInputElement` wrapper so the OS delivers typed characters (including IME,
///      dead keys, non-ASCII) via `replace_text_in_range`.
///   2. `on_key_down` on the container div — handles Backspace, Delete, and a fallback
///      for `key_char` in case the OS skips the input-handler path.
use std::ops::Range;

use gpui::{
    div, prelude::*, AnyElement, App, Bounds, Context, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    LayoutId, MouseButton, Pixels, SharedString, UTF16Selection, Window,
};

use crate::components::TextChangeHandler;
use crate::theme::Theme;

// ─── TextInput view ───────────────────────────────────────────────────────────

pub struct TextInput {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) value: String,
    placeholder: SharedString,
    label: Option<SharedString>,
    error: Option<SharedString>,
    pub(crate) masked: bool,
    disabled: bool,
    pub(crate) on_change: Option<TextChangeHandler>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, id: impl Into<SharedString>) -> Self {
        let _ = id;
        Self {
            focus_handle: cx.focus_handle(),
            value: String::new(),
            placeholder: SharedString::default(),
            label: None,
            error: None,
            masked: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.label = Some(text.into());
        self
    }

    pub fn with_error(mut self, err: Option<impl Into<SharedString>>) -> Self {
        self.error = err.map(Into::into);
        self
    }

    pub fn masked(mut self, m: bool) -> Self {
        self.masked = m;
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn on_change(mut self, cb: TextChangeHandler) -> Self {
        self.on_change = Some(cb);
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: String, cx: &mut Context<Self>) {
        self.value = value;
        cx.notify();
    }

    pub fn set_error(&mut self, err: Option<String>, cx: &mut Context<Self>) {
        self.error = err.map(SharedString::from);
        cx.notify();
    }

    fn fire_change(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cb) = &self.on_change {
            cb(&self.value, window, cx);
        }
    }
}

// ─── EntityInputHandler ───────────────────────────────────────────────────────
// Implements the OS text-input protocol (NSTextInputClient on macOS).
// `replace_text_in_range` is the primary entry point for all typed characters.

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.value.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len());
        *adjusted_range = Some(start..end);
        String::from_utf16(&utf16[start..end]).ok()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.disabled {
            return None;
        }
        let len = self.value.encode_utf16().count();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Primary entry point for typed characters — called by the OS for every
    /// printable keystroke including IME composition commits.
    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        self.value.push_str(text);
        self.fire_change(window, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        self.value.push_str(new_text);
        self.fire_change(window, cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

// ─── Render ───────────────────────────────────────────────────────────────────
// Builds the visual div tree and wraps it in a TextInputElement so that
// `handle_input` is called during the paint phase (not here in render).

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let is_focused = self.focus_handle.is_focused(window);
        let has_error = self.error.is_some();
        let entity = cx.entity().clone();

        let border_color = if has_error {
            theme.status_dnd
        } else if is_focused {
            theme.brand
        } else {
            theme.border
        };

        let display_value: String = if self.masked {
            "●".repeat(self.value.len())
        } else {
            self.value.clone()
        };

        let show_placeholder = self.value.is_empty();
        let placeholder = self.placeholder.clone();
        let focus_handle = self.focus_handle.clone();

        let input_box = div()
            .track_focus(&focus_handle)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(gpui::px(40.0))
            .px_3()
            .bg(theme.bg_secondary)
            .rounded_md()
            .border_1()
            .border_color(border_color)
            .cursor_text()
            // Click → grab focus.
            .on_mouse_down(MouseButton::Left, {
                let focus_handle = focus_handle.clone();
                move |_event, window, cx| {
                    window.focus(&focus_handle, cx);
                }
            })
            // Keyboard: Backspace + fallback for key_char (in case the OS skips handle_input).
            .on_key_down({
                let entity = entity.clone();
                move |event, window, cx| {
                    match event.keystroke.key.as_str() {
                        "backspace" => {
                            entity.update(cx, |this, cx| {
                                if !this.disabled && !this.value.is_empty() {
                                    let new_len = this
                                        .value
                                        .char_indices()
                                        .next_back()
                                        .map(|(i, _)| i)
                                        .unwrap_or(0);
                                    this.value.truncate(new_len);
                                    this.fire_change(window, cx);
                                    cx.notify();
                                }
                            });
                        }
                        _ => {
                            // Printable characters are delivered via replace_text_in_range
                            // by the OS input handler — no fallback needed here.
                        }
                    }
                }
            })
            .child(if show_placeholder {
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(placeholder)
            } else {
                div()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(display_value)
            })
            .when(is_focused, |el| {
                el.child(
                    div()
                        .w(gpui::px(1.5))
                        .h(gpui::px(18.0))
                        .ml_1()
                        .bg(theme.brand),
                )
            });

        let mut container = div().flex().flex_col().gap_1().w_full();

        if let Some(label) = &self.label {
            container = container.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(label.clone()),
            );
        }

        container = container.child(input_box);

        if let Some(err) = &self.error {
            container = container.child(
                div()
                    .text_xs()
                    .text_color(theme.status_dnd)
                    .child(err.clone()),
            );
        }

        // Wrap in TextInputElement so handle_input is called during paint.
        TextInputElement {
            entity,
            focus_handle: self.focus_handle.clone(),
            child: container.into_any_element(),
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ─── TextInputElement — custom Element wrapper ────────────────────────────────
//
// Delegates layout + prepaint to the child `AnyElement`, then in `paint()`
// additionally calls `window.handle_input()` so the OS knows this element
// accepts text input.  This is the only legal place to call `handle_input`.

struct TextInputElement {
    entity: Entity<TextInput>,
    focus_handle: FocusHandle,
    child: AnyElement,
}

impl IntoElement for TextInputElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextInputElement {
    /// No persistent per-frame state needed beyond what the child carries.
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.child.request_layout(window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Paint the visual child tree first.
        self.child.paint(window, cx);

        // Now register the input handler — legal only during paint.
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}
