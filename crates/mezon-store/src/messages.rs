use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, Rgba, SharedString, Subscription, Task,
};
use mezon_client::transport::{
    ApiActionRow, ApiComponentPayload, ApiEmbed, ApiMessage, ApiMessageComponent,
    ApiMessageContent, OutgoingEmoji as TransportEmoji, OutgoingHashtag as TransportHashtag,
    OutgoingMention as TransportMention, OutgoingReply, detect_markdown, emoji_content_tokens,
    hashtag_content_tokens, markdown_content_tokens, mention_content_tokens,
};
use mezon_client::{
    AppApi, ConnectionStatus, MezonTransport, RealtimeEvent, UploadFile, UploadThumbnail,
    UrlAttachment,
};

use crate::AppConfig;
use crate::KeyedCache;
use crate::account::AccountStore;
use crate::album_layout::{AlbumLayout, calculate_album_layout};
use crate::badge::BadgeService;
use crate::channel::{ChannelEvent, ChannelList, ChannelType};
use crate::clan_members::ClanMembersStore;
use crate::direct::DirectMessageStore;
use crate::message::{
    CallLog, CallLogType, Embed, EmbedAuthor, EmbedField, EmbedFooter, EmbedImage, InvitePreview,
    MentionTarget, Message, MessageAttachment, MessageButton, MessageCode, MessageComponent,
    MessageComponentRow, MessageReference, MessageSelect, MessageSelectOption, MessageSpan,
    OgpPreview, PollAnswerView, PollData, PollDetail, PollLabelSegment, PollVoter, ViewerMedia,
    aggregate_reactions, apply_reaction_event, message_combined_with_prev, message_sort_key,
    parse_spans, reaction_key, recompute_message_grouping, rollback_reaction, sort_messages,
    spans_only_emoji, viewer_highlight_direct,
};
use crate::presign;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MESSAGE_PAGE_LIMIT: u32 = 50;
const DIRECTION_BEFORE: i32 = 3;
const DIRECTION_AFTER: i32 = 1;
/// `Direction_Mode.AROUND_TIMESTAMP` — fetch a window centered on a message
/// (used by jump-to-message when the target is not loaded).
const DIRECTION_AROUND: i32 = 2;
const CHANNEL_TYPE_CHANNEL: i32 = 1;
const CHANNEL_TYPE_THREAD: i32 = 7;
const STICKER_FILETYPE: &str = "sticker";
const MAX_MESSAGES_PER_CHANNEL: usize = 200;
const MAX_CACHED_CHANNELS: usize = 30;
const LAST_SEEN_DEBOUNCE: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug)]
struct PendingLastSeen {
    clan_id: ClanId,
    channel_id: ChannelId,
    message_id: MessageId,
    create_time: i64,
    mode: i32,
    badge_count: u32,
}

#[derive(Debug, Clone)]
pub enum MessagesEvent {
    /// The whole viewport was replaced (channel switch / fetch). `count` is the
    /// new row count.
    Reset {
        count: usize,
    },
    /// The viewport window slid: rows were added/removed at either edge. The UI
    /// applies the matching splices so the visible scroll position is preserved.
    Shifted {
        added_top: usize,
        removed_top: usize,
        added_bottom: usize,
        removed_bottom: usize,
    },
    /// An in-place change to an existing row (e.g. a reaction add/remove) that
    /// does not alter the row count — the UI just needs to re-render.
    /// `message_id` is the affected row when known (lets scoped observers skip
    /// unrelated updates), or `None` for a broad in-place change.
    Updated {
        message_id: Option<MessageId>,
    },
    /// Scroll to and briefly highlight a message that is now in the buffer
    /// (cf. React `idMessageToJump`). Emitted by [`MessagesStore::jump_to_message`]
    /// once the target is loaded — either it was already present, or an
    /// AROUND fetch (which emits `Reset` first) just brought it in.
    JumpTo {
        message_id: MessageId,
    },
    RemovedAt {
        index: usize,
        message_id: MessageId,
    },
    ReplyTargetChanged,
}

/// The message currently being replied to (composer state), mirroring React's
/// reply reference draft in `references.slice`.
#[derive(Debug, Clone)]
pub struct ReplyDraft {
    pub message_ref_id: MessageId,
    pub sender_id: UserId,
    pub sender_name: String,
    pub sender_avatar: String,
    pub content_preview: String,
    pub has_attachment: bool,
    pub has_embed: bool,
    pub is_poll: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingMention {
    pub user_id: String,
    pub role_id: String,
    pub display: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingMention {
    fn into_transport(self) -> TransportMention {
        TransportMention {
            user_id: self.user_id,
            role_id: self.role_id,
            username: self.display,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingHashtag {
    pub channel_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingHashtag {
    fn into_transport(self) -> TransportHashtag {
        TransportHashtag {
            channel_id: self.channel_id,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingEmoji {
    pub emoji_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingEmoji {
    fn into_transport(self) -> TransportEmoji {
        TransportEmoji {
            emoji_id: self.emoji_id,
            s: self.s,
            e: self.e,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutgoingContent {
    pub mentions: Vec<OutgoingMention>,
    pub hashtags: Vec<OutgoingHashtag>,
    pub emojis: Vec<OutgoingEmoji>,
}

impl OutgoingContent {
    pub fn is_empty(&self) -> bool {
        self.mentions.is_empty() && self.hashtags.is_empty() && self.emojis.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct OutgoingAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub filetype: String,
    pub width: i32,
    pub height: i32,
    pub poster_jpeg: Option<Vec<u8>>,
}

#[derive(Default)]
struct MessageList {
    items: Vec<Message>,
    index: HashMap<MessageId, usize>,
    temp_ids: Vec<MessageId>,
}

impl MessageList {
    fn from_messages(items: Vec<Message>) -> Self {
        let mut list = Self {
            items,
            index: HashMap::new(),
            temp_ids: Vec::new(),
        };
        list.reindex();
        list
    }

    fn reindex(&mut self) {
        self.index.clear();
        self.index.reserve(self.items.len());
        self.temp_ids.clear();
        for (i, m) in self.items.iter().enumerate() {
            self.index.insert(m.id, i);
            if m.id.is_optimistic() {
                self.temp_ids.push(m.id);
            }
        }
    }

    fn as_slice(&self) -> &[Message] {
        &self.items
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn first(&self) -> Option<&Message> {
        self.items.first()
    }

    fn last(&self) -> Option<&Message> {
        self.items.last()
    }

    fn contains_id(&self, id: MessageId) -> bool {
        self.index.contains_key(&id)
    }

    fn position(&self, id: MessageId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    fn get_by_id(&self, id: MessageId) -> Option<&Message> {
        let idx = *self.index.get(&id)?;
        self.items.get(idx)
    }

    fn get_mut_by_id(&mut self, id: MessageId) -> Option<&mut Message> {
        let idx = *self.index.get(&id)?;
        self.items.get_mut(idx)
    }

    fn temp_match_position(&self, sender_id: &str, content: &str) -> Option<usize> {
        self.temp_ids.iter().find_map(|temp_id| {
            let idx = *self.index.get(temp_id)?;
            let candidate = &self.items[idx];
            if candidate.send_failed {
                return None;
            }
            if candidate.content != content {
                return None;
            }
            let sender_match =
                candidate.sender_id == sender_id || sender_id.is_empty() || sender_id == "0";
            sender_match.then_some(idx)
        })
    }

    fn replace(&mut self, items: Vec<Message>) {
        self.items = items;
        self.reindex();
    }

    fn push_sorted(&mut self, msg: Message) {
        self.items.push(msg);
        sort_messages(&mut self.items);
        trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
    }

    fn push_grouped(&mut self, msg: Message) {
        let in_order = self
            .items
            .last()
            .map(|last| message_sort_key(last) <= message_sort_key(&msg))
            .unwrap_or(true);
        if !in_order {
            self.items.push(msg);
            sort_messages(&mut self.items);
            trim_messages(&mut self.items);
            recompute_message_grouping(&mut self.items);
            self.reindex();
            return;
        }
        self.items.push(msg);
        let dropped = self.items.len().saturating_sub(MAX_MESSAGES_PER_CHANNEL);
        if dropped > 0 {
            let evicted_temp_ids: Vec<MessageId> = self.items[..dropped]
                .iter()
                .filter(|m| m.id.is_optimistic())
                .map(|m| m.id)
                .collect();
            for evicted in self.items[..dropped].iter() {
                self.index.remove(&evicted.id);
            }
            self.items.drain(0..dropped);
            for position in self.index.values_mut() {
                *position -= dropped;
            }
            self.temp_ids.retain(|t| !evicted_temp_ids.contains(t));
        }
        let last_idx = self.items.len() - 1;
        let new_id = self.items[last_idx].id;
        self.index.insert(new_id, last_idx);
        if new_id.is_optimistic() {
            self.temp_ids.push(new_id);
        }
        self.regroup_row(last_idx);
        if dropped > 0 {
            self.regroup_row(0);
        }
    }

    fn regroup_row(&mut self, idx: usize) {
        let combined = {
            let prev = idx.checked_sub(1).map(|p| &self.items[p]);
            message_combined_with_prev(prev, &self.items[idx])
        };
        self.items[idx].combined_with_prev = combined;
    }

    fn replace_at(&mut self, idx: usize, msg: Message) {
        let old_id = self.items[idx].id;
        let new_id = msg.id;
        self.items[idx] = msg;
        if old_id == new_id {
            return;
        }
        self.index.remove(&old_id);
        self.index.insert(new_id, idx);
        if old_id.is_optimistic() {
            self.temp_ids.retain(|t| *t != old_id);
        }
        if new_id.is_optimistic() {
            self.temp_ids.push(new_id);
        }
    }

    fn replace_at_and_regroup(&mut self, idx: usize, msg: Message) {
        self.replace_at(idx, msg);
        recompute_message_grouping(&mut self.items);
    }

    /// Merge a re-delivered/echoed copy of an already-present message in place,
    /// then recompute grouping. The incoming echo is a fresh row whose derived
    /// `combined_with_prev` defaults to `false`; without the regroup the avatar/
    /// name head would re-appear on the just-sent row until the next mutation.
    fn merge_existing(&mut self, id: MessageId, incoming: Message) -> bool {
        let Some(existing) = self.get_by_id(id).cloned() else {
            return false;
        };
        let merged = merge_sparse_sender(&existing, incoming);
        if let Some(slot) = self.get_mut_by_id(id) {
            *slot = merged;
        }
        recompute_message_grouping(&mut self.items);
        true
    }

    fn replace_resort(&mut self, idx: usize, msg: Message) {
        self.items[idx] = msg;
        sort_messages(&mut self.items);
        trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
    }

    fn prepend_older(&mut self, mut older: Vec<Message>) -> usize {
        older.append(&mut self.items);
        sort_messages(&mut older);
        let dropped_bottom = trim_messages_back(&mut older);
        self.items = older;
        recompute_message_grouping(&mut self.items);
        self.reindex();
        dropped_bottom
    }

    fn append_newer(&mut self, mut newer: Vec<Message>) -> usize {
        self.items.append(&mut newer);
        sort_messages(&mut self.items);
        let dropped = trim_messages(&mut self.items);
        recompute_message_grouping(&mut self.items);
        self.reindex();
        dropped
    }

    fn remove_id(&mut self, id: MessageId) -> Option<usize> {
        let idx = self.index.get(&id).copied()?;
        self.items.remove(idx);
        recompute_message_grouping(&mut self.items);
        self.reindex();
        Some(idx)
    }
}

struct ChannelMessages {
    messages: MessageList,
    /// More history exists above (older). Mirrors React `hasMoreTop`.
    has_more: bool,
}

const STREAM_MODE_CHANNEL: i32 = 2;
const STREAM_MODE_THREAD: i32 = 6;

pub struct MessagesStore {
    cache: KeyedCache<ChannelId, ChannelMessages>,
    /// Channel tail message id (React `lastMessageByChannel`), keyed by parent channel.
    last_message_by_channel: HashMap<ChannelId, MessageId>,
    /// Last read message id per channel (React `unreadMessagesEntries`). The "New
    /// messages" break renders after this id.
    last_read_message_by_channel: HashMap<ChannelId, MessageId>,
    /// User scrolled away from the bottom (React `isViewingOlderMessagesByChannelId`).
    viewing_older_by_channel: HashMap<ChannelId, bool>,
    active_channel_id: Option<ChannelId>,
    active_clan_id: Option<ClanId>,
    pending_jump: Option<(ChannelId, MessageId)>,
    is_public: bool,
    is_dm: bool,
    mode: i32,
    loading: bool,
    loading_more: bool,
    /// Throttle state for older-history paging: when the backend answers very
    /// fast (<100ms) and the user flings the scrollbar, back off progressively
    /// so we don't blast through the whole history (cf. React `handleOnChange`).
    last_load_more: Option<Instant>,
    consecutive_loads: u32,
    fetch_generation: u64,
    reset_generation: u64,
    /// Active reply target for the composer, if any.
    reply_target: Option<ReplyDraft>,
    /// Message currently being edited inline in its row (self-only; one at a time).
    editing: Option<MessageId>,
    joined_channels: HashSet<ChannelId>,
    pending_self_adds: HashMap<(ChannelId, MessageId, String), u32>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _conn_watch: Task<()>,
    pending_last_seen: Option<PendingLastSeen>,
    _last_seen_timer: Option<Task<()>>,
    last_seen_fingerprint: HashMap<ChannelId, String>,
    queued_last_seen: Vec<PendingLastSeen>,
    /// Transient per-poll UI state (selected answers, results toggle, in-flight
    /// vote), keyed by poll message id — mezon-react component-local state.
    poll_ui: HashMap<MessageId, PollUiState>,
    /// My submitted answer indices per poll (React `pollsSlice.myVote`), set from
    /// `VotePollResponse.my_answer_indices`.
    poll_my_vote: HashMap<MessageId, Vec<i32>>,
    select_ui: HashMap<MessageId, HashMap<SharedString, Vec<SharedString>>>,
}

#[derive(Debug, Clone)]
pub struct ForwardTarget {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub channel_type: i32,
    pub mode: i32,
    pub is_public: bool,
}

#[derive(Debug, Clone)]
struct ForwardPlan {
    clan_id: i64,
    channel_id: i64,
    channel_type: i32,
    mode: i32,
    is_public: bool,
    content: String,
    attachments: Vec<mezon_client::transport::ApiAttachment>,
    note: Option<String>,
}

fn attachment_to_api(a: &MessageAttachment) -> mezon_client::transport::ApiAttachment {
    mezon_client::transport::ApiAttachment {
        url: a.url.clone(),
        filename: a.filename.clone(),
        filetype: a.filetype.clone(),
        width: i32::try_from(a.width).unwrap_or(i32::MAX),
        height: i32::try_from(a.height).unwrap_or(i32::MAX),
        thumbnail: a.thumbnail.clone(),
        duration: a.duration,
        size: i32::try_from(a.size).unwrap_or(i32::MAX),
    }
}

fn plan_forwards(
    content: &str,
    attachments: &[mezon_client::transport::ApiAttachment],
    targets: &[ForwardTarget],
    note: Option<&str>,
) -> Vec<ForwardPlan> {
    targets
        .iter()
        .map(|target| ForwardPlan {
            clan_id: target.clan_id.get(),
            channel_id: target.channel_id.get(),
            channel_type: target.channel_type,
            mode: target.mode,
            is_public: target.is_public,
            content: content.to_string(),
            attachments: attachments.to_vec(),
            note: note.map(str::to_string),
        })
        .collect()
}

/// Transient UI state for a single poll card.
#[derive(Debug, Default, Clone)]
pub struct PollUiState {
    pub selected: Vec<i32>,
    pub show_results: bool,
    pub voting: bool,
}

struct GlobalMessagesStore(Entity<MessagesStore>);
impl Global for GlobalMessagesStore {}

impl EventEmitter<MessagesEvent> for MessagesStore {}

impl MessagesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalMessagesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMessagesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMessagesStore>().map(|g| g.0.clone())
    }

    pub fn debug_memory(&self) -> (usize, usize) {
        let mut channels = 0;
        let mut messages = 0;
        for (_, channel) in self.cache.iter() {
            channels += 1;
            messages += channel.messages.len();
        }
        (channels, messages)
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
        self.cache.clear();
        self.last_message_by_channel.clear();
        self.last_read_message_by_channel.clear();
        self.viewing_older_by_channel.clear();
        self.active_channel_id = None;
        self.active_clan_id = None;
        self.is_public = true;
        self.is_dm = false;
        self.mode = STREAM_MODE_CHANNEL;
        self.loading = false;
        self.loading_more = false;
        self.last_load_more = None;
        self.consecutive_loads = 0;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.reply_target = None;
        self.editing = None;
        self.joined_channels.clear();
        self.pending_self_adds.clear();
        self.pending_last_seen = None;
        self.last_seen_fingerprint.clear();
        self.queued_last_seen.clear();
        self.poll_ui.clear();
        self.poll_my_vote.clear();
        self.select_ui.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(channel_id) = event {
                this.on_active_channel_changed(*channel_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            last_message_by_channel: HashMap::new(),
            last_read_message_by_channel: HashMap::new(),
            viewing_older_by_channel: HashMap::new(),
            active_channel_id: None,
            active_clan_id: None,
            pending_jump: None,
            is_public: true,
            is_dm: false,
            mode: STREAM_MODE_CHANNEL,
            loading: false,
            loading_more: false,
            last_load_more: None,
            consecutive_loads: 0,
            fetch_generation: 0,
            reset_generation: 0,
            reply_target: None,
            editing: None,
            joined_channels: HashSet::new(),
            pending_self_adds: HashMap::new(),
            api,
            _channel_sub: channel_sub,
            _conn_watch: conn_watch,
            pending_last_seen: None,
            _last_seen_timer: None,
            last_seen_fingerprint: HashMap::new(),
            queued_last_seen: Vec::new(),
            poll_ui: HashMap::new(),
            poll_my_vote: HashMap::new(),
            select_ui: HashMap::new(),
        }
    }

    /// Register realtime handlers with the central dispatcher (cf. `add_message_handler`).
    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on(RealtimeKind::MessageReaction, &entity, |this, event, cx| {
                this.handle_reaction(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| this.resync(cx));
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    if this
                        .update(cx, |this, cx| {
                            this.resync(cx);
                            this.flush_queued_last_seen(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    /// Full message buffer for the active channel (internal cache; may be large).
    pub fn messages(&self) -> &[Message] {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.messages.as_slice())
            .unwrap_or(&[])
    }

    pub fn last_cached_message(&self, channel_id: &str) -> Option<&Message> {
        let channel_id = channel_id.parse::<ChannelId>().ok()?;
        self.cache
            .get(&channel_id)
            .and_then(|channel| channel.messages.last())
    }

    /// The messages exposed to the UI. The buffer is already bounded to
    /// `MAX_MESSAGES_PER_CHANNEL` — it *is* the sliding window — and `gpui::list`
    /// virtualizes painting, so the UI mirrors the whole buffer 1:1. Older/newer
    /// rows enter and leave the buffer as the user pages (cf. React's bounded
    /// `selectMessageViewportIdsByChannelId`).
    pub fn viewport_messages(&self) -> &[Message] {
        self.messages()
    }

    pub fn message_in_channel(
        &self,
        channel_id: ChannelId,
        message_id: MessageId,
    ) -> Option<&Message> {
        self.cache.get(&channel_id)?.messages.get_by_id(message_id)
    }

    pub fn reaction_view(
        &self,
        message_id: MessageId,
        emoji_id: &str,
        emoji: &str,
    ) -> Option<(u32, Vec<(String, u32)>)> {
        let channel_id = self.active_channel_id?;
        let msg = self
            .cache
            .get(&channel_id)?
            .messages
            .get_by_id(message_id)?;
        let key = reaction_key(emoji_id, emoji);
        let reaction = msg.reactions.iter().find(|r| r.key == key)?;
        Some((
            reaction.count(),
            reaction
                .senders
                .iter()
                .map(|s| (s.sender_id.clone(), s.count))
                .collect(),
        ))
    }

    /// Emit the splice for a single row appended at the bottom, accounting for
    /// any front-trim that dropped the oldest rows to keep the buffer within the
    /// cap. `old_len` is the buffer length before the push.
    fn emit_appended(&mut self, old_len: usize, cx: &mut Context<Self>) {
        let new_len = self.messages().len();
        if new_len < old_len {
            cx.emit(MessagesEvent::Updated { message_id: None });
            cx.notify();
            return;
        }
        let removed_top = (old_len + 1).saturating_sub(new_len);
        cx.emit(MessagesEvent::Shifted {
            added_top: 0,
            removed_top,
            added_bottom: 1,
            removed_bottom: 0,
        });
        cx.notify();
    }

    /// Called by the timeline when the user scrolls to the top: fetch the next
    /// older page from the server. The buffer is the whole window, so there is
    /// no local "reveal" step — reaching the top always pages over the network.
    pub fn scroll_reached_top(&mut self, cx: &mut Context<Self>) {
        if self.active_channel_id.is_none() {
            return;
        }
        self.load_more(cx);
    }

    /// Batch-update channel tail ids from channel list fetch (React
    /// `setManyLastMessages`).
    pub fn set_many_last_messages(
        &mut self,
        entries: impl IntoIterator<Item = (ChannelId, MessageId)>,
    ) {
        for (channel_id, message_id) in entries {
            self.set_last_message(channel_id, message_id);
        }
    }

    fn set_last_message(&mut self, channel_id: ChannelId, message_id: MessageId) {
        if message_id.is_zero() || message_id.is_optimistic() {
            return;
        }
        self.last_message_by_channel.insert(channel_id, message_id);
    }

    /// Mirrors React `setViewingOlder` — when true, live WS messages only update
    /// `lastMessageByChannel`, not the loaded buffer.
    pub fn set_viewing_older(&mut self, channel_id: ChannelId, viewing: bool) {
        if viewing {
            self.viewing_older_by_channel.insert(channel_id, true);
        } else {
            self.viewing_older_by_channel.remove(&channel_id);
        }
    }

    fn is_viewing_older(&self, storage_id: ChannelId) -> bool {
        self.viewing_older_by_channel
            .get(&storage_id)
            .copied()
            .unwrap_or(false)
    }

    /// Latest known channel tail id (from channel list / WS / send). Used by the
    /// scroll-down FAB unread badge (cf. React `selectLatestMessageId`).
    pub fn channel_tail_message_id(&self) -> Option<MessageId> {
        let channel_id = self.active_channel_id?;
        self.last_message_by_channel.get(&channel_id).copied()
    }

    /// Last read message for the active channel (React `selectUnreadMessageIdByChannelId`).
    pub fn last_read_message_id(&self) -> Option<MessageId> {
        let channel_id = self.active_channel_id?;
        self.last_read_message_by_channel.get(&channel_id).copied()
    }

    pub fn set_last_read_message(&mut self, channel_id: ChannelId, message_id: MessageId) {
        if message_id.is_zero() || message_id.is_optimistic() {
            self.last_read_message_by_channel.remove(&channel_id);
            return;
        }
        self.last_read_message_by_channel
            .insert(channel_id, message_id);
    }

    pub fn clear_last_read_message(&mut self, channel_id: ChannelId) {
        self.last_read_message_by_channel.remove(&channel_id);
    }

    /// Schedule a last-seen write when the viewport tail is visible (cf. React
    /// `useChannelSeen` + `updateLastSeenMessage`).
    pub fn note_viewport_seen(
        &mut self,
        message_id: MessageId,
        create_time: i64,
        app_focused: bool,
        cx: &mut Context<Self>,
    ) {
        if !app_focused || message_id.is_optimistic() {
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        if !should_write_last_seen(
            self.known_last_seen_id(channel_id, cx),
            self.last_message_by_channel.get(&channel_id).copied(),
            message_id,
        ) {
            return;
        }
        let badge_count = self.channel_badge_count(channel_id, clan_id, cx);
        self.pending_last_seen = Some(PendingLastSeen {
            clan_id,
            channel_id,
            message_id,
            create_time,
            mode: self.mode,
            badge_count,
        });
        self.arm_last_seen_debounce(cx);
    }

    fn known_last_seen_id(&self, channel_id: ChannelId, cx: &App) -> Option<MessageId> {
        self.last_read_message_by_channel
            .get(&channel_id)
            .copied()
            .or_else(|| {
                ChannelList::global(cx)
                    .read(cx)
                    .find_channel_in_active_clan(channel_id)
                    .map(|ch| ch.last_seen_message_id)
            })
            .filter(|id| !id.is_zero())
    }

    fn channel_badge_count(&self, channel_id: ChannelId, clan_id: ClanId, cx: &App) -> u32 {
        if self.is_dm {
            DirectMessageStore::global(cx)
                .read(cx)
                .find(channel_id)
                .map(|c| c.unread_count)
                .unwrap_or(0)
        } else {
            ChannelList::global(cx)
                .read(cx)
                .channel(clan_id, channel_id)
                .map(|c| c.badge_count)
                .unwrap_or(0)
        }
    }

    fn arm_last_seen_debounce(&mut self, cx: &mut Context<Self>) {
        if self.pending_last_seen.is_none() {
            return;
        }
        self._last_seen_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LAST_SEEN_DEBOUNCE).await;
            this.update(cx, |this, cx| this.flush_pending_last_seen(cx))
                .ok();
        }));
    }

    fn flush_pending_last_seen(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_last_seen.take() else {
            return;
        };
        self._last_seen_timer = None;
        self.send_last_seen(pending, cx);
    }

    fn flush_queued_last_seen(&mut self, cx: &mut Context<Self>) {
        if self.api.connection_status() != ConnectionStatus::Connected {
            return;
        }
        let queue = std::mem::take(&mut self.queued_last_seen);
        for pending in queue {
            self.send_last_seen(pending, cx);
        }
    }

    fn send_last_seen(&mut self, pending: PendingLastSeen, cx: &mut Context<Self>) {
        let fingerprint = format!(
            "{}|{}|{}|{}|{}",
            pending.clan_id.get(),
            pending.mode,
            pending.badge_count,
            pending.create_time,
            pending.message_id.get()
        );
        if self.last_seen_fingerprint.get(&pending.channel_id) == Some(&fingerprint) {
            return;
        }

        if self.api.connection_status() != ConnectionStatus::Connected {
            self.queued_last_seen.push(pending);
            return;
        }

        self.apply_local_last_seen(&pending, cx);
        self.last_seen_fingerprint
            .insert(pending.channel_id, fingerprint);

        let api = self.api.clone();
        let clan_id = pending.clan_id.get();
        let channel_id = pending.channel_id.get();
        let message_id = pending.message_id.get();
        let mode = pending.mode;
        let ts = pending.create_time.max(0);
        let timestamp_seconds = u32::try_from(ts).unwrap_or(u32::MAX);
        let badge_count = i32::try_from(pending.badge_count).unwrap_or(i32::MAX);
        let generation = self.reset_generation;

        cx.spawn(async move |this, cx| {
            let result = api
                .write_last_seen_message(
                    clan_id,
                    channel_id,
                    message_id,
                    mode,
                    timestamp_seconds,
                    badge_count,
                )
                .await;
            if let Err(e) = result {
                tracing::warn!(
                    channel_id,
                    message_id,
                    "write_last_seen_message failed: {e}"
                );
                this.update(cx, |this, _| {
                    if this.reset_generation != generation {
                        return;
                    }
                    this.last_seen_fingerprint.remove(&ChannelId(channel_id));
                    this.queued_last_seen.push(PendingLastSeen {
                        clan_id: ClanId(clan_id),
                        channel_id: ChannelId(channel_id),
                        message_id: MessageId(message_id),
                        create_time: ts,
                        mode,
                        badge_count: badge_count.max(0) as u32,
                    });
                })
                .ok();
            }
        })
        .detach();
    }

    fn apply_local_last_seen(&mut self, pending: &PendingLastSeen, cx: &mut Context<Self>) {
        self.set_last_read_message(pending.channel_id, pending.message_id);
        let ts = pending.create_time.max(0);
        if self.is_dm {
            DirectMessageStore::global(cx).update(cx, |dm, cx| {
                let _ = dm.note_read(pending.channel_id, cx);
            });
        } else if !pending.clan_id.is_zero() {
            let clan_id = pending.clan_id;
            let channel_id = pending.channel_id;
            ChannelList::global(cx).update(cx, |cl, cx| {
                cl.note_channel_message(
                    clan_id,
                    channel_id,
                    false,
                    true,
                    ts,
                    pending.message_id,
                    cx,
                );
                cl.apply_read(clan_id, channel_id, cx);
            });
        }
    }

    /// True when the channel tail is not in the loaded buffer (jump-to-message
    /// or cap trimmed the newest rows). Scroll UX uses `at_bottom` in the UI.
    pub fn has_more_bottom(&self) -> bool {
        let Some(channel_id) = self.active_channel_id else {
            return false;
        };
        let Some(channel) = self.cache.get(&channel_id) else {
            return false;
        };
        has_more_bottom_for(
            self.last_message_by_channel.get(&channel_id).copied(),
            &channel.messages,
        )
    }

    /// Called by the timeline when the user scrolls to the bottom: fetch the
    /// next newer page from the server (only relevant after a jump-to-message,
    /// when the newest message is not loaded). This is a network load — there is
    /// no local "reveal newer", since in normal flow the newest is always shown.
    pub fn scroll_reached_bottom(&mut self, cx: &mut Context<Self>) {
        tracing::debug!(
            has_more_bottom = self.has_more_bottom(),
            "scroll_reached_bottom"
        );
        self.load_more_bottom(cx);
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn active_channel_id(&self) -> Option<ChannelId> {
        self.active_channel_id
    }

    pub fn is_dm(&self) -> bool {
        self.is_dm
    }

    /// True while an older-history (load-more) fetch is in flight.
    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    fn channel_has_more(&self) -> bool {
        self.active_channel_id
            .as_ref()
            .and_then(|id| self.cache.get(id))
            .map(|c| c.has_more)
            .unwrap_or(false)
    }

    /// True while there is more history to show above the current viewport —
    /// either cached rows not yet revealed, or older pages still on the server.
    /// Mirrors React `selectHasMoreMessageByChannelId` (drives the persistent
    /// top loading skeleton).
    pub fn has_more_top(&self) -> bool {
        self.channel_has_more()
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.loading {
            // Guard against duplicate fetches while one is already in flight
            // (cf. React `debounce`/loadingStatus). Logged to verify no dup call.
            tracing::debug!(
                loading_more = self.loading_more,
                loading = self.loading,
                "load_more skipped: fetch already in flight"
            );
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        if !channel.has_more {
            return;
        }
        let Some(oldest_id) = channel
            .messages
            .first()
            .map(|m| m.id)
            .filter(|id| !id.is_optimistic())
        else {
            return;
        };

        // Progressive backoff (cf. React `handleOnChange`): if loads keep firing
        // in quick succession (the user is flinging the scrollbar and the
        // backend answers in <100ms), delay each successive fetch a bit more so
        // we don't auto-page through the whole channel. Resets once the user
        // pauses for >300ms.
        let now = Instant::now();
        let rapid = self
            .last_load_more
            .map(|t| now.duration_since(t) < Duration::from_millis(300))
            .unwrap_or(false);
        self.consecutive_loads = if rapid {
            (self.consecutive_loads + 1).min(3)
        } else {
            0
        };
        self.last_load_more = Some(now);
        let backoff = Duration::from_millis(u64::from(self.consecutive_loads) * 333);

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            before_message_id = oldest_id.get(),
            backoff_ms = backoff.as_millis() as u64,
            "load_more: fetching older page"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if !backoff.is_zero() {
                cx.background_executor().timer(backoff).await;
            }
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    oldest_id.get(),
                    DIRECTION_BEFORE,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(page) => page.messages,
                    Err(e) => {
                        tracing::error!("Failed to load more messages for {channel_id}: {e}");
                        cx.notify();
                        return;
                    }
                };
                tracing::debug!(
                    channel_id = channel_id.get(),
                    fetched = msgs.len(),
                    "load_more: page received"
                );
                let cfg = AppConfig::try_global(cx);
                let viewer_id = viewer_user_id(cx);
                let (prepended, dropped_bottom) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let older: Vec<Message> = msgs
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(MessageId(m.message_id)))
                        .map(|m| message_from_api(m, cfg, viewer_id))
                        .collect();
                    if older.is_empty() {
                        channel.has_more = false;
                        // No more history above: tell the UI so it can drop the
                        // persistent top loading skeleton.
                        cx.emit(MessagesEvent::Updated { message_id: None });
                        cx.notify();
                        return;
                    }
                    let prepended = older.len();
                    let dropped_bottom = channel.messages.prepend_older(older);
                    // Reached the channel start once the oldest row is the
                    // FIRST_MESSAGE sentinel (cf. React `hasMore` check).
                    channel.has_more = has_more_from_oldest(channel.messages.as_slice());
                    (prepended, dropped_bottom)
                };
                if this.active_channel_id == Some(channel_id) {
                    // Older rows were prepended; the cap may have dropped the same
                    // many newest rows off the back. Emit the exact splice so the
                    // UI window matches the buffer 1:1 — the prepend re-anchors to
                    // the prior first row, and the back-trim removes off-screen
                    // rows below.
                    cx.emit(MessagesEvent::Shifted {
                        added_top: prepended,
                        removed_top: 0,
                        added_bottom: 0,
                        removed_bottom: dropped_bottom,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the next newer page from the server and append it (the bottom
    /// counterpart of [`Self::load_more`]). Active when the channel tail is not
    /// yet in the loaded buffer (React `loadMoreMessage` AFTER_TIMESTAMP).
    pub fn load_more_bottom(&mut self, cx: &mut Context<Self>) {
        if self.loading_more || self.loading {
            tracing::debug!(
                loading_more = self.loading_more,
                loading = self.loading,
                "load_more_bottom skipped: fetch already in flight"
            );
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        let last_channel_id = self.last_message_by_channel.get(&channel_id).copied();
        let newest_loaded = channel
            .messages
            .last()
            .map(|m| m.id)
            .filter(|id| !id.is_optimistic());
        let can_load = match (last_channel_id, newest_loaded) {
            (Some(last), Some(newest)) => last != newest,
            _ => false,
        };
        if !can_load || !has_more_bottom_for(last_channel_id, &channel.messages) {
            tracing::debug!("load_more_bottom skipped: at channel tail");
            return;
        }
        let Some(newest_id) = newest_loaded else {
            tracing::debug!("load_more_bottom skipped: no non-optimistic newest id");
            return;
        };

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            after_message_id = newest_id.get(),
            buffer_len = channel.messages.len(),
            "load_more_bottom: fetching newer page"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    newest_id.get(),
                    DIRECTION_AFTER,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(page) => page.messages,
                    Err(e) => {
                        tracing::error!("Failed to load newer messages for {channel_id}: {e}");
                        cx.notify();
                        return;
                    }
                };
                tracing::debug!(
                    channel_id = channel_id.get(),
                    anchor_after = newest_id.get(),
                    fetched = msgs.len(),
                    raw_first = msgs.first().map(|m| m.message_id).unwrap_or(0),
                    raw_last = msgs.last().map(|m| m.message_id).unwrap_or(0),
                    raw_min = msgs.iter().map(|m| m.message_id).min().unwrap_or(0),
                    raw_max = msgs.iter().map(|m| m.message_id).max().unwrap_or(0),
                    "load_more_bottom: page received (raw server ids)"
                );
                let cfg = AppConfig::try_global(cx);
                let viewer_id = viewer_user_id(cx);
                let (added, dropped) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let newer: Vec<Message> = msgs
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(MessageId(m.message_id)))
                        .map(|m| message_from_api(m, cfg, viewer_id))
                        .collect();
                    if newer.is_empty() {
                        cx.emit(MessagesEvent::Updated { message_id: None });
                        cx.notify();
                        return;
                    }
                    let added = newer.len();
                    // Appending newer drops the oldest (front) at the cap; those
                    // older rows then become re-fetchable from the top again.
                    let dropped = channel.messages.append_newer(newer);
                    if dropped > 0 {
                        channel.has_more = true;
                    }
                    (added, dropped)
                };
                if this.active_channel_id == Some(channel_id) {
                    if let Some(ch) = this.cache.get(&channel_id) {
                        tracing::debug!(
                            anchor_after = newest_id.get(),
                            added,
                            dropped,
                            buffer_oldest = ch.messages.first().map(|m| m.id.get()).unwrap_or(0),
                            buffer_newest = ch.messages.last().map(|m| m.id.get()).unwrap_or(0),
                            "load_more_bottom: appended newer page"
                        );
                    }
                    // Newer rows were appended; the cap may have dropped the same
                    // many oldest rows off the front. Emit the exact splice so the
                    // UI window matches the buffer 1:1 and the scroll stays
                    // anchored to the prior content.
                    cx.emit(MessagesEvent::Shifted {
                        added_top: 0,
                        removed_top: dropped,
                        added_bottom: added,
                        removed_bottom: 0,
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Jump to a message (cf. React `jumpToMessage`, used by reply previews).
    /// If the target is already in the buffer, emit [`MessagesEvent::JumpTo`] so
    /// the UI scrolls to it. Otherwise fetch a window centered on it
    /// (`AROUND_TIMESTAMP`), replace the buffer, and emit `Reset` then `JumpTo`.
    pub fn request_jump(
        &mut self,
        channel_id: ChannelId,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        self.pending_jump = Some((channel_id, message_id));
        self.try_consume_pending_jump(cx);
    }

    fn try_consume_pending_jump(&mut self, cx: &mut Context<Self>) {
        let Some((channel_id, message_id)) = self.pending_jump else {
            return;
        };
        if self.active_channel_id != Some(channel_id) || self.loading {
            return;
        }
        self.pending_jump = None;
        self.jump_to_message(message_id, cx);
    }

    pub fn jump_to_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        if self
            .cache
            .get(&channel_id)
            .is_some_and(|c| c.messages.contains_id(message_id))
        {
            cx.emit(MessagesEvent::JumpTo { message_id });
            return;
        }
        if self.loading_more || self.loading {
            return;
        }
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let anchor = message_id.get();

        self.loading_more = true;
        cx.notify();
        tracing::debug!(
            channel_id = channel_id.get(),
            message_id = anchor,
            "jump_to_message: fetching AROUND window"
        );

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(
                    clan_id.get(),
                    channel_id.get(),
                    anchor,
                    DIRECTION_AROUND,
                    MESSAGE_PAGE_LIMIT,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let msgs = match result {
                    Ok(page) => page.messages,
                    Err(e) => {
                        tracing::error!(
                            "jump_to_message AROUND fetch failed for {channel_id}: {e}"
                        );
                        cx.notify();
                        return;
                    }
                };
                let cfg = AppConfig::try_global(cx);
                let viewer_id = viewer_user_id(cx);
                let mut window: Vec<Message> = msgs
                    .into_iter()
                    .map(|m| message_from_api(m, cfg, viewer_id))
                    .collect();
                sort_messages(&mut window);
                // Centered trim if the window somehow exceeds the cap, keeping the
                // target near the middle so both directions stay scrollable.
                if window.len() > MAX_MESSAGES_PER_CHANNEL {
                    let target = window.iter().position(|m| m.id == message_id).unwrap_or(0);
                    let half = MAX_MESSAGES_PER_CHANNEL / 2;
                    let start = target
                        .saturating_sub(half)
                        .min(window.len() - MAX_MESSAGES_PER_CHANNEL);
                    window = window[start..start + MAX_MESSAGES_PER_CHANNEL].to_vec();
                }
                let found = window.iter().any(|m| m.id == message_id);
                if !found {
                    tracing::warn!(
                        message_id = anchor,
                        "jump_to_message: target not in AROUND window"
                    );
                    cx.notify();
                    return;
                }
                recompute_message_grouping(&mut window);
                let has_more = has_more_from_oldest(&window);
                if let Some(channel) = this.cache.get_mut(&channel_id) {
                    channel.messages.replace(window);
                    channel.has_more = has_more;
                }
                if this.active_channel_id == Some(channel_id) {
                    let count = this.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.emit(MessagesEvent::JumpTo { message_id });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Current composer reply target (React reply reference draft).
    pub fn reply_target(&self) -> Option<&ReplyDraft> {
        self.reply_target.as_ref()
    }

    /// Set the composer reply target (from a "Reply" action on a message).
    pub fn set_reply(&mut self, draft: ReplyDraft, cx: &mut Context<Self>) {
        self.reply_target = Some(draft);
        cx.emit(MessagesEvent::ReplyTargetChanged);
        cx.notify();
    }

    pub fn set_reply_to(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(draft) = self
            .cache
            .get(&channel_id)
            .and_then(|c| c.messages.get_by_id(message_id))
            .map(|msg| ReplyDraft {
                message_ref_id: msg.id,
                sender_id: msg.sender_user_id.unwrap_or_default(),
                sender_name: msg.sender_name.to_string(),
                sender_avatar: msg.avatar_url.to_string(),
                content_preview: msg.content.clone(),
                has_attachment: !msg.attachments.is_empty(),
                has_embed: msg.content.is_empty() && !msg.embeds.is_empty(),
                is_poll: msg.poll.is_some(),
            })
        else {
            return;
        };
        self.set_reply(draft, cx);
    }

    /// Clear the composer reply target.
    pub fn clear_reply(&mut self, cx: &mut Context<Self>) {
        if self.reply_target.take().is_some() {
            cx.emit(MessagesEvent::ReplyTargetChanged);
            cx.notify();
        }
    }

    /// Message currently being edited inline in its row, if any.
    pub fn editing_message_id(&self) -> Option<MessageId> {
        self.editing
    }

    /// Enter inline-edit mode for a message (own message only; enforced by callers).
    pub fn start_edit(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        self.editing = Some(message_id);
        cx.notify();
    }

    /// Leave inline-edit mode without saving.
    pub fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    /// Apply an edited message locally, then send the update to the server.
    /// No rollback on network failure — a channel refresh reconciles true failures.
    pub fn edit_message(
        &mut self,
        message_id: MessageId,
        content: String,
        content_tokens: OutgoingContent,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let mode = self.mode;
        let is_public = self.is_public;
        let (spans, transport_mentions, transport_hashtags, transport_emojis) =
            edit_content_spans(&content, content_tokens);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        msg.content = content.clone();
        msg.rich_layout = crate::message::build_rich_layout(&spans);
        msg.is_only_emoji = crate::message::spans_only_emoji(&spans);
        msg.spans = spans;
        msg.is_edited = true;
        patch_reply_previews_after_update(&mut channel.messages, message_id, &content);
        self.editing = None;
        cx.emit(MessagesEvent::Updated {
            message_id: Some(message_id),
        });
        cx.notify();

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_channel_message(
                    clan_id,
                    channel_num,
                    message_num,
                    &content,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                    mode,
                    is_public,
                )
                .await
            {
                tracing::error!("update_channel_message failed: {e}");
            }
        })
        .detach();
    }

    /// Remove a message locally, then send the delete to the server.
    /// No rollback on network failure — a channel refresh reconciles true failures.
    pub fn delete_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        if self.editing == Some(message_id) {
            self.editing = None;
        }
        self.apply_message_remove(channel_id, message_id, cx);

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .delete_channel_message(clan_id, channel_num, message_num)
                .await
            {
                tracing::error!("delete_channel_message failed: {e}");
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub fn mark_unread(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let clan_id = self.active_clan_id.unwrap_or(ClanId(0));
        let mode = self.mode;
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        let Some(pos) = channel.messages.position(message_id) else {
            return;
        };
        let slice = channel.messages.as_slice();
        let unread_count = i32::try_from(slice.len().saturating_sub(pos)).unwrap_or(i32::MAX);
        let (seen_id, seen_time) = if pos == 0 {
            (MessageId(0), slice[pos].create_time.saturating_sub(1))
        } else {
            let prior = &slice[pos - 1];
            (prior.id, prior.create_time)
        };

        self.set_last_read_message(channel_id, seen_id);
        self.last_seen_fingerprint.remove(&channel_id);
        cx.notify();

        let api = self.api.clone();
        let clan_num = clan_id.get();
        let channel_num = channel_id.get();
        let seen_num = seen_id.get();
        let timestamp_seconds = u32::try_from(seen_time.max(0)).unwrap_or(0);
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .write_last_seen_message(
                    clan_num,
                    channel_num,
                    seen_num,
                    mode,
                    timestamp_seconds,
                    unread_count,
                )
                .await
            {
                tracing::error!("mark_unread write_last_seen_message failed: {e}");
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub fn add_to_inbox(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_by_id(message_id) else {
            return;
        };
        let content_json = serde_json::to_string(&ApiMessageContent {
            t: msg.content.clone(),
            ..Default::default()
        })
        .unwrap_or_else(|_| msg.content.clone());

        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .create_message_2_inbox(message_num, channel_num, clan_id, &content_json)
                .await
            {
                tracing::error!("add_to_inbox create_message_2_inbox failed: {e}");
            }
        })
        .detach();
    }

    pub fn report_message(
        &self,
        message_id: MessageId,
        abuse_type: String,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.report_message_abuse(message_num, &abuse_type).await {
                tracing::error!("report_message report_message_abuse failed: {e}");
            }
        })
        .detach();
    }

    pub fn create_topic(&self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.create_sd_topic(message_num, clan_id, channel_num).await {
                tracing::error!("create_topic create_sd_topic failed: {e}");
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub fn forward(
        &mut self,
        message_id: MessageId,
        targets: Vec<ForwardTarget>,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() {
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(channel) = self.cache.get(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_by_id(message_id) else {
            return;
        };
        let content = msg.content.clone();
        let attachments: Vec<mezon_client::transport::ApiAttachment> =
            msg.attachments.iter().map(attachment_to_api).collect();
        let plans = plan_forwards(&content, &attachments, &targets, note.as_deref());

        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            for plan in plans {
                if let Err(e) = api
                    .join_chat(
                        plan.clan_id,
                        plan.channel_id,
                        plan.channel_type,
                        plan.is_public,
                    )
                    .await
                {
                    tracing::error!("forward join_chat failed: {e}");
                    continue;
                }
                if let Err(e) = api
                    .forward_channel_message(
                        plan.clan_id,
                        plan.channel_id,
                        &plan.content,
                        plan.is_public,
                        plan.mode,
                        plan.attachments,
                    )
                    .await
                {
                    tracing::error!("forward_channel_message failed: {e}");
                    continue;
                }
                if let Some(note) = plan.note
                    && !note.is_empty()
                    && let Err(e) = api
                        .send_channel_message(
                            plan.clan_id,
                            plan.channel_id,
                            &note,
                            plan.is_public,
                            plan.mode,
                            Vec::new(),
                            Vec::new(),
                            Vec::new(),
                        )
                        .await
                {
                    tracing::error!("forward note send failed: {e}");
                }
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub fn remove_attachment(
        &mut self,
        message_id: MessageId,
        attachment_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let mode = self.mode;
        let is_public = self.is_public;
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let cfg = AppConfig::try_global(cx).cloned();
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        if attachment_index >= msg.attachments.len() {
            return;
        }
        msg.attachments.remove(attachment_index);
        let (album_layout, viewer_media) = build_media_presentation(&msg.attachments, cfg.as_ref());
        msg.album_layout = album_layout;
        msg.viewer_media = viewer_media;
        let content = msg.content.clone();
        let remaining: Vec<mezon_client::transport::ApiAttachment> =
            msg.attachments.iter().map(attachment_to_api).collect();
        cx.emit(MessagesEvent::Updated {
            message_id: Some(message_id),
        });
        cx.notify();

        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_channel_message_with_attachments(
                    clan_id,
                    channel_num,
                    message_num,
                    &content,
                    remaining,
                    mode,
                    is_public,
                )
                .await
            {
                tracing::error!("remove_attachment update failed: {e}");
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub fn message_select_selection(
        &self,
        message_id: MessageId,
        select_id: &str,
    ) -> &[SharedString] {
        self.select_ui
            .get(&message_id)
            .and_then(|by_select| by_select.get(select_id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[allow(dead_code)]
    pub fn set_message_select_selection(
        &mut self,
        message_id: MessageId,
        select_id: SharedString,
        values: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.select_ui
            .entry(message_id)
            .or_default()
            .insert(select_id, values);
        cx.notify();
    }

    #[allow(dead_code)]
    pub fn select_message_option(
        &mut self,
        message_id: MessageId,
        select_id: SharedString,
        values: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        self.set_message_select_selection(message_id, select_id.clone(), values, cx);

        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        let select_id = select_id.to_string();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .dropdown_box_selected(message_num, channel_num, &select_id)
                .await
            {
                tracing::error!("select_message_option dropdown_box_selected failed: {e}");
            }
        })
        .detach();
    }

    pub fn click_message_button(
        &mut self,
        message_id: MessageId,
        button_id: SharedString,
        sender_id: i64,
        user_id: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let extra_data = self
            .select_ui
            .get(&message_id)
            .map(|by_select| {
                let map: std::collections::BTreeMap<String, Vec<String>> = by_select
                    .iter()
                    .map(|(id, values)| {
                        (
                            id.to_string(),
                            values.iter().map(|v| v.to_string()).collect(),
                        )
                    })
                    .collect();
                serde_json::to_string(&map).unwrap_or_default()
            })
            .unwrap_or_default();
        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        let button_id = button_id.to_string();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .message_button_click(
                    message_num,
                    channel_num,
                    &button_id,
                    sender_id,
                    user_id,
                    &extra_data,
                )
                .await
            {
                tracing::error!("click_message_button message_button_click failed: {e}");
            }
        })
        .detach();
    }

    pub fn send_message(
        &mut self,
        content: String,
        sender_id: String,
        sender_name: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let is_public = self.is_public;
        let mode = self.mode;
        let has_attachments = !attachments.is_empty();
        let reply = self.reply_target.take();
        if reply.is_some() {
            cx.emit(MessagesEvent::ReplyTargetChanged);
        }

        self.clear_last_read_message(channel_id);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let create_time = optimistic_create_time(&channel.messages, &sender_id);
        let temp_id = MessageId::next_optimistic();

        let OutgoingContent {
            mentions,
            hashtags,
            emojis,
        } = content_tokens;
        let transport_mentions: Vec<TransportMention> = mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let transport_hashtags: Vec<TransportHashtag> = hashtags
            .into_iter()
            .map(OutgoingHashtag::into_transport)
            .collect();
        let transport_emojis: Vec<TransportEmoji> = emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect();
        let markdowns = detect_markdown(&content);
        let (display_name, avatar_url, avatar_proxied) =
            outgoing_sender_profile(&sender_id, &sender_name, clan_id, cx);
        let mut optimistic = Message::new(
            temp_id,
            content.clone(),
            sender_id,
            display_name,
            create_time,
        )
        .with_avatar(avatar_url)
        .with_avatar_proxied(avatar_proxied);
        if !transport_mentions.is_empty()
            || !transport_hashtags.is_empty()
            || !transport_emojis.is_empty()
            || !markdowns.is_empty()
        {
            let tokens = ApiMessageContent {
                t: content.clone(),
                mentions: mention_content_tokens(&transport_mentions),
                hg: hashtag_content_tokens(&transport_hashtags),
                ej: emoji_content_tokens(&transport_emojis),
                mk: markdown_content_tokens(&markdowns),
                ..Default::default()
            };
            optimistic = optimistic.with_spans(parse_spans(&tokens));
        }
        if let Some(draft) = &reply {
            optimistic = optimistic.with_references(vec![MessageReference {
                message_ref_id: draft.message_ref_id,
                sender_id: draft.sender_id,
                sender_name: draft.sender_name.clone(),
                sender_avatar: draft.sender_avatar.clone(),
                content_preview: crate::message::reply_preview_line(&draft.content_preview).into(),
                content: draft.content_preview.clone(),
                has_attachment: draft.has_attachment,
                has_embed: draft.has_embed,
                is_poll: draft.is_poll,
            }]);
        }
        if has_attachments {
            let optimistic_attachments: Vec<MessageAttachment> = attachments
                .iter()
                .map(MessageAttachment::optimistic_local)
                .collect();
            let (album_layout, viewer_media) =
                build_media_presentation(&optimistic_attachments, AppConfig::try_global(cx));
            optimistic = optimistic
                .with_attachments(optimistic_attachments)
                .with_media_presentation(album_layout, viewer_media);
        }
        let old_len = channel.messages.len();
        channel.messages.push_grouped(optimistic);
        self.emit_appended(old_len, cx);

        let api = self.api.clone();
        let reply_ref = reply.map(|draft| OutgoingReply {
            message_ref_id: draft.message_ref_id.get(),
            content: draft.content_preview,
            has_attachment: draft.has_attachment,
            message_sender_id: draft.sender_id.get(),
            message_sender_username: draft.sender_name.clone(),
            message_sender_avatar: draft.sender_avatar,
            message_sender_clan_nick: String::new(),
            message_sender_display_name: draft.sender_name,
        });
        cx.spawn(async move |this, cx| {
            if has_attachments {
                let files: Vec<UploadFile> = attachments
                    .into_iter()
                    .map(|att| {
                        let thumbnail = att.poster_jpeg.map(|jpeg| UploadThumbnail {
                            filename: format!("{}.jpg", att.filename),
                            data: jpeg,
                        });
                        UploadFile {
                            path: att.path,
                            filename: att.filename,
                            filetype: att.filetype,
                            width: att.width,
                            height: att.height,
                            thumbnail,
                        }
                    })
                    .collect();
                let presigned = match api.presign_files(files).await {
                    Ok(presigned) => presigned,
                    Err(e) => {
                        tracing::error!("presign attachments failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.mark_temp_failed(channel_id, temp_id, cx);
                        });
                        return;
                    }
                };
                let msg_attachments: Vec<_> =
                    presigned.iter().map(|p| p.attachment.clone()).collect();
                let keys: Vec<String> = msg_attachments
                    .iter()
                    .map(|a| presign::normalize_presign_key(&a.url))
                    .collect();
                let update_mentions = transport_mentions.clone();
                let sent = match api
                    .send_presigned_message(
                        clan_id.get(),
                        channel_id.get(),
                        &content,
                        is_public,
                        mode,
                        msg_attachments,
                        reply_ref,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                        Vec::new(),
                    )
                    .await
                {
                    Ok(sent) => sent,
                    Err(e) => {
                        tracing::error!("send_presigned_message failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.mark_temp_failed(channel_id, temp_id, cx);
                        });
                        return;
                    }
                };
                let real_message_id = sent.message_id;
                let create_time_seconds = sent.create_time.max(0) as u32;
                let _ = this.update(cx, |this, cx| {
                    let confirmed =
                        message_from_api(sent, AppConfig::try_global(cx), viewer_user_id(cx));
                    this.reconcile_temp(channel_id, temp_id, confirmed, cx);
                });
                let (on_complete, mut completions) =
                    tokio::sync::mpsc::unbounded_channel::<String>();
                let drain_this = this.clone();
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    while let Some(key) = completions.recv().await {
                        let _ = drain_this.update(cx, |this, cx| {
                            this.mark_attachment_uploaded(
                                channel_id,
                                MessageId(real_message_id),
                                &key,
                                cx,
                            );
                        });
                    }
                })
                .detach();
                api.upload_presigned_and_patch(
                    clan_id.get(),
                    channel_id.get(),
                    real_message_id,
                    &content,
                    update_mentions,
                    create_time_seconds,
                    presigned,
                    keys,
                    mode,
                    is_public,
                    on_complete,
                )
                .await;
            } else {
                let result = if let Some(reply_ref) = reply_ref {
                    api.send_channel_message_reply(
                        clan_id.get(),
                        channel_id.get(),
                        &content,
                        is_public,
                        mode,
                        reply_ref,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                    )
                    .await
                } else {
                    api.send_channel_message(
                        clan_id.get(),
                        channel_id.get(),
                        &content,
                        is_public,
                        mode,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                    )
                    .await
                };
                match result {
                    Ok(sent) => {
                        let _ = this.update(cx, |this, cx| {
                            let confirmed = message_from_api(
                                sent,
                                AppConfig::try_global(cx),
                                viewer_user_id(cx),
                            );
                            this.reconcile_temp(channel_id, temp_id, confirmed, cx);
                        });
                    }
                    Err(e) => {
                        tracing::error!("send_channel_message failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.mark_temp_failed(channel_id, temp_id, cx);
                        });
                    }
                }
            }
        })
        .detach();
    }

    pub fn send_sticker(
        &mut self,
        url: String,
        filename: String,
        sender_id: String,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        if url.is_empty() {
            return;
        }
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let is_public = self.is_public;
        let mode = self.mode;
        self.clear_last_read_message(channel_id);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let create_time = optimistic_create_time(&channel.messages, &sender_id);
        let temp_id = MessageId::next_optimistic();

        let optimistic_attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: url.clone(),
                filename: filename.clone(),
                filetype: STICKER_FILETYPE.to_string(),
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
                size: 0,
            },
            AppConfig::try_global(cx),
        );

        let (display_name, avatar_url, avatar_proxied) =
            outgoing_sender_profile(&sender_id, &sender_name, clan_id, cx);
        let optimistic = Message::new(temp_id, String::new(), sender_id, display_name, create_time)
            .with_avatar(avatar_url)
            .with_avatar_proxied(avatar_proxied)
            .with_attachments(vec![optimistic_attachment]);
        let old_len = channel.messages.len();
        channel.messages.push_grouped(optimistic);
        self.emit_appended(old_len, cx);

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .send_message_with_attachment_urls(
                    clan_id.get(),
                    channel_id.get(),
                    is_public,
                    mode,
                    vec![UrlAttachment {
                        url,
                        filename,
                        filetype: STICKER_FILETYPE.to_string(),
                    }],
                )
                .await;
            match result {
                Ok(sent) => {
                    let _ = this.update(cx, |this, cx| {
                        let confirmed =
                            message_from_api(sent, AppConfig::try_global(cx), viewer_user_id(cx));
                        this.reconcile_temp(channel_id, temp_id, confirmed, cx);
                    });
                }
                Err(e) => {
                    tracing::error!("send sticker failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.mark_temp_failed(channel_id, temp_id, cx);
                    });
                }
            }
        })
        .detach();
    }

    fn message_is_loaded(&self, id: MessageId) -> bool {
        self.cache
            .iter()
            .any(|(_, channel)| channel.messages.contains_id(id))
    }

    fn prune_message_ui_state(&mut self) {
        if self.poll_ui.is_empty() && self.poll_my_vote.is_empty() && self.select_ui.is_empty() {
            return;
        }
        let stale: Vec<MessageId> = self
            .poll_ui
            .keys()
            .chain(self.poll_my_vote.keys())
            .chain(self.select_ui.keys())
            .copied()
            .filter(|id| !self.message_is_loaded(*id))
            .collect();
        for id in stale {
            self.poll_ui.remove(&id);
            self.poll_my_vote.remove(&id);
            self.select_ui.remove(&id);
        }
    }

    fn on_active_channel_changed(&mut self, channel_id: Option<ChannelId>, cx: &mut Context<Self>) {
        self.pending_self_adds.clear();
        self.prune_message_ui_state();
        let Some(channel_id) = channel_id else {
            self.flush_pending_last_seen(cx);
            self.active_channel_id = None;
            self.active_clan_id = None;
            self.is_dm = false;
            self.loading = false;
            self.loading_more = false;
            self.reply_target = None;
            cx.emit(MessagesEvent::Reset { count: 0 });
            cx.notify();
            return;
        };
        self.open_channel(channel_id, cx);
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.pending_jump = None;
        if self.active_channel_id.is_none() && !self.is_dm {
            return;
        }
        self.on_active_channel_changed(None, cx);
    }

    /// Open a clan channel as the active conversation (looks up clan/privacy from `ChannelList`).
    pub fn open_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.active_channel_id == Some(channel_id) && !self.is_dm {
            if self.loading {
                return;
            }
            let empty = self
                .cache
                .get(&channel_id)
                .map(|c| c.messages.is_empty())
                .unwrap_or(true);
            if !empty || self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
                return;
            }
            self.refetch_current_messages(cx);
            return;
        }
        let Some(channel) = ChannelList::global(cx)
            .read(cx)
            .find_channel_in_active_clan(channel_id)
            .cloned()
        else {
            return;
        };
        let (is_public, join_type, mode) =
            channel_join_params(channel.channel_type, channel.parent_id, channel.private);
        self.activate(
            channel.clan_id,
            channel_id,
            is_public,
            false,
            join_type,
            mode,
            cx,
        );
    }

    /// Open a direct message / group conversation (clan_id = 0) as the active conversation.
    /// `channel_type` is the raw DM type (3 = DM, 2 = group).
    pub fn open_direct(
        &mut self,
        channel_id: ChannelId,
        channel_type: i32,
        cx: &mut Context<Self>,
    ) {
        if self.active_channel_id == Some(channel_id) && self.is_dm {
            if self.loading {
                return;
            }
            let empty = self
                .cache
                .get(&channel_id)
                .map(|c| c.messages.is_empty())
                .unwrap_or(true);
            if !empty || self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
                return;
            }
            self.refetch_current_messages(cx);
            return;
        }
        let mode = if channel_type == 2 { 3 } else { 4 };
        self.activate(ClanId(0), channel_id, false, true, channel_type, mode, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn activate(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        is_public: bool,
        is_dm: bool,
        join_type: i32,
        mode: i32,
        cx: &mut Context<Self>,
    ) {
        self.flush_pending_last_seen(cx);
        if self.pending_jump.is_some_and(|(pc, _)| pc != channel_id) {
            self.pending_jump = None;
        }
        self.active_channel_id = Some(channel_id);
        self.active_clan_id = Some(clan_id);
        self.is_public = is_public;
        self.is_dm = is_dm;
        self.mode = mode;
        self.viewing_older_by_channel.insert(channel_id, false);
        self.loading_more = false;
        self.reply_target = None;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;

        if !self.joined_channels.contains(&channel_id) {
            self.joined_channels.insert(channel_id);
            self.spawn_join(clan_id, channel_id, join_type, is_public, cx);
        }

        self.seed_last_read_from_channel(channel_id, cx);

        if self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
            self.cache.touch(&channel_id);
            self.loading = false;
            let count = self.messages().len();
            cx.emit(MessagesEvent::Reset { count });
            cx.notify();
            self.try_consume_pending_jump(cx);
            return;
        }

        self.loading = true;
        if self.cache.contains(&channel_id) {
            self.cache.touch(&channel_id);
            let count = self.messages().len();
            cx.emit(MessagesEvent::Reset { count });
        } else {
            cx.emit(MessagesEvent::Reset { count: 0 });
        }
        cx.notify();
        self.spawn_initial_fetch(clan_id, channel_id, generation, cx);
    }

    fn spawn_join(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        join_type: i32,
        is_public: bool,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .join_chat(clan_id.get(), channel_id.get(), join_type, is_public)
                .await
            {
                tracing::warn!("join_chat failed: {e}");
            }
        })
        .detach();
    }

    fn spawn_initial_fetch(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(clan_id.get(), channel_id.get(), 0, 0, MESSAGE_PAGE_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_initial_fetch_result(channel_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn apply_initial_fetch_result(
        &mut self,
        channel_id: ChannelId,
        generation: u64,
        result: Result<mezon_client::transport::ListChannelMessagesResult, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_channel_id == Some(channel_id);
        let is_current = is_active && self.fetch_generation == generation;

        match result {
            Ok(page) => {
                if !self.last_read_message_by_channel.contains_key(&channel_id)
                    && page.last_seen_message_id > 0
                {
                    self.set_last_read_message(channel_id, MessageId(page.last_seen_message_id));
                }
                let messages =
                    prepare_messages(page.messages, AppConfig::try_global(cx), viewer_user_id(cx));
                self.set_channel(channel_id, messages);
                if is_current {
                    self.loading = false;
                    let count = self.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.notify();
                    self.try_consume_pending_jump(cx);
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch messages for {channel_id}: {e}");
                if is_current {
                    self.loading = false;
                    let count = self.messages().len();
                    cx.emit(MessagesEvent::Reset { count });
                    cx.notify();
                }
            }
        }
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ChannelMessage(m) = event else {
            return;
        };

        let code = MessageCode::from_raw(m.code);
        if matches!(code, MessageCode::Typing) {
            return;
        }

        let storage_id = storage_channel_id(m);
        let parent_id = parent_channel_id(m);
        let message_id = MessageId(synthesize_ws_message_id(
            self,
            storage_id,
            parent_id,
            m.message_id,
        ));
        let cfg = AppConfig::try_global(cx);
        let viewer_id = viewer_user_id(cx);

        match code {
            MessageCode::ChatUpdate | MessageCode::UpdateEphemeralMsg => {
                let incoming = message_from_channel_proto(m, message_id.get(), cfg, viewer_id);
                self.apply_message_update(storage_id, message_id, incoming, cx);
            }
            MessageCode::ChatRemove | MessageCode::DeleteEphemeralMsg => {
                self.apply_message_remove(storage_id, message_id, cx);
            }
            _ => {
                if !self.cache.contains(&storage_id) {
                    self.set_last_message(storage_id, message_id);
                    return;
                }
                if self.is_viewing_older(storage_id) {
                    self.set_last_message(storage_id, message_id);
                    return;
                }
                let tail_loaded = self.cache.get(&storage_id).is_some_and(|channel| {
                    !has_more_bottom_for(
                        self.last_message_by_channel.get(&storage_id).copied(),
                        &channel.messages,
                    )
                });
                if !tail_loaded {
                    self.set_last_message(storage_id, message_id);
                    return;
                }
                let incoming = message_from_channel_proto(m, message_id.get(), cfg, viewer_id);
                self.apply_incoming_message(storage_id, incoming, cx);
            }
        }
    }

    fn apply_incoming_message(
        &mut self,
        storage_id: ChannelId,
        msg: Message,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_channel_id == Some(storage_id);
        let incoming_id = msg.id;
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            self.set_last_message(storage_id, msg.id);
            return;
        };
        if channel.messages.contains_id(msg.id) {
            if channel.messages.merge_existing(msg.id, msg) && is_active {
                cx.emit(MessagesEvent::Updated {
                    message_id: Some(incoming_id),
                });
                cx.notify();
            }
            return;
        }
        let tail_id = msg.id;
        let old_len = channel.messages.len();
        let appended = match channel
            .messages
            .temp_match_position(&msg.sender_id, &msg.content)
        {
            Some(idx) => {
                let prior = channel.messages.items[idx].clone();
                let merged = merge_sparse_sender(&prior, msg);
                channel.messages.replace_resort(idx, merged);
                false
            }
            None => {
                channel.messages.push_grouped(msg);
                true
            }
        };
        let last_id = channel.messages.last().map(|m| m.id).unwrap_or(tail_id);
        self.set_last_message(storage_id, last_id);
        if is_active {
            if appended {
                self.emit_appended(old_len, cx);
            } else {
                cx.emit(MessagesEvent::Updated {
                    message_id: Some(tail_id),
                });
                cx.notify();
            }
        }
    }

    fn apply_message_update(
        &mut self,
        storage_id: ChannelId,
        message_id: MessageId,
        incoming: Message,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_channel_id == Some(storage_id);
        let preview = incoming.content.clone();
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            return;
        };
        let Some(existing) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        merge_message_update(existing, &incoming);
        patch_reply_previews_after_update(&mut channel.messages, message_id, &preview);
        if is_active {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
            cx.notify();
        }
    }

    fn apply_message_remove(
        &mut self,
        storage_id: ChannelId,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        if self
            .reply_target
            .as_ref()
            .is_some_and(|draft| draft.message_ref_id == message_id)
        {
            self.reply_target = None;
            cx.emit(MessagesEvent::ReplyTargetChanged);
        }
        self.retreat_last_message(storage_id, message_id);

        let is_active = self.active_channel_id == Some(storage_id);
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            return;
        };
        patch_reply_previews_after_delete(&mut channel.messages, message_id);
        if let Some(index) = channel.messages.remove_id(message_id)
            && is_active
        {
            cx.emit(MessagesEvent::RemovedAt { index, message_id });
            cx.notify();
        }
    }

    fn retreat_last_message(&mut self, storage_id: ChannelId, deleted_id: MessageId) {
        if self.last_message_by_channel.get(&storage_id) != Some(&deleted_id) {
            return;
        }
        if self.is_viewing_older(storage_id) {
            self.last_message_by_channel.remove(&storage_id);
            return;
        }
        if let Some(prev) = self.cache.get(&storage_id).and_then(|c| {
            c.messages
                .as_slice()
                .iter()
                .rev()
                .find(|m| m.id != deleted_id)
        }) {
            self.set_last_message(storage_id, prev.id);
        } else {
            self.last_message_by_channel.remove(&storage_id);
        }
    }

    pub fn add_reaction(
        &mut self,
        message_id: MessageId,
        emoji_id: String,
        emoji: String,
        cx: &mut Context<Self>,
    ) {
        self.send_reaction(message_id, emoji_id, emoji, false, cx);
    }

    pub fn remove_reaction(
        &mut self,
        message_id: MessageId,
        emoji_id: String,
        emoji: String,
        cx: &mut Context<Self>,
    ) {
        self.send_reaction(message_id, emoji_id, emoji, true, cx);
    }

    fn send_reaction(
        &mut self,
        message_id: MessageId,
        emoji_id: String,
        emoji: String,
        remove: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(current_uid) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return;
        };
        let uid_str = current_uid.get().to_string();
        let key = reaction_key(&emoji_id, &emoji).to_string();
        let cfg = AppConfig::try_global(cx);
        let applied = self
            .cache
            .get_mut(&channel_id)
            .and_then(|channel| channel.messages.get_mut_by_id(message_id))
            .map(|msg| {
                apply_reaction_event(&mut msg.reactions, &emoji_id, &emoji, &uid_str, remove, cfg);
            })
            .is_some();
        if applied {
            if !remove {
                *self
                    .pending_self_adds
                    .entry((channel_id, message_id, key))
                    .or_insert(0) += 1;
            }
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
            cx.notify();
        }

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let mode = self.mode;
        let is_public = self.is_public;
        let message_sender_id = current_uid.get();
        let emoji_id_num = emoji_id.parse::<i64>().unwrap_or(0);
        let channel = channel_id.get();
        let message = message_id.get();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .react_channel_message(
                    clan_id,
                    channel,
                    message,
                    emoji_id_num,
                    &emoji,
                    1,
                    message_sender_id,
                    mode,
                    is_public,
                    remove,
                )
                .await
            {
                tracing::error!("react_channel_message failed: {e}");
                if applied {
                    let _ = this.update(cx, |store, cx| {
                        store.rollback_reaction_send(
                            channel_id, message_id, &emoji_id, &emoji, &uid_str, remove, cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    fn rollback_reaction_send(
        &mut self,
        channel_id: ChannelId,
        message_id: MessageId,
        emoji_id: &str,
        emoji: &str,
        sender_id: &str,
        was_remove: bool,
        cx: &mut Context<Self>,
    ) {
        let cfg = AppConfig::try_global(cx);
        if let Some(msg) = self
            .cache
            .get_mut(&channel_id)
            .and_then(|channel| channel.messages.get_mut_by_id(message_id))
        {
            rollback_reaction(
                &mut msg.reactions,
                emoji_id,
                emoji,
                sender_id,
                was_remove,
                cfg,
            );
        }
        if !was_remove {
            let entry_key = (
                channel_id,
                message_id,
                reaction_key(emoji_id, emoji).to_string(),
            );
            if let Some(count) = self.pending_self_adds.get_mut(&entry_key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.pending_self_adds.remove(&entry_key);
                }
            }
        }
        cx.emit(MessagesEvent::Updated {
            message_id: Some(message_id),
        });
        cx.notify();
    }

    pub fn active_message(&self, message_id: MessageId) -> Option<&Message> {
        let channel_id = self.active_channel_id?;
        self.cache.get(&channel_id)?.messages.get_by_id(message_id)
    }

    pub fn poll_ui_state(&self, message_id: MessageId) -> Option<&PollUiState> {
        self.poll_ui.get(&message_id)
    }

    pub fn poll_my_vote(&self, message_id: MessageId) -> Option<&[i32]> {
        self.poll_my_vote.get(&message_id).map(Vec::as_slice)
    }

    pub fn toggle_poll_answer(
        &mut self,
        message_id: MessageId,
        index: i32,
        allow_multiple: bool,
        cx: &mut Context<Self>,
    ) {
        let state = self.poll_ui.entry(message_id).or_default();
        if allow_multiple {
            if let Some(pos) = state.selected.iter().position(|&i| i == index) {
                state.selected.remove(pos);
            } else {
                state.selected.push(index);
            }
        } else {
            state.selected = vec![index];
        }
        cx.notify();
    }

    pub fn toggle_poll_results(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let state = self.poll_ui.entry(message_id).or_default();
        state.show_results = !state.show_results;
        cx.notify();
    }

    pub fn submit_poll_vote(
        &mut self,
        poll_id: i64,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .poll_ui
            .get(&message_id)
            .map(|s| s.selected.clone())
            .unwrap_or_default();
        if selected.is_empty() {
            return;
        }
        self.send_poll_vote(poll_id, message_id, selected, cx);
    }

    pub fn remove_poll_vote(
        &mut self,
        poll_id: i64,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        self.send_poll_vote(poll_id, message_id, Vec::new(), cx);
    }

    fn send_poll_vote(
        &mut self,
        poll_id: i64,
        message_id: MessageId,
        answer_indices: Vec<i32>,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        self.poll_ui.entry(message_id).or_default().voting = true;
        cx.notify();
        let api = self.api.clone();
        let cid = channel_id.get();
        let mid = message_id.get();
        cx.spawn(async move |this, cx| {
            let result = api.vote_poll(poll_id, mid, cid, answer_indices).await;
            let _ = this.update(cx, |store, cx| {
                if let Some(state) = store.poll_ui.get_mut(&message_id) {
                    state.voting = false;
                    state.selected.clear();
                    state.show_results = false;
                }
                match result {
                    Ok(resp) => {
                        store
                            .poll_my_vote
                            .insert(message_id, resp.my_answer_indices);
                    }
                    Err(e) => tracing::error!("vote_poll failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn close_poll(&mut self, poll_id: i64, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let api = self.api.clone();
        let cid = channel_id.get();
        let mid = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.close_poll(poll_id, mid, cid).await {
                tracing::error!("close_poll failed: {e}");
            }
        })
        .detach();
    }

    pub fn fetch_poll_detail(
        &self,
        poll_id: i64,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<PollDetail>> {
        let api = self.api.clone();
        let cid = self.active_channel_id.map_or(0, |c| c.get());
        let clan_id = self.active_clan_id.unwrap_or(ClanId(0));
        let mid = message_id.get();
        cx.spawn(async move |this, cx| {
            let resp = api.get_poll(poll_id, mid, cid).await?;
            let detail = this.update(cx, |_store, cx| map_poll_detail(&resp, clan_id, cx))?;
            Ok(detail)
        })
    }

    fn handle_reaction(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MessageReaction(r) = event else {
            return;
        };
        let channel_id = ChannelId(r.channel_id);
        let message_id = MessageId(r.message_id);
        let is_active = self.active_channel_id == Some(channel_id);
        let sender_id = r.sender_id.to_string();
        let emoji_id = r.emoji_id.to_string();

        if !r.action
            && self.consume_pending_self_add(
                channel_id, message_id, &emoji_id, &r.emoji, &sender_id, cx,
            )
        {
            return;
        }

        let cfg = AppConfig::try_global(cx);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        apply_reaction_event(
            &mut msg.reactions,
            &emoji_id,
            &r.emoji,
            &sender_id,
            r.action,
            cfg,
        );
        if is_active {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
            cx.notify();
        }
    }

    fn consume_pending_self_add(
        &mut self,
        channel_id: ChannelId,
        message_id: MessageId,
        emoji_id: &str,
        emoji: &str,
        sender_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(current_uid) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return false;
        };
        if current_uid.get().to_string() != sender_id {
            return false;
        }
        let entry_key = (
            channel_id,
            message_id,
            reaction_key(emoji_id, emoji).to_string(),
        );
        let Some(count) = self.pending_self_adds.get_mut(&entry_key) else {
            return false;
        };
        *count -= 1;
        if *count == 0 {
            self.pending_self_adds.remove(&entry_key);
        }
        true
    }

    fn reconcile_temp(
        &mut self,
        channel_id: ChannelId,
        temp_id: MessageId,
        confirmed: Message,
        cx: &mut Context<Self>,
    ) {
        let confirmed_id = confirmed.id;
        let (pushed, old_len) = {
            let Some(channel) = self.cache.get_mut(&channel_id) else {
                return;
            };
            let old_len = channel.messages.len();
            if let Some(idx) = channel.messages.position(temp_id) {
                let temp = channel
                    .messages
                    .get_by_id(temp_id)
                    .expect("temp row must exist at position")
                    .clone();
                let confirmed = merge_sparse_sender(&temp, confirmed);
                channel.messages.replace_at_and_regroup(idx, confirmed);
                (false, old_len)
            } else if !channel.messages.contains_id(confirmed.id) {
                channel.messages.push_sorted(confirmed);
                (true, old_len)
            } else {
                (false, old_len)
            }
        };
        self.set_last_message(channel_id, confirmed_id);
        if self.active_channel_id != Some(channel_id) {
            return;
        }
        if pushed {
            self.emit_appended(old_len, cx);
        } else {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(confirmed_id),
            });
            cx.notify();
        }
    }

    fn mark_temp_failed(
        &mut self,
        channel_id: ChannelId,
        temp_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let marked = {
            let Some(channel) = self.cache.get_mut(&channel_id) else {
                return;
            };
            match channel.messages.get_mut_by_id(temp_id) {
                Some(message) => {
                    message.send_failed = true;
                    true
                }
                None => false,
            }
        };
        if marked && self.active_channel_id == Some(channel_id) {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(temp_id),
            });
            cx.notify();
        }
    }

    fn mark_attachment_uploaded(
        &mut self,
        channel_id: ChannelId,
        message_id: MessageId,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        let cleared = {
            let Some(channel) = self.cache.get_mut(&channel_id) else {
                return;
            };
            let Some(message) = channel.messages.get_mut_by_id(message_id) else {
                return;
            };
            let mut cleared = false;
            for att in message.attachments.iter_mut() {
                if att.uploading && presign::normalize_presign_key(&att.url) == key {
                    att.uploading = false;
                    cleared = true;
                }
            }
            cleared
        };
        if cleared && self.active_channel_id == Some(channel_id) {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
            cx.notify();
        }
    }

    fn resync(&mut self, cx: &mut Context<Self>) {
        tracing::info!("MessagesStore resync — marking message cache stale");
        self.cache.mark_all_stale();
        self.joined_channels.clear();
        self.refetch_current_messages(cx);
    }

    /// Force a refetch of the open channel ignoring the cache (cf. React `noCache: true`).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refetch_current_messages(cx);
    }

    /// Reload the latest message page from the server (cf. React
    /// `fetchMessages({ toPresent: true, isClearMessage: true })`).
    pub fn jump_to_present(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        self.set_viewing_older(channel_id, false);
        self.refetch_current_messages(cx);
    }

    fn refetch_current_messages(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };

        self.loading = true;
        self.loading_more = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_messages(clan_id.get(), channel_id.get(), 0, 0, MESSAGE_PAGE_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_initial_fetch_result(channel_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn seed_last_read_from_channel(&mut self, channel_id: ChannelId, cx: &App) {
        if self.last_read_message_by_channel.contains_key(&channel_id) {
            return;
        }
        let Some(last_seen_id) = ChannelList::global(cx)
            .read(cx)
            .find_channel_in_active_clan(channel_id)
            .map(|ch| ch.last_seen_message_id)
            .filter(|id| !id.is_zero())
        else {
            return;
        };
        self.last_read_message_by_channel
            .insert(channel_id, last_seen_id);
    }

    fn set_channel(&mut self, channel_id: ChannelId, messages: Vec<Message>) {
        let active = self.active_channel_id;
        let has_more = has_more_from_oldest(&messages);
        if let Some(newest) = messages.last()
            && !self.last_message_by_channel.contains_key(&channel_id)
        {
            self.set_last_message(channel_id, newest.id);
        }
        self.cache.insert(
            channel_id,
            ChannelMessages {
                messages: MessageList::from_messages(messages),
                has_more,
            },
            active.as_ref(),
        );
    }
}

const DELETED_REPLY_PREVIEW: &str = "Original message was deleted";

fn snowflake_seq(id: MessageId) -> i64 {
    id.get() >> 22
}

fn should_write_last_seen(
    last_seen_id: Option<MessageId>,
    channel_tail: Option<MessageId>,
    viewport_id: MessageId,
) -> bool {
    if let Some(seen) = last_seen_id
        && snowflake_seq(viewport_id) >= snowflake_seq(seen)
    {
        return true;
    }
    channel_tail == Some(viewport_id)
}

fn channel_join_params(
    channel_type: ChannelType,
    parent_id: Option<ChannelId>,
    private: bool,
) -> (bool, i32, i32) {
    let is_thread = channel_type == ChannelType::Thread || parent_id.is_some();
    if is_thread {
        (false, CHANNEL_TYPE_THREAD, STREAM_MODE_THREAD)
    } else {
        (!private, CHANNEL_TYPE_CHANNEL, STREAM_MODE_CHANNEL)
    }
}

fn storage_channel_id(m: &mezon_proto::api::ChannelMessage) -> ChannelId {
    if m.topic_id != 0 {
        ChannelId(m.topic_id)
    } else {
        ChannelId(m.channel_id)
    }
}

fn parent_channel_id(m: &mezon_proto::api::ChannelMessage) -> ChannelId {
    ChannelId(m.channel_id)
}

fn synthesize_ws_message_id(
    store: &MessagesStore,
    storage_id: ChannelId,
    parent_id: ChannelId,
    raw_id: i64,
) -> i64 {
    if raw_id > 0 {
        return raw_id;
    }
    store
        .cache
        .get(&storage_id)
        .and_then(|c| c.messages.last().map(|m| m.id.get()))
        .or_else(|| {
            store
                .last_message_by_channel
                .get(&parent_id)
                .map(|id| id.get())
        })
        .or_else(|| {
            store
                .last_message_by_channel
                .get(&storage_id)
                .map(|id| id.get())
        })
        .map(|id| id.saturating_add(1))
        .filter(|id| *id > 0)
        .unwrap_or(1)
}

fn message_from_channel_proto(
    m: &mezon_proto::api::ChannelMessage,
    message_id: i64,
    cfg: Option<&AppConfig>,
    viewer_id: Option<UserId>,
) -> Message {
    let mut api_msg = MezonTransport::message_from_proto(m);
    api_msg.message_id = message_id;
    message_from_api(api_msg, cfg, viewer_id)
}

fn merge_message_update(existing: &mut Message, incoming: &Message) {
    existing.content = incoming.content.clone();
    existing.spans = incoming.spans.clone();
    existing.rich_layout = incoming.rich_layout.clone();
    let prior_attachments = std::mem::take(&mut existing.attachments);
    if incoming.attachments.is_empty() {
        existing.attachments = prior_attachments;
    } else {
        let mut new_attachments = incoming.attachments.clone();
        if prior_attachments.len() == new_attachments.len() {
            for (att, prior_att) in new_attachments.iter_mut().zip(prior_attachments.iter()) {
                if att.local_source.is_none() {
                    att.local_source = prior_att.local_source.clone();
                }
                att.uploading = prior_att.uploading;
            }
        }
        existing.attachments = new_attachments;
    }
    existing.references = incoming.references.clone();
    existing.update_time = incoming.update_time;
    existing.is_edited = incoming.is_edited;
    existing.ogp = incoming.ogp.clone();
    existing.embeds = incoming.embeds.clone();
    existing.components = incoming.components.clone();
    existing.call_log = incoming.call_log;
    existing.invite = incoming.invite.clone();
    existing.is_card = incoming.is_card;
    existing.is_only_emoji = incoming.is_only_emoji;
    existing.is_deleted_placeholder = incoming.is_deleted_placeholder;
    existing.topic_id = incoming.topic_id;
    existing.topic_creator_id = incoming.topic_creator_id;
    existing.highlights_viewer_direct = incoming.highlights_viewer_direct;
    if incoming.poll.is_some() {
        existing.poll = incoming.poll.clone();
    }
    existing.code = if existing.poll.is_some() {
        MessageCode::Poll
    } else {
        MessageCode::Chat
    };
}

fn patch_reply_previews_after_update(
    messages: &mut MessageList,
    updated_id: MessageId,
    new_content: &str,
) {
    for msg in messages.items.iter_mut() {
        for reference in msg.references.iter_mut() {
            if reference.message_ref_id == updated_id {
                reference.content = new_content.to_string();
                reference.content_preview = crate::message::reply_preview_line(new_content).into();
            }
        }
    }
}

fn patch_reply_previews_after_delete(messages: &mut MessageList, deleted_id: MessageId) {
    for msg in messages.items.iter_mut() {
        for reference in msg.references.iter_mut() {
            if reference.message_ref_id == deleted_id {
                reference.content = DELETED_REPLY_PREVIEW.to_string();
                reference.content_preview =
                    crate::message::reply_preview_line(DELETED_REPLY_PREVIEW).into();
                reference.message_ref_id = MessageId(0);
            }
        }
    }
}

/// Whether newer messages exist on the server that are not in the loaded buffer.
fn has_more_bottom_for(last_message_id: Option<MessageId>, messages: &MessageList) -> bool {
    let Some(last_id) = last_message_id.filter(|id| !id.is_zero() && !id.is_optimistic()) else {
        return false;
    };
    if messages.is_empty() {
        return false;
    }
    !messages.contains_id(last_id)
}

/// Whether there is more history above the loaded buffer, mirroring React
/// `hasMore = lastLoadMessage?.code !== EMessageCode.FIRST_MESSAGE`
/// (`messages.slice.ts`). The very first message of a channel carries code 4
/// (`FIRST_MESSAGE`, which we map to `MessageCode::Indicator`); once it is the
/// oldest loaded row there is nothing older to fetch. An empty buffer has
/// nothing more to load.
fn has_more_from_oldest(messages: &[Message]) -> bool {
    messages
        .first()
        .is_some_and(|m| m.code != MessageCode::Indicator)
}

fn prepare_messages(
    msgs: Vec<ApiMessage>,
    cfg: Option<&AppConfig>,
    viewer_id: Option<UserId>,
) -> Vec<Message> {
    let mut messages: Vec<Message> = msgs
        .into_iter()
        .filter(|m| {
            !matches!(
                MessageCode::from_raw(m.code),
                MessageCode::ChatUpdate
                    | MessageCode::ChatRemove
                    | MessageCode::Typing
                    | MessageCode::UpdateEphemeralMsg
                    | MessageCode::DeleteEphemeralMsg
            )
        })
        .map(|m| message_from_api(m, cfg, viewer_id))
        .collect();
    sort_messages(&mut messages);
    trim_messages(&mut messages);
    recompute_message_grouping(&mut messages);
    messages
}

/// Cap the buffer to `MAX_MESSAGES_PER_CHANNEL`, dropping the oldest rows.
/// Returns how many rows were dropped from the front. Used when newer rows are
/// appended (the window slides toward the present).
fn trim_messages(messages: &mut Vec<Message>) -> usize {
    if messages.len() <= MAX_MESSAGES_PER_CHANNEL {
        return 0;
    }
    let drop = messages.len() - MAX_MESSAGES_PER_CHANNEL;
    messages.drain(0..drop);
    drop
}

/// Cap the buffer to `MAX_MESSAGES_PER_CHANNEL`, dropping the newest rows.
/// Returns how many rows were dropped from the back. Used when older rows are
/// prepended (the window slides toward history) so the just-loaded older rows
/// are kept; the dropped newest rows can be re-fetched via `load_more_bottom`.
fn trim_messages_back(messages: &mut Vec<Message>) -> usize {
    if messages.len() <= MAX_MESSAGES_PER_CHANNEL {
        return 0;
    }
    let drop = messages.len() - MAX_MESSAGES_PER_CHANNEL;
    messages.truncate(MAX_MESSAGES_PER_CHANNEL);
    drop
}

/// Display name + avatar for an outgoing optimistic row (React `fakeItUntilYouMakeIt`).
fn outgoing_sender_profile(
    sender_id: &str,
    fallback_username: &str,
    clan_id: ClanId,
    cx: &App,
) -> (String, String, SharedString) {
    let user_id = sender_id.parse::<i64>().ok().map(UserId);

    let display_name = user_id
        .and_then(|uid| {
            ClanMembersStore::try_global(cx).and_then(|store| {
                store
                    .read(cx)
                    .member(clan_id, uid)
                    .map(|member| member.name().to_string())
            })
        })
        .filter(|name| !name.is_empty())
        .or_else(|| {
            let account = AccountStore::global(cx).read(cx);
            account
                .clan_profile
                .as_ref()
                .filter(|profile| profile.clan_id == clan_id && !profile.nick_name.is_empty())
                .map(|profile| profile.nick_name.clone())
                .or_else(|| {
                    account.account.as_ref().map(|acct| {
                        if !acct.display_name.is_empty() {
                            acct.display_name.clone()
                        } else {
                            acct.username.clone()
                        }
                    })
                })
        })
        .unwrap_or_else(|| fallback_username.to_string());

    let avatar_url = user_id
        .and_then(|uid| {
            ClanMembersStore::try_global(cx).and_then(|store| {
                store
                    .read(cx)
                    .member(clan_id, uid)
                    .map(|member| member.avatar().to_string())
            })
        })
        .filter(|avatar| !avatar.is_empty())
        .or_else(|| {
            AccountStore::global(cx)
                .read(cx)
                .clan_profile
                .as_ref()
                .filter(|profile| profile.clan_id == clan_id)
                .and_then(|profile| profile.avatar_url.clone())
        })
        .or_else(|| {
            AccountStore::global(cx)
                .read(cx)
                .account
                .as_ref()
                .and_then(|acct| acct.avatar_url.clone())
        })
        .unwrap_or_default();

    let avatar_proxied = AppConfig::try_global(cx)
        .map(|cfg| cfg.avatar_proxy(&avatar_url))
        .unwrap_or_else(|| avatar_url.clone());

    (display_name, avatar_url, avatar_proxied.into())
}

fn carry_local_previews(prior: &Message, confirmed: &mut Message) {
    if prior.attachments.len() != confirmed.attachments.len() {
        return;
    }
    for (att, prior_att) in confirmed
        .attachments
        .iter_mut()
        .zip(prior.attachments.iter())
    {
        if att.local_source.is_none() {
            att.local_source = prior_att.local_source.clone();
        }
        att.uploading = prior_att.uploading;
    }
}

fn merge_sparse_sender(prior: &Message, mut incoming: Message) -> Message {
    if incoming.sender_id.is_empty() || incoming.sender_id == "0" {
        incoming.sender_id = prior.sender_id.clone();
        incoming.sender_user_id = prior.sender_user_id;
    }
    if incoming.sender_name.is_empty() {
        incoming.sender_name = prior.sender_name.clone();
    }
    if incoming.avatar_url.is_empty() {
        incoming.avatar_url = prior.avatar_url.clone();
        incoming.avatar_proxied = prior.avatar_proxied.clone();
    }
    if prior.id.is_optimistic() {
        incoming.create_time = prior.create_time;
        incoming.day_label = prior.day_label.clone();
        incoming.row_anchor_id = prior.row_anchor_id;
    }
    if incoming.references.is_empty() && !prior.references.is_empty() {
        incoming.references = prior.references.clone();
    }
    carry_local_previews(prior, &mut incoming);
    incoming
}

/// Client send timestamp for an optimistic row (React `client_send_time / 1000`).
/// Keeps times strictly increasing within a same-sender burst so combine matches
/// before and after ack.
fn optimistic_create_time(messages: &MessageList, sender_id: &str) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    optimistic_create_time_at(messages, sender_id, now)
}

fn optimistic_create_time_at(messages: &MessageList, sender_id: &str, now: i64) -> i64 {
    let Some(last) = messages.last() else {
        return now;
    };
    let probe = Message::new(MessageId(0), "", sender_id, "", now);
    if message_combined_with_prev(Some(last), &probe) {
        last.create_time.max(now) + 1
    } else {
        now
    }
}

fn viewer_user_id(cx: &App) -> Option<UserId> {
    BadgeService::try_global(cx)?.read(cx).current_user_id(cx)
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn message_from_api(m: ApiMessage, cfg: Option<&AppConfig>, viewer_id: Option<UserId>) -> Message {
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&m.avatar))
        .unwrap_or_else(|| m.avatar.clone());
    let mut spans = parse_spans(&m.content_tokens);
    if let Some(cfg) = cfg {
        for span in &mut spans {
            if let MessageSpan::Emoji { emoji_id, src, .. } = span
                && !emoji_id.is_empty()
            {
                *src = cfg.emoji_src(emoji_id).into();
            }
        }
    }
    let mention_targets: Vec<MentionTarget> = m
        .entity_mentions
        .iter()
        .map(|mention| MentionTarget {
            user_id: (mention.user_id != 0).then(|| mention.user_id.to_string()),
            role_id: (mention.role_id != 0).then(|| mention.role_id.to_string()),
        })
        .collect();
    let references: Vec<MessageReference> = m
        .references
        .iter()
        .map(|r| message_reference_from_api(r, cfg))
        .collect();
    let reactions = aggregate_reactions(&m.reactions, cfg);
    let mut attachments: Vec<MessageAttachment> = m
        .attachments
        .into_iter()
        .map(|a| MessageAttachment::from_api(a, cfg))
        .collect();
    if let Some(keys) = presign::parse_presign_finish_keys(&m.content) {
        let base_img = cfg.map(|c| c.base_img_url.as_str()).unwrap_or_default();
        let now = now_unix_seconds();
        attachments.retain(|a| {
            !presign::is_expired_presign_attachment(
                &a.url,
                Some(&keys),
                base_img,
                m.create_time,
                now,
            )
        });
        for a in attachments.iter_mut() {
            a.presign_pending = presign::presign_pending(&a.url, Some(&keys), base_img);
        }
    }
    let (album_layout, viewer_media) = build_media_presentation(&attachments, cfg);
    let is_forwarded = m.content_tokens.fwd;
    let ogp = build_ogp_preview(&m.content_tokens, cfg);
    let code = MessageCode::from_raw(m.code);
    let poll = build_poll_data(&m.content_tokens, &m.content, cfg);
    let call_log = build_call_log(&m.content_tokens);
    let token_transaction = (code == MessageCode::SendToken)
        .then(|| Box::new(crate::message::split_token_transaction(&m.content)));
    let embeds = build_embeds(&m.content_tokens, cfg);
    let components = build_components(&m.content_tokens);
    let invite = build_invite(&m.content_tokens, cfg);
    let is_only_emoji = spans_only_emoji(&spans);
    let is_deleted_placeholder =
        code == MessageCode::ChatRemove || m.content == DELETED_REPLY_PREVIEW;
    let highlight = viewer_highlight_direct(&references, &mention_targets, &spans, viewer_id);
    let topic_id = m
        .content_tokens
        .tp
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&id| id != 0)
        .map(ChannelId);
    let topic_creator_id = m
        .content_tokens
        .cid
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&id| id != 0)
        .map(UserId);
    if !mention_targets.is_empty() {
        tracing::debug!(
            message_id = m.message_id,
            entity = mention_targets.len(),
            span_mentions = spans
                .iter()
                .filter(|s| matches!(s, crate::message::MessageSpan::Mention { .. }))
                .count(),
            "message mention targets parsed"
        );
    }
    Message::new(
        MessageId(m.message_id),
        m.content,
        m.sender_id.to_string(),
        m.sender_name,
        m.create_time,
    )
    .with_code(code)
    .with_spans(spans)
    .with_mention_targets(mention_targets)
    .with_references(references)
    .with_reactions(reactions)
    .with_edited(m.update_time, m.hide_editted)
    .with_forwarded(is_forwarded)
    .with_ogp(ogp)
    .with_poll(poll)
    .with_call_log(call_log)
    .with_embeds(embeds)
    .with_components(components)
    .with_is_card(m.content_tokens.is_card)
    .with_topic(topic_id, topic_creator_id)
    .with_invite(invite)
    .with_token_transaction(token_transaction)
    .with_only_emoji(is_only_emoji)
    .with_deleted_placeholder(is_deleted_placeholder)
    .with_viewer_highlight(highlight)
    .with_avatar(m.avatar)
    .with_avatar_proxied(avatar_proxied)
    .with_attachments(attachments)
    .with_media_presentation(album_layout, viewer_media)
}

fn map_poll_detail(
    resp: &mezon_proto::api::GetPollResponse,
    clan_id: ClanId,
    cx: &App,
) -> PollDetail {
    let members_entity = ClanMembersStore::global(cx);
    let members = members_entity.read(cx);
    let cfg = AppConfig::try_global(cx);
    let answer_count = resp.answers.len().max(resp.answer_counts.len());
    let mut voters_by_answer: Vec<Vec<PollVoter>> = vec![Vec::new(); answer_count];
    for detail in &resp.voter_details {
        let idx = detail.answer_index.max(0) as usize;
        let Some(slot) = voters_by_answer.get_mut(idx) else {
            continue;
        };
        for &uid in &detail.user_ids {
            let user_id = UserId(uid);
            let voter = match members.member(clan_id, user_id) {
                Some(member) => {
                    let avatar = member.avatar();
                    let avatar_proxied = cfg
                        .map(|c| c.avatar_proxy(avatar))
                        .unwrap_or_else(|| avatar.to_string());
                    PollVoter {
                        user_id,
                        display_name: member.name().to_string().into(),
                        username: member.user.username.clone().into(),
                        avatar_proxied: avatar_proxied.into(),
                    }
                }
                None => PollVoter {
                    user_id,
                    display_name: uid.to_string().into(),
                    username: SharedString::default(),
                    avatar_proxied: SharedString::default(),
                },
            };
            slot.push(voter);
        }
    }
    PollDetail {
        total_votes: resp.total_votes,
        answer_counts: resp.answer_counts.clone(),
        voters_by_answer,
    }
}

fn build_ogp_preview(
    content: &ApiMessageContent,
    cfg: Option<&AppConfig>,
) -> Option<Box<OgpPreview>> {
    let token = content.mk.iter().find(|tok| {
        tok.kind.as_deref() == Some("lk_ogp")
            && tok
                .url
                .as_deref()
                .is_some_and(|url| !url.to_ascii_lowercase().contains("/invite/"))
    })?;
    let url = token.url.clone().unwrap_or_default();
    if url.is_empty() {
        return None;
    }
    let image = token.image.as_deref().unwrap_or_default();
    let image_proxied = cfg
        .map(|c| c.imgproxy_url(image, 350, 200, "fit"))
        .unwrap_or_else(|| image.to_string());
    Some(Box::new(OgpPreview {
        url,
        title: token.title.clone().unwrap_or_default().into(),
        description: token.description.clone().unwrap_or_default().into(),
        image_proxied: image_proxied.into(),
    }))
}

fn proxy_image(url: &str, width: u32, height: u32, cfg: Option<&AppConfig>) -> String {
    cfg.map(|c| c.imgproxy_url(url, width, height, "fit"))
        .unwrap_or_else(|| url.to_string())
}

fn proxy_icon(url: &str, cfg: Option<&AppConfig>) -> String {
    cfg.map(|c| c.avatar_proxy(url))
        .unwrap_or_else(|| url.to_string())
}

fn text_to_spans(text: &str) -> Vec<MessageSpan> {
    if text.is_empty() {
        return Vec::new();
    }
    let markdowns = detect_markdown(text);
    parse_spans(&ApiMessageContent {
        t: text.to_string(),
        mk: markdown_content_tokens(&markdowns),
        ..Default::default()
    })
}

type EditTransportTokens = (
    Vec<MessageSpan>,
    Vec<TransportMention>,
    Vec<TransportHashtag>,
    Vec<TransportEmoji>,
);

fn edit_content_spans(content: &str, content_tokens: OutgoingContent) -> EditTransportTokens {
    let OutgoingContent {
        mentions,
        hashtags,
        emojis,
    } = content_tokens;
    let transport_mentions: Vec<TransportMention> = mentions
        .into_iter()
        .map(OutgoingMention::into_transport)
        .collect();
    let transport_hashtags: Vec<TransportHashtag> = hashtags
        .into_iter()
        .map(OutgoingHashtag::into_transport)
        .collect();
    let transport_emojis: Vec<TransportEmoji> = emojis
        .into_iter()
        .map(OutgoingEmoji::into_transport)
        .collect();
    let markdowns = detect_markdown(content);
    let tokens = ApiMessageContent {
        t: content.to_string(),
        mentions: mention_content_tokens(&transport_mentions),
        hg: hashtag_content_tokens(&transport_hashtags),
        ej: emoji_content_tokens(&transport_emojis),
        mk: markdown_content_tokens(&markdowns),
        ..Default::default()
    };
    let spans = parse_spans(&tokens);
    (
        spans,
        transport_mentions,
        transport_hashtags,
        transport_emojis,
    )
}

fn parse_embed_accent(color: Option<&str>) -> Option<Rgba> {
    let raw = color?.trim().trim_start_matches('#');
    if raw.len() != 6 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(raw, 16).ok().map(gpui::rgb)
}

fn build_call_log(content: &ApiMessageContent) -> Option<CallLog> {
    let raw = content.call_log.as_ref()?;
    Some(CallLog {
        is_video: raw.is_video,
        log_type: CallLogType::from_raw(raw.call_log_type),
        show_call_back: raw.show_call_back,
    })
}

fn build_embeds(content: &ApiMessageContent, cfg: Option<&AppConfig>) -> Arc<[Embed]> {
    if content.embed.is_empty() {
        return Vec::new().into();
    }
    content
        .embed
        .iter()
        .map(|embed| build_embed(embed, cfg))
        .collect::<Vec<_>>()
        .into()
}

fn build_embed(embed: &ApiEmbed, cfg: Option<&AppConfig>) -> Embed {
    let author = embed
        .author
        .as_ref()
        .filter(|a| !a.name.is_empty() || a.icon_url.is_some())
        .map(|a| EmbedAuthor {
            name: a.name.clone().into(),
            icon_proxied: a
                .icon_url
                .as_deref()
                .map(|url| proxy_icon(url, cfg))
                .unwrap_or_default()
                .into(),
            url: a.url.clone().map(Into::into),
        });
    let thumbnail_proxied = embed
        .thumbnail
        .as_ref()
        .filter(|t| !t.url.is_empty())
        .map(|t| proxy_image(&t.url, 64, 64, cfg))
        .unwrap_or_default()
        .into();
    let image = embed.image.as_ref().filter(|i| !i.url.is_empty()).map(|i| {
        let width = i.width.map(|v| v.max(0) as u32);
        let height = i.height.map(|v| v.max(0) as u32);
        EmbedImage {
            url_proxied: proxy_image(&i.url, width.unwrap_or(400), height.unwrap_or(300), cfg)
                .into(),
            width,
            height,
        }
    });
    let footer = embed
        .footer
        .as_ref()
        .filter(|f| !f.text.is_empty() || f.icon_url.is_some())
        .map(|f| EmbedFooter {
            text: f.text.clone().into(),
            icon_proxied: f
                .icon_url
                .as_deref()
                .map(|url| proxy_icon(url, cfg))
                .unwrap_or_default()
                .into(),
        });
    let fields: Arc<[EmbedField]> = embed
        .fields
        .iter()
        .map(|field| EmbedField {
            name: field.name.clone().into(),
            value: field.value.clone().into(),
            inline: field.inline,
        })
        .collect::<Vec<_>>()
        .into();
    let timestamp: SharedString = embed.timestamp.clone().unwrap_or_default().into();
    let footer_date = format_embed_footer_date(&timestamp);
    Embed {
        accent: parse_embed_accent(embed.color.as_deref()),
        title: embed.title.clone().unwrap_or_default().into(),
        url: embed.url.clone().map(Into::into),
        author,
        description_spans: text_to_spans(embed.description.as_deref().unwrap_or_default()),
        thumbnail_proxied,
        image,
        footer,
        fields,
        timestamp,
        footer_date,
    }
}

fn format_embed_footer_date(raw: &SharedString) -> SharedString {
    if raw.is_empty() {
        return raw.clone();
    }
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => SharedString::from(dt.format("%-m/%-d/%Y").to_string()),
        Err(_) => raw.clone(),
    }
}

fn build_components(content: &ApiMessageContent) -> Arc<[MessageComponentRow]> {
    if content.components.is_empty() {
        return Vec::new().into();
    }
    content
        .components
        .iter()
        .map(build_component_row)
        .collect::<Vec<_>>()
        .into()
}

fn build_component_row(row: &ApiActionRow) -> MessageComponentRow {
    MessageComponentRow {
        components: row.components.iter().map(build_component).collect(),
    }
}

fn build_component(component: &ApiMessageComponent) -> MessageComponent {
    match &component.component {
        ApiComponentPayload::Button(b) => MessageComponent::Button(MessageButton {
            label: b.label.clone().into(),
            style: b.style.unwrap_or(1),
            url: b.url.clone().map(Into::into),
            disable: b.disable,
            id: component.id.clone().map(Into::into),
            icon: b.icon.clone().map(Into::into),
        }),
        ApiComponentPayload::Select(s) => MessageComponent::Select(MessageSelect {
            select_type: s.select_type.unwrap_or(1),
            options: s
                .options
                .iter()
                .map(|o| MessageSelectOption {
                    label: o.label.clone().into(),
                    value: o.value.clone().into(),
                    description: o.description.clone().map(Into::into),
                    default: o.default,
                })
                .collect(),
            placeholder: s.placeholder.clone().map(Into::into),
            min_options: s.min_options,
            max_options: component.max_options.or(s.max_options),
            disabled: s.disabled,
            id: component.id.clone().map(Into::into),
        }),
        ApiComponentPayload::Other(_) => MessageComponent::Other,
    }
}

fn build_invite(
    content: &ApiMessageContent,
    cfg: Option<&AppConfig>,
) -> Option<Box<InvitePreview>> {
    let token = content.mk.iter().find(|tok| {
        tok.kind.as_deref() == Some("lk_ogp")
            && tok
                .url
                .as_deref()
                .is_some_and(|url| url.to_ascii_lowercase().contains("/invite/"))
    })?;
    let url = token.url.clone().unwrap_or_default();
    if url.is_empty() {
        return None;
    }
    let image = token.image.as_deref().unwrap_or_default();
    let banner = token.banner.as_deref().unwrap_or_default();
    Some(Box::new(InvitePreview {
        title: token.title.clone().unwrap_or_default().into(),
        image_proxied: proxy_image(image, 72, 72, cfg).into(),
        banner_proxied: proxy_image(banner, 350, 76, cfg).into(),
        member_count: token.member_count.unwrap_or(0),
        is_community: token.is_community,
        clan_id: token.clan_id.clone(),
        url,
        is_error: token.title.as_deref() == Some("Invite Error"),
    }))
}

fn poll_label_segments(label: &str, cfg: Option<&AppConfig>) -> Vec<PollLabelSegment> {
    let mut segments = Vec::new();
    let mut rest = label;
    while let Some(start) = rest.find("[e:") {
        if start > 0 {
            segments.push(PollLabelSegment::Text(rest[..start].into()));
        }
        let after = &rest[start + 3..];
        let Some(end) = after.find(']') else {
            segments.push(PollLabelSegment::Text(rest[start..].into()));
            return segments;
        };
        let emoji_id = &after[..end];
        let src = cfg.map(|c| c.emoji_src(emoji_id)).unwrap_or_default();
        segments.push(PollLabelSegment::Emoji(src.into()));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        segments.push(PollLabelSegment::Text(rest.into()));
    }
    segments
}

fn build_poll_data(
    content: &ApiMessageContent,
    text: &str,
    cfg: Option<&AppConfig>,
) -> Option<Box<PollData>> {
    let (question, raw_answers, allow_multiple) =
        if content.question.is_some() || !content.answers.is_empty() {
            let answers: Vec<(Option<i64>, String)> = content
                .answers
                .iter()
                .map(|a| (a.index, a.label.clone()))
                .collect();
            (
                content.question.clone().unwrap_or_default(),
                answers,
                content.poll_type == Some(1),
            )
        } else {
            let parsed = parse_poll_markdown(text)?;
            let answers = parsed.answers.into_iter().map(|l| (None, l)).collect();
            (parsed.question, answers, parsed.allow_multiple)
        };
    if raw_answers.is_empty() {
        return None;
    }
    let answers: Vec<PollAnswerView> = raw_answers
        .into_iter()
        .enumerate()
        .map(|(i, (index, label))| PollAnswerView {
            index: index.map(|v| v as i32).unwrap_or(i as i32),
            segments: poll_label_segments(&label, cfg),
            label: label.into(),
        })
        .collect();
    let answer_counts = normalise_answer_counts(&content.answer_counts, answers.len());
    let total_votes = content
        .total_votes
        .unwrap_or_else(|| answer_counts.iter().sum());
    let percentages = answer_counts
        .iter()
        .map(|&count| poll_percentage(count, total_votes))
        .collect();
    Some(Box::new(PollData {
        poll_id: content.poll_id.unwrap_or(0),
        question: question.into(),
        answers,
        answer_counts,
        total_votes,
        percentages,
        expire_at: content.expire_at,
        is_closed: content.is_closed,
        allow_multiple,
    }))
}

fn normalise_answer_counts(counts: &[i32], len: usize) -> Vec<i32> {
    let mut out = vec![0; len];
    for (slot, &count) in out.iter_mut().zip(counts.iter()) {
        *slot = count.max(0);
    }
    out
}

fn poll_percentage(count: i32, total: i32) -> u8 {
    if total <= 0 {
        return 0;
    }
    ((count.max(0) as f64 / total as f64) * 100.0).round() as u8
}

struct ParsedPollMarkdown {
    question: String,
    answers: Vec<String>,
    allow_multiple: bool,
}

fn parse_poll_markdown(text: &str) -> Option<ParsedPollMarkdown> {
    if !text.starts_with('📊') {
        return None;
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let question = lines
        .first()
        .map(|l| l.replace('📊', "").replace("**", "").trim().to_string())
        .unwrap_or_default();
    let mut answers = Vec::new();
    for line in &lines {
        let trimmed = line.trim_start();
        if let Some(dot) = trimmed.find('.')
            && trimmed[..dot].chars().all(|c| c.is_ascii_digit())
            && dot > 0
        {
            let answer = trimmed[dot + 1..].trim();
            if !answer.is_empty() {
                answers.push(answer.to_string());
            }
        }
    }
    let allow_multiple = text.contains("☑️ Multiple answers allowed");
    Some(ParsedPollMarkdown {
        question,
        answers,
        allow_multiple,
    })
}

fn build_media_presentation(
    attachments: &[MessageAttachment],
    cfg: Option<&AppConfig>,
) -> (Option<AlbumLayout>, Arc<[ViewerMedia]>) {
    let images: Vec<&MessageAttachment> = attachments
        .iter()
        .filter(|a| !a.is_unsupported_media() && !a.is_video() && a.is_image())
        .collect();
    if images.is_empty() {
        return (None, Vec::new().into());
    }
    let viewer_media: Arc<[ViewerMedia]> = images
        .iter()
        .map(|a| {
            let viewer_src = cfg
                .map(|c| c.imgproxy_url(&a.url, 1600, 900, "fit"))
                .unwrap_or_else(|| a.url.clone());
            ViewerMedia {
                url: a.url.clone().into(),
                filename: a.filename.clone().into(),
                viewer_src: viewer_src.into(),
            }
        })
        .collect();
    let album_layout = if images.len() >= 2 {
        let dims: Vec<(u32, u32)> = images.iter().map(|a| (a.width, a.height)).collect();
        Some(calculate_album_layout(&dims))
    } else {
        None
    };
    (album_layout, viewer_media)
}

fn message_reference_from_api(
    r: &mezon_client::transport::ApiMessageRef,
    cfg: Option<&AppConfig>,
) -> MessageReference {
    let sender_name = if !r.message_sender_clan_nick.is_empty() {
        r.message_sender_clan_nick.clone()
    } else if !r.message_sender_display_name.is_empty() {
        r.message_sender_display_name.clone()
    } else {
        r.message_sender_username.clone()
    };
    let parsed =
        serde_json::from_str::<mezon_client::transport::ApiMessageContent>(&r.content).ok();
    let content = parsed
        .as_ref()
        .map(|c| c.t.clone())
        .unwrap_or_else(|| r.content.clone());
    let has_embed = parsed
        .as_ref()
        .is_some_and(|c| c.t.is_empty() && !c.embed.is_empty());
    let is_poll = parsed
        .as_ref()
        .is_some_and(|c| build_poll_data(c, &content, cfg).is_some());
    let sender_avatar = cfg
        .map(|c| c.avatar_proxy(&r.message_sender_avatar))
        .unwrap_or_else(|| r.message_sender_avatar.clone());
    let content_preview = crate::message::reply_preview_line(&content).into();
    MessageReference {
        message_ref_id: MessageId(r.message_ref_id),
        sender_id: UserId(r.message_sender_id),
        sender_name,
        sender_avatar,
        content,
        content_preview,
        has_attachment: r.has_attachment,
        has_embed,
        is_poll,
    }
}

impl MessageAttachment {
    pub(crate) fn from_api(
        a: mezon_client::transport::ApiAttachment,
        cfg: Option<&AppConfig>,
    ) -> Self {
        let width = a.width.max(0) as u32;
        let height = a.height.max(0) as u32;
        let (proxied_src, display_width, display_height) = cfg
            .map(|c| c.attachment_proxy(&a.url, width, height))
            .unwrap_or_else(|| {
                let (w, h) = crate::config::attachment_display_dimensions(width, height);
                (a.url.clone(), w, h)
            });
        let thumbnail_proxied: SharedString = if a.thumbnail.is_empty() {
            SharedString::default()
        } else {
            cfg.map(|c| {
                c.imgproxy_url(
                    &a.thumbnail,
                    display_width.ceil() as u32,
                    display_height.ceil() as u32,
                    "fit",
                )
            })
            .unwrap_or_else(|| a.thumbnail.clone())
            .into()
        };
        let tenor_mp4 = crate::message::tenor_mp4_url(&a.url).map(SharedString::from);
        let size = a.size.max(0) as u64;
        Self {
            url: a.url,
            filename: a.filename,
            filetype: a.filetype,
            width,
            height,
            thumbnail: a.thumbnail,
            duration: a.duration,
            size,
            size_label: crate::message::format_file_size(size).into(),
            presign_pending: false,
            proxied_src: proxied_src.into(),
            thumbnail_proxied,
            display_width,
            display_height,
            tenor_mp4,
            local_source: None,
            uploading: false,
        }
    }

    pub(crate) fn optimistic_local(att: &OutgoingAttachment) -> Self {
        let width = att.width.max(0) as u32;
        let height = att.height.max(0) as u32;
        let (display_width, display_height) =
            crate::config::attachment_display_dimensions(width, height);
        let is_image = att.filetype.starts_with("image/");
        Self {
            url: String::new(),
            filename: att.filename.clone(),
            filetype: att.filetype.clone(),
            width,
            height,
            thumbnail: String::new(),
            duration: 0,
            size: 0,
            size_label: SharedString::default(),
            presign_pending: false,
            proxied_src: SharedString::default(),
            thumbnail_proxied: SharedString::default(),
            display_width,
            display_height,
            tenor_mp4: None,
            local_source: is_image.then(|| att.path.clone()),
            uploading: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::UserId;
    use crate::message::MessageSpan;

    #[test]
    fn text_to_spans_parses_embed_code_block() {
        let spans = text_to_spans("```1 - timesheet.nccsoft.vn 2 - ims.nccsoft.vn```");
        assert!(
            spans
                .iter()
                .any(|s| matches!(s, MessageSpan::CodeBlock { .. })),
            "embed description should parse a ``` fence into a CodeBlock span"
        );
    }

    #[test]
    fn plan_forwards_builds_one_send_per_target() {
        let targets = vec![
            ForwardTarget {
                clan_id: ClanId(1),
                channel_id: ChannelId(10),
                channel_type: 1,
                mode: 2,
                is_public: true,
            },
            ForwardTarget {
                clan_id: ClanId(0),
                channel_id: ChannelId(20),
                channel_type: 4,
                mode: 3,
                is_public: false,
            },
        ];
        let attachment = mezon_client::transport::ApiAttachment {
            url: "https://cdn.mezon.ai/a.png".to_string(),
            filename: "a.png".to_string(),
            filetype: "image/png".to_string(),
            ..Default::default()
        };
        let plans = plan_forwards(
            "hi",
            std::slice::from_ref(&attachment),
            &targets,
            Some("note"),
        );
        assert_eq!(plans.len(), targets.len());
        assert_eq!(plans[0].channel_id, 10);
        assert_eq!(plans[0].content, "hi");
        assert_eq!(plans[0].note.as_deref(), Some("note"));
        assert_eq!(plans[0].attachments.len(), 1);
        assert_eq!(plans[1].channel_id, 20);
        assert_eq!(plans[1].mode, 3);
        assert!(!plans[1].is_public);
    }

    #[test]
    fn poll_percentage_rounds_ratio_and_guards_zero_total() {
        assert_eq!(poll_percentage(1, 4), 25);
        assert_eq!(poll_percentage(1, 3), 33);
        assert_eq!(poll_percentage(2, 3), 67);
        assert_eq!(poll_percentage(0, 0), 0);
        assert_eq!(poll_percentage(-5, 10), 0);
    }

    #[test]
    fn normalise_answer_counts_pads_and_clamps_negatives() {
        assert_eq!(normalise_answer_counts(&[5, -2], 3), vec![5, 0, 0]);
        assert_eq!(normalise_answer_counts(&[1, 2, 3], 2), vec![1, 2]);
    }

    #[test]
    fn parse_poll_markdown_extracts_question_answers_and_multiple_flag() {
        let text = "📊 **Favourite colour?**\n1. Red\n2. Blue\n☑️ Multiple answers allowed";
        let parsed = parse_poll_markdown(text).expect("poll markdown");
        assert_eq!(parsed.question, "Favourite colour?");
        assert_eq!(parsed.answers, vec!["Red".to_string(), "Blue".to_string()]);
        assert!(parsed.allow_multiple);
    }

    #[test]
    fn parse_poll_markdown_rejects_non_poll_text() {
        assert!(parse_poll_markdown("just a normal message").is_none());
    }

    #[test]
    fn build_poll_data_from_structured_content_computes_percentages() {
        let content = ApiMessageContent {
            question: Some("Q?".into()),
            answers: vec![
                mezon_client::transport::ApiPollAnswer {
                    index: Some(0),
                    label: "A".into(),
                },
                mezon_client::transport::ApiPollAnswer {
                    index: Some(1),
                    label: "B".into(),
                },
            ],
            answer_counts: vec![3, 1],
            total_votes: Some(4),
            poll_id: Some(77),
            poll_type: Some(1),
            ..Default::default()
        };
        let poll = build_poll_data(&content, "", None).expect("poll data");
        assert_eq!(poll.poll_id, 77);
        assert_eq!(poll.total_votes, 4);
        assert_eq!(poll.percentages, vec![75, 25]);
        assert_eq!(poll.answer_counts, vec![3, 1]);
        assert_eq!(poll.answers.len(), 2);
        assert!(poll.allow_multiple);
    }

    #[test]
    fn build_poll_data_falls_back_to_markdown_content() {
        let text = "📊 Pick one\n1. Yes\n2. No";
        let poll =
            build_poll_data(&ApiMessageContent::default(), text, None).expect("markdown poll");
        assert_eq!(poll.question, "Pick one");
        assert_eq!(poll.answers.len(), 2);
        assert!(!poll.allow_multiple);
    }

    #[test]
    fn build_poll_data_none_without_answers() {
        assert!(build_poll_data(&ApiMessageContent::default(), "no poll here", None).is_none());
    }

    #[test]
    fn channel_join_params_thread_by_type_joins_as_thread() {
        assert_eq!(
            channel_join_params(ChannelType::Thread, None, false),
            (false, CHANNEL_TYPE_THREAD, STREAM_MODE_THREAD)
        );
    }

    #[test]
    fn channel_join_params_thread_by_parent_joins_as_thread() {
        assert_eq!(
            channel_join_params(ChannelType::Text, Some(ChannelId(99)), true),
            (false, CHANNEL_TYPE_THREAD, STREAM_MODE_THREAD)
        );
    }

    #[test]
    fn channel_join_params_public_channel_keeps_channel_type() {
        assert_eq!(
            channel_join_params(ChannelType::Text, None, false),
            (true, CHANNEL_TYPE_CHANNEL, STREAM_MODE_CHANNEL)
        );
    }

    #[test]
    fn channel_join_params_private_channel_is_not_public() {
        assert_eq!(
            channel_join_params(ChannelType::Text, None, true),
            (false, CHANNEL_TYPE_CHANNEL, STREAM_MODE_CHANNEL)
        );
    }

    #[test]
    fn outgoing_mention_maps_to_transport_with_utf16_offsets() {
        let mention = OutgoingMention {
            user_id: "42".into(),
            role_id: String::new(),
            display: "@bob".into(),
            s: 2,
            e: 6,
        };
        let transport = mention.into_transport();
        assert_eq!(transport.user_id, "42");
        assert_eq!(transport.username, "@bob");
        assert_eq!(transport.s, 2);
        assert_eq!(transport.e, 6);
    }

    #[test]
    fn sticker_attachment_is_recognized_as_image() {
        let attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: "https://cdn/1.webp".into(),
                filename: "1".into(),
                filetype: STICKER_FILETYPE.into(),
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
                size: 0,
            },
            None,
        );
        assert_eq!(attachment.filetype, "sticker");
        assert_eq!(attachment.url, "https://cdn/1.webp");
        assert!(attachment.is_image());
        assert_eq!(
            (attachment.display_width, attachment.display_height),
            (100.0, 100.0)
        );
    }

    #[test]
    fn video_attachment_maps_thumbnail_and_duration() {
        let attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: "https://cdn/clip.mp4".into(),
                filename: "clip.mp4".into(),
                filetype: "video/mp4".into(),
                width: 1280,
                height: 720,
                thumbnail: "https://cdn/clip-thumb.jpg".into(),
                duration: 42,
                size: 0,
            },
            None,
        );
        assert!(attachment.is_video());
        assert!(!attachment.is_image());
        assert_eq!(attachment.duration, 42);
        assert_eq!(attachment.thumbnail, "https://cdn/clip-thumb.jpg");
        assert_eq!(attachment.thumbnail_proxied, "https://cdn/clip-thumb.jpg");
    }

    #[test]
    fn optimistic_mention_tokens_round_trip_to_a_coloured_span() {
        let mentions = vec![OutgoingMention {
            user_id: "42".into(),
            role_id: String::new(),
            display: "@bob".into(),
            s: 0,
            e: 4,
        }];
        let transport: Vec<TransportMention> = mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let tokens = ApiMessageContent {
            t: "@bob hi".into(),
            mentions: mention_content_tokens(&transport),
            ..Default::default()
        };
        let spans = parse_spans(&tokens);
        assert_eq!(
            spans,
            vec![
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
                MessageSpan::Text(" hi".into()),
            ]
        );
    }

    #[test]
    fn edit_preserves_mention_token_and_does_not_strip_it() {
        let content_tokens = OutgoingContent {
            mentions: vec![OutgoingMention {
                user_id: "42".into(),
                role_id: String::new(),
                display: "@bob".into(),
                s: 0,
                e: 4,
            }],
            hashtags: Vec::new(),
            emojis: Vec::new(),
        };
        let (spans, transport_mentions, _, _) = edit_content_spans("@bob hi", content_tokens);

        assert!(
            transport_mentions.iter().any(|m| m.user_id == "42"),
            "edit must forward the mention token to the transport, not drop it"
        );
        assert_eq!(
            spans,
            vec![
                MessageSpan::Mention {
                    display: "@bob".into(),
                    user_id: Some("42".into()),
                    role_id: None,
                },
                MessageSpan::Text(" hi".into()),
            ],
            "edit must keep the mention span instead of collapsing to plain text"
        );
    }

    #[test]
    fn message_from_api_maps_fields() {
        let m = message_from_api(
            ApiMessage {
                message_id: 1,
                content: "hi".into(),
                content_tokens: mezon_client::transport::ApiMessageContent {
                    t: "hi".into(),
                    ..Default::default()
                },
                code: 0,
                sender_id: 1,
                sender_name: "Alice".into(),
                avatar: "av.png".into(),
                create_time: 100,
                update_time: 0,
                hide_editted: false,
                attachments: vec![],
                references: vec![],
                reactions: vec![],
                entity_mentions: vec![],
            },
            None,
            None,
        );
        assert_eq!(m.id, MessageId(1));
        assert_eq!(m.content, "hi");
        assert_eq!(m.sender_id, "1");
        assert_eq!(m.sender_user_id, Some(UserId(1)));
        assert_eq!(m.sender_name, "Alice");
        assert_eq!(m.avatar_url, "av.png");
        assert_eq!(m.avatar_proxied, "av.png");
    }

    #[test]
    fn message_from_api_precomputes_album_and_viewer_media() {
        let image = |url: &str| mezon_client::transport::ApiAttachment {
            url: url.into(),
            filename: "a.png".into(),
            filetype: "image/png".into(),
            width: 800,
            height: 600,
            thumbnail: String::new(),
            duration: 0,
            size: 0,
        };
        let m = message_from_api(
            ApiMessage {
                message_id: 5,
                content: String::new(),
                content_tokens: mezon_client::transport::ApiMessageContent::default(),
                code: 0,
                sender_id: 1,
                sender_name: "Alice".into(),
                avatar: String::new(),
                create_time: 0,
                update_time: 0,
                hide_editted: false,
                attachments: vec![image("https://cdn/1.png"), image("https://cdn/2.png")],
                references: vec![],
                reactions: vec![],
                entity_mentions: vec![],
            },
            None,
            None,
        );
        assert!(m.album_layout.is_some());
        assert_eq!(m.viewer_media.len(), 2);
        assert_eq!(m.viewer_media[0].url, "https://cdn/1.png");
        assert_eq!(m.viewer_media[0].viewer_src, "https://cdn/1.png");
    }

    fn plain_api_message(
        code: i32,
        references: Vec<mezon_client::transport::ApiMessageRef>,
    ) -> ApiMessage {
        ApiMessage {
            message_id: 11,
            content: "yo".into(),
            content_tokens: mezon_client::transport::ApiMessageContent {
                t: "yo".into(),
                ..Default::default()
            },
            code,
            sender_id: 3,
            sender_name: "Bob".into(),
            avatar: String::new(),
            create_time: 100,
            update_time: 0,
            hide_editted: false,
            attachments: vec![],
            references,
            reactions: vec![],
            entity_mentions: vec![],
        }
    }

    #[test]
    fn buzz_message_ingests_as_timeline_row() {
        let rows = prepare_messages(vec![plain_api_message(8, vec![])], None, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, MessageCode::MessageBuzz);
        assert!(rows[0].code.is_user_timeline());
    }

    #[test]
    fn reply_to_viewer_sets_highlights_viewer_direct() {
        let reply = || {
            plain_api_message(
                0,
                vec![mezon_client::transport::ApiMessageRef {
                    message_ref_id: 9,
                    message_sender_id: 42,
                    content: r#"{"t":"hi"}"#.into(),
                    ..Default::default()
                }],
            )
        };
        assert!(message_from_api(reply(), None, Some(UserId(42))).highlights_viewer_direct);
        assert!(!message_from_api(reply(), None, Some(UserId(7))).highlights_viewer_direct);
        assert!(!message_from_api(reply(), None, None).highlights_viewer_direct);
    }

    #[test]
    fn merge_sparse_sender_keeps_optimistic_avatar_and_name() {
        let optimistic = Message::new(MessageId::next_optimistic(), "2", "42", "huy.lexuan", 100)
            .with_avatar("avatar.png");
        let ack = Message::new(MessageId(99), "2", "0", String::new(), 500);
        let merged = merge_sparse_sender(&optimistic, ack);
        assert_eq!(merged.sender_id, "42");
        assert_eq!(merged.sender_name, "huy.lexuan");
        assert_eq!(merged.avatar_url, "avatar.png");
        assert_eq!(merged.create_time, 100);
        assert_eq!(merged.row_anchor_id, optimistic.row_anchor_id);
    }

    #[test]
    fn merge_sparse_sender_carries_local_preview() {
        let optimistic = Message::new(MessageId::next_optimistic(), "", "42", "Me", 100)
            .with_attachments(vec![MessageAttachment {
                filename: "photo.png".into(),
                local_source: Some(std::path::PathBuf::from("/tmp/photo.png")),
                ..Default::default()
            }]);
        let confirmed = Message::new(MessageId(99), "", "42", "Me", 500).with_attachments(vec![
            MessageAttachment {
                filename: "sanitized_photo.png".into(),
                url: "https://cdn.mezon.ai/photo.png".into(),
                ..Default::default()
            },
        ]);
        let merged = merge_sparse_sender(&optimistic, confirmed);
        assert_eq!(
            merged.attachments[0].local_source,
            Some(std::path::PathBuf::from("/tmp/photo.png"))
        );
    }

    #[test]
    fn merge_sparse_sender_carries_reply_reference() {
        let optimistic = Message::new(MessageId::next_optimistic(), "hi", "42", "Me", 100)
            .with_references(vec![MessageReference {
                message_ref_id: MessageId(7),
                sender_name: "Alice".into(),
                content: "original".into(),
                content_preview: "original".into(),
                ..Default::default()
            }]);
        let confirmed = Message::new(MessageId(99), "hi", "42", "Me", 500);
        assert!(confirmed.references.is_empty());
        let merged = merge_sparse_sender(&optimistic, confirmed);
        assert_eq!(merged.references.len(), 1);
        assert_eq!(merged.references[0].message_ref_id, MessageId(7));
        assert_eq!(merged.references[0].sender_name, "Alice");
    }

    #[test]
    fn merge_sparse_sender_keeps_confirmed_reference_when_present() {
        let optimistic = Message::new(MessageId::next_optimistic(), "hi", "42", "Me", 100)
            .with_references(vec![MessageReference {
                message_ref_id: MessageId(7),
                sender_name: "Stale".into(),
                ..Default::default()
            }]);
        let confirmed = Message::new(MessageId(99), "hi", "42", "Me", 500).with_references(vec![
            MessageReference {
                message_ref_id: MessageId(7),
                sender_name: "Fresh".into(),
                ..Default::default()
            },
        ]);
        let merged = merge_sparse_sender(&optimistic, confirmed);
        assert_eq!(merged.references[0].sender_name, "Fresh");
    }

    #[test]
    fn carry_local_previews_copies_local_path_by_index() {
        let prior = Message::new(MessageId::next_optimistic(), "", "42", "Me", 100)
            .with_attachments(vec![MessageAttachment {
                filename: "photo.png".into(),
                local_source: Some(std::path::PathBuf::from("/tmp/photo.png")),
                ..Default::default()
            }]);
        let mut confirmed = Message::new(MessageId(7), "", "42", "Me", 500).with_attachments(vec![
            MessageAttachment {
                filename: "1700_0_photo.png".into(),
                url: "https://cdn.mezon.ai/photo.png".into(),
                ..Default::default()
            },
        ]);
        carry_local_previews(&prior, &mut confirmed);
        assert_eq!(
            confirmed.attachments[0].local_source,
            Some(std::path::PathBuf::from("/tmp/photo.png"))
        );
    }

    #[test]
    fn optimistic_multi_image_gets_album_layout() {
        let outgoing = |name: &str| OutgoingAttachment {
            path: format!("/tmp/{name}").into(),
            filename: name.to_string(),
            filetype: "image/png".to_string(),
            width: 100,
            height: 100,
            poster_jpeg: None,
        };
        let atts = [outgoing("a.png"), outgoing("b.png")];
        let optimistic: Vec<MessageAttachment> = atts
            .iter()
            .map(MessageAttachment::optimistic_local)
            .collect();
        let (album_layout, _) = build_media_presentation(&optimistic, None);
        assert!(album_layout.is_some());
        assert!(optimistic.iter().all(|a| a.local_source.is_some()));
    }

    #[test]
    fn carry_local_previews_skips_on_count_mismatch() {
        let prior = Message::new(MessageId::next_optimistic(), "", "42", "Me", 100)
            .with_attachments(vec![MessageAttachment {
                local_source: Some(std::path::PathBuf::from("/tmp/a.png")),
                ..Default::default()
            }]);
        let mut confirmed = Message::new(MessageId(7), "", "42", "Me", 500);
        carry_local_previews(&prior, &mut confirmed);
        assert!(confirmed.attachments.is_empty());
    }

    #[test]
    fn optimistic_create_time_increments_within_same_sender_burst() {
        let now = 1_700_000_000i64;
        let mut list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "42", "Me", now - 5)]);
        assert_eq!(optimistic_create_time_at(&list, "42", now), now + 1);
        list.push_grouped(Message::new(
            MessageId::next_optimistic(),
            "b",
            "42",
            "Me",
            now + 1,
        ));
        assert_eq!(optimistic_create_time_at(&list, "42", now), now + 2);
    }

    #[test]
    fn optimistic_create_time_resets_after_combine_window() {
        let now = 1_700_000_000i64;
        let list = MessageList::from_messages(vec![Message::new(
            MessageId(1),
            "a",
            "42",
            "Me",
            now - 700,
        )]);
        assert_eq!(optimistic_create_time_at(&list, "42", now), now);
    }

    fn assert_list_consistent(list: &MessageList) {
        assert_eq!(list.index.len(), list.items.len());
        for (i, m) in list.items.iter().enumerate() {
            assert_eq!(list.index.get(&m.id), Some(&i));
        }
        let mut expected_temps: Vec<MessageId> = list
            .items
            .iter()
            .filter(|m| m.id.is_optimistic())
            .map(|m| m.id)
            .collect();
        let mut actual_temps: Vec<MessageId> = list.temp_ids.clone();
        actual_temps.sort();
        expected_temps.sort();
        assert_eq!(actual_temps, expected_temps);
    }

    #[test]
    fn push_message_grouped_appends_in_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U1", 100),
            Message::new(MessageId(2), "b", "u1", "U1", 110),
        ]);
        list.push_grouped(Message::new(MessageId(3), "c", "u1", "U1", 120));
        assert_eq!(list.len(), 3);
        assert_eq!(list.as_slice()[2].id, MessageId(3));
        assert!(list.as_slice()[2].combined_with_prev);
        assert_list_consistent(&list);
    }

    #[test]
    fn push_message_grouped_resorts_when_out_of_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U1", 100),
            Message::new(MessageId(3), "c", "u1", "U1", 120),
        ]);
        list.push_grouped(Message::new(MessageId(2), "b", "u1", "U1", 110));
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(1), MessageId(2), MessageId(3)]);
        assert_list_consistent(&list);
    }

    #[test]
    fn push_message_grouped_breaks_group_for_different_sender() {
        let mut list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U1", 100)]);
        list.push_grouped(Message::new(MessageId(2), "b", "u2", "U2", 105));
        assert!(!list.as_slice()[1].combined_with_prev);
        assert_list_consistent(&list);
    }

    #[test]
    fn trim_messages_drops_oldest() {
        let mut msgs: Vec<Message> = (0..MAX_MESSAGES_PER_CHANNEL + 5)
            .map(|i| Message::new(MessageId(i as i64), format!("m{i}"), "u", "User", i as i64))
            .collect();
        trim_messages(&mut msgs);
        assert_eq!(msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(msgs.first().unwrap().id, MessageId(5));
        assert_eq!(
            msgs.last().unwrap().id,
            MessageId((MAX_MESSAGES_PER_CHANNEL + 4) as i64)
        );
    }

    fn channel_msgs(msgs: Vec<Message>) -> ChannelMessages {
        ChannelMessages {
            messages: MessageList::from_messages(msgs),
            has_more: false,
        }
    }

    fn remove_temp_in(ch: &mut ChannelMessages, temp_id: MessageId) {
        ch.messages.remove_id(temp_id);
    }

    fn reconcile_temp_in(ch: &mut ChannelMessages, temp_id: MessageId, confirmed: Message) {
        if let Some(idx) = ch.messages.position(temp_id) {
            let temp = ch.messages.as_slice()[idx].clone();
            let merged = merge_sparse_sender(&temp, confirmed);
            ch.messages.replace_at_and_regroup(idx, merged);
        } else if !ch.messages.contains_id(confirmed.id) {
            ch.messages.push_sorted(confirmed);
        }
    }

    #[test]
    fn remove_temp_drops_message_by_id() {
        let temp1 = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![
            Message::new(temp1, "hello", "u1", "U", 100),
            Message::new(MessageId(2), "world", "u1", "U", 200),
        ]);
        remove_temp_in(&mut ch, temp1);
        assert_eq!(ch.messages.len(), 1);
        assert_eq!(ch.messages.as_slice()[0].id, MessageId(2));
        assert_list_consistent(&ch.messages);
    }

    #[test]
    fn remove_temp_noop_when_id_not_found() {
        let non_existent = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![Message::new(MessageId(1), "hello", "u1", "U", 100)]);
        remove_temp_in(&mut ch, non_existent);
        assert_eq!(ch.messages.len(), 1);
    }

    #[test]
    fn reconcile_temp_preserves_sender_from_optimistic_when_ack_sparse() {
        let temp_id = MessageId::next_optimistic();
        let temp = Message::new(temp_id, "hello", "42", "gia.chuvan", 1_700_000_000)
            .with_avatar("avatar.png")
            .with_avatar_proxied(gpui::SharedString::from("proxy.png"));
        let mut ch = channel_msgs(vec![temp]);
        let sparse_ack = Message::new(MessageId(99), "hello", "0", String::new(), 0);
        reconcile_temp_in(&mut ch, temp_id, sparse_ack);
        let row = &ch.messages.as_slice()[0];
        assert_eq!(row.id, MessageId(99));
        assert_eq!(row.sender_id, "42");
        assert_eq!(row.sender_name, "gia.chuvan");
        assert_eq!(row.avatar_url, "avatar.png");
        assert_eq!(row.create_time, 1_700_000_000);
    }

    #[test]
    fn reconcile_temp_matches_only_by_temp_id_not_content() {
        let temp1 = MessageId::next_optimistic();
        let temp2 = MessageId::next_optimistic();
        let mut ch = channel_msgs(vec![
            Message::new(temp1, "same text", "u1", "U", 100),
            Message::new(temp2, "same text", "u1", "U", 110),
        ]);
        let confirmed = Message::new(MessageId(42), "same text", "u1", "U", 120);
        reconcile_temp_in(&mut ch, temp1, confirmed);
        assert_eq!(ch.messages.len(), 2);
        assert_eq!(ch.messages.as_slice()[0].id, MessageId(42));
        assert_eq!(ch.messages.as_slice()[1].id, temp2);
        assert_list_consistent(&ch.messages);
    }

    #[test]
    fn temp_match_reconciles_optimistic_row_in_place() {
        let temp1 = MessageId::next_optimistic();
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(100), "earlier", "u1", "U", 100),
            Message::new(temp1, "hello world", "u9", "Me", 200),
        ]);
        assert_eq!(list.temp_match_position("u9", "hello world"), Some(1));
        assert_eq!(list.temp_match_position("u9", "other"), None);
        let idx = list.temp_match_position("u9", "hello world").unwrap();
        list.replace_resort(
            idx,
            Message::new(MessageId(250), "hello world", "u9", "Me", 200),
        );
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(100), MessageId(250)]);
        assert!(list.temp_ids.is_empty());
        assert_list_consistent(&list);
    }

    #[test]
    fn server_echo_merge_preserves_message_grouping() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(100), "first", "u9", "Me", 200),
            Message::new(MessageId(101), "second", "u9", "Me", 201),
        ]);
        let echo = Message::new(MessageId(101), "second", "0", "", 201);
        assert!(list.merge_existing(MessageId(101), echo));
        assert!(
            list.as_slice()[1].combined_with_prev,
            "echo merge must recompute grouping, else the head re-appears until the next send"
        );
        assert_list_consistent(&list);
    }

    #[test]
    fn append_update_remove_keep_index_and_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(10), "a", "u1", "U", 100),
            Message::new(MessageId(20), "b", "u1", "U", 110),
        ]);
        list.push_grouped(Message::new(MessageId(30), "c", "u1", "U", 120));
        assert_eq!(list.position(MessageId(30)), Some(2));
        list.get_mut_by_id(MessageId(20)).unwrap().content = "edited".into();
        assert_eq!(list.as_slice()[1].content, "edited");
        assert!(list.remove_id(MessageId(10)).is_some());
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(ids, [MessageId(20), MessageId(30)]);
        assert_eq!(list.position(MessageId(10)), None);
        assert_list_consistent(&list);
    }

    #[test]
    fn prepend_older_and_append_newer_preserve_order_and_index() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(50), "e", "u1", "U", 150),
            Message::new(MessageId(60), "f", "u1", "U", 160),
        ]);
        let dropped = list.prepend_older(vec![
            Message::new(MessageId(30), "c", "u1", "U", 130),
            Message::new(MessageId(40), "d", "u1", "U", 140),
        ]);
        assert_eq!(dropped, 0);
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [MessageId(30), MessageId(40), MessageId(50), MessageId(60)]
        );
        assert_list_consistent(&list);
        list.append_newer(vec![Message::new(MessageId(70), "g", "u1", "U", 170)]);
        let ids: Vec<MessageId> = list.as_slice().iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            [
                MessageId(30),
                MessageId(40),
                MessageId(50),
                MessageId(60),
                MessageId(70)
            ]
        );
        assert_eq!(list.position(MessageId(70)), Some(4));
        assert_list_consistent(&list);
    }

    #[test]
    fn window_replace_rebuilds_index() {
        let mut list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U", 100)]);
        list.replace(vec![
            Message::new(MessageId(8), "h", "u1", "U", 180),
            Message::new(MessageId(9), "i", "u1", "U", 190),
        ]);
        assert_eq!(list.position(MessageId(1)), None);
        assert_eq!(list.position(MessageId(8)), Some(0));
        assert_eq!(list.position(MessageId(9)), Some(1));
        assert_list_consistent(&list);
    }

    #[test]
    fn append_at_cap_evicts_front_and_reindexes() {
        let mut list = MessageList::from_messages(
            (0..MAX_MESSAGES_PER_CHANNEL)
                .map(|i| Message::new(MessageId(i as i64), "m", "u", "U", i as i64))
                .collect(),
        );
        list.push_grouped(Message::new(
            MessageId(MAX_MESSAGES_PER_CHANNEL as i64),
            "newest",
            "u",
            "U",
            MAX_MESSAGES_PER_CHANNEL as i64,
        ));
        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.position(MessageId(0)), None);
        assert_eq!(list.as_slice()[0].id, MessageId(1));
        assert_eq!(
            list.as_slice().last().unwrap().id,
            MessageId(MAX_MESSAGES_PER_CHANNEL as i64)
        );
        assert_list_consistent(&list);
    }

    #[test]
    fn incremental_in_order_append_at_cap_matches_full_reindex() {
        let temp_old = MessageId::next_optimistic();
        let mut items = vec![Message::new(temp_old, "x", "u", "U", 0)];
        items.extend(
            (1..MAX_MESSAGES_PER_CHANNEL)
                .map(|i| Message::new(MessageId(i as i64), "m", "u", "U", i as i64)),
        );
        let mut list = MessageList::from_messages(items);
        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.temp_ids, vec![temp_old]);

        let temp_new = MessageId::next_optimistic();
        list.push_grouped(Message::new(temp_new, "y", "u", "U", 999));

        let incremental_index = list.index.clone();
        let incremental_temp_ids = list.temp_ids.clone();
        list.reindex();
        assert_eq!(list.index, incremental_index);
        assert_eq!(list.temp_ids, incremental_temp_ids);

        assert_eq!(list.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(list.position(temp_old), None);
        assert_eq!(list.temp_ids, vec![temp_new]);
        assert_eq!(list.as_slice()[0].id, MessageId(1));
        assert_eq!(list.as_slice().last().unwrap().id, temp_new);
        assert_list_consistent(&list);
    }

    #[test]
    fn has_more_bottom_false_when_tail_in_buffer() {
        let list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U", 100),
            Message::new(MessageId(99), "z", "u1", "U", 200),
        ]);
        assert!(!has_more_bottom_for(Some(MessageId(99)), &list));
    }

    #[test]
    fn has_more_bottom_true_when_tail_not_in_buffer() {
        let list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "a", "u1", "U", 100),
            Message::new(MessageId(50), "m", "u1", "U", 150),
        ]);
        assert!(has_more_bottom_for(Some(MessageId(99)), &list));
    }

    #[test]
    fn has_more_bottom_false_without_tail_or_empty_buffer() {
        let list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U", 100)]);
        assert!(!has_more_bottom_for(None, &list));
        assert!(!has_more_bottom_for(
            Some(MessageId(1)),
            &MessageList::default()
        ));
    }

    #[test]
    fn storage_channel_id_uses_topic_bucket() {
        let mut m = mezon_proto::api::ChannelMessage {
            channel_id: 10,
            topic_id: 99,
            ..Default::default()
        };
        assert_eq!(storage_channel_id(&m), ChannelId(99));
        assert_eq!(parent_channel_id(&m), ChannelId(10));
        m.topic_id = 0;
        assert_eq!(storage_channel_id(&m), ChannelId(10));
        assert_eq!(parent_channel_id(&m), ChannelId(10));
    }

    #[test]
    fn tail_keyed_by_storage_bucket_avoids_parent_poison() {
        let parent_buffer = MessageList::from_messages(vec![
            Message::new(MessageId(100), "a", "u1", "U", 1),
            Message::new(MessageId(200), "b", "u1", "U", 2),
        ]);

        let topic_msg = mezon_proto::api::ChannelMessage {
            channel_id: 10,
            topic_id: 99,
            message_id: 4242,
            ..Default::default()
        };
        let topic_bucket = storage_channel_id(&topic_msg);
        let parent_bucket = parent_channel_id(&topic_msg);
        assert_ne!(topic_bucket, parent_bucket);

        let topic_tail = MessageId(topic_msg.message_id);
        assert!(has_more_bottom_for(Some(topic_tail), &parent_buffer));

        let parent_tail = parent_buffer.last().map(|m| m.id);
        assert!(!has_more_bottom_for(parent_tail, &parent_buffer));
    }

    #[test]
    fn temp_match_position_skips_failed_temp() {
        let mut failed = Message::new(MessageId::next_optimistic(), "hello", "42", "Me", 1);
        failed.send_failed = true;
        let pending = Message::new(MessageId::next_optimistic(), "hello", "42", "Me", 2);
        let failed_id = failed.id;
        let pending_id = pending.id;

        let list = MessageList::from_messages(vec![failed, pending]);
        let idx = list
            .temp_match_position("42", "hello")
            .expect("a non-failed temp should match");
        assert_eq!(list.items[idx].id, pending_id);
        assert_ne!(list.items[idx].id, failed_id);
    }

    #[test]
    fn patch_reply_previews_after_delete_marks_reference() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "reply", "u1", "U", 100).with_references(vec![
                MessageReference {
                    message_ref_id: MessageId(42),
                    sender_id: UserId(1),
                    sender_name: "x".into(),
                    sender_avatar: String::new(),
                    content: "orig".into(),
                    content_preview: "orig".into(),
                    has_attachment: false,
                    has_embed: false,
                    is_poll: false,
                },
            ]),
        ]);
        patch_reply_previews_after_delete(&mut list, MessageId(42));
        assert_eq!(
            list.as_slice()[0].references[0].content,
            DELETED_REPLY_PREVIEW
        );
        assert!(list.as_slice()[0].references[0].message_ref_id.is_zero());
    }

    #[test]
    fn should_write_last_seen_matches_react_rules() {
        let seen = MessageId(10_i64 << 22);
        let newer = MessageId(12_i64 << 22);
        let tail = MessageId(15_i64 << 22);
        assert!(should_write_last_seen(Some(seen), Some(tail), newer));
        assert!(should_write_last_seen(Some(seen), Some(tail), tail));
        assert!(!should_write_last_seen(Some(newer), Some(tail), seen));
    }
}
