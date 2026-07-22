use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, EventEmitter, Global, Rgba, SharedString,
    Subscription, Task,
};
use mezon_audio::{AudioPlayer, decode_audio};
use mezon_client::transport::{
    ApiActionRow, ApiComponentPayload, ApiEmbed, ApiEmbedInputWrapper, ApiMessage,
    ApiMessageComponent, ApiMessageContent, ApiMessageInput, ApiSelectComponent, LOCATION_CODE,
    MESSAGE_BUZZ_CODE, OutgoingEmoji as TransportEmoji, OutgoingHashtag as TransportHashtag,
    OutgoingMention as TransportMention, OutgoingMessageFlags, OutgoingOgp, OutgoingReply,
    SHARE_CONTACT_CODE, build_send_content, build_share_contact_content_json, detect_markdown,
    emoji_content_tokens, hashtag_content_tokens, markdown_content_tokens, mention_content_tokens,
};
use mezon_client::{
    AppApi, AttachmentUploadOutcome, ConnectionStatus, MezonTransport, RealtimeEvent, UploadFile,
    UploadThumbnail, UrlAttachment,
};

use crate::AppConfig;
use crate::KeyedCache;
use crate::account::AccountStore;
use crate::album_layout::{AlbumLayout, calculate_album_layout};
use crate::badge::BadgeService;
use crate::channel::{ChannelEvent, ChannelList, ChannelType};
use crate::clan_members::ClanMembersStore;
use crate::direct::{DirectKind, DirectMessageStore};
use crate::message::{
    CallLog, CallLogType, Embed, EmbedAuthor, EmbedField, EmbedFooter, EmbedImage, EmbedInput,
    EmbedTextInput, InvitePreview, MentionTarget, Message, MessageAttachment, MessageButton,
    MessageCode, MessageComponent, MessageComponentRow, MessageReference, MessageSelect,
    MessageSelectOption, MessageSpan, OgpPreview, PollAnswerView, PollData, PollDetail,
    PollLabelSegment, PollVoter, ViewerMedia, aggregate_reactions, apply_reaction_event,
    message_combined_with_prev, message_sort_key, parse_spans, reaction_key,
    recompute_message_grouping, rollback_reaction, sort_messages, spans_only_emoji,
    viewer_highlight_direct,
};
use crate::message_time::{
    format_local_time_hhmm, local_datetime, local_day_key, unix_now_seconds,
};
use crate::presign;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::topics::TopicsStore;

const MESSAGE_PAGE_LIMIT: u32 = 50;
const MAX_COUNTED_TOPIC_REPLIES: usize = 2048;
const DIRECTION_BEFORE: i32 = 3;
const DIRECTION_AFTER: i32 = 1;
/// `Direction_Mode.AROUND_TIMESTAMP` — fetch a window centered on a message
/// (used by jump-to-message when the target is not loaded).
const DIRECTION_AROUND: i32 = 2;
const CHANNEL_TYPE_CHANNEL: i32 = 1;
const CHANNEL_TYPE_THREAD: i32 = 7;
const STICKER_FILETYPE: &str = "sticker";
const AUDIO_FILETYPE: &str = "audio/mpeg";
const MAX_MESSAGES_PER_CHANNEL: usize = 200;
const MAX_CACHED_CHANNELS: usize = 30;
const LAST_SEEN_DEBOUNCE: Duration = Duration::from_millis(1000);
const TYPING_THROTTLE: Duration = Duration::from_millis(1000);
const GIVE_COFFEE_EMOJI_ID: &str = "7280417126303261185";
const GIVE_COFFEE_EMOJI: &str = ":coffee:";

#[derive(Clone)]
struct PendingSendPayload {
    content: String,
    content_tokens: OutgoingContent,
    attachments: Vec<OutgoingAttachment>,
    ogp: Option<OutgoingOgp>,
    reply: Option<ReplyDraft>,
    anonymous: bool,
    message_code: i32,
}

#[derive(Clone, Debug)]
struct PendingLastSeen {
    clan_id: ClanId,
    channel_id: ChannelId,
    message_id: MessageId,
    create_time: i64,
    mode: i32,
    badge_count: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TopicAppend {
    pub appended: bool,
    pub should_count_reply: bool,
    pub create_time: i64,
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
    /// A discussion topic's message bucket changed (loaded / new reply). Drives
    /// the topic side panel to re-read `messages_in_channel(topic_id)`.
    TopicUpdated {
        topic_id: i64,
    },
    /// One forward destination finished (cf. React `sendingProgress`).
    ForwardProgress {
        current: usize,
        total: usize,
    },
    /// Every forward destination has been attempted. `failed` names only the
    /// destinations that did not go through — a partial failure must not be
    /// reported as if the whole forward failed.
    ForwardFinished {
        sent: usize,
        failed: Vec<SharedString>,
    },
    ShareContactFinished {
        sent: usize,
        failed: Vec<SharedString>,
    },
    AnonymousModeChanged,
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
    pub(crate) fn into_transport(self) -> TransportMention {
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
    pub(crate) fn into_transport(self) -> TransportHashtag {
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
    pub(crate) fn into_transport(self) -> TransportEmoji {
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
    pub duration: i32,
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

const POLL_RESULT_ANIMATION_WINDOW: Duration = Duration::from_millis(1200);
const STREAM_MODE_CHANNEL: i32 = 2;
const STREAM_MODE_THREAD: i32 = 6;
static BUZZ_SOUND: &[u8] = include_bytes!("../assets/audio/buzz.mp3");

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
    /// Topic bucket currently shown in the discussion side panel, if any. Lets
    /// realtime replies to a non-active topic bucket still notify the panel.
    active_topic_id: Option<ChannelId>,
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
    counted_topic_replies: HashSet<MessageId>,
    counted_topic_reply_order: std::collections::VecDeque<MessageId>,
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
    embed_form: HashMap<MessageId, HashMap<SharedString, SharedString>>,
    forward_task: Option<Task<()>>,
    forward_in_flight: bool,
    pending_send_payloads: HashMap<MessageId, PendingSendPayload>,
    anonymous_clans: HashSet<ClanId>,
    topic_anonymous_mode: bool,
    last_anonymous_mode: bool,
    last_typing_sent: Option<(ChannelId, Instant)>,
    buzz_player: Option<AudioPlayer>,
    buzz_sound_loading: bool,
}

/// Longest additional note that may ride along with a forward
/// (React `MAX_FORWARD_MESSAGE_LENGTH`).
pub const MAX_FORWARD_MESSAGE_LENGTH: usize = 2000;

/// A forward destination. A friend has no channel yet — the DM is created on
/// send (React `createDirectMessageWithUser`).
#[derive(Debug, Clone)]
pub enum ForwardTarget {
    Channel {
        clan_id: ClanId,
        channel_id: ChannelId,
        channel_type: i32,
        mode: i32,
        is_public: bool,
        label: SharedString,
    },
    Friend {
        user_id: UserId,
        label: String,
        avatar: String,
        username: String,
    },
}

impl ForwardTarget {
    pub fn label(&self) -> SharedString {
        match self {
            Self::Channel { label, .. } => label.clone(),
            Self::Friend { label, .. } => label.clone().into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedTarget {
    clan_id: i64,
    channel_id: i64,
    channel_type: i32,
    mode: i32,
    is_public: bool,
}

/// One source message, snapshotted off the store before the send task runs.
#[derive(Debug, Clone)]
struct ForwardSource {
    content_raw: String,
    text: String,
    attachments: Vec<mezon_client::transport::ApiAttachment>,
    mentions: Vec<TransportMention>,
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

/// Mentions carried by the source message. The server delivers them in the binary
/// proto field, never in the content JSON, so they are read off the domain message
/// rather than re-parsed out of `content_raw`. Only sent when the destination is the
/// `message.channel_id === channel_id ? message.mentions : []`.
fn forward_mentions(targets: &[MentionTarget]) -> Vec<TransportMention> {
    targets
        .iter()
        .filter_map(|target| {
            let user_id = target.user_id.clone().unwrap_or_default();
            let role_id = target.role_id.clone().unwrap_or_default();
            if user_id.is_empty() && role_id.is_empty() {
                return None;
            }
            Some(TransportMention {
                user_id,
                role_id,
                username: target.username.clone(),
                s: target.s,
                e: target.e,
            })
        })
        .collect()
}

/// Fallback for content payloads that carry their own mention tokens (a message
/// composed on web can), used only when the proto field delivered none.
fn content_json_mentions(content_raw: &str) -> Vec<TransportMention> {
    let Ok(content) = serde_json::from_str::<ApiMessageContent>(content_raw) else {
        return Vec::new();
    };
    content
        .mentions
        .iter()
        .filter_map(|token| {
            let user_id = token.user_id.clone().unwrap_or_default();
            let role_id = token.role_id.clone().unwrap_or_default();
            if user_id.is_empty() && role_id.is_empty() {
                return None;
            }
            Some(TransportMention {
                user_id,
                role_id,
                username: token.username.clone().unwrap_or_default(),
                s: i32::try_from(token.s?).ok()?,
                e: i32::try_from(token.e?).ok()?,
            })
        })
        .collect()
}

fn forward_source(msg: &Message) -> ForwardSource {
    let content_raw = msg.raw_content.as_deref().unwrap_or_default().to_string();
    ForwardSource {
        mentions: {
            let proto = forward_mentions(&msg.mention_targets);
            if proto.is_empty() {
                content_json_mentions(&content_raw)
            } else {
                proto
            }
        },
        content_raw,
        text: msg.content.clone(),
        attachments: msg.attachments.iter().map(attachment_to_api).collect(),
    }
}

/// Turn a picked destination into a real channel. A friend row has no DM yet,
/// so one is created first (React `createDirectMessageWithUser`).
async fn resolve_forward_target(
    target: &ForwardTarget,
    cx: &mut AsyncApp,
) -> anyhow::Result<ResolvedTarget> {
    match target {
        ForwardTarget::Channel {
            clan_id,
            channel_id,
            channel_type,
            mode,
            is_public,
            ..
        } => Ok(ResolvedTarget {
            clan_id: clan_id.get(),
            channel_id: channel_id.get(),
            channel_type: *channel_type,
            mode: *mode,
            is_public: *is_public,
        }),
        ForwardTarget::Friend {
            user_id,
            label,
            avatar,
            username,
        } => {
            let create = cx.update(|cx| {
                DirectMessageStore::global(cx).update(cx, |store, cx| {
                    store.create_dm_with_user(
                        *user_id,
                        label.clone(),
                        avatar.clone(),
                        username.clone(),
                        cx,
                    )
                })
            });
            let (channel_id, channel_type) = create.await?;
            Ok(ResolvedTarget {
                clan_id: 0,
                channel_id: channel_id.get(),
                channel_type,
                mode: DirectKind::Dm.stream_mode(),
                is_public: false,
            })
        }
    }
}

async fn send_forward(
    api: &AppApi,
    dest: ResolvedTarget,
    sources: &[ForwardSource],
    source_channel_id: ChannelId,
    note: Option<&str>,
) -> anyhow::Result<()> {
    api.join_chat(
        dest.clan_id,
        dest.channel_id,
        dest.channel_type,
        dest.is_public,
    )
    .await?;
    let same_channel = dest.channel_id == source_channel_id.get();
    let mut failures = 0usize;
    for source in sources {
        let mentions = if same_channel {
            source.mentions.clone()
        } else {
            Vec::new()
        };
        if let Err(e) = api
            .forward_channel_message(
                dest.clan_id,
                dest.channel_id,
                &source.content_raw,
                &source.text,
                dest.is_public,
                dest.mode,
                source.attachments.clone(),
                mentions,
            )
            .await
        {
            tracing::error!(
                "forwarding one message to channel {} failed: {e}",
                dest.channel_id
            );
            failures += 1;
        }
    }
    let all_failed = failures == sources.len();
    if let Some(note) = note
        && !all_failed
        && let Err(e) = api
            .send_channel_message(
                dest.clan_id,
                dest.channel_id,
                note,
                dest.is_public,
                dest.mode,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
            )
            .await
    {
        tracing::error!("forward note to channel {} failed: {e}", dest.channel_id);
        failures += 1;
    }
    if failures > 0 {
        anyhow::bail!(
            "{failures} of {} forwarded items failed",
            sources.len() + usize::from(note.is_some())
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ShareContactSubject {
    pub user_id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar: String,
}

async fn send_share_contact_to_target(
    api: &AppApi,
    dest: ResolvedTarget,
    content_json: &str,
) -> anyhow::Result<()> {
    api.join_chat(
        dest.clan_id,
        dest.channel_id,
        dest.channel_type,
        dest.is_public,
    )
    .await?;
    let flags = OutgoingMessageFlags {
        anonymous_message: false,
        message_code: SHARE_CONTACT_CODE,
    };
    api.send_channel_message_prebuilt(
        dest.clan_id,
        dest.channel_id,
        content_json,
        dest.is_public,
        dest.mode,
        flags,
    )
    .await?;
    Ok(())
}

/// Transient UI state for a single poll card.
#[derive(Debug, Default, Clone)]
pub struct PollUiState {
    pub selected: Vec<i32>,
    pub show_results: bool,
    pub voting: bool,
    pub voted_at: Option<std::time::Instant>,
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

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
        self.cache.clear();
        self.last_message_by_channel.clear();
        self.last_read_message_by_channel.clear();
        self.viewing_older_by_channel.clear();
        self.active_channel_id = None;
        self.active_clan_id = None;
        self.active_topic_id = None;
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
        self.embed_form.clear();
        self.forward_task = None;
        self.forward_in_flight = false;
        self.pending_send_payloads.clear();
        self.anonymous_clans.clear();
        self.topic_anonymous_mode = false;
        self.sync_anonymous_mode(cx);
        self.last_typing_sent = None;
        self.buzz_player = None;
        self.buzz_sound_loading = false;
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
            active_topic_id: None,
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
            counted_topic_replies: HashSet::new(),
            counted_topic_reply_order: std::collections::VecDeque::new(),
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
            embed_form: HashMap::new(),
            forward_task: None,
            forward_in_flight: false,
            pending_send_payloads: HashMap::new(),
            anonymous_clans: HashSet::new(),
            topic_anonymous_mode: false,
            last_anonymous_mode: false,
            last_typing_sent: None,
            buzz_player: None,
            buzz_sound_loading: false,
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

    pub fn viewport_message_by_id(&self, id: MessageId) -> Option<&Message> {
        self.active_channel_id
            .as_ref()
            .and_then(|channel| self.cache.get(channel))
            .and_then(|channel| channel.messages.get_by_id(id))
    }

    pub fn viewport_position(&self, id: MessageId) -> Option<usize> {
        self.active_channel_id
            .as_ref()
            .and_then(|channel| self.cache.get(channel))
            .and_then(|channel| channel.messages.position(id))
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
        let storage_id = self.reaction_storage_channel(message_id);
        let msg = self
            .cache
            .get(&storage_id)?
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
        if self.is_viewing_older(channel_id) == viewing {
            return;
        }
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

    pub fn active_clan_id(&self) -> Option<ClanId> {
        self.active_clan_id
    }

    /// Stream mode of the active channel (`STREAM_MODE_CHANNEL` / `STREAM_MODE_THREAD`).
    pub fn mode(&self) -> i32 {
        self.mode
    }

    pub fn is_public(&self) -> bool {
        self.is_public
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
        let cfg = AppConfig::try_global(cx).cloned();
        let viewer_id = viewer_user_id(cx);
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
            let msgs = match result {
                Ok(page) => page.messages,
                Err(e) => {
                    tracing::error!("Failed to load more messages for {channel_id}: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.loading_more = false;
                        cx.notify();
                    });
                    return;
                }
            };
            tracing::debug!(
                channel_id = channel_id.get(),
                fetched = msgs.len(),
                "load_more: page received"
            );
            let parsed: Vec<Message> = cx
                .background_executor()
                .spawn(async move {
                    msgs.into_iter()
                        .map(|m| message_from_api(m, cfg.as_ref(), viewer_id))
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let (prepended, dropped_bottom) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let older: Vec<Message> = parsed
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(m.id))
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
        let cfg = AppConfig::try_global(cx).cloned();
        let viewer_id = viewer_user_id(cx);
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
            let msgs = match result {
                Ok(page) => page.messages,
                Err(e) => {
                    tracing::error!("Failed to load newer messages for {channel_id}: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.loading_more = false;
                        cx.notify();
                    });
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
            let parsed: Vec<Message> = cx
                .background_executor()
                .spawn(async move {
                    msgs.into_iter()
                        .map(|m| message_from_api(m, cfg.as_ref(), viewer_id))
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let (added, dropped) = {
                    let Some(channel) = this.cache.get_mut(&channel_id) else {
                        return;
                    };
                    let newer: Vec<Message> = parsed
                        .into_iter()
                        .filter(|m| !channel.messages.contains_id(m.id))
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
        let cfg = AppConfig::try_global(cx).cloned();
        let viewer_id = viewer_user_id(cx);
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
            let msgs = match result {
                Ok(page) => page.messages,
                Err(e) => {
                    tracing::error!("jump_to_message AROUND fetch failed for {channel_id}: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.loading_more = false;
                        cx.notify();
                    });
                    return;
                }
            };
            let parsed: Vec<Message> = cx
                .background_executor()
                .spawn(async move {
                    msgs.into_iter()
                        .map(|m| message_from_api(m, cfg.as_ref(), viewer_id))
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                let mut window: Vec<Message> = parsed;
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
        let Some(draft) = self.reply_draft_for(message_id) else {
            return;
        };
        self.set_reply(draft, cx);
    }

    pub fn reply_draft_for(&self, message_id: MessageId) -> Option<ReplyDraft> {
        let storage_id = self.reaction_storage_channel(message_id);
        self.cache
            .get(&storage_id)
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

    pub fn is_anonymous_mode(&self) -> bool {
        if self.active_topic_id.is_some() && self.topic_anonymous_mode {
            return true;
        }
        self.active_clan_id
            .is_some_and(|id| self.anonymous_clans.contains(&id))
    }

    pub(crate) fn sync_anonymous_mode(&mut self, cx: &mut Context<Self>) {
        let next = self.is_anonymous_mode();
        if self.last_anonymous_mode == next {
            return;
        }
        self.last_anonymous_mode = next;
        cx.emit(MessagesEvent::AnonymousModeChanged);
    }

    pub fn toggle_anonymous_mode(&mut self, cx: &mut Context<Self>) {
        if self.active_topic_id.is_some() {
            self.topic_anonymous_mode = !self.topic_anonymous_mode;
            self.sync_anonymous_mode(cx);
            cx.notify();
            return;
        }
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        if self.anonymous_clans.contains(&clan_id) {
            self.anonymous_clans.remove(&clan_id);
        } else {
            self.anonymous_clans.insert(clan_id);
        }
        self.sync_anonymous_mode(cx);
        cx.notify();
    }

    pub fn notify_typing(&mut self, cx: &mut Context<Self>) {
        if self.is_anonymous_mode() {
            return;
        }
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let now = Instant::now();
        if self.last_typing_sent.is_some_and(|(last_channel, last)| {
            last_channel == channel_id && now.duration_since(last) < TYPING_THROTTLE
        }) {
            return;
        }
        let display_name = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .map(|acct| {
                if !acct.display_name.is_empty() {
                    acct.display_name.clone()
                } else {
                    acct.username.clone()
                }
            })
            .unwrap_or_default();
        if display_name.is_empty() {
            return;
        }
        self.last_typing_sent = Some((channel_id, now));
        let mode = self.mode;
        let is_public = self.is_public;
        let topic_id = self.active_topic_id.map(|t| t.get()).unwrap_or(0);
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .write_message_typing(
                    clan_id.get(),
                    channel_id.get(),
                    mode,
                    is_public,
                    &display_name,
                    topic_id,
                )
                .await
            {
                tracing::debug!("write_message_typing failed: {e}");
            }
        })
        .detach();
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
        if self.active_channel_id.is_none() {
            return;
        };
        let storage_id = self.reaction_storage_channel(message_id);
        let mode = self.mode;
        let is_public = self.is_public;
        let edit_meta = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id))
            .map(|msg| (!msg.attachments.is_empty(), msg.create_time.max(0) as u32));
        let (spans, transport_mentions, transport_hashtags, transport_emojis) =
            edit_content_spans(&content, content_tokens);
        let Some(channel) = self.cache.get_mut(&storage_id) else {
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
        if self.active_topic_id == Some(storage_id) {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
        } else {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
        }
        cx.notify();

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let message_num = message_id.get();
        let (api_channel_id, api_topic_id, is_update_msg_topic) =
            if self.active_topic_id == Some(storage_id) {
                (storage_id.get(), storage_id.get(), true)
            } else {
                (storage_id.get(), 0, false)
            };
        let create_time_seconds = edit_meta
            .filter(|(has_attachments, _)| *has_attachments)
            .map(|(_, ts)| ts)
            .unwrap_or(0);
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_channel_message(
                    clan_id,
                    api_channel_id,
                    message_num,
                    &content,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                    mode,
                    is_public,
                    api_topic_id,
                    is_update_msg_topic,
                    false,
                    create_time_seconds,
                )
                .await
            {
                tracing::error!("update_channel_message failed: {e}");
            }
        })
        .detach();
    }

    /// Remove the OGP link preview from a message (author only), mirroring the
    /// React `DeleteOgpButton`: drop it locally and re-send the message content
    /// without the `lk_ogp` token so it is gone for everyone.
    pub fn remove_message_ogp(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if self.active_channel_id.is_none() {
            return;
        }
        let storage_id = self.reaction_storage_channel(message_id);
        let mode = self.mode;
        let is_public = self.is_public;
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        if msg.ogp.is_none() {
            return;
        }
        let create_time_seconds = if msg.attachments.is_empty() {
            0
        } else {
            msg.create_time.max(0) as u32
        };
        msg.ogp = None;
        let content = msg.content.clone();
        let outgoing = msg
            .raw_content
            .as_deref()
            .and_then(outgoing_content_from_raw)
            .unwrap_or_default();
        if self.active_topic_id == Some(storage_id) {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
        } else {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
        }
        cx.notify();

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let message_num = message_id.get();
        let (api_channel_id, api_topic_id, is_update_msg_topic) =
            if self.active_topic_id == Some(storage_id) {
                (storage_id.get(), storage_id.get(), true)
            } else {
                (storage_id.get(), 0, false)
            };
        let OutgoingContent {
            mentions,
            hashtags,
            emojis,
        } = outgoing;
        let transport_mentions = mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let transport_hashtags = hashtags
            .into_iter()
            .map(OutgoingHashtag::into_transport)
            .collect();
        let transport_emojis = emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_channel_message(
                    clan_id,
                    api_channel_id,
                    message_num,
                    &content,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                    mode,
                    is_public,
                    api_topic_id,
                    is_update_msg_topic,
                    false,
                    create_time_seconds,
                )
                .await
            {
                tracing::error!("remove ogp update failed: {e}");
            }
        })
        .detach();
    }

    /// Remove a message locally, then send the delete to the server.
    /// No rollback on network failure — a channel refresh reconciles true failures.
    pub fn delete_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(parent_channel_id) = self.active_channel_id else {
            return;
        };
        let storage_id = self.reaction_storage_channel(message_id);
        let deleted = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id));
        let has_attachment = deleted.is_some_and(|msg| !msg.attachments.is_empty());
        let has_mentions = deleted.is_some_and(|msg| !msg.mention_targets.is_empty());
        let has_references = deleted.is_some_and(|msg| !msg.references.is_empty());
        if self.editing == Some(message_id) {
            self.editing = None;
        }
        self.apply_message_remove(storage_id, message_id, cx);

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let mode = self.mode;
        let is_public = self.is_public;
        let message_num = message_id.get();
        let (api_channel_id, api_topic_id) = if self.active_topic_id == Some(storage_id) {
            (parent_channel_id.get(), storage_id.get())
        } else {
            (storage_id.get(), 0)
        };
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .delete_channel_message(
                    clan_id,
                    api_channel_id,
                    message_num,
                    mode,
                    is_public,
                    has_attachment,
                    api_topic_id,
                    has_mentions,
                    has_references,
                )
                .await
            {
                tracing::error!("delete_channel_message failed: {e}");
            }
        })
        .detach();
    }

    pub fn remove_failed_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if self.active_channel_id.is_none() {
            return;
        }
        let storage_id = self.reaction_storage_channel(message_id);
        let is_failed = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id))
            .is_some_and(|msg| msg.send_failed);
        if !is_failed {
            return;
        }
        self.pending_send_payloads.remove(&message_id);
        if self.editing == Some(message_id) {
            self.editing = None;
        }
        self.apply_message_remove(storage_id, message_id, cx);
    }

    pub fn resend_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        if self.active_channel_id.is_none() {
            return;
        }
        let storage_id = self.reaction_storage_channel(message_id);
        let payload = self.pending_send_payloads.remove(&message_id);
        let snapshot = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id))
            .filter(|msg| msg.send_failed)
            .cloned();
        let Some(failed) = snapshot else {
            return;
        };
        self.apply_message_remove(storage_id, message_id, cx);
        if let Some(payload) = payload {
            let (uid, uname) = (failed.sender_id.clone(), failed.sender_name.to_string());
            self.send_message_with_payload(
                payload.content,
                uid,
                uname,
                payload.content_tokens,
                payload.attachments,
                payload.ogp,
                payload.reply,
                payload.anonymous,
                payload.message_code,
                cx,
            );
            return;
        }
        let content = failed.content.clone();
        let attachments: Vec<OutgoingAttachment> = failed
            .attachments
            .iter()
            .filter_map(|att| {
                let path = att.local_source.clone()?;
                Some(OutgoingAttachment {
                    path,
                    filename: att.filename.clone(),
                    filetype: att.filetype.clone(),
                    width: i32::try_from(att.width).unwrap_or(0),
                    height: i32::try_from(att.height).unwrap_or(0),
                    duration: att.duration,
                    poster_jpeg: None,
                })
            })
            .collect();
        self.send_message_with_payload(
            content,
            failed.sender_id.clone(),
            failed.sender_name.to_string(),
            OutgoingContent::default(),
            attachments,
            None,
            None,
            false,
            0,
            cx,
        );
    }

    pub fn send_location_message(&mut self, latitude: f64, longitude: f64, cx: &mut Context<Self>) {
        if self.is_anonymous_mode() {
            return;
        }
        let Some(user_id) = viewer_user_id(cx) else {
            return;
        };
        let sender_id = user_id.0.to_string();
        let sender_name = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .map(|acct| {
                if !acct.display_name.is_empty() {
                    acct.display_name.clone()
                } else {
                    acct.username.clone()
                }
            })
            .unwrap_or_else(|| sender_id.clone());
        let link = mezon_client::transport::build_location_maps_link(latitude, longitude);
        self.send_message_with_payload(
            link,
            sender_id,
            sender_name,
            OutgoingContent::default(),
            Vec::new(),
            None,
            None,
            false,
            LOCATION_CODE,
            cx,
        );
    }

    pub fn send_buzz_message(&mut self, content: String, cx: &mut Context<Self>) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return;
        }
        let Some(user_id) = viewer_user_id(cx) else {
            return;
        };
        let sender_id = user_id.0.to_string();
        let sender_name = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .map(|acct| {
                if !acct.display_name.is_empty() {
                    acct.display_name.clone()
                } else {
                    acct.username.clone()
                }
            })
            .unwrap_or_else(|| sender_id.clone());
        self.send_message_with_payload(
            trimmed.to_string(),
            sender_id,
            sender_name,
            OutgoingContent::default(),
            Vec::new(),
            None,
            None,
            false,
            MESSAGE_BUZZ_CODE,
            cx,
        );
        self.play_buzz_sound(cx);
    }

    fn play_buzz_sound(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.buzz_player {
            player.play();
            return;
        }
        if self.buzz_sound_loading {
            return;
        }
        self.buzz_sound_loading = true;
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { decode_audio(BUZZ_SOUND.to_vec()) })
                .await;
            this.update(cx, |this, _| {
                this.buzz_sound_loading = false;
                let Ok(pcm) = decoded else {
                    return;
                };
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                player.play();
                this.buzz_player = Some(player);
            })
            .ok();
        })
        .detach();
    }

    pub fn give_coffee_reaction(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        self.add_reaction(
            message_id,
            GIVE_COFFEE_EMOJI_ID.to_string(),
            GIVE_COFFEE_EMOJI.to_string(),
            cx,
        );
    }

    pub fn execute_quick_menu(
        &mut self,
        menu_name: &str,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let Some(clan_id) = self.active_clan_id else {
            return;
        };
        let storage_id = self.reaction_storage_channel(message_id);
        let Some(msg) = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id))
            .cloned()
        else {
            return;
        };
        let content_json = msg
            .raw_content
            .as_deref()
            .filter(|raw| !raw.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| build_send_content(&msg.content, &[], &[], &[]).json);
        let mentions: Vec<mezon_proto::api::MessageMention> = msg
            .mention_targets
            .iter()
            .filter_map(|target| {
                let user_id = target.user_id.clone()?.parse().ok()?;
                Some(mezon_proto::api::MessageMention {
                    user_id,
                    role_id: target
                        .role_id
                        .as_ref()
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0),
                    s: target.s,
                    e: target.e,
                    username: target.username.to_string(),
                    ..Default::default()
                })
            })
            .collect();
        let attachments: Vec<mezon_proto::api::MessageAttachment> = msg
            .attachments
            .iter()
            .map(|att| mezon_proto::api::MessageAttachment {
                url: att.url.clone(),
                filename: att.filename.clone(),
                filetype: att.filetype.clone(),
                width: i32::try_from(att.width).unwrap_or(0),
                height: i32::try_from(att.height).unwrap_or(0),
                thumbnail: att.thumbnail.clone(),
                duration: att.duration,
                size: i32::try_from(att.size).unwrap_or(0),
            })
            .collect();
        let references: Vec<mezon_proto::api::MessageRef> = msg
            .references
            .iter()
            .map(|reference| mezon_proto::api::MessageRef {
                message_ref_id: reference.message_ref_id.get(),
                content: reference.content.clone(),
                has_attachment: reference.has_attachment,
                message_sender_id: reference.sender_id.get(),
                message_sender_username: reference.sender_name.to_string(),
                message_sender_avatar: reference.sender_avatar.to_string(),
                ..Default::default()
            })
            .collect();
        let sender_id = msg.sender_id.parse().unwrap_or(0);
        let avatar = AccountStore::global(cx)
            .read(cx)
            .clan_profile
            .as_ref()
            .filter(|profile| profile.clan_id == clan_id)
            .and_then(|profile| profile.avatar_url.clone())
            .or_else(|| {
                AccountStore::global(cx)
                    .read(cx)
                    .account
                    .as_ref()
                    .and_then(|acct| acct.avatar_url.clone())
            })
            .unwrap_or_default();
        let mode = self.mode;
        let is_public = self.is_public;
        let topic_id = self.active_topic_id.map(|t| t.get()).unwrap_or(0);
        let api = self.api.clone();
        let menu_name = menu_name.to_string();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .write_quick_menu_event(
                    &menu_name,
                    clan_id.get(),
                    channel_id.get(),
                    mode,
                    is_public,
                    &content_json,
                    mentions,
                    attachments,
                    references,
                    false,
                    false,
                    &avatar,
                    0,
                    topic_id,
                    message_id.get(),
                    sender_id,
                )
                .await
            {
                tracing::error!("write_quick_menu_event failed: {e}");
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

    pub fn add_to_inbox(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let storage_id = self.reaction_storage_channel(message_id);
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let Some(channel) = self.cache.get(&storage_id) else {
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

    /// Messages held in an arbitrary bucket (used by the discussion topic panel,
    /// whose replies are stored under the topic id — mirroring mezon-react).
    pub fn messages_in_channel(&self, channel_id: ChannelId) -> &[Message] {
        self.cache
            .get(&channel_id)
            .map(|c| c.messages.as_slice())
            .unwrap_or(&[])
    }

    /// Mark the topic bucket the discussion panel is watching so realtime replies
    /// to it (a non-active channel) still notify the panel. Closing (`None`) drops
    /// the previous topic bucket so it does not permanently occupy channel-cache slots.
    pub fn set_active_topic(&mut self, topic_id: Option<i64>, cx: &mut Context<Self>) {
        let next = topic_id.map(ChannelId);
        if self.active_topic_id == next {
            return;
        }
        if let Some(prev) = self.active_topic_id
            && next != Some(prev)
        {
            self.cache.remove(&prev);
            self.last_message_by_channel.remove(&prev);
            self.pending_self_adds
                .retain(|(channel_id, _, _), _| *channel_id != prev);
        }
        self.active_topic_id = next;
        self.sync_anonymous_mode(cx);
        cx.notify();
    }

    /// Load a discussion topic's replies into their own bucket (keyed by topic id).
    /// Mirrors mezon-js `listChannelMessages(clanId, channelId, undefined, dir, limit, topicId)`.
    pub fn fetch_topic_messages(
        &mut self,
        clan_id: i64,
        parent_channel_id: i64,
        topic_id: i64,
        cx: &mut Context<Self>,
    ) {
        let topic_key = ChannelId(topic_id);
        let api = self.api.clone();
        let cfg = AppConfig::try_global(cx).cloned();
        let viewer_id = viewer_user_id(cx);
        cx.spawn(async move |this, cx| {
            let result = api
                .list_topic_messages(clan_id, parent_channel_id, topic_id, 0, MESSAGE_PAGE_LIMIT)
                .await;
            let msgs = match result {
                Ok(page) => page.messages,
                Err(e) => {
                    tracing::error!("fetch_topic_messages failed for topic {topic_id}: {e}");
                    return;
                }
            };
            let parsed = prepare_messages(msgs, cfg.as_ref(), viewer_id);
            let _ = this.update(cx, |this, cx| {
                if this.active_topic_id != Some(topic_key) {
                    return;
                }
                this.set_channel(topic_key, parsed);
                cx.emit(MessagesEvent::TopicUpdated { topic_id });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn append_topic_message(
        &mut self,
        topic_id: i64,
        api_msg: mezon_client::transport::ApiMessage,
        cx: &mut Context<Self>,
    ) -> TopicAppend {
        let topic_key = ChannelId(topic_id);
        let cfg = AppConfig::try_global(cx);
        let viewer_id = viewer_user_id(cx);
        let mut msg = message_from_api(api_msg, cfg, viewer_id);
        enrich_sparse_topic_ack(&mut msg, viewer_id, self.active_clan_id, cx);
        mark_pending_attachments_uploading(&mut msg.attachments);
        let message_id = msg.id;
        let create_time = if msg.create_time > 0 {
            msg.create_time
        } else {
            unix_now_seconds()
        };
        if self.cache.contains(&topic_key) {
            if let Some(channel) = self.cache.get_mut(&topic_key) {
                if channel.messages.contains_id(msg.id) {
                    return TopicAppend::default();
                }
                channel.messages.push_grouped(msg);
            }
        } else {
            self.set_channel(topic_key, vec![msg]);
        }
        let should_count_reply = topic_id != 0
            && mark_topic_reply_counted(
                &mut self.counted_topic_replies,
                &mut self.counted_topic_reply_order,
                message_id,
            );
        cx.emit(MessagesEvent::TopicUpdated { topic_id });
        cx.notify();
        TopicAppend {
            appended: true,
            should_count_reply,
            create_time,
        }
    }

    pub fn mark_message_as_topic(
        &mut self,
        parent_channel_id: ChannelId,
        origin_message_id: MessageId,
        topic_id: i64,
        creator_id: Option<UserId>,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.cache.get_mut(&parent_channel_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(origin_message_id) else {
            return;
        };
        if msg.code == MessageCode::Topic && msg.topic_id == Some(ChannelId(topic_id)) {
            return;
        }
        msg.code = MessageCode::Topic;
        msg.topic_id = Some(ChannelId(topic_id));
        msg.topic_creator_id = creator_id;
        if self.active_channel_id == Some(parent_channel_id) {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(origin_message_id),
            });
        }
        cx.notify();
    }

    pub fn is_forwarding(&self) -> bool {
        self.forward_in_flight
    }

    /// Forward `message_ids` to every destination, one destination at a time so
    /// the copies land in order (React awaits each `forwardToSingleDestination`).
    /// The note rides along after the last message of each destination.
    ///
    /// Returns `false` when nothing was started — the caller must not enter a
    /// "sending" state it would never be released from, since `ForwardProgress`
    /// / `ForwardFinished` are only emitted for a send that actually began.
    pub fn forward(
        &mut self,
        message_ids: Vec<MessageId>,
        targets: Vec<ForwardTarget>,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if message_ids.is_empty() || targets.is_empty() || self.forward_in_flight {
            return false;
        }
        let Some(source_channel_id) = self.active_channel_id else {
            return false;
        };
        let storage_id = self.reaction_storage_channel(message_ids[0]);
        let Some(channel) = self.cache.get(&storage_id) else {
            return false;
        };
        let sources: Vec<ForwardSource> = message_ids
            .iter()
            .filter_map(|id| channel.messages.get_by_id(*id))
            .map(forward_source)
            .collect();
        if sources.is_empty() {
            return false;
        }
        let note = note
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty() && n.chars().count() <= MAX_FORWARD_MESSAGE_LENGTH);

        let api = self.api.clone();
        let total = targets.len();
        self.forward_in_flight = true;
        let task = cx.spawn(async move |this, cx| {
            let mut failed: Vec<SharedString> = Vec::new();
            for (index, target) in targets.iter().enumerate() {
                match resolve_forward_target(target, cx).await {
                    Ok(dest) => {
                        if let Err(e) =
                            send_forward(&api, dest, &sources, source_channel_id, note.as_deref())
                                .await
                        {
                            tracing::error!("forward to channel {} failed: {e}", dest.channel_id);
                            failed.push(target.label());
                        }
                    }
                    Err(e) => {
                        tracing::error!("forward target could not be resolved: {e}");
                        failed.push(target.label());
                    }
                }
                let _ = this.update(cx, |_, cx| {
                    cx.emit(MessagesEvent::ForwardProgress {
                        current: index + 1,
                        total,
                    });
                });
            }
            let _ = this.update(cx, |this, cx| {
                this.forward_in_flight = false;
                let sent = total - failed.len();
                cx.emit(MessagesEvent::ForwardFinished { sent, failed });
            });
        });
        self.forward_task = Some(task);
        true
    }

    pub fn share_contact(
        &mut self,
        contact: ShareContactSubject,
        targets: Vec<ForwardTarget>,
        cx: &mut Context<Self>,
    ) -> bool {
        if targets.is_empty() {
            return false;
        }
        if contact.username.is_empty() {
            return false;
        }
        let content_json = build_share_contact_content_json(
            &contact.user_id.0.to_string(),
            &contact.username,
            &contact.display_name,
            &contact.avatar,
        );
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let mut failed = Vec::new();
            let total = targets.len();
            for target in targets {
                match resolve_forward_target(&target, cx).await {
                    Ok(dest) => {
                        if let Err(e) =
                            send_share_contact_to_target(&api, dest, &content_json).await
                        {
                            tracing::error!("share contact to {} failed: {e}", target.label());
                            failed.push(target.label());
                        }
                    }
                    Err(e) => {
                        tracing::error!("share contact target could not be resolved: {e}");
                        failed.push(target.label());
                    }
                }
            }
            let sent = total.saturating_sub(failed.len());
            let _ = this.update(cx, |_, cx| {
                cx.emit(MessagesEvent::ShareContactFinished { sent, failed });
            });
        })
        .detach();
        true
    }

    #[allow(dead_code)]
    pub fn remove_attachment(
        &mut self,
        message_id: MessageId,
        attachment_index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.active_channel_id.is_none() {
            return;
        }
        let storage_id = self.reaction_storage_channel(message_id);
        let is_topic = self.active_topic_id == Some(storage_id);
        let mode = self.mode;
        let is_public = self.is_public;
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let cfg = AppConfig::try_global(cx).cloned();
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            return;
        };
        let Some(msg) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        if attachment_index >= msg.attachments.len() {
            return;
        }
        let create_time_seconds = msg.create_time.max(0) as u32;
        msg.attachments.remove(attachment_index);
        let (album_layout, viewer_media) = build_media_presentation(&msg.attachments, cfg.as_ref());
        msg.album_layout = album_layout;
        msg.viewer_media = viewer_media;
        let content = msg.content.clone();
        let remaining: Vec<mezon_client::transport::ApiAttachment> =
            msg.attachments.iter().map(attachment_to_api).collect();
        if is_topic {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
        } else {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
        }
        cx.notify();

        let api = self.api.clone();
        let (api_channel_id, api_topic_id) = if is_topic {
            (storage_id.get(), storage_id.get())
        } else {
            (storage_id.get(), 0)
        };
        let message_num = message_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .update_channel_message_with_attachments(
                    clan_id,
                    api_channel_id,
                    message_num,
                    &content,
                    remaining,
                    mode,
                    is_public,
                    api_topic_id,
                    is_topic,
                    create_time_seconds,
                )
                .await
            {
                tracing::error!("remove_attachment update failed: {e}");
            }
        })
        .detach();
    }

    pub fn embed_form_value(&self, message_id: MessageId, input_id: &str) -> Option<&SharedString> {
        self.embed_form
            .get(&message_id)
            .and_then(|by_input| by_input.get(input_id))
    }

    pub fn set_embed_form_value(
        &mut self,
        message_id: MessageId,
        input_id: SharedString,
        value: SharedString,
    ) {
        self.embed_form
            .entry(message_id)
            .or_default()
            .insert(input_id, value);
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
            tracing::warn!("message button '{button_id}' ignored: no active channel");
            return;
        };
        let mut form = serde_json::Map::new();
        if let Some(by_select) = self.select_ui.get(&message_id) {
            for (id, values) in by_select {
                let values: Vec<serde_json::Value> = values
                    .iter()
                    .map(|v| serde_json::Value::String(v.to_string()))
                    .collect();
                form.insert(id.to_string(), serde_json::Value::Array(values));
            }
        }
        if let Some(by_input) = self.embed_form.get(&message_id) {
            for (id, value) in by_input {
                form.insert(id.to_string(), serde_json::Value::String(value.to_string()));
            }
        }
        let extra_data =
            serde_json::to_string(&serde_json::Value::Object(form)).unwrap_or_else(|_| "{}".into());
        let api = self.api.clone();
        let channel_num = channel_id.get();
        let message_num = message_id.get();
        let button_id = button_id.to_string();
        let button_id_log = button_id.clone();
        let extra_len = extra_data.len();
        cx.spawn(async move |_this, _cx| {
            match api
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
                Ok(()) => tracing::info!(
                    "message button '{button_id_log}' submitted (extra_data {extra_len} bytes)"
                ),
                Err(e) => {
                    tracing::error!("message button '{button_id_log}' submit failed: {e}")
                }
            }
        })
        .detach();
    }

    /// Send an ephemeral message (visible only to `receiver_id`). No optimistic
    /// row — the message arrives back through the normal realtime pipeline with
    /// `code = Ephemeral`, so nothing is echoed locally to the sender.
    pub fn send_ephemeral_message(
        &mut self,
        receiver_id: i64,
        content: String,
        content_tokens: OutgoingContent,
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

        let api = self.api.clone();
        let clan_num = clan_id.get();
        let channel_num = channel_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .write_ephemeral_message(
                    receiver_id,
                    clan_num,
                    channel_num,
                    &content,
                    is_public,
                    mode,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                )
                .await
            {
                tracing::error!("send_ephemeral_message failed: {e}");
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
        ogp: Option<OutgoingOgp>,
        cx: &mut Context<Self>,
    ) {
        let anonymous = self.is_anonymous_mode();
        self.send_message_with_payload(
            content,
            sender_id,
            sender_name,
            content_tokens,
            attachments,
            ogp,
            None,
            anonymous,
            0,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_with_payload(
        &mut self,
        content: String,
        sender_id: String,
        sender_name: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        ogp: Option<OutgoingOgp>,
        reply_override: Option<ReplyDraft>,
        anonymous: bool,
        message_code: i32,
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
        let reply = match reply_override {
            Some(draft) => Some(draft),
            None => self.reply_target.take(),
        };
        if reply.is_some() {
            cx.emit(MessagesEvent::ReplyTargetChanged);
        }
        let ogp = if has_attachments { None } else { ogp };
        let send_flags = OutgoingMessageFlags {
            anonymous_message: anonymous,
            message_code,
        };
        let pending_tokens = content_tokens.clone();
        let pending_attachments = attachments.clone();
        let pending_ogp = ogp.clone();

        self.clear_last_read_message(channel_id);
        let Some(channel) = self.cache.get_mut(&channel_id) else {
            return;
        };
        let grouping_sender_id = if anonymous {
            AppConfig::try_global(cx)
                .filter(|c| !c.anonymous_user_id.is_empty())
                .map(|c| c.anonymous_user_id.clone())
                .unwrap_or(sender_id.clone())
        } else {
            sender_id.clone()
        };
        let create_time = optimistic_create_time(&channel.messages, &grouping_sender_id);
        let temp_id = MessageId::next_optimistic();
        self.pending_send_payloads.insert(
            temp_id,
            PendingSendPayload {
                content: content.clone(),
                content_tokens: pending_tokens,
                attachments: pending_attachments,
                ogp: pending_ogp,
                reply: reply.clone(),
                anonymous,
                message_code,
            },
        );

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
        let sent = build_send_content(
            &content,
            &transport_mentions,
            &transport_hashtags,
            &transport_emojis,
        );
        let (display_name, avatar_url, avatar_proxied) =
            outgoing_sender_profile(&sender_id, &sender_name, clan_id, cx);
        let mut optimistic = Message::new(
            temp_id,
            sent.text.clone(),
            sender_id,
            display_name,
            create_time,
        )
        .with_avatar(avatar_url)
        .with_avatar_proxied(avatar_proxied);
        if !sent.mentions.is_empty()
            || !sent.hashtags.is_empty()
            || !sent.emojis.is_empty()
            || !sent.markdowns.is_empty()
            || ogp.is_some()
        {
            let mut mk = markdown_content_tokens(&sent.markdowns);
            if let Some(ogp) = &ogp {
                mk.push(ogp.to_content_token(sent.text.encode_utf16().count()));
            }
            let tokens = ApiMessageContent {
                t: sent.text.clone(),
                mentions: mention_content_tokens(&sent.mentions),
                hg: hashtag_content_tokens(&sent.hashtags),
                ej: emoji_content_tokens(&sent.emojis),
                mk,
                ..Default::default()
            };
            optimistic = optimistic.with_spans(parse_spans(&tokens));
            if ogp.is_some() {
                optimistic =
                    optimistic.with_ogp(build_ogp_preview(&tokens, AppConfig::try_global(cx)));
            }
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
        if let Some(config) = AppConfig::try_global(cx)
            && anonymous
            && !config.anonymous_user_id.is_empty()
        {
            optimistic.sender_id = config.anonymous_user_id.clone();
            optimistic.sender_name = "Anonymous".into();
            optimistic.avatar_url = SharedString::default();
            optimistic.avatar_proxied = SharedString::default();
        }
        if message_code == MESSAGE_BUZZ_CODE {
            optimistic.code = MessageCode::MessageBuzz;
        } else if message_code == LOCATION_CODE {
            optimistic.code = MessageCode::Location;
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
                            duration: att.duration,
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
                    tokio::sync::mpsc::unbounded_channel::<AttachmentUploadOutcome>();
                let drain_this = this.clone();
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    while let Some(outcome) = completions.recv().await {
                        let _ = drain_this.update(cx, |this, cx| {
                            this.mark_channel_attachment_outcome(
                                channel_id,
                                MessageId(real_message_id),
                                outcome,
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
                    0,
                    false,
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
                        ogp,
                    )
                    .await
                } else {
                    api.send_channel_message_with_flags(
                        clan_id.get(),
                        channel_id.get(),
                        &content,
                        is_public,
                        mode,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                        ogp,
                        send_flags,
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
        self.send_url_attachment(
            url,
            filename,
            STICKER_FILETYPE,
            0,
            0,
            sender_id,
            sender_name,
            cx,
        );
    }

    pub fn send_gif(
        &mut self,
        url: String,
        width: i32,
        height: i32,
        sender_id: String,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        self.send_url_attachment(
            url,
            String::new(),
            STICKER_FILETYPE,
            width,
            height,
            sender_id,
            sender_name,
            cx,
        );
    }

    pub fn send_sound(
        &mut self,
        url: String,
        filename: String,
        sender_id: String,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        self.send_url_attachment(
            url,
            filename,
            AUDIO_FILETYPE,
            0,
            0,
            sender_id,
            sender_name,
            cx,
        );
    }

    fn send_url_attachment(
        &mut self,
        url: String,
        filename: String,
        filetype: &'static str,
        width: i32,
        height: i32,
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
                filetype: filetype.to_string(),
                width,
                height,
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
                        filetype: filetype.to_string(),
                        width,
                        height,
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
                    tracing::error!("send url attachment failed: {e}");
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
        if self.poll_ui.is_empty()
            && self.poll_my_vote.is_empty()
            && self.select_ui.is_empty()
            && self.embed_form.is_empty()
            && self.pending_send_payloads.is_empty()
        {
            return;
        }
        let stale: Vec<MessageId> = self
            .poll_ui
            .keys()
            .chain(self.poll_my_vote.keys())
            .chain(self.select_ui.keys())
            .chain(self.embed_form.keys())
            .chain(self.pending_send_payloads.keys())
            .copied()
            .filter(|id| !self.message_is_loaded(*id))
            .collect();
        for id in stale {
            self.poll_ui.remove(&id);
            self.poll_my_vote.remove(&id);
            self.select_ui.remove(&id);
            self.embed_form.remove(&id);
            self.pending_send_payloads.remove(&id);
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
            if self.reply_target.take().is_some() {
                cx.emit(MessagesEvent::ReplyTargetChanged);
            }
            self.sync_anonymous_mode(cx);
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
        self.sync_anonymous_mode(cx);
        if self.reply_target.take().is_some() {
            cx.emit(MessagesEvent::ReplyTargetChanged);
        }
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
        let viewer_id = viewer_user_id(cx);

        match code {
            MessageCode::ChatUpdate | MessageCode::UpdateEphemeralMsg => {
                let target_id = self.mutation_storage_channel(m, message_id);
                let incoming = message_from_channel_proto(
                    m,
                    message_id.get(),
                    AppConfig::try_global(cx),
                    viewer_id,
                );
                let presign_keys = presign::parse_presign_finish_keys(&m.content);
                self.apply_message_update(target_id, message_id, incoming, presign_keys, cx);
            }
            MessageCode::ChatRemove | MessageCode::DeleteEphemeralMsg => {
                let target_id = self.mutation_storage_channel(m, message_id);
                self.apply_message_remove(target_id, message_id, cx);
                if m.topic_id != 0 {
                    TopicsStore::global(cx).update(cx, |store, cx| {
                        store.decrement_topic_reply_count(m.topic_id, cx);
                    });
                }
            }
            _ => {
                self.count_topic_reply(m, storage_id, message_id, cx);
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
                let incoming = message_from_channel_proto(
                    m,
                    message_id.get(),
                    AppConfig::try_global(cx),
                    viewer_id,
                );
                self.apply_incoming_message(storage_id, incoming, cx);
            }
        }
    }

    fn count_topic_reply(
        &mut self,
        m: &mezon_proto::api::ChannelMessage,
        _storage_id: ChannelId,
        message_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        self.note_topic_reply(m.topic_id, message_id, i64::from(m.create_time_seconds), cx);
    }

    fn note_topic_reply(
        &mut self,
        topic_id: i64,
        message_id: MessageId,
        create_time: i64,
        cx: &mut Context<Self>,
    ) {
        if topic_id == 0 {
            return;
        }
        if !mark_topic_reply_counted(
            &mut self.counted_topic_replies,
            &mut self.counted_topic_reply_order,
            message_id,
        ) {
            return;
        }
        TopicsStore::global(cx).update(cx, |store, cx| {
            store.increment_topic_reply_count(topic_id, create_time, cx);
        });
    }

    fn apply_incoming_message(
        &mut self,
        storage_id: ChannelId,
        msg: Message,
        cx: &mut Context<Self>,
    ) {
        let is_active = self.active_channel_id == Some(storage_id);
        let is_active_topic = self.active_topic_id == Some(storage_id);
        let incoming_id = msg.id;
        let is_buzz = msg.code == MessageCode::MessageBuzz;
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            self.set_last_message(storage_id, msg.id);
            return;
        };
        if channel.messages.contains_id(msg.id) {
            if channel.messages.merge_existing(msg.id, msg) {
                if is_active_topic {
                    cx.emit(MessagesEvent::TopicUpdated {
                        topic_id: storage_id.get(),
                    });
                    cx.notify();
                } else if is_active {
                    cx.emit(MessagesEvent::Updated {
                        message_id: Some(incoming_id),
                    });
                    cx.notify();
                }
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
        if is_buzz && appended {
            self.play_buzz_sound(cx);
        }
        if is_active {
            if appended {
                self.emit_appended(old_len, cx);
            } else {
                cx.emit(MessagesEvent::Updated {
                    message_id: Some(tail_id),
                });
                cx.notify();
            }
        } else if is_active_topic {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
            cx.notify();
        }
    }

    fn apply_message_update(
        &mut self,
        storage_id: ChannelId,
        message_id: MessageId,
        incoming: Message,
        presign_keys: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let base_img = AppConfig::try_global(cx)
            .map(|c| c.base_img_url.clone())
            .unwrap_or_default();
        let is_active = self.active_channel_id == Some(storage_id);
        let preview = incoming.content.clone();
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            return;
        };
        let Some(existing) = channel.messages.get_mut_by_id(message_id) else {
            return;
        };
        merge_message_update(existing, &incoming);
        if let Some(keys) = &presign_keys {
            apply_presign_gate(
                &mut existing.attachments,
                keys,
                &base_img,
                existing.create_time,
            );
        }
        patch_reply_previews_after_update(&mut channel.messages, message_id, &preview);
        if self.active_topic_id == Some(storage_id) {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
            cx.notify();
        } else if is_active {
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
        let deleted_topic_id = self
            .cache
            .get(&storage_id)
            .and_then(|channel| channel.messages.get_by_id(message_id))
            .and_then(|msg| msg.topic_id);
        let Some(channel) = self.cache.get_mut(&storage_id) else {
            self.on_message_deleted(message_id, deleted_topic_id, cx);
            return;
        };
        patch_reply_previews_after_delete(&mut channel.messages, message_id);
        if let Some(index) = channel.messages.remove_id(message_id) {
            self.on_message_deleted(message_id, deleted_topic_id, cx);
            if self.active_topic_id == Some(storage_id) {
                cx.emit(MessagesEvent::TopicUpdated {
                    topic_id: storage_id.get(),
                });
            } else if is_active {
                cx.emit(MessagesEvent::RemovedAt { index, message_id });
            }
            cx.notify();
        }
    }

    fn on_message_deleted(
        &mut self,
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(topics) = TopicsStore::try_global(cx) {
            topics.update(cx, |store, _| store.clear_init_topic_message_if(message_id));
        }
        self.close_topic_panel_if_origin_deleted(message_id, message_topic_id, cx);
    }

    fn close_topic_panel_if_origin_deleted(
        &mut self,
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) {
        let Some(topics) = TopicsStore::try_global(cx) else {
            return;
        };
        let should_close = topics
            .read(cx)
            .should_close_on_message_deleted(message_id, message_topic_id);
        if !should_close {
            return;
        }
        self.set_active_topic(None, cx);
        topics.update(cx, |store, cx| store.close_panel_from_messages(cx));
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
        let Some(parent_channel_id) = self.active_channel_id else {
            return;
        };
        let storage_id = self.reaction_storage_channel(message_id);
        let Some(current_uid) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return;
        };
        let uid_str = current_uid.get().to_string();
        let key = reaction_key(&emoji_id, &emoji).to_string();
        let applied = self
            .cache
            .get_mut(&storage_id)
            .and_then(|channel| channel.messages.get_mut_by_id(message_id))
            .map(|msg| {
                apply_reaction_event(&mut msg.reactions, &emoji_id, &emoji, &uid_str, remove);
            })
            .is_some();
        if applied {
            if !remove {
                *self
                    .pending_self_adds
                    .entry((storage_id, message_id, key))
                    .or_insert(0) += 1;
            }
            if self.active_topic_id == Some(storage_id) {
                cx.emit(MessagesEvent::TopicUpdated {
                    topic_id: storage_id.get(),
                });
            } else {
                cx.emit(MessagesEvent::Updated {
                    message_id: Some(message_id),
                });
            }
            cx.notify();
        }

        let api = self.api.clone();
        let clan_id = self.active_clan_id.map_or(0, |c| c.get());
        let mode = self.mode;
        let is_public = self.is_public;
        let message_sender_id = current_uid.get();
        let emoji_id_num = emoji_id.parse::<i64>().unwrap_or(0);
        let api_channel = parent_channel_id.get();
        let api_topic_id = if self.active_topic_id == Some(storage_id) {
            storage_id.get()
        } else {
            0
        };
        let message = message_id.get();
        let storage_for_rollback = storage_id;
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .react_channel_message(
                    clan_id,
                    api_channel,
                    message,
                    emoji_id_num,
                    &emoji,
                    1,
                    message_sender_id,
                    mode,
                    is_public,
                    remove,
                    api_topic_id,
                )
                .await
            {
                tracing::error!("react_channel_message failed: {e}");
                if applied {
                    let _ = this.update(cx, |store, cx| {
                        store.rollback_reaction_send(
                            storage_for_rollback,
                            message_id,
                            &emoji_id,
                            &emoji,
                            &uid_str,
                            remove,
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    fn bucket_contains(&self, bucket: ChannelId, message_id: MessageId) -> bool {
        self.cache
            .get(&bucket)
            .is_some_and(|c| c.messages.contains_id(message_id))
    }

    fn reaction_storage_channel(&self, message_id: MessageId) -> ChannelId {
        if let Some(topic_id) = self.active_topic_id {
            if self.bucket_contains(topic_id, message_id) {
                return topic_id;
            }
            if let Some(parent) = self.active_channel_id
                && self.bucket_contains(parent, message_id)
            {
                return parent;
            }
            return topic_id;
        }
        self.active_channel_id.unwrap_or(ChannelId(0))
    }

    fn mutation_storage_channel(
        &self,
        m: &mezon_proto::api::ChannelMessage,
        message_id: MessageId,
    ) -> ChannelId {
        mutation_bucket_for(m, self.active_topic_id, self.active_channel_id, |bucket| {
            self.bucket_contains(bucket, message_id)
        })
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
        if let Some(msg) = self
            .cache
            .get_mut(&channel_id)
            .and_then(|channel| channel.messages.get_mut_by_id(message_id))
        {
            rollback_reaction(&mut msg.reactions, emoji_id, emoji, sender_id, was_remove);
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

    pub fn poll_result_animating(&self, message_id: MessageId) -> bool {
        self.poll_ui
            .get(&message_id)
            .and_then(|state| state.voted_at)
            .is_some_and(|at| at.elapsed() < POLL_RESULT_ANIMATION_WINDOW)
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
        self.notify_poll_row(message_id, cx);
    }

    pub fn toggle_poll_results(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let state = self.poll_ui.entry(message_id).or_default();
        state.show_results = !state.show_results;
        self.notify_poll_row(message_id, cx);
    }

    fn notify_poll_row(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        cx.emit(MessagesEvent::Updated {
            message_id: Some(message_id),
        });
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
        self.notify_poll_row(message_id, cx);
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
                        store.poll_ui.entry(message_id).or_default().voted_at =
                            Some(std::time::Instant::now());
                    }
                    Err(e) => tracing::error!("vote_poll failed: {e}"),
                }
                store.notify_poll_row(message_id, cx);
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

    pub fn create_poll(
        &mut self,
        question: String,
        answers: Vec<String>,
        expire_hours: i32,
        poll_type: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(channel_id) = self.active_channel_id else {
            return;
        };
        let clan_id = self.active_clan_id.map_or(0, |clan| clan.get());
        let api = self.api.clone();
        let cid = channel_id.get();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .create_poll(cid, clan_id, question, answers, expire_hours, poll_type)
                .await
            {
                tracing::error!("create_poll failed: {e}");
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
        let storage_id = if r.topic_id != 0 {
            ChannelId(r.topic_id)
        } else {
            ChannelId(r.channel_id)
        };
        let message_id = MessageId(r.message_id);
        let is_active = self.active_channel_id == Some(storage_id);
        let is_active_topic = self.active_topic_id == Some(storage_id);
        let sender_id = r.sender_id.to_string();
        let emoji_id = r.emoji_id.to_string();

        if !r.action
            && self.consume_pending_self_add(
                storage_id, message_id, &emoji_id, &r.emoji, &sender_id, cx,
            )
        {
            return;
        }

        let Some(channel) = self.cache.get_mut(&storage_id) else {
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
        );
        if is_active_topic {
            cx.emit(MessagesEvent::TopicUpdated {
                topic_id: storage_id.get(),
            });
            cx.notify();
        } else if is_active {
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
        self.pending_send_payloads.remove(&temp_id);
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

    fn apply_attachment_outcome(
        &mut self,
        bucket: ChannelId,
        message_id: MessageId,
        outcome: &AttachmentUploadOutcome,
    ) -> bool {
        let (key, failed) = match outcome {
            AttachmentUploadOutcome::Uploaded(key) => (key.as_str(), false),
            AttachmentUploadOutcome::Failed(key) => (key.as_str(), true),
        };
        let Some(channel) = self.cache.get_mut(&bucket) else {
            return false;
        };
        let Some(message) = channel.messages.get_mut_by_id(message_id) else {
            return false;
        };
        let mut changed = false;
        for att in message.attachments.iter_mut() {
            if att.uploading && presign::normalize_presign_key(&att.url) == key {
                att.uploading = false;
                if failed {
                    att.upload_failed = true;
                }
                changed = true;
            }
        }
        changed
    }

    fn mark_channel_attachment_outcome(
        &mut self,
        channel_id: ChannelId,
        message_id: MessageId,
        outcome: AttachmentUploadOutcome,
        cx: &mut Context<Self>,
    ) {
        if self.apply_attachment_outcome(channel_id, message_id, &outcome)
            && self.active_channel_id == Some(channel_id)
        {
            cx.emit(MessagesEvent::Updated {
                message_id: Some(message_id),
            });
            cx.notify();
        }
    }

    pub fn apply_topic_attachment_outcome(
        &mut self,
        topic_id: i64,
        message_id: MessageId,
        outcome: AttachmentUploadOutcome,
        cx: &mut Context<Self>,
    ) {
        let bucket = ChannelId(topic_id);
        if self.apply_attachment_outcome(bucket, message_id, &outcome)
            && self.active_topic_id == Some(bucket)
        {
            cx.emit(MessagesEvent::TopicUpdated { topic_id });
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

fn carries_topic_marker(m: &mezon_proto::api::ChannelMessage) -> bool {
    serde_json::from_str::<ApiMessageContent>(&m.content)
        .ok()
        .and_then(|content| content.tp)
        .and_then(|tp| tp.parse::<i64>().ok())
        .is_some_and(|id| id != 0)
}

fn mark_pending_attachments_uploading(attachments: &mut [MessageAttachment]) {
    for att in attachments.iter_mut() {
        if att.presign_pending {
            att.uploading = true;
        }
    }
}

fn named_mutation_channel(m: &mezon_proto::api::ChannelMessage) -> ChannelId {
    if m.topic_id != 0 && !carries_topic_marker(m) {
        ChannelId(m.topic_id)
    } else {
        ChannelId(m.channel_id)
    }
}

fn mutation_bucket_for(
    m: &mezon_proto::api::ChannelMessage,
    active_topic_id: Option<ChannelId>,
    active_channel_id: Option<ChannelId>,
    bucket_contains: impl Fn(ChannelId) -> bool,
) -> ChannelId {
    let named = named_mutation_channel(m);
    if bucket_contains(named) {
        return named;
    }
    [
        Some(ChannelId(m.channel_id)),
        (m.topic_id != 0).then_some(ChannelId(m.topic_id)),
        active_topic_id,
        active_channel_id,
    ]
    .into_iter()
    .flatten()
    .find(|bucket| bucket_contains(*bucket))
    .unwrap_or(named)
}

fn mark_topic_reply_counted(
    seen: &mut HashSet<MessageId>,
    order: &mut std::collections::VecDeque<MessageId>,
    message_id: MessageId,
) -> bool {
    if message_id.is_zero() {
        return false;
    }
    if !seen.insert(message_id) {
        return false;
    }
    order.push_back(message_id);
    if order.len() > MAX_COUNTED_TOPIC_REPLIES {
        while order.len() > MAX_COUNTED_TOPIC_REPLIES / 2 {
            if let Some(evicted) = order.pop_front() {
                seen.remove(&evicted);
            }
        }
    }
    true
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

pub(crate) fn message_from_channel_proto(
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
                att.upload_failed = prior_att.upload_failed;
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
    existing.raw_content = incoming.raw_content.clone();
    existing.mention_targets = incoming.mention_targets.clone();
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

pub(crate) fn prepare_messages(
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

#[derive(Clone, Copy)]
struct SparseAckGaps {
    sender: bool,
    name: bool,
    avatar: bool,
    time: bool,
}

fn sparse_topic_ack_gaps(msg: &Message) -> Option<SparseAckGaps> {
    let gaps = SparseAckGaps {
        sender: msg.sender_id.is_empty() || msg.sender_id.as_str() == "0",
        name: msg.sender_name.is_empty(),
        avatar: msg.avatar_url.is_empty(),
        time: msg.create_time <= 0,
    };
    (gaps.sender || gaps.name || gaps.avatar || gaps.time).then_some(gaps)
}

fn fill_sparse_topic_ack(
    msg: &mut Message,
    gaps: SparseAckGaps,
    viewer_id: Option<UserId>,
    sender_id: String,
    profile: (String, String, SharedString),
    now: i64,
) {
    let (display_name, avatar_url, avatar_proxied) = profile;
    if gaps.sender {
        msg.sender_id = sender_id;
        msg.sender_user_id = viewer_id.or_else(|| msg.sender_id.parse().ok().map(UserId));
    }
    if gaps.name && !display_name.is_empty() {
        msg.sender_name = display_name.into();
    }
    if gaps.avatar && !avatar_url.is_empty() {
        msg.avatar_url = avatar_url.into();
        msg.avatar_proxied = avatar_proxied;
    }
    if gaps.time {
        msg.create_time = now;
        msg.day_label = local_day_key(now);
        msg.time_hhmm = format_local_time_hhmm(now).into();
        msg.local_date = local_datetime(now).map(|dt| dt.date_naive());
    }
}

fn enrich_sparse_topic_ack(
    msg: &mut Message,
    viewer_id: Option<UserId>,
    clan_id: Option<ClanId>,
    cx: &App,
) {
    let Some(gaps) = sparse_topic_ack_gaps(msg) else {
        return;
    };
    let sender_id = if gaps.sender {
        viewer_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| msg.sender_id.to_string())
    } else {
        msg.sender_id.to_string()
    };
    let profile = outgoing_sender_profile(
        &sender_id,
        msg.sender_name.as_ref(),
        clan_id.unwrap_or_default(),
        cx,
    );
    fill_sparse_topic_ack(msg, gaps, viewer_id, sender_id, profile, unix_now_seconds());
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
        att.upload_failed = prior_att.upload_failed;
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

fn apply_presign_gate(
    attachments: &mut Vec<MessageAttachment>,
    keys: &[String],
    base_img: &str,
    create_time: i64,
) {
    apply_presign_gate_at(attachments, keys, base_img, create_time, now_unix_seconds());
}

fn apply_presign_gate_at(
    attachments: &mut Vec<MessageAttachment>,
    keys: &[String],
    base_img: &str,
    create_time: i64,
    now: i64,
) {
    let presignable = attachments
        .iter()
        .filter(|a| presign::is_mezon_cdn(&a.url, base_img))
        .count();
    let all_finished = presign::all_presign_finished(presignable, keys.len());
    if all_finished {
        for a in attachments.iter_mut() {
            a.presign_pending = false;
        }
    } else {
        attachments.retain(|a| {
            !presign::is_expired_presign_attachment(&a.url, Some(keys), base_img, create_time, now)
        });
        for a in attachments.iter_mut() {
            a.presign_pending = presign::presign_pending(&a.url, Some(keys), base_img);
        }
    }
    #[cfg(debug_assertions)]
    for a in attachments
        .iter()
        .filter(|a| presign::is_mezon_cdn(&a.url, base_img))
    {
        tracing::info!(
            key = %presign::normalize_presign_key(&a.url),
            finish_keys = keys.len(),
            all_finished,
            pending = a.presign_pending,
            "presign gate"
        );
    }
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

pub(crate) fn viewer_user_id(cx: &App) -> Option<UserId> {
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
            username: mention.username.clone(),
            s: mention.s,
            e: mention.e,
        })
        .collect();
    let references: Vec<MessageReference> = m
        .references
        .iter()
        .map(|r| message_reference_from_api(r, cfg))
        .collect();
    let reactions = aggregate_reactions(&m.reactions);
    let mut attachments: Vec<MessageAttachment> = m
        .attachments
        .into_iter()
        .map(|a| MessageAttachment::from_api(a, cfg))
        .collect();
    let presign_keys: Option<Vec<String>> = m.content_tokens.presign_finish.as_ref().map(|ks| {
        ks.iter()
            .map(|k| presign::normalize_presign_key(k))
            .collect()
    });
    if let Some(keys) = presign_keys {
        let base_img = cfg.map(|c| c.base_img_url.as_str()).unwrap_or_default();
        apply_presign_gate(&mut attachments, &keys, base_img, m.create_time);
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
    Message::new(
        MessageId(m.message_id),
        m.content,
        m.sender_id.to_string(),
        m.sender_name,
        m.create_time,
    )
    .with_raw_content(&m.content_raw)
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

fn outgoing_content_from_raw(raw: &str) -> Option<OutgoingContent> {
    let content: ApiMessageContent = serde_json::from_str(raw).ok()?;
    let to_i32 = |value: Option<i64>| i32::try_from(value.unwrap_or(0)).unwrap_or(0);
    let mentions = content
        .mentions
        .iter()
        .map(|token| OutgoingMention {
            user_id: token.user_id.clone().unwrap_or_default(),
            role_id: token.role_id.clone().unwrap_or_default(),
            display: token.username.clone().unwrap_or_default(),
            s: to_i32(token.s),
            e: to_i32(token.e),
        })
        .collect();
    let hashtags = content
        .hg
        .iter()
        .map(|token| OutgoingHashtag {
            channel_id: token.channel_id.clone().unwrap_or_default(),
            s: to_i32(token.s),
            e: to_i32(token.e),
        })
        .collect();
    let emojis = content
        .ej
        .iter()
        .map(|token| OutgoingEmoji {
            emoji_id: token.emojiid.clone().unwrap_or_default(),
            s: to_i32(token.s),
            e: to_i32(token.e),
        })
        .collect();
    Some(OutgoingContent {
        mentions,
        hashtags,
        emojis,
    })
}

pub(crate) fn build_ogp_preview(
    content: &ApiMessageContent,
    cfg: Option<&AppConfig>,
) -> Option<Box<OgpPreview>> {
    let token = content.mk.iter().find(|tok| {
        tok.kind.as_deref() == Some("lk_ogp")
            && !tok
                .url
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("/invite/")
    })?;
    let mut url = token
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        url = first_content_link_url(content).unwrap_or_default();
    }
    if url.is_empty() {
        let s = token.s.unwrap_or(0);
        let e = token.e.unwrap_or(0);
        if e > s {
            url = utf16_slice(&content.t, s, e);
        }
    }
    let title = token.title.clone().unwrap_or_default();
    let description = token.description.clone().unwrap_or_default();
    let image = token.image.as_deref().unwrap_or_default();
    if url.is_empty() && title.is_empty() && description.is_empty() && image.is_empty() {
        return None;
    }
    let image_proxied = cfg
        .map(|c| c.imgproxy_url(image, 350, 200, "fit"))
        .unwrap_or_else(|| image.to_string());
    Some(Box::new(OgpPreview {
        url,
        title: title.into(),
        description: description.into(),
        image_proxied: image_proxied.into(),
    }))
}

fn utf16_slice(text: &str, start: i64, end: i64) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let total = units.len() as i64;
    let s = start.clamp(0, total) as usize;
    let e = end.clamp(0, total) as usize;
    if e <= s {
        return String::new();
    }
    String::from_utf16_lossy(&units[s..e])
}

fn first_content_link_url(content: &ApiMessageContent) -> Option<String> {
    let resolve = |tok: &mezon_client::transport::ContentToken| -> Option<String> {
        if let Some(url) = tok
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            return Some(url.to_string());
        }
        let s = tok.s.unwrap_or(0);
        let e = tok.e.unwrap_or(0);
        if e > s {
            let sliced = utf16_slice(&content.t, s, e);
            if !sliced.is_empty() {
                return Some(sliced);
            }
        }
        None
    };
    for tok in &content.mk {
        if matches!(
            tok.kind.as_deref(),
            Some("lk" | "vk" | "lk_yt" | "lk_fb" | "lk_tt")
        ) && let Some(url) = resolve(tok)
        {
            return Some(url);
        }
    }
    for tok in content
        .lk
        .iter()
        .chain(content.vk.iter())
        .chain(content.lky.iter())
    {
        if let Some(url) = resolve(tok) {
            return Some(url);
        }
    }
    detect_markdown(&content.t)
        .into_iter()
        .find(|tok| tok.kind == "lk")
        .map(|tok| utf16_slice(&content.t, i64::from(tok.s), i64::from(tok.e)))
        .filter(|url| !url.is_empty())
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

pub(crate) fn build_embeds(content: &ApiMessageContent, cfg: Option<&AppConfig>) -> Arc<[Embed]> {
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
            input: parse_embed_input(field.inputs.as_ref()),
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

const EMBED_COMPONENT_TYPE_SELECT: i32 = 2;
const EMBED_COMPONENT_TYPE_INPUT: i32 = 3;

fn parse_embed_input(value: Option<&serde_json::Value>) -> Option<EmbedInput> {
    let value = value?;
    let wrapper: ApiEmbedInputWrapper = serde_json::from_value(value.clone()).ok()?;
    let id = wrapper.id.filter(|id| !id.is_empty())?;
    match wrapper.component_type {
        Some(EMBED_COMPONENT_TYPE_INPUT) => {
            let component: ApiMessageInput =
                serde_json::from_value(wrapper.component).unwrap_or_default();
            Some(EmbedInput::Text(EmbedTextInput {
                id: id.into(),
                placeholder: component.placeholder.unwrap_or_default().into(),
                default_value: component.default_value.unwrap_or_default().into(),
                multiline: component.textarea,
                required: component.required,
                disabled: component.disabled,
            }))
        }
        Some(EMBED_COMPONENT_TYPE_SELECT) => {
            let component: ApiSelectComponent =
                serde_json::from_value(wrapper.component).unwrap_or_default();
            let options = component
                .options
                .iter()
                .map(|option| MessageSelectOption {
                    label: option.label.clone().into(),
                    value: option.value.clone().into(),
                    description: option.description.clone().map(Into::into),
                    default: option.default,
                })
                .collect();
            Some(EmbedInput::Select(MessageSelect {
                select_type: component.select_type.unwrap_or(1),
                options,
                placeholder: component.placeholder.clone().map(Into::into),
                min_options: component.min_options,
                max_options: wrapper.max_options.or(component.max_options),
                disabled: component.disabled,
                id: Some(id.into()),
                value_selected: component
                    .value_selected
                    .as_ref()
                    .map(|option| option.value.clone().into()),
            }))
        }
        _ => None,
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
            value_selected: s
                .value_selected
                .as_ref()
                .map(|option| option.value.clone().into()),
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
        mut a: mezon_client::transport::ApiAttachment,
        cfg: Option<&AppConfig>,
    ) -> Self {
        if let Some(cfg) = cfg {
            if let std::borrow::Cow::Owned(url) = cfg.read_media_url(&a.url) {
                a.url = url;
            }
            if let std::borrow::Cow::Owned(thumbnail) = cfg.read_media_url(&a.thumbnail) {
                a.thumbnail = thumbnail;
            }
        }
        let width = a.width.max(0) as u32;
        let height = a.height.max(0) as u32;
        let is_video = MessageAttachment::media_is_video(&a.filetype, &a.url);
        let (proxied_src, display_width, display_height) = cfg
            .map(|c| c.attachment_proxy(&a.url, width, height, is_video))
            .unwrap_or_else(|| {
                let (w, h) = if is_video {
                    crate::config::video_attachment_display_dimensions(width, height)
                } else {
                    crate::config::attachment_display_dimensions(width, height)
                };
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
            upload_failed: false,
        }
    }

    pub(crate) fn optimistic_local(att: &OutgoingAttachment) -> Self {
        let width = att.width.max(0) as u32;
        let height = att.height.max(0) as u32;
        let is_video = MessageAttachment::media_is_video(&att.filetype, "");
        let (display_width, display_height) = if is_video {
            crate::config::video_attachment_display_dimensions(width, height)
        } else {
            crate::config::attachment_display_dimensions(width, height)
        };
        let is_image = att.filetype.starts_with("image/");
        Self {
            url: String::new(),
            filename: att.filename.clone(),
            filetype: att.filetype.clone(),
            width,
            height,
            thumbnail: String::new(),
            duration: att.duration,
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
            upload_failed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::UserId;
    use crate::message::MessageSpan;

    #[test]
    fn parse_embed_accent_accepts_normalized_hex_colors() {
        let accent = parse_embed_accent(Some("#00BFFF")).expect("hex color");
        assert_eq!(accent, gpui::rgb(0x00_bf_ff));
    }

    #[test]
    fn parse_embed_accent_preserves_red_channel() {
        let accent = parse_embed_accent(Some("#FF0000")).expect("red color");
        assert_eq!(accent, gpui::rgb(0xff_00_00));
    }

    #[test]
    fn build_embeds_maps_numeric_json_color_to_accent() {
        let content: ApiMessageContent =
            serde_json::from_str(r#"{"embed":[{"color":49151,"title":"Saved"}]}"#)
                .expect("embed json");
        let embeds = build_embeds(&content, None);
        let embed = embeds.first().expect("one embed");
        assert_eq!(embed.accent, Some(gpui::rgb(0x00_bf_ff)));
    }

    #[test]
    fn build_embeds_maps_discord_red_numeric_json_to_accent() {
        let content: ApiMessageContent =
            serde_json::from_str(r#"{"embed":[{"color":16711680,"title":"Alert"}]}"#)
                .expect("embed json");
        let embeds = build_embeds(&content, None);
        let embed = embeds.first().expect("one embed");
        assert_eq!(embed.accent, Some(gpui::rgb(0xff_00_00)));
    }

    #[test]
    fn parse_embed_input_extracts_text_input() {
        let value = serde_json::json!({
            "type": 3,
            "id": "project",
            "component": {
                "placeholder": "Enter project",
                "type": "text",
                "required": true,
                "textarea": false,
                "defaultValue": "Alpha",
                "disabled": false
            }
        });
        let EmbedInput::Text(input) =
            parse_embed_input(Some(&value)).expect("input component should parse")
        else {
            panic!("expected a text input");
        };
        assert_eq!(input.id.as_ref(), "project");
        assert_eq!(input.placeholder.as_ref(), "Enter project");
        assert_eq!(input.default_value.as_ref(), "Alpha");
        assert!(input.required);
        assert!(!input.multiline);
        assert!(!input.disabled);
    }

    #[test]
    fn parse_embed_input_extracts_select() {
        let value = serde_json::json!({
            "type": 2,
            "id": "typeOfWork",
            "component": {
                "type": 1,
                "placeholder": "Pick one",
                "max_options": 1,
                "options": [
                    { "label": "Coding", "value": "code" },
                    { "label": "Review", "value": "review" }
                ]
            }
        });
        let EmbedInput::Select(select) =
            parse_embed_input(Some(&value)).expect("select component should parse")
        else {
            panic!("expected a select");
        };
        assert_eq!(select.id.as_deref(), Some("typeOfWork"));
        assert_eq!(select.options.len(), 2);
        assert_eq!(select.options[0].value.as_ref(), "code");
    }

    #[test]
    fn parse_embed_input_ignores_unsupported_and_incomplete() {
        let datepicker = serde_json::json!({ "type": 4, "id": "date", "component": {} });
        assert!(parse_embed_input(Some(&datepicker)).is_none());
        let missing_id = serde_json::json!({ "type": 3, "component": {} });
        assert!(parse_embed_input(Some(&missing_id)).is_none());
        assert!(parse_embed_input(None).is_none());
    }

    #[test]
    fn build_embed_maps_field_input_including_textarea() {
        let field = mezon_client::transport::ApiEmbedField {
            name: "Project".into(),
            inputs: Some(serde_json::json!({
                "type": 3,
                "id": "project",
                "component": { "textarea": true }
            })),
            ..Default::default()
        };
        let api = ApiEmbed {
            fields: vec![field],
            ..Default::default()
        };
        let embed = build_embed(&api, None);
        let mapped = embed.fields.first().expect("one field");
        let Some(EmbedInput::Text(input)) = mapped.input.as_ref() else {
            panic!("field should carry a text input");
        };
        assert_eq!(input.id.as_ref(), "project");
        assert!(input.multiline);
    }

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
    fn forward_target_label_names_the_destination() {
        let channel = ForwardTarget::Channel {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
            channel_type: 1,
            mode: 2,
            is_public: true,
            label: "#general".into(),
        };
        let friend = ForwardTarget::Friend {
            user_id: UserId(9),
            label: "bob".into(),
            avatar: String::new(),
            username: "bob".into(),
        };

        assert_eq!(channel.label(), "#general");
        assert_eq!(friend.label(), "bob");
    }

    #[test]
    fn forward_source_carries_the_whole_content_payload() {
        let raw = r#"{"t":"hey @bob","mentions":[{"s":4,"e":8,"user_id":"77","username":"bob"}],"mk":[{"s":0,"e":3,"type":"b"}]}"#;
        let msg = Message::new(
            MessageId(1),
            "hey @bob".to_string(),
            "5".to_string(),
            "me",
            100,
        )
        .with_raw_content(raw);

        let source = forward_source(&msg);

        assert_eq!(
            source.content_raw, raw,
            "the server content payload must be forwarded verbatim, not collapsed to plain text"
        );
        assert_eq!(source.mentions.len(), 1);
        assert_eq!(source.mentions[0].user_id, "77");
        assert_eq!(source.mentions[0].s, 4);
        assert_eq!(source.mentions[0].e, 8);
    }

    #[test]
    fn forward_source_falls_back_to_plain_text_for_optimistic_messages() {
        let msg = Message::new(
            MessageId(1),
            "hello".to_string(),
            "5".to_string(),
            "me",
            100,
        );

        let source = forward_source(&msg);

        assert!(source.content_raw.is_empty());
        assert_eq!(source.text, "hello");
        assert!(source.mentions.is_empty());
    }

    #[test]
    fn forward_mentions_skips_targets_without_a_user_or_role() {
        let targets = vec![
            MentionTarget {
                user_id: None,
                role_id: None,
                username: String::new(),
                s: 0,
                e: 1,
            },
            MentionTarget {
                user_id: None,
                role_id: Some("9".into()),
                username: "@mods".into(),
                s: 2,
                e: 3,
            },
        ];

        let mentions = forward_mentions(&targets);

        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].role_id, "9");
        assert_eq!(mentions[0].username, "@mods");
        assert_eq!((mentions[0].s, mentions[0].e), (2, 3));
    }

    #[test]
    fn forward_mentions_come_from_the_proto_targets_not_the_content_json() {
        let mut msg = Message::new(MessageId(1), "hey @bob", "7", "alice", 100);
        msg.raw_content = Some(r#"{"t":"hey @bob"}"#.into());
        msg.mention_targets = vec![MentionTarget {
            user_id: Some("42".into()),
            role_id: None,
            username: "bob".into(),
            s: 4,
            e: 8,
        }];

        let source = forward_source(&msg);

        assert_eq!(source.mentions.len(), 1);
        assert_eq!(source.mentions[0].user_id, "42");
        assert_eq!(source.mentions[0].username, "bob");
        assert_eq!((source.mentions[0].s, source.mentions[0].e), (4, 8));
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
    fn attachment_get_urls_use_the_read_cdn() {
        let cfg = AppConfig::dev_defaults();
        let attachment = MessageAttachment::from_api(
            mezon_client::transport::ApiAttachment {
                url: format!("{}/clip.mp4", cfg.upload_img_url),
                filename: "clip.mp4".into(),
                filetype: "video/mp4".into(),
                width: 1280,
                height: 720,
                thumbnail: format!("{}/clip-thumb.jpg", cfg.upload_img_url),
                duration: 42,
                size: 0,
            },
            Some(&cfg),
        );
        assert_eq!(attachment.url, format!("{}/clip.mp4", cfg.base_img_url));
        assert_eq!(
            attachment.thumbnail,
            format!("{}/clip-thumb.jpg", cfg.base_img_url)
        );
        assert!(attachment.thumbnail_proxied.contains(&cfg.base_img_url));
        assert!(!attachment.thumbnail_proxied.contains(&cfg.upload_img_url));
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
                content_raw: String::new(),
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
                content_raw: String::new(),
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

    #[test]
    fn message_from_api_gates_cdn_attachment_until_presign_finished() {
        let cfg = AppConfig {
            base_img_url: "https://cdn.example".into(),
            ..AppConfig::dev_defaults()
        };
        let msg = |finish: Option<Vec<String>>| ApiMessage {
            message_id: 5,
            content: "hi".into(),
            content_raw: String::new(),
            content_tokens: mezon_client::transport::ApiMessageContent {
                t: "hi".into(),
                presign_finish: finish,
                ..Default::default()
            },
            code: 0,
            sender_id: 1,
            sender_name: "Alice".into(),
            avatar: String::new(),
            create_time: 0,
            update_time: 0,
            hide_editted: false,
            attachments: vec![mezon_client::transport::ApiAttachment {
                url: "https://cdn.example/uploads/photo.png".into(),
                filename: "photo.png".into(),
                filetype: "image/png".into(),
                width: 800,
                height: 600,
                thumbnail: String::new(),
                duration: 0,
                size: 0,
            }],
            references: vec![],
            reactions: vec![],
            entity_mentions: vec![],
        };

        let pending = message_from_api(msg(Some(vec![])), Some(&cfg), None);
        assert!(pending.attachments[0].presign_pending);

        let finished = message_from_api(msg(Some(vec!["photo".into()])), Some(&cfg), None);
        assert!(!finished.attachments[0].presign_pending);

        let mismatched_key_but_count_reached =
            message_from_api(msg(Some(vec!["some-other-key".into()])), Some(&cfg), None);
        assert!(!mismatched_key_but_count_reached.attachments[0].presign_pending);

        let no_field = message_from_api(msg(None), Some(&cfg), None);
        assert!(!no_field.attachments[0].presign_pending);
    }

    #[test]
    fn partial_update_recomputes_presign_pending_on_kept_attachments() {
        let base = "https://cdn.example";
        let mut existing = Message::new(MessageId(1), "hi", "u1", "U1", 100);
        existing.attachments = vec![MessageAttachment {
            url: "https://cdn.example/uploads/photo.png".into(),
            presign_pending: true,
            ..Default::default()
        }];

        let incoming = Message::new(MessageId(1), "hi", "u1", "U1", 100);
        assert!(incoming.attachments.is_empty());
        merge_message_update(&mut existing, &incoming);
        assert!(
            existing.attachments[0].presign_pending,
            "partial update keeps the prior attachment with its stale flag"
        );

        apply_presign_gate(
            &mut existing.attachments,
            &["photo".to_string()],
            base,
            existing.create_time,
        );
        assert!(
            !existing.attachments[0].presign_pending,
            "recompute against the arrived presign_finish flips the gate"
        );
    }

    fn plain_api_message(
        code: i32,
        references: Vec<mezon_client::transport::ApiMessageRef>,
    ) -> ApiMessage {
        ApiMessage {
            message_id: 11,
            content: "yo".into(),
            content_raw: String::new(),
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
                url: "https://cdn.example/photo.png".into(),
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
                url: "https://cdn.example/photo.png".into(),
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
            duration: 0,
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
    fn optimistic_markdown_content_matches_stripped_echo() {
        let optimistic_text = build_send_content("**bold**", &[], &[], &[]).text;
        assert_eq!(
            optimistic_text, "bold",
            "optimistic text must be stripped like the server-stored text"
        );
        let temp = MessageId::next_optimistic();
        let list =
            MessageList::from_messages(vec![Message::new(temp, optimistic_text, "u9", "Me", 200)]);
        assert_eq!(
            list.temp_match_position("u9", "bold"),
            Some(0),
            "stripped optimistic must reconcile with the stripped realtime echo"
        );
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

    fn topic_proto(
        channel_id: i64,
        topic_id: i64,
        content: &str,
    ) -> mezon_proto::api::ChannelMessage {
        mezon_proto::api::ChannelMessage {
            channel_id,
            topic_id,
            content: content.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn storage_channel_id_routes_topic_reply_to_topic_bucket_not_parent() {
        let reply = topic_proto(10, 99, r#"{"t":"a reply"}"#);
        assert_eq!(storage_channel_id(&reply), ChannelId(99));
        assert_eq!(parent_channel_id(&reply), ChannelId(10));
    }

    #[test]
    fn storage_channel_id_routes_plain_message_to_its_channel_bucket() {
        let plain = topic_proto(10, 0, r#"{"t":"hello"}"#);
        assert_eq!(storage_channel_id(&plain), ChannelId(10));
        assert_eq!(parent_channel_id(&plain), ChannelId(10));
    }

    #[test]
    fn carries_topic_marker_only_for_a_nonzero_numeric_tp() {
        assert!(carries_topic_marker(&topic_proto(
            10,
            0,
            r#"{"t":"origin","tp":"99"}"#
        )));
        assert!(!carries_topic_marker(&topic_proto(
            10,
            0,
            r#"{"t":"origin","tp":"0"}"#
        )));
        assert!(!carries_topic_marker(&topic_proto(
            10,
            0,
            r#"{"t":"plain"}"#
        )));
        assert!(!carries_topic_marker(&topic_proto(10, 0, "not json")));
    }

    #[test]
    fn named_mutation_channel_sends_a_topic_reply_to_the_topic_bucket() {
        let reply = topic_proto(10, 99, r#"{"t":"a reply"}"#);
        assert_eq!(named_mutation_channel(&reply), ChannelId(99));
    }

    #[test]
    fn named_mutation_channel_keeps_a_topic_origin_in_its_parent_channel() {
        let origin = topic_proto(10, 99, r#"{"t":"origin","tp":"99"}"#);
        assert_eq!(
            named_mutation_channel(&origin),
            ChannelId(10),
            "a message that merely HAS a topic hanging off it lives in the parent channel"
        );
    }

    #[test]
    fn named_mutation_channel_sends_a_plain_message_to_its_channel() {
        let plain = topic_proto(10, 0, r#"{"t":"hello"}"#);
        assert_eq!(named_mutation_channel(&plain), ChannelId(10));
    }

    #[test]
    fn mutation_bucket_for_prefers_the_named_bucket_over_the_parent_that_also_holds_the_id() {
        let reply = topic_proto(10, 99, r#"{"t":"a reply"}"#);
        let bucket = mutation_bucket_for(&reply, Some(ChannelId(99)), Some(ChannelId(10)), |b| {
            b == ChannelId(99) || b == ChannelId(10)
        });
        assert_eq!(
            bucket,
            ChannelId(99),
            "a topic reply edits the copy in the topic bucket, not a same-id row in the parent"
        );
    }

    #[test]
    fn mutation_bucket_for_falls_back_to_the_bucket_that_actually_holds_the_message() {
        let reply = topic_proto(10, 99, r#"{"t":"a reply"}"#);
        let bucket = mutation_bucket_for(&reply, Some(ChannelId(99)), Some(ChannelId(10)), |b| {
            b == ChannelId(10)
        });
        assert_eq!(
            bucket,
            ChannelId(10),
            "the edit must land in the bucket that really holds the row"
        );
    }

    #[test]
    fn mutation_bucket_for_falls_back_to_the_active_topic_panel() {
        let orphan = topic_proto(0, 0, r#"{"t":"a reply"}"#);
        let bucket = mutation_bucket_for(&orphan, Some(ChannelId(99)), Some(ChannelId(10)), |b| {
            b == ChannelId(99)
        });
        assert_eq!(bucket, ChannelId(99));
    }

    #[test]
    fn mutation_bucket_for_returns_the_named_bucket_when_nothing_holds_the_message() {
        let reply = topic_proto(10, 99, r#"{"t":"a reply"}"#);
        let bucket = mutation_bucket_for(&reply, Some(ChannelId(7)), Some(ChannelId(8)), |_| false);
        assert_eq!(bucket, ChannelId(99));
    }

    #[test]
    fn mark_topic_reply_counted_counts_the_same_message_id_only_once() {
        let mut seen = HashSet::new();
        let mut order = std::collections::VecDeque::new();
        let id = MessageId(42);
        assert!(mark_topic_reply_counted(&mut seen, &mut order, id));
        assert!(
            !mark_topic_reply_counted(&mut seen, &mut order, id),
            "the realtime echo and our own send ack must not both bump the reply count"
        );
        assert!(!mark_topic_reply_counted(&mut seen, &mut order, id));
    }

    #[test]
    fn mark_topic_reply_counted_counts_distinct_message_ids() {
        let mut seen = HashSet::new();
        let mut order = std::collections::VecDeque::new();
        assert!(mark_topic_reply_counted(
            &mut seen,
            &mut order,
            MessageId(42)
        ));
        assert!(mark_topic_reply_counted(
            &mut seen,
            &mut order,
            MessageId(43)
        ));
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn mark_topic_reply_counted_ignores_a_zero_message_id() {
        let mut seen = HashSet::new();
        let mut order = std::collections::VecDeque::new();
        assert!(!mark_topic_reply_counted(
            &mut seen,
            &mut order,
            MessageId(0)
        ));
        assert!(seen.is_empty());
        assert!(order.is_empty());
    }

    #[test]
    fn mark_topic_reply_counted_evicts_oldest_but_still_dedups_recent_ids() {
        let mut seen = HashSet::new();
        let mut order = std::collections::VecDeque::new();
        for i in 1..=(MAX_COUNTED_TOPIC_REPLIES as i64 + 1) {
            assert!(mark_topic_reply_counted(
                &mut seen,
                &mut order,
                MessageId(i)
            ));
        }
        assert!(seen.len() <= MAX_COUNTED_TOPIC_REPLIES);
        assert_eq!(seen.len(), order.len());

        let newest = MessageId(MAX_COUNTED_TOPIC_REPLIES as i64 + 1);
        assert!(seen.contains(&newest));
        assert!(
            !mark_topic_reply_counted(&mut seen, &mut order, newest),
            "eviction must not break dedup for recently counted replies"
        );
        assert!(!seen.contains(&MessageId(1)));
    }

    fn cdn_attachment(url: &str) -> MessageAttachment {
        MessageAttachment {
            url: url.into(),
            presign_pending: false,
            ..Default::default()
        }
    }

    const TEST_CDN: &str = "https://cdn.example";

    #[test]
    fn presign_gate_keeps_attachment_pending_when_the_finish_list_is_empty() {
        let mut attachments = vec![cdn_attachment("https://cdn.example/uploads/photo.png")];
        apply_presign_gate_at(&mut attachments, &[], TEST_CDN, 1000, 1000);
        assert_eq!(attachments.len(), 1);
        assert!(
            attachments[0].presign_pending,
            "an ack with no finished keys must not advertise the upload as done"
        );
    }

    #[test]
    fn presign_gate_clears_pending_when_the_finish_list_contains_the_key() {
        let mut attachments = vec![cdn_attachment("https://cdn.example/uploads/photo.png")];
        attachments[0].presign_pending = true;
        apply_presign_gate_at(
            &mut attachments,
            &["photo".to_string()],
            TEST_CDN,
            1000,
            1000,
        );
        assert_eq!(attachments.len(), 1);
        assert!(!attachments[0].presign_pending);
    }

    #[test]
    fn presign_gate_short_circuits_on_key_count_even_when_the_key_does_not_match() {
        let mut attachments = vec![cdn_attachment("https://cdn.example/uploads/photo.png")];
        attachments[0].presign_pending = true;
        apply_presign_gate_at(
            &mut attachments,
            &["some-other-key".to_string()],
            TEST_CDN,
            1000,
            1000,
        );
        assert!(!attachments[0].presign_pending);
    }

    #[test]
    fn presign_gate_keeps_second_attachment_pending_until_both_keys_arrive() {
        let mut attachments = vec![
            cdn_attachment("https://cdn.example/uploads/a.png"),
            cdn_attachment("https://cdn.example/uploads/b.png"),
        ];
        apply_presign_gate_at(&mut attachments, &["a".to_string()], TEST_CDN, 1000, 1000);
        assert_eq!(attachments.len(), 2);
        assert!(!attachments[0].presign_pending);
        assert!(attachments[1].presign_pending);
    }

    #[test]
    fn presign_gate_drops_a_pending_attachment_that_never_finished_uploading() {
        let mut attachments = vec![cdn_attachment("https://cdn.example/uploads/photo.png")];
        apply_presign_gate_at(
            &mut attachments,
            &[],
            TEST_CDN,
            1000,
            1000 + presign::PRESIGN_PENDING_MAX_AGE_SEC,
        );
        assert!(
            attachments.is_empty(),
            "a stale never-uploaded attachment is dropped instead of rendering broken forever"
        );
    }

    #[test]
    fn presign_gate_leaves_a_non_cdn_attachment_alone() {
        let mut attachments = vec![cdn_attachment("https://tenor.com/view/cat.gif")];
        apply_presign_gate_at(
            &mut attachments,
            &[],
            TEST_CDN,
            1000,
            1000 + presign::PRESIGN_PENDING_MAX_AGE_SEC,
        );
        assert_eq!(attachments.len(), 1);
        assert!(!attachments[0].presign_pending);
    }

    #[test]
    fn topic_ack_marks_only_presign_pending_attachments_as_uploading() {
        let mut attachments = vec![
            MessageAttachment {
                url: "https://cdn.example/uploads/pending.png".into(),
                presign_pending: true,
                ..Default::default()
            },
            MessageAttachment {
                url: "https://cdn.example/uploads/done.png".into(),
                presign_pending: false,
                ..Default::default()
            },
        ];
        mark_pending_attachments_uploading(&mut attachments);
        assert!(
            attachments[0].uploading,
            "a topic attachment still waiting on presign must render as uploading"
        );
        assert!(!attachments[1].uploading);
    }

    fn sparse_ack_message() -> Message {
        let mut msg = Message::new(MessageId(7), "a topic reply", "", "", 0);
        msg.avatar_url = "".into();
        msg
    }

    fn test_profile() -> (String, String, SharedString) {
        (
            "Alice".to_string(),
            "https://cdn.example/alice.png".to_string(),
            SharedString::from("https://proxy/alice.png"),
        )
    }

    #[test]
    fn sparse_topic_ack_gaps_detects_each_missing_field() {
        assert!(sparse_topic_ack_gaps(&sparse_ack_message()).is_some());

        let mut zero_sender = Message::new(MessageId(7), "hi", "0", "Alice", 100);
        zero_sender.avatar_url = "https://cdn.example/alice.png".into();
        let gaps = sparse_topic_ack_gaps(&zero_sender).expect("sender id 0 is sparse");
        assert!(gaps.sender);
        assert!(!gaps.name);
        assert!(!gaps.time);
    }

    #[test]
    fn sparse_topic_ack_gaps_is_none_for_a_complete_ack() {
        let mut complete = Message::new(MessageId(7), "hi", "42", "Alice", 100);
        complete.avatar_url = "https://cdn.example/alice.png".into();
        assert!(
            sparse_topic_ack_gaps(&complete).is_none(),
            "a fully populated ack must not be rewritten"
        );
    }

    #[test]
    fn fill_sparse_topic_ack_fills_sender_name_avatar_and_time() {
        let mut msg = sparse_ack_message();
        let gaps = sparse_topic_ack_gaps(&msg).expect("the ack is sparse");
        fill_sparse_topic_ack(
            &mut msg,
            gaps,
            Some(UserId(42)),
            "42".to_string(),
            test_profile(),
            1_700_000_000,
        );

        assert_eq!(msg.sender_id, "42");
        assert_eq!(msg.sender_user_id, Some(UserId(42)));
        assert_eq!(msg.sender_name, "Alice");
        assert_eq!(msg.avatar_url, "https://cdn.example/alice.png");
        assert_eq!(msg.avatar_proxied, "https://proxy/alice.png");
        assert_eq!(msg.create_time, 1_700_000_000);
        assert!(!msg.day_label.is_empty());
        assert!(!msg.time_hhmm.is_empty());
    }

    #[test]
    fn fill_sparse_topic_ack_never_stamps_a_topic_id_on_a_reply() {
        let mut msg = sparse_ack_message();
        assert_eq!(msg.topic_id, None);
        let gaps = sparse_topic_ack_gaps(&msg).expect("the ack is sparse");
        fill_sparse_topic_ack(
            &mut msg,
            gaps,
            Some(UserId(42)),
            "42".to_string(),
            test_profile(),
            1_700_000_000,
        );
        assert_eq!(
            msg.topic_id, None,
            "topic_id marks a message as a topic ORIGIN; a reply that carries one masquerades as the anchor"
        );
        assert_ne!(msg.code, MessageCode::Topic);
    }

    #[test]
    fn fill_sparse_topic_ack_keeps_fields_the_server_already_sent() {
        let mut msg = Message::new(MessageId(7), "hi", "42", "Bob", 500);
        let gaps = sparse_topic_ack_gaps(&msg).expect("the avatar is missing");
        fill_sparse_topic_ack(
            &mut msg,
            gaps,
            Some(UserId(99)),
            "99".to_string(),
            test_profile(),
            1_700_000_000,
        );

        assert_eq!(
            msg.sender_id, "42",
            "a server-sent sender is never rewritten"
        );
        assert_eq!(msg.sender_name, "Bob");
        assert_eq!(msg.create_time, 500);
        assert_eq!(msg.avatar_url, "https://cdn.example/alice.png");
    }

    #[test]
    fn has_more_bottom_ignores_an_optimistic_tail_id() {
        let list =
            MessageList::from_messages(vec![Message::new(MessageId(1), "a", "u1", "U", 100)]);
        assert!(
            !has_more_bottom_for(Some(MessageId::next_optimistic()), &list),
            "an un-acked optimistic row is not evidence of unloaded server history"
        );
    }

    #[test]
    fn removing_a_reply_in_the_middle_keeps_the_index_and_order() {
        let mut list = MessageList::from_messages(vec![
            Message::new(MessageId(1), "first", "u1", "U1", 100),
            Message::new(MessageId(2), "second", "u2", "U2", 200),
            Message::new(MessageId(3), "third", "u3", "U3", 300),
        ]);

        assert_eq!(list.remove_id(MessageId(2)), Some(1));
        assert_list_consistent(&list);
        assert_eq!(
            list.as_slice().iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![MessageId(1), MessageId(3)]
        );
        assert_eq!(
            list.position(MessageId(3)),
            Some(1),
            "a stale index after a mid-list delete renders the WRONG row"
        );
        assert_eq!(list.remove_id(MessageId(404)), None);
    }
}
