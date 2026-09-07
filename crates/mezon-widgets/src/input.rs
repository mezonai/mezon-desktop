use std::ops::Range;

use super::blink_manager::{CaretBlink, HasCaretBlink};

use gpui::{
    App, Bounds, ClipboardItem, Context, Corners, CursorStyle, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, GlobalElementId, Hsla, ImeSurroundingText, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, RenderOnce, ShapedLine, SharedString, Style, StyleRefinement, Styled,
    TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, div, fill, point,
    prelude::*, px, size, svg,
};
use unicode_segmentation::UnicodeSegmentation;

use mezon_theme::ActiveTheme;

use crate::text_actions::{
    Backspace, Copy, Cut, Delete, DeleteToLineEnd, DeleteToLineStart, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, Down, End, Enter, Home, Left, MoveToDocEnd, MoveToDocStart,
    MoveToNextWordEnd, MoveToPreviousWordStart, Newline, Paste, Redo, Right, SelectAll, SelectDown,
    SelectLeft, SelectRight, SelectToDocEnd, SelectToDocStart, SelectToLineEnd, SelectToLineStart,
    SelectToNextWordEnd, SelectToPreviousWordStart, SelectUp, ShowCharacterPalette,
    TEXT_INPUT_CONTEXT, Undo, Up,
};
use crate::text_edit::{
    EditKind, HistoryEntry, MAX_UNDO_HISTORY, SelectGranularity, extend_range_for_granularity,
    granularity_for_click, home_target, ime_replace_range, line_end, line_start,
    marked_caret_range, marked_range_after_delete, next_word_boundary, previous_word_boundary,
    range_for_granularity, should_coalesce, splice_out_byte_range, surrounding_delete_range,
    swallow_discarded_ime_commit,
};

const MASK: char = '\u{2022}';

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
    discard_ime_commit: Option<String>,
    last_layout: Option<ShapedLine>,
    last_lines: Vec<InputDocLine>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    page_scroll_offset: Point<Pixels>,
    is_selecting: bool,
    select_granularity: SelectGranularity,
    select_anchor: Range<usize>,
    masked: bool,
    multi_line: bool,
    embedded: bool,
    validate: Option<ValidateFn>,
    height: Option<Pixels>,
    radius: Option<Pixels>,
    bg_override: Option<Hsla>,
    text_color_override: Option<Hsla>,
    text_size_override: Option<Pixels>,
    font_weight_override: Option<FontWeight>,
    padding_x: Option<Pixels>,
    padding_right: Option<Pixels>,
    show_border: bool,
    center_text: bool,
    single_line_scroll: Pixels,
    draw_offset: Pixels,
    filter_token_chips: bool,
    token_bg_ranges: Vec<Range<usize>>,
    token_bg_color: Option<Hsla>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    last_edit_kind: Option<EditKind>,
    pub(crate) caret_blink: CaretBlink,
}

impl EventEmitter<InputEvent> for InputState {}

impl HasCaretBlink for InputState {
    fn caret_blink_mut(&mut self) -> &mut CaretBlink {
        &mut self.caret_blink
    }
}

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
            discard_ime_commit: None,
            last_layout: None,
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: Pixels::ZERO,
            page_scroll_offset: Point::default(),
            is_selecting: false,
            select_granularity: SelectGranularity::Character,
            select_anchor: 0..0,
            masked: false,
            multi_line: false,
            embedded: false,
            validate: None,
            height: None,
            radius: None,
            bg_override: None,
            text_color_override: None,
            text_size_override: None,
            font_weight_override: None,
            padding_x: None,
            padding_right: None,
            show_border: true,
            center_text: false,
            single_line_scroll: px(0.),
            draw_offset: px(0.),
            filter_token_chips: false,
            token_bg_ranges: Vec::new(),
            token_bg_color: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
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

    pub fn font_weight(mut self, weight: FontWeight) -> Self {
        self.font_weight_override = Some(weight);
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

    pub fn text_align_center(mut self) -> Self {
        self.center_text = true;
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

    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    pub fn drop_uncommitted_preedit(&mut self, cx: &mut Context<Self>) {
        let Some(marked) = self.marked_range.take() else {
            return;
        };
        let Some((next, discarded, caret)) = splice_out_byte_range(&self.content, marked) else {
            return;
        };
        self.discard_ime_commit = Some(discarded);
        self.content = next.into();
        self.selected_range = caret..caret;
        self.refresh_filter_token_chips(cx);
        cx.notify();
        cx.emit(InputEvent::Change);
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
        self.clear_history();
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
        self.clear_history();
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

    fn caret_line_target(&self, delta: isize) -> usize {
        if !self.multi_line || self.last_lines.len() <= 1 || self.line_height <= Pixels::ZERO {
            return if delta < 0 { 0 } else { self.content.len() };
        }
        let line_height = self.line_height;
        let display_offset = self.to_display_offset(self.cursor_offset());
        let (line_ix, local) = self
            .last_lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, line)| line.start <= display_offset)
            .map(|(ix, line)| (ix, display_offset - line.start))
            .unwrap_or((0, 0));
        let caret_x = self.last_lines[line_ix]
            .line
            .position_for_index(local, line_height)
            .map(|position| position.x)
            .unwrap_or(Pixels::ZERO);
        let target_ix = line_ix as isize + delta;
        if target_ix < 0 {
            return 0;
        }
        let Some(target) = self.last_lines.get(target_ix as usize) else {
            return self.content.len();
        };
        let local_target = target
            .line
            .closest_index_for_position(point(caret_x, px(0.)), line_height)
            .unwrap_or_else(|ix| ix);
        self.display_to_content_offset(target.start + local_target)
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line {
            cx.propagate();
            return;
        }
        self.move_to(self.caret_line_target(-1), cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line {
            cx.propagate();
            return;
        }
        self.move_to(self.caret_line_target(1), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line {
            cx.propagate();
            return;
        }
        self.select_to(self.caret_line_target(-1), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line {
            cx.propagate();
            return;
        }
        self.select_to(self.caret_line_target(1), cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line {
            cx.propagate();
            return;
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(home_target(&self.content, self.cursor_offset()), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(line_end(&self.content, self.cursor_offset()), cx);
    }

    fn select_to_line_start(
        &mut self,
        _: &SelectToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(home_target(&self.content, self.cursor_offset()), cx);
    }

    fn select_to_line_end(&mut self, _: &SelectToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(line_end(&self.content, self.cursor_offset()), cx);
    }

    fn move_to_doc_start(&mut self, _: &MoveToDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn move_to_doc_end(&mut self, _: &MoveToDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_to_doc_start(
        &mut self,
        _: &SelectToDocStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_to_doc_end(&mut self, _: &SelectToDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn move_to_previous_word_start(
        &mut self,
        _: &MoveToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(
            previous_word_boundary(&self.content, self.cursor_offset()),
            cx,
        );
    }

    fn move_to_next_word_end(
        &mut self,
        _: &MoveToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(next_word_boundary(&self.content, self.cursor_offset()), cx);
    }

    fn select_to_previous_word_start(
        &mut self,
        _: &SelectToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(
            previous_word_boundary(&self.content, self.cursor_offset()),
            cx,
        );
    }

    fn select_to_next_word_end(
        &mut self,
        _: &SelectToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(next_word_boundary(&self.content, self.cursor_offset()), cx);
    }

    fn delete_to_previous_word_start(
        &mut self,
        _: &DeleteToPreviousWordStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let prev = previous_word_boundary(&self.content, self.cursor_offset());
            if prev == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.extend_selection(prev, cx);
        }
        self.delete_selected_range(window, cx);
    }

    fn delete_to_next_word_end(
        &mut self,
        _: &DeleteToNextWordEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let next = next_word_boundary(&self.content, self.cursor_offset());
            if next == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.extend_selection(next, cx);
        }
        self.delete_selected_range(window, cx);
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let target = line_start(&self.content, self.cursor_offset());
            if target == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.extend_selection(target, cx);
        }
        self.delete_selected_range(window, cx);
    }

    fn delete_to_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let target = line_end(&self.content, self.cursor_offset());
            if target == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.extend_selection(target, cx);
        }
        self.delete_selected_range(window, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.undo_stack.pop() {
            self.redo_stack.push(self.history_snapshot());
            self.restore_history(entry, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.redo_stack.pop() {
            self.undo_stack.push(self.history_snapshot());
            self.restore_history(entry, cx);
        }
    }

    fn history_snapshot(&self) -> HistoryEntry {
        HistoryEntry {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
            payload: None,
        }
    }

    fn record_history(&mut self, kind: EditKind) {
        let coalesce = should_coalesce(self.last_edit_kind, kind);
        self.redo_stack.clear();
        if !coalesce {
            self.undo_stack.push(self.history_snapshot());
            if self.undo_stack.len() > MAX_UNDO_HISTORY {
                let overflow = self.undo_stack.len() - MAX_UNDO_HISTORY;
                self.undo_stack.drain(..overflow);
            }
        }
        self.last_edit_kind = Some(kind);
    }

    fn restore_history(&mut self, entry: HistoryEntry, cx: &mut Context<Self>) {
        self.content = entry.content;
        self.selected_range = entry.selected_range;
        self.selection_reversed = entry.selection_reversed;
        self.marked_range = None;
        self.last_edit_kind = None;
        self.refresh_filter_token_chips(cx);
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(InputEvent::Change);
    }

    fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit_kind = None;
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.extend_selection(prev, cx)
        }
        self.delete_selected_range(window, cx)
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
            self.extend_selection(next, cx)
        }
        self.delete_selected_range(window, cx)
    }

    fn delete_selected_range(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let range_utf16 = self.range_to_utf16(&self.selected_range);
        self.replace_text_in_range(Some(range_utf16), "", window, cx)
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
        let offset = self.index_for_mouse_position(event.position);

        if event.modifiers.shift {
            self.select_granularity = SelectGranularity::Character;
            self.select_to(offset, cx);
            return;
        }

        self.select_granularity = granularity_for_click(event.click_count);
        if self.select_granularity == SelectGranularity::Character {
            self.select_anchor = offset..offset;
            self.move_to(offset, cx);
            return;
        }

        let range = self.granularity_range(offset, self.select_granularity);
        self.select_anchor = range.clone();
        self.selection_reversed = false;
        self.selected_range = range;
        self.last_edit_kind = None;
        self.pause_caret_blink(cx);
        cx.notify();
    }

    fn granularity_range(&self, offset: usize, granularity: SelectGranularity) -> Range<usize> {
        if self.is_masked() {
            return 0..self.content.len();
        }
        range_for_granularity(&self.content, offset, granularity, self.multi_line)
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let offset = self.index_for_mouse_position(event.position);
        if self.select_granularity == SelectGranularity::Character {
            self.select_to(offset, cx);
            return;
        }
        if self.is_masked() {
            return;
        }
        let (range, reversed) = extend_range_for_granularity(
            &self.content,
            &self.select_anchor,
            offset,
            self.select_granularity,
            self.multi_line,
        );
        if range == self.selected_range && reversed == self.selection_reversed {
            return;
        }
        self.selected_range = range;
        self.selection_reversed = reversed;
        self.last_edit_kind = None;
        self.pause_caret_blink(cx);
        cx.notify();
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
        crate::clipboard::read_then(self, window, cx, |this, item, window, cx| {
            if let Some(text) = item.text() {
                let sanitized = if this.multi_line {
                    text
                } else {
                    text.replace('\n', " ")
                };
                this.replace_text_in_range(None, &sanitized, window, cx);
            }
        });
    }

    fn on_key_down(&mut self, _: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        self.discard_ime_commit = None;
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
            self.delete_selected_range(window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.last_edit_kind = None;
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

        if self.multi_line && !self.last_lines.is_empty() {
            let Some(bounds) = self.last_bounds.as_ref() else {
                return 0;
            };
            let line_height = self.line_height;
            let rel_y = position.y - bounds.top() + self.page_scroll_offset.y;
            let rel_x = position.x - bounds.left() + self.page_scroll_offset.x;
            let line_ix = if rel_y < Pixels::ZERO {
                0
            } else {
                ((rel_y / line_height) as usize).min(self.last_lines.len() - 1)
            };
            let line = &self.last_lines[line_ix];
            let local = line
                .line
                .closest_index_for_position(point(rel_x.max(Pixels::ZERO), px(0.)), line_height)
                .unwrap_or_else(|ix| ix);
            return self.display_to_content_offset(line.start + local);
        }

        if self.multi_line {
            let Some(bounds) = self.last_bounds.as_ref() else {
                return 0;
            };
            let line_height = if self.line_height > Pixels::ZERO {
                self.line_height
            } else {
                px(20.)
            };
            let rel_y = position.y - bounds.top();
            if rel_y <= Pixels::ZERO {
                return 0;
            }
            let line_ix = (rel_y / line_height) as usize;
            return self
                .content
                .char_indices()
                .filter(|(_, c)| *c == '\n')
                .nth(line_ix.saturating_sub(1))
                .map(|(idx, _)| idx + 1)
                .unwrap_or(0)
                .min(self.content.len());
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
        let display_index = line.closest_index_for_x(position.x - bounds.left() - self.draw_offset);
        self.display_to_content_offset(display_index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.last_edit_kind = None;
        self.extend_selection(offset, cx);
    }

    fn extend_selection(&mut self, offset: usize, cx: &mut Context<Self>) {
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
        #[cfg(target_os = "linux")]
        if let Some(marked) = self.marked_range.clone() {
            let start = marked.start.min(self.content.len());
            let end = marked.end.min(self.content.len()).max(start);
            self.discard_ime_commit = self.content.get(start..end).map(str::to_string);
        }
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if swallow_discarded_ime_commit(
            &mut self.discard_ime_commit,
            range_utf16.as_ref(),
            self.marked_range.is_some(),
            new_text,
        ) {
            return;
        }
        let range = if let Some(range_utf16) = range_utf16.as_ref() {
            self.range_from_utf16(range_utf16)
        } else {
            ime_replace_range(&self.selected_range, self.marked_range.as_ref())
        };
        let prior_marked = self.marked_range.clone();

        let candidate =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];

        let valid = match &self.validate {
            Some(validate) => validate(&candidate, cx),
            None => true,
        };
        if !valid {
            return;
        }

        let kind = if new_text.is_empty() {
            EditKind::Delete
        } else if self.marked_range.is_some() || (range.is_empty() && !new_text.contains('\n')) {
            EditKind::Insert
        } else {
            EditKind::Other
        };
        self.record_history(kind);

        self.content = candidate.into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        if new_text.is_empty() {
            self.marked_range = marked_range_after_delete(prior_marked.as_ref(), &range);
        } else {
            self.marked_range.take();
        }
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
        if swallow_discarded_ime_commit(
            &mut self.discard_ime_commit,
            range_utf16.as_ref(),
            self.marked_range.is_some(),
            new_text,
        ) {
            return;
        }
        if new_text.is_empty() && range_utf16.is_none() && self.marked_range.is_none() {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        if self.marked_range.is_none() {
            self.record_history(EditKind::Insert);
        }

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range =
            marked_caret_range(range.start, new_text, new_selected_range_utf16.as_ref());

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
        let range = self.range_from_utf16(&range_utf16);
        if self.multi_line && !self.last_lines.is_empty() {
            let line_height = self.line_height;
            let scroll = self.page_scroll_offset;
            let (s_line, s_local) =
                locate_display_offset(&self.last_lines, self.to_display_offset(range.start));
            let (e_line, e_local) =
                locate_display_offset(&self.last_lines, self.to_display_offset(range.end));
            let x0 = self.last_lines[s_line]
                .line
                .position_for_index(s_local, line_height)
                .map(|p| p.x)
                .unwrap_or(Pixels::ZERO);
            let x1 = self.last_lines[e_line]
                .line
                .position_for_index(e_local, line_height)
                .map(|p| p.x)
                .unwrap_or(Pixels::ZERO);
            let y0 = bounds.top() + s_line as f32 * line_height - scroll.y;
            let y1 = bounds.top() + e_line as f32 * line_height + line_height - scroll.y;
            return Some(Bounds::from_corners(
                point(bounds.left() + x0 - scroll.x, y0),
                point(bounds.left() + x1 - scroll.x, y1),
            ));
        }
        let Some(last_layout) = self.last_layout.as_ref() else {
            return Some(Bounds::from_corners(
                point(bounds.left(), bounds.top()),
                point(bounds.left(), bounds.bottom()),
            ));
        };
        Some(Bounds::from_corners(
            point(
                bounds.left()
                    + self.draw_offset
                    + last_layout.x_for_index(self.to_display_offset(range.start)),
                bounds.top(),
            ),
            point(
                bounds.left()
                    + self.draw_offset
                    + last_layout.x_for_index(self.to_display_offset(range.end)),
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
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }

    fn surrounding_text(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<ImeSurroundingText> {
        if self.masked {
            return None;
        }
        Some(ImeSurroundingText::from_selection(
            &self.content,
            self.selected_range.clone(),
            self.selection_reversed,
            self.marked_range.clone(),
        ))
    }

    fn delete_surrounding_text(
        &mut self,
        before_len: usize,
        after_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = surrounding_delete_range(
            &self.content,
            &self.selected_range,
            self.marked_range.as_ref(),
            self.selection_reversed,
            before_len,
            after_len,
        );
        if range.is_empty() {
            return;
        }
        let range_utf16 = self.range_to_utf16(&range);
        self.replace_text_in_range(Some(range_utf16), "", window, cx);
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
            .key_context(TEXT_INPUT_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_to_line_start))
            .on_action(cx.listener(Self::select_to_line_end))
            .on_action(cx.listener(Self::move_to_doc_start))
            .on_action(cx.listener(Self::move_to_doc_end))
            .on_action(cx.listener(Self::select_to_doc_start))
            .on_action(cx.listener(Self::select_to_doc_end))
            .on_action(cx.listener(Self::move_to_previous_word_start))
            .on_action(cx.listener(Self::move_to_next_word_end))
            .on_action(cx.listener(Self::select_to_previous_word_start))
            .on_action(cx.listener(Self::select_to_next_word_end))
            .on_action(cx.listener(Self::delete_to_previous_word_start))
            .on_action(cx.listener(Self::delete_to_next_word_end))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .when(self.multi_line, |el| {
                el.flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .py(px(8.))
            })
            .when(!self.multi_line, |el| el.flex().items_center())
            .w_full()
            .when(self.embedded, |el| {
                if self.multi_line {
                    el.flex_1().min_h_0().w_full().px_0()
                } else {
                    let min = self.height.unwrap_or(px(24.));
                    el.min_h(min).h(min).px_0()
                }
            })
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
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

fn to_single_line_display(text: SharedString) -> SharedString {
    if text.contains(['\n', '\r']) {
        text.replace(['\n', '\r'], " ").into()
    } else {
        text
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

struct InputDocLine {
    line: WrappedLine,
    start: usize,
}

struct PreparedInputLine {
    line: WrappedLine,
    origin: Point<Pixels>,
    start: usize,
}

fn locate_display_offset(lines: &[InputDocLine], display_off: usize) -> (usize, usize) {
    for (ix, doc) in lines.iter().enumerate() {
        if display_off <= doc.start + doc.line.len() {
            return (ix, display_off.saturating_sub(doc.start));
        }
    }
    match lines.last() {
        Some(doc) => (lines.len() - 1, display_off.saturating_sub(doc.start)),
        None => (0, 0),
    }
}

fn locate_input_span(spans: &[(usize, usize)], off: usize) -> (usize, usize) {
    for (ix, &(start, len)) in spans.iter().enumerate() {
        if off <= start + len {
            return (ix, off.saturating_sub(start));
        }
    }
    match spans.last() {
        Some(&(start, _)) => (spans.len() - 1, off.saturating_sub(start)),
        None => (0, 0),
    }
}

fn compute_draw_offset(
    center: bool,
    prev_scroll: Pixels,
    content_width: Pixels,
    text_width: Pixels,
    cursor_pos: Pixels,
) -> Pixels {
    if center {
        return ((content_width - text_width) / 2.).max(px(0.));
    }
    let caret_pad = px(1.);
    let mut scroll = prev_scroll;
    if cursor_pos - scroll > content_width - caret_pad {
        scroll = cursor_pos - content_width + caret_pad;
    }
    if cursor_pos - scroll < px(0.) {
        scroll = cursor_pos;
    }
    let max_scroll = (text_width - content_width).max(px(0.));
    if scroll > max_scroll {
        scroll = max_scroll;
    }
    if scroll < px(0.) {
        scroll = px(0.);
    }
    -scroll
}

struct TextElement {
    input: Entity<InputState>,
}

#[allow(clippy::large_enum_variant)]
enum PrepaintState {
    Single {
        line: ShapedLine,
        cursor: Option<PaintQuad>,
        selection: Option<PaintQuad>,
    },
    Multi {
        lines: Vec<PreparedInputLine>,
        cursor: Option<PaintQuad>,
        selection: Vec<PaintQuad>,
        line_height: Pixels,
        scroll_offset: Point<Pixels>,
    },
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
        style.size.height = if self.input.read(cx).multi_line {
            gpui::relative(1.).into()
        } else {
            window.line_height().into()
        };
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
        let multi_line = input.multi_line;
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let masked = input.is_masked();
        let center_text = input.center_text;
        let prev_scroll = input.single_line_scroll;
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

        let run = {
            let mut font = style.font();
            if let Some(weight) = input.font_weight_override {
                font.weight = weight;
            }
            TextRun {
                len: display_text.len(),
                font,
                color: text_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }
        };
        let runs = build_input_text_runs(
            display_text.len(),
            &run,
            if masked { None } else { marked_range },
            &[],
            None,
        );

        let font_size = input
            .text_size_override
            .unwrap_or_else(|| style.font_size.to_pixels(window.rem_size()));

        if multi_line {
            let mut scroll_offset = input.page_scroll_offset;
            let line_height = input
                .text_size_override
                .map(|size| size * 1.214)
                .unwrap_or_else(|| window.line_height());
            let wrapped = window
                .text_system()
                .shape_text(display_text, font_size, &runs, None, None)
                .unwrap_or_default();

            let spans: Vec<(usize, usize)> = {
                let mut start = 0usize;
                wrapped
                    .iter()
                    .map(|line| {
                        let span = (start, line.len());
                        start += line.len() + 1;
                        span
                    })
                    .collect()
            };
            let line_count = wrapped.len().max(1);

            let (caret_line, caret_local) = locate_input_span(&spans, display_cursor);
            let caret_x = wrapped
                .get(caret_line)
                .and_then(|line| line.position_for_index(caret_local, line_height))
                .map(|p| p.x)
                .unwrap_or(Pixels::ZERO);
            let caret_top = caret_line as f32 * line_height;

            let visible_h = bounds.size.height;
            let visible_w = bounds.size.width;
            let total_h = line_count as f32 * line_height;
            if caret_top < scroll_offset.y {
                scroll_offset.y = caret_top;
            }
            if caret_top + line_height > scroll_offset.y + visible_h {
                scroll_offset.y = caret_top + line_height - visible_h;
            }
            scroll_offset.y = scroll_offset
                .y
                .clamp(Pixels::ZERO, (total_h - visible_h).max(Pixels::ZERO));
            if caret_x < scroll_offset.x {
                scroll_offset.x = caret_x;
            }
            if caret_x > scroll_offset.x + visible_w - px(6.) {
                scroll_offset.x = caret_x - visible_w + px(6.);
            }
            scroll_offset.x = scroll_offset.x.max(Pixels::ZERO);

            let mut prepared = Vec::with_capacity(wrapped.len());
            let mut selection = Vec::new();
            for (ix, line) in wrapped.into_iter().enumerate() {
                let (line_start, line_len) = spans[ix];
                let line_end = line_start + line_len;
                let y = bounds.top() + (ix as f32 * line_height) - scroll_offset.y;
                let origin = point(bounds.left() - scroll_offset.x, y);
                if !display_selection.is_empty()
                    && display_selection.start <= line_end
                    && display_selection.end >= line_start
                {
                    let seg_start = display_selection.start.max(line_start) - line_start;
                    let x0 = line
                        .position_for_index(seg_start, line_height)
                        .map(|p| p.x)
                        .unwrap_or(Pixels::ZERO);
                    let x1 = if display_selection.end > line_end {
                        line.position_for_index(line_len, line_height)
                            .map(|p| p.x)
                            .unwrap_or(Pixels::ZERO)
                            + px(4.)
                    } else {
                        line.position_for_index(display_selection.end - line_start, line_height)
                            .map(|p| p.x)
                            .unwrap_or(Pixels::ZERO)
                    };
                    selection.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + x0 - scroll_offset.x, y),
                            point(bounds.left() + x1 - scroll_offset.x, y + line_height),
                        ),
                        selection_color.opacity(0.3),
                    ));
                }
                prepared.push(PreparedInputLine {
                    line,
                    origin,
                    start: line_start,
                });
            }

            let cursor = if display_selection.is_empty() {
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.left() + caret_x - scroll_offset.x,
                            bounds.top() + caret_top - scroll_offset.y,
                        ),
                        size(px(2.), line_height),
                    ),
                    cursor_color,
                ))
            } else {
                None
            };

            return PrepaintState::Multi {
                lines: prepared,
                cursor,
                selection,
                line_height,
                scroll_offset,
            };
        }

        let line = window.text_system().shape_line(
            to_single_line_display(display_text),
            font_size,
            &runs,
            None,
        );

        let cursor_pos = line.x_for_index(display_cursor);
        let draw_offset = compute_draw_offset(
            center_text,
            prev_scroll,
            bounds.size.width,
            line.width,
            cursor_pos,
        );
        let single_line_scroll = if center_text { px(0.) } else { -draw_offset };
        self.input.update(cx, |input, _cx| {
            input.draw_offset = draw_offset;
            input.single_line_scroll = single_line_scroll;
        });

        let (selection, cursor) = if display_selection.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + draw_offset + cursor_pos, bounds.top()),
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
                            bounds.left() + draw_offset + line.x_for_index(display_selection.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + draw_offset + line.x_for_index(display_selection.end),
                            bounds.bottom(),
                        ),
                    ),
                    selection_color.opacity(0.3),
                )),
                None,
            )
        };
        PrepaintState::Single {
            line,
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

        match prepaint {
            PrepaintState::Multi {
                lines,
                cursor,
                selection,
                line_height,
                scroll_offset,
            } => {
                for quad in selection.drain(..) {
                    window.paint_quad(quad);
                }
                let line_height = *line_height;
                let scroll_offset = *scroll_offset;
                let painted = std::mem::take(lines);
                let mut stored = Vec::with_capacity(painted.len());
                for prepared in painted {
                    if prepared.origin.y + line_height >= bounds.top()
                        && prepared.origin.y <= bounds.bottom()
                        && let Err(e) = prepared.line.paint(
                            prepared.origin,
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                    {
                        tracing::warn!("input multiline text paint failed: {e}");
                    }
                    stored.push(InputDocLine {
                        line: prepared.line,
                        start: prepared.start,
                    });
                }
                if focus_handle.is_focused(window)
                    && self.input.read(cx).caret_blink.visible()
                    && let Some(cursor) = cursor.take()
                {
                    window.paint_quad(cursor);
                }
                self.input.update(cx, |input, _cx| {
                    input.last_lines = stored;
                    input.last_layout = None;
                    input.last_bounds = Some(bounds);
                    input.line_height = line_height;
                    input.page_scroll_offset = scroll_offset;
                });
            }
            PrepaintState::Single {
                line,
                cursor,
                selection,
            } => {
                if let Some(selection) = selection.take() {
                    window.paint_quad(selection)
                }

                let (token_ranges, token_color, draw_offset) = {
                    let input = self.input.read(cx);
                    (
                        input.token_bg_ranges.clone(),
                        input.token_bg_color,
                        input.draw_offset,
                    )
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
                                    point(bounds.left() + draw_offset + x0 - pad_x, chip_top),
                                    size((x1 - x0) + pad_x * 2., chip_height),
                                ),
                                color,
                            )
                            .corner_radii(Corners::all(px(4.))),
                        );
                    }
                }

                let stored = line.clone();
                if let Err(e) = line.paint(
                    point(bounds.origin.x + draw_offset, bounds.origin.y),
                    window.line_height(),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    tracing::warn!("input text paint failed: {e}");
                }

                if focus_handle.is_focused(window)
                    && self.input.read(cx).caret_blink.visible()
                    && let Some(cursor) = cursor.take()
                {
                    window.paint_quad(cursor);
                }

                self.input.update(cx, |input, _cx| {
                    input.last_layout = Some(stored);
                    input.last_lines.clear();
                    input.last_bounds = Some(bounds);
                });
            }
        }
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
        let multi_line = state.read(cx).multi_line;
        let masked = state.read(cx).masked;
        let toggle_color = cx.theme().text_muted;
        let toggle_id = state.entity_id();

        let toggle = self.mask_toggle.then(|| {
            let state = state.clone();
            div()
                .id(("mask-toggle", toggle_id))
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
            .when(multi_line, |el| {
                el.flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .h(gpui::relative(1.))
            })
            .child(state)
            .when_some(toggle, |el, toggle| el.child(toggle))
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_draw_offset, to_single_line_display};
    use gpui::{SharedString, px};

    #[test]
    fn single_line_display_collapses_newlines_preserving_byte_len() {
        let input = SharedString::from("line1\nline2\r\nline3");
        let output = to_single_line_display(input.clone());
        assert!(!output.contains('\n'));
        assert!(!output.contains('\r'));
        assert_eq!(output.len(), input.len());
        assert_eq!(output.as_ref(), "line1 line2  line3");
    }

    #[test]
    fn single_line_display_leaves_plain_text_untouched() {
        let input = SharedString::from("no breaks here");
        assert_eq!(
            to_single_line_display(input.clone()).as_ref(),
            input.as_ref()
        );
    }

    #[test]
    fn center_offset_centers_short_text() {
        assert_eq!(
            compute_draw_offset(true, px(0.), px(44.), px(12.), px(6.)),
            px(16.)
        );
    }

    #[test]
    fn center_offset_never_negative_for_overflowing_text() {
        assert_eq!(
            compute_draw_offset(true, px(0.), px(20.), px(30.), px(30.)),
            px(0.)
        );
    }

    #[test]
    fn left_offset_zero_when_text_fits() {
        assert_eq!(
            compute_draw_offset(false, px(0.), px(100.), px(50.), px(50.)),
            px(0.)
        );
    }

    #[test]
    fn left_offset_scrolls_to_keep_caret_visible() {
        assert_eq!(
            compute_draw_offset(false, px(0.), px(100.), px(200.), px(200.)),
            px(-100.)
        );
    }

    #[test]
    fn left_offset_scrolls_back_to_start_when_caret_returns() {
        assert_eq!(
            compute_draw_offset(false, px(100.), px(100.), px(200.), px(0.)),
            px(0.)
        );
    }
}
