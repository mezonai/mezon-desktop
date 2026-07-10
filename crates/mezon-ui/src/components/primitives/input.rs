#[path = "blink_manager.rs"]
mod blink_manager;

use std::ops::Range;

use blink_manager::CaretBlink;

use gpui::{
    App, Bounds, ClipboardItem, Context, Corners, CursorStyle, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, RenderOnce,
    ShapedLine, SharedString, Style, StyleRefinement, Styled, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, size, svg,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::ActiveTheme;

const MASK: char = '\u{2022}';
const KEY_CONTEXT: &str = "MezonInput";

actions!(
    mezon_input,
    [
        Backspace,
        Delete,
        Enter,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Enter, Some(KEY_CONTEXT)),
        KeyBinding::new("left", Left, Some(KEY_CONTEXT)),
        KeyBinding::new("right", Right, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(KEY_CONTEXT)),
        KeyBinding::new("home", Home, Some(KEY_CONTEXT)),
        KeyBinding::new("end", End, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Change,
    PressEnter,
}

type ValidateFn = Box<dyn Fn(&str, &mut App) -> bool + 'static>;

pub struct InputState {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    masked: bool,
    multi_line: bool,
    embedded: bool,
    validate: Option<ValidateFn>,
    height: Option<Pixels>,
    radius: Option<Pixels>,
    bg_override: Option<Hsla>,
    text_color_override: Option<Hsla>,
    text_size_override: Option<Pixels>,
    padding_x: Option<Pixels>,
    padding_right: Option<Pixels>,
    show_border: bool,
    filter_token_chips: bool,
    token_bg_ranges: Vec<Range<usize>>,
    token_bg_color: Option<Hsla>,
    pub(crate) caret_blink: CaretBlink,
}

impl EventEmitter<InputEvent> for InputState {}

impl InputState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let this = Self {
            focus_handle: focus_handle.clone(),
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            masked: false,
            multi_line: false,
            embedded: false,
            validate: None,
            height: None,
            radius: None,
            bg_override: None,
            text_color_override: None,
            text_size_override: None,
            padding_x: None,
            padding_right: None,
            show_border: true,
            filter_token_chips: false,
            token_bg_ranges: Vec::new(),
            token_bg_color: None,
            caret_blink: CaretBlink::new(),
        };

        cx.on_focus(&focus_handle, window, |this, _window, cx| {
            this.caret_blink.sync_focused(cx);
        })
        .detach();

        cx.on_blur(&focus_handle, window, |this, _window, cx| {
            this.caret_blink.sync_blurred(cx);
        })
        .detach();

        this
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = Some(radius);
        self
    }

    pub fn bg(mut self, bg: impl Into<Hsla>) -> Self {
        self.bg_override = Some(bg.into());
        self
    }

    pub fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_color_override = Some(color.into());
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size_override = Some(size);
        self
    }

    pub fn padding_x(mut self, padding: Pixels) -> Self {
        self.padding_x = Some(padding);
        self
    }

    pub fn padding_right(mut self, padding: Pixels) -> Self {
        self.padding_right = Some(padding);
        self
    }

    pub fn borderless(mut self) -> Self {
        self.show_border = false;
        self
    }

    pub fn multi_line(mut self, multi_line: bool) -> Self {
        self.multi_line = multi_line;
        self
    }

    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    pub fn filter_token_chips(mut self, enabled: bool) -> Self {
        self.filter_token_chips = enabled;
        self
    }

    pub fn validate(mut self, validate: impl Fn(&str, &mut App) -> bool + 'static) -> Self {
        self.validate = Some(Box::new(validate));
        self
    }

    pub fn value(&self) -> &str {
        self.content.as_ref()
    }

    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.content = value.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        self.refresh_filter_token_chips(cx);
        cx.notify();
        cx.emit(InputEvent::Change);
    }

    pub fn set_token_backgrounds(
        &mut self,
        ranges: Vec<Range<usize>>,
        color: Hsla,
        cx: &mut Context<Self>,
    ) {
        self.token_bg_ranges = ranges;
        self.token_bg_color = Some(color);
        cx.notify();
    }

    pub fn clear_token_backgrounds(&mut self, cx: &mut Context<Self>) {
        if self.token_bg_ranges.is_empty() && self.token_bg_color.is_none() {
            return;
        }
        self.token_bg_ranges.clear();
        self.token_bg_color = None;
        cx.notify();
    }

    fn refresh_filter_token_chips(&mut self, cx: &mut Context<Self>) {
        if !self.filter_token_chips {
            return;
        }
        let ranges = mezon_store::search_filter_chip_ranges(self.content.as_ref());
        if ranges.is_empty() {
            self.token_bg_ranges.clear();
            self.token_bg_color = None;
            return;
        }
        self.token_bg_ranges = ranges;
        self.token_bg_color = Some(cx.theme().tokens.bg_item_hover.into());
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.content.is_empty() {
            return;
        }
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.marked_range = None;
        self.token_bg_ranges.clear();
        self.token_bg_color = None;
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.caret_blink.sync_focused(cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.multi_line {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(InputEvent::PressEnter);
        }
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.caret_blink.sync_focused(cx);
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let sanitized = if self.multi_line {
                text
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &sanitized, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            if !self.masked {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    self.content[self.selected_range.clone()].to_string(),
                ));
            }
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.pause_caret_blink(cx);
        cx.notify()
    }

    fn pause_caret_blink(&mut self, cx: &mut Context<Self>) {
        self.caret_blink.pause_blinking(cx);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn is_masked(&self) -> bool {
        self.masked && !self.content.is_empty()
    }

    fn display_text(&self) -> SharedString {
        if self.is_masked() {
            MASK.to_string().repeat(self.content.chars().count()).into()
        } else {
            self.content.clone()
        }
    }

    fn to_display_offset(&self, content_offset: usize) -> usize {
        if self.is_masked() {
            self.content[..content_offset].chars().count() * MASK.len_utf8()
        } else {
            content_offset
        }
    }

    fn display_to_content_offset(&self, display_offset: usize) -> usize {
        if self.is_masked() {
            let char_index = display_offset / MASK.len_utf8();
            self.content
                .char_indices()
                .nth(char_index)
                .map(|(idx, _)| idx)
                .unwrap_or(self.content.len())
        } else {
            display_offset
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let display_index = line.closest_index_for_x(position.x - bounds.left());
        self.display_to_content_offset(display_index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.pause_caret_blink(cx);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for InputState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let candidate =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];

        let valid = match &self.validate {
            Some(validate) => validate(&candidate, cx),
            None => true,
        };
        if !valid {
            return;
        }

        self.content = candidate.into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.refresh_filter_token_chips(cx);
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(InputEvent::Change);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.refresh_filter_token_chips(cx);
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(InputEvent::Change);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(self.to_display_offset(range.start)),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(self.to_display_offset(range.end)),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let display_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(self.display_to_content_offset(display_index)))
    }
}

impl Focusable for InputState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused {
            self.caret_blink.sync_focused(cx);
        } else {
            self.caret_blink.sync_blurred(cx);
        }

        let bg: Hsla = self.bg_override.unwrap_or(cx.theme().bg_tertiary.into());
        let text_color: Hsla = self
            .text_color_override
            .unwrap_or(cx.theme().text_primary.into());
        let border = cx.theme().border;
        let focus_border = cx.theme().brand;
        let height = self
            .height
            .unwrap_or(if self.multi_line { px(72.) } else { px(36.) });
        let radius = self.radius;
        let padding_x = self.padding_x.unwrap_or(px(10.));
        let padding_right = self.padding_right;
        let text_size = self.text_size_override.unwrap_or(px(14.));
        let show_border = self.show_border;

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex()
            .when(self.multi_line, |el| el.items_start().py(px(8.)))
            .when(!self.multi_line, |el| el.items_center())
            .w_full()
            .when(self.embedded, |el| el.min_h(px(24.)).px_0())
            .when(!self.embedded, |el| {
                el.min_h(height)
                    .px(padding_x)
                    .when_some(padding_right, |el, p| el.pr(p))
                    .when(radius.is_none(), |el| el.rounded_md())
                    .when_some(radius, |el, r| el.rounded(r))
                    .bg(bg)
                    .when(show_border, |el| {
                        el.border_1()
                            .border_color(if focused { focus_border } else { border })
                    })
            })
            .text_color(text_color)
            .text_size(text_size)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

fn build_input_text_runs(
    text_len: usize,
    base: &TextRun,
    marked: Option<Range<usize>>,
    token_ranges: &[Range<usize>],
    token_bg: Option<Hsla>,
) -> Vec<TextRun> {
    if marked.is_none() && (token_ranges.is_empty() || token_bg.is_none()) {
        return vec![base.clone()];
    }

    let mut bounds = vec![0usize, text_len];
    if let Some(marked) = &marked {
        bounds.push(marked.start.min(text_len));
        bounds.push(marked.end.min(text_len));
    }
    for range in token_ranges {
        bounds.push(range.start.min(text_len));
        bounds.push(range.end.min(text_len));
    }
    bounds.sort_unstable();
    bounds.dedup();

    bounds
        .windows(2)
        .filter_map(|window| {
            let (start, end) = (window[0], window[1]);
            if end <= start {
                return None;
            }
            let in_marked = marked
                .as_ref()
                .is_some_and(|marked| marked.start <= start && end <= marked.end);
            let in_token = token_bg.is_some()
                && token_ranges
                    .iter()
                    .any(|range| range.start <= start && end <= range.end);
            let mut run = base.clone();
            run.len = end - start;
            if in_token {
                run.background_color = token_bg;
            }
            if in_marked {
                run.underline = Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                });
            }
            Some(run)
        })
        .collect()
}

struct TextElement {
    input: Entity<InputState>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let placeholder_color: Hsla = cx.theme().text_muted.into();
        let cursor_color: Hsla = cx.theme().brand.into();
        let selection_color: Hsla = cx.theme().brand.into();

        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let masked = input.is_masked();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), placeholder_color)
        } else {
            (input.display_text(), style.color)
        };

        let marked_range = input.marked_range.clone();
        let display_cursor = input.to_display_offset(cursor);
        let display_selection = input.to_display_offset(selected_range.start)
            ..input.to_display_offset(selected_range.end);

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = build_input_text_runs(
            display_text.len(),
            &run,
            if masked { None } else { marked_range },
            &[],
            None,
        );

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(display_cursor);
        let (selection, cursor) = if display_selection.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    cursor_color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(display_selection.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(display_selection.end),
                            bounds.bottom(),
                        ),
                    ),
                    selection_color.opacity(0.3),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let Some(line) = prepaint.line.take() else {
            return;
        };

        let (token_ranges, token_color) = {
            let input = self.input.read(cx);
            (input.token_bg_ranges.clone(), input.token_bg_color)
        };
        if let Some(color) = token_color {
            let pad_x = px(2.);
            let chip_height = (bounds.size.height - px(2.)).max(px(16.));
            let chip_top = bounds.top() + (bounds.size.height - chip_height) / 2.;
            for range in token_ranges {
                if range.start >= range.end || range.end > line.len() {
                    continue;
                }
                let x0 = line.x_for_index(range.start);
                let x1 = line.x_for_index(range.end);
                if x1 <= x0 {
                    continue;
                }
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(bounds.left() + x0 - pad_x, chip_top),
                            size((x1 - x0) + pad_x * 2., chip_height),
                        ),
                        color,
                    )
                    .corner_radii(Corners::all(px(4.))),
                );
            }
        }

        if let Err(e) = line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        ) {
            tracing::warn!("input text paint failed: {e}");
        }

        if focus_handle.is_focused(window)
            && self.input.read(cx).caret_blink.visible()
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
    base: Div,
    mask_toggle: bool,
}

impl Input {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            base: div().w_full(),
            mask_toggle: false,
        }
    }

    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for Input {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        let masked = state.read(cx).masked;
        let toggle_color = cx.theme().text_muted;

        let toggle = self.mask_toggle.then(|| {
            let state = state.clone();
            div()
                .id("mask-toggle")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .px(px(16.))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(
                    svg()
                        .path(if masked {
                            "icons/eye-open.svg"
                        } else {
                            "icons/eye-close.svg"
                        })
                        .size(px(20.))
                        .text_color(toggle_color),
                )
                .on_click(move |_, window, cx| {
                    state.update(cx, |input, cx| {
                        let next = !input.masked;
                        input.set_masked(next, window, cx);
                    });
                })
        });

        self.base
            .relative()
            .child(state)
            .when_some(toggle, |el, toggle| el.child(toggle))
    }
}
