use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Anchor, Animation, AnimationExt as _, Context, DismissEvent, Entity, Focusable, ListAlignment,
    ListState, Pixels, Point, SharedString, Subscription, Task, Window, anchored, deferred, div,
    ease_in_out, list, prelude::*, px,
};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use mezon_store::{
    BadgeService, ChannelId, ChannelList, ClanId, ClanList, ClanMembersStore, DirectMessageStore,
    Emoji, EmojiStore, MessageCode, MessageId, MessagesEvent, MessagesStore, ProfileContext,
    Settings, UserId, message::Message,
};

use super::audio_player::{AudioActivation, AudioPlayerView};
use super::context::{OnboardingContext, RowCtx, WelcomeContext};
use super::dispatch::render_message_item;
use super::gif_video::GifVideoView;
use super::message_context_menu;
use super::reaction_picker::{ReactionPicker, ReactionPickerEvent};
use super::skeleton::message_skeleton;
use super::system_row::{build_onboarding_context, build_welcome_context};
use super::video_player::{VideoActivation, VideoPlayerView};
use crate::app::shell::Shell;
use crate::chat::mention_input::{MentionInput, MentionInputEvent};
use crate::chat::user_profile_popover::UserProfilePopover;
use crate::components::primitives::{Icon, IconName, context_menu_at};
use crate::image_cache::{
    LruImageCache, MESSAGE_ENTRY_MAX_BYTES, MESSAGE_IMAGE_CACHE_BYTES, MESSAGE_IMAGE_CACHE_CAPACITY,
};
use crate::theme::{ActiveTheme, Theme};

const LOAD_MORE_ITEM_THRESHOLD: usize = 12;
const LIST_OVERDRAW: f32 = 1024.;
const LIST_BOTTOM_PADDING: f32 = 20.;
const SCROLL_HOVER_RELEASE_MS: u64 = 150;
const HOVER_SHOW_DELAY_MS: u64 = 200;
const HOVER_HIDE_DELAY_MS: u64 = 100;
const PAGINATE_THROTTLE: Duration = Duration::from_millis(250);
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

pub struct ChannelMessages {
    pub(crate) list_state: ListState,
    settings: Entity<Settings>,
    image_cache: Entity<LruImageCache>,
    avatar_image_cache: Entity<LruImageCache>,
    small_avatar_image_cache: Entity<LruImageCache>,
    active_videos: HashMap<(MessageId, usize), Entity<VideoPlayerView>>,
    active_audios: HashMap<(MessageId, usize), Entity<AudioPlayerView>>,
    gif_videos: HashMap<(MessageId, usize), Entity<GifVideoView>>,
    cached_for_channel: Option<ChannelId>,
    skeleton_phase: SkeletonPhase,
    skeleton_key: SkeletonKey,
    last_cold_inputs: (Option<ClanId>, bool, bool),
    _channel_list_observe: Subscription,
    _clan_list_observe: Subscription,
    _skeleton_timer: Option<Task<()>>,
    suppress_hover: bool,
    hovered_row: Option<MessageId>,
    raw_hover: Option<MessageId>,
    _hover_show_task: Option<Task<()>>,
    _hover_hide_task: Option<Task<()>>,
    _scroll_relief: Option<Task<()>>,
    last_paginate: Option<Instant>,
    last_scroll_at: Option<Instant>,
    at_bottom: bool,
    last_visible_start: usize,
    header_shown: bool,
    pending_jump: Option<MessageId>,
    highlight_id: Option<MessageId>,
    _highlight_timer: Option<Task<()>>,
    last_seen_at_bottom: Option<MessageId>,
    fab_scroll_pending: bool,
    scroll_anchors: HashMap<ChannelId, MessageId>,
    current_channel: Option<ChannelId>,
    restore_pending: Option<ChannelId>,
    welcome: Option<WelcomeContext>,
    onboarding: Option<OnboardingContext>,
    cached_unread_boundary: Option<MessageId>,
    _clan_members_observe: Subscription,
    _direct_messages_observe: Subscription,
    _window_activation: Option<Subscription>,
    mention_popover: Option<(Entity<UserProfilePopover>, Point<Pixels>)>,
    _mention_popover_sub: Option<Subscription>,
    reaction_picker: Option<(Entity<ReactionPicker>, Point<Pixels>)>,
    _reaction_picker_sub: Option<Subscription>,
    _reaction_picker_dismiss_sub: Option<Subscription>,
    cached_locale: SharedString,
    cached_current_user_id: SharedString,
    cached_role_ids: Rc<Vec<i64>>,
    cached_is_clan_owner: bool,
    identity_inputs: (Option<ClanId>, Option<UserId>),
    edit_input: Option<(MessageId, Entity<MentionInput>)>,
    _edit_input_sub: Option<Subscription>,
    context_menu_target: Option<(MessageId, Point<Pixels>)>,
    context_menu_forward_all: bool,
    emoji_recent: Rc<Vec<Emoji>>,
    _emoji_observe: Subscription,
}

impl ChannelMessages {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |this, settings, cx| {
            this.cached_locale = settings.read(cx).language.clone().into();
            cx.notify();
        })
        .detach();

        let channel_list = ChannelList::global(cx);
        let channel_list_observe = cx.observe(&channel_list, |this, _, cx| this.reconcile_cold(cx));

        let clan_list = ClanList::global(cx);
        let clan_list_observe = cx.observe(&clan_list, |this, _, cx| this.reconcile_cold(cx));

        let clan_members = ClanMembersStore::global(cx);
        let clan_members_observe = cx.observe(&clan_members, |this, _, cx| {
            this.store_identity(Self::compute_identity(cx));
            if this.refresh_derived_state(cx) {
                cx.notify();
            }
        });

        let direct_messages = DirectMessageStore::global(cx);
        let direct_messages_observe = cx.observe(&direct_messages, |this, _, cx| {
            if this.refresh_derived_state(cx) {
                cx.notify();
            }
        });

        let store = MessagesStore::global(cx);
        cx.subscribe(&store, |this, _store, event, cx| {
            let structural = matches!(
                event,
                MessagesEvent::Reset { .. }
                    | MessagesEvent::Shifted { .. }
                    | MessagesEvent::RemovedAt { .. }
            );
            match event {
                MessagesEvent::Reset { count } => {
                    this.reaction_picker = None;
                    this._reaction_picker_sub = None;
                    this._reaction_picker_dismiss_sub = None;
                    this.mention_popover = None;
                    this._mention_popover_sub = None;
                    this.context_menu_target = None;
                    this.suppress_hover = false;
                    this.hovered_row = None;
                    this.raw_hover = None;
                    this._hover_show_task = None;
                    this._hover_hide_task = None;
                    this.edit_input = None;
                    this._edit_input_sub = None;
                    this.active_videos.clear();
                    this.active_audios.clear();
                    this.gif_videos.clear();
                    this.list_state.reset(*count);
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
                        ResetScroll::Restore(item_ix) => {
                            this.list_state.scroll_to(gpui::ListOffset {
                                item_ix,
                                offset_in_item: px(0.),
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
                    let was_at_end = this
                        .list_state
                        .is_scrolled_to_end()
                        .unwrap_or(this.at_bottom);
                    let prev_top = this.list_state.logical_scroll_top();
                    let prev_count = this.list_state.item_count();
                    let h = usize::from(this.header_shown);
                    if *removed_top > 0 {
                        this.list_state.splice(h..h + *removed_top, 0);
                    }
                    if *added_top > 0 {
                        this.list_state.splice(h..h, *added_top);
                    }
                    if *removed_bottom > 0 {
                        let n = this.list_state.item_count();
                        this.list_state
                            .splice(n.saturating_sub(*removed_bottom)..n, 0);
                    }
                    if *added_bottom > 0 {
                        let n = this.list_state.item_count();
                        this.list_state.splice(n..n, *added_bottom);
                    }
                    let following_new =
                        *added_bottom > 0 && was_at_end && !_store.read(cx).has_more_bottom();
                    if following_new {
                        this.list_state.scroll_to_end();
                        this.at_bottom = true;
                        this.sync_channel_seen(cx);
                    } else if *added_top > 0 {
                        let first_real = h + *added_top;
                        if this.list_state.logical_scroll_top().item_ix < first_real {
                            this.list_state.scroll_to(gpui::ListOffset {
                                item_ix: first_real,
                                offset_in_item: px(0.),
                            });
                        }
                    } else if (*added_bottom > 0 || *removed_top > 0)
                        && prev_top.item_ix < prev_count
                    {
                        let (item_ix, offset_in_item) = if prev_top.item_ix < *removed_top {
                            (0, px(0.))
                        } else {
                            (prev_top.item_ix - *removed_top, prev_top.offset_in_item)
                        };
                        this.list_state.scroll_to(gpui::ListOffset {
                            item_ix,
                            offset_in_item,
                        });
                    }
                }
                MessagesEvent::Updated { message_id } => {
                    if let Some(id) = message_id {
                        let vp_index = _store
                            .read(cx)
                            .viewport_messages()
                            .iter()
                            .position(|m| m.id == *id);
                        if let Some(vp_index) = vp_index {
                            let at = usize::from(this.header_shown) + vp_index;
                            if at < this.list_state.item_count() {
                                this.list_state.remeasure_items(at..at + 1);
                                cx.notify();
                            }
                        }
                    }
                }
                MessagesEvent::RemovedAt { index, message_id } => {
                    this.active_videos.retain(|(id, _), _| id != message_id);
                    this.active_audios.retain(|(id, _), _| id != message_id);
                    this.gif_videos.retain(|(id, _), _| id != message_id);
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
                MessagesEvent::ReplyTargetChanged => return,
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
        })
        .detach();

        let list_state = ListState::new(0, ListAlignment::Bottom, px(LIST_OVERDRAW));
        let timeline = cx.weak_entity();
        list_state.set_scroll_handler(move |event, _window, cx| {
            let near_top = event.visible_range.start < LOAD_MORE_ITEM_THRESHOLD
                && event.visible_range.end < event.count;
            let near_bottom = event.visible_range.end + LOAD_MORE_ITEM_THRESHOLD >= event.count
                && event.visible_range.start > 0;
            let at_bottom = event.visible_range.end + LOAD_MORE_ITEM_THRESHOLD >= event.count;
            let visible_start = event.visible_range.start;
            let _ = timeline.update(cx, |this, cx| {
                let range_moved = this.last_visible_start != visible_start;
                this.at_bottom = at_bottom;
                this.last_visible_start = visible_start;

                if range_moved {
                    if this.context_menu_target.take().is_some() {
                        cx.notify();
                    }
                    if this.reaction_picker.take().is_some() {
                        this._reaction_picker_sub = None;
                        this._reaction_picker_dismiss_sub = None;
                        cx.notify();
                    }
                    this._scroll_relief = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(SCROLL_RELIEF_DELAY).await;
                        this.update(cx, |_this, cx| {
                            crate::image_cache::release_freed_memory_to_os(cx);
                        })
                        .ok();
                    }));
                }

                let store_entity = MessagesStore::global(cx);
                if let Some(channel_id) = store_entity.read(cx).active_channel_id() {
                    store_entity.update(cx, |store, _cx| {
                        store.set_viewing_older(channel_id, !at_bottom);
                    });
                    this.capture_scroll_anchor(
                        channel_id,
                        at_bottom,
                        visible_start,
                        store_entity.read(cx).viewport_messages(),
                    );
                }
                if at_bottom && !store_entity.read(cx).has_more_bottom() {
                    this.sync_channel_seen(cx);
                }

                this.last_scroll_at = Some(Instant::now());
                if !this.suppress_hover {
                    this.suppress_hover = true;
                    this.hovered_row = None;
                    this.raw_hover = None;
                    this._hover_show_task = None;
                    this._hover_hide_task = None;
                    cx.notify();
                }

                if !(near_top || near_bottom) {
                    return;
                }
                let now = Instant::now();
                if this
                    .last_paginate
                    .is_some_and(|t| now.duration_since(t) < PAGINATE_THROTTLE)
                {
                    return;
                }
                let store = MessagesStore::global(cx);
                tracing::debug!(
                    near_top,
                    near_bottom,
                    start = event.visible_range.start,
                    end = event.visible_range.end,
                    count = event.count,
                    has_more_bottom = store.read(cx).has_more_bottom(),
                    "timeline pagination trigger"
                );
                if near_top {
                    this.last_paginate = Some(now);
                    store.update(cx, |store, cx| store.scroll_reached_top(cx));
                } else if store.read(cx).has_more_bottom() {
                    this.last_paginate = Some(now);
                    store.update(cx, |store, cx| store.scroll_reached_bottom(cx));
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
        Self {
            list_state,
            settings,
            image_cache,
            avatar_image_cache,
            small_avatar_image_cache,
            active_videos: HashMap::new(),
            active_audios: HashMap::new(),
            gif_videos: HashMap::new(),
            cached_for_channel: None,
            skeleton_phase: SkeletonPhase::Hidden,
            skeleton_key: SkeletonKey::None,
            last_cold_inputs,
            _channel_list_observe: channel_list_observe,
            _clan_list_observe: clan_list_observe,
            _skeleton_timer: None,
            suppress_hover: false,
            hovered_row: None,
            raw_hover: None,
            _hover_show_task: None,
            _hover_hide_task: None,
            _scroll_relief: None,
            last_paginate: None,
            last_scroll_at: None,
            at_bottom: true,
            last_visible_start: 0,
            header_shown: false,
            pending_jump: None,
            highlight_id: None,
            _highlight_timer: None,
            last_seen_at_bottom: None,
            fab_scroll_pending: false,
            scroll_anchors: HashMap::new(),
            current_channel: None,
            restore_pending: None,
            welcome,
            onboarding,
            cached_unread_boundary,
            _clan_members_observe: clan_members_observe,
            _direct_messages_observe: direct_messages_observe,
            _window_activation: None,
            mention_popover: None,
            _mention_popover_sub: None,
            reaction_picker: None,
            _reaction_picker_sub: None,
            _reaction_picker_dismiss_sub: None,
            cached_locale,
            cached_current_user_id,
            cached_role_ids,
            cached_is_clan_owner,
            identity_inputs,
            edit_input: None,
            _edit_input_sub: None,
            context_menu_target: None,
            context_menu_forward_all: false,
            emoji_recent,
            _emoji_observe: emoji_observe,
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
        let (initial_content, initial_spans) = MessagesStore::global(cx)
            .read(cx)
            .viewport_messages()
            .iter()
            .find(|m| m.id == message_id)
            .map(|m| (m.content.clone(), m.spans.clone()))
            .unwrap_or_default();
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
                MentionInputEvent::SendSticker { .. } => {}
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

        let Some((text, content_tokens, _attachments)) = payload else {
            store.update(cx, |store, cx| store.cancel_edit(cx));
            let locale = self.cached_locale.clone();
            Shell::global(cx).update(cx, |shell, cx| {
                shell.confirm_delete_message(message_id, &locale, window, cx);
            });
            cx.notify();
            return;
        };

        let original = store
            .read(cx)
            .viewport_messages()
            .iter()
            .find(|m| m.id == message_id)
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
        let sender_and_poll = {
            let store = MessagesStore::global(cx);
            let store = store.read(cx);
            store
                .messages()
                .iter()
                .find(|m| m.id == message_id)
                .map(|m| (m.sender_id.clone(), m.code == MessageCode::Poll))
        };
        self.context_menu_forward_all = match sender_and_poll {
            Some((sender_id, false)) => {
                message_context_menu::resolve_forward_group(message_id, sender_id.as_str(), cx)
                    .len()
                    > 1
            }
            _ => false,
        };
        self.context_menu_target = Some((message_id, position));
        cx.notify();
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu_target.take().is_some() {
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
        let previous: Vec<_> = self.active_videos.drain().map(|(_, view)| view).collect();
        for view in previous {
            view.update(cx, |video, cx| video.release_textures(window, cx));
        }
        let view = cx.new(|cx| VideoPlayerView::new(activation, window, cx));
        self.active_videos.insert(key, view);
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
        if self.active_audios.len() >= MAX_AUDIO_PLAYERS
            && let Some(evicted) = self.active_audios.keys().next().copied()
        {
            self.active_audios.remove(&evicted);
        }
        let view = cx.new(|cx| AudioPlayerView::new(activation, cx));
        self.active_audios.insert(key, view);
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
        let header = usize::from(self.header_shown);
        let count = self.list_state.item_count();
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
            if let Some(view) = self.gif_videos.remove(&key) {
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
            self.gif_videos.insert(gif.key, view);
            changed = true;
        }

        if changed {
            cx.notify();
        }
    }

    fn reconcile_cold(&mut self, cx: &mut Context<Self>) {
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
        if welcome == self.welcome
            && onboarding == self.onboarding
            && unread == self.cached_unread_boundary
        {
            return false;
        }
        self.welcome = welcome;
        self.onboarding = onboarding;
        self.cached_unread_boundary = unread;
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
        self.last_seen_at_bottom = None;
        self.fab_scroll_pending = false;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        crate::image_cache::release_freed_memory_to_os(cx);
        self.refresh_derived_state(cx);
    }

    fn capture_scroll_anchor(
        &mut self,
        channel_id: ChannelId,
        at_bottom: bool,
        visible_start: usize,
        messages: &[Message],
    ) {
        match capture_anchor(messages, at_bottom, visible_start, self.header_shown) {
            AnchorUpdate::Clear => {
                self.scroll_anchors.remove(&channel_id);
            }
            AnchorUpdate::Set(message_id) => {
                self.scroll_anchors.insert(channel_id, message_id);
            }
            AnchorUpdate::Keep => {}
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
        self.last_seen_at_bottom = Some(last_id);
        store_entity.update(cx, |store, cx| {
            store.note_viewport_seen(last_id, last_create_time, app_focused, cx);
        });
        if self.refresh_derived_state(cx) {
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
            .bottom(px(20.))
            .right(px(12.))
            .size(px(32.))
            .rounded_full()
            .bg(theme.bg_tertiary)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .opacity(if visible { 1.0 } else { 0.0 })
            .when(visible, |el| {
                el.on_click(cx.listener(|this, _event, _window, cx| {
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

impl Render for ChannelMessages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!(
            "ChannelMessages ch={:?}",
            MessagesStore::global(cx).read(cx).active_channel_id()
        );
        self.clear_image_cache_if_channel_changed(window, cx);
        self.image_cache
            .update(cx, |cache, cx| cache.sweep(window, cx));
        cx.defer_in(window, |this, window, cx| {
            this.apply_gif_reconcile(window, cx)
        });
        self.sync_render_identity(cx);

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
        let coming_soon: SharedString = mezon_i18n::t(&locale, "common.comingSoon")
            .to_string()
            .into();
        let frame_now = chrono::Local::now();
        let list_state = self.list_state.clone();
        let suppress_hover = self.suppress_hover;
        let hovered_row = self.hovered_row;
        let avatar_image_cache = self.avatar_image_cache.clone();
        let small_avatar_image_cache = self.small_avatar_image_cache.clone();
        let unread_boundary_id = self.cached_unread_boundary;
        let highlight_id = self.highlight_id;
        let reply_highlight_id = store.read(cx).reply_target().map(|d| d.message_ref_id);
        let profile_context = channel_profile_context(is_dm, dm_channel, active_clan, cx);
        let settings = self.settings.clone();
        let active_videos = self.active_videos.clone();
        let active_audios = self.active_audios.clone();
        let gif_videos = self.gif_videos.clone();
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
        let unread_count = fab_unread_count(
            self.last_seen_at_bottom,
            store.read(cx).channel_tail_message_id(),
        );
        let scroll_down_fab = self.scroll_down_fab(show_scroll_down, unread_count, cx);

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .image_cache(self.image_cache.clone())
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
                        hovered_row,
                        avatar_cache: small_avatar_image_cache.clone(),
                        large_avatar_cache: avatar_image_cache.clone(),
                        unread_boundary_id,
                        highlight_id,
                        reply_highlight_id,
                        profile_context,
                        settings: settings.clone(),
                        active_videos: &active_videos,
                        active_audios: &active_audios,
                        gif_videos: &gif_videos,
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
                    };
                    render_message_item(store.read(cx).viewport_messages(), msg_ix, &ctx, cx)
                })
                .flex_1()
                .size_full()
                .pb(px(LIST_BOTTOM_PADDING)),
            )
            .children(skeleton_overlay)
            .child(scroll_down_fab)
            .on_mouse_move(cx.listener(|this, _event, _window, cx| {
                if this.suppress_hover
                    && this.last_scroll_at.is_none_or(|t| {
                        t.elapsed() >= Duration::from_millis(SCROLL_HOVER_RELEASE_MS)
                    })
                {
                    this.suppress_hover = false;
                    cx.notify();
                }
            }))
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
                let Some(target_msg) = store_ref
                    .viewport_messages()
                    .iter()
                    .find(|m| m.id == message_id)
                else {
                    return el;
                };
                let menu = message_context_menu::build(
                    target_msg,
                    &self.cached_current_user_id,
                    self.cached_is_clan_owner,
                    &self.cached_locale,
                    self.context_menu_forward_all,
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
            )
            .into_any_element()
    }
}

fn fab_unread_count(
    last_seen_at_bottom: Option<MessageId>,
    channel_tail: Option<MessageId>,
) -> u32 {
    let (Some(seen), Some(tail)) = (last_seen_at_bottom, channel_tail) else {
        return 0;
    };
    if tail <= seen || tail.is_optimistic() {
        return 0;
    }
    let diff = (tail.get() >> 22).saturating_sub(seen.get() >> 22);
    diff.clamp(0, i64::from(u32::MAX)) as u32
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
    prev_channel: Option<ChannelId>,
    new_channel: Option<ChannelId>,
    prev_restore_pending: Option<ChannelId>,
    fab_scroll_pending: bool,
    is_loading: bool,
) -> ResetTransition {
    let channel_changed = new_channel != prev_channel;
    let armed = if channel_changed {
        new_channel
    } else {
        prev_restore_pending
    };
    let want_restore = !fab_scroll_pending && new_channel.is_some() && armed == new_channel;
    let restore_pending = if is_loading { armed } else { None };
    ResetTransition {
        current_channel: new_channel,
        restore_pending,
        want_restore,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ResetScroll {
    Restore(usize),
    ToBottom,
    Defer,
}

fn decide_reset_scroll(
    want_restore: bool,
    is_loading: bool,
    anchor: Option<MessageId>,
    messages: &[Message],
    has_more_bottom: bool,
) -> ResetScroll {
    if !want_restore {
        return ResetScroll::ToBottom;
    }
    if is_loading {
        return ResetScroll::Defer;
    }
    let Some(anchor) = anchor.filter(|id| !id.is_optimistic()) else {
        return ResetScroll::ToBottom;
    };
    match messages.iter().position(|m| m.id == anchor) {
        Some(position) => ResetScroll::Restore(position),
        None if has_more_bottom => {
            ResetScroll::Restore(messages.len().saturating_sub(RESET_NEAR_BOTTOM_ROWS))
        }
        None => ResetScroll::ToBottom,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AnchorUpdate {
    Clear,
    Set(MessageId),
    Keep,
}

fn capture_anchor(
    messages: &[Message],
    at_bottom: bool,
    visible_start: usize,
    header_shown: bool,
) -> AnchorUpdate {
    if at_bottom {
        return AnchorUpdate::Clear;
    }
    let message_ix = visible_start.saturating_sub(usize::from(header_shown));
    match messages.get(message_ix) {
        Some(message) if !message.id.is_optimistic() => AnchorUpdate::Set(message.id),
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

        let seen = MessageId(1 << 22);
        let near = MessageId(6 << 22);
        assert_eq!(fab_unread_count(Some(seen), Some(near)), 5);

        let far = MessageId(200 << 22);
        assert!(fab_unread_count(Some(seen), Some(far)) > 99);

        assert_eq!(fab_unread_count(None, Some(far)), 0);
        assert_eq!(fab_unread_count(Some(far), Some(seen)), 0);
    }
}

#[cfg(test)]
mod scroll_restore_tests {
    use super::{
        AnchorUpdate, ResetScroll, ResetTransition, capture_anchor, decide_reset_scroll,
        reset_transition,
    };
    use mezon_store::{ChannelId, Message, MessageId};

    fn rows(ids: &[i64]) -> Vec<Message> {
        ids.iter()
            .map(|&id| Message::new(MessageId(id), "m", "1", "u", 0))
            .collect()
    }

    #[test]
    fn restores_at_anchor_position_when_present() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            decide_reset_scroll(true, false, Some(MessageId(12)), &messages, false),
            ResetScroll::Restore(2)
        );
    }

    #[test]
    fn falls_to_bottom_when_anchor_absent_and_at_tail() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, false, Some(MessageId(99)), &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn near_bottom_when_anchor_absent_but_more_below() {
        let ids: Vec<i64> = (100..120).collect();
        let messages = rows(&ids);
        assert_eq!(
            decide_reset_scroll(true, false, Some(MessageId(9999)), &messages, true),
            ResetScroll::Restore(10)
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
    fn does_not_restore_on_same_channel_refetch() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(false, false, Some(MessageId(11)), &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn never_anchors_on_optimistic_message() {
        let mut messages = rows(&[10, 11]);
        let optimistic = MessageId::next_optimistic();
        messages.push(Message::new(optimistic, "m", "1", "u", 0));
        assert_eq!(
            decide_reset_scroll(true, false, Some(optimistic), &messages, false),
            ResetScroll::ToBottom
        );
    }

    #[test]
    fn defers_positioning_on_intermediate_loading_reset() {
        let messages = rows(&[10, 11, 12]);
        assert_eq!(
            decide_reset_scroll(true, true, Some(MessageId(11)), &messages, false),
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
            capture_anchor(&messages, true, 2, false),
            AnchorUpdate::Clear
        );
    }

    #[test]
    fn capture_sets_first_visible_row_when_scrolled_up() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            capture_anchor(&messages, false, 1, false),
            AnchorUpdate::Set(MessageId(11))
        );
    }

    #[test]
    fn capture_subtracts_header_offset() {
        let messages = rows(&[10, 11, 12, 13]);
        assert_eq!(
            capture_anchor(&messages, false, 2, true),
            AnchorUpdate::Set(MessageId(11))
        );
    }

    #[test]
    fn capture_keeps_existing_when_row_is_optimistic() {
        let mut messages = rows(&[10, 11]);
        messages.push(Message::new(MessageId::next_optimistic(), "m", "1", "u", 0));
        assert_eq!(
            capture_anchor(&messages, false, 2, false),
            AnchorUpdate::Keep
        );
    }

    #[test]
    fn capture_keeps_existing_when_row_out_of_range() {
        let messages = rows(&[10, 11]);
        assert_eq!(
            capture_anchor(&messages, false, 9, false),
            AnchorUpdate::Keep
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
    fn same_channel_resync_does_not_restore() {
        let loading = reset_transition(Y, Y, None, false, true);
        assert!(!loading.want_restore);
        let settled = reset_transition(Y, Y, None, false, false);
        assert!(!settled.want_restore);
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
