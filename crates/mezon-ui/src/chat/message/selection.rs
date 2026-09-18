use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Rgba, Window, fill,
};
use gpui::{Bounds, HighlightStyle, Hsla, Pixels, Point, SharedString, TextLayout};
use mezon_store::MessageId;

pub type SharedSelection = Rc<RefCell<MessageSelectionState>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SelPoint {
    pub message_id: MessageId,
    pub offset: usize,
}

enum SegmentGeometry {
    Text {
        layout: TextLayout,
        end_truncated: bool,
    },
    Bounds {
        bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        vertical: bool,
    },
}

pub struct TextSegment {
    geometry: SegmentGeometry,
    range: Range<usize>,
    clip: Option<Rc<Cell<Option<Bounds<Pixels>>>>>,
}

/// Wraps an existing element to expose its painted bounds to the selection
/// model without adding another GPUI layout node. The overlay is painted after
/// the child so replaced content (emoji, cards, attachments) gets the same
/// translucent selection treatment as text.
pub struct SelectableRegion {
    child: AnyElement,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    overlay: Option<Rgba>,
}

impl SelectableRegion {
    pub fn new(
        child: AnyElement,
        bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        overlay: Option<Rgba>,
    ) -> Self {
        Self {
            child,
            bounds,
            overlay,
        }
    }
}

impl Element for SelectableRegion {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        (self.child.request_layout(window, cx), ())
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
        self.child.prepaint(window, cx);
        self.bounds.set(Some(bounds));
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
        self.child.paint(window, cx);
        if let Some(overlay) = self.overlay {
            window.paint_quad(fill(bounds, overlay));
        }
    }
}

impl IntoElement for SelectableRegion {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextSegment {
    pub fn text(layout: TextLayout, range: Range<usize>) -> Self {
        Self {
            geometry: SegmentGeometry::Text {
                layout,
                end_truncated: false,
            },
            range,
            clip: None,
        }
    }

    pub fn end_truncated_text(layout: TextLayout, range: Range<usize>) -> Self {
        Self {
            geometry: SegmentGeometry::Text {
                layout,
                end_truncated: true,
            },
            range,
            clip: None,
        }
    }

    pub fn bounded(range: Range<usize>, bounds: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
        Self {
            geometry: SegmentGeometry::Bounds {
                bounds,
                vertical: false,
            },
            range,
            clip: None,
        }
    }

    pub fn block(range: Range<usize>, bounds: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
        Self {
            geometry: SegmentGeometry::Bounds {
                bounds,
                vertical: true,
            },
            range,
            clip: None,
        }
    }

    pub fn clipped(mut self, clip: Rc<Cell<Option<Bounds<Pixels>>>>) -> Self {
        self.clip = Some(clip);
        self
    }

    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = match &self.geometry {
            SegmentGeometry::Text { layout, .. } => layout.try_bounds(),
            SegmentGeometry::Bounds { bounds, .. } => bounds.get(),
        }?;
        match &self.clip {
            Some(clip) => {
                let clipped = bounds.intersect(&clip.get()?);
                (clipped.size.width > Pixels::ZERO && clipped.size.height > Pixels::ZERO)
                    .then_some(clipped)
            }
            None => Some(bounds),
        }
    }

    pub fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.bounds()?;
        if !bounds.contains(&position) {
            return None;
        }
        match &self.geometry {
            SegmentGeometry::Text {
                layout,
                end_truncated,
            } => {
                let local = layout
                    .try_index_for_position(position)?
                    .unwrap_or_else(|offset| offset);
                let layout_len = layout.try_len()?;
                Some(map_text_segment_offset(
                    local,
                    layout_len,
                    &self.range,
                    *end_truncated,
                ))
            }
            SegmentGeometry::Bounds { vertical, .. } => {
                let before = if *vertical {
                    position.y < bounds.top() + bounds.size.height / 2.
                } else {
                    position.x < bounds.left() + bounds.size.width / 2.
                };
                Some(if before {
                    self.range.start
                } else {
                    self.range.end
                })
            }
        }
    }

    pub fn snapped_offset(&self, position: Point<Pixels>) -> Option<(Pixels, usize)> {
        if let Some(offset) = self.offset_at(position) {
            return Some((Pixels::ZERO, offset));
        }
        let bounds = self.bounds()?;
        if position.y < bounds.top() || position.y > bounds.bottom() {
            return None;
        }
        if position.x <= bounds.left() {
            Some((bounds.left() - position.x, self.range.start))
        } else {
            Some((position.x - bounds.right(), self.range.end))
        }
    }

    pub fn end(&self) -> usize {
        self.range.end
    }
}

#[derive(Default)]
pub struct MessageEntry {
    pub segments: Vec<TextSegment>,
    pub text: SharedString,
    vertical_bounds: Cell<Option<(Pixels, Pixels)>>,
}

impl MessageEntry {
    pub fn vertical_bounds(&self) -> Option<(Pixels, Pixels)> {
        if let Some(bounds) = self.vertical_bounds.get() {
            return Some(bounds);
        }
        let mut band: Option<(Pixels, Pixels)> = None;
        for segment in &self.segments {
            if let Some(bounds) = segment.bounds() {
                band = Some(match band {
                    Some((top, bottom)) => (top.min(bounds.top()), bottom.max(bounds.bottom())),
                    None => (bounds.top(), bounds.bottom()),
                });
            }
        }
        self.vertical_bounds.set(band);
        band
    }
}

#[derive(Default)]
pub struct MessageSelectionState {
    pub anchor: Option<SelPoint>,
    pub head: Option<SelPoint>,
    pub selecting: bool,
    pub registry: HashMap<MessageId, TextLayout>,
    pub segment_registry: HashMap<MessageId, MessageEntry>,
    segment_pool: HashMap<MessageId, MessageEntry>,
    pub order_map: HashMap<MessageId, usize>,
}

impl MessageSelectionState {
    pub fn new_shared() -> SharedSelection {
        Rc::new(RefCell::new(Self::default()))
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.selecting = false;
        self.order_map.clear();
    }

    /// Starts a new render pass while retaining the allocations used by the
    /// previous pass. Geometry itself is never reused because it belongs to
    /// the old frame; only the hash-map buckets and segment vector capacities
    /// are recycled.
    pub fn begin_render(&mut self) {
        self.registry.clear();
        self.segment_pool.clear();
        std::mem::swap(&mut self.segment_registry, &mut self.segment_pool);
    }

    fn segment_entry(&mut self, message_id: MessageId) -> &mut MessageEntry {
        let pool = &mut self.segment_pool;
        self.segment_registry
            .entry(message_id)
            .or_insert_with(|| pool.remove(&message_id).unwrap_or_default())
    }

    pub fn take_segment_buffer(
        &mut self,
        message_id: MessageId,
        text: SharedString,
    ) -> Vec<TextSegment> {
        let entry = self.segment_entry(message_id);
        entry.text = text;
        entry.vertical_bounds.set(None);
        let mut segments = std::mem::take(&mut entry.segments);
        segments.clear();
        segments
    }

    pub fn store_segment_buffer(
        &mut self,
        message_id: MessageId,
        text: SharedString,
        mut segments: Vec<TextSegment>,
    ) {
        let entry = self.segment_entry(message_id);
        entry.text = text;
        entry.vertical_bounds.set(None);
        if entry.segments.is_empty() {
            entry.segments = segments;
        } else {
            entry.segments.append(&mut segments);
        }
    }

    pub fn has_selection(&self) -> bool {
        match (self.anchor, self.head) {
            (Some(a), Some(h)) => a != h,
            _ => false,
        }
    }

    pub fn includes_message(&self, id: MessageId) -> bool {
        let (Some(anchor), Some(head)) = (self.anchor, self.head) else {
            return false;
        };
        if anchor == head {
            return false;
        }
        if anchor.message_id == head.message_id {
            return anchor.message_id == id;
        }

        let (Some(&anchor_ord), Some(&head_ord), Some(&message_ord)) = (
            self.order_map.get(&anchor.message_id),
            self.order_map.get(&head.message_id),
            self.order_map.get(&id),
        ) else {
            return false;
        };
        let (start, end) = if anchor_ord <= head_ord {
            (anchor_ord, head_ord)
        } else {
            (head_ord, anchor_ord)
        };
        (start..=end).contains(&message_ord)
    }

    pub fn range_for_message(&self, id: MessageId, text: &str) -> Option<Range<usize>> {
        let len = text.len();
        let anchor = self.anchor?;
        let head = self.head?;

        if anchor.message_id == id && head.message_id == id {
            let (start, end) = if anchor.offset <= head.offset {
                (anchor.offset, head.offset)
            } else {
                (head.offset, anchor.offset)
            };
            let start = floor_char_boundary(text, start);
            let end = floor_char_boundary(text, end);
            return (start < end).then_some(start..end);
        }

        let anchor_ord = *self.order_map.get(&anchor.message_id)?;
        let head_ord = *self.order_map.get(&head.message_id)?;
        let msg_ord = *self.order_map.get(&id)?;

        let a = (anchor_ord, anchor.offset);
        let h = (head_ord, head.offset);
        let (start, end) = if a <= h { (a, h) } else { (h, a) };

        if msg_ord < start.0 || msg_ord > end.0 {
            return None;
        }
        let start_off = if msg_ord == start.0 { start.1 } else { 0 };
        let end_off = if msg_ord == end.0 { end.1 } else { len };
        let start_off = floor_char_boundary(text, start_off);
        let end_off = floor_char_boundary(text, end_off);
        if start_off >= end_off {
            return None;
        }
        Some(start_off..end_off)
    }
}

fn map_text_segment_offset(
    local: usize,
    layout_len: usize,
    source_range: &Range<usize>,
    end_truncated: bool,
) -> usize {
    let source_len = source_range.len();
    if end_truncated && layout_len < source_len {
        let visible_source_len = layout_len.saturating_sub('…'.len_utf8());
        if local >= visible_source_len {
            return source_range.end;
        }
    }
    source_range.start + local.min(source_len)
}

pub fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub fn word_range(text: &str, index: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let index = floor_char_boundary(text, index);
    let probe = if index == text.len() {
        text.char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset)
    } else {
        index
    };
    let Some(character) = text[probe..].chars().next() else {
        return 0..0;
    };
    let class = |value: char| {
        if value.is_whitespace() {
            0
        } else if value.is_alphanumeric() || value == '_' {
            1
        } else {
            2
        }
    };
    let target = class(character);
    let mut start = probe;
    for (offset, value) in text[..probe].char_indices().rev() {
        if class(value) != target {
            break;
        }
        start = offset;
    }
    let mut end = probe + character.len_utf8();
    let tail_start = end;
    for (offset, value) in text[tail_start..].char_indices() {
        if class(value) != target {
            break;
        }
        end = tail_start + offset + value.len_utf8();
    }
    start..end
}

pub fn merge_selection_background(
    base: &[(Range<usize>, HighlightStyle)],
    sel: Range<usize>,
    background: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(base.len() + 2);

    let emit = |out: &mut Vec<(Range<usize>, HighlightStyle)>,
                start: usize,
                end: usize,
                style: Option<&HighlightStyle>| {
        let mut cursor = start;
        while cursor < end {
            let in_sel = cursor >= sel.start && cursor < sel.end;
            let next = if in_sel {
                end.min(sel.end)
            } else if cursor < sel.start {
                end.min(sel.start)
            } else {
                end
            };
            if next > cursor && (style.is_some() || in_sel) {
                let mut resolved = style.cloned().unwrap_or_default();
                if in_sel {
                    resolved.background_color = Some(background);
                }
                out.push((cursor..next, resolved));
            }
            cursor = next;
        }
    };

    let mut covered = 0usize;
    for (range, style) in base {
        if range.start > covered {
            emit(&mut out, covered, range.start, None);
        }
        emit(&mut out, range.start, range.end, Some(style));
        covered = covered.max(range.end);
    }
    if covered < sel.end {
        emit(&mut out, covered, sel.end, None);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, hsla, point, px, size};
    use mezon_store::MessageId;

    fn bg() -> Hsla {
        hsla(0.6, 0.9, 0.6, 0.3)
    }

    fn color(base: f32) -> HighlightStyle {
        HighlightStyle {
            color: Some(hsla(base, 1.0, 0.5, 1.0)),
            ..Default::default()
        }
    }

    #[test]
    fn merge_over_unstyled_text_emits_only_selection() {
        let out = merge_selection_background(&[], 2..5, bg());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 2..5);
        assert_eq!(out[0].1.background_color, Some(bg()));
    }

    #[test]
    fn merge_splits_existing_highlight_preserving_color() {
        let base = vec![(0..10, color(0.1))];
        let out = merge_selection_background(&base, 3..6, bg());
        assert_eq!(
            out.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..3, 3..6, 6..10]
        );
        assert_eq!(out[0].1.background_color, None);
        assert_eq!(out[1].1.background_color, Some(bg()));
        assert_eq!(out[2].1.background_color, None);
        assert_eq!(out[0].1.color, color(0.1).color);
        assert_eq!(out[1].1.color, color(0.1).color);
        assert_eq!(out[2].1.color, color(0.1).color);
    }

    /// The character reserving space for an inline channel icon is hidden with `fade_out`,
    /// and selecting a chip must not bring it back.
    #[test]
    fn merge_keeps_a_faded_out_run_invisible() {
        let hidden = HighlightStyle {
            fade_out: Some(1.),
            ..color(0.1)
        };
        let base = vec![(0..3, hidden), (3..10, color(0.1))];
        let out = merge_selection_background(&base, 0..10, bg());
        assert_eq!(out[0].0, 0..3);
        assert_eq!(out[0].1.fade_out, Some(1.));
        assert_eq!(out[0].1.background_color, Some(bg()));
        assert_eq!(out[1].1.fade_out, None);
    }

    #[test]
    fn merge_covers_selection_past_last_highlight() {
        let base = vec![(0..2, color(0.1))];
        let out = merge_selection_background(&base, 1..5, bg());
        assert_eq!(out[0].1.background_color, None);
        assert_eq!(out[1].0, 1..2);
        assert_eq!(out[1].1.background_color, Some(bg()));
        assert_eq!(out.last().unwrap().0, 2..5);
        assert_eq!(out.last().unwrap().1.background_color, Some(bg()));
    }

    #[test]
    fn range_for_message_single_message_orders_and_clamps() {
        let id = MessageId::default();
        let state = MessageSelectionState {
            anchor: Some(SelPoint {
                message_id: id,
                offset: 7,
            }),
            head: Some(SelPoint {
                message_id: id,
                offset: 2,
            }),
            order_map: HashMap::from([(id, 0)]),
            ..Default::default()
        };
        assert_eq!(state.range_for_message(id, &"x".repeat(100)), Some(2..7));
        assert_eq!(state.range_for_message(id, "xxxxx"), Some(2..5));
    }

    #[test]
    fn range_for_message_clamps_stale_offsets_to_utf8_boundaries() {
        let id = MessageId::default();
        let state = MessageSelectionState {
            anchor: Some(SelPoint {
                message_id: id,
                offset: 2,
            }),
            head: Some(SelPoint {
                message_id: id,
                offset: 8,
            }),
            order_map: HashMap::from([(id, 0)]),
            ..Default::default()
        };

        let text = "ađỏ";
        assert_eq!(state.range_for_message(id, text), Some(1..6));
        assert_eq!(&text[state.range_for_message(id, text).unwrap()], "đỏ");
    }

    #[test]
    fn truncated_text_affix_maps_to_the_source_end() {
        let source = 10..30;

        assert_eq!(map_text_segment_offset(4, 9, &source, true), 14);
        assert_eq!(map_text_segment_offset(6, 9, &source, true), 30);
        assert_eq!(map_text_segment_offset(9, 9, &source, false), 19);
    }

    #[test]
    fn range_for_message_none_when_empty_or_cross_message() {
        let a = MessageId::default();
        let point = SelPoint {
            message_id: a,
            offset: 3,
        };
        let state = MessageSelectionState {
            anchor: Some(point),
            head: Some(point),
            order_map: HashMap::from([(a, 0)]),
            ..Default::default()
        };
        assert_eq!(state.range_for_message(a, "xxxxxxxxxx"), None);
    }

    #[test]
    fn range_for_message_cross_message_boundaries_and_middle() {
        let a = MessageId::new(10);
        let b = MessageId::new(20);
        let c = MessageId::new(30);
        let state = MessageSelectionState {
            anchor: Some(SelPoint {
                message_id: a,
                offset: 3,
            }),
            head: Some(SelPoint {
                message_id: c,
                offset: 5,
            }),
            order_map: HashMap::from([(a, 0), (b, 1), (c, 2)]),
            ..Default::default()
        };
        assert_eq!(state.range_for_message(a, "xxxxxxxx"), Some(3..8));
        assert_eq!(state.range_for_message(b, "xxxxxxxx"), Some(0..8));
        assert_eq!(state.range_for_message(c, "xxxxxxxx"), Some(0..5));
    }

    #[test]
    fn range_for_message_cross_message_reverse_drag() {
        let a = MessageId::new(10);
        let b = MessageId::new(20);
        let c = MessageId::new(30);
        let state = MessageSelectionState {
            anchor: Some(SelPoint {
                message_id: c,
                offset: 5,
            }),
            head: Some(SelPoint {
                message_id: a,
                offset: 3,
            }),
            order_map: HashMap::from([(a, 0), (b, 1), (c, 2)]),
            ..Default::default()
        };
        assert_eq!(state.range_for_message(a, "xxxxxxxx"), Some(3..8));
        assert_eq!(state.range_for_message(b, "xxxxxxxx"), Some(0..8));
        assert_eq!(state.range_for_message(c, "xxxxxxxx"), Some(0..5));
    }

    #[test]
    fn includes_message_filters_to_the_selected_interval() {
        let a = MessageId::new(10);
        let b = MessageId::new(20);
        let c = MessageId::new(30);
        let outside = MessageId::new(40);
        let state = MessageSelectionState {
            anchor: Some(SelPoint {
                message_id: c,
                offset: 5,
            }),
            head: Some(SelPoint {
                message_id: a,
                offset: 3,
            }),
            order_map: HashMap::from([(a, 0), (b, 1), (c, 2), (outside, 3)]),
            ..Default::default()
        };

        assert!(state.includes_message(a));
        assert!(state.includes_message(b));
        assert!(state.includes_message(c));
        assert!(!state.includes_message(outside));
    }

    #[test]
    fn word_range_respects_utf8_boundaries() {
        let text = "xin chào, bạn";
        assert_eq!(word_range(text, 5), 4..9);
        assert_eq!(&text[word_range(text, 5)], "chào");
        assert_eq!(&text[word_range(text, 9)], ",");
    }

    #[test]
    fn bounded_segment_maps_each_half_to_a_caret_boundary() {
        let bounds = Rc::new(Cell::new(Some(Bounds::new(
            point(px(10.), px(20.)),
            size(px(20.), px(24.)),
        ))));
        let segment = TextSegment::bounded(4..11, bounds);

        assert_eq!(segment.offset_at(point(px(14.), px(30.))), Some(4));
        assert_eq!(segment.offset_at(point(px(26.), px(30.))), Some(11));
        assert_eq!(
            segment.snapped_offset(point(px(5.), px(30.))),
            Some((px(5.), 4))
        );
    }

    #[test]
    fn block_segment_maps_each_half_to_a_caret_boundary() {
        let bounds = Rc::new(Cell::new(Some(Bounds::new(
            point(px(10.), px(20.)),
            size(px(200.), px(100.)),
        ))));
        let segment = TextSegment::block(4..7, bounds);

        assert_eq!(segment.offset_at(point(px(50.), px(30.))), Some(4));
        assert_eq!(segment.offset_at(point(px(50.), px(100.))), Some(7));
    }

    #[test]
    fn clipped_segment_ignores_the_unpainted_scroll_area() {
        let bounds = Rc::new(Cell::new(Some(Bounds::new(
            point(px(10.), px(20.)),
            size(px(100.), px(100.)),
        ))));
        let clip = Rc::new(Cell::new(Some(Bounds::new(
            point(px(10.), px(40.)),
            size(px(100.), px(30.)),
        ))));
        let segment = TextSegment::block(4..7, bounds).clipped(clip);

        assert_eq!(segment.bounds().unwrap().top(), px(40.));
        assert_eq!(segment.bounds().unwrap().bottom(), px(70.));
        assert_eq!(segment.offset_at(point(px(50.), px(25.))), None);
        assert_eq!(segment.offset_at(point(px(50.), px(60.))), Some(7));
    }

    #[test]
    fn segment_buffers_reuse_capacity_without_reusing_geometry() {
        let id = MessageId::new(42);
        let mut state = MessageSelectionState::default();
        state.begin_render();

        let mut segments = state.take_segment_buffer(id, "hello".into());
        segments.reserve(8);
        let pointer = segments.as_ptr();
        let capacity = segments.capacity();
        state.store_segment_buffer(id, "hello".into(), segments);

        state.begin_render();
        let reused = state.take_segment_buffer(id, "hello".into());
        assert_eq!(reused.as_ptr(), pointer);
        assert_eq!(reused.capacity(), capacity);
        assert!(reused.is_empty());
    }
}
