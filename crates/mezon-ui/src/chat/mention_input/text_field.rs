#[path = "blink_manager.rs"]
mod blink_manager;

use std::any::Any;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use blink_manager::CaretBlink;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, GlobalElementId, Hsla, Image, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    RenderOnce, ScrollWheelEvent, SharedString, Style, StyleRefinement, Styled, Subscription,
    TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, div, fill, point,
    prelude::*, px, rgb, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::components::primitives::text_actions::{
    Backspace, Copy, Cut, Delete, DeleteToLineEnd, DeleteToLineStart, DeleteToNextWordEnd,
    DeleteToPreviousWordStart, Down, End, Enter, Home, Left, MoveToDocEnd, MoveToDocStart,
    MoveToNextWordEnd, MoveToPreviousWordStart, Newline, Paste, Redo, Right, SelectAll, SelectDown,
    SelectLeft, SelectRight, SelectToDocEnd, SelectToDocStart, SelectToLineEnd, SelectToLineStart,
    SelectToNextWordEnd, SelectToPreviousWordStart, SelectUp, ShowCharacterPalette,
    TEXT_INPUT_CONTEXT, Undo, Up,
};
use crate::theme::ActiveTheme;
use crate::util::text_edit::{
    EditKind, HistoryEntry, MAX_UNDO_HISTORY, SelectGranularity, extend_range_for_granularity,
    granularity_for_click, home_target, line_end, line_start, next_word_boundary,
    previous_word_boundary, range_for_granularity, should_coalesce,
};

const MASK: char = '\u{2022}';
const MAX_VISIBLE_LINES: usize = 10;

struct DocLine {
    line: WrappedLine,
    start: usize,
    top: Pixels,
    height: Pixels,
}

fn wrapped_line_height(line: &WrappedLine, line_height: Pixels) -> Pixels {
    line_height * (line.wrap_boundaries.len() as f32 + 1.0)
}

fn locate_display_offset(lines: &[DocLine], display_off: usize) -> (usize, usize) {
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

fn normalize_pasted(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

const EMOJI_SPAN_COLOR: u32 = 0x5A62F4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MentionSpanKind {
    Mention,
    Hashtag,
    Emoji,
}

#[derive(Clone, PartialEq)]
pub(crate) struct MentionSpan {
    pub range: Range<usize>,
    pub kind: MentionSpanKind,
}

struct ResolvedSpan {
    range: Range<usize>,
    color: Hsla,
    bold: bool,
}

pub(crate) fn byte_offset_to_utf16(text: &str, byte_offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;
    for ch in text.chars() {
        if utf8_count >= byte_offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }
    utf16_offset
}

fn build_text_runs(
    text_len: usize,
    base: &TextRun,
    marked: Option<Range<usize>>,
    spans: &[ResolvedSpan],
) -> Vec<TextRun> {
    if marked.is_none() && spans.is_empty() {
        return vec![base.clone()];
    }

    let mut bounds = vec![0usize, text_len];
    if let Some(marked) = &marked {
        bounds.push(marked.start.min(text_len));
        bounds.push(marked.end.min(text_len));
    }
    for span in spans {
        bounds.push(span.range.start.min(text_len));
        bounds.push(span.range.end.min(text_len));
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
            let mut run = base.clone();
            run.len = end - start;
            if let Some(span) = spans
                .iter()
                .find(|span| span.range.start <= start && end <= span.range.end)
            {
                run.color = span.color;
                if span.bold {
                    run.font.weight = FontWeight::BOLD;
                }
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

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MentionFieldEvent {
    Change,
    HistoryRestored,
    PressEnter,
    NavUp,
    NavDown,
    Paste(String),
    PasteImages(Vec<Image>),
    PastePaths(Vec<PathBuf>),
}

pub(crate) struct MentionInputState {
    focus_handle: FocusHandle,
    content: SharedString,
    line_count: usize,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    discard_ime_commit: Option<String>,
    last_lines: Vec<DocLine>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    measured_rows: usize,
    content_height: Pixels,
    pending_caret_reveal: bool,
    is_selecting: bool,
    select_granularity: SelectGranularity,
    select_anchor: Range<usize>,
    masked: bool,
    compact: bool,
    mention_spans: Vec<MentionSpan>,
    caret_blink: CaretBlink,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    last_edit_kind: Option<EditKind>,
    history_payload: Option<Rc<dyn Any>>,
    _window_activation_sub: Subscription,
}

impl EventEmitter<MentionFieldEvent> for MentionInputState {}

impl MentionInputState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let window_activation_sub = cx.observe_window_activation(window, |this, window, cx| {
            this.caret_blink
                .sync_window_active(window.is_window_active(), cx);
        });
        let this = Self {
            focus_handle: focus_handle.clone(),
            content: SharedString::default(),
            line_count: 1,
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            discard_ime_commit: None,
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: px(20.),
            scroll_offset: Point::default(),
            measured_rows: 1,
            content_height: px(0.),
            pending_caret_reveal: true,
            is_selecting: false,
            select_granularity: SelectGranularity::Character,
            select_anchor: 0..0,
            masked: false,
            compact: false,
            mention_spans: Vec::new(),
            caret_blink: CaretBlink::new(window.is_window_active()),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
            history_payload: None,
            _window_activation_sub: window_activation_sub,
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
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            cx.notify();
        }
    }

    pub(crate) fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn value(&self) -> &str {
        self.content.as_ref()
    }

    pub fn value_shared(&self) -> SharedString {
        self.content.clone()
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor_offset()
    }

    pub(crate) fn set_mention_spans(&mut self, spans: Vec<MentionSpan>, cx: &mut Context<Self>) {
        if self.mention_spans != spans {
            self.mention_spans = spans;
            cx.notify();
        }
    }

    pub(crate) fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len()).max(start);
        self.record_history(EditKind::Other);
        let mut next = String::with_capacity(self.content.len() - (end - start) + text.len());
        next.push_str(&self.content[..start]);
        next.push_str(text);
        next.push_str(&self.content[end..]);
        self.set_content(next);
        let caret = start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(MentionFieldEvent::Change);
    }

    pub(crate) fn prepare_channel_switch(&mut self) {
        if let Some(marked) = self.marked_range.take() {
            self.discard_ime_commit = self.content.get(marked).map(str::to_string);
        }
    }

    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_content(value);
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        self.clear_history();
        self.history_payload = None;
        cx.notify();
        cx.emit(MentionFieldEvent::Change);
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
                window.play_system_bell();
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
                window.play_system_bell();
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
                window.play_system_bell();
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
            payload: self.history_payload.clone(),
        }
    }

    pub(crate) fn set_history_payload(&mut self, payload: Option<Rc<dyn Any>>) {
        self.history_payload = payload;
    }

    pub(crate) fn history_payload(&self) -> Option<Rc<dyn Any>> {
        self.history_payload.clone()
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
        self.history_payload = entry.payload;
        self.pending_caret_reveal = true;
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(MentionFieldEvent::HistoryRestored);
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
        self.replace_text_in_range(None, "", window, cx)
    }

    fn enter(&mut self, _: &Enter, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MentionFieldEvent::PressEnter);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn up(&mut self, _: &Up, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MentionFieldEvent::NavUp);
    }

    fn down(&mut self, _: &Down, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MentionFieldEvent::NavDown);
    }

    fn caret_line_target(&self, delta: isize) -> usize {
        if self.last_lines.len() <= 1 {
            return if delta < 0 { 0 } else { self.content.len() };
        }
        let line_height = self.line_height;
        let display_cursor = self.to_display_offset(self.cursor_offset());
        let (line_ix, local) = locate_display_offset(&self.last_lines, display_cursor);
        let caret_x = self.last_lines[line_ix]
            .line
            .position_for_index(local, line_height)
            .map(|p| p.x)
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

    pub(crate) fn move_caret_line(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.move_to(self.caret_line_target(delta), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.caret_line_target(-1), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.caret_line_target(1), cx);
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

        let range = range_for_granularity(&self.content, offset, self.select_granularity, true);
        self.select_anchor = range.clone();
        self.selection_reversed = false;
        self.selected_range = range;
        self.last_edit_kind = None;
        self.pending_caret_reveal = true;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
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
        let (range, reversed) = extend_range_for_granularity(
            &self.content,
            &self.select_anchor,
            offset,
            self.select_granularity,
            true,
        );
        if range == self.selected_range && reversed == self.selection_reversed {
            return;
        }
        self.selected_range = range;
        self.selection_reversed = reversed;
        self.last_edit_kind = None;
        self.pending_caret_reveal = true;
        self.caret_blink.pause_blinking(cx);
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

    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let images: Vec<Image> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
            .collect();
        if !images.is_empty() {
            cx.emit(MentionFieldEvent::PasteImages(images));
            return;
        }
        let paths: Vec<PathBuf> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
                _ => None,
            })
            .flatten()
            .collect();
        if !paths.is_empty() {
            cx.emit(MentionFieldEvent::PastePaths(paths));
            return;
        }
        if let Some(text) = item.text() {
            cx.emit(MentionFieldEvent::Paste(normalize_pasted(&text)));
        }
    }

    pub(crate) fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
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
        self.last_edit_kind = None;
        self.pending_caret_reveal = true;
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

    fn set_content(&mut self, content: impl Into<SharedString>) {
        self.content = content.into();
        self.line_count = self.content.split('\n').count().max(1);
        self.selected_range = self.clamp_range(self.selected_range.clone());
        if let Some(marked) = self.marked_range.clone() {
            self.marked_range = Some(self.clamp_range(marked));
        }
        self.pending_caret_reveal = true;
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let max_scroll = (self.content_height - bounds.size.height).max(Pixels::ZERO);
        if max_scroll <= Pixels::ZERO {
            return;
        }
        let delta = event.delta.pixel_delta(self.line_height).y;
        let next = (self.scroll_offset.y - delta).clamp(Pixels::ZERO, max_scroll);
        if next != self.scroll_offset.y {
            self.scroll_offset.y = next;
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn visible_line_count(&self) -> usize {
        if self.is_masked() || self.content.is_empty() {
            1
        } else {
            self.measured_rows.max(self.line_count)
        }
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
            return self.display_to_content_offset(self.last_lines[0].start);
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
        self.display_to_content_offset(doc.start + local)
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
        self.pending_caret_reveal = true;
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
        byte_offset_to_utf16(&self.content, offset)
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

impl EntityInputHandler for MentionInputState {
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
            let marked = self.clamp_range(marked);
            self.discard_ime_commit = self.content.get(marked).map(str::to_string);
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
        if range_utf16.is_none()
            && self.marked_range.is_none()
            && let Some(expected) = self.discard_ime_commit.as_deref()
        {
            if new_text == expected {
                self.discard_ime_commit = None;
                return;
            }
            if !new_text.is_empty() {
                self.discard_ime_commit = None;
            }
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
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
        self.marked_range.take();
        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(MentionFieldEvent::Change);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if range_utf16.is_none()
            && self.marked_range.is_none()
            && let Some(expected) = self.discard_ime_commit.as_deref()
        {
            if new_text == expected {
                self.discard_ime_commit = None;
                return;
            }
            if !new_text.is_empty() {
                self.discard_ime_commit = None;
            }
        }
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(range);

        if self.marked_range.is_none() {
            self.record_history(EditKind::Insert);
        }

        let next = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.set_content(next);
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.pause_caret_blink(cx);
        cx.notify();
        cx.emit(MentionFieldEvent::Change);
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
        let (s_line, s_local) =
            locate_display_offset(&self.last_lines, self.to_display_offset(range.start));
        let (e_line, e_local) =
            locate_display_offset(&self.last_lines, self.to_display_offset(range.end));
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
            point(bounds.left() + s_pos.x, top),
            point(bounds.left() + e_pos.x, top + line_height),
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

impl Focusable for MentionInputState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MentionInputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused {
            self.caret_blink.sync_focused(cx);
        } else {
            self.caret_blink.sync_blurred(cx);
        }

        let compact = self.compact;
        let text_color: Hsla = if compact {
            cx.theme().tokens.text_theme_message.into()
        } else {
            cx.theme().text_primary.into()
        };

        let viewport_h = self
            .last_bounds
            .map(|bounds| bounds.size.height)
            .unwrap_or(Pixels::ZERO);
        let content_h = self.content_height;
        let scrollbar = (viewport_h > Pixels::ZERO && content_h > viewport_h + px(1.)).then(|| {
            let ratio = viewport_h / content_h;
            let thumb_h = (viewport_h * ratio).max(px(24.)).min(viewport_h);
            let max_scroll = (content_h - viewport_h).max(px(1.));
            let frac = (self.scroll_offset.y / max_scroll).clamp(0., 1.);
            let thumb_top = (viewport_h - thumb_h) * frac;
            let thumb_color: Hsla = cx.theme().tokens.thread_scroll.into();
            div()
                .absolute()
                .top(px(9.))
                .right(px(3.))
                .w(px(4.))
                .h(viewport_h)
                .child(
                    div()
                        .id("mention-input-scrollbar-thumb")
                        .absolute()
                        .top(thumb_top)
                        .w(px(4.))
                        .h(thumb_h)
                        .rounded_full()
                        .bg(thumb_color)
                        .when(!focused, |thumb| {
                            thumb
                                .opacity(0.)
                                .group_hover("mention-input-scrollbar", |style| style.opacity(1.))
                        }),
                )
        });

        div()
            .relative()
            .when(!focused, |input| input.group("mention-input-scrollbar"))
            .key_context(TEXT_INPUT_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
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
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .flex()
            .items_start()
            .w_full()
            .when(compact, |d| d.min_h(px(40.)).px(px(10.)).py(px(9.)))
            .when(!compact, |d| {
                d.min_h(px(44.)).pl(px(44.)).pr(px(120.)).py(px(9.))
            })
            .text_color(text_color)
            .text_size(px(16.))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(MentionTextElement { input: cx.entity() }),
            )
            .when_some(scrollbar, |el, bar| el.child(bar))
    }
}

struct MentionTextElement {
    input: Entity<MentionInputState>,
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

impl IntoElement for MentionTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MentionTextElement {
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
        let line_count = self.input.read(cx).visible_line_count();
        let visible = line_count.clamp(1, MAX_VISIBLE_LINES);
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
        let brand_color: Hsla = cx.theme().brand.into();
        let emoji_color: Hsla = rgb(EMOJI_SPAN_COLOR).into();

        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let masked = input.is_masked();
        let reveal_caret = input.pending_caret_reveal;
        let resolved_spans: Vec<ResolvedSpan> = input
            .mention_spans
            .iter()
            .map(|span| {
                let (color, bold) = match span.kind {
                    MentionSpanKind::Mention | MentionSpanKind::Hashtag => (brand_color, false),
                    MentionSpanKind::Emoji => (emoji_color, true),
                };
                ResolvedSpan {
                    range: span.range.clone(),
                    color,
                    bold,
                }
            })
            .collect();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), placeholder_color)
        } else {
            (input.display_text(), style.color)
        };

        let marked_range = input.marked_range.clone();
        let display_cursor = input.to_display_offset(cursor).min(display_text.len());
        let display_selection = input
            .to_display_offset(selected_range.start)
            .min(display_text.len())
            ..input
                .to_display_offset(selected_range.end)
                .min(display_text.len());

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if masked {
            vec![run]
        } else {
            build_text_runs(display_text.len(), &run, marked_range, &resolved_spans)
        };

        let mut scroll_offset = input.scroll_offset;
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let visible_h = bounds.size.height;
        let visible_w = bounds.size.width;
        let wrap_width = if masked { None } else { Some(visible_w) };
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

        let (caret_line, caret_local) = locate_span(&spans, display_cursor);
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
        scroll_offset.x = Pixels::ZERO;

        let mut prepared = Vec::with_capacity(wrapped.len());
        let mut selection = Vec::new();
        for (ix, line) in wrapped.into_iter().enumerate() {
            let (line_start, line_len) = spans[ix];
            let line_end = line_start + line_len;
            let (line_top, line_h) = tops[ix];
            let origin = point(bounds.left(), bounds.top() + line_top - scroll_offset.y);
            if !display_selection.is_empty()
                && display_selection.start <= line_end
                && display_selection.end >= line_start
            {
                let local_start = display_selection.start.max(line_start) - line_start;
                let extends = display_selection.end > line_end;
                let local_end = if extends {
                    line_len
                } else {
                    display_selection.end - line_start
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
                            point(bounds.left() + x_from, row_y),
                            point(bounds.left() + x_to, row_y + line_height),
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

        let cursor = if display_selection.is_empty() {
            Some(fill(
                Bounds::new(
                    point(
                        bounds.left() + caret_x,
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
                tracing::warn!("mention input text paint failed: {e}");
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
pub(crate) struct MentionInputField {
    state: Entity<MentionInputState>,
    base: Div,
}

impl MentionInputField {
    pub fn new(state: &Entity<MentionInputState>) -> Self {
        Self {
            state: state.clone(),
            base: div().w_full(),
        }
    }
}

impl Styled for MentionInputField {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for MentionInputField {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.base.relative().child(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::font;

    #[test]
    fn byte_to_utf16_ascii_is_identity() {
        assert_eq!(byte_offset_to_utf16("hello", 0), 0);
        assert_eq!(byte_offset_to_utf16("hello", 5), 5);
        assert_eq!(byte_offset_to_utf16("@bob world", 4), 4);
    }

    #[test]
    fn byte_to_utf16_counts_leading_emoji_as_surrogate_pair() {
        let text = "😀@bob";
        assert_eq!("😀".len(), 4);
        assert_eq!(byte_offset_to_utf16(text, 4), 2);
        assert_eq!(byte_offset_to_utf16(text, 5), 3);
        assert_eq!(byte_offset_to_utf16(text, 8), 6);
    }

    #[test]
    fn byte_to_utf16_counts_vietnamese_as_single_unit() {
        let text = "Tô@an";
        assert_eq!("ô".len(), 2);
        assert_eq!(byte_offset_to_utf16(text, 3), 2);
        assert_eq!(byte_offset_to_utf16(text, 4), 3);
    }

    fn base_run(len: usize) -> TextRun {
        TextRun {
            len,
            font: font("Helvetica"),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn resolved(range: Range<usize>, color: Hsla, bold: bool) -> ResolvedSpan {
        ResolvedSpan { range, color, bold }
    }

    #[test]
    fn runs_without_mentions_or_marked_stay_single() {
        let runs = build_text_runs(5, &base_run(5), None, &[]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 5);
    }

    #[test]
    fn runs_color_mention_segment() {
        let brand = Hsla {
            h: 0.6,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        let runs = build_text_runs(10, &base_run(10), None, &[resolved(0..4, brand, false)]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 4);
        assert_eq!(runs[0].color, brand);
        assert_eq!(runs[1].len, 6);
        assert_ne!(runs[1].color, brand);
    }

    #[test]
    fn runs_make_emoji_span_bold() {
        let color = rgb(EMOJI_SPAN_COLOR).into();
        let runs = build_text_runs(10, &base_run(10), None, &[resolved(0..7, color, true)]);
        assert_eq!(runs[0].len, 7);
        assert_eq!(runs[0].color, color);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_ne!(runs[1].font.weight, FontWeight::BOLD);
    }

    #[test]
    fn runs_preserve_marked_underline() {
        let runs = build_text_runs(6, &base_run(6), Some(2..4), &[]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 6);
        let underlined: usize = runs
            .iter()
            .filter(|r| r.underline.is_some())
            .map(|r| r.len)
            .sum();
        assert_eq!(underlined, 2);
    }

    fn line_spans(text: &str) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut start = 0usize;
        for line in text.split('\n') {
            spans.push((start, line.len()));
            start += line.len() + 1;
        }
        spans
    }

    #[test]
    fn paste_preserves_newlines_and_tabs() {
        let pasted = "fn main() {\r\n\tprintln!(\"hi\");\r\n}";
        let normalized = normalize_pasted(pasted);
        assert_eq!(normalized, "fn main() {\n\tprintln!(\"hi\");\n}");
        assert_eq!(normalized.matches('\n').count(), 2);
        assert!(normalized.contains('\t'));
        assert!(!normalized.contains('\r'));
    }

    #[test]
    fn line_count_follows_hard_newlines() {
        assert_eq!("".split('\n').count(), 1);
        assert_eq!("one".split('\n').count(), 1);
        assert_eq!("a\nb\nc".split('\n').count(), 3);
        assert_eq!("a\n\nb".split('\n').count(), 3);
    }

    #[test]
    fn locate_span_maps_offset_to_line_and_column() {
        let text = "ab\ncde\nf";
        let spans = line_spans(text);
        assert_eq!(spans, vec![(0, 2), (3, 3), (7, 1)]);
        assert_eq!(locate_span(&spans, 0), (0, 0));
        assert_eq!(locate_span(&spans, 2), (0, 2));
        assert_eq!(locate_span(&spans, 3), (1, 0));
        assert_eq!(locate_span(&spans, 6), (1, 3));
        assert_eq!(locate_span(&spans, 7), (2, 0));
        assert_eq!(locate_span(&spans, 8), (2, 1));
    }

    #[test]
    fn locate_span_on_empty_line() {
        let spans = line_spans("a\n\nb");
        assert_eq!(spans, vec![(0, 1), (2, 0), (3, 1)]);
        assert_eq!(locate_span(&spans, 2), (1, 0));
    }
}
