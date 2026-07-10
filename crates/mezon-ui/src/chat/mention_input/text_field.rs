#[path = "blink_manager.rs"]
mod blink_manager;

use std::ops::Range;
use std::path::PathBuf;

use blink_manager::CaretBlink;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, GlobalElementId, Hsla, Image, InspectorElementId, IntoElement, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    Render, RenderOnce, SharedString, Style, StyleRefinement, Styled, Subscription, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point,
    prelude::*, px, rgb, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::ActiveTheme;

const MASK: char = '\u{2022}';
const KEY_CONTEXT: &str = "MezonMentionInput";
const MAX_VISIBLE_LINES: usize = 10;

struct DocLine {
    line: WrappedLine,
    start: usize,
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

actions!(
    mezon_mention_input,
    [
        Backspace,
        Delete,
        Enter,
        Newline,
        Up,
        Down,
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

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Enter, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(KEY_CONTEXT)),
        KeyBinding::new("up", Up, Some(KEY_CONTEXT)),
        KeyBinding::new("down", Down, Some(KEY_CONTEXT)),
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

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MentionFieldEvent {
    Change,
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
    last_lines: Vec<DocLine>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    is_selecting: bool,
    masked: bool,
    compact: bool,
    mention_spans: Vec<MentionSpan>,
    caret_blink: CaretBlink,
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
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: px(20.),
            scroll_offset: Point::default(),
            is_selecting: false,
            masked: false,
            compact: false,
            mention_spans: Vec::new(),
            caret_blink: CaretBlink::new(window.is_window_active()),
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

    pub(crate) fn move_caret_line(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.last_lines.len() <= 1 {
            if delta < 0 {
                self.move_to(0, cx);
            } else {
                self.move_to(self.content.len(), cx);
            }
            return;
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
        let display_offset = target.start + local_target;
        let offset = self.display_to_content_offset(display_offset);
        self.move_to(offset, cx);
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
    }

    fn visible_line_count(&self) -> usize {
        if self.is_masked() { 1 } else { self.line_count }
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
        let rel_x = position.x - bounds.left() + self.scroll_offset.x;
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
        self.display_to_content_offset(line.start + local)
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
        let range = self.clamp_range(range);

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
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(range);

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
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
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
        if self.last_lines.is_empty() {
            return None;
        }
        let line_height = self.line_height;
        let range = self.range_from_utf16(&range_utf16);
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
        let top = bounds.top() + (s_line as f32 * line_height) - self.scroll_offset.y;
        Some(Bounds::from_corners(
            point(bounds.left() + x0 - self.scroll_offset.x, top),
            point(bounds.left() + x1 - self.scroll_offset.x, top + line_height),
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

        let text_color: Hsla = cx.theme().text_primary.into();
        let compact = self.compact;

        div()
            .key_context(KEY_CONTEXT)
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
    }
}

struct MentionTextElement {
    input: Entity<MentionInputState>,
}

struct PreparedLine {
    line: WrappedLine,
    origin: Point<Pixels>,
    start: usize,
}

struct PrepaintState {
    lines: Vec<PreparedLine>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
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
        let runs = if masked {
            vec![run]
        } else {
            build_text_runs(display_text.len(), &run, marked_range, &resolved_spans)
        };

        let mut scroll_offset = input.scroll_offset;
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
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

        let (caret_line, caret_local) = locate_span(&spans, display_cursor);
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
            prepared.push(PreparedLine {
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

        PrepaintState {
            lines: prepared,
            cursor,
            selection,
            line_height,
            scroll_offset,
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
        let lines = std::mem::take(&mut prepaint.lines);
        let mut stored = Vec::with_capacity(lines.len());
        for prepared in lines {
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
                tracing::warn!("mention input text paint failed: {e}");
            }
            stored.push(DocLine {
                line: prepared.line,
                start: prepared.start,
            });
        }

        if focus_handle.is_focused(window)
            && self.input.read(cx).caret_blink.visible()
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let scroll_offset = prepaint.scroll_offset;
        self.input.update(cx, |input, _cx| {
            input.last_lines = stored;
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
            input.scroll_offset = scroll_offset;
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
