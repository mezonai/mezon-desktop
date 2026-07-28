use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Div, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, RenderOnce, SharedString,
    Style, StyleRefinement, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine, actions, div, fill, point, prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

use mezon_theme::ActiveTheme;
use mezon_widgets::blink_manager::{CaretBlink, HasCaretBlink};

use crate::util::text_edit::{
    EditKind, HistoryEntry, MAX_UNDO_HISTORY, home_target, line_end, line_start,
    next_word_boundary, previous_word_boundary, should_coalesce,
};

const KEY_CONTEXT: &str = "MezonTextArea";
const DEFAULT_MAX_VISIBLE_LINES: usize = 8;

actions!(
    mezon_textarea,
    [
        Backspace,
        Delete,
        Enter,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        MoveToPreviousWordStart,
        MoveToNextWordEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        SelectToLineStart,
        SelectToLineEnd,
        MoveToDocStart,
        MoveToDocEnd,
        SelectToDocStart,
        SelectToDocEnd,
        DeleteToLineStart,
        DeleteToLineEnd,
        Undo,
        Redo,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

pub fn init(cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Enter, Some(KEY_CONTEXT)),
        KeyBinding::new("up", Up, Some(KEY_CONTEXT)),
        KeyBinding::new("down", Down, Some(KEY_CONTEXT)),
        KeyBinding::new("left", Left, Some(KEY_CONTEXT)),
        KeyBinding::new("right", Right, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("home", Home, Some(KEY_CONTEXT)),
        KeyBinding::new("end", End, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-home", SelectToLineStart, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-end", SelectToLineEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-z", Undo, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-z", Redo, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(KEY_CONTEXT)),
    ];

    #[cfg(target_os = "macos")]
    bindings.extend([
        KeyBinding::new("alt-left", MoveToPreviousWordStart, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-right", MoveToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "alt-shift-left",
            SelectToPreviousWordStart,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "alt-backspace",
            DeleteToPreviousWordStart,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-left", Home, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-right", End, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-left", SelectToLineStart, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-right", SelectToLineEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-up", MoveToDocStart, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-down", MoveToDocEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-up", SelectToDocStart, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-down", SelectToDocEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, Some(KEY_CONTEXT)),
    ]);

    #[cfg(not(target_os = "macos"))]
    bindings.extend([
        KeyBinding::new("ctrl-left", MoveToPreviousWordStart, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-right", MoveToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-left",
            SelectToPreviousWordStart,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "ctrl-backspace",
            DeleteToPreviousWordStart,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-home", MoveToDocStart, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-end", MoveToDocEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-home", SelectToDocStart, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-end", SelectToDocEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-y", Redo, Some(KEY_CONTEXT)),
    ]);

    cx.bind_keys(bindings);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextAreaEvent {
    Change,
    PressEnter,
}

fn normalize_pasted(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn byte_offset_to_utf16(text: &str, byte_offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in text.chars() {
        if utf8 >= byte_offset {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

struct DocLine {
    line: WrappedLine,
    start: usize,
    top: Pixels,
    height: Pixels,
}

fn wrapped_line_height(line: &WrappedLine, line_height: Pixels) -> Pixels {
    line_height * (line.wrap_boundaries.len() as f32 + 1.0)
}

fn locate_display_offset(lines: &[DocLine], offset: usize) -> (usize, usize) {
    for (ix, doc) in lines.iter().enumerate() {
        if offset <= doc.start + doc.line.len() {
            return (ix, offset.saturating_sub(doc.start));
        }
    }
    match lines.last() {
        Some(doc) => (lines.len() - 1, offset.saturating_sub(doc.start)),
        None => (0, 0),
    }
}

fn locate_span(spans: &[(usize, usize)], off: usize) -> (usize, usize) {
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

pub struct TextArea {
    focus_handle: FocusHandle,
    content: SharedString,
    line_count: usize,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<DocLine>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    measured_rows: usize,
    content_height: Pixels,
    pending_caret_reveal: bool,
    is_selecting: bool,
    single_line: bool,
    bg: Option<Hsla>,
    text_color: Option<Hsla>,
    text_size: Pixels,
    radius: Pixels,
    padding_x: Pixels,
    min_height: Pixels,
    max_visible_lines: usize,
    caret_blink: CaretBlink,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    last_edit_kind: Option<EditKind>,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

impl HasCaretBlink for TextArea {
    fn caret_blink_mut(&mut self) -> &mut CaretBlink {
        &mut self.caret_blink
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextArea {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let this = Self {
            focus_handle: focus_handle.clone(),
            content: SharedString::default(),
            line_count: 1,
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: px(20.),
            scroll_offset: Point::default(),
            measured_rows: 1,
            content_height: px(0.),
            pending_caret_reveal: true,
            is_selecting: false,
            single_line: false,
            bg: None,
            text_color: None,
            text_size: px(14.),
            radius: px(4.),
            padding_x: px(12.),
            min_height: px(36.),
            max_visible_lines: DEFAULT_MAX_VISIBLE_LINES,
            caret_blink: CaretBlink::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
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

    pub fn single_line(mut self, single_line: bool) -> Self {
        self.single_line = single_line;
        if single_line {
            self.max_visible_lines = 1;
        }
        self
    }

    pub fn bg(mut self, bg: impl Into<Hsla>) -> Self {
        self.bg = Some(bg.into());
        self
    }

    pub fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_color = Some(color.into());
        self
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = size;
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    pub fn padding_x(mut self, padding: Pixels) -> Self {
        self.padding_x = padding;
        self
    }

    pub fn min_height(mut self, height: Pixels) -> Self {
        self.min_height = height;
        self
    }

    pub fn max_visible_lines(mut self, lines: usize) -> Self {
        if !self.single_line {
            self.max_visible_lines = lines.max(1);
        }
        self
    }

    pub fn value(&self) -> &str {
        self.content.as_ref()
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.set_content(value);
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        self.clear_history();
        cx.notify();
        cx.emit(TextAreaEvent::Change);
    }

    fn set_content(&mut self, content: impl Into<SharedString>) {
        self.content = content.into();
        self.line_count = self.content.split('\n').count().max(1);
        self.selected_range = self.clamp_range(self.selected_range.clone());
        if let Some(marked) = self.marked_range.clone() {
            self.marked_range = Some(self.clamp_range(marked));
        }
        self.pending_caret_reveal = true;
    }

    fn visible_line_count(&self) -> usize {
        if self.single_line || self.content.is_empty() {
            1
        } else {
            self.measured_rows.max(self.line_count)
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.clamp_offset(range.start);
        let end = self.clamp_offset(range.end).max(start);
        start..end
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.last_edit_kind = None;
        self.pending_caret_reveal = true;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.last_edit_kind = None;
        self.extend_selection(offset, cx);
    }

    fn extend_selection(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.pending_caret_reveal = true;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
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

    fn move_caret_line(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.single_line || self.last_lines.len() <= 1 {
            if delta < 0 {
                self.move_to(0, cx);
            } else {
                self.move_to(self.content.len(), cx);
            }
            return;
        }
        let line_height = self.line_height;
        let (line_ix, local) = locate_display_offset(&self.last_lines, self.cursor_offset());
        let caret_x = self.last_lines[line_ix]
            .line
            .position_for_index(local, line_height)
            .map(|p| p.x)
            .unwrap_or(Pixels::ZERO);
        let target_ix = line_ix as isize + delta;
        if target_ix < 0 {
            self.move_to(0, cx);
            return;
        }
        if target_ix as usize >= self.last_lines.len() {
            self.move_to(self.content.len(), cx);
            return;
        }
        let target = &self.last_lines[target_ix as usize];
        let local_target = target
            .line
            .closest_index_for_position(point(caret_x, px(0.)), line_height)
            .unwrap_or_else(|ix| ix);
        self.move_to(target.start + local_target, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() || self.last_lines.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        let line_height = self.line_height;
        let rel_y = position.y - bounds.top() + self.scroll_offset.y;
        let rel_x = (position.x - bounds.left() + self.scroll_offset.x).max(Pixels::ZERO);
        if rel_y < Pixels::ZERO {
            return self.last_lines[0].start;
        }
        let doc = self
            .last_lines
            .iter()
            .find(|doc| rel_y < doc.top + doc.height)
            .unwrap_or_else(|| self.last_lines.last().expect("non-empty"));
        let local_y =
            (rel_y - doc.top).clamp(Pixels::ZERO, (doc.height - line_height).max(Pixels::ZERO));
        let local = doc
            .line
            .closest_index_for_position(point(rel_x, local_y), line_height)
            .unwrap_or_else(|ix| ix);
        doc.start + local
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_caret_line(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_caret_line(1, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
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
                return;
            }
            self.extend_selection(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
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
                return;
            }
            self.extend_selection(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
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
                return;
            }
            self.extend_selection(target, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
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
                return;
            }
            self.extend_selection(target, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
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
        self.line_count = self.content.split('\n').count().max(1);
        self.selected_range = entry.selected_range;
        self.selection_reversed = entry.selection_reversed;
        self.marked_range = None;
        self.last_edit_kind = None;
        self.pending_caret_reveal = true;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
        cx.emit(TextAreaEvent::Change);
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
                return;
            }
            self.extend_selection(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.extend_selection(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.single_line {
            cx.emit(TextAreaEvent::PressEnter);
        } else {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let normalized = normalize_pasted(&text);
        let sanitized = if self.single_line {
            normalized.replace('\n', " ")
        } else {
            normalized
        };
        self.replace_text_in_range(None, &sanitized, window, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
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
            self.move_to(self.index_for_mouse_position(event.position), cx);
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

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        byte_offset_to_utf16(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for TextArea {
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
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(range);
        let kind = if self.marked_range.is_some() {
            EditKind::Insert
        } else if new_text.is_empty() {
            EditKind::Delete
        } else if range.is_empty() && !new_text.contains('\n') {
            EditKind::Insert
        } else {
            EditKind::Other
        };
        self.record_history(kind);
        let next = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.set_content(next);
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range = None;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
        cx.emit(TextAreaEvent::Change);
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
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(range);
        if self.marked_range.is_none() {
            self.record_history(EditKind::Insert);
        }
        let next = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.set_content(next);
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.caret_blink.pause_blinking(cx);
        cx.notify();
        cx.emit(TextAreaEvent::Change);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line_height = self.line_height;
        let caret_x = bounds.left() - self.scroll_offset.x;
        let caret_top = bounds.top() - self.scroll_offset.y;
        let fallback = Bounds::from_corners(
            point(caret_x, caret_top),
            point(caret_x, caret_top + line_height),
        );
        if self.last_lines.is_empty() {
            return Some(fallback);
        }
        let range = self.range_from_utf16(&range_utf16);
        let (s_line, s_local) = locate_display_offset(&self.last_lines, range.start);
        let (e_line, e_local) = locate_display_offset(&self.last_lines, range.end);
        let (Some(s_doc), Some(e_doc)) = (self.last_lines.get(s_line), self.last_lines.get(e_line))
        else {
            return Some(fallback);
        };
        let s_pos = s_doc
            .line
            .position_for_index(s_local, line_height)
            .unwrap_or_default();
        let e_pos = e_doc
            .line
            .position_for_index(e_local, line_height)
            .unwrap_or_default();
        let top = bounds.top() + s_doc.top + s_pos.y - self.scroll_offset.y;
        Some(Bounds::from_corners(
            point(bounds.left() + s_pos.x - self.scroll_offset.x, top),
            point(
                bounds.left() + e_pos.x - self.scroll_offset.x,
                top + line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.last_bounds?;
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_handle.is_focused(window) {
            self.caret_blink.sync_focused(cx);
        } else {
            self.caret_blink.sync_blurred(cx);
        }
        let bg = self.bg.unwrap_or(cx.theme().bg_tertiary.into());
        let padding_x = self.padding_x;
        let radius = self.radius;
        let min_height = self.min_height;

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
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
            .flex()
            .items_start()
            .w_full()
            .min_h(min_height)
            .px(padding_x)
            .py(px(8.))
            .rounded(radius)
            .bg(bg)
            .text_size(self.text_size)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(TextAreaElement { input: cx.entity() }),
            )
    }
}

struct TextAreaElement {
    input: Entity<TextArea>,
}

struct PreparedLine {
    line: WrappedLine,
    origin: Point<Pixels>,
    start: usize,
    top: Pixels,
    height: Pixels,
}

struct PrepaintState {
    lines: Vec<PreparedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    content_height: Pixels,
}

impl IntoElement for TextAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextAreaElement {
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
        let input = self.input.read(cx);
        let visible = input.visible_line_count().clamp(1, input.max_visible_lines);
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = (window.line_height() * visible as f32).into();
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
        let single_line = input.single_line;
        let reveal_caret = input.pending_caret_reveal;
        let style = window.text_style();
        let text_color = input.text_color.unwrap_or(style.color);

        let (display_text, color) = if content.is_empty() {
            (input.placeholder.clone(), placeholder_color)
        } else {
            (content.clone(), text_color)
        };
        let marked_range = input.marked_range.clone();

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match &marked_range {
            Some(marked) if !marked.is_empty() && marked.end <= display_text.len() => {
                let mut before = run.clone();
                before.len = marked.start;
                let mut mid = run.clone();
                mid.len = marked.end - marked.start;
                mid.underline = Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                });
                let mut after = run.clone();
                after.len = display_text.len() - marked.end;
                [before, mid, after]
                    .into_iter()
                    .filter(|r| r.len > 0)
                    .collect()
            }
            _ => vec![run],
        };

        let mut scroll_offset = input.scroll_offset;
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let visible_h = bounds.size.height;
        let visible_w = bounds.size.width;
        let wrap_width = if single_line { None } else { Some(visible_w) };
        let wrapped = window
            .text_system()
            .shape_text(display_text, font_size, &runs, wrap_width, None)
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

        let mut tops: Vec<(Pixels, Pixels)> = Vec::with_capacity(wrapped.len());
        let mut y_acc = Pixels::ZERO;
        for line in wrapped.iter() {
            let h = wrapped_line_height(line, line_height);
            tops.push((y_acc, h));
            y_acc += h;
        }
        let total_h = y_acc.max(line_height);

        let (caret_line, caret_local) = locate_span(&spans, cursor);
        let caret_point = wrapped
            .get(caret_line)
            .and_then(|line| line.position_for_index(caret_local, line_height))
            .unwrap_or_default();
        let caret_x = caret_point.x;
        let caret_top = tops
            .get(caret_line)
            .map(|(t, _)| *t)
            .unwrap_or(Pixels::ZERO)
            + caret_point.y;

        if reveal_caret {
            if caret_top < scroll_offset.y {
                scroll_offset.y = caret_top;
            }
            if caret_top + line_height > scroll_offset.y + visible_h {
                scroll_offset.y = caret_top + line_height - visible_h;
            }
        }
        scroll_offset.y = scroll_offset
            .y
            .clamp(Pixels::ZERO, (total_h - visible_h).max(Pixels::ZERO));
        if single_line {
            if reveal_caret {
                if caret_x < scroll_offset.x {
                    scroll_offset.x = caret_x;
                }
                if caret_x > scroll_offset.x + visible_w - px(6.) {
                    scroll_offset.x = caret_x - visible_w + px(6.);
                }
            }
            scroll_offset.x = scroll_offset.x.max(Pixels::ZERO);
        } else {
            scroll_offset.x = Pixels::ZERO;
        }

        let selection_range = selected_range.start.min(selected_range.end)
            ..selected_range.start.max(selected_range.end);
        let mut prepared = Vec::with_capacity(wrapped.len());
        let mut selection = Vec::new();
        for (ix, line) in wrapped.into_iter().enumerate() {
            let (line_start, line_len) = spans[ix];
            let line_end = line_start + line_len;
            let (line_top, line_h) = tops[ix];
            let origin = point(
                bounds.left() - scroll_offset.x,
                bounds.top() + line_top - scroll_offset.y,
            );
            if !selection_range.is_empty()
                && selection_range.start <= line_end
                && selection_range.end >= line_start
            {
                let local_start = selection_range.start.max(line_start) - line_start;
                let extends = selection_range.end > line_end;
                let local_end = if extends {
                    line_len
                } else {
                    selection_range.end - line_start
                };
                let p0 = line
                    .position_for_index(local_start, line_height)
                    .unwrap_or_default();
                let p1 = line
                    .position_for_index(local_end, line_height)
                    .unwrap_or_default();
                let row0 = (p0.y / line_height).round() as usize;
                let row1 = (p1.y / line_height).round().max(row0 as f32) as usize;
                for row in row0..=row1 {
                    let row_y =
                        bounds.top() + line_top + line_height * row as f32 - scroll_offset.y;
                    let x_from = if row == row0 { p0.x } else { Pixels::ZERO };
                    let mut x_to = if row == row1 { p1.x } else { visible_w };
                    if row == row1 && extends {
                        x_to += px(4.);
                    }
                    selection.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + x_from - scroll_offset.x, row_y),
                            point(bounds.left() + x_to - scroll_offset.x, row_y + line_height),
                        ),
                        selection_color.opacity(0.3),
                    ));
                }
            }
            prepared.push(PreparedLine {
                line,
                origin,
                start: line_start,
                top: line_top,
                height: line_h,
            });
        }

        let cursor = if selection_range.is_empty() {
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

        PrepaintState {
            lines: prepared,
            cursor,
            selection,
            line_height,
            scroll_offset,
            content_height: total_h,
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
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = prepaint.line_height;
        let content_height = prepaint.content_height;
        let lines = std::mem::take(&mut prepaint.lines);
        let mut stored = Vec::with_capacity(lines.len());
        for prepared in lines {
            if prepared.origin.y + prepared.height >= bounds.top()
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
                tracing::warn!("textarea text paint failed: {e}");
            }
            stored.push(DocLine {
                line: prepared.line,
                start: prepared.start,
                top: prepared.top,
                height: prepared.height,
            });
        }

        if focus_handle.is_focused(window)
            && self.input.read(cx).caret_blink.visible()
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let scroll_offset = prepaint.scroll_offset;
        let measured_rows = (content_height / line_height).round().max(1.) as usize;
        self.input.update(cx, |input, cx| {
            input.last_lines = stored;
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
            input.scroll_offset = scroll_offset;
            input.content_height = content_height;
            input.pending_caret_reveal = false;
            if input.measured_rows != measured_rows {
                input.measured_rows = measured_rows;
                if !input.content.is_empty() {
                    cx.notify();
                }
            }
        });
    }
}

#[derive(IntoElement)]
pub struct TextAreaField {
    state: Entity<TextArea>,
    base: Div,
}

impl TextAreaField {
    pub fn new(state: &Entity<TextArea>) -> Self {
        Self {
            state: state.clone(),
            base: div().w_full(),
        }
    }
}

impl Styled for TextAreaField {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for TextAreaField {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.base.child(self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{byte_offset_to_utf16, locate_span, normalize_pasted};

    #[test]
    fn normalize_pasted_unifies_line_endings() {
        assert_eq!(normalize_pasted("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(normalize_pasted("plain"), "plain");
    }

    #[test]
    fn byte_offset_to_utf16_counts_surrogates() {
        assert_eq!(byte_offset_to_utf16("abc", 2), 2);
        assert_eq!(byte_offset_to_utf16("é", 2), 1);
        assert_eq!(byte_offset_to_utf16("😀x", 4), 2);
    }

    #[test]
    fn locate_span_maps_offset_to_line_and_local() {
        let spans = [(0usize, 5usize), (6, 4), (11, 3)];
        assert_eq!(locate_span(&spans, 0), (0, 0));
        assert_eq!(locate_span(&spans, 3), (0, 3));
        assert_eq!(locate_span(&spans, 7), (1, 1));
        assert_eq!(locate_span(&spans, 13), (2, 2));
    }
}
