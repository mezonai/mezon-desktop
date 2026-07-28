use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, ClipboardItem, Context, DismissEvent,
    DispatchPhase, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, KeyDownEvent, LayoutId, ListAlignment, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    Subscription, Task, TextLayout, WeakEntity, Window, anchored, deferred, div, ease_in_out, list,
    prelude::*, px,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use mezon_store::{
    BadgeService, ChannelId, ChannelList, ChannelPermissionsEvent, ChannelPermissionsStore, ClanId,
    ClanList, ClanMembersStore, DirectMessageStore, EmbedInput, EmbedTextInput, Emoji, EmojiStore,
    GroupMembersStore, MessageCode, MessageId, MessagesEvent, MessagesStore,
    PERMISSION_DELETE_MESSAGE, ProfileContext, Settings, TopicsEvent, TopicsStore, UserId,
    UsersByUserStore,
    message::{Message, markdown_edit_source},
};

use super::audio_player::{AudioActivation, AudioPlayerView};
use super::context::{OnboardingContext, RowCtx, RowMemo, WelcomeContext};
use super::dispatch::render_message_item;
use super::gif_video::GifVideoView;
use super::message_context_menu;
use super::reaction_picker::{ReactionPicker, ReactionPickerEvent};
use super::selection::{MessageSelectionState, SelPoint, SharedSelection, TextSegment, word_range};
use super::skeleton::message_skeleton;
use super::system_row::{build_onboarding_context, build_welcome_context};
use super::video_player::{VideoActivation, VideoPlayerView};
use crate::app::shell::Shell;
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::primitives::input::Copy;
use crate::components::primitives::{Icon, IconName, TextArea, TextAreaEvent, context_menu_at};
use crate::image_cache::{
    LruImageCache, MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};

fn register_selection_listeners(
    window: &mut Window,
    host: WeakEntity<ChannelMessages>,
    selection: SharedSelection,
    hitbox: Hitbox,
) {
    let down_host = host.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || !hitbox.is_hovered(window)
        {
            return;
        }
        let event = event.clone();
        let host = down_host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_down(&event, window, cx));
            }
        });
    });
    let move_host = host.clone();
    let move_selection = selection.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || !move_selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        let event = event.clone();
        let host = move_host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_move(&event, window, cx));
            }
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || !selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        let event = event.clone();
        let host = host.clone();
        window.defer(cx, move |window, cx| {
            if let Some(view) = host.upgrade() {
                view.update(cx, |this, cx| this.on_selection_up(&event, window, cx));
            }
        });
    });
}

/// Adds the list-wide selection hitbox without inserting a canvas/layout node
/// beside the virtualized list.
struct SelectionCapture {
    child: AnyElement,
    host: WeakEntity<ChannelMessages>,
    selection: SharedSelection,
}

impl SelectionCapture {
    fn new(
        child: AnyElement,
        host: WeakEntity<ChannelMessages>,
        selection: SharedSelection,
    ) -> Self {
        Self {
            child,
            host,
            selection,
        }
    }
}

impl Element for SelectionCapture {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

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
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.child.prepaint(window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        register_selection_listeners(
            window,
            self.host.clone(),
            self.selection.clone(),
            hitbox.clone(),
        );
        self.child.paint(window, cx);
    }
}

impl IntoElement for SelectionCapture {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn text_layout_offset_at(layout: &TextLayout, position: Point<Pixels>) -> Option<usize> {
    let bounds = layout.try_bounds()?;
    if bounds.contains(&position) {
        Some(
            layout
                .try_index_for_position(position)?
                .unwrap_or_else(|err| err),
        )
    } else {
        None
    }
}

fn text_layout_offset_snap(layout: &TextLayout, position: Point<Pixels>) -> Option<usize> {
    let bounds = layout.try_bounds()?;
    if position.y >= bounds.top() && position.y <= bounds.bottom() {
        Some(
            layout
                .try_index_for_position(position)?
                .unwrap_or_else(|offset| offset),
        )
    } else {
        None
    }
}

fn segment_offset_at(segments: &[TextSegment], position: Point<Pixels>) -> Option<usize> {
    segments
        .iter()
        .find_map(|segment| segment.offset_at(position))
}

fn segment_offset_snap(segments: &[TextSegment], position: Point<Pixels>) -> Option<usize> {
    let mut best: Option<(Pixels, usize, usize)> = None;
    for (index, segment) in segments.iter().enumerate() {
        let Some((dx, offset)) = segment.snapped_offset(position) else {
            continue;
        };
        if best.is_none_or(|(best_dx, _, _)| dx < best_dx) {
            best = Some((dx, offset, index));
        }
    }
    let (_, mut offset, index) = best?;
    let bounds = segments[index].bounds()?;
    if position.x >= bounds.right() {
        for segment in &segments[index + 1..] {
            if segment.bounds().is_some() {
                break;
            }
            offset = segment.end();
        }
    }
    Some(offset)
}

fn message_point_at(state: &MessageSelectionState, position: Point<Pixels>) -> Option<SelPoint> {
    for (id, layout) in &state.registry {
        if let Some(offset) = text_layout_offset_snap(layout, position) {
            return Some(SelPoint {
                message_id: *id,
                offset,
            });
        }
    }
    for (id, entry) in &state.segment_registry {
        let Some((top, bottom)) = entry.vertical_bounds() else {
            continue;
        };
        if position.y >= top
            && position.y <= bottom
            && let Some(offset) = segment_offset_snap(&entry.segments, position)
        {
            return Some(SelPoint {
                message_id: *id,
                offset,
            });
        }
    }

    let mut best: Option<(Pixels, SelPoint)> = None;
    let mut consider = |id: MessageId, top: Pixels, bottom: Pixels, end: usize| {
        let (dy, offset) = if position.y < top {
            (top - position.y, 0usize)
        } else if position.y > bottom {
            (position.y - bottom, end)
        } else {
            return;
        };
        if best.as_ref().is_none_or(|(best_dy, _)| dy < *best_dy) {
            best = Some((
                dy,
                SelPoint {
                    message_id: id,
                    offset,
                },
            ));
        }
    };
    for (id, layout) in &state.registry {
        if let (Some(bounds), Some(len)) = (layout.try_bounds(), layout.try_len()) {
            consider(*id, bounds.top(), bounds.bottom(), len);
        }
    }
    for (id, entry) in &state.segment_registry {
        if let Some((t, bot)) = entry.vertical_bounds() {
            consider(*id, t, bot, entry.text.len());
        }
    }
    best.map(|(_, point)| point)
}

fn message_offset_at(
    state: &MessageSelectionState,
    message_id: MessageId,
    position: Point<Pixels>,
) -> Option<usize> {
    state
        .registry
        .get(&message_id)
        .and_then(|layout| {
            text_layout_offset_at(layout, position)
                .or_else(|| text_layout_offset_snap(layout, position))
        })
        .or_else(|| {
            let entry = state.segment_registry.get(&message_id)?;
            segment_offset_at(&entry.segments, position)
                .or_else(|| segment_offset_snap(&entry.segments, position))
        })
}

const EXPANDED_SELECTION_DRAG_THRESHOLD_PX: f32 = 2.;

#[derive(Clone, Copy)]
struct ExpandedSelection {
    message_id: MessageId,
    start: usize,
    end: usize,
    origin: Point<Pixels>,
}

impl ExpandedSelection {
    fn drag_started(self, position: Point<Pixels>) -> bool {
        (position.x - self.origin.x).as_f32().abs() > EXPANDED_SELECTION_DRAG_THRESHOLD_PX
            || (position.y - self.origin.y).as_f32().abs() > EXPANDED_SELECTION_DRAG_THRESHOLD_PX
    }
}

fn update_selection_head(
    state: &mut MessageSelectionState,
    point: SelPoint,
    expanded: Option<ExpandedSelection>,
) -> bool {
    let (anchor, head) = if let Some(expanded) = expanded {
        let start = SelPoint {
            message_id: expanded.message_id,
            offset: expanded.start,
        };
        let end = SelPoint {
            message_id: expanded.message_id,
            offset: expanded.end,
        };
        if point.message_id == expanded.message_id {
            if point.offset < expanded.start {
                (end, point)
            } else if point.offset > expanded.end {
                (start, point)
            } else {
                (start, end)
            }
        } else {
            let point_before = state
                .order_map
                .get(&point.message_id)
                .zip(state.order_map.get(&expanded.message_id))
                .is_some_and(|(point_order, expanded_order)| point_order < expanded_order);
            if point_before {
                (end, point)
            } else {
                (start, point)
            }
        }
    } else {
        let Some(anchor) = state.anchor else {
            return false;
        };
        (anchor, point)
    };
    let changed = state.anchor != Some(anchor) || state.head != Some(head);
    state.anchor = Some(anchor);
    state.head = Some(head);
    changed
}

fn selected_text_for_messages(
    state: &MessageSelectionState,
    messages: &[Message],
    locale: &str,
    current_user_id: &str,
    cx: &App,
) -> Option<String> {
    use super::selection::floor_char_boundary;

    let mut parts = Vec::new();
    for message in messages {
        if !state.includes_message(message.id) {
            continue;
        }
        let full = super::content::selectable_message_text(message, locale, current_user_id, cx);
        let Some(range) = state.range_for_message(message.id, &full) else {
            continue;
        };
        let start = floor_char_boundary(&full, range.start);
        let end = floor_char_boundary(&full, range.end);
        if start < end
            && let Some(cleaned) = clipboard_selection_slice(&full[start..end])
        {
            parts.push(cleaned);
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn clipboard_selection_slice(text: &str) -> Option<String> {
    let cleaned = text.replace(
        [
            super::content::INLINE_ICON_PLACEHOLDER,
            super::content::ATTACHMENT_PLACEHOLDER,
        ],
        "",
    );
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod selection_copy_tests {
    use super::clipboard_selection_slice;
    use crate::chat::message::content::{ATTACHMENT_PLACEHOLDER, INLINE_ICON_PLACEHOLDER};

    #[test]
    fn placeholder_only_selection_does_not_clear_the_clipboard() {
        let placeholders = format!("{INLINE_ICON_PLACEHOLDER}{ATTACHMENT_PLACEHOLDER}");
        assert_eq!(clipboard_selection_slice(&placeholders), None);
    }

    #[test]
    fn clipboard_selection_preserves_real_whitespace() {
        assert_eq!(clipboard_selection_slice(" a "), Some(" a ".to_string()));
    }
}

const FAB_SIZE: f32 = 32.;
const FAB_HOVER_GROW: f32 = 4.;
const FAB_HOVER_SIZE: f32 = FAB_SIZE + FAB_HOVER_GROW;
const FAB_BOTTOM: f32 = 20.;
const FAB_RIGHT: f32 = 12.;
const LOAD_MORE_ITEM_THRESHOLD: usize = 12;
const LIST_OVERDRAW: f32 = 1024.;
const LIST_BOTTOM_PADDING: f32 = 20.;
const KEY_SCROLL_STEP_PX: f32 = 40.;
const KEY_SCROLL_DURATION: Duration = Duration::from_millis(150);
const BOTTOM_THRESHOLD_PX: f32 = 100.;
const KEY_SCROLL_BEZIER_X1: f32 = 0.42;
const KEY_SCROLL_BEZIER_X2: f32 = 0.58;
const IDLE_CACHE_SWEEP_INTERVAL: Duration = Duration::from_millis(100);
const HOVER_SHOW_DELAY_MS: u64 = 200;
const HOVER_HIDE_DELAY_MS: u64 = 100;
const SCROLL_RELIEF_DELAY: Duration = Duration::from_millis(1500);
const MAX_GIF_VIDEOS: usize = 6;
const MAX_AUDIO_PLAYERS: usize = 8;

struct PendingGif {
    key: (MessageId, usize),
    mp4: SharedString,
    fallback: SharedString,
    width: f32,
    height: f32,
}
const SKELETON_THRESHOLD_MS: u64 = 500;
const SKELETON_FADE_IN_MS: u64 = 150;
const SKELETON_SETTLE_MS: u64 = 250;
const SKELETON_FADE_OUT_MS: u64 = 180;
const JUMP_PRESENT_MIN_MESSAGES: usize = 20;

#[derive(Clone, Copy, PartialEq, Debug)]
enum SkeletonPhase {
    Hidden,
    Pending,
    Showing,
    Settling,
    FadingOut,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SkeletonTimer {
    Keep,
    Drop,
    Threshold,
    Settle,
    Unmount,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SkeletonKey {
    #[default]
    None,
    Clan(ClanId),
    Conversation(ChannelId),
}

#[derive(Clone, Copy)]
enum SkeletonInput {
    Sync { loading: bool, same_key: bool },
    ThresholdElapsed,
    SettleElapsed,
    UnmountElapsed,
}

enum TimerAction {
    Keep,
    Clear,
    Arm(SkeletonInput, u64),
}

fn skeleton_timer_action(timer: SkeletonTimer) -> TimerAction {
    match timer {
        SkeletonTimer::Keep => TimerAction::Keep,
        SkeletonTimer::Drop => TimerAction::Clear,
        SkeletonTimer::Threshold => {
            TimerAction::Arm(SkeletonInput::ThresholdElapsed, SKELETON_THRESHOLD_MS)
        }
        SkeletonTimer::Settle => TimerAction::Arm(SkeletonInput::SettleElapsed, SKELETON_SETTLE_MS),
        SkeletonTimer::Unmount => {
            TimerAction::Arm(SkeletonInput::UnmountElapsed, SKELETON_FADE_OUT_MS)
        }
    }
}

fn skeleton_transition(
    phase: SkeletonPhase,
    input: SkeletonInput,
) -> (SkeletonPhase, SkeletonTimer) {
    match input {
        SkeletonInput::Sync { loading, same_key } => {
            let phase = if same_key {
                phase
            } else {
                SkeletonPhase::Hidden
            };
            match (phase, loading) {
                (SkeletonPhase::Hidden, true) => (SkeletonPhase::Pending, SkeletonTimer::Threshold),
                (SkeletonPhase::Hidden, false) => (
                    SkeletonPhase::Hidden,
                    if same_key {
                        SkeletonTimer::Keep
                    } else {
                        SkeletonTimer::Drop
                    },
                ),
                (SkeletonPhase::Pending, true) => (SkeletonPhase::Pending, SkeletonTimer::Keep),
                (SkeletonPhase::Pending, false) => (SkeletonPhase::Hidden, SkeletonTimer::Drop),
                (SkeletonPhase::Showing, true) => (SkeletonPhase::Showing, SkeletonTimer::Keep),
                (SkeletonPhase::Showing, false) => (SkeletonPhase::Settling, SkeletonTimer::Settle),
                (SkeletonPhase::Settling, true) => (SkeletonPhase::Showing, SkeletonTimer::Drop),
                (SkeletonPhase::Settling, false) => (SkeletonPhase::Settling, SkeletonTimer::Keep),
                (SkeletonPhase::FadingOut, true) => (SkeletonPhase::Showing, SkeletonTimer::Drop),
                (SkeletonPhase::FadingOut, false) => {
                    (SkeletonPhase::FadingOut, SkeletonTimer::Keep)
                }
            }
        }
        SkeletonInput::ThresholdElapsed => match phase {
            SkeletonPhase::Pending => (SkeletonPhase::Showing, SkeletonTimer::Keep),
            other => (other, SkeletonTimer::Keep),
        },
        SkeletonInput::SettleElapsed => match phase {
            SkeletonPhase::Settling => (SkeletonPhase::FadingOut, SkeletonTimer::Unmount),
            other => (other, SkeletonTimer::Keep),
        },
        SkeletonInput::UnmountElapsed => match phase {
            SkeletonPhase::FadingOut => (SkeletonPhase::Hidden, SkeletonTimer::Keep),
            other => (other, SkeletonTimer::Keep),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyboardScrollAnimation {
    start: f32,
    target: f32,
    started_at: Instant,
    initial_slope: f32,
    applied: f32,
}

fn retarget_keyboard_scroll(
    current: f32,
    previous: Option<KeyboardScrollAnimation>,
    delta: f32,
    now: Instant,
) -> Option<KeyboardScrollAnimation> {
    let velocity = previous.map_or(0., |animation| animation.sample(now).1);
    let start = current;
    let target = start + delta;
    if (target - start).abs() <= f32::EPSILON
        || previous.is_some_and(|animation| (animation.target - target).abs() <= f32::EPSILON)
    {
        return previous;
    }
    Some(KeyboardScrollAnimation::new(start, target, velocity, now))
}

fn shifted_scroll_anchor(
    previous: gpui::ListOffset,
    previous_count: usize,
    header_shown: bool,
    added_top: usize,
    removed_top: usize,
) -> Option<gpui::ListOffset> {
    if previous.item_ix >= previous_count {
        return None;
    }
    let header = usize::from(header_shown);
    if previous.item_ix < header {
        return Some(if added_top > 0 {
            gpui::ListOffset {
                item_ix: header + added_top,
                offset_in_item: px(0.),
            }
        } else {
            previous
        });
    }
    let message_ix = previous.item_ix - header;
    if message_ix < removed_top {
        return Some(gpui::ListOffset {
            item_ix: header + added_top,
            offset_in_item: px(0.),
        });
    }
    Some(gpui::ListOffset {
        item_ix: header + added_top + message_ix - removed_top,
        offset_in_item: previous.offset_in_item,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SavedScrollAnchor {
    message_id: MessageId,
    offset_in_item: Pixels,
}

fn saved_message_scroll_anchor(
    anchor: SavedScrollAnchor,
    message_ix: usize,
    header_shown: bool,
) -> gpui::ListOffset {
    gpui::ListOffset {
        item_ix: usize::from(header_shown) + message_ix,
        offset_in_item: anchor.offset_in_item,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaginationDirection {
    Top,
    Bottom,
}

fn pagination_proximity(
    visible_start: usize,
    visible_end: usize,
    item_count: usize,
    header_shown: bool,
) -> (bool, bool) {
    let messages_before = visible_start.saturating_sub(usize::from(header_shown));
    let messages_after = item_count.saturating_sub(visible_end);
    (
        messages_before <= LOAD_MORE_ITEM_THRESHOLD && visible_end < item_count,
        messages_after <= LOAD_MORE_ITEM_THRESHOLD && visible_start > 0,
    )
}

fn pagination_direction(
    near_top: bool,
    near_bottom: bool,
    has_more_top: bool,
    has_more_bottom: bool,
    armed_top: bool,
    armed_bottom: bool,
) -> Option<PaginationDirection> {
    if near_top && has_more_top && armed_top {
        Some(PaginationDirection::Top)
    } else if near_bottom && has_more_bottom && armed_bottom {
        Some(PaginationDirection::Bottom)
    } else {
        None
    }
}

fn deadline_remaining(last_activity: Instant, now: Instant, delay: Duration) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last_activity);
    if elapsed < delay {
        Some(delay - elapsed)
    } else {
        None
    }
}

impl KeyboardScrollAnimation {
    fn new(start: f32, target: f32, initial_velocity: f32, started_at: Instant) -> Self {
        let delta = target - start;
        let duration = KEY_SCROLL_DURATION.as_secs_f32();
        let initial_slope = if delta.abs() <= f32::EPSILON {
            0.
        } else {
            let slope = initial_velocity * duration / delta;
            if slope.is_finite() && slope > 0. {
                // Keeping y1 in [0, 1] makes the retargeted curve monotonic.
                // Opposite-direction velocity is intentionally discarded so a
                // quick direction reversal remains responsive.
                slope.min(1. / KEY_SCROLL_BEZIER_X1)
            } else {
                0.
            }
        };
        Self {
            start,
            target,
            started_at,
            initial_slope,
            applied: start,
        }
    }

    fn sample(self, now: Instant) -> (f32, f32, bool) {
        let duration = KEY_SCROLL_DURATION.as_secs_f32();
        let elapsed = now.saturating_duration_since(self.started_at).as_secs_f32();
        if elapsed >= duration {
            return (self.target, 0., true);
        }

        let progress = (elapsed / duration).clamp(0., 1.);
        let parameter = solve_keyboard_scroll_bezier(progress);
        let y1 = KEY_SCROLL_BEZIER_X1 * self.initial_slope;
        let eased = cubic_bezier_coordinate(parameter, y1, 1.);
        let delta = self.target - self.start;
        let position = self.start + delta * eased;

        let dx = cubic_bezier_derivative(parameter, KEY_SCROLL_BEZIER_X1, KEY_SCROLL_BEZIER_X2);
        let dy = cubic_bezier_derivative(parameter, y1, 1.);
        let velocity = if dx.abs() <= f32::EPSILON {
            0.
        } else {
            delta * (dy / dx) / duration
        };
        (position, velocity, false)
    }
}

fn cubic_bezier_coordinate(t: f32, p1: f32, p2: f32) -> f32 {
    let inverse = 1. - t;
    3. * inverse * inverse * t * p1 + 3. * inverse * t * t * p2 + t * t * t
}

fn cubic_bezier_derivative(t: f32, p1: f32, p2: f32) -> f32 {
    let inverse = 1. - t;
    3. * inverse * inverse * p1 + 6. * inverse * t * (p2 - p1) + 3. * t * t * (1. - p2)
}

fn solve_keyboard_scroll_bezier(progress: f32) -> f32 {
    if progress <= 0. || progress >= 1. {
        return progress.clamp(0., 1.);
    }

    // The x control points are monotonic, so a small fixed bisection is
    // deterministic and accurate enough for sub-pixel scrolling.
    let mut lower = 0.;
    let mut upper = 1.;
    for _ in 0..12 {
        let parameter = (lower + upper) * 0.5;
        let x = cubic_bezier_coordinate(parameter, KEY_SCROLL_BEZIER_X1, KEY_SCROLL_BEZIER_X2);
        if x < progress {
            lower = parameter;
        } else {
            upper = parameter;
        }
    }
    (lower + upper) * 0.5
}

#[cfg(test)]
mod selection_hit_tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use gpui::{Bounds, point, size};

    use super::*;

    #[test]
    fn snap_past_text_includes_trailing_unmeasured_emoji() {
        let word_bounds = Rc::new(Cell::new(Some(Bounds::new(
            point(px(10.), px(20.)),
            size(px(40.), px(24.)),
        ))));
        let emoji_bounds = Rc::new(Cell::new(None));
        let segments = [
            TextSegment::bounded(0..4, word_bounds),
            TextSegment::bounded(4..11, emoji_bounds),
        ];

        assert_eq!(
            segment_offset_snap(&segments, point(px(60.), px(30.))),
            Some(11)
        );
    }

    #[test]
    fn expanded_selection_survives_mouse_up_inside_its_range() {
        let message_id = MessageId::new(42);
        let expanded = ExpandedSelection {
            message_id,
            start: 4,
            end: 9,
            origin: point(px(10.), px(10.)),
        };
        let mut state = MessageSelectionState::default();
        state.anchor = Some(SelPoint {
            message_id,
            offset: expanded.start,
        });
        state.head = Some(SelPoint {
            message_id,
            offset: expanded.end,
        });

        assert!(!update_selection_head(
            &mut state,
            SelPoint {
                message_id,
                offset: 6,
            },
            Some(expanded),
        ));
        assert_eq!(state.anchor.unwrap().offset, 4);
        assert_eq!(state.head.unwrap().offset, 9);
    }

    #[test]
    fn dragging_an_expanded_selection_keeps_the_original_word_selected() {
        let message_id = MessageId::new(42);
        let expanded = ExpandedSelection {
            message_id,
            start: 4,
            end: 9,
            origin: point(px(10.), px(10.)),
        };
        let mut state = MessageSelectionState::default();

        assert!(update_selection_head(
            &mut state,
            SelPoint {
                message_id,
                offset: 1,
            },
            Some(expanded),
        ));
        assert_eq!(state.anchor.unwrap().offset, 9);
        assert_eq!(state.head.unwrap().offset, 1);

        assert!(update_selection_head(
            &mut state,
            SelPoint {
                message_id,
                offset: 12,
            },
            Some(expanded),
        ));
        assert_eq!(state.anchor.unwrap().offset, 4);
        assert_eq!(state.head.unwrap().offset, 12);
    }

    #[test]
    fn dragging_expanded_selection_across_messages_uses_message_order() {
        let earlier = MessageId::new(10);
        let selected = MessageId::new(20);
        let expanded = ExpandedSelection {
            message_id: selected,
            start: 4,
            end: 9,
            origin: point(px(10.), px(10.)),
        };
        let mut state = MessageSelectionState::default();
        state.order_map = HashMap::from([(earlier, 0), (selected, 1)]);

        assert!(update_selection_head(
            &mut state,
            SelPoint {
                message_id: earlier,
                offset: 2,
            },
            Some(expanded),
        ));
        assert_eq!(state.anchor.unwrap().offset, 9);
        assert_eq!(state.head.unwrap().message_id, earlier);
    }

    #[test]
    fn expanded_selection_ignores_pointer_jitter_until_drag_threshold() {
        let expanded = ExpandedSelection {
            message_id: MessageId::new(42),
            start: 4,
            end: 9,
            origin: point(px(10.), px(10.)),
        };

        assert!(!expanded.drag_started(point(px(12.), px(8.))));
        assert!(expanded.drag_started(point(px(12.1), px(10.))));
    }
}

#[cfg(test)]
mod keyboard_scroll_animation_tests {
    use super::*;

    #[test]
    fn curve_is_time_based_and_finishes_at_constant_duration() {
        let started_at = Instant::now();
        let animation = KeyboardScrollAnimation::new(0., 48., 0., started_at);

        let (midpoint, midpoint_velocity, midpoint_finished) =
            animation.sample(started_at + KEY_SCROLL_DURATION / 2);
        assert!((midpoint - 24.).abs() < 0.05);
        assert!(midpoint_velocity > 0.);
        assert!(!midpoint_finished);

        let (end, end_velocity, end_finished) = animation.sample(started_at + KEY_SCROLL_DURATION);
        assert_eq!(end, 48.);
        assert_eq!(end_velocity, 0.);
        assert!(end_finished);
    }

    #[test]
    fn same_direction_retarget_preserves_position_and_velocity() {
        let started_at = Instant::now();
        let animation = KeyboardScrollAnimation::new(0., 48., 0., started_at);
        let retargeted_at = started_at + Duration::from_millis(60);
        let (position, velocity, _) = animation.sample(retargeted_at);

        let retargeted = KeyboardScrollAnimation::new(position, 96., velocity, retargeted_at);
        let (new_position, new_velocity, finished) = retargeted.sample(retargeted_at);

        assert!((new_position - position).abs() < 0.01);
        assert!((new_velocity - velocity).abs() < 0.1);
        assert!(!finished);
    }

    #[test]
    fn direction_reversal_does_not_continue_with_stale_velocity() {
        let started_at = Instant::now();
        let animation = KeyboardScrollAnimation::new(0., 48., 0., started_at);
        let retargeted_at = started_at + Duration::from_millis(60);
        let (position, velocity, _) = animation.sample(retargeted_at);
        assert!(velocity > 0.);

        let reversed = KeyboardScrollAnimation::new(position, -48., velocity, retargeted_at);
        let (_, reversed_velocity, _) = reversed.sample(retargeted_at);
        assert_eq!(reversed_velocity, 0.);
    }

    #[test]
    fn same_frame_key_repeat_does_not_queue_another_step() {
        let started_at = Instant::now();
        let animation = KeyboardScrollAnimation::new(-200., -152., 0., started_at);
        let retargeted = retarget_keyboard_scroll(-200., Some(animation), 48., started_at);

        assert_eq!(retargeted, Some(animation));
    }

    #[test]
    fn held_key_retargets_from_visible_position() {
        let started_at = Instant::now();
        let mut animation = KeyboardScrollAnimation::new(-200., -152., 0., started_at);
        let now = started_at + Duration::from_millis(60);
        animation.applied = -188.;
        let retargeted = retarget_keyboard_scroll(-188., Some(animation), 48., now).unwrap();

        assert_eq!(retargeted.start, -188.);
        assert_eq!(retargeted.applied, -188.);
        assert_eq!(retargeted.target, -140.);
        assert!(retargeted.target < animation.target + 48.);
    }

    #[test]
    fn retarget_does_not_queue_unapplied_progress() {
        let started_at = Instant::now();
        let animation = KeyboardScrollAnimation::new(-200., -152., 0., started_at);
        let retargeted = retarget_keyboard_scroll(
            -200.,
            Some(animation),
            48.,
            started_at + KEY_SCROLL_DURATION,
        )
        .unwrap();

        assert_eq!(retargeted, animation);
        assert_eq!(retargeted.target - retargeted.applied, 48.);
    }

    #[test]
    fn target_stays_one_step_ahead_of_live_offset() {
        let started_at = Instant::now();
        let fresh = retarget_keyboard_scroll(-10., None, 48., started_at).unwrap();
        assert_eq!(fresh.target, 38.);
        assert_eq!(fresh.applied, -10.);

        let held =
            retarget_keyboard_scroll(-10., Some(fresh), 48., started_at + KEY_SCROLL_DURATION)
                .unwrap();
        assert_eq!(held, fresh);
        assert_eq!(held.target - held.applied, 48.);
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    #[test]
    fn top_threshold_excludes_the_fixed_skeleton_row() {
        assert_eq!(pagination_proximity(13, 21, 100, true), (true, false));
        assert_eq!(pagination_proximity(14, 22, 100, true), (false, false));
    }

    #[test]
    fn bottom_threshold_uses_remaining_items() {
        assert_eq!(pagination_proximity(70, 88, 100, false), (false, true));
        assert_eq!(pagination_proximity(69, 87, 100, false), (false, false));
    }

    #[test]
    fn short_list_prioritizes_older_history() {
        assert_eq!(
            pagination_direction(true, true, true, true, true, true),
            Some(PaginationDirection::Top)
        );
    }

    #[test]
    fn bottom_loads_when_top_has_no_more_history() {
        assert_eq!(
            pagination_direction(true, true, false, true, true, true),
            Some(PaginationDirection::Bottom)
        );
    }

    #[test]
    fn disarmed_edge_does_not_repeat() {
        assert_eq!(
            pagination_direction(true, false, true, false, false, true),
            None
        );
    }
}

#[cfg(test)]
mod scroll_idle_tests {
    use super::*;

    #[test]
    fn memory_relief_waits_for_the_latest_scroll_activity() {
        let started_at = Instant::now();
        let before_idle = started_at + SCROLL_RELIEF_DELAY - Duration::from_millis(1);
        assert_eq!(
            deadline_remaining(started_at, before_idle, SCROLL_RELIEF_DELAY),
            Some(Duration::from_millis(1))
        );
        assert_eq!(
            deadline_remaining(
                started_at,
                started_at + SCROLL_RELIEF_DELAY,
                SCROLL_RELIEF_DELAY
            ),
            None
        );
        assert_eq!(
            deadline_remaining(
                started_at,
                started_at + SCROLL_RELIEF_DELAY + Duration::from_secs(1),
                SCROLL_RELIEF_DELAY
            ),
            None
        );

        let latest_activity = started_at + Duration::from_millis(900);
        assert_eq!(
            deadline_remaining(
                latest_activity,
                started_at + SCROLL_RELIEF_DELAY,
                SCROLL_RELIEF_DELAY
            ),
            Some(Duration::from_millis(900))
        );
    }
}

pub struct ChannelMessages {
    pub(crate) list_state: ListState,
    focus_handle: FocusHandle,
    selection: SharedSelection,
    selection_pointer: Option<Point<Pixels>>,
    expanded_selection: Option<ExpandedSelection>,
    selection_autoscroll_scheduled: bool,
    keyboard_scroll: Option<KeyboardScrollAnimation>,
    settings: Entity<Settings>,
    image_cache: Entity<LruImageCache>,
    avatar_image_cache: Entity<LruImageCache>,
    small_avatar_image_cache: Entity<LruImageCache>,
    icon_image_cache: Entity<LruImageCache>,
    active_videos: Rc<HashMap<(MessageId, usize), Entity<VideoPlayerView>>>,
    active_audios: Rc<indexmap::IndexMap<(MessageId, usize), Entity<AudioPlayerView>>>,
    gif_videos: Rc<HashMap<(MessageId, usize), Entity<GifVideoView>>>,
    embed_inputs: Rc<HashMap<(MessageId, SharedString), Entity<TextArea>>>,
    embed_input_subs: HashMap<(MessageId, SharedString), Subscription>,
    embed_input_fingerprint: Option<(Option<ChannelId>, usize)>,
    embed_select_seeded: HashSet<(MessageId, SharedString)>,
    cached_for_channel: Option<ChannelId>,
    skeleton_phase: SkeletonPhase,
    skeleton_key: SkeletonKey,
    last_cold_inputs: (Option<ClanId>, bool, bool),
    _channel_list_observe: Subscription,
    _clan_list_observe: Subscription,
    _skeleton_timer: Option<Task<()>>,
    hovered_row: Option<MessageId>,
    raw_hover: Option<MessageId>,
    _hover_show_task: Option<Task<()>>,
    _hover_hide_task: Option<Task<()>>,
    scroll_relief_armed: bool,
    paginate_armed_top: bool,
    paginate_armed_bottom: bool,
    pagination_check_scheduled: bool,
    last_paginate_count: usize,
    last_paginate_edges: (
        Option<mezon_store::MessageId>,
        Option<mezon_store::MessageId>,
    ),
    last_scroll_at: Option<Instant>,
    at_bottom: bool,
    last_visible_start: usize,
    last_visible_end: usize,
    header_shown: bool,
    pending_jump: Option<MessageId>,
    highlight_id: Option<MessageId>,
    _highlight_timer: Option<Task<()>>,
    last_seen_at_bottom: Option<MessageId>,
    fab_scroll_pending: bool,
    scroll_anchors: HashMap<ChannelId, SavedScrollAnchor>,
    last_scroll_sync: Option<(ChannelId, usize, u32, usize, u32, u32, bool)>,
    current_channel: Option<ChannelId>,
    restore_pending: Option<ChannelId>,
    welcome: Option<WelcomeContext>,
    onboarding: Option<OnboardingContext>,
    cached_unread_boundary: Option<MessageId>,
    cached_fab_unread_count: u32,
    _clan_members_observe: Subscription,
    _direct_messages_observe: Subscription,
    _users_by_user_observe: Subscription,
    _group_members_observe: Subscription,
    _window_activation: Option<Subscription>,
    mention_popover: Option<(Entity<UserProfilePopover>, Point<Pixels>)>,
    _mention_popover_sub: Option<Subscription>,
    reaction_picker: Option<(Entity<ReactionPicker>, Point<Pixels>)>,
    _reaction_picker_sub: Option<Subscription>,
    _reaction_picker_dismiss_sub: Option<Subscription>,
    cached_locale: SharedString,
    cached_coming_soon: SharedString,
    cached_current_user_id: SharedString,
    cached_role_ids: Rc<Vec<i64>>,
    cached_is_clan_owner: bool,
    identity_inputs: (Option<ClanId>, Option<UserId>),
    edit_input: Option<(MessageId, Entity<MentionInput>)>,
    _edit_input_sub: Option<Subscription>,
    context_menu_target: Option<(MessageId, Point<Pixels>)>,
    context_menu_forward_all: bool,
    reaction_submenu_open: bool,
    emoji_recent: Rc<Vec<Emoji>>,
    _emoji_observe: Subscription,
    channel_permissions_fp: Option<(bool, bool)>,
    _channel_permissions_observe: Subscription,
    gif_reconcile_fingerprint: Option<(Option<ChannelId>, usize, usize)>,
    last_gif_reconcile: Option<Instant>,
    last_image_cache_sweep: Option<Instant>,
    row_memo: Rc<RefCell<RowMemo>>,
    row_memo_day: Option<chrono::NaiveDate>,
    is_topic_box: bool,
    topic_align_timeline: Option<gpui::WeakEntity<ChannelMessages>>,
    topic_spacer_h: Option<Pixels>,
    topic_spacer_active: bool,
    topic_aligned: bool,
    topic_align_probe: bool,
    topic_list_topic: Option<i64>,
    topic_row_ids: Vec<MessageId>,
    topic_messages: Rc<Vec<Message>>,
    topics_viewport_fp: Option<u64>,
    permissions_ensured_for: Option<(ClanId, ChannelId)>,
    _topics_event_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl ChannelMessages {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let mut subs: Vec<Subscription> = Vec::new();
        subs.push(cx.observe(&settings, |this, settings, cx| {
            this.cached_locale = settings.read(cx).language.clone().into();
            this.cached_coming_soon =
                mezon_i18n::t(&this.cached_locale, "common.comingSoon").into();
            let mut memo = this.row_memo.borrow_mut();
            memo.time_labels.clear();
            memo.rich_text.clear();
            memo.selection_layouts.clear();
            memo.selection_text_pieces.clear();
            cx.notify();
        }));

        let channel_list = ChannelList::global(cx);
        let channel_list_observe = cx.observe(&channel_list, |this, _, cx| {
            this.row_memo.borrow_mut().selection_layouts.clear();
            this.reconcile_cold(cx);
        });

        let clan_list = ClanList::global(cx);
        let clan_list_observe = cx.observe(&clan_list, |this, _, cx| this.reconcile_cold(cx));

        let clan_members = ClanMembersStore::global(cx);
        let clan_members_observe = cx.observe(&clan_members, |this, _, cx| {
            if !MessagesStore::global(cx).read(cx).is_dm() {
                let mut memo = this.row_memo.borrow_mut();
                memo.avatars.clear();
                memo.display_names.clear();
            }
            this.store_identity(Self::compute_identity(cx));
            if this.refresh_derived_state(cx) {
                cx.notify();
            }
        });

        let direct_messages = DirectMessageStore::global(cx);
        let direct_messages_observe = cx.observe(&direct_messages, |this, _, cx| {
            if !MessagesStore::global(cx).read(cx).is_dm() {
                return;
            }
            {
                let mut memo = this.row_memo.borrow_mut();
                memo.avatars.clear();
                memo.display_names.clear();
            }
            if this.refresh_derived_state(cx) {
                cx.notify();
            }
        });

        let users_by_user = UsersByUserStore::global(cx);
        let users_by_user_observe = cx.observe(&users_by_user, |this, _, cx| {
            if MessagesStore::global(cx).read(cx).is_dm()
                && !this.row_memo.borrow().avatars.is_empty()
            {
                let mut memo = this.row_memo.borrow_mut();
                memo.avatars.clear();
                memo.display_names.clear();
                cx.notify();
            }
        });

        let group_members = GroupMembersStore::global(cx);
        let group_members_observe = cx.observe(&group_members, |this, _, cx| {
            if MessagesStore::global(cx).read(cx).is_dm()
                && !this.row_memo.borrow().avatars.is_empty()
            {
                let mut memo = this.row_memo.borrow_mut();
                memo.avatars.clear();
                memo.display_names.clear();
                cx.notify();
            }
        });

        let topics_event_sub = cx.subscribe(&TopicsStore::global(cx), |this, store, event, cx| {
            if this.is_topic_box || !matches!(event, TopicsEvent::Updated) {
                return;
            }
            let topics = store.read(cx);
            let messages = MessagesStore::global(cx).read(cx);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let mut any_topic = false;
            for msg in messages.viewport_messages() {
                let Some(topic_id) = msg.topic_id else {
                    continue;
                };
                any_topic = true;
                topic_id.hash(&mut hasher);
                if let Some(meta) = topics.topic_meta_for_topic(topic_id) {
                    meta.rpl.hash(&mut hasher);
                    meta.lsnt.hash(&mut hasher);
                }
            }
            if !any_topic {
                return;
            }
            let fp = hasher.finish();
            if this.topics_viewport_fp == Some(fp) {
                return;
            }
            this.topics_viewport_fp = Some(fp);
            cx.notify();
        });

        let store = MessagesStore::global(cx);
        subs.push(cx.subscribe(&store, |this, _store, event, cx| {
            if this.is_topic_box {
                this.on_topic_store_event(event, cx);
                return;
            }
            let structural = matches!(
                event,
                MessagesEvent::Reset { .. }
                    | MessagesEvent::Shifted { .. }
                    | MessagesEvent::RemovedAt { .. }
            );
            if structural {
                this.keyboard_scroll = None;
            }
            match event {
                MessagesEvent::Reset { count } => {
                    this.reaction_picker = None;
                    this._reaction_picker_sub = None;
                    this._reaction_picker_dismiss_sub = None;
                    this.mention_popover = None;
                    this._mention_popover_sub = None;
                    this.context_menu_target = None;
                    this.hovered_row = None;
                    this.raw_hover = None;
                    this._hover_show_task = None;
                    this._hover_hide_task = None;
                    this.edit_input = None;
                    this._edit_input_sub = None;
                    Rc::make_mut(&mut this.active_videos).clear();
                    Rc::make_mut(&mut this.active_audios).clear();
                    Rc::make_mut(&mut this.gif_videos).clear();
                    Rc::make_mut(&mut this.embed_inputs).clear();
                    this.embed_input_subs.clear();
                    this.embed_input_fingerprint = None;
                    this.embed_select_seeded.clear();
                    this.list_state.reset(*count);
                    this.last_visible_start = 0;
                    this.last_visible_end = 0;
                    this.header_shown = false;

                    let new_channel = _store.read(cx).active_channel_id();
                    if new_channel != this.current_channel {
                        this.last_seen_at_bottom = None;
                    }
                    let is_loading = _store.read(cx).is_loading();
                    let transition = reset_transition(
                        this.current_channel,
                        new_channel,
                        this.restore_pending,
                        this.fab_scroll_pending,
                        is_loading,
                    );
                    this.current_channel = transition.current_channel;
                    this.restore_pending = transition.restore_pending;
                    this.fab_scroll_pending = false;

                    let anchor = new_channel
                        .and_then(|channel_id| this.scroll_anchors.get(&channel_id).copied());
                    let decision = decide_reset_scroll(
                        transition.want_restore,
                        is_loading,
                        anchor,
                        _store.read(cx).viewport_messages(),
                        _store.read(cx).has_more_bottom(),
                    );
                    let at_bottom = match decision {
                        ResetScroll::Restore {
                            item_ix,
                            offset_in_item,
                        } => {
                            this.list_state.scroll_to(gpui::ListOffset {
                                item_ix,
                                offset_in_item,
                            });
                            false
                        }
                        ResetScroll::ToBottom => {
                            this.list_state.scroll_to_end();
                            true
                        }
                        ResetScroll::Defer => true,
                    };
                    this.at_bottom = at_bottom;

                    if let Some(channel_id) = new_channel {
                        _store.update(cx, |store, _cx| {
                            store.set_viewing_older(channel_id, !at_bottom);
                        });
                    }
                    if matches!(decision, ResetScroll::ToBottom) {
                        if let Some(last) = _store
                            .read(cx)
                            .viewport_messages()
                            .last()
                            .filter(|m| !m.id.is_optimistic())
                        {
                            this.last_seen_at_bottom = Some(last.id);
                        }
                        this.sync_channel_seen(cx);
                    }
                }
                MessagesEvent::Shifted {
                    added_top,
                    removed_top,
                    added_bottom,
                    removed_bottom,
                } => {
                    let prev_top = this.list_state.logical_scroll_top();
                    let prev_count = this.list_state.item_count();
                    let preserved_message_anchor = {
                        let store = _store.read(cx);
                        store
                            .active_channel_id()
                            .and_then(|channel_id| this.scroll_anchors.get(&channel_id).copied())
                            .and_then(|anchor| {
                                store
                                    .viewport_position(anchor.message_id)
                                    .map(|message_ix| {
                                        saved_message_scroll_anchor(
                                            anchor,
                                            message_ix,
                                            this.header_shown,
                                        )
                                    })
                            })
                    };
                    let was_at_end = prev_top.item_ix >= prev_count;
                    let own_recent_send = *added_bottom > 0
                        && _store.read(cx).viewport_messages().last().is_some_and(|m| {
                            m.id.is_optimistic()
                                || (!this.cached_current_user_id.is_empty()
                                    && m.sender_id.as_str() == this.cached_current_user_id.as_ref()
                                    && chrono::Utc::now().timestamp() - m.create_time <= 1)
                        });
                    let h = usize::from(this.header_shown);
                    let inserted_size_hint = this.list_state.average_measured_item_size();
                    if *removed_top > 0 {
                        this.list_state.splice(h..h + *removed_top, 0);
                    }
                    if *added_top > 0 {
                        if let Some(size_hint) = inserted_size_hint {
                            this.list_state
                                .splice_with_size_hint(h..h, *added_top, size_hint);
                        } else {
                            this.list_state.splice(h..h, *added_top);
                        }
                    }
                    if *removed_bottom > 0 {
                        let n = this.list_state.item_count();
                        this.list_state
                            .splice(n.saturating_sub(*removed_bottom)..n, 0);
                    }
                    if *added_bottom > 0 {
                        let n = this.list_state.item_count();
                        if let Some(size_hint) = inserted_size_hint {
                            this.list_state
                                .splice_with_size_hint(n..n, *added_bottom, size_hint);
                        } else {
                            this.list_state.splice(n..n, *added_bottom);
                        }
                    }
                    let following_new = *added_bottom > 0
                        && (was_at_end || own_recent_send)
                        && !_store.read(cx).has_more_bottom();
                    if following_new {
                        this.list_state.scroll_to_end();
                        this.at_bottom = true;
                        if let Some(last) = _store.read(cx).viewport_messages().last() {
                            this.last_seen_at_bottom = Some(last.id);
                        }
                        this.sync_channel_seen(cx);
                    } else if *added_top > 0 || *removed_top > 0 {
                        let anchor = preserved_message_anchor.or_else(|| {
                            shifted_scroll_anchor(
                                prev_top,
                                prev_count,
                                this.header_shown,
                                *added_top,
                                *removed_top,
                            )
                        });
                        if let Some(anchor) = anchor {
                            this.list_state.scroll_to(anchor);
                        }
                    } else if *added_bottom > 0 && prev_top.item_ix < prev_count {
                        this.list_state.scroll_to(prev_top);
                    }
                }
                MessagesEvent::Updated { message_id } => {
                    let Some(id) = message_id else {
                        this.refresh_derived_state(cx);
                        cx.notify();
                        return;
                    };
                    let vp_index = _store.read(cx).viewport_position(*id);
                    if let Some(vp_index) = vp_index {
                        let at = usize::from(this.header_shown) + vp_index;
                        if at < this.list_state.item_count() {
                            this.list_state.remeasure_items(at..at + 1);
                        }
                        this.refresh_derived_state(cx);
                        cx.notify();
                    }
                    return;
                }
                MessagesEvent::RemovedAt { index, message_id } => {
                    Rc::make_mut(&mut this.active_videos).retain(|(id, _), _| id != message_id);
                    Rc::make_mut(&mut this.active_audios).retain(|(id, _), _| id != message_id);
                    Rc::make_mut(&mut this.gif_videos).retain(|(id, _), _| id != message_id);
                    Rc::make_mut(&mut this.embed_inputs).retain(|(id, _), _| id != message_id);
                    this.embed_input_subs.retain(|(id, _), _| id != message_id);
                    this.embed_select_seeded.retain(|(id, _)| id != message_id);
                    this.embed_input_fingerprint = None;
                    let at = usize::from(this.header_shown) + *index;
                    if at < this.list_state.item_count() {
                        this.list_state.splice(at..at + 1, 0);
                        if at < this.list_state.item_count() {
                            this.list_state.remeasure_items(at..at + 1);
                        }
                    }
                }
                MessagesEvent::JumpTo { message_id } => {
                    this.pending_jump = Some(*message_id);
                    this.highlight_id = Some(*message_id);
                    this._highlight_timer = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(1500))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.highlight_id = None;
                            cx.notify();
                        });
                    }));
                }
                MessagesEvent::ReplyTargetChanged
                | MessagesEvent::ForwardProgress { .. }
                | MessagesEvent::ForwardFinished { .. }
                | MessagesEvent::ShareContactFinished { .. }
                | MessagesEvent::AnonymousModeChanged => return,
                MessagesEvent::TopicUpdated { .. } => {}
            }
            if structural {
                {
                    let (is_empty, has_more_top, is_dm, dm_channel, conversation_loading) = {
                        let s = _store.read(cx);
                        Self::read_store_inputs(s)
                    };
                    this.sync_header(is_empty, has_more_top);
                    this.sync_skeleton(is_dm, dm_channel, conversation_loading, is_empty, cx);
                }
                this.refresh_derived_state(cx);
            }
            cx.notify();
        }));

        let list_state = ListState::new(0, ListAlignment::Bottom, px(LIST_OVERDRAW))
            .smooth_line_scroll()
            .suppress_hover_while_scrolling();
        let timeline = cx.weak_entity();
        list_state.set_scroll_handler(move |event, window, cx| {
            let at_bottom = !event.is_scrolled;
            let scroll_top = event.scroll_top;
            let visible_start = event.visible_range.start;
            let visible_end = event.visible_range.end;
            let _ = timeline.update(cx, |this, cx| {
                // Direct wheel/trackpad input must take ownership from an in-flight
                // keyboard scroll animation. Otherwise the next RAF moves the list
                // back toward the stale keyboard target and makes wheel deltas feel
                // shorter than they actually are.
                this.keyboard_scroll = None;
                let range_moved = this.last_visible_start != visible_start;
                let visible_range_changed = range_moved || this.last_visible_end != visible_end;
                let at_bottom_changed = this.at_bottom != at_bottom;
                this.at_bottom = at_bottom;
                this.last_visible_start = visible_start;
                this.last_visible_end = visible_end;

                if range_moved && this.context_menu_target.take().is_some() {
                    cx.notify();
                }

                this.mark_scroll_activity(cx);

                if this.is_topic_box {
                    return;
                }

                let store_entity = MessagesStore::global(cx);
                let live_anchor = {
                    let store = store_entity.read(cx);
                    store.active_channel_id().map(|channel_id| {
                        (
                            channel_id,
                            capture_anchor(
                                store.viewport_messages(),
                                at_bottom,
                                scroll_top,
                                this.header_shown,
                            ),
                        )
                    })
                };
                if let Some((channel_id, anchor_update)) = live_anchor {
                    this.apply_scroll_anchor(channel_id, anchor_update);
                }
                if at_bottom_changed {
                    if let Some(channel_id) = store_entity.read(cx).active_channel_id() {
                        store_entity.update(cx, |store, _cx| {
                            store.set_viewing_older(channel_id, !at_bottom);
                        });
                    }
                    if at_bottom && !store_entity.read(cx).has_more_bottom() {
                        this.sync_channel_seen(cx);
                    }
                }

                if visible_range_changed {
                    this.schedule_pagination_check(window, cx);
                }
                if this.hovered_row.is_some()
                    || this.raw_hover.is_some()
                    || this._hover_show_task.is_some()
                    || this._hover_hide_task.is_some()
                {
                    this.hovered_row = None;
                    this.raw_hover = None;
                    this._hover_show_task = None;
                    this._hover_hide_task = None;
                }
            });
        });

        let image_cache = cx.new(|cx| {
            LruImageCache::message(
                "msg-image",
                MESSAGE_IMAGE_CACHE_CAPACITY,
                MESSAGE_IMAGE_CACHE_BYTES,
                MESSAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let avatar_image_cache = crate::image_cache::shared_avatar_cache(cx);
        let icon_image_cache = cx.new(|cx| {
            crate::image_cache::LruImageCache::icon_thumbnail(
                "message-icons",
                1024,
                8 * 1024 * 1024,
                256 * 1024,
                cx,
            )
        });
        let small_avatar_image_cache = cx.new(|cx| {
            crate::image_cache::LruImageCache::avatar_thumbnail_small(
                "message-authors",
                512,
                16 * 1024 * 1024,
                4 * 1024 * 1024,
                cx,
            )
        });
        let last_cold_inputs = Self::cold_inputs(cx);
        let (welcome, onboarding) = Self::compute_indicator_contexts(cx);
        let cached_unread_boundary = unread_boundary(&MessagesStore::global(cx), None, cx);
        let cached_locale: SharedString = settings.read(cx).language.clone().into();
        let cached_coming_soon = mezon_i18n::t(&cached_locale, "common.comingSoon").into();
        let (identity_inputs, cached_current_user_id, cached_role_ids, cached_is_clan_owner) =
            Self::compute_identity(cx);
        let emoji_store = EmojiStore::global(cx);
        let emoji_recent: Rc<Vec<Emoji>> = Rc::new(
            emoji_store
                .read(cx)
                .recent(3)
                .into_iter()
                .cloned()
                .collect(),
        );
        let emoji_observe = cx.observe(&emoji_store, |this, store, cx| {
            let next: Vec<Emoji> = store.read(cx).recent(3).into_iter().cloned().collect();
            let changed = this.emoji_recent.len() != next.len()
                || this
                    .emoji_recent
                    .iter()
                    .zip(&next)
                    .any(|(a, b)| a.id != b.id);
            if changed {
                this.emoji_recent = Rc::new(next);
                cx.notify();
            }
        });
        let channel_permissions_observe = cx.subscribe(
            &ChannelPermissionsStore::global(cx),
            |this, store, event, cx| {
                let ChannelPermissionsEvent::Changed {
                    clan_id,
                    channel_id,
                } = event;
                let messages = MessagesStore::global(cx).read(cx);
                if messages.active_channel_id() != Some(*channel_id)
                    || messages.active_clan_id() != Some(*clan_id)
                {
                    return;
                }
                let store = store.read(cx);
                let fp = (
                    store.has_permission("send-message", *clan_id, *channel_id),
                    store.has_permission(PERMISSION_DELETE_MESSAGE, *clan_id, *channel_id),
                );
                if this.channel_permissions_fp == Some(fp) {
                    return;
                }
                this.channel_permissions_fp = Some(fp);
                cx.notify();
            },
        );
        Self {
            list_state,
            focus_handle: cx.focus_handle(),
            selection: MessageSelectionState::new_shared(),
            selection_pointer: None,
            expanded_selection: None,
            selection_autoscroll_scheduled: false,
            keyboard_scroll: None,
            settings,
            image_cache,
            avatar_image_cache,
            small_avatar_image_cache,
            icon_image_cache,
            active_videos: Rc::new(HashMap::new()),
            active_audios: Rc::new(indexmap::IndexMap::new()),
            gif_videos: Rc::new(HashMap::new()),
            embed_inputs: Rc::new(HashMap::new()),
            embed_input_subs: HashMap::new(),
            embed_input_fingerprint: None,
            embed_select_seeded: HashSet::new(),
            cached_for_channel: None,
            skeleton_phase: SkeletonPhase::Hidden,
            skeleton_key: SkeletonKey::None,
            last_cold_inputs,
            _channel_list_observe: channel_list_observe,
            _clan_list_observe: clan_list_observe,
            _skeleton_timer: None,
            hovered_row: None,
            raw_hover: None,
            _hover_show_task: None,
            _hover_hide_task: None,
            scroll_relief_armed: false,
            paginate_armed_top: true,
            paginate_armed_bottom: true,
            pagination_check_scheduled: false,
            last_paginate_count: 0,
            last_paginate_edges: (None, None),
            last_scroll_at: None,
            at_bottom: true,
            last_visible_start: 0,
            last_visible_end: 0,
            header_shown: false,
            pending_jump: None,
            highlight_id: None,
            _highlight_timer: None,
            last_seen_at_bottom: None,
            fab_scroll_pending: false,
            scroll_anchors: HashMap::new(),
            last_scroll_sync: None,
            current_channel: None,
            restore_pending: None,
            welcome,
            onboarding,
            cached_unread_boundary,
            cached_fab_unread_count: 0,
            _clan_members_observe: clan_members_observe,
            _direct_messages_observe: direct_messages_observe,
            _users_by_user_observe: users_by_user_observe,
            _group_members_observe: group_members_observe,
            _window_activation: None,
            mention_popover: None,
            _mention_popover_sub: None,
            reaction_picker: None,
            _reaction_picker_sub: None,
            _reaction_picker_dismiss_sub: None,
            cached_locale,
            cached_coming_soon,
            cached_current_user_id,
            cached_role_ids,
            cached_is_clan_owner,
            identity_inputs,
            edit_input: None,
            _edit_input_sub: None,
            context_menu_target: None,
            context_menu_forward_all: false,
            reaction_submenu_open: false,
            emoji_recent,
            _emoji_observe: emoji_observe,
            channel_permissions_fp: None,
            _channel_permissions_observe: channel_permissions_observe,
            gif_reconcile_fingerprint: None,
            last_gif_reconcile: None,
            last_image_cache_sweep: None,
            row_memo: Rc::new(RefCell::new(RowMemo::default())),
            row_memo_day: None,
            is_topic_box: false,
            topic_align_timeline: None,
            topic_spacer_h: None,
            topic_spacer_active: false,
            topic_aligned: false,
            topic_align_probe: false,
            topic_list_topic: None,
            topic_row_ids: Vec::new(),
            topic_messages: Rc::new(Vec::new()),
            topics_viewport_fp: None,
            permissions_ensured_for: None,
            _topics_event_sub: Some(topics_event_sub),
            _subs: subs,
        }
    }

    pub fn new_topic_box(
        settings: Entity<Settings>,
        align_timeline: Entity<ChannelMessages>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(settings, cx);
        this.is_topic_box = true;
        this.topic_align_timeline = Some(align_timeline.downgrade());
        this.topic_spacer_h = None;
        this._subs.push(
            cx.subscribe(&TopicsStore::global(cx), |this, _, event, cx| match event {
                TopicsEvent::Opened => {
                    this.refresh_topic_messages(cx);
                    cx.notify();
                }
                TopicsEvent::ReplyTargetChanged => cx.notify(),
                _ => {}
            }),
        );
        this.refresh_topic_messages(cx);
        this
    }

    pub fn message_viewport_top(&self, message_id: MessageId, cx: &App) -> Option<Pixels> {
        let msg_ix = MessagesStore::global(cx)
            .read(cx)
            .viewport_position(message_id)?;
        let list_ix = usize::from(self.header_shown) + msg_ix;
        self.list_state.bounds_for_item(list_ix).map(|b| b.top())
    }

    fn collect_topic_messages(cx: &App) -> Vec<Message> {
        let topics = TopicsStore::global(cx).read(cx);
        let store = MessagesStore::global(cx).read(cx);

        let origin_id = topics.origin_message().map(|m| m.id).or_else(|| {
            topics.active_topic_id().and_then(|topic_id| {
                topics
                    .topic_by_id(&topic_id.to_string())
                    .and_then(|topic| topic.message_id.parse::<i64>().ok())
                    .map(MessageId)
            })
        });

        let origin = origin_id.and_then(|id| {
            store
                .viewport_message_by_id(id)
                .cloned()
                .or_else(|| topics.origin_message().cloned())
        });

        let mut messages = Vec::new();
        if let Some(mut origin) = origin {
            origin.combined_with_prev = false;
            messages.push(origin);
        }
        if let Some(topic_id) = topics.active_topic_id() {
            for msg in store.messages_in_channel(ChannelId(topic_id)) {
                if origin_id != Some(msg.id) {
                    messages.push(msg.clone());
                }
            }
        }
        messages
    }

    fn refresh_topic_messages(&mut self, cx: &App) -> bool {
        if !self.is_topic_box {
            return false;
        }
        self.topic_messages = Rc::new(Self::collect_topic_messages(cx));
        self.sync_topic_rows(cx)
    }

    fn topic_rows_own_recent_send(&self) -> bool {
        self.topic_messages.last().is_some_and(|m| {
            m.id.is_optimistic()
                || (!self.cached_current_user_id.is_empty()
                    && m.sender_id.as_str() == self.cached_current_user_id.as_ref()
                    && chrono::Utc::now().timestamp() - m.create_time <= 1)
        })
    }

    fn reset_topic_rows(&mut self, topic_id: Option<i64>, ids: Vec<MessageId>) {
        self.topic_list_topic = topic_id;
        self.topic_row_ids = ids;
        self.topic_spacer_active = false;
        self.topic_spacer_h = None;
        self.topic_aligned = false;
        self.topic_align_probe = false;
        self.list_state.reset(self.topic_row_ids.len());
        self.list_state.scroll_to_end();
        self.at_bottom = true;
    }

    fn sync_topic_rows(&mut self, cx: &App) -> bool {
        let active_topic = TopicsStore::global(cx).read(cx).active_topic_id();
        let new_ids: Vec<MessageId> = self.topic_messages.iter().map(|m| m.id).collect();

        if active_topic != self.topic_list_topic {
            self.reset_topic_rows(active_topic, new_ids);
            return true;
        }
        if new_ids == self.topic_row_ids {
            return false;
        }

        let base = usize::from(self.topic_spacer_active);
        if self.list_state.item_count() != base + self.topic_row_ids.len() {
            self.reset_topic_rows(active_topic, new_ids);
            return true;
        }

        let old = &self.topic_row_ids;
        let prefix = old
            .iter()
            .zip(new_ids.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let max_suffix = old.len().min(new_ids.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(new_ids.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        let old_end = old.len() - suffix;
        let new_end = new_ids.len() - suffix;

        let appended_at_tail = prefix == old.len() && suffix == 0;
        let follow = appended_at_tail
            && (self
                .list_state
                .is_scrolled_to_end()
                .unwrap_or(self.at_bottom)
                || self.topic_rows_own_recent_send());

        self.list_state
            .splice(base + prefix..base + old_end, new_end - prefix);
        self.topic_row_ids = new_ids;

        if follow {
            self.list_state.scroll_to_end();
            self.at_bottom = true;
        }
        true
    }

    fn sync_topic_spacer(&mut self, active: bool, height: Pixels) {
        if active != self.topic_spacer_active {
            if active {
                self.list_state.splice(0..0, 1);
                self.topic_spacer_active = true;
                self.topic_spacer_h = Some(height);
                if !self.topic_aligned {
                    self.topic_aligned = true;
                    self.list_state.scroll_to(gpui::ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    });
                }
            } else {
                self.list_state.splice(0..1, 0);
                self.topic_spacer_active = false;
                self.topic_spacer_h = None;
            }
            return;
        }
        if active && self.topic_spacer_h != Some(height) {
            self.topic_spacer_h = Some(height);
            self.list_state.remeasure_items(0..1);
        }
    }

    fn remeasure_topic_rows(&self, range: std::ops::Range<usize>) {
        let base = usize::from(self.topic_spacer_active);
        let count = self.list_state.item_count();
        let start = (base + range.start).min(count);
        let end = (base + range.end).min(count);
        if start < end {
            self.list_state.remeasure_items(start..end);
        }
    }

    fn on_topic_store_event(&mut self, event: &MessagesEvent, cx: &mut Context<Self>) {
        let concerns_topic = match event {
            MessagesEvent::TopicUpdated { topic_id } => {
                TopicsStore::global(cx).read(cx).active_topic_id() == Some(*topic_id)
            }
            MessagesEvent::Updated {
                message_id: Some(id),
            }
            | MessagesEvent::RemovedAt { message_id: id, .. } => self.topic_row_ids.contains(id),
            _ => false,
        };
        if !concerns_topic {
            return;
        }

        let changed_row = match event {
            MessagesEvent::Updated {
                message_id: Some(id),
            } => self.topic_row_ids.iter().position(|row| row == id),
            _ => None,
        };

        let structural = self.refresh_topic_messages(cx);
        self.row_memo.borrow_mut().time_labels.clear();
        if !structural {
            match changed_row {
                Some(ix) => self.remeasure_topic_rows(ix..ix + 1),
                None => self.remeasure_topic_rows(0..self.topic_row_ids.len()),
            }
        }
        cx.notify();
    }

    fn ensure_topic_create_permissions(&mut self, cx: &mut Context<Self>) {
        let key = {
            let messages = MessagesStore::global(cx).read(cx);
            messages.active_clan_id().zip(messages.active_channel_id())
        };
        let Some(key) = key else {
            return;
        };
        if self.permissions_ensured_for == Some(key) {
            return;
        }
        TopicsStore::ensure_create_permissions(cx);
        let loaded = ChannelPermissionsStore::global(cx)
            .read(cx)
            .permission_value("send-message", key.0, key.1)
            .is_some();
        if loaded {
            self.permissions_ensured_for = Some(key);
        }
    }

    pub(crate) fn resolve_topic_forward_group(
        message_id: MessageId,
        sender_id: &str,
        cx: &App,
    ) -> Vec<MessageId> {
        let messages = Self::collect_topic_messages(cx);
        message_context_menu::resolve_forward_group_in(&messages, message_id, sender_id)
    }

    fn find_local_message(&self, message_id: MessageId, cx: &App) -> Option<Message> {
        if self.is_topic_box {
            self.topic_messages
                .iter()
                .find(|m| m.id == message_id)
                .cloned()
        } else {
            MessagesStore::global(cx)
                .read(cx)
                .viewport_message_by_id(message_id)
                .cloned()
        }
    }

    pub(crate) fn set_mention_popover(
        &mut self,
        popover: Entity<UserProfilePopover>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus_handle = popover.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self._mention_popover_sub =
            Some(cx.subscribe(&popover, |this, _, _: &DismissEvent, cx| {
                this.mention_popover = None;
                this._mention_popover_sub = None;
                cx.notify();
            }));
        self.mention_popover = Some((popover, position));
        cx.notify();
    }

    pub(crate) fn open_reaction_picker(
        &mut self,
        message_id: MessageId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|cx| ReactionPicker::new(window, cx));
        let focus_handle = picker.read(cx).focus_handle(cx);
        window.focus(&focus_handle, cx);
        self._reaction_picker_sub = Some(cx.subscribe(&picker, move |this, _picker, event, cx| {
            let ReactionPickerEvent::Picked { emoji_id, emoji } = event;
            MessagesStore::global(cx).update(cx, |store, cx| {
                store.add_reaction(message_id, emoji_id.clone(), emoji.clone(), cx);
            });
            this.reaction_picker = None;
            this._reaction_picker_sub = None;
            this._reaction_picker_dismiss_sub = None;
            cx.notify();
        }));
        self._reaction_picker_dismiss_sub = Some(cx.subscribe(
            &picker,
            |this, _picker, _: &DismissEvent, cx| {
                this.reaction_picker = None;
                this._reaction_picker_sub = None;
                this._reaction_picker_dismiss_sub = None;
                cx.notify();
            },
        ));
        self.reaction_picker = Some((picker, position));
        cx.notify();
    }

    /// Enter inline-edit mode for a message (Discord-style: in the row, not the composer).
    pub(crate) fn begin_edit(
        &mut self,
        message_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection.borrow_mut().clear();
        self.selection_pointer = None;
        self.expanded_selection = None;
        self.selection_autoscroll_scheduled = false;
        let (initial_content, initial_spans) = MessagesStore::global(cx)
            .read(cx)
            .viewport_messages()
            .iter()
            .find(|m| m.id == message_id)
            .map(|m| {
                let source =
                    markdown_edit_source(&m.content, &m.spans).unwrap_or_else(|| m.content.clone());
                (source, m.spans.clone())
            })
            .unwrap_or_else(|| {
                self.find_local_message(message_id, cx)
                    .map(|m| (m.content.clone(), m.spans.clone()))
                    .unwrap_or_default()
            });
        let settings = self.settings.clone();
        let input = cx.new(|cx| {
            MentionInput::new_edit(
                "Edit message…",
                settings,
                &initial_content,
                &initial_spans,
                window,
                cx,
            )
        });
        input.update(cx, |input, cx| input.focus_input(window, cx));
        self._edit_input_sub = Some(cx.subscribe_in(
            &input,
            window,
            move |this, _input, event: &MentionInputEvent, window, cx| match event {
                MentionInputEvent::Submit => this.save_edit(window, cx),
                MentionInputEvent::Cancel => this.cancel_edit(cx),
                MentionInputEvent::SendSticker { .. }
                | MentionInputEvent::SendGif { .. }
                | MentionInputEvent::SendSound { .. } => {}
            },
        ));
        self.edit_input = Some((message_id, input));
        MessagesStore::global(cx).update(cx, |store, cx| store.start_edit(message_id, cx));
        cx.notify();
    }

    pub(crate) fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.edit_input = None;
        self._edit_input_sub = None;
        MessagesStore::global(cx).update(cx, |store, cx| store.cancel_edit(cx));
        cx.notify();
    }

    pub(crate) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((message_id, input)) = self.edit_input.take() else {
            return;
        };
        self._edit_input_sub = None;
        let payload = input.update(cx, |input, cx| input.take_payload(window, cx));
        let store = MessagesStore::global(cx);

        let Some((text, content_tokens, _attachments, _ogp)) = payload else {
            store.update(cx, |store, cx| store.cancel_edit(cx));
            let locale = self.cached_locale.clone();
            Shell::global(cx).update(cx, |shell, cx| {
                shell.confirm_delete_message(message_id, &locale, window, cx);
            });
            cx.notify();
            return;
        };

        let original = self
            .find_local_message(message_id, cx)
            .map(|m| m.content.clone());

        if original.as_deref() == Some(text.as_str()) {
            store.update(cx, |store, cx| store.cancel_edit(cx));
            cx.notify();
            return;
        }

        store.update(cx, |store, cx| {
            store.edit_message(message_id, text, content_tokens, cx)
        });
        cx.notify();
    }

    pub(crate) fn open_context_menu(
        &mut self,
        message_id: MessageId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.is_topic_box {
            self.refresh_topic_messages(cx);
        }
        let sender_and_poll = self
            .find_local_message(message_id, cx)
            .map(|m| (m.sender_id.clone(), m.code == MessageCode::Poll));
        self.context_menu_forward_all = match sender_and_poll {
            Some((sender_id, false)) => {
                if self.is_topic_box {
                    message_context_menu::resolve_forward_group_in(
                        &self.topic_messages,
                        message_id,
                        sender_id.as_str(),
                    )
                    .len()
                        > 1
                } else {
                    message_context_menu::resolve_forward_group(message_id, sender_id.as_str(), cx)
                        .len()
                        > 1
                }
            }
            _ => false,
        };
        self.ensure_topic_create_permissions(cx);
        self._hover_show_task = None;
        self._hover_hide_task = None;
        self.hovered_row = None;
        self.raw_hover = None;
        self.reaction_submenu_open = false;
        self.context_menu_target = Some((message_id, position));
        cx.notify();
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu_target.take().is_some() {
            self.reaction_submenu_open = false;
            if self.raw_hover.is_none() {
                self.hovered_row = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn set_reaction_submenu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.reaction_submenu_open != open {
            self.reaction_submenu_open = open;
            cx.notify();
        }
    }

    pub(crate) fn set_row_hover(
        &mut self,
        message_id: MessageId,
        entered: bool,
        cx: &mut Context<Self>,
    ) {
        if entered {
            self.ensure_topic_create_permissions(cx);
            self.raw_hover = Some(message_id);
        } else if self.raw_hover == Some(message_id) {
            self.raw_hover = None;
        }
        let raw = self.raw_hover;

        match raw {
            Some(target) if self.hovered_row != Some(target) => {
                self._hover_show_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(HOVER_SHOW_DELAY_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.raw_hover == Some(target) && this.hovered_row != Some(target) {
                            this.hovered_row = Some(target);
                            cx.notify();
                        }
                    });
                }));
            }
            _ => self._hover_show_task = None,
        }

        match self.hovered_row {
            Some(shown) if raw != Some(shown) => {
                if self.context_menu_target.is_some_and(|(id, _)| id == shown) {
                    return;
                }
                self._hover_hide_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(HOVER_HIDE_DELAY_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.raw_hover != Some(shown) && this.hovered_row == Some(shown) {
                            this.hovered_row = None;
                            cx.notify();
                        }
                    });
                }));
            }
            _ => self._hover_hide_task = None,
        }
    }

    pub fn bind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self._window_activation.is_some() {
            return;
        }
        self._window_activation =
            Some(cx.observe_window_activation(window, Self::on_window_activation_changed));
    }

    fn on_window_activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            for view in self.gif_videos.values() {
                view.update(cx, |gif, cx| gif.set_playing(true, cx));
            }
            if self.is_topic_box {
                return;
            }
            if self
                .list_state
                .is_scrolled_to_end()
                .unwrap_or(self.at_bottom)
            {
                self.sync_channel_seen_when_focused(true, cx);
            }
        } else {
            for view in self.gif_videos.values() {
                view.update(cx, |gif, cx| gif.set_playing(false, cx));
            }
            for view in self.active_videos.values() {
                view.update(cx, |video, cx| video.pause_for_background(cx));
            }
        }
    }

    pub fn activate_video(
        &mut self,
        key: (MessageId, usize),
        activation: VideoActivation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_videos.contains_key(&key) {
            return;
        }
        let previous: Vec<_> = Rc::make_mut(&mut self.active_videos)
            .drain()
            .map(|(_, view)| view)
            .collect();
        for view in previous {
            view.update(cx, |video, cx| video.release_textures(window, cx));
        }
        let view = cx.new(|cx| VideoPlayerView::new(activation, window, cx));
        Rc::make_mut(&mut self.active_videos).insert(key, view);
        cx.notify();
    }

    pub fn activate_audio(
        &mut self,
        key: (MessageId, usize),
        activation: AudioActivation,
        cx: &mut Context<Self>,
    ) {
        if self.active_audios.contains_key(&key) {
            return;
        }
        while self.active_audios.len() >= MAX_AUDIO_PLAYERS {
            if Rc::make_mut(&mut self.active_audios)
                .shift_remove_index(0)
                .is_none()
            {
                break;
            }
        }
        let view = cx.new(|cx| AudioPlayerView::new(activation, cx));
        Rc::make_mut(&mut self.active_audios).insert(key, view);
        cx.notify();
    }

    fn compute_identity(
        cx: &gpui::App,
    ) -> (
        (Option<ClanId>, Option<UserId>),
        SharedString,
        Rc<Vec<i64>>,
        bool,
    ) {
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let current_user = BadgeService::global(cx).read(cx).current_user_id(cx);
        let current_user_id: SharedString = current_user
            .map(|u| u.0.to_string())
            .unwrap_or_default()
            .into();
        let role_ids: Vec<i64> = active_clan
            .and_then(|clan_id| {
                current_user.and_then(|uid| {
                    ClanMembersStore::global(cx)
                        .read(cx)
                        .member(clan_id, uid)
                        .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
                })
            })
            .unwrap_or_default();
        let is_clan_owner = ClanList::global(cx)
            .read(cx)
            .active_clan()
            .zip(current_user)
            .is_some_and(|(clan, uid)| clan.creator_id == uid);
        (
            (active_clan, current_user),
            current_user_id,
            Rc::new(role_ids),
            is_clan_owner,
        )
    }

    fn store_identity(
        &mut self,
        identity: (
            (Option<ClanId>, Option<UserId>),
            SharedString,
            Rc<Vec<i64>>,
            bool,
        ),
    ) {
        let (inputs, current_user_id, role_ids, is_clan_owner) = identity;
        self.identity_inputs = inputs;
        self.cached_current_user_id = current_user_id;
        self.cached_role_ids = role_ids;
        self.cached_is_clan_owner = is_clan_owner;
    }

    fn sync_render_identity(&mut self, cx: &gpui::App) {
        let key = (
            ClanList::global(cx).read(cx).active_clan_id,
            BadgeService::global(cx).read(cx).current_user_id(cx),
        );
        if key != self.identity_inputs {
            self.store_identity(Self::compute_identity(cx));
        }
    }

    fn apply_gif_reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const GIF_RECONCILE_INTERVAL: Duration = Duration::from_millis(250);
        let header = usize::from(self.header_shown);
        let count = self.list_state.item_count();
        let fingerprint = (
            self.cached_for_channel,
            self.list_state.logical_scroll_top().item_ix,
            count,
        );
        if self.gif_reconcile_fingerprint == Some(fingerprint)
            && self
                .last_gif_reconcile
                .is_some_and(|at| at.elapsed() < GIF_RECONCILE_INTERVAL)
        {
            return;
        }
        self.gif_reconcile_fingerprint = Some(fingerprint);
        self.last_gif_reconcile = Some(Instant::now());
        let start = self.list_state.logical_scroll_top().item_ix.max(header);
        let mut wanted: Vec<PendingGif> = Vec::new();
        {
            let store = MessagesStore::global(cx);
            let store = store.read(cx);
            let messages = store.viewport_messages();
            for list_ix in start..count {
                let below = self.list_state.item_is_below_viewport(list_ix);
                if below == Some(true) {
                    break;
                }
                if self.list_state.item_is_above_viewport(list_ix) != Some(false)
                    || below != Some(false)
                {
                    continue;
                }
                let Some(message) = messages.get(list_ix - header) else {
                    continue;
                };
                let image_count = message
                    .attachments
                    .iter()
                    .filter(|att| !att.is_unsupported_media() && !att.is_video() && att.is_image())
                    .count();
                if image_count >= 2 && message.album_layout.is_some() {
                    continue;
                }
                let first_image = message.attachments.iter().enumerate().find(|(_, att)| {
                    !att.is_unsupported_media() && !att.is_video() && att.is_image()
                });
                if let Some((att_ix, att)) = first_image
                    && let Some(mp4) = att.tenor_mp4.clone()
                {
                    wanted.push(PendingGif {
                        key: (message.id, att_ix),
                        mp4,
                        fallback: att.proxied_src.clone(),
                        width: att.display_width,
                        height: att.display_height,
                    });
                }
            }
        }

        let wanted_keys: std::collections::HashSet<(MessageId, usize)> =
            wanted.iter().map(|gif| gif.key).collect();
        let before = self.gif_videos.len();
        let stale: Vec<(MessageId, usize)> = self
            .gif_videos
            .keys()
            .filter(|key| !wanted_keys.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(view) = Rc::make_mut(&mut self.gif_videos).remove(&key) {
                view.update(cx, |gif, cx| gif.release_textures(window, cx));
            }
        }
        let mut changed = self.gif_videos.len() != before;

        for gif in wanted {
            if self.gif_videos.len() >= MAX_GIF_VIDEOS {
                break;
            }
            if self.gif_videos.contains_key(&gif.key) {
                continue;
            }
            let view =
                cx.new(|cx| GifVideoView::new(gif.mp4, gif.fallback, gif.width, gif.height, cx));
            if !window.is_window_active() {
                view.update(cx, |gif, cx| gif.set_playing(false, cx));
            }
            Rc::make_mut(&mut self.gif_videos).insert(gif.key, view);
            changed = true;
        }

        if changed {
            cx.notify();
        }
    }

    fn apply_embed_input_reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_topic_box {
            return;
        }
        let fingerprint = (self.cached_for_channel, self.list_state.item_count());
        if self.embed_input_fingerprint == Some(fingerprint) {
            return;
        }
        self.embed_input_fingerprint = Some(fingerprint);

        let desired: Vec<(MessageId, EmbedTextInput)> = {
            let store = MessagesStore::global(cx);
            let store = store.read(cx);
            store
                .viewport_messages()
                .iter()
                .flat_map(|message| {
                    let message_id = message.id;
                    message.embeds.iter().flat_map(move |embed| {
                        embed
                            .fields
                            .iter()
                            .filter_map(move |field| match field.input.as_ref() {
                                Some(EmbedInput::Text(text)) if !text.disabled => {
                                    Some((message_id, text.clone()))
                                }
                                _ => None,
                            })
                    })
                })
                .collect()
        };
        let wanted: HashSet<(MessageId, SharedString)> = desired
            .iter()
            .map(|(id, input)| (*id, input.id.clone()))
            .collect();

        let mut changed = false;
        if self.embed_inputs.keys().any(|key| !wanted.contains(key)) {
            Rc::make_mut(&mut self.embed_inputs).retain(|key, _| wanted.contains(key));
            self.embed_input_subs.retain(|key, _| wanted.contains(key));
            changed = true;
        }

        let bg = cx.theme().tokens.bg_markdown_code;
        let text_color = cx.theme().tokens.text_theme_message;
        for (message_id, input) in desired {
            let key = (message_id, input.id.clone());
            if self.embed_inputs.contains_key(&key) {
                continue;
            }
            let restored = MessagesStore::global(cx)
                .read(cx)
                .embed_form_value(message_id, &input.id)
                .cloned();
            let initial = restored.unwrap_or_else(|| input.default_value.clone());
            let placeholder = if input.required && !input.placeholder.is_empty() {
                SharedString::from(format!("{}*", input.placeholder))
            } else {
                input.placeholder.clone()
            };
            let multiline = input.multiline;
            let min_height = if multiline { px(72.) } else { px(36.) };
            let input_state = cx.new(|cx| {
                TextArea::new(window, cx)
                    .placeholder(placeholder)
                    .single_line(!multiline)
                    .min_height(min_height)
                    .radius(px(4.))
                    .padding_x(px(12.))
                    .text_size(px(14.))
                    .bg(bg)
                    .text_color(text_color)
            });
            if !initial.is_empty() {
                input_state.update(cx, |state, cx| {
                    state.set_value(initial.clone(), cx);
                });
                MessagesStore::global(cx).update(cx, |store, _cx| {
                    store.set_embed_form_value(message_id, input.id.clone(), initial.clone());
                });
            }
            let sub_key = key.clone();
            let sub = cx.subscribe(&input_state, move |_this, entity, event, cx| {
                if *event == TextAreaEvent::Change {
                    let value: SharedString = entity.read(cx).value().to_string().into();
                    let (message_id, input_id) = &sub_key;
                    MessagesStore::global(cx).update(cx, |store, _cx| {
                        store.set_embed_form_value(*message_id, input_id.clone(), value);
                    });
                }
            });
            Rc::make_mut(&mut self.embed_inputs).insert(key.clone(), input_state);
            self.embed_input_subs.insert(key, sub);
            changed = true;
        }

        let select_defaults: Vec<(MessageId, SharedString, Vec<SharedString>)> = {
            let store = MessagesStore::global(cx);
            let store = store.read(cx);
            store
                .viewport_messages()
                .iter()
                .flat_map(|message| {
                    let message_id = message.id;
                    message.embeds.iter().flat_map(move |embed| {
                        embed.fields.iter().filter_map(move |field| {
                            let Some(EmbedInput::Select(select)) = field.input.as_ref() else {
                                return None;
                            };
                            let select_id = select.id.clone()?;
                            let mut defaults: Vec<SharedString> =
                                select.value_selected.clone().into_iter().collect();
                            if defaults.is_empty() {
                                defaults = select
                                    .options
                                    .iter()
                                    .filter(|option| option.default)
                                    .map(|option| option.value.clone())
                                    .collect();
                            }
                            Some((message_id, select_id, defaults))
                        })
                    })
                })
                .collect()
        };
        for (message_id, select_id, defaults) in select_defaults {
            if !self
                .embed_select_seeded
                .insert((message_id, select_id.clone()))
            {
                continue;
            }
            if defaults.is_empty() {
                continue;
            }
            MessagesStore::global(cx).update(cx, |store, cx| {
                if store
                    .message_select_selection(message_id, &select_id)
                    .is_empty()
                {
                    store.set_message_select_selection(message_id, select_id, defaults, cx);
                }
            });
        }

        if changed {
            cx.notify();
        }
    }

    fn reconcile_cold(&mut self, cx: &mut Context<Self>) {
        if self.is_topic_box {
            return;
        }
        let inputs = Self::cold_inputs(cx);
        if inputs != self.last_cold_inputs {
            self.last_cold_inputs = inputs;
            let store = MessagesStore::global(cx);
            let (is_empty, has_more_top, is_dm, dm_channel, conversation_loading) = {
                let s = store.read(cx);
                Self::read_store_inputs(s)
            };
            self.sync_header(is_empty, has_more_top);
            self.sync_skeleton(is_dm, dm_channel, conversation_loading, is_empty, cx);
            cx.notify();
        }
        if self.refresh_derived_state(cx) {
            cx.notify();
        }
    }

    fn compute_indicator_contexts(
        cx: &gpui::App,
    ) -> (Option<WelcomeContext>, Option<OnboardingContext>) {
        let store = MessagesStore::global(cx);
        let (is_dm, channel_id, has_indicator) = {
            let s = store.read(cx);
            (
                s.is_dm(),
                s.active_channel_id(),
                s.viewport_messages()
                    .first()
                    .is_some_and(|m| m.code == MessageCode::Indicator),
            )
        };
        if has_indicator {
            (
                build_welcome_context(is_dm, channel_id, cx),
                build_onboarding_context(is_dm, cx),
            )
        } else {
            (None, None)
        }
    }

    fn refresh_derived_state(&mut self, cx: &mut Context<Self>) -> bool {
        let (welcome, onboarding) = Self::compute_indicator_contexts(cx);
        let store = MessagesStore::global(cx);
        let unread = unread_boundary(&store, self.last_seen_at_bottom, cx);
        let fab_unread_count = fab_unread_count(
            self.last_seen_at_bottom,
            store
                .read(cx)
                .viewport_messages()
                .iter()
                .rev()
                .map(|message| message.id),
        );
        if welcome == self.welcome
            && onboarding == self.onboarding
            && unread == self.cached_unread_boundary
            && fab_unread_count == self.cached_fab_unread_count
        {
            return false;
        }
        self.welcome = welcome;
        self.onboarding = onboarding;
        self.cached_unread_boundary = unread;
        self.cached_fab_unread_count = fab_unread_count;
        true
    }

    fn sync_header(&mut self, is_empty: bool, has_more_top: bool) {
        let want_header = !is_empty && has_more_top;
        if want_header && !self.header_shown {
            self.list_state.splice(0..0, 1);
            self.header_shown = true;
        } else if !want_header && self.header_shown {
            self.list_state.splice(0..1, 0);
            self.header_shown = false;
        }
    }

    fn sync_skeleton(
        &mut self,
        is_dm: bool,
        dm_channel: Option<ChannelId>,
        conversation_loading: bool,
        is_empty: bool,
        cx: &mut Context<Self>,
    ) {
        let (active_clan, has_clan_channel, clan_loading) = Self::cold_inputs(cx);
        let has_conversation = has_clan_channel || is_dm;
        let cold_clan = !has_conversation && clan_loading;
        let loading_conversation = has_conversation && conversation_loading && is_empty;
        let loading = cold_clan || loading_conversation;
        let skeleton_key = if is_dm {
            dm_channel.map_or(SkeletonKey::None, SkeletonKey::Conversation)
        } else {
            active_clan.map_or(SkeletonKey::None, SkeletonKey::Clan)
        };
        self.advance_skeleton(loading, skeleton_key, cx);
    }

    fn read_store_inputs(store: &MessagesStore) -> (bool, bool, bool, Option<ChannelId>, bool) {
        let is_empty = store.viewport_messages().is_empty();
        let has_more_top = store.has_more_top();
        let is_dm = store.is_dm();
        let dm_channel = store.active_channel_id();
        let conversation_loading = store.is_loading();
        (
            is_empty,
            has_more_top,
            is_dm,
            dm_channel,
            conversation_loading,
        )
    }

    fn cold_inputs(cx: &mut Context<Self>) -> (Option<ClanId>, bool, bool) {
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let channel_list = ChannelList::global(cx);
        let channel_list = channel_list.read(cx);
        (
            active_clan,
            channel_list.active_channel().is_some(),
            active_clan.is_some_and(|clan| channel_list.is_loading_clan(clan)),
        )
    }

    fn clear_image_cache_if_channel_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel_id = ChannelList::global(cx).read(cx).active_channel_id;
        if self.cached_for_channel == channel_id {
            return;
        }
        #[cfg(debug_assertions)]
        tracing::debug!(
            target: "render",
            "ChannelMessages switch {:?} -> {:?}",
            self.cached_for_channel,
            channel_id
        );
        self.cached_for_channel = channel_id;
        self.selection.borrow_mut().clear();
        self.selection_pointer = None;
        self.expanded_selection = None;
        self.selection_autoscroll_scheduled = false;
        self.last_seen_at_bottom = None;
        self.fab_scroll_pending = false;
        self.last_scroll_sync = None;
        {
            let mut memo = self.row_memo.borrow_mut();
            memo.avatars.clear();
            memo.display_names.clear();
            memo.time_labels.clear();
            memo.rich_text.clear();
            memo.selection_layouts.clear();
            memo.selection_text_pieces.clear();
        }
        self.channel_permissions_fp = None;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        crate::image_cache::release_freed_memory_to_os(cx);
        self.refresh_derived_state(cx);
    }

    fn apply_scroll_anchor(&mut self, channel_id: ChannelId, update: AnchorUpdate) {
        match update {
            AnchorUpdate::Clear => {
                self.scroll_anchors.remove(&channel_id);
            }
            AnchorUpdate::Set(anchor) => {
                if self.scroll_anchors.get(&channel_id) != Some(&anchor) {
                    self.scroll_anchors.insert(channel_id, anchor);
                }
            }
            AnchorUpdate::Keep => {}
        }
    }

    fn live_at_bottom(&self) -> bool {
        let count = self.list_state.item_count();
        let scroll_top = self.list_state.logical_scroll_top();
        if scroll_top.item_ix >= count {
            return true;
        }
        let tail_visible =
            count == 0 || self.list_state.item_is_below_viewport(count - 1) == Some(false);
        if !tail_visible {
            return false;
        }
        let current = self.list_state.scroll_px_offset_for_scrollbar().y;
        let max = self.list_state.max_offset_for_scrollbar().y;
        max + current <= px(BOTTOM_THRESHOLD_PX)
    }

    fn sync_live_scroll_state(&mut self, cx: &mut Context<Self>) {
        let store_entity = MessagesStore::global(cx);
        let Some(channel_id) = store_entity.read(cx).active_channel_id() else {
            return;
        };
        let at_bottom = self.live_at_bottom();
        let scroll_top = self.list_state.logical_scroll_top();
        let anchor_update = {
            let store = store_entity.read(cx);
            capture_anchor(
                store.viewport_messages(),
                at_bottom,
                scroll_top,
                self.header_shown,
            )
        };
        self.at_bottom = at_bottom;
        self.apply_scroll_anchor(channel_id, anchor_update);
        store_entity.update(cx, |store, _cx| {
            store.set_viewing_older(channel_id, !at_bottom);
        });
        if at_bottom && !store_entity.read(cx).has_more_bottom() {
            self.sync_channel_seen(cx);
        }
        self.maybe_paginate_by_items(cx);
    }

    fn schedule_scroll_state_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel_id) = MessagesStore::global(cx).read(cx).active_channel_id() else {
            return;
        };
        let scroll_top = self.list_state.logical_scroll_top();
        let current = self.list_state.scroll_px_offset_for_scrollbar().y;
        let max = self.list_state.max_offset_for_scrollbar().y;
        let fingerprint = (
            channel_id,
            scroll_top.item_ix,
            scroll_top.offset_in_item.as_f32().to_bits(),
            self.list_state.item_count(),
            current.as_f32().to_bits(),
            max.as_f32().to_bits(),
            self.header_shown,
        );
        if self.last_scroll_sync == Some(fingerprint) {
            return;
        }
        self.last_scroll_sync = Some(fingerprint);
        cx.defer_in(window, |this, _window, cx| {
            this.sync_live_scroll_state(cx);
        });
    }

    fn schedule_pagination_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pagination_check_scheduled {
            return;
        }
        self.pagination_check_scheduled = true;
        cx.on_next_frame(window, |this, _window, cx| {
            this.pagination_check_scheduled = false;
            this.maybe_paginate_by_items(cx);
        });
    }

    fn maybe_paginate_by_items(&mut self, cx: &mut Context<Self>) {
        let item_count = self.list_state.item_count();
        let scroll_top = self.list_state.logical_scroll_top().item_ix;
        let (visible_start, visible_end) = if self.last_visible_start == scroll_top {
            (self.last_visible_start, self.last_visible_end)
        } else {
            (scroll_top, scroll_top)
        };
        let (near_top, near_bottom) =
            pagination_proximity(visible_start, visible_end, item_count, self.header_shown);
        let store_entity = MessagesStore::global(cx);
        let (count, edges, has_more_top, has_more_bottom) = {
            let store = store_entity.read(cx);
            let messages = store.viewport_messages();
            (
                messages.len(),
                (
                    messages.first().map(|message| message.id),
                    messages.last().map(|message| message.id),
                ),
                store.has_more_top(),
                store.has_more_bottom(),
            )
        };
        if count != self.last_paginate_count || edges != self.last_paginate_edges {
            self.last_paginate_count = count;
            self.last_paginate_edges = edges;
            self.paginate_armed_top = true;
            self.paginate_armed_bottom = true;
        }
        if !near_top || !has_more_top {
            self.paginate_armed_top = true;
        }
        if !near_bottom || !has_more_bottom {
            self.paginate_armed_bottom = true;
        }
        match pagination_direction(
            near_top,
            near_bottom,
            has_more_top,
            has_more_bottom,
            self.paginate_armed_top,
            self.paginate_armed_bottom,
        ) {
            Some(PaginationDirection::Top) => {
                self.paginate_armed_top = false;
                store_entity.update(cx, |store, cx| store.scroll_reached_top(cx));
            }
            Some(PaginationDirection::Bottom) => {
                self.paginate_armed_bottom = false;
                store_entity.update(cx, |store, cx| store.scroll_reached_bottom(cx));
            }
            None => {}
        }
    }

    fn sync_channel_seen(&mut self, cx: &mut Context<Self>) {
        let app_focused = cx.active_window().is_some();
        self.sync_channel_seen_when_focused(app_focused, cx);
    }

    fn sync_channel_seen_when_focused(&mut self, app_focused: bool, cx: &mut Context<Self>) {
        let store_entity = MessagesStore::global(cx);
        if store_entity.read(cx).has_more_bottom() {
            return;
        }
        let Some((last_id, last_create_time)) = store_entity
            .read(cx)
            .viewport_messages()
            .last()
            .filter(|m| !m.id.is_optimistic())
            .map(|m| (m.id, m.create_time))
        else {
            return;
        };
        let tail_changed = self.last_seen_at_bottom != Some(last_id);
        self.last_seen_at_bottom = Some(last_id);
        store_entity.update(cx, |store, cx| {
            store.note_viewport_seen(last_id, last_create_time, app_focused, cx);
        });
        if tail_changed && self.refresh_derived_state(cx) {
            cx.notify();
        }
    }

    fn scroll_down_clicked(&mut self, cx: &mut Context<Self>) {
        let store_entity = MessagesStore::global(cx);
        let (has_more_bottom, buffer_len) = {
            let s = store_entity.read(cx);
            (s.has_more_bottom(), s.viewport_messages().len())
        };

        if has_more_bottom && buffer_len >= JUMP_PRESENT_MIN_MESSAGES {
            self.fab_scroll_pending = true;
            store_entity.update(cx, |store, cx| store.jump_to_present(cx));
            return;
        }

        if has_more_bottom {
            store_entity.update(cx, |store, cx| store.scroll_reached_bottom(cx));
        }
        self.list_state.scroll_to_end();
        self.at_bottom = true;
        if let Some(channel_id) = store_entity.read(cx).active_channel_id() {
            self.scroll_anchors.remove(&channel_id);
            store_entity.update(cx, |store, _cx| {
                store.set_viewing_older(channel_id, false);
            });
        }
        if let Some(last) = store_entity
            .read(cx)
            .viewport_messages()
            .last()
            .filter(|m| !m.id.is_optimistic())
        {
            self.last_seen_at_bottom = Some(last.id);
        }
        self.refresh_derived_state(cx);
        cx.notify();
    }

    fn scroll_down_fab(
        &self,
        visible: bool,
        unread_count: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let badge_label = if unread_count > 99 {
            "99+".to_string()
        } else {
            unread_count.to_string()
        };

        div()
            .id("scroll-down-fab")
            .absolute()
            .bottom(px(FAB_BOTTOM))
            .right(px(FAB_RIGHT))
            .size(px(FAB_SIZE))
            .rounded_full()
            .bg(theme.bg_tertiary)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .opacity(if visible { 1.0 } else { 0.0 })
            .when(visible, |el| {
                el.cursor_pointer()
                    .occlude()
                    .hover(|style| {
                        style
                            .size(px(FAB_HOVER_SIZE))
                            .bottom(px(FAB_BOTTOM - FAB_HOVER_GROW / 2.))
                            .right(px(FAB_RIGHT - FAB_HOVER_GROW / 2.))
                            .bg(theme.bg_hover)
                    })
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.scroll_down_clicked(cx);
                    }))
            })
            .when(unread_count > 0, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(-4.))
                        .right(px(-4.))
                        .min_w(px(18.))
                        .h(px(18.))
                        .px_1()
                        .rounded_full()
                        .bg(theme.status_dnd)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(badge_label),
                )
            })
            .child(
                Icon::new(IconName::ArrowDown)
                    .size(px(18.))
                    .text_color(theme.text_primary),
            )
    }

    fn advance_skeleton(&mut self, loading: bool, key: SkeletonKey, cx: &mut Context<Self>) {
        let same_key = self.skeleton_key == key;
        let (next, timer) = skeleton_transition(
            self.skeleton_phase,
            SkeletonInput::Sync { loading, same_key },
        );
        self.skeleton_key = key;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, key, cx);
    }

    fn arm_skeleton_timer(
        &mut self,
        timer: SkeletonTimer,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) {
        match skeleton_timer_action(timer) {
            TimerAction::Keep => {}
            TimerAction::Clear => self._skeleton_timer = None,
            TimerAction::Arm(input, delay_ms) => {
                self._skeleton_timer = Some(Self::spawn_skeleton_timer(input, delay_ms, key, cx));
            }
        }
    }

    fn spawn_skeleton_timer(
        input: SkeletonInput,
        delay_ms: u64,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            this.update(cx, |this, cx| this.on_skeleton_timer(input, key, cx))
                .ok();
        })
    }

    fn on_skeleton_timer(
        &mut self,
        input: SkeletonInput,
        key: SkeletonKey,
        cx: &mut Context<Self>,
    ) {
        if self.skeleton_key != key {
            return;
        }
        let (next, timer) = skeleton_transition(self.skeleton_phase, input);
        let changed = next != self.skeleton_phase;
        self.skeleton_phase = next;
        self.arm_skeleton_timer(timer, key, cx);
        if changed {
            cx.notify();
        }
    }

    fn skeleton_overlay(&self, theme: &Theme) -> Option<gpui::AnyElement> {
        let base = || {
            div()
                .absolute()
                .inset_0()
                .bg(theme.bg_primary)
                .flex()
                .flex_col()
                .justify_end()
                .child(message_skeleton(theme, 5))
        };
        match self.skeleton_phase {
            SkeletonPhase::Showing => Some(
                base()
                    .with_animation(
                        self.skeleton_anim_id("skeleton-in"),
                        Animation::new(Duration::from_millis(SKELETON_FADE_IN_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(delta),
                    )
                    .into_any_element(),
            ),
            SkeletonPhase::Settling => Some(base().into_any_element()),
            SkeletonPhase::FadingOut => Some(
                base()
                    .with_animation(
                        self.skeleton_anim_id("skeleton-out"),
                        Animation::new(Duration::from_millis(SKELETON_FADE_OUT_MS))
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(1.0 - delta),
                    )
                    .into_any_element(),
            ),
            SkeletonPhase::Hidden | SkeletonPhase::Pending => None,
        }
    }

    fn skeleton_anim_id(&self, prefix: &'static str) -> SharedString {
        match self.skeleton_key {
            SkeletonKey::Clan(id) => SharedString::from(format!("{prefix}-c{id}")),
            SkeletonKey::Conversation(id) => SharedString::from(format!("{prefix}-d{id}")),
            SkeletonKey::None => SharedString::from(prefix),
        }
    }
}

impl ChannelMessages {
    fn mark_scroll_activity(&mut self, cx: &mut Context<Self>) {
        self.last_scroll_at = Some(Instant::now());
        self.arm_scroll_memory_relief(cx);
    }

    fn arm_scroll_memory_relief(&mut self, cx: &mut Context<Self>) {
        if self.scroll_relief_armed {
            return;
        }
        self.scroll_relief_armed = true;
        cx.spawn(async move |this, cx| {
            let mut remaining = SCROLL_RELIEF_DELAY;
            loop {
                cx.background_executor().timer(remaining).await;
                let next = this
                    .update(cx, |this, cx| {
                        let next = this.last_scroll_at.and_then(|last_activity| {
                            deadline_remaining(last_activity, Instant::now(), SCROLL_RELIEF_DELAY)
                        });
                        if next.is_none() {
                            this.scroll_relief_armed = false;
                            crate::image_cache::release_freed_memory_to_os(cx);
                        }
                        next
                    })
                    .ok()
                    .flatten();
                let Some(next) = next else {
                    break;
                };
                remaining = next;
            }
        })
        .detach();
    }

    fn scroll_messages_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        let now = Instant::now();
        let current = self.list_state.scroll_px_offset_for_scrollbar().y.as_f32();
        let previous = self.keyboard_scroll;
        let next = retarget_keyboard_scroll(current, previous, delta.as_f32(), now);
        if next == previous {
            return;
        }
        self.keyboard_scroll = next;
        cx.notify();
    }

    fn drive_scroll_anim(&mut self, window: &mut Window) {
        if self.list_state.is_scrollbar_dragging() || self.list_state.is_smooth_wheel_scrolling() {
            self.keyboard_scroll = None;
            return;
        }
        let Some(mut animation) = self.keyboard_scroll else {
            return;
        };

        let (position, _, finished) = animation.sample(Instant::now());
        let step = position - animation.applied;
        if step.abs() > f32::EPSILON {
            let live = self.list_state.scroll_px_offset_for_scrollbar().y;
            self.list_state
                .set_offset_from_scrollbar(Point::new(px(0.), live + px(step)));
            if step.abs() > 1. && self.list_state.scroll_px_offset_for_scrollbar().y == live {
                self.keyboard_scroll = None;
                return;
            }
        }
        if finished {
            self.keyboard_scroll = None;
        } else {
            animation.applied = position;
            self.keyboard_scroll = Some(animation);
            window.request_animation_frame();
        }
    }

    fn on_messages_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if event.keystroke.key == "c" {
            let modifiers = &event.keystroke.modifiers;
            let copy_combo = if cfg!(target_os = "macos") {
                modifiers.platform
            } else {
                modifiers.control
            };
            if copy_combo && !modifiers.alt && self.copy_selection(cx) {
                cx.stop_propagation();
                return;
            }
        }
        let step = px(KEY_SCROLL_STEP_PX);
        let page = self.list_state.viewport_bounds().size.height * 0.9;
        let handled = match event.keystroke.key.as_str() {
            "up" => {
                self.scroll_messages_by(step, cx);
                true
            }
            "down" => {
                self.scroll_messages_by(-step, cx);
                true
            }
            "pageup" => {
                self.scroll_messages_by(page, cx);
                true
            }
            "pagedown" => {
                self.scroll_messages_by(-page, cx);
                true
            }
            "home" => {
                self.keyboard_scroll = None;
                self.list_state.scroll_to(gpui::ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                });
                cx.notify();
                true
            }
            "end" => {
                self.keyboard_scroll = None;
                self.list_state.scroll_to_end();
                cx.notify();
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn on_selection_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hit = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            if let Some(message_id) = self.raw_hover {
                message_offset_at(&state, message_id, event.position)
                    .map(|offset| (message_id, offset))
            } else {
                state
                    .registry
                    .iter()
                    .find_map(|(id, layout)| {
                        text_layout_offset_at(layout, event.position).map(|offset| (*id, offset))
                    })
                    .or_else(|| {
                        state.segment_registry.iter().find_map(|(id, entry)| {
                            segment_offset_at(&entry.segments, event.position)
                                .map(|offset| (*id, offset))
                        })
                    })
                    .or_else(|| {
                        state.segment_registry.iter().find_map(|(id, entry)| {
                            segment_offset_snap(&entry.segments, event.position)
                                .map(|offset| (*id, offset))
                        })
                    })
            }
        };
        match hit {
            Some((message_id, offset)) => {
                let had_selection = self
                    .selection
                    .try_borrow()
                    .is_ok_and(|selection| selection.has_selection());
                let range = if event.click_count >= 2 {
                    let text = self
                        .find_local_message(message_id, cx)
                        .map(|message| {
                            super::content::selectable_message_text(
                                &message,
                                &self.cached_locale,
                                &self.cached_current_user_id,
                                cx,
                            )
                        })
                        .or_else(|| {
                            let state = self.selection.try_borrow().ok()?;
                            state
                                .registry
                                .get(&message_id)
                                .and_then(TextLayout::try_text)
                                .or_else(|| {
                                    state
                                        .segment_registry
                                        .get(&message_id)
                                        .map(|entry| entry.text.to_string())
                                })
                        })
                        .unwrap_or_default();
                    let offset = offset.min(text.len());
                    if event.click_count >= 3 {
                        0..text.len()
                    } else {
                        word_range(&text, offset)
                    }
                } else {
                    offset..offset
                };
                let expanded_selection = (range.start < range.end).then_some(ExpandedSelection {
                    message_id,
                    start: range.start,
                    end: range.end,
                    origin: event.position,
                });
                let needs_repaint = had_selection || range.start < range.end;
                {
                    let Ok(mut state) = self.selection.try_borrow_mut() else {
                        return;
                    };
                    state.anchor = Some(SelPoint {
                        message_id,
                        offset: range.start,
                    });
                    state.head = Some(SelPoint {
                        message_id,
                        offset: range.end,
                    });
                    state.selecting = true;
                    // Cross-message ordering is only needed once the pointer
                    // actually leaves the anchor message.
                    state.order_map.clear();
                }
                // Autoscroll starts only after a real drag. Keeping the pointer
                // unset here prevents a click near a viewport edge from moving
                // the message list while the button is merely held down.
                self.selection_pointer = None;
                self.expanded_selection = expanded_selection;
                if !self.focus_handle.is_focused(window) {
                    window.focus(&self.focus_handle, cx);
                }
                if needs_repaint {
                    cx.notify();
                }
            }
            None => {
                let had = self
                    .selection
                    .try_borrow()
                    .is_ok_and(|selection| selection.has_selection());
                let Ok(mut selection) = self.selection.try_borrow_mut() else {
                    return;
                };
                selection.clear();
                drop(selection);
                self.selection_pointer = None;
                self.expanded_selection = None;
                if had {
                    cx.notify();
                }
            }
        }
    }

    fn replace_selection_order(&self, order: impl Iterator<Item = (MessageId, usize)>) {
        let Ok(mut state) = self.selection.try_borrow_mut() else {
            return;
        };
        state.order_map.clear();
        let (minimum, _) = order.size_hint();
        state.order_map.reserve(minimum);
        state.order_map.extend(order);
    }

    fn sync_selection_order(&self, cx: &App) {
        if self.is_topic_box {
            let stale = {
                let Ok(state) = self.selection.try_borrow() else {
                    return;
                };
                state.order_map.len() != self.topic_messages.len()
                    || self
                        .topic_messages
                        .iter()
                        .enumerate()
                        .any(|(index, message)| state.order_map.get(&message.id) != Some(&index))
            };
            if stale {
                self.replace_selection_order(
                    self.topic_messages
                        .iter()
                        .enumerate()
                        .map(|(index, message)| (message.id, index)),
                );
            }
            return;
        }
        let store = MessagesStore::global(cx);
        let messages = store.read(cx);
        let messages = messages.viewport_messages();
        let stale = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            state.order_map.len() != messages.len()
                || messages
                    .iter()
                    .enumerate()
                    .any(|(index, message)| state.order_map.get(&message.id) != Some(&index))
        };
        if !stale {
            return;
        }
        self.replace_selection_order(
            messages
                .iter()
                .enumerate()
                .map(|(index, message)| (message.id, index)),
        );
    }

    fn on_selection_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .selection
            .try_borrow()
            .is_ok_and(|selection| selection.selecting)
        {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            if let Ok(mut selection) = self.selection.try_borrow_mut() {
                selection.selecting = false;
            }
            self.selection_pointer = None;
            self.expanded_selection = None;
            return;
        }
        if self
            .expanded_selection
            .is_some_and(|expanded| !expanded.drag_started(event.position))
        {
            return;
        }
        self.selection_pointer = Some(event.position);
        let point = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            message_point_at(&state, event.position)
        };
        let crosses_messages = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            point.is_some_and(|point| {
                state
                    .anchor
                    .is_some_and(|anchor| anchor.message_id != point.message_id)
            })
        };
        if crosses_messages {
            self.sync_selection_order(cx);
        }
        let changed = self.selection.try_borrow_mut().is_ok_and(|mut state| {
            point.is_some_and(|point| {
                update_selection_head(&mut state, point, self.expanded_selection)
            })
        });
        if changed {
            cx.notify();
        }
        self.schedule_selection_autoscroll(window, cx);
    }

    fn on_selection_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let point = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            message_point_at(&state, event.position)
        };
        let crosses_messages = {
            let Ok(state) = self.selection.try_borrow() else {
                return;
            };
            point.is_some_and(|point| {
                state
                    .anchor
                    .is_some_and(|anchor| anchor.message_id != point.message_id)
            })
        };
        if crosses_messages {
            self.sync_selection_order(cx);
        }
        let preserve_expanded = self
            .expanded_selection
            .is_some_and(|expanded| !expanded.drag_started(event.position));
        let (empty, head_changed) = {
            let Ok(mut state) = self.selection.try_borrow_mut() else {
                return;
            };
            if !state.selecting {
                return;
            }
            let head_changed = !preserve_expanded
                && point.is_some_and(|point| {
                    update_selection_head(&mut state, point, self.expanded_selection)
                });
            state.selecting = false;
            (!state.has_selection(), head_changed)
        };
        self.selection_pointer = None;
        self.expanded_selection = None;
        if empty && let Ok(mut selection) = self.selection.try_borrow_mut() {
            selection.clear();
        }
        if head_changed {
            cx.notify();
        }
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut App) -> bool {
        self.sync_selection_order(cx);
        let text = {
            let state = self.selection.borrow();
            if !state.has_selection() {
                return false;
            }
            if self.is_topic_box {
                selected_text_for_messages(
                    &state,
                    &self.topic_messages,
                    &self.cached_locale,
                    &self.cached_current_user_id,
                    cx,
                )
            } else {
                let store = MessagesStore::global(cx);
                let messages = store.read(cx);
                selected_text_for_messages(
                    &state,
                    messages.viewport_messages(),
                    &self.cached_locale,
                    &self.cached_current_user_id,
                    cx,
                )
            }
        };
        let Some(text) = text else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn on_copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_handle.is_focused(window) && self.copy_selection(cx) {
            cx.stop_propagation();
        }
    }

    fn selection_scroll_delta(&self) -> Pixels {
        let Some(pointer) = self.selection_pointer else {
            return px(0.);
        };
        let bounds = self.list_state.viewport_bounds();
        if bounds.size.height <= px(0.) {
            return px(0.);
        }
        let edge = px(40.);
        let max_step = 18.;
        if pointer.y < bounds.top() + edge {
            let ratio = ((bounds.top() + edge - pointer.y).as_f32() / edge.as_f32()).clamp(0.2, 1.);
            return px(-max_step * ratio);
        }
        if pointer.y > bounds.bottom() - edge {
            let ratio =
                ((pointer.y - (bounds.bottom() - edge)).as_f32() / edge.as_f32()).clamp(0.2, 1.);
            return px(max_step * ratio);
        }
        px(0.)
    }

    fn schedule_selection_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection_autoscroll_scheduled
            || !self
                .selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
            || self.selection_scroll_delta() == px(0.)
        {
            return;
        }
        self.selection_autoscroll_scheduled = true;
        cx.on_next_frame(window, |this, _window, cx| {
            this.selection_autoscroll_scheduled = false;
            if !this
                .selection
                .try_borrow()
                .is_ok_and(|selection| selection.selecting)
            {
                return;
            }
            let Some(pointer) = this.selection_pointer else {
                return;
            };
            this.sync_selection_order(cx);
            let point = {
                let Ok(state) = this.selection.try_borrow() else {
                    return;
                };
                message_point_at(&state, pointer)
            };
            let changed = this.selection.try_borrow_mut().is_ok_and(|mut state| {
                point.is_some_and(|point| {
                    update_selection_head(&mut state, point, this.expanded_selection)
                })
            });
            let delta = this.selection_scroll_delta();
            let before = this.list_state.logical_scroll_top();
            this.list_state.scroll_by(delta);
            let after = this.list_state.logical_scroll_top();
            let scrolled =
                after.item_ix != before.item_ix || after.offset_in_item != before.offset_in_item;
            if !changed && !scrolled {
                this.selection_pointer = None;
                this.expanded_selection = None;
                return;
            }
            cx.notify();
        });
    }

    fn render_topic_box(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.sync_render_identity(cx);
        self.drive_scroll_anim(window);
        self.schedule_selection_autoscroll(window, cx);
        let suppress_hover = self.list_state.is_scroll_hover_suppressed();
        let scroll_active =
            self.keyboard_scroll.is_some() || self.list_state.is_scroll_hover_active();
        let store = MessagesStore::global(cx);
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let (is_dm, dm_channel) = {
            let s = store.read(cx);
            (s.is_dm(), s.active_channel_id())
        };
        let (channel_type, channel_top_level) = ChannelList::global(cx)
            .read(cx)
            .active_channel()
            .map(|c| (Some(c.channel_type), c.parent_id.is_none()))
            .unwrap_or((None, true));
        let is_clan_owner = self.cached_is_clan_owner;
        let emoji_recent = self.emoji_recent.clone();
        let (editing_id, edit_input) = match &self.edit_input {
            Some((id, input)) => (Some(*id), Some(input.clone())),
            None => (None, None),
        };
        let origin_id = TopicsStore::global(cx)
            .read(cx)
            .origin_message()
            .map(|m| m.id);
        let channel_origin_top = origin_id.and_then(|id| {
            self.topic_align_timeline
                .as_ref()?
                .upgrade()?
                .read(cx)
                .message_viewport_top(id, cx)
        });
        let own_bounds = self.list_state.viewport_bounds();
        let align_delta = (own_bounds.size.height > px(0.))
            .then(|| channel_origin_top.map(|y| y - own_bounds.top()))
            .flatten()
            .filter(|delta| *delta > px(0.));
        if align_delta.is_none()
            && channel_origin_top.is_some()
            && own_bounds.size.height <= px(0.)
            && !self.topic_align_probe
        {
            self.topic_align_probe = true;
            cx.defer_in(window, |_, _, cx| cx.notify());
        }
        let use_align_spacer = align_delta.is_some();
        let align_spacer_h = align_delta.unwrap_or(px(0.));
        self.sync_topic_spacer(use_align_spacer, align_spacer_h);

        let locale = self.cached_locale.clone();
        let coming_soon: SharedString = mezon_i18n::t(&locale, "common.comingSoon").into();
        let frame_now = chrono::Local::now();
        let today = frame_now.date_naive();
        if self.row_memo_day != Some(today) {
            self.row_memo_day = Some(today);
            self.row_memo.borrow_mut().time_labels.clear();
        }
        let row_memo = self.row_memo.clone();
        let selection = self.selection.clone();
        let list_state = self.list_state.clone();
        let hovered_row = self.hovered_row;
        let context_menu_message = self.context_menu_target.map(|(id, _)| id);
        let avatar_image_cache = self.avatar_image_cache.clone();
        let small_avatar_image_cache = self.small_avatar_image_cache.clone();
        let icon_image_cache = self.icon_image_cache.clone();
        let reply_highlight_id = TopicsStore::global(cx)
            .read(cx)
            .reply_target()
            .map(|d| d.message_ref_id);
        let profile_context = channel_profile_context(is_dm, dm_channel, active_clan, cx);
        let settings = self.settings.clone();
        let active_videos = self.active_videos.clone();
        let active_audios = self.active_audios.clone();
        let gif_videos = self.gif_videos.clone();
        let embed_inputs = self.embed_inputs.clone();
        let video_host = cx.entity().downgrade();
        let current_user_id = self.cached_current_user_id.clone();
        let role_ids = self.cached_role_ids.clone();
        let entity = cx.entity();
        let selection_host = cx.entity().downgrade();
        let selection_state = self.selection.clone();
        let key_listener = cx.listener(Self::on_messages_key);
        let copy_listener = cx.listener(Self::on_copy);
        let focus_on_click = cx.listener(|this, _: &MouseDownEvent, window, cx| {
            if !this.focus_handle.contains_focused(window, cx) {
                window.focus(&this.focus_handle, cx);
            }
        });

        let content = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .image_cache(self.image_cache.clone())
            .track_focus(&self.focus_handle)
            .on_action(copy_listener)
            .on_key_down(key_listener)
            .on_mouse_down(MouseButton::Left, focus_on_click)
            .child(
                list(list_state, move |ix, _window, cx| {
                    if use_align_spacer && ix == 0 {
                        return div()
                            .id("topic-align-spacer")
                            .w_full()
                            .h(align_spacer_h)
                            .flex_shrink_0()
                            .into_any_element();
                    }
                    let msg_ix = ix - usize::from(use_align_spacer);
                    let ctx = RowCtx {
                        app: cx,
                        theme: cx.theme(),
                        locale: &locale,
                        current_user_id: &current_user_id,
                        current_role_ids: &role_ids,
                        welcome: None,
                        onboarding: None,
                        suppress_hover,
                        is_topic_box: true,
                        scroll_active,
                        hovered_row,
                        context_menu_message,
                        avatar_cache: small_avatar_image_cache.clone(),
                        large_avatar_cache: avatar_image_cache.clone(),
                        icon_cache: icon_image_cache.clone(),
                        unread_boundary_id: None,
                        highlight_id: None,
                        reply_highlight_id,
                        profile_context,
                        settings: settings.clone(),
                        active_videos: &active_videos,
                        active_audios: &active_audios,
                        gif_videos: &gif_videos,
                        embed_inputs: &embed_inputs,
                        video_host: video_host.clone(),
                        now: frame_now,
                        clan_id: active_clan,
                        channel_type,
                        channel_top_level,
                        is_clan_owner,
                        editing_id,
                        edit_input: edit_input.clone(),
                        emoji_recent: &emoji_recent,
                        coming_soon: coming_soon.clone(),
                        row_memo: row_memo.clone(),
                        selection: selection.clone(),
                    };
                    render_message_item(&entity.read(cx).topic_messages, msg_ix, &ctx, cx)
                })
                .flex_1()
                .size_full()
                .pb(px(LIST_BOTTOM_PADDING)),
            )
            .when_some(self.mention_popover.clone(), |el, (popover, position)| {
                el.child(deferred(
                    anchored().position(position).snap_to_window().child(
                        div()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.mention_popover = None;
                                this._mention_popover_sub = None;
                                cx.notify();
                            }))
                            .child(popover),
                    ),
                ))
            })
            .when_some(self.reaction_picker.clone(), |el, (picker, position)| {
                el.child(deferred(
                    anchored()
                        .position(position)
                        .anchor(Anchor::TopRight)
                        .snap_to_window()
                        .child(
                            div()
                                .occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.reaction_picker = None;
                                    this._reaction_picker_sub = None;
                                    this._reaction_picker_dismiss_sub = None;
                                    cx.notify();
                                }))
                                .child(picker),
                        ),
                ))
            })
            .when_some(self.context_menu_target, |el, (message_id, position)| {
                let Some(target_msg) = self.find_local_message(message_id, cx) else {
                    return el;
                };
                let selected_text = {
                    let state = self.selection.borrow();
                    selected_text_for_messages(
                        &state,
                        &self.topic_messages,
                        &self.cached_locale,
                        &self.cached_current_user_id,
                        cx,
                    )
                };
                let menu = message_context_menu::build(
                    &target_msg,
                    &self.cached_current_user_id,
                    self.cached_is_clan_owner,
                    &self.cached_locale,
                    self.context_menu_forward_all,
                    true,
                    self.reaction_submenu_open,
                    selected_text,
                    cx.entity().downgrade(),
                    cx,
                );
                el.child(context_menu_at(position, menu))
            })
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical)
                    .tracked_scroll_handle(&self.list_state),
                window,
                cx,
            );
        SelectionCapture::new(content.into_any_element(), selection_host, selection_state)
            .into_any_element()
    }
}

impl Render for ChannelMessages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::image_cache::sweep_ogp_cache(window, cx);
        {
            let mut selection = self.selection.borrow_mut();
            selection.begin_render();
        }
        if self.is_topic_box {
            return self.render_topic_box(window, cx);
        }
        self.schedule_selection_autoscroll(window, cx);
        crate::trace_render!(
            "ChannelMessages ch={:?}",
            MessagesStore::global(cx).read(cx).active_channel_id()
        );
        self.clear_image_cache_if_channel_changed(window, cx);
        self.sync_render_identity(cx);
        self.drive_scroll_anim(window);
        let suppress_hover = self.list_state.is_scroll_hover_suppressed();
        let scroll_active =
            self.keyboard_scroll.is_some() || self.list_state.is_scroll_hover_active();
        if !scroll_active
            && self
                .last_image_cache_sweep
                .is_none_or(|at| at.elapsed() >= IDLE_CACHE_SWEEP_INTERVAL)
        {
            self.image_cache
                .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
            self.last_image_cache_sweep = Some(Instant::now());
        }
        if !scroll_active {
            self.schedule_scroll_state_sync(window, cx);
            cx.defer_in(window, |this, window, cx| {
                this.apply_gif_reconcile(window, cx);
                this.apply_embed_input_reconcile(window, cx);
            });
        }

        let store = MessagesStore::global(cx);
        let active_clan = ClanList::global(cx).read(cx).active_clan_id;
        let (is_dm, dm_channel) = {
            let s = store.read(cx);
            (s.is_dm(), s.active_channel_id())
        };
        let (channel_type, channel_top_level) = ChannelList::global(cx)
            .read(cx)
            .active_channel()
            .map(|c| (Some(c.channel_type), c.parent_id.is_none()))
            .unwrap_or((None, true));
        let is_clan_owner = self.cached_is_clan_owner;
        let emoji_recent = self.emoji_recent.clone();
        let (editing_id, edit_input) = match &self.edit_input {
            Some((id, input)) => (Some(*id), Some(input.clone())),
            None => (None, None),
        };
        let skeleton_overlay = self.skeleton_overlay(cx.theme());
        let header_shown = self.header_shown;

        if let Some(target) = self.pending_jump.take()
            && let Some(pos) = store
                .read(cx)
                .viewport_messages()
                .iter()
                .position(|m| m.id == target)
        {
            self.list_state
                .scroll_to_reveal_item(usize::from(header_shown) + pos);
        }

        let locale = self.cached_locale.clone();
        let coming_soon = self.cached_coming_soon.clone();
        let frame_now = chrono::Local::now();
        let today = frame_now.date_naive();
        if self.row_memo_day != Some(today) {
            self.row_memo_day = Some(today);
            self.row_memo.borrow_mut().time_labels.clear();
        }
        let row_memo = self.row_memo.clone();
        let selection = self.selection.clone();
        let list_state = self.list_state.clone();
        let hovered_row = self.hovered_row;
        let context_menu_message = self.context_menu_target.map(|(id, _)| id);
        let avatar_image_cache = self.avatar_image_cache.clone();
        let small_avatar_image_cache = self.small_avatar_image_cache.clone();
        let icon_image_cache = self.icon_image_cache.clone();
        let unread_boundary_id = self.cached_unread_boundary;
        let highlight_id = self.highlight_id;
        let reply_highlight_id = store.read(cx).reply_target().map(|d| d.message_ref_id);
        let profile_context = channel_profile_context(is_dm, dm_channel, active_clan, cx);
        let settings = self.settings.clone();
        let active_videos = self.active_videos.clone();
        let active_audios = self.active_audios.clone();
        let gif_videos = self.gif_videos.clone();
        let embed_inputs = self.embed_inputs.clone();
        let video_host = cx.entity().downgrade();
        let current_user_id = self.cached_current_user_id.clone();
        let role_ids = self.cached_role_ids.clone();
        let welcome = self.welcome.clone();
        let onboarding = self.onboarding.clone();
        let has_more_bottom = store.read(cx).has_more_bottom();
        let show_scroll_down = has_more_bottom
            || !self
                .list_state
                .is_scrolled_to_end()
                .unwrap_or(self.at_bottom);
        let unread_count = self.cached_fab_unread_count;
        let key_listener = cx.listener(Self::on_messages_key);
        let copy_listener = cx.listener(Self::on_copy);
        let focus_on_click = cx.listener(|this, _: &MouseDownEvent, window, cx| {
            if !this.focus_handle.contains_focused(window, cx) {
                window.focus(&this.focus_handle, cx);
            }
        });
        let selection_host = cx.entity().downgrade();
        let selection_state = self.selection.clone();
        let scroll_down_fab = self.scroll_down_fab(show_scroll_down, unread_count, cx);

        let content = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .image_cache(self.image_cache.clone())
            .track_focus(&self.focus_handle)
            .on_action(copy_listener)
            .on_key_down(key_listener)
            .on_mouse_down(MouseButton::Left, focus_on_click)
            .child(
                list(list_state, move |ix, _window, cx| {
                    if header_shown && ix == 0 {
                        return div()
                            .id("msg-loading-top")
                            .py_2()
                            .child(message_skeleton(cx.theme(), 5))
                            .into_any_element();
                    }
                    let msg_ix = ix - usize::from(header_shown);
                    let ctx = RowCtx {
                        app: cx,
                        theme: cx.theme(),
                        locale: &locale,
                        current_user_id: &current_user_id,
                        current_role_ids: &role_ids,
                        welcome: welcome.clone(),
                        onboarding: onboarding.clone(),
                        suppress_hover,
                        is_topic_box: false,
                        scroll_active,
                        hovered_row,
                        context_menu_message,
                        avatar_cache: small_avatar_image_cache.clone(),
                        large_avatar_cache: avatar_image_cache.clone(),
                        icon_cache: icon_image_cache.clone(),
                        unread_boundary_id,
                        highlight_id,
                        reply_highlight_id,
                        profile_context,
                        settings: settings.clone(),
                        active_videos: &active_videos,
                        active_audios: &active_audios,
                        gif_videos: &gif_videos,
                        embed_inputs: &embed_inputs,
                        video_host: video_host.clone(),
                        now: frame_now,
                        clan_id: active_clan,
                        channel_type,
                        channel_top_level,
                        is_clan_owner,
                        editing_id,
                        edit_input: edit_input.clone(),
                        emoji_recent: &emoji_recent,
                        coming_soon: coming_soon.clone(),
                        row_memo: row_memo.clone(),
                        selection: selection.clone(),
                    };
                    render_message_item(store.read(cx).viewport_messages(), msg_ix, &ctx, cx)
                })
                .flex_1()
                .size_full()
                .pb(px(LIST_BOTTOM_PADDING)),
            )
            .children(skeleton_overlay)
            .child(scroll_down_fab)
            .when_some(self.mention_popover.clone(), |el, (popover, position)| {
                el.child(deferred(
                    anchored().position(position).snap_to_window().child(
                        div()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.mention_popover = None;
                                this._mention_popover_sub = None;
                                cx.notify();
                            }))
                            .child(popover),
                    ),
                ))
            })
            .when_some(self.reaction_picker.clone(), |el, (picker, position)| {
                el.child(deferred(
                    anchored()
                        .position(position)
                        .anchor(Anchor::TopRight)
                        .snap_to_window()
                        .child(
                            div()
                                .occlude()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.reaction_picker = None;
                                    this._reaction_picker_sub = None;
                                    this._reaction_picker_dismiss_sub = None;
                                    cx.notify();
                                }))
                                .child(picker),
                        ),
                ))
            })
            .when_some(self.context_menu_target, |el, (message_id, position)| {
                let store = MessagesStore::global(cx);
                let store_ref = store.read(cx);
                let Some(target_msg) = store_ref.viewport_message_by_id(message_id) else {
                    return el;
                };
                let selected_text = {
                    let state = self.selection.borrow();
                    selected_text_for_messages(
                        &state,
                        store_ref.viewport_messages(),
                        &self.cached_locale,
                        &self.cached_current_user_id,
                        cx,
                    )
                };
                let menu = message_context_menu::build(
                    target_msg,
                    &self.cached_current_user_id,
                    self.cached_is_clan_owner,
                    &self.cached_locale,
                    self.context_menu_forward_all,
                    false,
                    self.reaction_submenu_open,
                    selected_text,
                    cx.entity().downgrade(),
                    cx,
                );
                el.child(context_menu_at(position, menu))
            })
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical)
                    .tracked_scroll_handle(&self.list_state),
                window,
                cx,
            );
        SelectionCapture::new(content.into_any_element(), selection_host, selection_state)
            .into_any_element()
    }
}

fn fab_unread_count(
    last_seen_at_bottom: Option<MessageId>,
    ids_newest_first: impl IntoIterator<Item = MessageId>,
) -> u32 {
    let Some(seen) = last_seen_at_bottom else {
        return 0;
    };
    ids_newest_first
        .into_iter()
        .take_while(|id| *id > seen)
        .filter(|id| !id.is_optimistic())
        .count()
        .min(u32::MAX as usize) as u32
}

fn channel_profile_context(
    is_dm: bool,
    dm_channel: Option<ChannelId>,
    active_clan: Option<ClanId>,
    _cx: &Context<ChannelMessages>,
) -> Option<ProfileContext> {
    if is_dm {
        dm_channel.map(ProfileContext::Direct)
    } else {
        active_clan.map(ProfileContext::Clan)
    }
}

pub(crate) fn unread_boundary_for_messages(
    messages: &[Message],
    last_read: Option<MessageId>,
    channel_tail: Option<MessageId>,
    current_user_id: Option<UserId>,
) -> Option<MessageId> {
    let last_read = last_read.filter(|id| !id.is_zero())?;
    if messages.is_empty() {
        return None;
    }
    for i in 1..messages.len() {
        let prev = &messages[i - 1];
        let curr = &messages[i];
        if curr.id.is_optimistic() {
            continue;
        }
        if prev.id != last_read {
            continue;
        }
        if channel_tail.is_some_and(|tail| prev.id == tail) {
            return None;
        }
        if current_user_id.is_some_and(|uid| curr.sender_user_id == Some(uid)) {
            return None;
        }
        return Some(curr.id);
    }
    None
}

fn unread_boundary(
    store: &Entity<MessagesStore>,
    last_seen_at_bottom: Option<MessageId>,
    cx: &Context<ChannelMessages>,
) -> Option<MessageId> {
    let store_read = store.read(cx);
    let messages = store_read.viewport_messages();
    let last_read = last_seen_at_bottom.or_else(|| store_read.last_read_message_id());
    let channel_tail = messages
        .last()
        .filter(|m| !m.id.is_optimistic())
        .map(|m| m.id)
        .or_else(|| store_read.channel_tail_message_id());
    let current_user_id = BadgeService::global(cx).read(cx).current_user_id(cx);
    unread_boundary_for_messages(messages, last_read, channel_tail, current_user_id)
}

const RESET_NEAR_BOTTOM_ROWS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ResetTransition {
    current_channel: Option<ChannelId>,
    restore_pending: Option<ChannelId>,
    want_restore: bool,
}

fn reset_transition(
    _prev_channel: Option<ChannelId>,
    new_channel: Option<ChannelId>,
    _prev_restore_pending: Option<ChannelId>,
    fab_scroll_pending: bool,
    is_loading: bool,
) -> ResetTransition {
    let want_restore = !fab_scroll_pending && new_channel.is_some();
    let restore_pending = if is_loading && want_restore {
        new_channel
    } else {
        None
    };
    ResetTransition {
        current_channel: new_channel,
        restore_pending,
        want_restore,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ResetScroll {
    Restore {
        item_ix: usize,
        offset_in_item: Pixels,
    },
    ToBottom,
    Defer,
}

fn decide_reset_scroll(
    want_restore: bool,
    is_loading: bool,
    anchor: Option<SavedScrollAnchor>,
    messages: &[Message],
    has_more_bottom: bool,
) -> ResetScroll {
    if !want_restore {
        return ResetScroll::ToBottom;
    }
    if is_loading {
        return ResetScroll::Defer;
    }
    let Some(anchor) = anchor.filter(|anchor| !anchor.message_id.is_optimistic()) else {
        return ResetScroll::ToBottom;
    };
    match messages.iter().position(|m| m.id == anchor.message_id) {
        Some(position) => ResetScroll::Restore {
            item_ix: position,
            offset_in_item: anchor.offset_in_item,
        },
        None if has_more_bottom => ResetScroll::Restore {
            item_ix: messages.len().saturating_sub(RESET_NEAR_BOTTOM_ROWS),
            offset_in_item: px(0.),
        },
        None => ResetScroll::ToBottom,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum AnchorUpdate {
    Clear,
    Set(SavedScrollAnchor),
    Keep,
}

fn capture_anchor(
    messages: &[Message],
    at_bottom: bool,
    scroll_top: gpui::ListOffset,
    header_shown: bool,
) -> AnchorUpdate {
    if at_bottom {
        return AnchorUpdate::Clear;
    }
    let header = usize::from(header_shown);
    let message_ix = scroll_top.item_ix.saturating_sub(header);
    let offset_in_item = if scroll_top.item_ix < header {
        px(0.)
    } else {
        scroll_top.offset_in_item
    };
    match messages.get(message_ix) {
        Some(message) if !message.id.is_optimistic() => AnchorUpdate::Set(SavedScrollAnchor {
            message_id: message.id,
            offset_in_item,
        }),
        _ => AnchorUpdate::Keep,
    }
}

#[cfg(test)]
mod skeleton_tests {
    use super::{
        SkeletonInput, SkeletonPhase, SkeletonTimer, TimerAction, skeleton_timer_action,
        skeleton_transition,
    };

    fn sync(loading: bool, same_key: bool) -> SkeletonInput {
        SkeletonInput::Sync { loading, same_key }
    }

    const PHASES: [SkeletonPhase; 5] = [
        SkeletonPhase::Hidden,
        SkeletonPhase::Pending,
        SkeletonPhase::Showing,
        SkeletonPhase::Settling,
        SkeletonPhase::FadingOut,
    ];

    #[test]
    fn fast_load_arms_then_cancels_without_showing() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Hidden, sync(true, true)),
            (SkeletonPhase::Pending, SkeletonTimer::Threshold)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, sync(false, true)),
            (SkeletonPhase::Hidden, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn slow_load_stays_pending_then_threshold_shows() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, sync(true, true)),
            (SkeletonPhase::Pending, SkeletonTimer::Keep)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Pending, SkeletonInput::ThresholdElapsed),
            (SkeletonPhase::Showing, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn empty_but_loaded_leaves_showing_for_settle() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Showing, sync(false, true)),
            (SkeletonPhase::Settling, SkeletonTimer::Settle)
        );
    }

    #[test]
    fn settle_elapsed_starts_fade_out() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed),
            (SkeletonPhase::FadingOut, SkeletonTimer::Unmount)
        );
    }

    #[test]
    fn settle_tick_arms_unmount_timer() {
        let (next, timer) =
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed);
        assert_eq!(next, SkeletonPhase::FadingOut);
        assert!(matches!(
            skeleton_timer_action(timer),
            TimerAction::Arm(SkeletonInput::UnmountElapsed, _)
        ));
    }

    #[test]
    fn timer_action_maps_each_token() {
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Keep),
            TimerAction::Keep
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Drop),
            TimerAction::Clear
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Threshold),
            TimerAction::Arm(SkeletonInput::ThresholdElapsed, _)
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Settle),
            TimerAction::Arm(SkeletonInput::SettleElapsed, _)
        ));
        assert!(matches!(
            skeleton_timer_action(SkeletonTimer::Unmount),
            TimerAction::Arm(SkeletonInput::UnmountElapsed, _)
        ));
    }

    #[test]
    fn data_arrives_settles_then_fades_then_hides() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Showing, sync(false, true)),
            (SkeletonPhase::Settling, SkeletonTimer::Settle)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, SkeletonInput::SettleElapsed),
            (SkeletonPhase::FadingOut, SkeletonTimer::Unmount)
        );
        assert_eq!(
            skeleton_transition(SkeletonPhase::FadingOut, SkeletonInput::UnmountElapsed),
            (SkeletonPhase::Hidden, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn resync_mid_settle_returns_to_showing_without_double_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Settling, sync(true, true)),
            (SkeletonPhase::Showing, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn resync_mid_fade_returns_to_showing_without_double_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::FadingOut, sync(true, true)),
            (SkeletonPhase::Showing, SkeletonTimer::Drop)
        );
    }

    #[test]
    fn channel_switch_resets_every_phase_to_hidden() {
        for phase in PHASES {
            assert_eq!(
                skeleton_transition(phase, sync(false, false)),
                (SkeletonPhase::Hidden, SkeletonTimer::Drop)
            );
        }
    }

    #[test]
    fn channel_switch_into_cold_arms_threshold_from_every_phase() {
        for phase in PHASES {
            assert_eq!(
                skeleton_transition(phase, sync(true, false)),
                (SkeletonPhase::Pending, SkeletonTimer::Threshold)
            );
        }
    }

    #[test]
    fn cached_channel_stays_hidden_without_touching_timer() {
        assert_eq!(
            skeleton_transition(SkeletonPhase::Hidden, sync(false, true)),
            (SkeletonPhase::Hidden, SkeletonTimer::Keep)
        );
    }

    #[test]
    fn elapsed_inputs_are_noops_off_their_phase() {
        for phase in PHASES {
            if phase != SkeletonPhase::Pending {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::ThresholdElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
            if phase != SkeletonPhase::Settling {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::SettleElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
            if phase != SkeletonPhase::FadingOut {
                assert_eq!(
                    skeleton_transition(phase, SkeletonInput::UnmountElapsed),
                    (phase, SkeletonTimer::Keep)
                );
            }
        }
    }

    #[test]
    fn unread_break_hidden_for_own_message_after_last_read() {
        use super::unread_boundary_for_messages;
        use mezon_store::{Message, MessageId, UserId};

        let read = MessageId(10);
        let own = Message::new(MessageId(11), "hi", "1", "me", 0);
        let messages = vec![Message::new(read, "old", "2", "them", 0), own.clone()];
        assert_eq!(
            unread_boundary_for_messages(&messages, Some(read), Some(own.id), Some(UserId(1))),
            None
        );

        let other = Message::new(MessageId(12), "hey", "2", "them", 0);
        let messages = vec![Message::new(read, "old", "2", "them", 0), other.clone()];
        assert_eq!(
            unread_boundary_for_messages(&messages, Some(read), Some(other.id), Some(UserId(1))),
            Some(other.id)
        );
    }

    #[test]
    fn fab_unread_count_is_not_capped_so_badge_can_render_plus() {
        use super::fab_unread_count;
        use mezon_store::MessageId;

        let newest_first: Vec<MessageId> = (1..=5).rev().map(MessageId).collect();
        assert_eq!(
            fab_unread_count(Some(MessageId(0)), newest_first.iter().copied()),
            5
        );
        assert_eq!(
            fab_unread_count(Some(MessageId(3)), newest_first.iter().copied()),
            2
        );
        assert_eq!(
            fab_unread_count(Some(MessageId(5)), newest_first.iter().copied()),
            0
        );
        assert_eq!(fab_unread_count(None, newest_first.iter().copied()), 0);

        let many: Vec<MessageId> = (1..=150).rev().map(MessageId).collect();
        assert!(fab_unread_count(Some(MessageId(0)), many.iter().copied()) > 99);
    }
}

#[cfg(test)]
mod scroll_restore_tests {
    use super::{
        AnchorUpdate, ResetScroll, ResetTransition, SavedScrollAnchor, capture_anchor,
        decide_reset_scroll, reset_transition, saved_message_scroll_anchor, shifted_scroll_anchor,
    };
    use gpui::{ListOffset, px};
    use mezon_store::{ChannelId, Message, MessageId};

    fn rows(ids: &[i64]) -> Vec<Message> {
        ids.iter()
            .map(|&id| Message::new(MessageId(id), "m", "1", "u", 0))
            .collect()
    }

    fn assert_offset(actual: Option<ListOffset>, item_ix: usize, offset_in_item: f32) {
        let actual = actual.unwrap();
        assert_eq!(actual.item_ix, item_ix);
        assert_eq!(actual.offset_in_item, px(offset_in_item));
    }

    fn saved(message_id: i64, offset_in_item: f32) -> SavedScrollAnchor {
        SavedScrollAnchor {
            message_id: MessageId(message_id),
            offset_in_item: px(offset_in_item),
        }
    }

    fn list_offset(item_ix: usize, offset_in_item: f32) -> ListOffset {
        ListOffset {
            item_ix,
            offset_in_item: px(offset_in_item),
        }
    }

    #[test]
    fn restores_at_anchor_position_when_present() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            decide_reset_scroll(true, false, Some(saved(12, 7.)), &messages, false),
            ResetScroll::Restore {
                item_ix: 2,
                offset_in_item: px(7.),
            }
        );
    }

    #[test]
    fn falls_to_bottom_when_anchor_absent_and_at_tail() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, false, Some(saved(99, 0.)), &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn near_bottom_when_anchor_absent_but_more_below() {
        let ids: Vec<i64> = (100..120).collect();
        let messages = rows(&ids);
        assert_eq!(
            decide_reset_scroll(true, false, Some(saved(9999, 9.)), &messages, true),
            ResetScroll::Restore {
                item_ix: 10,
                offset_in_item: px(0.),
            }
        );
    }

    #[test]
    fn falls_to_bottom_when_no_saved_anchor() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, false, None, &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn restores_on_same_channel_refetch() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, false, Some(saved(11, 4.)), &messages, false),
            ResetScroll::Restore {
                item_ix: 1,
                offset_in_item: px(4.),
            }
        );
    }

    #[test]
    fn never_anchors_on_optimistic_message() {
        let mut messages = rows(&[10, 11]);
        let optimistic = MessageId::next_optimistic();
        messages.push(Message::new(optimistic, "m", "1", "u", 0));
        assert_eq!(
            decide_reset_scroll(
                true,
                false,
                Some(SavedScrollAnchor {
                    message_id: optimistic,
                    offset_in_item: px(0.),
                }),
                &messages,
                false,
            ),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn defers_positioning_on_intermediate_loading_reset() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, true, Some(saved(11, 0.)), &messages, false),
            ResetScroll::Defer
        );
    }

    #[test]
    fn loading_without_restore_still_goes_to_bottom() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(false, true, None, &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn capture_clears_anchor_at_bottom() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            capture_anchor(&messages, true, list_offset(2, 8.), false),
            AnchorUpdate::Clear
        );
    }

    #[test]
    fn capture_sets_first_visible_row_when_scrolled_up() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            capture_anchor(&messages, false, list_offset(1, 6.), false),
            AnchorUpdate::Set(saved(11, 6.))
        );
    }

    #[test]
    fn capture_subtracts_header_offset() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            capture_anchor(&messages, false, list_offset(2, 5.), true),
            AnchorUpdate::Set(saved(11, 5.))
        );
    }

    #[test]
    fn capture_keeps_existing_when_row_is_optimistic() {
        let mut messages = rows(&[10, 11]);
        messages.push(Message::new(MessageId::next_optimistic(), "m", "1", "u", 0));
        assert_eq!(
            capture_anchor(&messages, false, list_offset(2, 0.), false),
            AnchorUpdate::Keep
        );
    }

    #[test]
    fn capture_keeps_existing_when_row_out_of_range() {
        let messages = rows(&[10, 11]);
        assert_eq!(
            capture_anchor(&messages, false, list_offset(9, 0.), false),
            AnchorUpdate::Keep
        );
    }

    #[test]
    fn prepend_preserves_existing_row_and_offset() {
        assert_offset(
            shifted_scroll_anchor(
                ListOffset {
                    item_ix: 8,
                    offset_in_item: px(13.),
                },
                30,
                true,
                20,
                0,
            ),
            28,
            13.,
        );
    }

    #[test]
    fn prepend_fallback_keeps_old_first_row_when_loading_header_is_visible() {
        assert_offset(
            shifted_scroll_anchor(
                ListOffset {
                    item_ix: 0,
                    offset_in_item: px(4.),
                },
                30,
                true,
                20,
                0,
            ),
            21,
            0.,
        );
    }

    #[test]
    fn prepend_restores_the_same_message_id_and_offset() {
        let before = rows(&[40, 41, 42, 43]);
        let captured = capture_anchor(&before, false, list_offset(2, 9.), true);
        let AnchorUpdate::Set(anchor) = captured else {
            panic!("expected a message anchor");
        };
        let after = rows(&[20, 21, 22, 40, 41, 42, 43]);
        let message_ix = after
            .iter()
            .position(|message| message.id == anchor.message_id)
            .unwrap();
        let restored = saved_message_scroll_anchor(anchor, message_ix, true);

        assert_eq!(anchor.message_id, MessageId(41));
        assert_eq!(restored.item_ix, 5);
        assert_eq!(restored.offset_in_item, px(9.));
    }

    #[test]
    fn front_trim_preserves_surviving_row_and_offset() {
        assert_offset(
            shifted_scroll_anchor(
                ListOffset {
                    item_ix: 10,
                    offset_in_item: px(9.),
                },
                30,
                true,
                0,
                5,
            ),
            5,
            9.,
        );
    }

    #[test]
    fn front_trim_moves_removed_anchor_to_first_survivor() {
        assert_offset(
            shifted_scroll_anchor(
                ListOffset {
                    item_ix: 3,
                    offset_in_item: px(9.),
                },
                30,
                true,
                0,
                5,
            ),
            1,
            0.,
        );
    }

    const X: Option<ChannelId> = Some(ChannelId(1));
    const Y: Option<ChannelId> = Some(ChannelId(2));
    const Z: Option<ChannelId> = Some(ChannelId(3));

    #[test]
    fn cold_miss_double_reset_arms_then_finalizes() {
        let intermediate = reset_transition(X, Y, None, false, true);
        assert_eq!(
            intermediate,
            ResetTransition {
                current_channel: Y,
                restore_pending: Y,
                want_restore: true,
            }
        );
        let settled = reset_transition(Y, Y, Y, false, false);
        assert_eq!(
            settled,
            ResetTransition {
                current_channel: Y,
                restore_pending: None,
                want_restore: true,
            }
        );
    }

    #[test]
    fn warm_stale_double_reset_keeps_restore_armed_until_settled() {
        let intermediate = reset_transition(X, Y, None, false, true);
        assert!(intermediate.want_restore && intermediate.restore_pending == Y);
        let settled = reset_transition(Y, Y, Y, false, false);
        assert!(settled.want_restore && settled.restore_pending.is_none());
    }

    #[test]
    fn fab_scroll_pending_forces_bottom() {
        let transition = reset_transition(Y, Y, Y, true, false);
        assert!(!transition.want_restore);
        assert_eq!(transition.restore_pending, None);
    }

    #[test]
    fn same_channel_resync_preserves_anchor() {
        let loading = reset_transition(Y, Y, None, false, true);
        assert!(loading.want_restore);
        assert_eq!(loading.restore_pending, Y);
        let settled = reset_transition(Y, Y, None, false, false);
        assert!(settled.want_restore);
        assert_eq!(settled.restore_pending, None);
    }

    #[test]
    fn switching_again_rearms_to_new_channel() {
        let transition = reset_transition(Y, Z, Y, false, true);
        assert_eq!(
            transition,
            ResetTransition {
                current_channel: Z,
                restore_pending: Z,
                want_restore: true,
            }
        );
    }
}
