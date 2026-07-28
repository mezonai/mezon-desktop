use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

#[path = "blink.rs"]
mod blink;

use blink::CaretBlink;
use gpui::{
    Anchor, App, Bounds, ClickEvent, ClipboardEntry, ClipboardItem, Context, CursorStyle,
    DispatchPhase, Div, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId, Hsla, Image, ImageFormat,
    InspectorElementId, InteractiveElement as _, IntoElement, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, RenderOnce,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle,
    Style, StyleRefinement, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine, actions, anchored, deferred, div, fill, point, prelude::*, px, size,
};
use mezon_store::{CanvasStore, PlatformStore};
use serde_json::{Value, json};
use ui::Tooltip as TooltipView;
use unicode_segmentation::UnicodeSegmentation;

use crate::image::{
    CANVAS_IMAGE_FALLBACK_HEIGHT, canvas_image_display_size, canvas_image_display_src,
    canvas_image_known_size, canvas_img, ensure_canvas_image_dimensions_loaded, is_data_image_url,
    remember_canvas_image_size,
};
use crate::view::{TipTapMark, TipTapNode, is_tiptap_content_empty, parse_tiptap_doc};
use mezon_theme::ActiveTheme;
use mezon_widgets::{
    Button, ButtonVariants, Icon, IconName, Input, InputState, Sizable, Size, h_flex, v_flex,
};

const KEY_CONTEXT: &str = "MezonCanvasEditor";
const CANVAS_IMAGE_MARGIN: Pixels = px(16.);
const CANVAS_HORIZONTAL_PADDING: Pixels = px(128.);
const CANVAS_MIN_LAYOUT_WIDTH: Pixels = px(64.);
const CANVAS_LAYOUT_WIDTH_FALLBACK: Pixels = px(480.);

pub(crate) fn italic_font_family() -> SharedString {
    #[cfg(target_os = "macos")]
    {
        SharedString::from(".SystemUIFont")
    }
    #[cfg(target_os = "windows")]
    {
        SharedString::from("Segoe UI")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        SharedString::from("Noto Sans")
    }
}

fn apply_mark_font(run: &mut TextRun, block_weight: FontWeight, bold: bool, italic: bool) {
    let mut font = run.font.clone();
    font.weight = if bold { FontWeight::BOLD } else { block_weight };
    if italic {
        font.style = FontStyle::Italic;
        font.family = italic_font_family();
    } else {
        font.style = FontStyle::Normal;
    }
    run.font = font;
}

actions!(
    canvas_editor,
    [
        Backspace,
        Delete,
        Enter,
        ShiftEnter,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectHome,
        SelectEnd,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Bold,
        Italic,
        StrikeThrough,
        CodeBlock,
        Link,
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", Enter, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-enter", ShiftEnter, Some(KEY_CONTEXT)),
        KeyBinding::new("left", Left, Some(KEY_CONTEXT)),
        KeyBinding::new("right", Right, Some(KEY_CONTEXT)),
        KeyBinding::new("up", Up, Some(KEY_CONTEXT)),
        KeyBinding::new("down", Down, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(KEY_CONTEXT)),
        KeyBinding::new("home", Home, Some(KEY_CONTEXT)),
        KeyBinding::new("end", End, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-b", Bold, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-i", Italic, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-secondary-s", StrikeThrough, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-k", Link, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-alt-c", CodeBlock, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    BulletItem,
    OrderedItem,
    TaskItem { checked: bool },
    Blockquote,
    CodeBlock,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarkKind {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorMark {
    range: Range<usize>,
    kind: MarkKind,
    href: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorLine {
    block: BlockKind,
    text: String,
    marks: Vec<EditorMark>,
    image_src: Option<String>,
}

impl Default for EditorLine {
    fn default() -> Self {
        Self {
            block: BlockKind::Paragraph,
            text: String::new(),
            marks: Vec::new(),
            image_src: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StyledMark {
    range: Range<usize>,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

struct DocLine {
    line: Option<Arc<WrappedLine>>,
    text_start: usize,
    start: usize,
    prefix_width: Pixels,
    top: Pixels,
    height: Pixels,
    row_height: Pixels,
}

#[derive(Clone)]
struct BlockPaintMeta {
    prefix: SharedString,
    indent: Pixels,
    font_size: Pixels,
    font_weight: FontWeight,
    line_height: Pixels,
    monospace: bool,
    code_background: bool,
    blockquote_bar: bool,
}

pub struct CanvasEditorState {
    focus_handle: FocusHandle,
    placeholder: SharedString,
    locale: SharedString,
    lines: Vec<EditorLine>,
    content: SharedString,
    line_starts: Vec<usize>,
    styled_marks: Vec<StyledMark>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<DocLine>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    max_scroll_y: Pixels,
    bubble_anchor: Option<Point<Pixels>>,
    overlay_images: Vec<EditorImageOverlay>,
    read_only: bool,
    page_scroll: bool,
    content_height: Pixels,
    layout_width: Pixels,
    parent_scroll: Option<ScrollHandle>,
    caret_blink: CaretBlink,
    block_menu_open: bool,
    link_menu_open: bool,
    link_input: Entity<InputState>,
    is_selecting: bool,
    layout_generation: u64,
    layout_cache: RefCell<Option<Arc<EditorShapeCache>>>,
    doc_json_cache: RefCell<Option<String>>,
    content_dirty: RefCell<bool>,
}

impl CanvasEditorState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        locale: impl Into<SharedString>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let link_input = cx.new(|cx| InputState::new(window, cx).placeholder("https://..."));
        let mut state = Self {
            focus_handle: focus_handle.clone(),
            placeholder: placeholder.into(),
            locale: locale.into(),
            lines: vec![EditorLine::default()],
            content: SharedString::default(),
            line_starts: vec![0],
            styled_marks: Vec::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            line_height: Pixels::ZERO,
            scroll_offset: Point::default(),
            max_scroll_y: Pixels::ZERO,
            bubble_anchor: None,
            overlay_images: Vec::new(),
            read_only: false,
            page_scroll: false,
            content_height: px(200.),
            layout_width: Pixels::ZERO,
            parent_scroll: None,
            caret_blink: CaretBlink::new(),
            block_menu_open: false,
            link_menu_open: false,
            link_input,
            is_selecting: false,
            layout_generation: 0,
            layout_cache: RefCell::new(None),
            doc_json_cache: RefCell::new(None),
            content_dirty: RefCell::new(false),
        };
        state.rebuild_buffer();
        *state.content_dirty.borrow_mut() = false;
        cx.on_focus(&focus_handle, window, |this, _window, cx| {
            this.caret_blink.sync_focused(cx);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus_handle, window, move |this, window, cx| {
            this.caret_blink.sync_blurred(cx);
            let link_focused = this.link_input.read(cx).focus_handle(cx).is_focused(window);
            if !link_focused {
                this.block_menu_open = false;
                this.link_menu_open = false;
            }
            cx.notify();
        })
        .detach();
        state
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only == read_only {
            return;
        }
        self.read_only = read_only;
        if read_only {
            self.selected_range = 0..0;
            self.selection_reversed = false;
            self.block_menu_open = false;
            self.link_menu_open = false;
        }
        cx.notify();
    }

    pub fn set_page_scroll(
        &mut self,
        enabled: bool,
        parent_scroll: ScrollHandle,
        cx: &mut Context<Self>,
    ) {
        self.parent_scroll = Some(parent_scroll);
        self.seed_layout_width_from_parent();
        if self.page_scroll == enabled {
            if enabled {
                self.refresh_content_height_estimate(cx);
            }
            return;
        }
        self.page_scroll = enabled;
        self.scroll_offset = Point::default();
        if enabled {
            self.refresh_content_height_estimate(cx);
        }
        cx.notify();
    }

    fn seed_layout_width_from_parent(&mut self) {
        let parent_w = self
            .parent_scroll
            .as_ref()
            .map(|handle| handle.bounds().size.width)
            .unwrap_or(Pixels::ZERO);
        if parent_w > CANVAS_MIN_LAYOUT_WIDTH {
            self.layout_width =
                (parent_w - CANVAS_HORIZONTAL_PADDING * 2.).max(CANVAS_MIN_LAYOUT_WIDTH);
        }
    }

    fn refresh_content_height_estimate(&mut self, cx: &App) {
        let width = self.layout_width.max(px(480.));
        self.content_height = self.estimate_content_height(cx, width);
    }

    fn estimate_content_height(&self, cx: &App, width: Pixels) -> Pixels {
        let width = width.max(px(1.));
        let mut content_y = Pixels::ZERO;
        for (logical_ix, editor_line) in self.lines.iter().enumerate() {
            if editor_line.block == BlockKind::Image {
                let src = editor_line.image_src.as_deref().unwrap_or("");
                let (_, image_h) = canvas_image_display_size(cx, src, width);
                content_y += image_h + CANVAS_IMAGE_MARGIN * 2.;
                continue;
            }
            let meta = block_paint_meta(editor_line, logical_ix, &self.lines, px(16.));
            let line_h = meta.line_height;
            let prefix_w = if meta.prefix.is_empty() {
                Pixels::ZERO
            } else {
                px(28.)
            };
            let wrap_width = (width - meta.indent - prefix_w).max(px(1.));
            if editor_line.text.is_empty() {
                content_y += line_h;
                continue;
            }
            let approx_chars = ((wrap_width / px(7.5)).max(1.).floor()) as usize;
            let wrapped_lines = editor_line.text.len().div_ceil(approx_chars.max(1)).max(1);
            content_y += line_h * wrapped_lines as f32;
        }
        content_y.max(px(200.))
    }

    fn layout_available_width(&self, bounds: Bounds<Pixels>) -> Pixels {
        let bounds_w = bounds.size.width;
        let cached = self.layout_width.max(
            self.last_bounds
                .map(|b| b.size.width)
                .unwrap_or(Pixels::ZERO),
        );
        if bounds_w > CANVAS_MIN_LAYOUT_WIDTH {
            bounds_w
        } else if cached > CANVAS_MIN_LAYOUT_WIDTH {
            cached
        } else {
            bounds_w.max(cached).max(CANVAS_LAYOUT_WIDTH_FALLBACK)
        }
    }

    fn content_layout_height(&self) -> Pixels {
        self.content_height.max(px(200.))
    }

    pub fn set_doc(&mut self, raw: &str, cx: &mut Context<Self>) {
        self.lines = lines_from_tiptap(raw);
        if self.lines.is_empty() {
            self.lines.push(EditorLine::default());
        }
        self.rebuild_buffer();
        *self.content_dirty.borrow_mut() = false;
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.overlay_images.clear();
        if self.page_scroll {
            self.refresh_content_height_estimate(cx);
        }
        cx.notify();
    }

    pub fn is_content_dirty(&self) -> bool {
        *self.content_dirty.borrow()
    }

    pub fn mark_content_saved(&mut self, cx: &mut Context<Self>) {
        *self.content_dirty.borrow_mut() = false;
        cx.notify();
    }

    pub fn doc_json(&self) -> String {
        if let Some(cached) = self.doc_json_cache.borrow().as_ref() {
            return cached.clone();
        }
        let json = lines_to_tiptap_json(&self.lines);
        *self.doc_json_cache.borrow_mut() = Some(json.clone());
        json
    }

    pub fn is_empty(&self) -> bool {
        is_tiptap_content_empty(&self.doc_json())
    }

    pub fn insert_image(
        &mut self,
        src: String,
        known_size: Option<(u32, u32)>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || src.is_empty() {
            return;
        }
        if let Some((width, height)) = known_size {
            remember_canvas_image_size(cx, &src, width, height);
        }
        let cursor = self.cursor_offset();
        let (line_ix, col) = self.offset_to_line_col(cursor);
        let col = snap_column(&self.lines[line_ix].text, col);
        let image_line = EditorLine {
            block: BlockKind::Image,
            text: String::new(),
            marks: Vec::new(),
            image_src: Some(src),
        };

        let image_ix = if self.lines[line_ix].block == BlockKind::Image {
            self.lines.insert(line_ix + 1, image_line);
            line_ix + 1
        } else {
            let block = self.lines[line_ix].block;
            let line_text = self.lines[line_ix].text.clone();
            if line_text.is_empty() {
                self.lines[line_ix] = image_line;
                line_ix
            } else if col == 0 {
                self.lines.insert(line_ix, image_line);
                line_ix
            } else if col >= line_text.len() {
                self.lines.insert(line_ix + 1, image_line);
                line_ix + 1
            } else {
                let tail = line_text[col..].to_string();
                self.lines[line_ix].text.truncate(col);
                self.lines[line_ix]
                    .marks
                    .retain(|mark| mark.range.end <= col);
                self.lines.insert(line_ix + 1, image_line);
                if !tail.is_empty() {
                    self.lines.insert(
                        line_ix + 2,
                        EditorLine {
                            block,
                            text: tail,
                            marks: Vec::new(),
                            image_src: None,
                        },
                    );
                }
                line_ix + 1
            }
        };

        let needs_trailing_paragraph = self
            .lines
            .get(image_ix + 1)
            .is_none_or(|line| line.block == BlockKind::Image);
        if needs_trailing_paragraph {
            self.lines.insert(image_ix + 1, EditorLine::default());
        }

        self.rebuild_buffer();
        let cursor = self.line_col_to_offset(image_ix + 1, 0);
        if self.page_scroll {
            self.refresh_content_height_estimate(cx);
        }
        self.move_to(cursor, cx);
    }

    fn rebuild_buffer(&mut self) {
        self.layout_generation = self.layout_generation.wrapping_add(1);
        *self.doc_json_cache.borrow_mut() = None;
        *self.layout_cache.borrow_mut() = None;
        *self.content_dirty.borrow_mut() = true;
        let mut content = String::new();
        let mut line_starts = Vec::with_capacity(self.lines.len());
        let mut styled_marks = Vec::new();
        for (ix, line) in self.lines.iter().enumerate() {
            if ix > 0 {
                content.push('\n');
            }
            line_starts.push(content.len());
            let base = content.len();
            content.push_str(&line.text);
            for mark in &line.marks {
                styled_marks.push(StyledMark {
                    range: base + mark.range.start..base + mark.range.end,
                    bold: mark.kind == MarkKind::Bold,
                    italic: mark.kind == MarkKind::Italic,
                    strike: mark.kind == MarkKind::Strike,
                    code: mark.kind == MarkKind::Code,
                    link: mark.kind == MarkKind::Link,
                });
            }
            if line.block == BlockKind::CodeBlock && !line.text.is_empty() {
                styled_marks.push(StyledMark {
                    range: base..base + line.text.len(),
                    bold: false,
                    italic: false,
                    strike: false,
                    code: true,
                    link: false,
                });
            }
        }
        self.content = content.into();
        self.line_starts = line_starts;
        self.styled_marks = styled_marks;
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = snap_offset(&self.content, offset.min(self.content.len()));
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = snap_offset(&self.content, offset.min(self.content.len()));
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.caret_blink.pause_blinking(cx);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Range<usize>,
        new_text: &str,
        cx: &mut Context<Self>,
    ) {
        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());
        let (start_line, start_col) = self.offset_to_line_col(start);
        let (end_line, end_col) = self.offset_to_line_col(end);
        let inserted_len = new_text.len();

        if new_text.is_empty() && start_line <= end_line {
            let mut remove: Vec<usize> = (start_line..=end_line)
                .filter(|ix| self.lines[*ix].block == BlockKind::Image)
                .collect();
            remove.sort_unstable_by(|a, b| b.cmp(a));
            if !remove.is_empty() {
                for ix in remove {
                    self.lines.remove(ix);
                }
                if self.lines.is_empty() {
                    self.lines.push(EditorLine::default());
                }
                self.rebuild_buffer();
                let cursor = self.line_starts.get(start_line).copied().unwrap_or(0);
                self.selected_range = cursor..cursor;
                self.selection_reversed = false;
                self.marked_range = None;
                self.caret_blink.pause_blinking(cx);
                cx.notify();
                return;
            }
        }

        if self
            .lines
            .get(start_line)
            .is_some_and(|l| l.block == BlockKind::Image)
        {
            return;
        }

        if start_line == end_line {
            let line = &mut self.lines[start_line];
            let start_col = snap_column(&line.text, start_col);
            let end_col = snap_column(&line.text, end_col);
            line.text.replace_range(start_col..end_col, new_text);
            shift_marks(&mut line.marks, start_col, end_col, inserted_len);
        } else {
            let start_col = snap_column(&self.lines[start_line].text, start_col);
            let end_col = snap_column(&self.lines[end_line].text, end_col);
            let mut merged = String::new();
            merged.push_str(&self.lines[start_line].text[..start_col]);
            merged.push_str(new_text);
            merged.push_str(&self.lines[end_line].text[end_col..]);
            let kept_marks = self.lines[start_line]
                .marks
                .iter()
                .filter(|m| m.range.end <= start_col)
                .cloned()
                .collect::<Vec<_>>();
            self.lines[start_line].text = merged;
            self.lines[start_line].marks = kept_marks;
            if end_line > start_line {
                self.lines.drain(start_line + 1..=end_line);
            }
        }

        self.rebuild_buffer();
        let new_cursor = start + inserted_len;
        self.selected_range = new_cursor..new_cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
    }

    fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let offset = snap_offset(&self.content, offset.min(self.content.len()));
        for (ix, start) in self.line_starts.iter().enumerate().rev() {
            if offset >= *start {
                let col = snap_column(&self.lines[ix].text, offset - start);
                return (ix, col);
            }
        }
        (
            0,
            snap_column(
                self.lines.first().map(|l| l.text.as_str()).unwrap_or(""),
                offset,
            ),
        )
    }

    fn line_col_to_offset(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_starts.len().saturating_sub(1));
        let start = self.line_starts.get(line).copied().unwrap_or(0);
        let line_len = self.lines.get(line).map(|l| l.text.len()).unwrap_or(0);
        start + col.min(line_len)
    }

    fn remove_line(&mut self, line_ix: usize, cx: &mut Context<Self>) {
        if line_ix >= self.lines.len() {
            return;
        }
        self.lines.remove(line_ix);
        if self.lines.is_empty() {
            self.lines.push(EditorLine::default());
        }
        self.rebuild_buffer();
        let offset = self
            .line_starts
            .get(line_ix)
            .copied()
            .unwrap_or(0)
            .min(self.content.len());
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
        self.caret_blink.pause_blinking(cx);
        cx.notify();
    }

    fn split_paragraph(&mut self, cx: &mut Context<Self>) {
        let cursor = snap_offset(&self.content, self.cursor_offset());
        let (line_ix, col) = self.offset_to_line_col(cursor);
        if self.lines[line_ix].block == BlockKind::Image {
            self.lines.insert(line_ix + 1, EditorLine::default());
            self.rebuild_buffer();
            self.move_to(self.line_col_to_offset(line_ix + 1, 0), cx);
            return;
        }
        let current = self.lines[line_ix].clone();
        let col = snap_column(&current.text, col);
        let tail_text = current.text[col..].to_string();
        let tail_marks = current
            .marks
            .iter()
            .filter_map(|m| {
                if m.range.start >= col {
                    Some(EditorMark {
                        range: m.range.start - col..m.range.end - col,
                        kind: m.kind,
                        href: m.href.clone(),
                    })
                } else if m.range.end > col {
                    Some(EditorMark {
                        range: 0..m.range.end - col,
                        kind: m.kind,
                        href: m.href.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();
        self.lines[line_ix].text.truncate(col);
        self.lines[line_ix].marks.retain(|m| m.range.end <= col);
        let new_block = match current.block {
            BlockKind::BulletItem => BlockKind::BulletItem,
            BlockKind::OrderedItem => BlockKind::OrderedItem,
            BlockKind::TaskItem { checked: _ } => BlockKind::TaskItem { checked: false },
            BlockKind::CodeBlock => {
                self.lines[line_ix].text.push('\n');
                self.lines[line_ix].text.push_str(&tail_text);
                self.rebuild_buffer();
                self.move_to(cursor + 1, cx);
                return;
            }
            other => other,
        };
        self.lines.insert(
            line_ix + 1,
            EditorLine {
                block: new_block,
                text: tail_text,
                marks: tail_marks,
                image_src: None,
            },
        );
        self.rebuild_buffer();
        self.move_to(self.line_col_to_offset(line_ix + 1, 0), cx);
    }

    fn insert_hard_break(&mut self, cx: &mut Context<Self>) {
        let cursor = snap_offset(&self.content, self.cursor_offset());
        let (line_ix, col) = self.offset_to_line_col(cursor);
        let col = snap_column(&self.lines[line_ix].text, col);
        self.lines[line_ix].text.insert(col, '\n');
        shift_marks(&mut self.lines[line_ix].marks, col, col, 1);
        self.rebuild_buffer();
        let new_cursor = self.line_starts[line_ix] + col + 1;
        self.move_to(new_cursor, cx);
    }

    fn selection_line_range(&self) -> (usize, usize, usize, usize) {
        let start = self.selected_range.start;
        let end = self.selected_range.end;
        let (sl, sc) = self.offset_to_line_col(start);
        let (el, ec) = self.offset_to_line_col(end);
        (sl, sc, el, ec)
    }

    fn toggle_mark(&mut self, kind: MarkKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let selection = self.selected_range.clone();
        let (sl, sc, el, ec) = self.selection_line_range();
        for line_ix in sl..=el {
            let sel_start = if line_ix == sl { sc } else { 0 };
            let sel_end = if line_ix == el {
                ec
            } else {
                self.lines[line_ix].text.len()
            };
            if sel_start >= sel_end {
                continue;
            }
            toggle_mark_on_line(&mut self.lines[line_ix], sel_start..sel_end, kind);
        }
        self.selected_range = selection;
        self.rebuild_buffer();
        self.focus(window, cx);
        cx.notify();
    }

    fn set_block_kind(&mut self, kind: BlockKind, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.selected_range.clone();
        let (sl, _, el, _) = self.selection_line_range();
        let start = sl;
        let end = if self.selected_range.is_empty() {
            sl
        } else {
            el
        };
        for line in &mut self.lines[start..=end] {
            if line.block == BlockKind::Image {
                continue;
            }
            line.block = match kind {
                BlockKind::TaskItem { checked: _ } => BlockKind::TaskItem { checked: false },
                other => other,
            };
        }
        self.selected_range = selection;
        self.block_menu_open = false;
        self.rebuild_buffer();
        self.focus(window, cx);
        cx.notify();
    }

    fn toggle_code_block(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sl, _, el, _) = self.selection_line_range();
        let start = sl;
        let end = if self.selected_range.is_empty() {
            sl
        } else {
            el
        };
        let all_code = self.lines[start..=end]
            .iter()
            .all(|l| l.block == BlockKind::CodeBlock);
        let new_kind = if all_code {
            BlockKind::Paragraph
        } else {
            BlockKind::CodeBlock
        };
        for line in &mut self.lines[start..=end] {
            if line.block == BlockKind::Image {
                continue;
            }
            line.block = new_kind;
        }
        self.rebuild_buffer();
        self.focus(window, cx);
        cx.notify();
    }

    fn active_block_kind(&self) -> BlockKind {
        let (sl, _, _, _) = self.selection_line_range();
        self.lines
            .get(sl)
            .map(|line| line.block)
            .unwrap_or(BlockKind::Paragraph)
    }

    fn link_href_at_offset(&self, offset: usize) -> Option<String> {
        let (line_ix, col) = self.offset_to_line_col(offset);
        let line = self.lines.get(line_ix)?;
        line.marks.iter().find_map(|mark| {
            (mark.kind == MarkKind::Link && mark.range.start <= col && mark.range.end > col)
                .then(|| mark.href.clone())
                .flatten()
        })
    }

    fn selection_has_mark(&self, kind: MarkKind) -> bool {
        if self.selected_range.is_empty() {
            return false;
        }
        let (sl, sc, el, ec) = self.selection_line_range();
        (sl..=el).any(|line_ix| {
            let sel_start = if line_ix == sl { sc } else { 0 };
            let sel_end = if line_ix == el {
                ec
            } else {
                self.lines[line_ix].text.len()
            };
            self.lines[line_ix].marks.iter().any(|mark| {
                mark.kind == kind && mark.range.start < sel_end && mark.range.end > sel_start
            })
        })
    }

    fn selection_has_link(&self) -> bool {
        self.selection_has_mark(MarkKind::Link)
    }

    fn link_href_for_selection(&self) -> Option<String> {
        if self.selected_range.is_empty() {
            return None;
        }
        let (sl, sc, el, ec) = self.selection_line_range();
        for line_ix in sl..=el {
            let sel_start = if line_ix == sl { sc } else { 0 };
            let sel_end = if line_ix == el {
                ec
            } else {
                self.lines[line_ix].text.len()
            };
            if let Some(href) = self.lines[line_ix].marks.iter().find_map(|mark| {
                (mark.kind == MarkKind::Link
                    && mark.range.start < sel_end
                    && mark.range.end > sel_start)
                    .then(|| mark.href.clone())
                    .flatten()
            }) {
                return Some(href);
            }
        }
        None
    }

    fn apply_link_mark(&mut self, href: &str, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let (sl, sc, el, ec) = self.selection_line_range();
        for line_ix in sl..=el {
            let sel_start = if line_ix == sl { sc } else { 0 };
            let sel_end = if line_ix == el {
                ec
            } else {
                self.lines[line_ix].text.len()
            };
            if sel_start >= sel_end {
                continue;
            }
            let line = &mut self.lines[line_ix];
            line.marks.retain(|mark| {
                !(mark.kind == MarkKind::Link
                    && mark.range.start < sel_end
                    && mark.range.end > sel_start)
            });
            line.marks.push(EditorMark {
                range: sel_start..sel_end,
                kind: MarkKind::Link,
                href: Some(href.to_string()),
            });
        }
        self.rebuild_buffer();
        cx.notify();
    }

    fn remove_link_from_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let (sl, sc, el, ec) = self.selection_line_range();
        for line_ix in sl..=el {
            let sel_start = if line_ix == sl { sc } else { 0 };
            let sel_end = if line_ix == el {
                ec
            } else {
                self.lines[line_ix].text.len()
            };
            self.lines[line_ix].marks.retain(|mark| {
                !(mark.kind == MarkKind::Link
                    && mark.range.start < sel_end
                    && mark.range.end > sel_start)
            });
        }
        self.rebuild_buffer();
        cx.notify();
    }

    fn open_link_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.block_menu_open = false;
        let href = self.link_href_for_selection().unwrap_or_default();
        self.link_menu_open = true;
        self.link_input.update(cx, |input, cx| {
            input.set_value(href, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn apply_link_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let href = self.link_input.read(cx).value().trim().to_string();
        if href.is_empty() {
            self.remove_link_from_selection(cx);
        } else {
            self.apply_link_mark(&href, cx);
        }
        self.link_menu_open = false;
        self.focus(window, cx);
        cx.notify();
    }

    fn cancel_link_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.link_menu_open = false;
        self.focus(window, cx);
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.last_lines.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        let max_offset = self.content.len();
        let rel_y = position.y - bounds.top() + self.scroll_offset.y;
        let rel_x = position.x - bounds.left() + self.scroll_offset.x;
        let block_ix = self
            .last_lines
            .iter()
            .position(|line| rel_y >= line.top && rel_y < line.top + line.height)
            .unwrap_or_else(|| {
                if rel_y < Pixels::ZERO {
                    0
                } else {
                    self.last_lines.len().saturating_sub(1)
                }
            });
        let block = &self.last_lines[block_ix];
        let Some(wrapped) = block.line.as_ref() else {
            return block.start.min(max_offset);
        };
        let local_x = (rel_x - block.prefix_width).max(Pixels::ZERO);
        let row_ranges = wrapped_line_row_ranges(wrapped);
        let row_idx = if block.row_height <= Pixels::ZERO {
            0
        } else {
            ((rel_y - block.top) / block.row_height) as usize
        }
        .min(row_ranges.len().saturating_sub(1));
        let (wr_start, wr_end) = row_ranges[row_idx];
        let row_start = block.text_start + wr_start;
        let row_end = block.text_start + wr_end;
        let offset = index_for_visual_row(wrapped, block.text_start, row_start, row_end, local_x);
        snap_offset(&self.content, offset.min(max_offset))
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
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = false;
        if self.selected_range.is_empty()
            && let Some(href) =
                self.link_href_at_offset(self.index_for_mouse_position(event.position))
        {
            open_canvas_link(&href, cx);
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_read_only_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(href) = self.link_href_at_offset(self.index_for_mouse_position(event.position))
        {
            open_canvas_link(&href, cx);
        }
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

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line, col) = self.offset_to_line_col(cursor);
        if self
            .lines
            .get(line)
            .is_some_and(|l| l.block == BlockKind::Image)
        {
            self.remove_line(line, cx);
            return;
        }
        if col == 0 && line > 0 && self.lines[line - 1].block == BlockKind::Image {
            self.remove_line(line - 1, cx);
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(self.selected_range.clone(), "", cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line, col) = self.offset_to_line_col(cursor);
        if self
            .lines
            .get(line)
            .is_some_and(|l| l.block == BlockKind::Image)
        {
            self.remove_line(line, cx);
            return;
        }
        if line + 1 < self.lines.len()
            && self.lines[line + 1].block == BlockKind::Image
            && col == self.lines[line].text.len()
        {
            self.remove_line(line + 1, cx);
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(self.selected_range.clone(), "", cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.split_paragraph(cx);
    }

    fn shift_enter(&mut self, _: &ShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_hard_break(cx);
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.previous_boundary(self.cursor_offset());
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.next_boundary(self.cursor_offset());
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        let (line, col) = self.offset_to_line_col(self.cursor_offset());
        if line == 0 {
            return;
        }
        let prev_len = self.lines[line - 1].text.len();
        let offset = self.line_col_to_offset(line - 1, col.min(prev_len));
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        let (line, col) = self.offset_to_line_col(self.cursor_offset());
        if line + 1 >= self.lines.len() {
            return;
        }
        let next_len = self.lines[line + 1].text.len();
        let offset = self.line_col_to_offset(line + 1, col.min(next_len));
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let (line, col) = self.offset_to_line_col(self.cursor_offset());
        if line == 0 {
            return;
        }
        let prev_len = self.lines[line - 1].text.len();
        self.select_to(self.line_col_to_offset(line - 1, col.min(prev_len)), cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let (line, col) = self.offset_to_line_col(self.cursor_offset());
        if line + 1 >= self.lines.len() {
            return;
        }
        let next_len = self.lines[line + 1].text.len();
        self.select_to(self.line_col_to_offset(line + 1, col.min(next_len)), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.offset_to_line_col(self.cursor_offset());
        self.select_to(self.line_col_to_offset(line, 0), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.offset_to_line_col(self.cursor_offset());
        self.select_to(
            self.line_col_to_offset(line, self.lines[line].text.len()),
            cx,
        );
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.offset_to_line_col(self.cursor_offset());
        let offset = self.line_col_to_offset(line, 0);
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.offset_to_line_col(self.cursor_offset());
        let offset = self.line_col_to_offset(line, self.lines[line].text.len());
        if window.modifiers().shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
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
            self.paste_images(images, window, cx);
            return;
        }
        if let Some(text) = item.text() {
            self.replace_text_in_range(
                self.selected_range.clone(),
                &text.replace("\r\n", "\n"),
                cx,
            );
        }
    }

    fn paste_images(&mut self, images: Vec<Image>, _window: &mut Window, cx: &mut Context<Self>) {
        let base = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        cx.spawn(async move |this, cx| {
            let payloads = cx
                .background_spawn(async move {
                    images
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, image)| {
                            clipboard_image_upload_payload(&image, base, index)
                        })
                        .collect::<Vec<(Vec<u8>, String, String)>>()
                })
                .await;
            if payloads.is_empty() {
                return;
            }
            for (data, filetype, filename) in payloads {
                let upload = this.update(cx, |_, cx| {
                    CanvasStore::global(cx).update(cx, |store, cx| {
                        store.upload_canvas_image(data, filetype, filename, cx)
                    })
                });
                let Ok(task) = upload else {
                    continue;
                };
                match task.await {
                    Ok(uploaded) => {
                        let _ = this.update(cx, |this, cx| {
                            this.insert_image(
                                uploaded.url,
                                Some((uploaded.width, uploaded.height)),
                                cx,
                            );
                        });
                    }
                    Err(error) => {
                        tracing::warn!("canvas image upload failed: {error}");
                    }
                }
            }
        })
        .detach();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(self.selected_range.clone(), "", cx);
        }
    }

    fn bold(&mut self, _: &Bold, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_mark(MarkKind::Bold, window, cx);
    }

    fn italic(&mut self, _: &Italic, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_mark(MarkKind::Italic, window, cx);
    }

    fn strike_through(&mut self, _: &StrikeThrough, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_mark(MarkKind::Strike, window, cx);
    }

    fn code_block(&mut self, _: &CodeBlock, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_code_block(window, cx);
    }

    fn link(&mut self, _: &Link, window: &mut Window, cx: &mut Context<Self>) {
        self.open_link_menu(window, cx);
    }
}

impl EntityInputHandler for CanvasEditorState {
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
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.replace_text_in_range(range, new_text, cx);
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
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let insert_start = range.start;
        self.replace_text_in_range(range, new_text, cx);
        if !new_text.is_empty() {
            self.marked_range = Some(insert_start..insert_start + new_text.len());
        } else {
            self.marked_range = None;
        }
        if let Some(sel) = new_selected_range_utf16 {
            let sel = self.range_from_utf16(&sel);
            self.selected_range = insert_start + sel.start..insert_start + sel.end;
            self.selection_reversed = false;
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

impl Focusable for CanvasEditorState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CanvasEditorState {
    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta_y = match event.delta {
            ScrollDelta::Pixels(p) => p.y,
            ScrollDelta::Lines(l) => l.y * px(20.),
        };
        if delta_y == Pixels::ZERO {
            return;
        }
        self.scroll_offset.y =
            (self.scroll_offset.y + delta_y).clamp(Pixels::ZERO, self.max_scroll_y);
        cx.notify();
    }
}

impl Render for CanvasEditorState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused {
            self.caret_blink.sync_focused(cx);
        } else {
            self.caret_blink.sync_blurred(cx);
        }
        let read_only = self.read_only;
        let locale = self.locale.clone();
        let has_selection = !read_only && !self.selected_range.is_empty();
        let bubble_anchor = self.bubble_anchor;
        let block_menu_open = self.block_menu_open;
        let link_menu_open = self.link_menu_open;
        let has_link = self.selection_has_link();
        let link_input = self.link_input.clone();
        let active_block = self.active_block_kind();
        let icon_color = cx.theme().text_primary;
        let active_bg = cx.theme().bg_tertiary;
        let bold_active = self.selection_has_mark(MarkKind::Bold);
        let italic_active = self.selection_has_mark(MarkKind::Italic);
        let strike_active = self.selection_has_mark(MarkKind::Strike);
        let code_active = matches!(
            self.lines
                .get(self.selection_line_range().0)
                .map(|l| l.block),
            Some(BlockKind::CodeBlock)
        );
        let scroll_offset = self.scroll_offset;
        let overlay_images = self.overlay_images.clone();
        let fallback_fg = cx.theme().text_muted;
        let fallback_bg = cx.theme().bg_tertiary;
        let page_scroll = self.page_scroll;
        let layout_height = self.content_layout_height();

        div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .when(page_scroll, |el| el.h(layout_height))
            .when(!page_scroll, |el| el.flex_1().min_h_0())
            .when(!read_only, |el| {
                el.key_context(KEY_CONTEXT)
                    .track_focus(&self.focus_handle)
                    .cursor(CursorStyle::IBeam)
                    .on_action(cx.listener(Self::backspace))
                    .on_action(cx.listener(Self::delete))
                    .on_action(cx.listener(Self::enter))
                    .on_action(cx.listener(Self::shift_enter))
                    .on_action(cx.listener(Self::left))
                    .on_action(cx.listener(Self::right))
                    .on_action(cx.listener(Self::up))
                    .on_action(cx.listener(Self::down))
                    .on_action(cx.listener(Self::select_left))
                    .on_action(cx.listener(Self::select_right))
                    .on_action(cx.listener(Self::select_up))
                    .on_action(cx.listener(Self::select_down))
                    .on_action(cx.listener(Self::select_home))
                    .on_action(cx.listener(Self::select_end))
                    .on_action(cx.listener(Self::select_all))
                    .on_action(cx.listener(Self::home))
                    .on_action(cx.listener(Self::end))
                    .on_action(cx.listener(Self::paste))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::bold))
                    .on_action(cx.listener(Self::italic))
                    .on_action(cx.listener(Self::strike_through))
                    .on_action(cx.listener(Self::code_block))
                    .on_action(cx.listener(Self::link))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
            })
            .when(read_only, |el| {
                el.cursor(CursorStyle::Arrow)
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_read_only_mouse_up))
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .when(page_scroll, |el| el.h(layout_height))
                    .when(!page_scroll, |el| {
                        el.flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
                    })
                    .children(overlay_images.into_iter().enumerate().map(|(ix, image)| {
                        div()
                            .absolute()
                            .left_0()
                            .top(if page_scroll {
                                image.top
                            } else {
                                image.top - scroll_offset.y
                            })
                            .w(image.width + CANVAS_IMAGE_MARGIN * 2.)
                            .h(image.height + CANVAS_IMAGE_MARGIN * 2.)
                            .px(CANVAS_IMAGE_MARGIN)
                            .child(canvas_img(
                                cx,
                                image.src.as_ref(),
                                ("canvas-edit-img", ix),
                                fallback_fg,
                                fallback_bg,
                                Some(image.width),
                                Some(image.height),
                            ))
                    }))
                    .child(CanvasEditorElement {
                        editor: cx.entity(),
                    }),
            )
            .when(has_selection, |el| {
                el.when_some(bubble_anchor, |el, anchor| {
                    el.child(deferred(
                        anchored()
                            .position(anchor)
                            .anchor(Anchor::BottomLeft)
                            .snap_to_window()
                            .child(render_bubble_menu(
                                cx.entity(),
                                locale,
                                block_menu_open,
                                link_menu_open,
                                has_link,
                                active_block,
                                icon_color,
                                active_bg,
                                bold_active,
                                italic_active,
                                strike_active,
                                code_active,
                                link_input,
                                cx,
                            )),
                    ))
                })
            })
    }
}

struct CanvasEditorElement {
    editor: Entity<CanvasEditorState>,
}

impl IntoElement for CanvasEditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[derive(Clone)]
struct PreparedLine {
    line: Option<Arc<WrappedLine>>,
    origin: Point<Pixels>,
    start: usize,
    prefix_width: Pixels,
    top: Pixels,
    height: Pixels,
    row_height: Pixels,
    logical_ix: usize,
}

struct PreparedPrefix {
    line: ShapedLine,
    origin: Point<Pixels>,
    height: Pixels,
}

#[derive(Clone)]
struct PreparedImage {
    top: Pixels,
    width: Pixels,
    height: Pixels,
    src: SharedString,
}

struct EditorShapeCache {
    generation: u64,
    width: Pixels,
    layout: Vec<PreparedLine>,
    prefix_layout: Vec<(Pixels, Pixels, ShapedLine, Pixels)>,
    images: Vec<PreparedImage>,
    total_h: Pixels,
    missing_dim_srcs: Vec<String>,
}

#[derive(Clone, Default)]
struct EditorImageOverlay {
    top: Pixels,
    width: Pixels,
    height: Pixels,
    src: SharedString,
}

struct EditorPrepaint {
    lines: Vec<PreparedLine>,
    prefixes: Vec<PreparedPrefix>,
    images: Vec<PreparedImage>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    decorations: Vec<PaintQuad>,
    line_height: Pixels,
    scroll_offset: Point<Pixels>,
    max_scroll_y: Pixels,
    content_height: Pixels,
    bubble_anchor: Option<Point<Pixels>>,
}

impl Element for CanvasEditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

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
        let editor = self.editor.read(cx);
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = if editor.page_scroll {
            editor.content_layout_height().into()
        } else {
            gpui::relative(1.).into()
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
        let text_color: Hsla = cx.theme().text_primary.into();
        let link_color: Hsla = cx.theme().tokens.button_theme_primary.into();
        let code_bg: Hsla = cx.theme().tokens.theme_input.into();
        let blockquote_color: Hsla = cx.theme().tokens.button_theme_primary.into();

        let editor = self.editor.read(cx);
        let content = editor.content.clone();
        let selected_range = editor.selected_range.clone();
        let cursor = editor.cursor_offset();
        let style = window.text_style();
        let rem_size = window.rem_size();
        let default_line_height = window.line_height();
        let mut scroll_offset = editor.scroll_offset;
        let available_w = editor.layout_available_width(bounds);

        if content.is_empty() {
            let display_text = editor.placeholder.clone();
            let display_len = display_text.len();
            let base_run = TextRun {
                len: display_len,
                font: style.font(),
                color: placeholder_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let font_size = style.font_size.to_pixels(rem_size);
            let wrap_width = available_w.max(Pixels::ZERO);
            let wrapped = window
                .text_system()
                .shape_text(display_text, font_size, &[base_run], Some(wrap_width), None)
                .unwrap_or_default();
            let spans: Vec<(usize, usize)> = {
                let mut start = 0usize;
                wrapped
                    .iter()
                    .map(|line| {
                        let span = (start, line.len());
                        start += line.len();
                        span
                    })
                    .collect()
            };
            let display_cursor = cursor.min(display_len);
            let display_selection = selected_range.clone();
            let (caret_line, caret_local) = locate_span(&spans, display_cursor);
            let mut caret_x = Pixels::ZERO;
            let mut caret_top = Pixels::ZERO;
            let mut content_top = Pixels::ZERO;
            let mut painted = Vec::with_capacity(wrapped.len());
            for (ix, line) in wrapped.into_iter().enumerate() {
                let (span_start, _) = spans[ix];
                let block_height = line.size(default_line_height).height;
                if ix == caret_line
                    && let Some(pos) = line.position_for_index(caret_local, default_line_height)
                {
                    caret_x = pos.x;
                    caret_top = content_top + pos.y;
                }
                let y = bounds.top() + content_top - scroll_offset.y;
                painted.push(PreparedLine {
                    line: Some(Arc::new(line)),
                    origin: point(bounds.left() - scroll_offset.x, y),
                    start: span_start,
                    prefix_width: Pixels::ZERO,
                    top: content_top,
                    height: block_height,
                    row_height: default_line_height,
                    logical_ix: 0,
                });
                content_top += block_height;
            }
            let total_h = content_top.max(default_line_height);
            let max_scroll_y = if editor.page_scroll {
                scroll_offset = Point::default();
                Pixels::ZERO
            } else {
                let visible_h = bounds.size.height;
                scroll_offset.y = scroll_offset
                    .y
                    .clamp(Pixels::ZERO, (total_h - visible_h).max(Pixels::ZERO));
                (total_h - visible_h).max(Pixels::ZERO)
            };
            let cursor_quad = if display_selection.is_empty() {
                Some(fill(
                    Bounds::new(
                        point(
                            bounds.left() + caret_x - scroll_offset.x,
                            bounds.top() + caret_top - scroll_offset.y,
                        ),
                        size(px(2.), default_line_height),
                    ),
                    cursor_color,
                ))
            } else {
                None
            };
            return EditorPrepaint {
                lines: painted,
                prefixes: Vec::new(),
                images: Vec::new(),
                cursor: cursor_quad,
                selection: Vec::new(),
                decorations: Vec::new(),
                line_height: default_line_height,
                scroll_offset,
                max_scroll_y,
                content_height: total_h,
                bubble_anchor: None,
            };
        }

        let page_scroll = editor.page_scroll;
        let base_font = style.font();
        let base_run = TextRun {
            len: 0,
            font: base_font.clone(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let display_selection = selected_range.clone();
        let layout_generation = editor.layout_generation;
        let cached_layout = editor
            .layout_cache
            .borrow()
            .as_ref()
            .filter(|cache| cache.generation == layout_generation && cache.width == available_w)
            .cloned();

        let (layout, prefix_layout, images, total_h, missing_dim_srcs) =
            if let Some(cache) = cached_layout {
                (
                    cache.layout.clone(),
                    cache.prefix_layout.clone(),
                    cache.images.clone(),
                    cache.total_h,
                    cache.missing_dim_srcs.clone(),
                )
            } else {
                let mut images = Vec::new();
                let mut layout: Vec<PreparedLine> = Vec::new();
                let mut prefix_layout: Vec<(Pixels, Pixels, ShapedLine, Pixels)> = Vec::new();
                let mut content_y = Pixels::ZERO;
                let mut missing_dim_srcs: Vec<String> = Vec::new();

                for (logical_ix, editor_line) in editor.lines.iter().enumerate() {
                    let line_start = editor.line_starts.get(logical_ix).copied().unwrap_or(0);
                    if editor_line.block == BlockKind::Image {
                        let src = editor_line.image_src.as_deref().unwrap_or("");
                        let (image_w, image_h) = canvas_image_display_size(cx, src, available_w);
                        if !src.is_empty()
                            && !is_data_image_url(src)
                            && canvas_image_known_size(cx, src).is_none()
                        {
                            missing_dim_srcs.push(src.to_string());
                        }
                        let block_h = image_h + CANVAS_IMAGE_MARGIN * 2.;
                        layout.push(PreparedLine {
                            line: None,
                            origin: point(Pixels::ZERO, content_y),
                            start: line_start,
                            prefix_width: Pixels::ZERO,
                            top: content_y,
                            height: block_h,
                            row_height: default_line_height,
                            logical_ix,
                        });
                        if !src.is_empty() {
                            images.push(PreparedImage {
                                top: content_y + CANVAS_IMAGE_MARGIN,
                                width: image_w,
                                height: image_h,
                                src: canvas_image_display_src(src).into(),
                            });
                        }
                        content_y += block_h;
                        continue;
                    }
                    let meta = block_paint_meta(editor_line, logical_ix, &editor.lines, rem_size);
                    let line_h = meta.line_height;
                    let font_size = meta.font_size;

                    let mut prefix_width = Pixels::ZERO;
                    if !meta.prefix.is_empty() {
                        let prefix_run = TextRun {
                            len: meta.prefix.len(),
                            font: {
                                let mut font = base_font.clone();
                                font.weight = meta.font_weight;
                                font
                            },
                            color: text_color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            meta.prefix.clone(),
                            font_size,
                            &[prefix_run],
                            None,
                        );
                        prefix_width = shaped.width();
                        prefix_layout.push((content_y, meta.indent, shaped, line_h));
                    }
                    let text_x = meta.indent + prefix_width;
                    let wrap_width = (available_w - text_x).max(Pixels::ZERO);

                    let runs = build_line_runs(editor_line, &base_run, &meta, link_color, code_bg);
                    let wrapped = if editor_line.text.is_empty() {
                        Vec::new()
                    } else {
                        window
                            .text_system()
                            .shape_text(
                                SharedString::from(editor_line.text.as_str()),
                                font_size,
                                &runs,
                                Some(wrap_width),
                                None,
                            )
                            .unwrap_or_default()
                            .into_iter()
                            .collect()
                    };

                    if wrapped.is_empty() {
                        layout.push(PreparedLine {
                            line: None,
                            origin: point(Pixels::ZERO, content_y),
                            start: line_start,
                            prefix_width: text_x,
                            top: content_y,
                            height: line_h,
                            row_height: line_h,
                            logical_ix,
                        });
                        content_y += line_h;
                        continue;
                    }

                    let wrapped_count = wrapped.len();
                    let mut byte_off = 0usize;
                    for (wrap_ix, line) in wrapped.into_iter().enumerate() {
                        let seg_start = line_start + byte_off;
                        let seg_len = line.len();
                        let block_height = line.size(line_h).height;
                        layout.push(PreparedLine {
                            line: Some(Arc::new(line)),
                            origin: point(text_x, content_y),
                            start: seg_start,
                            prefix_width: text_x,
                            top: content_y,
                            height: block_height,
                            row_height: line_h,
                            logical_ix,
                        });
                        byte_off += seg_len;
                        if wrap_ix + 1 < wrapped_count {
                            byte_off += 1;
                        }
                        content_y += block_height;
                    }
                }

                let total_h = content_y.max(default_line_height);
                *editor.layout_cache.borrow_mut() = Some(Arc::new(EditorShapeCache {
                    generation: layout_generation,
                    width: available_w,
                    layout: layout.clone(),
                    prefix_layout: prefix_layout.clone(),
                    images: images.clone(),
                    total_h,
                    missing_dim_srcs: missing_dim_srcs.clone(),
                }));
                (layout, prefix_layout, images, total_h, missing_dim_srcs)
            };

        let mut caret_x = Pixels::ZERO;
        let mut caret_top = Pixels::ZERO;
        let mut caret_h = default_line_height;
        let mut caret_prefix = Pixels::ZERO;
        for prepared in &layout {
            let line_end = prepared.start + prepared.line.as_ref().map(|l| l.len()).unwrap_or(0);
            if cursor >= prepared.start && cursor <= line_end {
                let local = cursor - prepared.start;
                if let Some(pos) = prepared
                    .line
                    .as_ref()
                    .and_then(|l| l.position_for_index(local, prepared.row_height))
                {
                    caret_x = pos.x;
                    caret_top = prepared.top + pos.y;
                } else {
                    caret_x = Pixels::ZERO;
                    caret_top = prepared.top;
                }
                caret_h = prepared.row_height;
                caret_prefix = prepared.prefix_width;
                break;
            }
            if cursor > line_end {
                caret_top = prepared.top;
                caret_h = prepared.row_height;
                caret_prefix = prepared.prefix_width;
                if let Some(pos) = prepared.line.as_ref().and_then(|l| {
                    l.position_for_index(
                        prepared.line.as_ref().map(|l| l.len()).unwrap_or(0),
                        prepared.row_height,
                    )
                }) {
                    caret_x = pos.x;
                    caret_top = prepared.top + pos.y;
                }
            }
        }

        let visible_h = bounds.size.height;
        let visible_w = bounds.size.width;
        let max_scroll_y = if page_scroll {
            scroll_offset = Point::default();
            Pixels::ZERO
        } else {
            let caret_screen_x = caret_prefix + caret_x;
            if caret_top < scroll_offset.y {
                scroll_offset.y = caret_top;
            }
            if caret_top + caret_h > scroll_offset.y + visible_h {
                scroll_offset.y = caret_top + caret_h - visible_h;
            }
            scroll_offset.y = scroll_offset
                .y
                .clamp(Pixels::ZERO, (total_h - visible_h).max(Pixels::ZERO));
            if caret_screen_x < scroll_offset.x {
                scroll_offset.x = caret_screen_x;
            }
            if caret_screen_x > scroll_offset.x + visible_w - px(6.) {
                scroll_offset.x = caret_screen_x - visible_w + px(6.);
            }
            scroll_offset.x = scroll_offset.x.max(Pixels::ZERO);
            (total_h - visible_h).max(Pixels::ZERO)
        };

        let mut painted = Vec::with_capacity(layout.len());
        let mut prefixes = Vec::new();
        let mut selection = Vec::new();
        let mut decorations = Vec::new();
        let mut bubble_anchor = None;

        for (top, indent, shaped_prefix, prefix_h) in prefix_layout {
            prefixes.push(PreparedPrefix {
                line: shaped_prefix,
                origin: point(
                    bounds.left() + indent - scroll_offset.x,
                    bounds.top() + top - scroll_offset.y,
                ),
                height: prefix_h,
            });
        }

        for prepared in layout {
            let y = bounds.top() + prepared.top - scroll_offset.y;
            let origin = point(bounds.left() + prepared.prefix_width - scroll_offset.x, y);
            let line_start = prepared.start;
            let line_len = prepared.line.as_ref().map(|l| l.len()).unwrap_or(0);
            let line_end = line_start + line_len;
            let logical_ix = prepared.logical_ix;
            let meta = block_paint_meta(
                editor
                    .lines
                    .get(logical_ix)
                    .unwrap_or(&EditorLine::default()),
                logical_ix,
                &editor.lines,
                rem_size,
            );
            if meta.code_background {
                decorations.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() - scroll_offset.x, y),
                        point(bounds.right() - scroll_offset.x, y + prepared.height),
                    ),
                    code_bg,
                ));
            }
            if meta.blockquote_bar {
                decorations.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() - scroll_offset.x, y),
                        point(
                            bounds.left() - scroll_offset.x + px(3.),
                            y + prepared.height,
                        ),
                    ),
                    blockquote_color,
                ));
            }
            if !display_selection.is_empty()
                && display_selection.start <= line_end
                && display_selection.end >= line_start
                && let Some(line) = prepared.line.as_ref()
            {
                for (row_idx, (wr_start, wr_end)) in
                    wrapped_line_row_ranges(line).into_iter().enumerate()
                {
                    let abs_row_start = line_start + wr_start;
                    let abs_row_end = line_start + wr_end;
                    if display_selection.start >= abs_row_end
                        || display_selection.end <= abs_row_start
                    {
                        continue;
                    }
                    let local_sel_start = display_selection.start.max(abs_row_start) - line_start;
                    let local_sel_end = display_selection.end.min(abs_row_end) - line_start;
                    if local_sel_start >= local_sel_end {
                        continue;
                    }
                    let row_y = bounds.top() + prepared.top + prepared.row_height * row_idx as f32
                        - scroll_offset.y;
                    let origin_x = bounds.left() + prepared.prefix_width - scroll_offset.x;
                    let (x0, x1) = selection_x_range_in_row(
                        line,
                        wr_start,
                        wr_end,
                        local_sel_start,
                        local_sel_end,
                    );
                    if x1 <= x0 {
                        continue;
                    }
                    selection.push(fill(
                        Bounds::from_corners(
                            point(origin_x + x0, row_y),
                            point(origin_x + x1, row_y + prepared.row_height),
                        ),
                        selection_color.opacity(0.3),
                    ));
                    if bubble_anchor.is_none() {
                        bubble_anchor = Some(point(origin_x + x0, row_y));
                    }
                }
            }
            painted.push(PreparedLine {
                line: prepared.line.clone(),
                origin,
                start: prepared.start,
                prefix_width: prepared.prefix_width,
                top: prepared.top,
                height: prepared.height,
                row_height: prepared.row_height,
                logical_ix: prepared.logical_ix,
            });
        }

        let cursor_quad = if display_selection.is_empty() {
            Some(fill(
                Bounds::new(
                    point(
                        bounds.left() + caret_prefix + caret_x - scroll_offset.x,
                        bounds.top() + caret_top - scroll_offset.y,
                    ),
                    size(px(2.), caret_h),
                ),
                cursor_color,
            ))
        } else {
            None
        };

        if !missing_dim_srcs.is_empty() {
            let entity_id = self.editor.entity_id();
            for src in missing_dim_srcs {
                ensure_canvas_image_dimensions_loaded(cx, &src, entity_id);
            }
        }

        EditorPrepaint {
            lines: painted,
            prefixes,
            images,
            cursor: cursor_quad,
            selection,
            decorations,
            line_height: default_line_height,
            scroll_offset,
            max_scroll_y,
            content_height: total_h,
            bubble_anchor,
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
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        self.editor.update(cx, |editor, _| {
            editor.last_bounds = Some(bounds);
            editor.scroll_offset = prepaint.scroll_offset;
        });
        let read_only = self.editor.read(cx).read_only;
        let is_selecting = self.editor.read(cx).is_selecting;
        if !read_only {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.editor.clone()),
                cx,
            );
        }
        if !read_only && is_selecting {
            let editor = self.editor.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase == DispatchPhase::Bubble {
                    editor.update(cx, |state, cx| {
                        if state.is_selecting {
                            state.select_to(state.index_for_mouse_position(event.position), cx);
                        }
                    });
                }
            });
        }
        for quad in prepaint.decorations.drain(..) {
            window.paint_quad(quad);
        }
        let overlay_images: Vec<EditorImageOverlay> = prepaint
            .images
            .iter()
            .map(|image| EditorImageOverlay {
                top: image.top,
                width: image.width,
                height: image.height,
                src: image.src.clone(),
            })
            .collect();
        let line_height = prepaint.line_height;
        let page_scroll = self.editor.read(cx).page_scroll;
        for prefix in prepaint.prefixes.drain(..) {
            let visible = page_scroll
                || (prefix.origin.y + prefix.height >= bounds.top()
                    && prefix.origin.y <= bounds.bottom());
            if visible
                && let Err(error) = prefix.line.paint(
                    prefix.origin,
                    prefix.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
            {
                tracing::warn!("canvas editor prefix paint failed: {error}");
            }
        }
        let lines = std::mem::take(&mut prepaint.lines);
        let mut stored = Vec::with_capacity(lines.len());
        for prepared in lines {
            let visible = page_scroll
                || (prepared.origin.y + prepared.height >= bounds.top()
                    && prepared.origin.y <= bounds.bottom());
            if let Some(line) = prepared.line.as_ref()
                && visible
                && let Err(error) = line.paint(
                    prepared.origin,
                    prepared.row_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
            {
                tracing::warn!("canvas editor text paint failed: {error}");
            }
            stored.push(doc_line_from_prepared(prepared));
        }
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        if focus_handle.is_focused(window)
            && !self.editor.read(cx).read_only
            && self.editor.read(cx).caret_blink.visible()
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let scroll_offset = prepaint.scroll_offset;
        let max_scroll_y = prepaint.max_scroll_y;
        let content_height = prepaint.content_height.max(px(200.));
        let bubble_anchor = prepaint.bubble_anchor;
        self.editor.update(cx, |editor, cx| {
            editor.last_lines = stored;
            editor.last_bounds = Some(bounds);
            editor.line_height = line_height;
            editor.scroll_offset = scroll_offset;
            editor.max_scroll_y = max_scroll_y;
            editor.bubble_anchor = bubble_anchor;
            editor.overlay_images = overlay_images;
            if bounds.size.width > CANVAS_MIN_LAYOUT_WIDTH {
                editor.layout_width = bounds.size.width;
            }
            if editor.page_scroll && editor.content_height != content_height {
                editor.content_height = content_height;
                cx.notify();
            }
        });
    }
}

fn clipboard_image_needs_png_transcode(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Bmp | ImageFormat::Tiff | ImageFormat::Ico | ImageFormat::Pnm
    )
}

fn clipboard_image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

fn clipboard_image_mime(ext: &str) -> &'static str {
    match ext {
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "pnm" => "image/x-portable-anymap",
        _ => "image/png",
    }
}

fn clipboard_image_upload_payload(
    image: &Image,
    base: u128,
    index: usize,
) -> Option<(Vec<u8>, String, String)> {
    if clipboard_image_needs_png_transcode(image.format()) {
        let decoded = image::load_from_memory(image.bytes()).ok()?;
        let mut buffer = std::io::Cursor::new(Vec::new());
        decoded
            .write_to(&mut buffer, image::ImageFormat::Png)
            .ok()?;
        let filename = format!("canvas-image-{base}-{index}.png");
        return Some((buffer.into_inner(), "image/png".into(), filename));
    }
    let ext = clipboard_image_extension(image.format());
    let filename = format!("canvas-image-{base}-{index}.{ext}");
    Some((
        image.bytes().to_vec(),
        clipboard_image_mime(ext).into(),
        filename,
    ))
}

fn snap_offset(text: &str, offset: usize) -> usize {
    let mut i = offset.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn snap_column(text: &str, col: usize) -> usize {
    snap_offset(text, col)
}

fn doc_line_from_prepared(prepared: PreparedLine) -> DocLine {
    DocLine {
        line: prepared.line,
        text_start: prepared.start,
        start: prepared.start,
        prefix_width: prepared.prefix_width,
        top: prepared.top,
        height: prepared.height,
        row_height: prepared.row_height,
    }
}

fn index_for_visual_row(
    wrapped: &WrappedLine,
    text_start: usize,
    row_start: usize,
    row_end: usize,
    local_x: Pixels,
) -> usize {
    let layout = &wrapped.unwrapped_layout;
    let rs = row_start.saturating_sub(text_start).min(wrapped.len());
    let re = row_end.saturating_sub(text_start).min(wrapped.len());
    if re <= rs {
        return row_start;
    }
    let row_origin_x = layout.x_for_index(rs);
    let row_end_x = row_trailing_x(wrapped, rs, re);
    let x = row_origin_x + local_x;
    if x <= row_origin_x {
        return row_start;
    }
    if x >= row_end_x {
        return row_end;
    }
    let local = layout.closest_index_for_x(x);
    text_start + local.clamp(rs, re.saturating_sub(1).max(rs))
}

fn row_trailing_x(wrapped: &WrappedLine, row_start: usize, row_end: usize) -> Pixels {
    let layout = &wrapped.unwrapped_layout;
    let len = wrapped.len();
    if row_end <= row_start {
        return layout.x_for_index(row_start.min(len));
    }
    layout.x_for_index(row_end.saturating_sub(1).max(row_start).min(len)) + px(2.)
}

fn selection_x_range_in_row(
    line: &WrappedLine,
    row_start: usize,
    row_end: usize,
    sel_start: usize,
    sel_end: usize,
) -> (Pixels, Pixels) {
    let layout = &line.unwrapped_layout;
    let rs = sel_start.max(row_start).min(line.len());
    let re = sel_end.min(row_end).min(line.len());
    if rs >= re {
        return (Pixels::ZERO, Pixels::ZERO);
    }
    let row_origin_x = layout.x_for_index(row_start.min(line.len()));
    let x0 = layout.x_for_index(rs) - row_origin_x;
    let x1 = if re < row_end {
        layout.x_for_index(re) - row_origin_x
    } else if row_end >= line.len() {
        layout.x_for_index(row_end) - row_origin_x
    } else {
        row_trailing_x(line, row_start, row_end) - row_origin_x
    };
    (x0.max(Pixels::ZERO), x1.max(x0 + px(1.)))
}

fn wrapped_line_row_ranges(line: &WrappedLine) -> Vec<(usize, usize)> {
    let len = line.len();
    let mut ends: Vec<usize> = line
        .wrap_boundaries()
        .iter()
        .map(|boundary| line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index)
        .collect();
    ends.push(len);
    let mut start = 0usize;
    ends.into_iter()
        .map(|end| {
            let range = (start, end);
            start = end;
            range
        })
        .collect()
}

fn locate_span(spans: &[(usize, usize)], off: usize) -> (usize, usize) {
    for (ix, &(start, len)) in spans.iter().enumerate() {
        if off <= start + len {
            return (ix, off.saturating_sub(start));
        }
    }
    match spans.last() {
        Some(&(start, _)) => (spans.len().saturating_sub(1), off.saturating_sub(start)),
        None => (0, 0),
    }
}

fn build_line_runs(
    line: &EditorLine,
    base: &TextRun,
    meta: &BlockPaintMeta,
    link_color: Hsla,
    code_bg: Hsla,
) -> Vec<TextRun> {
    let text_len = line.text.len();
    if text_len == 0 {
        return Vec::new();
    }
    let mut bounds = vec![0usize, text_len];
    for mark in &line.marks {
        bounds.push(mark.range.start.min(text_len));
        bounds.push(mark.range.end.min(text_len));
    }
    bounds.sort_unstable();
    bounds.dedup();
    bounds
        .windows(2)
        .filter_map(|w| {
            let (start, end) = (w[0], w[1]);
            if end <= start {
                return None;
            }
            let mut bold = false;
            let mut italic = false;
            let mut strike = false;
            let mut code = false;
            let mut link = false;
            for mark in &line.marks {
                if mark.range.start >= end || mark.range.end <= start {
                    continue;
                }
                match mark.kind {
                    MarkKind::Bold => bold = true,
                    MarkKind::Italic => italic = true,
                    MarkKind::Strike => strike = true,
                    MarkKind::Code => code = true,
                    MarkKind::Link => link = true,
                }
            }
            let mut run = base.clone();
            run.len = end - start;
            if meta.monospace || code {
                run.font.family = "monospace".into();
            }
            apply_mark_font(&mut run, meta.font_weight, bold, italic);
            if strike {
                run.strikethrough = Some(StrikethroughStyle {
                    color: Some(run.color),
                    thickness: px(1.),
                });
            }
            if meta.code_background || code {
                run.background_color = Some(code_bg);
            }
            if link {
                run.color = link_color;
                run.underline = Some(UnderlineStyle {
                    color: Some(link_color),
                    thickness: px(1.),
                    wavy: false,
                });
            }
            Some(run)
        })
        .collect()
}

fn ordered_line_number(lines: &[EditorLine], line_ix: usize) -> usize {
    let mut n = 1usize;
    for i in (0..line_ix).rev() {
        if lines[i].block == BlockKind::OrderedItem {
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn block_paint_meta(
    line: &EditorLine,
    line_ix: usize,
    lines: &[EditorLine],
    _rem_size: Pixels,
) -> BlockPaintMeta {
    let default_size = px(15.);
    let default_lh = px(22.5);
    let base = BlockPaintMeta {
        prefix: SharedString::default(),
        indent: Pixels::ZERO,
        font_size: default_size,
        font_weight: FontWeight::NORMAL,
        line_height: default_lh,
        monospace: false,
        code_background: false,
        blockquote_bar: false,
    };
    match line.block {
        BlockKind::Heading(1) => BlockPaintMeta {
            font_size: px(24.),
            font_weight: FontWeight::BOLD,
            line_height: px(36.),
            ..base
        },
        BlockKind::Heading(2) => BlockPaintMeta {
            font_size: px(20.),
            font_weight: FontWeight::SEMIBOLD,
            line_height: px(30.),
            ..base
        },
        BlockKind::Heading(3) | BlockKind::Heading(_) => BlockPaintMeta {
            font_size: px(16.),
            font_weight: FontWeight::SEMIBOLD,
            line_height: px(24.),
            ..base
        },
        BlockKind::BulletItem => BlockPaintMeta {
            prefix: "• ".into(),
            indent: px(8.),
            ..base
        },
        BlockKind::OrderedItem => BlockPaintMeta {
            prefix: format!("{}. ", ordered_line_number(lines, line_ix)).into(),
            indent: px(8.),
            ..base
        },
        BlockKind::TaskItem { checked } => BlockPaintMeta {
            prefix: (if checked { "☑ " } else { "☐ " }).into(),
            indent: px(4.),
            ..base
        },
        BlockKind::Blockquote => BlockPaintMeta {
            indent: px(12.),
            blockquote_bar: true,
            ..base
        },
        BlockKind::CodeBlock => BlockPaintMeta {
            indent: px(8.),
            monospace: true,
            code_background: true,
            line_height: px(24.),
            ..base
        },
        BlockKind::Paragraph => base,
        BlockKind::Image => BlockPaintMeta {
            line_height: CANVAS_IMAGE_FALLBACK_HEIGHT + CANVAS_IMAGE_MARGIN * 2.,
            ..base
        },
    }
}

fn open_canvas_link(url: &str, cx: &mut App) {
    if let Some(store) = PlatformStore::try_global(cx) {
        let _ = store.read(cx).open_url_external(url);
    }
}

fn render_bubble_menu(
    editor: Entity<CanvasEditorState>,
    locale: SharedString,
    block_menu_open: bool,
    link_menu_open: bool,
    has_link: bool,
    active_block: BlockKind,
    icon_color: gpui::Rgba,
    active_bg: gpui::Rgba,
    bold_active: bool,
    italic_active: bool,
    strike_active: bool,
    code_active: bool,
    link_input: Entity<InputState>,
    cx: &App,
) -> impl IntoElement {
    let menu_bg = cx.theme().bg_secondary;
    let border = cx.theme().border;
    let brand = cx.theme().tokens.button_theme_primary;
    let active_color = brand;
    let locale = locale.to_string();

    let block_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.blockFormat");
    let bold_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.bold");
    let italic_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.italic");
    let strike_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.strike");
    let code_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.codeBlock");
    let add_link_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.addLink");
    let remove_link_tooltip = mezon_i18n::t(&locale, "canvas.toolbar.removeLink");
    let cancel_label = mezon_i18n::t(&locale, "canvas.toolbar.cancel");
    let apply_label = mezon_i18n::t(&locale, "canvas.toolbar.apply");

    let link_popover = v_flex()
        .w(px(280.))
        .p(px(12.))
        .rounded(px(10.))
        .bg(menu_bg)
        .border_1()
        .border_color(brand)
        .shadow_lg()
        .gap_2()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Input::new(&link_input).w_full().min_w(px(0.)).text_sm())
        .child(
            h_flex()
                .w_full()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("canvas-link-cancel")
                        .label(cancel_label)
                        .ghost()
                        .with_size(Size::Small)
                        .on_click({
                            let editor = editor.clone();
                            move |_: &ClickEvent, window, cx| {
                                editor.update(cx, |this, cx| {
                                    this.cancel_link_menu(window, cx);
                                });
                            }
                        }),
                )
                .child(
                    Button::new("canvas-link-apply")
                        .label(apply_label)
                        .primary()
                        .with_size(Size::Small)
                        .on_click({
                            let editor = editor.clone();
                            move |_: &ClickEvent, window, cx| {
                                editor.update(cx, |this, cx| {
                                    this.apply_link_menu(window, cx);
                                });
                            }
                        }),
                ),
        );

    h_flex()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .gap(px(2.))
        .p(px(4.))
        .rounded(px(8.))
        .bg(menu_bg)
        .border_1()
        .border_color(border)
        .shadow_md()
        .child(
            div()
                .relative()
                .child(
                    div()
                        .id("canvas-bubble-block")
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .tooltip(TooltipView::text(block_tooltip))
                        .when(block_menu_open, |el| el.bg(cx.theme().bg_tertiary))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Icon::new(IconName::ParagraphIcon)
                                        .size(px(14.))
                                        .text_color(icon_color),
                                )
                                .child(
                                    Icon::new(IconName::ArrowDown)
                                        .size(px(12.))
                                        .text_color(icon_color),
                                ),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let editor = editor.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                editor.update(cx, |this, cx| {
                                    this.block_menu_open = !this.block_menu_open;
                                    this.link_menu_open = false;
                                    cx.notify();
                                });
                            }
                        }),
                )
                .when(block_menu_open, |el| {
                    el.child(
                        div().absolute().top(px(38.)).left_0().child(
                            v_flex()
                                .min_w(px(220.))
                                .py(px(6.))
                                .rounded(px(10.))
                                .bg(menu_bg)
                                .border_1()
                                .border_color(border)
                                .shadow_lg()
                                .occlude()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .children(block_menu_items(
                                    editor.clone(),
                                    active_block,
                                    &locale,
                                    icon_color,
                                    active_color,
                                    border,
                                )),
                        ),
                    )
                }),
        )
        .child(div().w(px(1.)).h(px(24.)).mx(px(4.)).bg(border))
        .child(bubble_action(
            editor.clone(),
            "canvas-bubble-bold",
            "B",
            BubbleLabelStyle::Plain,
            bold_active,
            active_bg,
            bold_tooltip,
            |this, window, cx| this.toggle_mark(MarkKind::Bold, window, cx),
        ))
        .child(bubble_action(
            editor.clone(),
            "canvas-bubble-italic",
            "𝐼",
            BubbleLabelStyle::Plain,
            italic_active,
            active_bg,
            italic_tooltip,
            |this, window, cx| this.toggle_mark(MarkKind::Italic, window, cx),
        ))
        .child(bubble_action(
            editor.clone(),
            "canvas-bubble-strike",
            "S",
            BubbleLabelStyle::Strike,
            strike_active,
            active_bg,
            strike_tooltip,
            |this, window, cx| this.toggle_mark(MarkKind::Strike, window, cx),
        ))
        .child(div().w(px(1.)).h(px(24.)).mx(px(4.)).bg(border))
        .child(bubble_action(
            editor.clone(),
            "canvas-bubble-code",
            "</>",
            BubbleLabelStyle::Plain,
            code_active,
            active_bg,
            code_tooltip,
            |this, window, cx| this.toggle_code_block(window, cx),
        ))
        .child(div().w(px(1.)).h(px(24.)).mx(px(4.)).bg(border))
        .child(
            div()
                .relative()
                .child(
                    div()
                        .id("canvas-bubble-link")
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .tooltip(TooltipView::text(add_link_tooltip))
                        .when(link_menu_open, |el| el.bg(cx.theme().bg_tertiary))
                        .text_sm()
                        .text_color(icon_color)
                        .child("Link")
                        .on_mouse_down(MouseButton::Left, {
                            let editor = editor.clone();
                            move |_, window, cx| {
                                cx.stop_propagation();
                                editor.update(cx, |this, cx| {
                                    this.open_link_menu(window, cx);
                                });
                            }
                        }),
                )
                .when(link_menu_open, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(40.))
                            .left(px(-115.))
                            .child(link_popover),
                    )
                }),
        )
        .when(has_link, |el| {
            el.child(
                div()
                    .id("canvas-bubble-remove-link")
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .tooltip(TooltipView::text(remove_link_tooltip))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(12.))
                            .text_color(icon_color),
                    )
                    .on_mouse_down(MouseButton::Left, {
                        let editor = editor.clone();
                        move |_, _, cx| {
                            cx.stop_propagation();
                            editor.update(cx, |this, cx| {
                                this.remove_link_from_selection(cx);
                                this.link_menu_open = false;
                            });
                        }
                    }),
            )
        })
}

#[derive(Clone, Copy)]
enum BubbleLabelStyle {
    Plain,
    Strike,
}

fn bubble_action(
    editor: Entity<CanvasEditorState>,
    id: impl Into<ElementId>,
    label: &'static str,
    label_style: BubbleLabelStyle,
    active: bool,
    active_bg: gpui::Rgba,
    hint: impl Into<SharedString>,
    action: fn(&mut CanvasEditorState, &mut Window, &mut Context<CanvasEditorState>),
) -> impl IntoElement {
    let hint = hint.into();
    let mut button = div()
        .id(id)
        .px(px(10.))
        .py(px(6.))
        .rounded(px(4.))
        .cursor_pointer()
        .text_sm()
        .when(active, |el| el.bg(active_bg))
        .tooltip(TooltipView::text(hint));
    button = match label_style {
        BubbleLabelStyle::Plain => button,
        BubbleLabelStyle::Strike => button.line_through(),
    };
    button
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            editor.update(cx, |this, cx| action(this, window, cx));
        })
}

fn block_kinds_equal(a: BlockKind, b: BlockKind) -> bool {
    match (a, b) {
        (BlockKind::Paragraph, BlockKind::Paragraph) => true,
        (BlockKind::Heading(x), BlockKind::Heading(y)) => x == y,
        (BlockKind::BulletItem, BlockKind::BulletItem) => true,
        (BlockKind::OrderedItem, BlockKind::OrderedItem) => true,
        (BlockKind::TaskItem { .. }, BlockKind::TaskItem { .. }) => true,
        (BlockKind::Blockquote, BlockKind::Blockquote) => true,
        (BlockKind::CodeBlock, BlockKind::CodeBlock) => true,
        (BlockKind::Image, BlockKind::Image) => true,
        _ => false,
    }
}

fn block_menu_items(
    editor: Entity<CanvasEditorState>,
    active_block: BlockKind,
    locale: &str,
    icon_color: gpui::Rgba,
    active_color: gpui::Rgba,
    border: gpui::Rgba,
) -> Vec<gpui::AnyElement> {
    let items: [(BlockKind, &str, IconName); 8] = [
        (
            BlockKind::Paragraph,
            "canvas.blocks.paragraph",
            IconName::ParagraphIcon,
        ),
        (BlockKind::Heading(1), "canvas.blocks.h1", IconName::H1Icon),
        (BlockKind::Heading(2), "canvas.blocks.h2", IconName::H2Icon),
        (BlockKind::Heading(3), "canvas.blocks.h3", IconName::H3Icon),
        (
            BlockKind::TaskItem { checked: false },
            "canvas.blocks.checkedList",
            IconName::CheckListIcon,
        ),
        (
            BlockKind::OrderedItem,
            "canvas.blocks.orderedList",
            IconName::OrderedListIcon,
        ),
        (
            BlockKind::BulletItem,
            "canvas.blocks.bulletedList",
            IconName::BulletListIcon,
        ),
        (
            BlockKind::Blockquote,
            "canvas.blocks.blockquote",
            IconName::BlockquoteIcon,
        ),
    ];
    let divider_ix = 4;
    let mut out = Vec::new();
    for (ix, (kind, key, icon)) in items.into_iter().enumerate() {
        let editor = editor.clone();
        let label = mezon_i18n::t(locale, key);
        let active = block_kinds_equal(active_block, kind);
        out.push(
            div()
                .id(("canvas-block-item", ix as u32))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .cursor_pointer()
                .when(active, |el| el.text_color(active_color))
                .child(
                    h_flex()
                        .w(px(20.))
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .when(active, |el| {
                            el.child(
                                Icon::new(IconName::Check)
                                    .size(px(16.))
                                    .text_color(active_color),
                            )
                        }),
                )
                .child(Icon::new(icon).size(px(16.)).text_color(if active {
                    active_color
                } else {
                    icon_color
                }))
                .child(label)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    editor.update(cx, |this, cx| this.set_block_kind(kind, window, cx));
                })
                .into_any_element(),
        );
        if ix + 1 == divider_ix {
            out.push(
                div()
                    .w_full()
                    .h(px(1.))
                    .my(px(6.))
                    .bg(border)
                    .into_any_element(),
            );
        }
    }
    out
}

fn shift_marks(marks: &mut Vec<EditorMark>, start: usize, end: usize, inserted: usize) {
    let delta = inserted as i64 - (end - start) as i64;
    let mut next = Vec::new();
    for mark in marks.drain(..) {
        if mark.range.end <= start {
            next.push(mark);
        } else if mark.range.start >= end {
            let mut m = mark;
            m.range.start = ((m.range.start as i64) + delta) as usize;
            m.range.end = ((m.range.end as i64) + delta) as usize;
            next.push(m);
        } else {
            continue;
        }
    }
    *marks = next;
}

fn toggle_mark_on_line(line: &mut EditorLine, range: Range<usize>, kind: MarkKind) {
    let active = line
        .marks
        .iter()
        .any(|m| m.kind == kind && m.range.start <= range.start && m.range.end >= range.end);
    line.marks
        .retain(|m| !(m.kind == kind && m.range.start < range.end && m.range.end > range.start));
    if !active {
        line.marks.push(EditorMark {
            range: range.clone(),
            kind,
            href: if kind == MarkKind::Link {
                Some("https://".into())
            } else {
                None
            },
        });
    }
}

fn lines_from_tiptap(raw: &str) -> Vec<EditorLine> {
    let Some(doc) = parse_tiptap_doc(raw) else {
        return vec![EditorLine::default()];
    };
    let mut lines = Vec::new();
    if let Some(children) = doc.content {
        for child in children {
            flatten_block(&child, &mut lines);
        }
    }
    lines
}

fn flatten_block(node: &TipTapNode, lines: &mut Vec<EditorLine>) {
    match node.kind.as_str() {
        "paragraph" => lines.push(EditorLine {
            block: BlockKind::Paragraph,
            text: inline_to_text(node.content.as_ref()),
            marks: inline_to_marks(node.content.as_ref()),
            image_src: None,
        }),
        "heading" => {
            let level = node
                .attrs
                .as_ref()
                .and_then(|a| a.get("level"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u8;
            lines.push(EditorLine {
                block: BlockKind::Heading(level),
                text: inline_to_text(node.content.as_ref()),
                marks: inline_to_marks(node.content.as_ref()),
                image_src: None,
            });
        }
        "blockquote" => {
            if let Some(children) = &node.content {
                for child in children {
                    if child.kind == "paragraph" {
                        lines.push(EditorLine {
                            block: BlockKind::Blockquote,
                            text: inline_to_text(child.content.as_ref()),
                            marks: inline_to_marks(child.content.as_ref()),
                            image_src: None,
                        });
                    } else {
                        flatten_block(child, lines);
                    }
                }
            }
        }
        "codeBlock" => lines.push(EditorLine {
            block: BlockKind::CodeBlock,
            text: inline_to_text(node.content.as_ref()),
            marks: Vec::new(),
            image_src: None,
        }),
        "bulletList" => flatten_list(node, lines, BlockKind::BulletItem),
        "orderedList" => flatten_list(node, lines, BlockKind::OrderedItem),
        "taskList" => flatten_task_list(node, lines),
        "image" => {
            let src = node
                .attrs
                .as_ref()
                .and_then(|a| a.get("src"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if let Some(src) = src {
                lines.push(EditorLine {
                    block: BlockKind::Image,
                    text: String::new(),
                    marks: Vec::new(),
                    image_src: Some(src),
                });
            }
        }
        _ => {
            if let Some(children) = &node.content {
                for child in children {
                    flatten_block(child, lines);
                }
            }
        }
    }
}

fn flatten_list(node: &TipTapNode, lines: &mut Vec<EditorLine>, kind: BlockKind) {
    if let Some(children) = &node.content {
        for child in children {
            if child.kind == "listItem"
                && let Some(inner) = &child.content
            {
                for block in inner {
                    if block.kind == "paragraph" {
                        lines.push(EditorLine {
                            block: kind,
                            text: inline_to_text(block.content.as_ref()),
                            marks: inline_to_marks(block.content.as_ref()),
                            image_src: None,
                        });
                    } else {
                        flatten_block(block, lines);
                    }
                }
            }
        }
    }
}

fn flatten_task_list(node: &TipTapNode, lines: &mut Vec<EditorLine>) {
    if let Some(children) = &node.content {
        for child in children {
            if child.kind == "taskItem" {
                let checked = child
                    .attrs
                    .as_ref()
                    .and_then(|a| a.get("checked"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(inner) = &child.content {
                    for block in inner {
                        if block.kind == "paragraph" {
                            lines.push(EditorLine {
                                block: BlockKind::TaskItem { checked },
                                text: inline_to_text(block.content.as_ref()),
                                marks: inline_to_marks(block.content.as_ref()),
                                image_src: None,
                            });
                        }
                    }
                }
            }
        }
    }
}

fn inline_to_text(content: Option<&Vec<TipTapNode>>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    let mut out = String::new();
    for node in content {
        match node.kind.as_str() {
            "text" => out.push_str(node.text.as_deref().unwrap_or("")),
            "hardBreak" => out.push('\n'),
            _ => {}
        }
    }
    out
}

fn inline_to_marks(content: Option<&Vec<TipTapNode>>) -> Vec<EditorMark> {
    let Some(content) = content else {
        return Vec::new();
    };
    let mut marks = Vec::new();
    let mut offset = 0usize;
    for node in content {
        if node.kind == "text" {
            let text = node.text.as_deref().unwrap_or("");
            if let Some(node_marks) = &node.marks {
                for mark in node_marks {
                    let Some(kind) = mark_kind_from_tiptap(mark) else {
                        continue;
                    };
                    marks.push(EditorMark {
                        range: offset..offset + text.len(),
                        kind,
                        href: mark
                            .attrs
                            .as_ref()
                            .and_then(|a| a.get("href"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    });
                }
            }
            offset += text.len();
        } else if node.kind == "hardBreak" {
            offset += 1;
        }
    }
    marks
}

fn mark_kind_from_tiptap(mark: &TipTapMark) -> Option<MarkKind> {
    match mark.kind.as_str() {
        "bold" => Some(MarkKind::Bold),
        "italic" => Some(MarkKind::Italic),
        "strike" => Some(MarkKind::Strike),
        "code" => Some(MarkKind::Code),
        "link" => Some(MarkKind::Link),
        _ => None,
    }
}

fn lines_to_tiptap_json(lines: &[EditorLine]) -> String {
    if lines.is_empty() {
        return json!({"type":"doc","content":[{"type":"paragraph"}]}).to_string();
    }
    let mut content: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].block {
            BlockKind::BulletItem => {
                let (items, next) = collect_list_lines(lines, i, BlockKind::BulletItem);
                content.push(json!({
                    "type": "bulletList",
                    "content": items.iter().map(list_item_json).collect::<Vec<_>>()
                }));
                i = next;
            }
            BlockKind::OrderedItem => {
                let (items, next) = collect_list_lines(lines, i, BlockKind::OrderedItem);
                content.push(json!({
                    "type": "orderedList",
                    "attrs": { "start": 1, "type": null },
                    "content": items.iter().map(list_item_json).collect::<Vec<_>>()
                }));
                i = next;
            }
            BlockKind::TaskItem { .. } => {
                let (items, next) = collect_task_lines(lines, i);
                content.push(json!({
                    "type": "taskList",
                    "content": items.iter().map(task_item_json).collect::<Vec<_>>()
                }));
                i = next;
            }
            BlockKind::Blockquote => {
                let (items, next) = collect_blockquote_lines(lines, i);
                content.push(json!({
                    "type": "blockquote",
                    "content": items.iter().map(|l| line_to_block_json(l)).collect::<Vec<_>>()
                }));
                i = next;
            }
            _ => {
                content.push(line_to_block_json(&lines[i]));
                i += 1;
            }
        }
    }
    json!({"type":"doc","content": content}).to_string()
}

fn collect_list_lines(
    lines: &[EditorLine],
    start: usize,
    kind: BlockKind,
) -> (Vec<&EditorLine>, usize) {
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() && lines[i].block == kind {
        items.push(&lines[i]);
        i += 1;
    }
    (items, i)
}

fn collect_task_lines(lines: &[EditorLine], start: usize) -> (Vec<&EditorLine>, usize) {
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if let BlockKind::TaskItem { .. } = lines[i].block {
            items.push(&lines[i]);
            i += 1;
        } else {
            break;
        }
    }
    (items, i)
}

fn collect_blockquote_lines(lines: &[EditorLine], start: usize) -> (Vec<&EditorLine>, usize) {
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() {
        if lines[i].block == BlockKind::Blockquote {
            items.push(&lines[i]);
            i += 1;
        } else {
            break;
        }
    }
    (items, i)
}

fn list_item_json(line: &&EditorLine) -> Value {
    json!({
        "type": "listItem",
        "content": [paragraph_json(line)]
    })
}

fn task_item_json(line: &&EditorLine) -> Value {
    let checked = matches!(line.block, BlockKind::TaskItem { checked: true });
    json!({
        "type": "taskItem",
        "attrs": { "checked": checked },
        "content": [paragraph_json(line)]
    })
}

fn line_to_block_json(line: &EditorLine) -> Value {
    match line.block {
        BlockKind::Image => {
            let src = line.image_src.as_deref().unwrap_or("");
            json!({
                "type": "image",
                "attrs": {
                    "src": src,
                    "alt": null,
                    "title": null,
                    "width": null,
                    "height": null
                }
            })
        }
        BlockKind::Heading(level) => {
            let content = inline_json(&line.text, &line.marks);
            if content.is_empty() {
                json!({
                    "type": "heading",
                    "attrs": { "level": level }
                })
            } else {
                json!({
                    "type": "heading",
                    "attrs": { "level": level },
                    "content": content
                })
            }
        }
        BlockKind::CodeBlock => {
            if line.text.is_empty() {
                json!({
                    "type": "codeBlock",
                    "attrs": { "language": null }
                })
            } else {
                json!({
                    "type": "codeBlock",
                    "attrs": { "language": null },
                    "content": [{ "type": "text", "text": line.text }]
                })
            }
        }
        _ => paragraph_json(line),
    }
}

fn paragraph_json(line: &EditorLine) -> Value {
    let content = inline_json(&line.text, &line.marks);
    if content.is_empty() {
        json!({ "type": "paragraph" })
    } else {
        json!({
            "type": "paragraph",
            "content": content
        })
    }
}

fn inline_json(text: &str, marks: &[EditorMark]) -> Vec<Value> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut nodes = Vec::new();
    let mut offset = 0usize;
    for segment in text.split('\n') {
        if !segment.is_empty() {
            nodes.extend(text_nodes_for_range(segment, offset, marks));
        }
        offset += segment.len();
        if offset < text.len() {
            nodes.push(json!({"type":"hardBreak"}));
            offset += 1;
        }
    }
    nodes
}

fn text_nodes_for_range(text: &str, base_offset: usize, marks: &[EditorMark]) -> Vec<Value> {
    let end_offset = base_offset + text.len();
    let mut bounds = vec![0usize, text.len()];
    for mark in marks {
        if mark.range.start < end_offset && mark.range.end > base_offset {
            bounds.push(mark.range.start.saturating_sub(base_offset));
            bounds.push(mark.range.end.saturating_sub(base_offset).min(text.len()));
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    bounds
        .windows(2)
        .filter_map(|w| {
            let (start, end) = (w[0], w[1]);
            if end <= start {
                return None;
            }
            let segment = &text[start..end];
            let abs_start = base_offset + start;
            let abs_end = base_offset + end;
            Some(text_node_json(
                segment,
                marks_for_range(abs_start..abs_end, marks),
            ))
        })
        .collect()
}

fn text_node_json(text: &str, marks: Vec<Value>) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("type".into(), Value::String("text".into()));
    if !marks.is_empty() {
        map.insert("marks".into(), Value::Array(marks));
    }
    map.insert("text".into(), Value::String(text.into()));
    Value::Object(map)
}

fn marks_for_range(range: Range<usize>, marks: &[EditorMark]) -> Vec<Value> {
    let mut out = Vec::new();
    for mark in marks {
        if mark.range.start < range.end && mark.range.end > range.start {
            let kind = match mark.kind {
                MarkKind::Bold => "bold",
                MarkKind::Italic => "italic",
                MarkKind::Strike => "strike",
                MarkKind::Code => "code",
                MarkKind::Link => "link",
            };
            if mark.kind == MarkKind::Link {
                out.push(json!({
                    "type": kind,
                    "attrs": {
                        "href": mark.href.clone().unwrap_or_else(|| "https://".into()),
                        "target": "_blank",
                        "rel": "noopener noreferrer nofollow",
                        "class": null,
                        "title": null
                    }
                }));
            } else {
                out.push(json!({ "type": kind }));
            }
        }
    }
    out
}

#[derive(IntoElement)]
pub struct CanvasEditor {
    state: Entity<CanvasEditorState>,
    base: Div,
}

impl CanvasEditor {
    pub fn new(state: &Entity<CanvasEditorState>) -> Self {
        Self {
            state: state.clone(),
            base: div().w_full().flex().flex_col().flex_1().min_h_0(),
        }
    }
}

impl Styled for CanvasEditor {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for CanvasEditor {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.base.child(self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn ime_composition_does_not_duplicate_characters(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let entity = cx.update(|window, cx| {
            cx.new(|cx| CanvasEditorState::new(window, cx, "placeholder", "en"))
        });
        cx.update(|window, cx| {
            entity.update(cx, |state, cx| {
                <CanvasEditorState as EntityInputHandler>::replace_and_mark_text_in_range(
                    state, None, "C", None, window, cx,
                );
                <CanvasEditorState as EntityInputHandler>::replace_and_mark_text_in_range(
                    state, None, "Ch", None, window, cx,
                );
                <CanvasEditorState as EntityInputHandler>::replace_and_mark_text_in_range(
                    state, None, "Chu", None, window, cx,
                );
                <CanvasEditorState as EntityInputHandler>::replace_text_in_range(
                    state, None, "Chu", window, cx,
                );
            });
        });
        let content = cx.update(|_, cx| entity.read(cx).content.to_string());
        assert_eq!(content, "Chu");
    }

    #[test]
    fn roundtrip_italic_text() {
        let raw = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hello","marks":[{"type":"italic"}]}]}]}"#;
        let lines = lines_from_tiptap(raw);
        let back = lines_to_tiptap_json(&lines);
        assert!(back.contains("italic"));
        assert!(back.contains("Hello"));
    }

    #[gpui::test]
    fn toggle_italic_updates_marks_immediately(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let entity = cx.update(|window, cx| {
            cx.new(|cx| CanvasEditorState::new(window, cx, "placeholder", "en"))
        });
        cx.update(|window, cx| {
            entity.update(cx, |state, cx| {
                state.set_doc(
                    r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Chu"}]}]}"#,
                    cx,
                );
                state.selected_range = 0..3;
                state.italic(&Italic, window, cx);
            });
        });
        cx.update(|_, cx| {
            let state = entity.read(cx);
            assert!(
                state.lines[0]
                    .marks
                    .iter()
                    .any(|m| m.kind == MarkKind::Italic && m.range == (0..3)),
                "italic mark missing on line after toggle"
            );
            assert!(
                state
                    .styled_marks
                    .iter()
                    .any(|m| m.italic && m.range == (0..3)),
                "styled_marks missing italic after toggle"
            );
            assert!(state.selection_has_mark(MarkKind::Italic));
        });
    }

    #[gpui::test]
    fn toggle_code_block_updates_block_immediately(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let entity = cx.update(|window, cx| {
            cx.new(|cx| CanvasEditorState::new(window, cx, "placeholder", "en"))
        });
        cx.update(|window, cx| {
            entity.update(cx, |state, cx| {
                state.set_doc(
                    r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"code"}]}]}"#,
                    cx,
                );
                state.selected_range = 0..4;
                state.code_block(&CodeBlock, window, cx);
            });
        });
        cx.update(|_, cx| {
            let state = entity.read(cx);
            assert_eq!(state.lines[0].block, BlockKind::CodeBlock);
            assert!(
                state.styled_marks.iter().any(|m| m.code),
                "code styled mark missing after toggle"
            );
        });
    }

    #[test]
    fn image_roundtrip_preserves_src() {
        let src = "https://cdn.mezon.ai/1821743062989148160/2077689353399701504.png";
        let raw = format!(
            r#"{{"type":"doc","content":[{{"type":"paragraph","content":[{{"type":"text","text":"Before"}}]}},{{"type":"image","attrs":{{"src":"{src}"}}}},{{"type":"paragraph","content":[{{"type":"text","text":"After"}}]}}]}}"#
        );
        let lines = lines_from_tiptap(&raw);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].block, BlockKind::Image);
        assert_eq!(lines[1].image_src.as_deref(), Some(src));
        let back = lines_to_tiptap_json(&lines);
        assert!(back.contains(r#""type":"image""#));
        assert!(back.contains(src));
    }

    #[test]
    fn serializes_like_tiptap_get_json() {
        let lines = vec![
            EditorLine {
                block: BlockKind::Heading(1),
                text: "G10".into(),
                marks: Vec::new(),
                image_src: None,
            },
            EditorLine {
                block: BlockKind::Paragraph,
                text: "Chu Van Gia".into(),
                marks: vec![
                    EditorMark {
                        range: 0..11,
                        kind: MarkKind::Bold,
                        href: None,
                    },
                    EditorMark {
                        range: 4..7,
                        kind: MarkKind::Strike,
                        href: None,
                    },
                ],
                image_src: None,
            },
            EditorLine::default(),
        ];
        let json = lines_to_tiptap_json(&lines);
        let expected = r#"{"type":"doc","content":[{"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"G10"}]},{"type":"paragraph","content":[{"type":"text","marks":[{"type":"bold"}],"text":"Chu "},{"type":"text","marks":[{"type":"bold"},{"type":"strike"}],"text":"Van"},{"type":"text","marks":[{"type":"bold"}],"text":" Gia"}]},{"type":"paragraph"}]}"#;
        assert_eq!(
            json, expected,
            "desktop serialize must match TipTap getJSON exactly"
        );
        assert!(
            !json.contains(r#""marks":[]"#),
            "empty marks arrays make web TipTap dirty on load: {json}"
        );
        assert!(
            json.contains(r#"{"type":"text","text":"G10"}"#),
            "plain text should omit marks: {json}"
        );
        assert!(
            json.contains(r#"{"type":"text","marks":[{"type":"bold"}],"text":"Chu "}"#),
            "marks should precede text like TipTap getJSON: {json}"
        );
        assert!(
            json.contains(r#"{"type":"paragraph"}"#) || json.ends_with(r#"{"type":"paragraph"}]}"#),
            "empty paragraph should omit empty content array: {json}"
        );
    }

    #[test]
    fn serializes_image_code_ordered_like_tiptap() {
        let image = EditorLine {
            block: BlockKind::Image,
            text: String::new(),
            marks: Vec::new(),
            image_src: Some("https://cdn.mezon.ai/x.png".into()),
        };
        let code = EditorLine {
            block: BlockKind::CodeBlock,
            text: "fn main() {}".into(),
            marks: Vec::new(),
            image_src: None,
        };
        let ordered = EditorLine {
            block: BlockKind::OrderedItem,
            text: "1".into(),
            marks: Vec::new(),
            image_src: None,
        };
        let link = EditorLine {
            block: BlockKind::Paragraph,
            text: "Mezon".into(),
            marks: vec![EditorMark {
                range: 0..5,
                kind: MarkKind::Link,
                href: Some("https://mezon.ai".into()),
            }],
            image_src: None,
        };

        let image_json = lines_to_tiptap_json(&[image]);
        assert_eq!(
            image_json,
            r#"{"type":"doc","content":[{"type":"image","attrs":{"src":"https://cdn.mezon.ai/x.png","alt":null,"title":null,"width":null,"height":null}}]}"#
        );

        let code_json = lines_to_tiptap_json(&[code]);
        assert_eq!(
            code_json,
            r#"{"type":"doc","content":[{"type":"codeBlock","attrs":{"language":null},"content":[{"type":"text","text":"fn main() {}"}]}]}"#
        );

        let ordered_json = lines_to_tiptap_json(&[ordered]);
        assert_eq!(
            ordered_json,
            r#"{"type":"doc","content":[{"type":"orderedList","attrs":{"start":1,"type":null},"content":[{"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"1"}]}]}]}]}"#
        );

        let link_json = lines_to_tiptap_json(&[link]);
        assert_eq!(
            link_json,
            r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","marks":[{"type":"link","attrs":{"href":"https://mezon.ai","target":"_blank","rel":"noopener noreferrer nofollow","class":null,"title":null}}],"text":"Mezon"}]}]}"#
        );
    }

    #[test]
    fn code_block_serializes_as_codeblock() {
        let line = EditorLine {
            block: BlockKind::CodeBlock,
            text: "fn main() {}".into(),
            marks: Vec::new(),
            image_src: None,
        };
        let json = lines_to_tiptap_json(&[line]);
        assert!(json.contains("codeBlock"));
        assert!(json.contains("fn main()"));
    }

    #[test]
    fn roundtrip_bold_text() {
        let raw = r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Hello","marks":[{"type":"bold"}]}]}]}"#;
        let lines = lines_from_tiptap(raw);
        let back = lines_to_tiptap_json(&lines);
        assert!(back.contains("bold"));
        assert!(back.contains("Hello"));
    }

    #[test]
    fn repeated_phrase_marks_do_not_bleed() {
        let line = EditorLine {
            text: "aaa aaa aaa".into(),
            marks: vec![
                EditorMark {
                    range: 0..3,
                    kind: MarkKind::Bold,
                    href: None,
                },
                EditorMark {
                    range: 4..7,
                    kind: MarkKind::Strike,
                    href: None,
                },
                EditorMark {
                    range: 8..11,
                    kind: MarkKind::Link,
                    href: Some("https://example.com".into()),
                },
            ],
            ..EditorLine::default()
        };
        let json = lines_to_tiptap_json(&[line]);
        let back = lines_from_tiptap(&json);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].text, "aaa aaa aaa");
        assert_eq!(back[0].marks.len(), 3);
        assert_eq!(back[0].marks[0].range, 0..3);
        assert_eq!(back[0].marks[1].range, 4..7);
        assert_eq!(back[0].marks[2].range, 8..11);
        assert_eq!(back[0].marks[0].kind, MarkKind::Bold);
        assert_eq!(back[0].marks[1].kind, MarkKind::Strike);
        assert_eq!(back[0].marks[2].kind, MarkKind::Link);

        let doc: Value = serde_json::from_str(&json).expect("json");
        let nodes = doc["content"][0]["content"].as_array().expect("inline");
        let bold_only = nodes
            .iter()
            .find(|n| n["text"] == "aaa" && n["marks"].as_array().is_some_and(|m| m.len() == 1));
        assert!(bold_only.is_some(), "expected bold-only first aaa node");
        let strike_only = nodes.iter().find(|n| {
            n["text"] == "aaa"
                && n["marks"]
                    .as_array()
                    .is_some_and(|m| m.len() == 1 && m[0]["type"] == "strike")
        });
        assert!(strike_only.is_some(), "expected strike-only aaa node");
    }

    #[test]
    fn block_paint_meta_task_list_uses_checkbox_prefix() {
        let lines = vec![EditorLine {
            block: BlockKind::TaskItem { checked: false },
            text: "item".into(),
            marks: Vec::new(),
            image_src: None,
        }];
        let meta = block_paint_meta(&lines[0], 0, &lines, px(16.));
        assert_eq!(meta.prefix.as_ref(), "☐ ");
    }

    #[test]
    fn block_paint_meta_code_block_enables_monospace_background() {
        let lines = vec![EditorLine {
            block: BlockKind::CodeBlock,
            text: "fn main() {}".into(),
            marks: Vec::new(),
            image_src: None,
        }];
        let meta = block_paint_meta(&lines[0], 0, &lines, px(16.));
        assert!(meta.monospace);
        assert!(meta.code_background);
    }

    #[test]
    fn task_list_serializes_as_task_list() {
        let line = EditorLine {
            block: BlockKind::TaskItem { checked: false },
            text: "todo".into(),
            marks: Vec::new(),
            image_src: None,
        };
        let json = lines_to_tiptap_json(&[line]);
        assert!(json.contains("taskList"));
        assert!(json.contains("taskItem"));
        assert!(json.contains("todo"));
    }

    #[gpui::test]
    fn shift_enter_hard_break_does_not_panic_on_vietnamese(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let entity = cx.update(|window, cx| {
            cx.new(|cx| CanvasEditorState::new(window, cx, "placeholder", "en"))
        });
        cx.update(|_window, cx| {
            entity.update(cx, |state, cx| {
                state.set_doc(
                    r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Chu Văn Giá"}]}]}"#,
                    cx,
                );
                state.move_to(state.content.len(), cx);
                state.insert_hard_break(cx);
            });
        });
        let content = cx.update(|_, cx| entity.read(cx).content.to_string());
        assert!(content.contains('\n'));
        assert!(content.contains("Chu Văn Giá"));
    }

    #[gpui::test]
    fn selection_x_range_covers_the_full_line_including_last_glyph(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let run = TextRun {
                len: 3,
                font: style.font(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let wrapped = window
                .text_system()
                .shape_text(SharedString::from("Chu"), font_size, &[run], None, None)
                .unwrap_or_default();
            let line = wrapped.into_iter().next().expect("shaped line");
            let full_width = line.unwrapped_layout.x_for_index(line.len());
            let (x0, x1) = selection_x_range_in_row(&line, 0, line.len(), 0, line.len());
            assert_eq!(x0, Pixels::ZERO);
            assert_eq!(
                x1, full_width,
                "selection highlight must reach the true end of the line, including the last glyph"
            );
            let _ = cx;
        });
    }

    #[test]
    fn snap_offset_stays_on_char_boundary_for_vietnamese() {
        let text = "Chu Văn Giá";
        let boundary = snap_offset(text, 5);
        assert!(text.is_char_boundary(boundary));
    }

    #[test]
    fn ordered_line_number_restarts_after_paragraph() {
        let lines = vec![
            EditorLine {
                block: BlockKind::OrderedItem,
                text: "one".into(),
                marks: Vec::new(),
                image_src: None,
            },
            EditorLine {
                block: BlockKind::OrderedItem,
                text: "two".into(),
                marks: Vec::new(),
                image_src: None,
            },
            EditorLine {
                block: BlockKind::Paragraph,
                text: "break".into(),
                marks: Vec::new(),
                image_src: None,
            },
            EditorLine {
                block: BlockKind::OrderedItem,
                text: "one again".into(),
                marks: Vec::new(),
                image_src: None,
            },
        ];
        assert_eq!(ordered_line_number(&lines, 0), 1);
        assert_eq!(ordered_line_number(&lines, 1), 2);
        assert_eq!(ordered_line_number(&lines, 3), 1);
    }
}
