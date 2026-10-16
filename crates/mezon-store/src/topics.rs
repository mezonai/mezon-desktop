use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{StreamExt, stream};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::transport::{
    LOCATION_CODE, MESSAGE_BUZZ_CODE, OutgoingMessageFlags, message_sender_name, prioritize_avatar,
};
use mezon_client::{
    AppApi, AttachmentUploadOutcome, ConnectionStatus, RealtimeEvent, TopicDiscussion, UploadFile,
    UrlAttachment, topic_discussion_from_api,
};
use mezon_proto::{api, realtime};

use crate::channel::{ChannelList, ChannelType};
use crate::channel_permissions::ChannelPermissionsStore;
use crate::message::MessageCode;
use crate::message_time::{normalize_unix_seconds, unix_now_seconds};
use crate::messages::{
    MessagesEvent, MessagesStore, OutgoingAttachment, OutgoingContent, OutgoingEmoji,
    OutgoingHashtag, OutgoingMention, ReplyDraft, viewer_user_id,
};
use crate::presign;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::upload_jobs::UploadJob;
use crate::{AppConfig, CACHE_TTL, ChannelId, ClanId, Message, MessageId, MessageRef, UserId};

const TOPICS_LIMIT: i32 = 50;
const INBOX_MESSAGE_DIRECTION_AROUND: i32 = 2;
const INBOX_AVATAR_LOOKUP_LIMIT: u32 = 2;
const INBOX_AVATAR_LOOKUP_CONCURRENCY: usize = 4;
const STREAM_MODE_CHANNEL: i32 = 2;
const STREAM_MODE_THREAD: i32 = 6;
const UPDATED_NOTIFY_COALESCE: Duration = Duration::from_millis(100);

fn topic_anonymous_send(existing_topic_id: Option<i64>, clan_id: i64, cx: &App) -> bool {
    existing_topic_id.is_some()
        && clan_id != 0
        && MessagesStore::try_global(cx).is_some_and(|store| store.read(cx).topic_anonymous_mode())
}

#[derive(Debug, Clone)]
pub enum TopicsEvent {
    Updated,
    Opened,
    Closed,
    ReplySent,
    ReplyTargetChanged,
}

#[derive(Debug, Clone)]
pub struct TopicMessageMeta {
    pub rpl: i32,
    pub lsnt: i64,
    pub tp_id: String,
}

#[derive(Default)]
struct TopicCompose {
    origin_message: Option<Message>,
    origin_message_id: Option<MessageId>,
    parent_channel_id: Option<i64>,
    clan_id: Option<i64>,
    mode: i32,
    is_public: bool,
    active_topic_id: Option<i64>,
    submitting: bool,
    creating: bool,
    error: Option<String>,
}

#[derive(Default)]
struct TopicsData {
    topics: Vec<TopicDiscussion>,
    topic_index: std::collections::HashMap<String, usize>,
    topic_meta: std::collections::HashMap<String, TopicMessageMeta>,
}

impl TopicsData {
    fn clear(&mut self) {
        self.topics.clear();
        self.topic_index.clear();
        self.topic_meta.clear();
    }

    fn clear_topics(&mut self) {
        self.topics.clear();
        self.topic_index.clear();
    }

    fn topics(&self) -> &[TopicDiscussion] {
        &self.topics
    }

    fn set_topics(&mut self, topics: Vec<TopicDiscussion>) {
        self.topics = topics;
        self.resort_topics();
    }

    fn topic_by_id(&self, id: &str) -> Option<&TopicDiscussion> {
        self.topic_index
            .get(id)
            .and_then(|&index| self.topics.get(index))
    }

    fn topic_id_for_origin_message(&self, origin_message_key: &str) -> Option<String> {
        self.topics
            .iter()
            .find(|t| t.message_id == origin_message_key)
            .map(|t| t.id.clone())
    }

    fn topic_meta(&self, topic_key: &str) -> Option<&TopicMessageMeta> {
        self.topic_meta.get(topic_key)
    }

    fn topic_reply_summary(&self, topic_key: &str) -> (i32, Option<i64>) {
        let Some(meta) = self.topic_meta(topic_key) else {
            return (0, None);
        };
        let lsnt = normalize_unix_seconds(meta.lsnt);
        (meta.rpl, (lsnt > 0).then_some(lsnt))
    }

    fn has_topic_meta(&self, topic_key: &str) -> bool {
        self.topic_meta.contains_key(topic_key)
    }

    fn rebuild_topic_index(&mut self) {
        self.topic_index = self
            .topics
            .iter()
            .enumerate()
            .map(|(index, topic)| (topic.id.clone(), index))
            .collect();
    }

    fn resort_topics(&mut self) {
        self.topics
            .sort_by_key(|t| std::cmp::Reverse(t.last_message_timestamp));
        self.rebuild_topic_index();
    }

    fn advance_topic_timestamp(&mut self, topic_id: &str, timestamp_sec: i64) {
        if timestamp_sec <= 0 {
            return;
        }
        let next = timestamp_sec.clamp(0, i64::from(u32::MAX)) as u32;
        let Some(idx) = self.topic_index.get(topic_id).copied() else {
            return;
        };
        let Some(topic) = self.topics.get_mut(idx) else {
            return;
        };
        if topic.last_message_timestamp >= next {
            return;
        }
        topic.last_message_timestamp = next;
        self.resort_topics();
    }

    fn note_topic_message(&mut self, message: &api::ChannelMessage) -> bool {
        if message.topic_id <= 0 || message.code == 2 {
            return false;
        }
        let topic_id = message.topic_id.to_string();
        let Some(index) = self.topic_index.get(&topic_id).copied() else {
            return false;
        };
        let Some(topic) = self.topics.get_mut(index) else {
            return false;
        };
        if !message.content.is_empty() || message.code == 1 {
            topic.last_message_content = message.content.clone();
        }
        topic.last_message_attachments =
            mezon_client::transport::parse_message_attachments(&message.attachments);
        update_topic_message_sender(topic, message.message_id, message.sender_id);
        let timestamp = message.create_time_seconds;
        if timestamp > topic.last_message_timestamp {
            topic.last_message_timestamp = timestamp;
            self.resort_topics();
        }
        true
    }

    fn note_api_topic_message(
        &mut self,
        topic_id: &str,
        message: &mezon_client::transport::ApiMessage,
    ) -> bool {
        if message.code == 2 {
            return false;
        }
        let Some(index) = self.topic_index.get(topic_id).copied() else {
            return false;
        };
        let Some(topic) = self.topics.get_mut(index) else {
            return false;
        };
        if !message.content.is_empty() || message.code == 1 {
            topic.last_message_content = message.content.clone();
        }
        topic.last_message_attachments = message.attachments.clone();
        update_topic_message_sender(topic, message.message_id, message.sender_id);
        let timestamp =
            normalize_unix_seconds(message.create_time).clamp(0, i64::from(u32::MAX)) as u32;
        if timestamp >= topic.last_message_timestamp {
            topic.last_message_timestamp = timestamp;
            self.resort_topics();
        }
        true
    }

    fn merge_topic(&mut self, topic: TopicDiscussion) {
        let topic_id = topic.id.clone();
        if let Some(idx) = self.topic_index.get(&topic_id).copied() {
            let existing = &mut self.topics[idx];
            if !topic.content.is_empty() {
                existing.content = topic.content;
            }
            if !topic.last_message_content.is_empty() {
                existing.last_message_content = topic.last_message_content;
            }
            if !topic.last_message_attachments.is_empty() {
                existing.last_message_attachments = topic.last_message_attachments;
            }
            if topic.last_message_timestamp > 0 {
                existing.last_message_timestamp = existing
                    .last_message_timestamp
                    .max(topic.last_message_timestamp);
            }
            if let Some(message_id) = topic
                .last_message_id
                .parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
            {
                let sender_id = if topic.last_sender_is_authoritative {
                    topic.last_sender_id.parse::<i64>().unwrap_or(0)
                } else {
                    0
                };
                update_topic_message_sender(existing, message_id, sender_id);
            } else if !existing.last_sender_is_authoritative
                && !topic.last_sender_id.is_empty()
                && topic.last_sender_id != "0"
            {
                existing.last_sender_id = topic.last_sender_id;
            }
            if !topic.message_id.is_empty() && topic.message_id != "0" {
                existing.message_id = topic.message_id;
            }
        } else {
            self.topics.push(topic);
            if let Some(last) = self.topics.last() {
                self.topic_index
                    .insert(last.id.clone(), self.topics.len() - 1);
            }
        }
    }

    fn upsert_topic(&mut self, topic: TopicDiscussion) {
        self.merge_topic(topic);
        self.resort_topics();
    }

    fn upsert_topics(&mut self, topics: impl IntoIterator<Item = TopicDiscussion>) {
        for topic in topics {
            self.merge_topic(topic);
        }
        self.resort_topics();
    }

    fn upsert_topic_meta(&mut self, topic_id: String, rpl: i32, lsnt: i64) {
        let lsnt = normalize_unix_seconds(lsnt);
        match self.topic_meta.entry(topic_id.clone()) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                existing.rpl = rpl;
                existing.lsnt = existing.lsnt.max(lsnt);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(TopicMessageMeta {
                    rpl,
                    lsnt,
                    tp_id: topic_id.clone(),
                });
            }
        }
        self.advance_topic_timestamp(&topic_id, lsnt);
    }

    fn touch_topic_last_sent(&mut self, topic_id: i64, timestamp_sec: i64) -> bool {
        if topic_id == 0 {
            return false;
        }
        let timestamp_sec = normalize_unix_seconds(timestamp_sec);
        if timestamp_sec <= 0 {
            return false;
        }
        let key = topic_id.to_string();
        if let Some(meta) = self.topic_meta.get_mut(&key) {
            meta.lsnt = meta.lsnt.max(timestamp_sec);
        } else {
            self.topic_meta.insert(
                key.clone(),
                TopicMessageMeta {
                    rpl: 0,
                    lsnt: timestamp_sec,
                    tp_id: key.clone(),
                },
            );
        }
        self.advance_topic_timestamp(&key, timestamp_sec);
        true
    }

    fn increment_topic_reply_count(&mut self, topic_id: i64, timestamp_sec: i64) -> bool {
        if topic_id == 0 {
            return false;
        }
        let timestamp_sec = normalize_unix_seconds(timestamp_sec);
        let key = topic_id.to_string();
        if let Some(meta) = self.topic_meta.get_mut(&key) {
            meta.rpl = meta.rpl.saturating_add(1);
            if timestamp_sec > 0 {
                meta.lsnt = meta.lsnt.max(timestamp_sec);
            }
        } else {
            self.topic_meta.insert(
                key.clone(),
                TopicMessageMeta {
                    rpl: 1,
                    lsnt: timestamp_sec,
                    tp_id: key.clone(),
                },
            );
        }
        self.advance_topic_timestamp(&key, timestamp_sec);
        true
    }

    fn decrement_topic_reply_count(&mut self, topic_id: i64) -> bool {
        if topic_id == 0 {
            return false;
        }
        let key = topic_id.to_string();
        let Some(meta) = self.topic_meta.get_mut(&key) else {
            return false;
        };
        meta.rpl = meta.rpl.saturating_sub(1).max(0);
        true
    }
}

const MAX_TOPIC_FETCH_FAILURES: u32 = 3;

const INBOX_TOPIC_JUMP_TIMEOUT: Duration = Duration::from_secs(20);

struct PendingInboxTopicJump {
    topic_id: i64,
    origin_id: MessageId,
    reply_id: MessageId,
    channel_id: ChannelId,
    request_id: u64,
    requested_at: Instant,
}

impl PendingInboxTopicJump {
    fn matches(
        &self,
        message_id: MessageId,
        channel_id: ChannelId,
        request_id: Option<u64>,
        now: Instant,
    ) -> bool {
        self.origin_id == message_id
            && self.channel_id == channel_id
            && Some(self.request_id) == request_id
            && now.duration_since(self.requested_at) <= INBOX_TOPIC_JUMP_TIMEOUT
    }

    fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.requested_at) > INBOX_TOPIC_JUMP_TIMEOUT
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InboxMessageAvatar {
    Loading,
    Ready(String, String),
    Missing,
    Failed,
}

impl InboxMessageAvatar {
    fn update_identity(&mut self, name: &str, avatar: &str) -> bool {
        if avatar.is_empty() {
            if *self == Self::Missing {
                return false;
            }
            *self = Self::Missing;
        } else {
            if matches!(self, Self::Ready(current_name, current_avatar)
                if current_name == name && current_avatar == avatar)
            {
                return false;
            }
            *self = Self::Ready(name.to_string(), avatar.to_string());
        }
        true
    }
}

pub struct TopicsStore {
    inbox_message_avatars: std::collections::HashMap<(ChannelId, MessageId), InboxMessageAvatar>,
    inbox_message_generation: u64,
    inbox_avatar_permits: Arc<tokio::sync::Semaphore>,
    data: TopicsData,
    clan_id: Option<String>,
    loading: bool,
    has_more: bool,
    next_page: i32,
    fetch_failures: u32,
    fetch_generation: u64,
    preview_generation: u64,
    hydrated_preview_topic_id: Option<String>,
    fetched_at: Option<Instant>,
    panel_open: bool,
    init_topic_message_id: Option<MessageId>,
    reply_target: Option<ReplyDraft>,
    compose: TopicCompose,
    compose_generation: u64,
    creating_topic_for: Option<MessageId>,
    pending_inbox_jump: Option<PendingInboxTopicJump>,
    api: Arc<AppApi>,
    updated_notify_task: Option<Task<()>>,
    _conn_watch: Task<()>,
    _messages_sub: Option<Subscription>,
}

struct GlobalTopicsStore(Entity<TopicsStore>);
impl Global for GlobalTopicsStore {}

impl EventEmitter<TopicsEvent> for TopicsStore {}

impl TopicsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalTopicsStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self::register_realtime(cx);
        let messages_sub = MessagesStore::try_global(cx).map(|store| {
            cx.subscribe(&store, |this, _, event, cx| {
                this.on_messages_event(event, cx);
            })
        });
        Self {
            inbox_message_avatars: Default::default(),
            inbox_message_generation: 0,
            inbox_avatar_permits: Arc::new(tokio::sync::Semaphore::new(
                INBOX_AVATAR_LOOKUP_CONCURRENCY,
            )),
            data: TopicsData::default(),
            clan_id: None,
            loading: false,
            has_more: true,
            next_page: 1,
            fetch_failures: 0,
            fetch_generation: 0,
            preview_generation: 0,
            hydrated_preview_topic_id: None,
            fetched_at: None,
            panel_open: false,
            init_topic_message_id: None,
            reply_target: None,
            compose: TopicCompose::default(),
            compose_generation: 0,
            creating_topic_for: None,
            pending_inbox_jump: None,
            api,
            updated_notify_task: None,
            _conn_watch: conn_watch,
            _messages_sub: messages_sub,
        }
    }

    fn schedule_updated_notify(&mut self, cx: &mut Context<Self>) {
        if self.updated_notify_task.is_some() {
            return;
        }
        self.updated_notify_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(UPDATED_NOTIFY_COALESCE)
                .await;
            let _ = this.update(cx, |store, cx| {
                store.updated_notify_task = None;
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            });
        }));
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
                        .update(cx, |this, cx| this.refetch_active_clan(cx))
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

    fn refetch_active_clan(&mut self, cx: &mut Context<Self>) {
        self.fetched_at = None;
        self.fetch_failures = 0;
        if let Some(clan_id) = self.clan_id.clone() {
            self.fetch(&clan_id, cx);
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalTopicsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalTopicsStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.inbox_message_avatars.clear();
        self.inbox_message_generation = self.inbox_message_generation.wrapping_add(1);
        self.data.clear();
        self.clan_id = None;
        self.loading = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.hydrated_preview_topic_id = None;
        self.fetched_at = None;
        self.init_topic_message_id = None;
        self.updated_notify_task = None;
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.creating_topic_for = None;
        self.pending_inbox_jump = None;
        self.close_panel(cx);
        cx.emit(TopicsEvent::Updated);
        cx.notify();
    }

    pub fn is_panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn origin_message(&self) -> Option<&Message> {
        self.compose.origin_message.as_ref()
    }

    pub fn is_init_topic_message(&self, message_id: MessageId) -> bool {
        self.init_topic_message_id == Some(message_id)
    }

    pub fn clear_init_topic_message_if(&mut self, message_id: MessageId) {
        if self.init_topic_message_id == Some(message_id) {
            self.init_topic_message_id = None;
        }
    }

    pub fn reply_target(&self) -> Option<&ReplyDraft> {
        self.reply_target.as_ref()
    }

    pub fn set_reply_to(&mut self, target: MessageRef, cx: &mut Context<Self>) {
        let Some(draft) = MessagesStore::global(cx)
            .read(cx)
            .reply_draft_for_with_config(target, AppConfig::try_global(cx))
        else {
            return;
        };
        self.reply_target = Some(draft);
        cx.emit(TopicsEvent::ReplyTargetChanged);
        cx.notify();
    }

    pub fn clear_reply(&mut self, cx: &mut Context<Self>) {
        if self.reply_target.take().is_some() {
            cx.emit(TopicsEvent::ReplyTargetChanged);
            cx.notify();
        }
    }

    pub fn active_topic_id(&self) -> Option<i64> {
        self.compose.active_topic_id
    }

    pub fn is_submitting(&self) -> bool {
        self.compose.submitting
    }

    pub fn create_error(&self) -> Option<&str> {
        self.compose.error.as_deref()
    }

    pub fn topic_meta_for_topic(&self, topic_id: ChannelId) -> Option<&TopicMessageMeta> {
        self.data.topic_meta(&topic_id.to_string())
    }

    pub fn topic_reply_summary(&self, topic_id: ChannelId) -> (i32, Option<i64>) {
        self.data.topic_reply_summary(&topic_id.to_string())
    }

    fn upsert_topic_meta(&mut self, topic_id: String, rpl: i32, lsnt: i64, cx: &mut Context<Self>) {
        self.data.upsert_topic_meta(topic_id, rpl, lsnt);
        self.schedule_updated_notify(cx);
    }

    pub fn touch_topic_last_sent(
        &mut self,
        topic_id: i64,
        timestamp_sec: i64,
        cx: &mut Context<Self>,
    ) {
        if self.data.touch_topic_last_sent(topic_id, timestamp_sec) {
            self.schedule_updated_notify(cx);
        }
    }

    pub fn increment_topic_reply_count(
        &mut self,
        topic_id: i64,
        timestamp_sec: i64,
        cx: &mut Context<Self>,
    ) {
        if self
            .data
            .increment_topic_reply_count(topic_id, timestamp_sec)
        {
            self.schedule_updated_notify(cx);
        }
    }

    pub fn decrement_topic_reply_count(&mut self, topic_id: i64, cx: &mut Context<Self>) {
        if self.data.decrement_topic_reply_count(topic_id) {
            self.schedule_updated_notify(cx);
        }
    }

    fn resolve_topic_id_for_origin(&self, origin_message_id: i64, cx: &App) -> Option<String> {
        let origin_key = origin_message_id.to_string();
        if let Some(topic_id) = self.data.topic_id_for_origin_message(&origin_key) {
            return Some(topic_id);
        }
        MessagesStore::try_global(cx).and_then(|store| {
            store
                .read(cx)
                .viewport_message_by_id(MessageId(origin_message_id))
                .and_then(|msg| msg.topic_id.map(|id| id.to_string()))
        })
    }

    pub fn ensure_create_permissions(cx: &mut App) {
        let messages = MessagesStore::global(cx).read(cx);
        let (Some(channel_id), Some(clan_id)) =
            (messages.active_channel_id(), messages.active_clan_id())
        else {
            return;
        };
        if clan_id.is_zero() {
            return;
        }
        ChannelPermissionsStore::global(cx).update(cx, |store, cx| {
            store.ensure_loaded(clan_id, channel_id, cx);
        });
    }

    pub fn can_create_topic(cx: &App) -> bool {
        let messages = MessagesStore::global(cx).read(cx);
        if messages.is_dm() {
            return false;
        }
        let mode = messages.mode();
        if mode != STREAM_MODE_CHANNEL && mode != STREAM_MODE_THREAD {
            return false;
        }
        let (Some(channel_id), Some(clan_id)) =
            (messages.active_channel_id(), messages.active_clan_id())
        else {
            return false;
        };
        if clan_id.is_zero() {
            return false;
        }
        let channel_lookup = ChannelList::global(cx)
            .read(cx)
            .channel(clan_id, channel_id)
            .map(|c| c.channel_type.as_raw());
        let Some(channel_type_raw) = channel_lookup else {
            return false;
        };
        if matches!(
            channel_type_raw,
            x if x == ChannelType::App.as_raw()
                || x == ChannelType::Voice.as_raw()
                || x == ChannelType::Stream.as_raw()
        ) {
            return false;
        }
        ChannelPermissionsStore::global(cx).read(cx).has_permission(
            "send-message",
            clan_id,
            channel_id,
        )
    }

    pub fn message_allows_topic_discussion(msg: &Message) -> bool {
        if matches!(msg.code, MessageCode::Topic | MessageCode::Poll) || msg.code.is_system() {
            false
        } else {
            !matches!(
                msg.code,
                MessageCode::CreateThread
                    | MessageCode::CreatePin
                    | MessageCode::MessageBuzz
                    | MessageCode::AuditLog
                    | MessageCode::Welcome
                    | MessageCode::UpcomingEvent
            )
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::SdTopicEvent, &entity, |this, event, cx| {
                this.handle_sd_topic_event(event, cx);
            });
            dispatch.on(
                RealtimeKind::TopicInMessageEvent,
                &entity,
                |this, event, cx| {
                    this.handle_topic_in_message_event(event, cx);
                },
            );
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_channel_message(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| this.resync(cx));
        });
    }

    fn resync(&mut self, cx: &mut Context<Self>) {
        tracing::info!("TopicsStore resync — refetching topics for the active clan");
        self.refetch_active_clan(cx);
    }

    fn is_active_clan(&self, clan_id: i64, cx: &App) -> bool {
        MessagesStore::global(cx).read(cx).active_clan_id() == Some(ClanId(clan_id))
    }

    fn handle_sd_topic_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::SdTopicEvent(ev) = event else {
            return;
        };
        if self.is_active_clan(ev.clan_id, cx) {
            let topic = topic_discussion_from_api(sd_topic_from_event(ev));
            self.data.upsert_topic(topic);
        }
        let lsnt = sd_topic_event_last_sent_seconds(ev, unix_now_seconds());
        let topic_key = ev.id.to_string();
        if self.data.has_topic_meta(&topic_key) {
            self.touch_topic_last_sent(ev.id, lsnt, cx);
        } else {
            self.upsert_topic_meta(topic_key, 0, lsnt, cx);
        }
        MessagesStore::global(cx).update(cx, |store, cx| {
            store.mark_message_as_topic(
                ChannelId(ev.channel_id),
                MessageId(ev.message_id),
                ev.id,
                (ev.user_id != 0).then_some(UserId(ev.user_id)),
                cx,
            );
        });
    }

    fn handle_topic_in_message_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::TopicInMessageEvent(ev) = event else {
            return;
        };
        let event_tp_id = Some(ev.tp_id.clone()).filter(|id| !id.is_empty() && id != "0");
        let Some(tp_id) =
            event_tp_id.or_else(|| self.resolve_topic_id_for_origin(ev.message_id, cx))
        else {
            return;
        };
        let lsnt = normalize_unix_seconds(ev.lsnt);
        self.upsert_topic_meta(tp_id, ev.rpl, lsnt, cx);
    }

    fn handle_channel_message(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ChannelMessage(message) = event else {
            return;
        };
        if !self.is_active_clan(message.clan_id, cx) {
            return;
        }
        let previous_avatar_key = self
            .data
            .topic_by_id(&message.topic_id.to_string())
            .and_then(inbox_message_key);
        if message.code == 2 && message.topic_id > 0 {
            self.hydrated_preview_topic_id = None;
            self.hydrate_latest_topic_preview(cx);
        } else if self.data.note_topic_message(message) {
            let next_key = (ChannelId(message.topic_id), MessageId(message.message_id));
            if let Some(key) = previous_avatar_key.filter(|key| *key != next_key) {
                self.inbox_message_avatars.remove(&key);
            }
            if message.message_id > 0 {
                let avatar = prioritize_avatar(&message.clan_avatar, &message.avatar);
                if avatar.is_empty() {
                    self.inbox_message_avatars
                        .entry(next_key)
                        .or_insert(InboxMessageAvatar::Missing);
                } else {
                    self.inbox_message_avatars
                        .entry(next_key)
                        .or_insert(InboxMessageAvatar::Missing)
                        .update_identity(message_sender_name(message), &avatar);
                }
            }
            self.schedule_updated_notify(cx);
        }
    }

    fn on_messages_event(&mut self, event: &MessagesEvent, cx: &mut Context<Self>) {
        if matches!(
            event,
            MessagesEvent::Updated { .. }
                | MessagesEvent::Reset { .. }
                | MessagesEvent::Resized { .. }
                | MessagesEvent::Shifted { .. }
                | MessagesEvent::TopicUpdated { .. }
        ) {
            self.refresh_inbox_message_avatars(event, cx);
        }
        let MessagesEvent::JumpTo {
            message_id,
            request_id,
        } = event
        else {
            return;
        };
        let now = Instant::now();
        let Some(pending) = self.pending_inbox_jump.as_ref() else {
            return;
        };
        if pending.is_expired(now) {
            self.pending_inbox_jump = None;
            return;
        }
        let Some(active_channel_id) =
            MessagesStore::try_global(cx).and_then(|store| store.read(cx).active_channel_id())
        else {
            return;
        };
        if !pending.matches(*message_id, active_channel_id, *request_id, now) {
            return;
        }
        let Some(pending) = self.pending_inbox_jump.take() else {
            return;
        };
        self.open_existing_topic(
            pending.origin_id,
            pending.topic_id,
            pending.reply_id,
            pending.channel_id,
            cx,
        );
    }

    pub fn begin_inbox_topic_jump(
        &mut self,
        channel_id: ChannelId,
        topic_id: i64,
        reply_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        if topic_id <= 0 {
            return;
        }
        if let Some(origin_id) = self.known_topic_origin(topic_id) {
            self.begin_inbox_topic_jump_to_origin(channel_id, topic_id, origin_id, reply_id, cx);
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let detail = match api.get_topic_detail(&topic_id.to_string()).await {
                Ok(topic) => topic,
                Err(e) => {
                    tracing::error!("get_topic_detail failed for inbox jump: {e}");
                    let _ = this.update(cx, |_this, cx| {
                        MessagesStore::global(cx).update(cx, |store, cx| {
                            store.request_jump(channel_id, reply_id, cx);
                        });
                    });
                    return;
                }
            };
            let Some(origin_id) = detail.message_id.parse::<i64>().ok().filter(|id| *id > 0) else {
                let _ = this.update(cx, |_this, cx| {
                    MessagesStore::global(cx).update(cx, |store, cx| {
                        store.request_jump(channel_id, reply_id, cx);
                    });
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.begin_inbox_topic_jump_to_origin(
                    channel_id,
                    topic_id,
                    MessageId(origin_id),
                    reply_id,
                    cx,
                );
            });
        })
        .detach();
    }

    pub fn begin_inbox_topic_jump_to_origin(
        &mut self,
        channel_id: ChannelId,
        topic_id: i64,
        origin_id: MessageId,
        reply_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        if topic_id <= 0 || origin_id.get() <= 0 {
            return;
        }
        let request_id = MessagesStore::global(cx).update(cx, |store, cx| {
            store.request_inbox_origin_jump(channel_id, origin_id, cx)
        });
        self.pending_inbox_jump = Some(PendingInboxTopicJump {
            topic_id,
            origin_id,
            reply_id,
            channel_id,
            request_id,
            requested_at: Instant::now(),
        });
    }

    fn known_topic_origin(&self, topic_id: i64) -> Option<MessageId> {
        self.data
            .topic_by_id(&topic_id.to_string())
            .and_then(|topic| topic.message_id.parse::<i64>().ok())
            .filter(|origin_id| *origin_id > 0)
            .map(MessageId)
    }

    pub fn start_create_for_message(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(origin) = MessagesStore::global(cx)
            .read(cx)
            .viewport_message_by_id(message_id)
            .cloned()
        else {
            return;
        };
        self.start_create(origin, cx);
    }

    pub fn open_existing_topic(
        &mut self,
        origin_id: MessageId,
        topic_id: i64,
        reply_id: MessageId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let origin = {
            let store = MessagesStore::global(cx).read(cx);
            store
                .viewport_message_by_id(origin_id)
                .cloned()
                .or_else(|| store.message_in_channel(channel_id, origin_id).cloned())
        };
        let Some(mut origin) = origin else {
            return;
        };
        origin.topic_id = Some(ChannelId(topic_id));
        let jump_reply = (reply_id != origin_id).then_some(reply_id);
        self.open_topic_panel(origin, Some(topic_id), jump_reply, cx);
    }

    /// Open the discussion panel for `origin` (mezon-react `handleCreateTopic`).
    /// Reuses `origin.topic_id` when already set (topic button or card footer);
    /// otherwise the topic is created on first reply.
    pub fn start_create(&mut self, origin: Message, cx: &mut Context<Self>) {
        let topic_id = origin.topic_id.map(|t| t.get()).filter(|id| *id != 0);
        self.open_topic_panel(origin, topic_id, None, cx);
    }

    fn open_topic_panel(
        &mut self,
        origin: Message,
        topic_id: Option<i64>,
        jump_reply: Option<MessageId>,
        cx: &mut Context<Self>,
    ) {
        let messages = MessagesStore::global(cx);
        let (parent_channel_id, clan_id, mode, is_public) = {
            let store = messages.read(cx);
            (
                store.active_channel_id().map(|c| c.get()),
                store.active_clan_id().map_or(0, |c| c.get()),
                store.mode(),
                store.is_public(),
            )
        };
        let Some(parent_channel_id) = parent_channel_id else {
            return;
        };

        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.init_topic_message_id = Some(origin.id);
        if self.compose.origin_message_id != Some(origin.id) && self.reply_target.take().is_some() {
            cx.emit(TopicsEvent::ReplyTargetChanged);
        }
        self.compose = TopicCompose {
            origin_message_id: Some(origin.id),
            origin_message: Some(origin),
            parent_channel_id: Some(parent_channel_id),
            clan_id: Some(clan_id),
            mode,
            is_public,
            active_topic_id: topic_id,
            submitting: false,
            creating: false,
            error: None,
        };
        self.panel_open = true;

        messages.update(cx, |store, cx| {
            store.set_active_topic(topic_id, cx);
            if let Some(reply_id) = jump_reply {
                store.queue_topic_jump(reply_id, cx);
            }
        });
        cx.emit(TopicsEvent::Opened);
        if let Some(topic_id) = topic_id {
            messages.update(cx, |store, cx| {
                store.fetch_topic_messages(clan_id, parent_channel_id, topic_id, cx);
            });
        }
        cx.notify();
    }

    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        if !self.panel_open && self.compose.origin_message.is_none() {
            return;
        }
        self.close_panel_state(cx);
        MessagesStore::global(cx).update(cx, |store, cx| store.set_active_topic(None, cx));
    }

    pub fn should_close_on_message_deleted(
        &self,
        deleted_in: ChannelId,
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
    ) -> bool {
        panel_should_close_on_message_deleted(
            self.panel_open,
            &self.compose,
            &self.data,
            deleted_in,
            message_id,
            message_topic_id,
        )
    }

    pub fn close_panel_from_messages(&mut self, cx: &mut Context<Self>) {
        self.close_panel_state(cx);
    }

    fn close_panel_state(&mut self, cx: &mut Context<Self>) {
        if !self.panel_open && self.compose.origin_message.is_none() {
            return;
        }
        self.panel_open = false;
        self.compose = TopicCompose::default();
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.init_topic_message_id = None;
        self.reply_target = None;
        cx.emit(TopicsEvent::Closed);
        cx.notify();
    }

    pub fn maybe_close_on_message_deleted(
        &mut self,
        deleted_in: ChannelId,
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) {
        if self.should_close_on_message_deleted(deleted_in, message_id, message_topic_id) {
            self.close_panel(cx);
        }
    }

    /// Send a reply into the topic composer. On the first reply the backend topic
    /// is created (mezon-js `CreateSdTopic`) then the message is sent with the
    /// returned topic id; subsequent replies reuse it. Guards against double-submit.
    pub fn submit_reply(
        &mut self,
        content: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        cx: &mut Context<Self>,
    ) {
        self.submit_reply_with_code(content, content_tokens, attachments, 0, cx);
    }

    pub fn submit_buzz(&mut self, content: String, cx: &mut Context<Self>) {
        if content.trim().is_empty() {
            return;
        }
        self.submit_reply_with_code(
            content,
            OutgoingContent::default(),
            Vec::new(),
            MESSAGE_BUZZ_CODE,
            cx,
        );
        MessagesStore::global(cx).update(cx, |store, cx| store.play_buzz_sound(cx));
    }

    pub fn submit_location(&mut self, latitude: f64, longitude: f64, cx: &mut Context<Self>) {
        let link = mezon_client::transport::build_location_maps_link(latitude, longitude);
        self.submit_reply_with_code(
            link,
            OutgoingContent::default(),
            Vec::new(),
            LOCATION_CODE,
            cx,
        );
    }

    fn submit_reply_with_code(
        &mut self,
        content: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        message_code: i32,
        cx: &mut Context<Self>,
    ) {
        let content = content.trim().to_string();
        if (content.is_empty() && attachments.is_empty()) || self.compose.submitting {
            return;
        }
        let (Some(parent_channel_id), Some(clan_id), Some(origin_message_id)) = (
            self.compose.parent_channel_id,
            self.compose.clan_id,
            self.compose.origin_message_id,
        ) else {
            return;
        };
        let mode = self.compose.mode;
        let is_public = self.compose.is_public;
        let existing_topic_id = self.compose.active_topic_id;
        if existing_topic_id.is_none() && !self.begin_topic_create(origin_message_id) {
            return;
        }
        let anonymous = topic_anonymous_send(existing_topic_id, clan_id, cx);
        let send_flags = OutgoingMessageFlags {
            anonymous_message: anonymous,
            message_code,
        };
        let has_attachments = !attachments.is_empty();
        // The reply row is built from the server's echo, which knows nothing about
        // the file on this disk. Keep the paths so the sender sees the picture they
        // just sent instead of fetching a proxied copy of it back — the same thing
        // `MessageAttachment::optimistic_local` does for a channel send.
        let local_sources: Vec<(String, Option<std::path::PathBuf>)> = attachments
            .iter()
            .map(|att| {
                (
                    att.filename.clone(),
                    att.filetype.starts_with("image/").then(|| att.path.clone()),
                )
            })
            .collect();
        let reply_ref =
            self.reply_target
                .take()
                .map(|draft| mezon_client::transport::OutgoingReply {
                    message_ref_id: draft.message_ref_id.get(),
                    content: draft.content_raw,
                    has_attachment: draft.has_attachment,
                    message_sender_id: draft.sender_id.get(),
                    message_sender_username: draft.sender_name.clone(),
                    message_sender_avatar: draft.sender_avatar,
                    message_sender_clan_nick: String::new(),
                    message_sender_display_name: draft.sender_name,
                });

        let transport_mentions: Vec<mezon_client::transport::OutgoingMention> = content_tokens
            .mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let transport_hashtags = crate::messages::outgoing_hashtags(
            &content,
            content_tokens
                .hashtags
                .into_iter()
                .map(OutgoingHashtag::into_transport)
                .collect(),
            cx,
        );
        let transport_emojis: Vec<mezon_client::transport::OutgoingEmoji> = content_tokens
            .emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect();

        self.compose.submitting = true;
        self.compose.creating = existing_topic_id.is_none();
        self.compose.error = None;
        cx.notify();

        let generation = self.compose_generation;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let topic_id = match existing_topic_id {
                Some(id) => id,
                None => match api
                    .create_sd_topic(origin_message_id.get(), clan_id, parent_channel_id)
                    .await
                {
                    Ok(topic) => topic.id,
                    Err(e) => {
                        tracing::error!("submit_reply create_sd_topic failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
                        });
                        return;
                    }
                },
            };

            let mut upload_ctx = None;
            let mut uploaded_ctx = None;
            let ack = if has_attachments {
                let files: Vec<UploadFile> = attachments
                    .into_iter()
                    .map(|attachment| attachment.into_upload(parent_channel_id))
                    .collect();
                let presigned = match api.presign_files(files).await {
                    Ok(presigned) => presigned,
                    Err(e) => {
                        tracing::error!("submit_reply presign failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
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
                let update_hashtags = transport_hashtags.clone();
                let update_emojis = transport_emojis.clone();
                let deferred_presigned = if anonymous {
                    if let Err(e) = api.upload_presigned_all(presigned).await {
                        tracing::error!("submit_reply anonymous attachment upload failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
                        });
                        return;
                    }
                    None
                } else {
                    Some(presigned)
                };
                let sent = match api
                    .send_topic_presigned_message(
                        clan_id,
                        parent_channel_id,
                        &content,
                        is_public,
                        mode,
                        topic_id,
                        msg_attachments,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                        deferred_presigned.is_some().then(Vec::new),
                        reply_ref.clone(),
                        send_flags,
                    )
                    .await
                {
                    Ok(sent) => sent,
                    Err(e) => {
                        tracing::error!("submit_reply send_topic_presigned_message failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
                        });
                        return;
                    }
                };
                match deferred_presigned {
                    Some(presigned) => {
                        upload_ctx = Some((
                            presigned,
                            keys,
                            update_mentions,
                            update_hashtags,
                            update_emojis,
                            sent.message_id,
                            sent.create_time.max(0) as u32,
                        ));
                    }
                    None => {
                        uploaded_ctx = Some((sent.message_id, keys));
                    }
                }
                sent
            } else {
                match api
                    .send_topic_message(
                        clan_id,
                        parent_channel_id,
                        &content,
                        is_public,
                        mode,
                        topic_id,
                        transport_mentions,
                        transport_hashtags,
                        transport_emojis,
                        reply_ref,
                        send_flags,
                    )
                    .await
                {
                    Ok(ack) => ack,
                    Err(e) => {
                        tracing::error!("submit_reply send_topic_message failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
                        });
                        return;
                    }
                }
            };

            let _ = this.update(cx, |this, cx| {
                this.finish_topic_create(origin_message_id);
                this.apply_reply_sent(
                    topic_id,
                    parent_channel_id,
                    origin_message_id,
                    existing_topic_id.is_none(),
                    generation,
                    ack,
                    anonymous,
                    local_sources,
                    cx,
                );
            });

            if let Some((real_message_id, keys)) = uploaded_ctx {
                cx.update(|cx| {
                    MessagesStore::global(cx).update(cx, |store, cx| {
                        for key in keys {
                            store.apply_topic_attachment_outcome(
                                topic_id,
                                MessageId(real_message_id),
                                AttachmentUploadOutcome::Uploaded(key),
                                cx,
                            );
                        }
                    });
                });
            }
            let Some((
                presigned,
                keys,
                update_mentions,
                update_hashtags,
                update_emojis,
                real_message_id,
                create_time_seconds,
            )) = upload_ctx
            else {
                return;
            };
            let user_id = cx
                .update(|cx| crate::messages::viewer_user_id(cx))
                .unwrap_or(UserId(0));
            let job = UploadJob {
                user_id,
                clan_id,
                channel_id: topic_id,
                parent_channel_id,
                topic_id,
                message_id: real_message_id,
                mode,
                is_public,
                content: content.clone(),
                mentions: update_mentions,
                hashtags: update_hashtags.tokens,
                hashtag_channels: update_hashtags.channels,
                emojis: update_emojis,
                create_time_seconds,
                started_at: unix_now_seconds(),
                finished: Vec::new(),
                pending: crate::messages::upload_job_files(&presigned, &keys),
                sync_failures: 0,
            };
            crate::messages::run_upload_job(api.clone(), job, presigned, cx).await;
        })
        .detach();
    }

    pub fn submit_ephemeral_reply(
        &mut self,
        receiver_id: i64,
        content: String,
        content_tokens: OutgoingContent,
        attachments: Vec<OutgoingAttachment>,
        cx: &mut Context<Self>,
    ) {
        let content = content.trim().to_string();
        if (content.is_empty() && attachments.is_empty()) || self.compose.submitting {
            return;
        }
        let (Some(parent_channel_id), Some(clan_id), Some(origin_message_id)) = (
            self.compose.parent_channel_id,
            self.compose.clan_id,
            self.compose.origin_message_id,
        ) else {
            return;
        };
        let mode = self.compose.mode;
        let is_public = self.compose.is_public;
        let existing_topic_id = self.compose.active_topic_id;
        if existing_topic_id.is_none() && !self.begin_topic_create(origin_message_id) {
            return;
        }
        let reply_ref =
            self.reply_target
                .take()
                .map(|draft| mezon_client::transport::OutgoingReply {
                    message_ref_id: draft.message_ref_id.get(),
                    content: draft.content_raw,
                    has_attachment: draft.has_attachment,
                    message_sender_id: draft.sender_id.get(),
                    message_sender_username: draft.sender_name.clone(),
                    message_sender_avatar: draft.sender_avatar,
                    message_sender_clan_nick: String::new(),
                    message_sender_display_name: draft.sender_name,
                });
        let transport_mentions: Vec<mezon_client::transport::OutgoingMention> = content_tokens
            .mentions
            .into_iter()
            .map(OutgoingMention::into_transport)
            .collect();
        let transport_hashtags = crate::messages::outgoing_hashtags(
            &content,
            content_tokens
                .hashtags
                .into_iter()
                .map(OutgoingHashtag::into_transport)
                .collect(),
            cx,
        );
        let transport_emojis: Vec<mezon_client::transport::OutgoingEmoji> = content_tokens
            .emojis
            .into_iter()
            .map(OutgoingEmoji::into_transport)
            .collect();

        self.compose.submitting = true;
        self.compose.creating = existing_topic_id.is_none();
        self.compose.error = None;
        cx.emit(TopicsEvent::ReplyTargetChanged);
        cx.notify();

        let generation = self.compose_generation;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let fail =
                |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp, e: anyhow::Error| {
                    let _ = this.update(cx, |this, cx| {
                        this.finish_topic_create(origin_message_id);
                        if this.compose_generation != generation {
                            return;
                        }
                        this.compose.submitting = false;
                        this.compose.creating = false;
                        this.compose.error = Some(e.to_string());
                        cx.notify();
                    });
                };
            let topic_id = match existing_topic_id {
                Some(id) => id,
                None => match api
                    .create_sd_topic(origin_message_id.get(), clan_id, parent_channel_id)
                    .await
                {
                    Ok(topic) => topic.id,
                    Err(e) => {
                        tracing::error!("submit_ephemeral_reply create_sd_topic failed: {e}");
                        fail(&this, cx, e);
                        return;
                    }
                },
            };
            let _ = this.update(cx, |this, cx| {
                this.finish_topic_create(origin_message_id);
                if this.compose_generation != generation {
                    return;
                }
                this.compose.active_topic_id = Some(topic_id);
                let creator_id = viewer_user_id(cx);
                MessagesStore::global(cx).update(cx, |store, cx| {
                    store.set_active_topic(Some(topic_id), cx);
                    store.ensure_topic_bucket(topic_id);
                    if existing_topic_id.is_none() {
                        store.mark_message_as_topic(
                            ChannelId(parent_channel_id),
                            origin_message_id,
                            topic_id,
                            creator_id,
                            cx,
                        );
                    }
                });
            });
            let proto_attachments =
                match crate::messages::upload_attachments_now(&api, attachments, parent_channel_id)
                    .await
                {
                    Ok(attachments) => attachments,
                    Err(e) => {
                        tracing::error!("submit_ephemeral_reply attachments failed: {e}");
                        fail(&this, cx, e);
                        return;
                    }
                };
            if let Err(e) = api
                .write_ephemeral_message(
                    receiver_id,
                    clan_id,
                    parent_channel_id,
                    &content,
                    is_public,
                    mode,
                    transport_mentions,
                    transport_hashtags,
                    transport_emojis,
                    proto_attachments,
                    reply_ref,
                    topic_id,
                )
                .await
            {
                tracing::error!("submit_ephemeral_reply failed: {e}");
                fail(&this, cx, e);
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.compose_generation != generation {
                    return;
                }
                this.compose.submitting = false;
                this.compose.creating = false;
                cx.emit(TopicsEvent::Updated);
                cx.emit(TopicsEvent::ReplySent);
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_topic_create(&mut self, origin_message_id: MessageId) -> bool {
        if self.creating_topic_for == Some(origin_message_id) {
            return false;
        }
        self.creating_topic_for = Some(origin_message_id);
        true
    }

    fn finish_topic_create(&mut self, origin_message_id: MessageId) {
        if self.creating_topic_for == Some(origin_message_id) {
            self.creating_topic_for = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_reply_sent(
        &mut self,
        topic_id: i64,
        parent_channel_id: i64,
        origin_message_id: MessageId,
        is_new_topic: bool,
        generation: u64,
        ack: mezon_client::transport::ApiMessage,
        anonymous: bool,
        local_sources: Vec<(String, Option<std::path::PathBuf>)>,
        cx: &mut Context<Self>,
    ) {
        if self.compose_generation != generation {
            return;
        }
        self.compose.submitting = false;
        self.compose.creating = false;
        self.compose.active_topic_id = Some(topic_id);
        let creator_id = viewer_user_id(cx);
        let append = MessagesStore::global(cx).update(cx, |store, cx| {
            store.set_active_topic(Some(topic_id), cx);
            let append = store.append_topic_message(
                topic_id,
                ack,
                anonymous,
                local_sources,
                is_new_topic,
                cx,
            );
            if is_new_topic {
                store.mark_message_as_topic(
                    ChannelId(parent_channel_id),
                    origin_message_id,
                    topic_id,
                    creator_id,
                    cx,
                );
            }
            append
        });
        if append.should_count_reply {
            self.increment_topic_reply_count(topic_id, append.create_time, cx);
        }
        cx.emit(TopicsEvent::Updated);
        cx.emit(TopicsEvent::ReplySent);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_reply_url_attachment(
        &mut self,
        url: String,
        filename: String,
        filetype: String,
        width: i32,
        height: i32,
        cx: &mut Context<Self>,
    ) {
        if url.is_empty() || self.compose.submitting {
            return;
        }
        let (Some(parent_channel_id), Some(clan_id), Some(origin_message_id)) = (
            self.compose.parent_channel_id,
            self.compose.clan_id,
            self.compose.origin_message_id,
        ) else {
            return;
        };
        let mode = self.compose.mode;
        let is_public = self.compose.is_public;
        let existing_topic_id = self.compose.active_topic_id;
        if existing_topic_id.is_none() && !self.begin_topic_create(origin_message_id) {
            return;
        }
        let anonymous = topic_anonymous_send(existing_topic_id, clan_id, cx);
        let reply_ref =
            self.reply_target
                .take()
                .map(|draft| mezon_client::transport::OutgoingReply {
                    message_ref_id: draft.message_ref_id.get(),
                    content: draft.content_raw,
                    has_attachment: draft.has_attachment,
                    message_sender_id: draft.sender_id.get(),
                    message_sender_username: draft.sender_name.clone(),
                    message_sender_avatar: draft.sender_avatar,
                    message_sender_clan_nick: String::new(),
                    message_sender_display_name: draft.sender_name,
                });

        self.compose.submitting = true;
        self.compose.creating = existing_topic_id.is_none();
        self.compose.error = None;
        cx.notify();

        let generation = self.compose_generation;
        let api = self.api.clone();
        let attachment = UrlAttachment {
            url,
            filename,
            filetype,
            width,
            height,
        };
        cx.spawn(async move |this, cx| {
            let topic_id = match existing_topic_id {
                Some(id) => id,
                None => match api
                    .create_sd_topic(origin_message_id.get(), clan_id, parent_channel_id)
                    .await
                {
                    Ok(topic) => topic.id,
                    Err(e) => {
                        tracing::error!("submit_reply_url_attachment create_sd_topic failed: {e}");
                        let _ = this.update(cx, |this, cx| {
                            this.finish_topic_create(origin_message_id);
                            if this.compose_generation != generation {
                                return;
                            }
                            this.compose.submitting = false;
                            this.compose.creating = false;
                            this.compose.error = Some(e.to_string());
                            cx.notify();
                        });
                        return;
                    }
                },
            };

            let ack = match api
                .send_topic_message_with_attachment_urls(
                    clan_id,
                    parent_channel_id,
                    is_public,
                    mode,
                    topic_id,
                    vec![attachment],
                    reply_ref,
                    OutgoingMessageFlags {
                        anonymous_message: anonymous,
                        message_code: 0,
                    },
                )
                .await
            {
                Ok(ack) => ack,
                Err(e) => {
                    tracing::error!("submit_reply_url_attachment send failed: {e}");
                    let _ = this.update(cx, |this, cx| {
                        this.finish_topic_create(origin_message_id);
                        if this.compose_generation != generation {
                            return;
                        }
                        this.compose.submitting = false;
                        this.compose.creating = false;
                        this.compose.error = Some(e.to_string());
                        cx.notify();
                    });
                    return;
                }
            };

            let _ = this.update(cx, |this, cx| {
                this.finish_topic_create(origin_message_id);
                this.apply_reply_sent(
                    topic_id,
                    parent_channel_id,
                    origin_message_id,
                    existing_topic_id.is_none(),
                    generation,
                    ack,
                    anonymous,
                    // A url attachment (Tenor, a pasted link) has no file on this
                    // disk to show.
                    Vec::new(),
                    cx,
                );
            });
        })
        .detach();
    }

    pub fn topics(&self) -> &[TopicDiscussion] {
        self.data.topics()
    }

    pub fn topic_by_id(&self, id: &str) -> Option<&TopicDiscussion> {
        self.data.topic_by_id(id)
    }

    pub fn topics_for(&self, clan_id: &str) -> &[TopicDiscussion] {
        if self.clan_id.as_deref() == Some(clan_id) {
            self.data.topics()
        } else {
            &[]
        }
    }

    pub fn latest_topic_for_channel(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Option<&TopicDiscussion> {
        self.topics_for(clan_id)
            .iter()
            .filter(|topic| topic.channel_id == channel_id)
            .max_by(|left, right| {
                left.last_message_timestamp
                    .cmp(&right.last_message_timestamp)
                    .then_with(|| left.id.cmp(&right.id))
            })
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_ready_for(&self, clan_id: &str) -> bool {
        self.clan_id.as_deref() == Some(clan_id)
            && (self.fetched_at.is_some() || self.fetch_failures >= MAX_TOPIC_FETCH_FAILURES)
    }

    pub fn has_more(&self) -> bool {
        self.has_more
    }

    fn is_fresh(&self, clan_id: &str) -> bool {
        self.clan_id.as_deref() == Some(clan_id)
            && self.fetched_at.is_some_and(|t| t.elapsed() < CACHE_TTL)
    }

    pub fn fetch_if_needed(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.is_fresh(clan_id)
            || (self.clan_id.as_deref() == Some(clan_id)
                && self.fetch_failures >= MAX_TOPIC_FETCH_FAILURES)
        {
            return;
        }
        self.fetch(clan_id, cx);
    }

    pub fn fetch(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.loading && self.clan_id.as_deref() == Some(clan_id) {
            return;
        }
        if self.clan_id.as_deref() != Some(clan_id) {
            self.inbox_message_avatars.clear();
            self.inbox_message_generation = self.inbox_message_generation.wrapping_add(1);
            self.data.clear_topics();
            self.clan_id = Some(clan_id.to_string());
            self.fetched_at = None;
            self.hydrated_preview_topic_id = None;
            self.fetch_failures = 0;
        }
        self.has_more = true;
        self.next_page = 1;
        self.fetch_page(clan_id, 1, false, cx);
    }

    pub fn fetch_more(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.loading || !self.has_more {
            return;
        }
        if self.clan_id.as_deref() != Some(clan_id) {
            return;
        }
        self.fetch_page(clan_id, self.next_page, true, cx);
    }

    fn fetch_page(&mut self, clan_id: &str, page: i32, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        let clan_id = clan_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = api.list_sd_topics(&clan_id, TOPICS_LIMIT, page).await;
            let _ = this.update(cx, |this, cx| {
                this.apply_fetch_result(&clan_id, generation, page, append, result, cx);
            });
        })
        .detach();
    }

    fn apply_fetch_result(
        &mut self,
        clan_id: &str,
        generation: u64,
        page: i32,
        append: bool,
        result: Result<Vec<TopicDiscussion>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.fetch_generation != generation {
            return;
        }
        match result {
            Ok(topics) => {
                self.fetch_failures = 0;
                self.has_more = topics.len() >= TOPICS_LIMIT as usize;
                self.next_page = page.saturating_add(1);
                if append {
                    self.data.upsert_topics(topics);
                } else {
                    self.inbox_message_avatars.clear();
                    self.inbox_message_generation = self.inbox_message_generation.wrapping_add(1);
                    self.data.set_topics(topics);
                }
                self.clan_id = Some(clan_id.to_string());
                self.fetched_at = Some(Instant::now());
                self.loading = false;
                cx.emit(TopicsEvent::Updated);
                cx.notify();
                self.hydrate_latest_topic_preview(cx);
            }
            Err(e) => {
                self.loading = false;
                tracing::error!("list_sd_topics failed: {e}");
                self.fetch_failures = self.fetch_failures.saturating_add(1);
                if self.fetch_failures >= MAX_TOPIC_FETCH_FAILURES {
                    self.has_more = false;
                }
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
        }
    }

    pub fn inbox_message_avatar(&self, topic: &TopicDiscussion) -> Option<(&str, &str)> {
        if self.clan_id.as_deref() != Some(topic.clan_id.as_str()) {
            return None;
        }
        match self.inbox_message_avatars.get(&inbox_message_key(topic)?)? {
            InboxMessageAvatar::Ready(name, avatar) => Some((name, avatar)),
            _ => None,
        }
    }

    pub fn inbox_avatar<'a>(
        &'a self,
        topic: &TopicDiscussion,
        cx: &'a App,
    ) -> Option<(&'a str, &'a str)> {
        if self.clan_id.as_deref() != Some(topic.clan_id.as_str()) {
            return None;
        }
        inbox_member_avatar(topic, cx).or_else(|| self.inbox_message_avatar(topic))
    }

    fn apply_inbox_message_avatar(
        &mut self,
        generation: u64,
        key: (ChannelId, MessageId),
        state: InboxMessageAvatar,
    ) -> bool {
        if self.inbox_message_generation != generation
            || self.inbox_message_avatars.get(&key) != Some(&InboxMessageAvatar::Loading)
        {
            return false;
        }
        self.inbox_message_avatars.insert(key, state);
        true
    }

    fn refresh_inbox_message_avatars(&mut self, event: &MessagesEvent, cx: &mut Context<Self>) {
        let Some(messages) = MessagesStore::try_global(cx) else {
            return;
        };
        let messages = messages.read(cx);
        let mut changed = false;
        for (key, state) in &mut self.inbox_message_avatars {
            match event {
                MessagesEvent::Updated {
                    message_id: Some(id),
                } if *id != key.1 => continue,
                MessagesEvent::TopicUpdated { topic_id } if *topic_id != key.0.get() => continue,
                _ => {}
            }
            if let Some(message) = messages.message_in_channel(key.0, key.1) {
                let avatar = if message.is_deleted_placeholder {
                    ""
                } else {
                    message.avatar_url.as_ref()
                };
                changed |= state.update_identity(&message.sender_name, avatar);
            }
        }
        if changed {
            self.schedule_updated_notify(cx);
        }
    }

    pub fn retry_inbox_message_avatars(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.clan_id.as_deref() == Some(clan_id) {
            self.inbox_message_avatars
                .retain(|_, state| *state != InboxMessageAvatar::Failed);
        }
        self.load_inbox_message_avatars(clan_id, cx);
    }

    pub fn load_inbox_message_avatars(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.clan_id.as_deref() != Some(clan_id) {
            return;
        }
        let Ok(clan) = clan_id.parse::<ClanId>() else {
            return;
        };
        if clan.get() <= 0 {
            return;
        }
        let keys: Vec<_> = self
            .data
            .topics()
            .iter()
            .filter_map(|topic| {
                Some((
                    inbox_message_key(topic)?,
                    topic.channel_id.parse::<ChannelId>().ok()?,
                    inbox_member_avatar(topic, cx).is_some(),
                ))
            })
            .collect();
        let mut missing = Vec::new();
        let active_keys: std::collections::HashSet<_> =
            keys.iter().map(|(key, _, _)| *key).collect();
        self.inbox_message_avatars
            .retain(|key, _| active_keys.contains(key));
        for (key, parent, has_member_avatar) in keys {
            if has_member_avatar {
                self.inbox_message_avatars.remove(&key);
                continue;
            }
            if self.inbox_message_avatars.contains_key(&key) {
                continue;
            }
            self.inbox_message_avatars
                .insert(key, InboxMessageAvatar::Loading);
            missing.push((key, parent));
        }
        if missing.is_empty() {
            return;
        }
        self.refresh_inbox_message_avatars(&MessagesEvent::Updated { message_id: None }, cx);
        let generation = self.inbox_message_generation;
        let permits = self.inbox_avatar_permits.clone();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request_entity = this.clone();
            let request_cx = cx.clone();
            let requests = stream::iter(missing.into_iter().map(move |(key, parent)| {
                let this = request_entity.clone();
                let cx = request_cx.clone();
                let api = api.clone();
                let permits = permits.clone();
                async move {
                    let _permit = permits.acquire_owned().await.ok()?;
                    let loading = this.read_with(&cx, |store, _| {
                        store.inbox_message_generation == generation
                            && store.inbox_message_avatars.get(&key)
                                == Some(&InboxMessageAvatar::Loading)
                    });
                    if !matches!(loading, Ok(true)) {
                        return None;
                    }
                    let page = if key.0 == parent {
                        api.list_channel_messages(
                            clan.get(),
                            parent.get(),
                            key.1.get(),
                            INBOX_MESSAGE_DIRECTION_AROUND,
                            INBOX_AVATAR_LOOKUP_LIMIT,
                        )
                        .await
                    } else {
                        api.list_topic_messages(
                            clan.get(),
                            parent.get(),
                            key.0.get(),
                            key.1.get(),
                            INBOX_MESSAGE_DIRECTION_AROUND,
                            INBOX_AVATAR_LOOKUP_LIMIT,
                        )
                        .await
                    };
                    Some((key, page))
                }
            }))
            .buffer_unordered(INBOX_AVATAR_LOOKUP_CONCURRENCY);
            futures::pin_mut!(requests);
            while let Some(result) = requests.next().await {
                let Some((key, page)) = result else {
                    continue;
                };
                let state = match page {
                    Ok(page) => message_avatar_from_messages(key.1, &page.messages),
                    Err(error) => {
                        tracing::warn!(
                            message_id = key.1.get(),
                            %error,
                            "failed to load inbox topic message avatar"
                        );
                        InboxMessageAvatar::Failed
                    }
                };
                let _ = this.update(cx, |store, cx| {
                    if store.apply_inbox_message_avatar(generation, key, state) {
                        store.schedule_updated_notify(cx);
                    }
                });
            }
        })
        .detach();
    }

    /// The topic listing is sufficient for ordering, but its `last_sent_message` is not
    /// consistently the final reply. Resolve the winner once pagination is complete and use the
    /// actual newest message for the compact clan activity preview.
    fn hydrate_latest_topic_preview(&mut self, cx: &mut Context<Self>) {
        let active_channel_id = MessagesStore::try_global(cx)
            .and_then(|store| store.read(cx).active_channel_id())
            .map(|channel_id| channel_id.to_string());
        self.hydrate_latest_topic_preview_for_channel(active_channel_id.as_deref(), cx);
    }

    pub fn hydrate_latest_topic_preview_for_channel(
        &mut self,
        channel_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(topic) = channel_id
            .and_then(|channel_id| {
                let clan_id = self.clan_id.as_deref()?;
                self.latest_topic_for_channel(clan_id, channel_id)
            })
            .or_else(|| self.data.topics().first())
            .cloned()
        else {
            return;
        };
        if self.hydrated_preview_topic_id.as_deref() == Some(topic.id.as_str()) {
            return;
        }
        let (Ok(clan_id), Ok(channel_id), Ok(topic_id)) = (
            topic.clan_id.parse::<i64>(),
            topic.channel_id.parse::<i64>(),
            topic.id.parse::<i64>(),
        ) else {
            return;
        };
        self.hydrated_preview_topic_id = Some(topic.id.clone());
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        let topic_key = topic.id;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_topic_messages(clan_id, channel_id, topic_id, 0, 0, TOPICS_LIMIT as u32)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.preview_generation != generation
                    || this.hydrated_preview_topic_id.as_deref() != Some(topic_key.as_str())
                {
                    return;
                }
                match result {
                    Ok(page) => {
                        if let Some(message) = page
                            .messages
                            .iter()
                            .filter(|message| message.code != 2)
                            .max_by_key(|message| normalize_unix_seconds(message.create_time))
                        {
                            this.data.note_api_topic_message(&topic_key, message);
                        }
                        cx.emit(TopicsEvent::Updated);
                        cx.notify();
                    }
                    Err(error) => {
                        tracing::warn!("failed to hydrate latest topic {topic_key}: {error}");
                        this.hydrated_preview_topic_id = None;
                    }
                }
            });
        })
        .detach();
    }
}

fn update_topic_message_sender(topic: &mut TopicDiscussion, message_id: i64, sender_id: i64) {
    let message_id = message_id.to_string();
    if sender_id > 0 {
        topic.last_sender_id = sender_id.to_string();
        topic.last_sender_is_authoritative = true;
    } else if topic.last_message_id != message_id {
        topic.last_sender_id.clear();
        topic.last_sender_is_authoritative = false;
    }
    topic.last_message_id = message_id;
}

fn inbox_message_key(topic: &TopicDiscussion) -> Option<(ChannelId, MessageId)> {
    let last = topic
        .last_message_id
        .parse::<MessageId>()
        .ok()
        .filter(|id| id.get() > 0);
    let (channel, message) = if let Some(last) = last {
        if topic.last_message_id == topic.message_id {
            (topic.channel_id.parse::<ChannelId>().ok()?, last)
        } else {
            (topic.id.parse::<ChannelId>().ok()?, last)
        }
    } else {
        (
            topic.channel_id.parse::<ChannelId>().ok()?,
            topic.message_id.parse::<MessageId>().ok()?,
        )
    };
    (channel.get() > 0 && message.get() > 0).then_some((channel, message))
}

fn inbox_member_avatar<'a>(topic: &TopicDiscussion, cx: &'a App) -> Option<(&'a str, &'a str)> {
    let clan = topic.clan_id.parse::<ClanId>().ok()?;
    let sender = if topic.last_sender_is_authoritative
        && topic
            .last_message_id
            .parse::<MessageId>()
            .is_ok_and(|id| id.get() > 0)
    {
        topic.last_sender_id.parse::<UserId>().ok()?
    } else {
        let key = inbox_message_key(topic)?;
        MessagesStore::try_global(cx)?
            .read(cx)
            .message_in_channel(key.0, key.1)?
            .sender_id
            .parse::<UserId>()
            .ok()?
    };
    if clan.get() <= 0 || sender.get() <= 0 {
        return None;
    }
    let member = crate::ClanMembersStore::try_global(cx)?
        .read(cx)
        .member(clan, sender)?;
    (!member.avatar().is_empty()).then(|| (member.name(), member.avatar()))
}

fn message_avatar_from_messages(
    original_id: MessageId,
    messages: &[mezon_client::transport::ApiMessage],
) -> InboxMessageAvatar {
    messages
        .iter()
        .find(|message| {
            message.message_id == original_id.get()
                && message.code != 2
                && !message.avatar.is_empty()
        })
        .map(|message| {
            InboxMessageAvatar::Ready(message.sender_name.clone(), message.avatar.clone())
        })
        .unwrap_or(InboxMessageAvatar::Missing)
}

fn sd_topic_event_last_sent_seconds(ev: &realtime::SdTopicEvent, now_seconds: i64) -> i64 {
    normalize_unix_seconds(
        ev.last_sent_message
            .as_ref()
            .map(|h| i64::from(h.timestamp_seconds))
            .filter(|t| *t > 0)
            .unwrap_or(now_seconds),
    )
}

fn panel_should_close_on_message_deleted(
    panel_open: bool,
    compose: &TopicCompose,
    data: &TopicsData,
    deleted_in: ChannelId,
    message_id: MessageId,
    message_topic_id: Option<ChannelId>,
) -> bool {
    if !panel_open {
        return false;
    }
    if compose.active_topic_id == Some(deleted_in.get()) {
        return false;
    }
    if compose.origin_message_id == Some(message_id) {
        return true;
    }
    let (Some(message_topic_id), Some(active_topic_id)) =
        (message_topic_id, compose.active_topic_id)
    else {
        return false;
    };
    message_topic_id.get() == active_topic_id
        && data
            .topic_by_id(&active_topic_id.to_string())
            .is_some_and(|topic| topic.message_id == message_id.get().to_string())
}

fn sd_topic_from_event(ev: &realtime::SdTopicEvent) -> api::SdTopic {
    let content = ev
        .message
        .as_ref()
        .map(|m| m.content.clone())
        .filter(|c| !c.is_empty())
        .or_else(|| {
            ev.last_sent_message
                .as_ref()
                .map(|h| h.content.clone())
                .filter(|c| !c.is_empty())
        })
        .unwrap_or_default();
    let create_time_seconds = ev
        .message
        .as_ref()
        .map(|m| m.create_time_seconds)
        .filter(|t| *t != 0)
        .unwrap_or(0);
    api::SdTopic {
        id: ev.id,
        creator_id: ev.user_id,
        message_id: ev.message_id,
        clan_id: ev.clan_id,
        channel_id: ev.channel_id,
        status: 0,
        create_time_seconds,
        update_time_seconds: ev
            .last_sent_message
            .as_ref()
            .map(|h| h.timestamp_seconds)
            .unwrap_or(0),
        content,
        last_sent_message: ev.last_sent_message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageCode;

    fn inbox_test_stores(cx: &mut App) -> (Entity<TopicsStore>, Entity<MessagesStore>) {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        crate::clan::ClanList::init(api.clone(), cx);
        crate::channel::ChannelList::init(api.clone(), cx);
        crate::account::AccountStore::init(api.clone(), cx);
        crate::ClanMembersStore::init(api.clone(), cx);
        let messages = MessagesStore::init(api.clone(), cx);
        messages.update(cx, |store, _| store.seed_active_clan_for_test(ClanId(9)));
        let topics = TopicsStore::init(api, cx);
        topics.update(cx, |store, _| store.clan_id = Some("9".into()));
        (topics, messages)
    }

    #[gpui::test]
    fn inbox_member_first_loader_needs_no_message_lookup_for_loaded_members(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (topics, _) = inbox_test_stores(cx);
            crate::ClanMembersStore::global(cx).update(cx, |store, _| {
                store.seed_members_for_test(
                    ClanId(9),
                    vec![crate::ClanMember {
                        user: crate::clan_members::User {
                            id: UserId(8),
                            username: "global".into(),
                            avatar_url: "global.png".into(),
                            ..Default::default()
                        },
                        clan_nick: "clan nick".into(),
                        clan_avatar: "clan.png".into(),
                        ..Default::default()
                    }],
                );
            });
            topics.update(cx, |store, cx| {
                let mut discussion = topic(10, 42, "8", "body", 100);
                discussion.last_message_id = "500".into();
                discussion.last_sender_is_authoritative = true;
                store.data.set_topics(
                    (10..60)
                        .map(|id| {
                            let mut row = discussion.clone();
                            row.id = id.to_string();
                            row.last_message_id = (id + 490).to_string();
                            row
                        })
                        .collect(),
                );
                store.load_inbox_message_avatars("9", cx);
                assert!(store.inbox_message_avatars.is_empty());
                assert_eq!(
                    store.inbox_avatar(&discussion, cx),
                    Some(("clan nick", "clan.png"))
                );
                discussion.last_sender_is_authoritative = false;
                assert_eq!(store.inbox_avatar(&discussion, cx), None);
                discussion.last_sender_is_authoritative = true;
                discussion.last_sender_id = "7".into();
                store.data.set_topics(vec![discussion.clone()]);
                store.load_inbox_message_avatars("9", cx);
                assert_eq!(
                    store
                        .inbox_message_avatars
                        .get(&(ChannelId(10), MessageId(500))),
                    Some(&InboxMessageAvatar::Loading)
                );
                store.reset(cx);
            });
        });
    }

    #[gpui::test]
    fn inbox_realtime_handler_matches_api_identity_and_preserves_sparse_updates(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (topics, _) = inbox_test_stores(cx);
            topics.update(cx, |store, cx| {
                store.data.set_topics(vec![
                    topic(10, 42, "3", "original", 100),
                    topic(11, 43, "7", "other", 150),
                ]);
                for (id, clan_avatar, avatar, clan_nick, display_name) in [
                    (500, "clan.png", "global.png", "clan nick", "display"),
                    (501, "clan-only.png", "", "", "display"),
                    (502, "", "global.png", "", ""),
                ] {
                    let message = api::ChannelMessage {
                        clan_id: 9,
                        channel_id: 5,
                        topic_id: 10,
                        message_id: id,
                        sender_id: 8,
                        username: "global name".into(),
                        clan_nick: clan_nick.into(),
                        display_name: display_name.into(),
                        clan_avatar: clan_avatar.into(),
                        avatar: avatar.into(),
                        content: "reply".into(),
                        create_time_seconds: 200,
                        ..Default::default()
                    };
                    let api_identity = message_avatar_from_messages(
                        MessageId(id),
                        &[mezon_client::MezonTransport::message_from_proto(&message)],
                    );
                    store.handle_channel_message(
                        &RealtimeEvent::ChannelMessage(message.clone()),
                        cx,
                    );
                    let key = (ChannelId(10), MessageId(id));
                    assert_eq!(store.inbox_message_avatars.get(&key), Some(&api_identity));
                    assert_eq!(store.data.topics()[0].id, "10");
                    assert_eq!(store.data.topics()[0].last_message_content, "reply");
                    let before = store.inbox_message_avatar(&store.data.topics()[0]).unwrap();
                    let pointers = (before.0.as_ptr(), before.1.as_ptr());
                    store.handle_channel_message(
                        &RealtimeEvent::ChannelMessage(message.clone()),
                        cx,
                    );
                    let after = store.inbox_message_avatar(&store.data.topics()[0]).unwrap();
                    assert_eq!((after.0.as_ptr(), after.1.as_ptr()), pointers);
                    let partial = api::ChannelMessage {
                        avatar: String::new(),
                        clan_avatar: String::new(),
                        code: 1,
                        ..message
                    };
                    store.handle_channel_message(&RealtimeEvent::ChannelMessage(partial), cx);
                    assert_eq!(store.inbox_message_avatars.get(&key), Some(&api_identity));
                    assert!(!store.apply_inbox_message_avatar(
                        store.inbox_message_generation,
                        key,
                        InboxMessageAvatar::Missing
                    ));
                }
                assert_eq!(store.inbox_message_avatars.len(), 1);
            });
        });
    }

    #[test]
    fn inbox_avatar_identity_does_not_replace_unchanged_strings() {
        let mut state = InboxMessageAvatar::Ready("name".into(), "avatar.png".into());
        let InboxMessageAvatar::Ready(name, avatar) = &state else {
            panic!()
        };
        let pointers = (name.as_ptr(), avatar.as_ptr());
        assert!(!state.update_identity("name", "avatar.png"));
        let InboxMessageAvatar::Ready(name, avatar) = &state else {
            panic!()
        };
        assert_eq!((name.as_ptr(), avatar.as_ptr()), pointers);
        assert!(state.update_identity("changed", "avatar.png"));
        assert!(state.update_identity("changed", ""));
        assert!(!state.update_identity("changed", ""));
    }

    #[gpui::test]
    fn inbox_sender_realtime_unknown_new_message_cannot_reuse_previous_member(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (topics, _) = inbox_test_stores(cx);
            crate::ClanMembersStore::global(cx).update(cx, |store, _| {
                store.seed_members_for_test(
                    ClanId(9),
                    vec![crate::ClanMember {
                        user: crate::clan_members::User {
                            id: UserId(8),
                            username: "member A".into(),
                            avatar_url: "member.png".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                );
            });
            topics.update(cx, |store, cx| {
                store
                    .data
                    .set_topics(vec![topic(10, 42, "3", "original", 100)]);
                let known = api::ChannelMessage {
                    clan_id: 9,
                    channel_id: 5,
                    topic_id: 10,
                    message_id: 500,
                    sender_id: 8,
                    username: "member A".into(),
                    avatar: "old.png".into(),
                    content: "reply".into(),
                    create_time_seconds: 200,
                    ..Default::default()
                };
                store.handle_channel_message(&RealtimeEvent::ChannelMessage(known.clone()), cx);
                assert_eq!(
                    store.inbox_avatar(&store.data.topics()[0], cx),
                    Some(("member A", "member.png"))
                );
                let partial = api::ChannelMessage {
                    sender_id: 0,
                    avatar: String::new(),
                    code: 1,
                    ..known.clone()
                };
                store.handle_channel_message(&RealtimeEvent::ChannelMessage(partial), cx);
                assert!(store.data.topics()[0].last_sender_is_authoritative);
                assert_eq!(
                    store.inbox_avatar(&store.data.topics()[0], cx),
                    Some(("member A", "member.png"))
                );
                let unknown = api::ChannelMessage {
                    message_id: 501,
                    sender_id: 0,
                    username: "unknown".into(),
                    avatar: "new.png".into(),
                    ..known.clone()
                };
                store.handle_channel_message(&RealtimeEvent::ChannelMessage(unknown), cx);
                assert!(!store.data.topics()[0].last_sender_is_authoritative);
                assert!(store.data.topics()[0].last_sender_id.is_empty());
                assert_eq!(
                    store.inbox_avatar(&store.data.topics()[0], cx),
                    Some(("unknown", "new.png"))
                );
                let empty = api::ChannelMessage {
                    message_id: 502,
                    sender_id: 0,
                    avatar: String::new(),
                    ..known
                };
                store.handle_channel_message(&RealtimeEvent::ChannelMessage(empty), cx);
                assert_eq!(store.inbox_avatar(&store.data.topics()[0], cx), None);
            });
        });
    }

    #[test]
    fn inbox_sender_api_updates_preserve_same_message_and_invalidate_new_identity() {
        let mut data = TopicsData::default();
        data.set_topics(vec![topic(10, 42, "3", "original", 100)]);
        assert!(data.note_api_topic_message("10", &original_test_message(500, 8, "old.png", 0)));
        assert!(data.note_api_topic_message("10", &original_test_message(500, 0, "", 1)));
        assert_eq!(data.topics()[0].last_sender_id, "8");
        assert!(data.topics()[0].last_sender_is_authoritative);
        assert!(data.note_api_topic_message("10", &original_test_message(501, 0, "new.png", 0)));
        assert_eq!(data.topics()[0].last_message_id, "501");
        assert!(!data.topics()[0].last_sender_is_authoritative);
        assert!(data.topics()[0].last_sender_id.is_empty());
    }

    #[test]
    fn inbox_sender_snapshot_cannot_attach_old_actor_to_new_message() {
        let mut data = TopicsData::default();
        let mut known = topic(10, 42, "8", "original", 100);
        known.last_message_id = "500".into();
        known.last_sender_is_authoritative = true;
        data.set_topics(vec![known.clone()]);
        let mut sparse = known.clone();
        sparse.last_sender_id = "3".into();
        sparse.last_sender_is_authoritative = false;
        data.upsert_topic(sparse.clone());
        assert_eq!(data.topics()[0].last_sender_id, "8");
        assert!(data.topics()[0].last_sender_is_authoritative);
        sparse.last_message_id = "501".into();
        data.upsert_topic(sparse);
        assert_eq!(data.topics()[0].last_message_id, "501");
        assert!(!data.topics()[0].last_sender_is_authoritative);
        assert!(data.topics()[0].last_sender_id.is_empty());
        known.last_message_id = "502".into();
        known.last_sender_id = "9".into();
        data.upsert_topic(known);
        assert_eq!(data.topics()[0].last_sender_id, "9");
        assert!(data.topics()[0].last_sender_is_authoritative);
    }

    #[gpui::test]
    fn inbox_avatar_lookup_budget_is_shared_across_context_resets(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (topics, _) = inbox_test_stores(cx);
            topics.update(cx, |store, cx| {
                let mut held = Vec::new();
                for _ in 0..INBOX_AVATAR_LOOKUP_CONCURRENCY {
                    held.push(
                        store
                            .inbox_avatar_permits
                            .clone()
                            .try_acquire_owned()
                            .unwrap(),
                    );
                }
                assert!(
                    store
                        .inbox_avatar_permits
                        .clone()
                        .try_acquire_owned()
                        .is_err()
                );
                store.reset(cx);
                assert!(
                    store
                        .inbox_avatar_permits
                        .clone()
                        .try_acquire_owned()
                        .is_err()
                );
                held.pop();
                assert!(
                    store
                        .inbox_avatar_permits
                        .clone()
                        .try_acquire_owned()
                        .is_ok()
                );
            });
        });
    }

    #[gpui::test]
    fn inbox_avatar_refresh_scopes_message_events_and_preserves_unchanged_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (topics, messages) = inbox_test_stores(cx);
            messages.update(cx, |store, cx| {
                store.append_topic_message(
                    10,
                    original_test_message(500, 8, "live.png", 0),
                    false,
                    vec![],
                    false,
                    cx,
                );
            });
            topics.update(cx, |store, cx| {
                let key = (ChannelId(10), MessageId(500));
                store.inbox_message_avatars.insert(
                    key,
                    InboxMessageAvatar::Ready("old".into(), "old.png".into()),
                );
                store.refresh_inbox_message_avatars(
                    &MessagesEvent::Updated {
                        message_id: Some(MessageId(501)),
                    },
                    cx,
                );
                assert_eq!(
                    store.inbox_message_avatars.get(&key),
                    Some(&InboxMessageAvatar::Ready("old".into(), "old.png".into()))
                );
                assert!(store.updated_notify_task.is_none());
                store.refresh_inbox_message_avatars(
                    &MessagesEvent::Updated {
                        message_id: Some(MessageId(500)),
                    },
                    cx,
                );
                let InboxMessageAvatar::Ready(name, avatar) = &store.inbox_message_avatars[&key]
                else {
                    panic!()
                };
                assert_eq!(avatar, "live.png");
                let pointers = (name.as_ptr(), avatar.as_ptr());
                store.refresh_inbox_message_avatars(&MessagesEvent::Reset { count: 1 }, cx);
                let InboxMessageAvatar::Ready(name, avatar) = &store.inbox_message_avatars[&key]
                else {
                    panic!()
                };
                assert_eq!((name.as_ptr(), avatar.as_ptr()), pointers);
            });
        });
    }

    fn original_test_message(
        id: i64,
        sender: i64,
        avatar: &str,
        code: i32,
    ) -> mezon_client::transport::ApiMessage {
        mezon_client::MezonTransport::message_from_proto(&api::ChannelMessage {
            message_id: id,
            sender_id: sender,
            username: format!("sender-{sender}"),
            avatar: avatar.into(),
            code,
            ..Default::default()
        })
    }

    #[test]
    fn inbox_message_avatar_matches_message_not_creator_or_last_sender() {
        let messages = vec![
            original_test_message(40, 3, "creator.png", 0),
            original_test_message(42, 8, "original.png", 0),
            original_test_message(43, 9, "last.png", 0),
        ];
        assert_eq!(
            message_avatar_from_messages(MessageId(42), &messages),
            InboxMessageAvatar::Ready("sender-8".into(), "original.png".into())
        );
        assert_eq!(
            message_avatar_from_messages(MessageId(41), &messages),
            InboxMessageAvatar::Missing
        );
    }

    #[test]
    fn inbox_message_avatar_missing_deleted_and_empty_use_fallback() {
        assert_eq!(
            message_avatar_from_messages(MessageId(42), &[]),
            InboxMessageAvatar::Missing
        );
        for message in [
            original_test_message(42, 8, "", 0),
            original_test_message(42, 8, "deleted.png", 2),
        ] {
            assert_eq!(
                message_avatar_from_messages(MessageId(42), &[message]),
                InboxMessageAvatar::Missing
            );
        }
    }

    #[test]
    fn inbox_message_key_rejects_invalid_ids_and_uses_parent_channel() {
        let original = topic(10, 42, "9", "body", 100);
        assert_eq!(
            inbox_message_key(&original),
            Some((ChannelId(5), MessageId(42)))
        );
        for invalid in ["", "0", "-1", "invalid"] {
            let mut value = original.clone();
            value.message_id = invalid.into();
            assert_eq!(inbox_message_key(&value), None);
            value = original.clone();
            value.channel_id = invalid.into();
            assert_eq!(inbox_message_key(&value), None);
        }
    }

    #[gpui::test]
    fn inbox_message_avatar_store_preserves_topics_and_rejects_stale_results(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let api = Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            let store = TopicsStore::init(api, cx);
            store.update(cx, |store, cx| {
                store.clan_id = Some("9".into());
                let original = topic(10, 42, "9", "original body", 100);
                store
                    .data
                    .set_topics(vec![original.clone(), topic(11, 43, "3", "other", 200)]);
                let before = store.data.topics().to_vec();
                let key = inbox_message_key(&original).unwrap();
                let generation = store.inbox_message_generation;
                store
                    .inbox_message_avatars
                    .insert(key, InboxMessageAvatar::Loading);
                assert!(store.apply_inbox_message_avatar(
                    generation,
                    key,
                    InboxMessageAvatar::Ready("original-author".into(), "original.png".into())
                ));
                assert_eq!(
                    store.inbox_message_avatar(&original),
                    Some(("original-author", "original.png"))
                );
                assert!(!store.apply_inbox_message_avatar(
                    generation,
                    key,
                    InboxMessageAvatar::Missing
                ));
                for (clan, channel, message) in
                    [("8", "5", "42"), ("9", "6", "42"), ("9", "5", "43")]
                {
                    let mut other = original.clone();
                    other.clan_id = clan.into();
                    other.channel_id = channel.into();
                    other.message_id = message.into();
                    assert_eq!(store.inbox_message_avatar(&other), None);
                }
                assert_eq!(store.data.topics(), before.as_slice());
                store.reset(cx);
                store
                    .inbox_message_avatars
                    .insert(key, InboxMessageAvatar::Loading);
                assert!(!store.apply_inbox_message_avatar(
                    generation,
                    key,
                    InboxMessageAvatar::Ready("stale".into(), "stale.png".into())
                ));
            });
        });
    }

    #[gpui::test]
    fn inbox_message_retry_preserves_other_clan_and_prunes_unused_entries(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let api = Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            let store = TopicsStore::init(api, cx);
            store.update(cx, |store, cx| {
                store.clan_id = Some("9".into());
                for (id, state) in [
                    (40, InboxMessageAvatar::Failed),
                    (41, InboxMessageAvatar::Missing),
                    (42, InboxMessageAvatar::Loading),
                    (
                        43,
                        InboxMessageAvatar::Ready("name".into(), "avatar.png".into()),
                    ),
                ] {
                    store
                        .inbox_message_avatars
                        .insert((ChannelId(5), MessageId(id)), state);
                }
                store.retry_inbox_message_avatars("8", cx);
                assert_eq!(store.inbox_message_avatars.len(), 4);
                store.retry_inbox_message_avatars("9", cx);
                assert!(store.inbox_message_avatars.is_empty());
            });
        });
    }

    #[gpui::test]
    fn inbox_message_source_event_recovers_missing_avatar_without_topic_mutation(
        cx: &mut gpui::TestAppContext,
    ) {
        let topics = cx.update(|cx| {
            let api = Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            crate::clan::ClanList::init(api.clone(), cx);
            crate::channel::ChannelList::init(api.clone(), cx);
            crate::account::AccountStore::init(api.clone(), cx);
            let messages = MessagesStore::init(api.clone(), cx);
            let topics = TopicsStore::init(api, cx);
            topics.update(cx, |store, _| {
                store.clan_id = Some("9".into());
                store.data.set_topics(vec![topic(10, 42, "9", "body", 100)]);
                store
                    .inbox_message_avatars
                    .insert((ChannelId(5), MessageId(42)), InboxMessageAvatar::Missing);
            });
            messages.update(cx, |store, cx| {
                store.append_topic_message(
                    5,
                    original_test_message(42, 8, "late-original.png", 0),
                    false,
                    vec![],
                    false,
                    cx,
                );
                cx.emit(MessagesEvent::Reset { count: 1 });
            });
            topics
        });
        cx.update(|cx| {
            topics.update(cx, |store, _| {
                assert_eq!(
                    store.inbox_message_avatar(&topic(10, 42, "9", "body", 100)),
                    Some(("sender-8", "late-original.png"))
                );
                assert_eq!(store.data.topics(), &[topic(10, 42, "9", "body", 100)]);
                assert!(!store.apply_inbox_message_avatar(
                    store.inbox_message_generation,
                    (ChannelId(5), MessageId(42)),
                    InboxMessageAvatar::Missing
                ));
            });
        });
    }

    #[gpui::test]
    fn inbox_message_avatar_switches_from_original_to_latest_reply(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let api = Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            let store = TopicsStore::init(api, cx);
            store.update(cx, |store, _| {
                store.clan_id = Some("9".into());
                store
                    .data
                    .set_topics(vec![topic(10, 42, "8", "original", 100)]);
                store.inbox_message_avatars.insert(
                    (ChannelId(5), MessageId(42)),
                    InboxMessageAvatar::Ready("original".into(), "original.png".into()),
                );
                assert_eq!(
                    store.inbox_message_avatar(&store.data.topics()[0]),
                    Some(("original", "original.png"))
                );
                for (id, sender, avatar) in [(500, 9, "reply.png"), (501, 7, "")] {
                    let message = api::ChannelMessage {
                        message_id: id,
                        topic_id: 10,
                        sender_id: sender,
                        content: "latest reply".into(),
                        create_time_seconds: 200,
                        avatar: avatar.into(),
                        username: "replier".into(),
                        ..Default::default()
                    };
                    assert!(store.data.note_topic_message(&message));
                    let discussion = store.data.topics()[0].clone();
                    let key = inbox_message_key(&discussion).unwrap();
                    store
                        .inbox_message_avatars
                        .insert(key, InboxMessageAvatar::Loading);
                    assert!(store.apply_inbox_message_avatar(
                        store.inbox_message_generation,
                        key,
                        message_avatar_from_messages(
                            MessageId(id),
                            &[original_test_message(id, sender, avatar, 0)]
                        )
                    ));
                    if avatar.is_empty() {
                        assert_eq!(store.inbox_message_avatar(&discussion), None);
                    } else {
                        assert_eq!(
                            store.inbox_message_avatar(&discussion),
                            Some(("sender-9", "reply.png"))
                        );
                    }
                    assert_eq!(discussion.last_message_content, "latest reply");
                    assert_eq!(discussion.content, "original");
                }
            });
        });
    }

    #[gpui::test]
    fn anonymous_send_requires_an_existing_topic(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let api = Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            crate::clan::ClanList::init(api.clone(), cx);
            crate::channel::ChannelList::init(api.clone(), cx);
            crate::account::AccountStore::init(api.clone(), cx);
            let messages = MessagesStore::init(api, cx);
            messages.update(cx, |store, cx| {
                store.set_active_topic(Some(77), cx);
                store.toggle_anonymous_mode(true, cx);
            });

            assert!(!topic_anonymous_send(None, 9, cx));
            assert!(topic_anonymous_send(Some(77), 9, cx));
        });
    }

    fn pending_jump(requested_at: Instant) -> PendingInboxTopicJump {
        PendingInboxTopicJump {
            topic_id: 99,
            origin_id: MessageId(7),
            reply_id: MessageId(8),
            channel_id: ChannelId(3),
            request_id: 42,
            requested_at,
        }
    }

    #[test]
    fn a_pending_inbox_jump_only_opens_its_own_origin() {
        let now = Instant::now();
        let pending = pending_jump(now);
        assert!(pending.matches(MessageId(7), ChannelId(3), Some(42), now));
        assert!(!pending.matches(MessageId(8), ChannelId(3), Some(42), now));
        assert!(!pending.matches(MessageId(7), ChannelId(3), Some(99), now));
        assert!(!pending.matches(MessageId(7), ChannelId(3), None, now));
    }

    #[test]
    fn a_stale_pending_inbox_jump_is_dropped_instead_of_hijacking_a_later_jump() {
        let requested_at = Instant::now();
        let pending = pending_jump(requested_at);
        let later = requested_at + INBOX_TOPIC_JUMP_TIMEOUT + Duration::from_secs(1);
        assert!(pending.is_expired(later));
        assert!(!pending.matches(MessageId(7), ChannelId(3), Some(42), later));
    }

    fn sample_message(code: MessageCode) -> Message {
        Message::new(MessageId(1), "hello", "1", "user", 0).with_code(code)
    }

    fn header(
        id: i64,
        timestamp_seconds: u32,
        sender_id: i64,
        content: &str,
    ) -> api::ChannelMessageHeader {
        api::ChannelMessageHeader {
            id,
            timestamp_seconds,
            sender_id,
            content: content.to_string(),
        }
    }

    fn channel_message(content: &str, create_time_seconds: u32) -> api::ChannelMessage {
        api::ChannelMessage {
            content: content.to_string(),
            create_time_seconds,
            ..Default::default()
        }
    }

    fn sd_topic_event(
        last_sent_message: Option<api::ChannelMessageHeader>,
        message: Option<api::ChannelMessage>,
    ) -> realtime::SdTopicEvent {
        realtime::SdTopicEvent {
            id: 77,
            clan_id: 9,
            channel_id: 5,
            message_id: 42,
            user_id: 3,
            last_sent_message,
            message,
        }
    }

    fn topic(
        id: i64,
        message_id: i64,
        last_sender_id: &str,
        content: &str,
        last_message_timestamp: u32,
    ) -> TopicDiscussion {
        TopicDiscussion {
            id: id.to_string(),
            message_id: message_id.to_string(),
            clan_id: "9".to_string(),
            channel_id: "5".to_string(),
            creator_id: "3".to_string(),
            last_sender_id: last_sender_id.to_string(),
            last_sender_is_authoritative: false,
            last_message_id: String::new(),
            content: content.to_string(),
            last_message_content: String::new(),
            last_message_attachments: Vec::new(),
            last_message_timestamp,
        }
    }

    fn assert_index_matches_topics(data: &TopicsData) {
        assert_eq!(
            data.topic_index.len(),
            data.topics.len(),
            "topic_index and topics have diverged in size"
        );
        for (id, &index) in &data.topic_index {
            let Some(slot) = data.topics.get(index) else {
                panic!("topic_index[{id}] = {index} is out of bounds");
            };
            assert_eq!(
                &slot.id, id,
                "topic_index[{id}] = {index} points at topic {}",
                slot.id
            );
        }
        for stored in &data.topics {
            assert_eq!(
                data.topic_by_id(&stored.id).map(|t| t.id.as_str()),
                Some(stored.id.as_str())
            );
        }
    }

    fn assert_sorted_by_timestamp_desc(data: &TopicsData) {
        let stamps: Vec<u32> = data
            .topics
            .iter()
            .map(|t| t.last_message_timestamp)
            .collect();
        let mut expected = stamps.clone();
        expected.sort_by(|a, b| b.cmp(a));
        assert_eq!(stamps, expected, "topics are not sorted newest-first");
    }

    fn topic_ids(data: &TopicsData) -> Vec<String> {
        data.topics.iter().map(|t| t.id.clone()).collect()
    }

    fn data_with_active_topic() -> TopicsData {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(555, 100, "8", "origin", 500));
        data
    }

    fn compose_for(origin_message_id: i64, active_topic_id: i64) -> TopicCompose {
        TopicCompose {
            origin_message_id: Some(MessageId(origin_message_id)),
            active_topic_id: Some(active_topic_id),
            ..Default::default()
        }
    }

    #[test]
    fn sd_topic_event_maps_every_field_when_fully_populated() {
        let ev = sd_topic_event(
            Some(header(4242, 1_700_000_500, 8, "last reply")),
            Some(channel_message("origin body", 1_700_000_000)),
        );

        let sd = sd_topic_from_event(&ev);
        assert_eq!(sd.id, 77);
        assert_eq!(sd.creator_id, 3);
        assert_eq!(sd.message_id, 42);
        assert_eq!(sd.clan_id, 9);
        assert_eq!(sd.channel_id, 5);
        assert_eq!(sd.create_time_seconds, 1_700_000_000);
        assert_eq!(sd.update_time_seconds, 1_700_000_500);
        assert_eq!(sd.content, "origin body");

        let discussion = topic_discussion_from_api(sd);
        assert_eq!(discussion.id, "77");
        assert_eq!(discussion.message_id, "42");
        assert_eq!(discussion.clan_id, "9");
        assert_eq!(discussion.channel_id, "5");
        assert_eq!(discussion.creator_id, "3");
        assert_eq!(discussion.last_sender_id, "8");
        assert_eq!(discussion.content, "origin body");
        assert_eq!(discussion.last_message_timestamp, 1_700_000_500);
    }

    #[test]
    fn sd_topic_event_without_last_sent_message_falls_back_to_creator_and_zero_timestamp() {
        let ev = sd_topic_event(None, Some(channel_message("origin body", 1_700_000_000)));

        let discussion = topic_discussion_from_api(sd_topic_from_event(&ev));
        assert_eq!(discussion.content, "origin body");
        assert_eq!(discussion.last_sender_id, "3");
        assert_eq!(discussion.last_message_timestamp, 0);
    }

    #[test]
    fn sd_topic_event_content_falls_back_to_last_sent_message_when_message_is_missing() {
        let ev = sd_topic_event(Some(header(4242, 1_700_000_500, 8, "last reply")), None);

        let discussion = topic_discussion_from_api(sd_topic_from_event(&ev));
        assert_eq!(discussion.content, "last reply");
        assert_eq!(discussion.last_sender_id, "8");
        assert_eq!(discussion.last_message_timestamp, 1_700_000_500);
    }

    #[test]
    fn sd_topic_event_content_falls_back_when_message_content_is_empty() {
        let ev = sd_topic_event(
            Some(header(4242, 1_700_000_500, 8, "last reply")),
            Some(channel_message("", 1_700_000_000)),
        );

        let discussion = topic_discussion_from_api(sd_topic_from_event(&ev));
        assert_eq!(discussion.content, "last reply");
    }

    #[test]
    fn sd_topic_event_last_sender_falls_back_to_creator_when_the_header_sender_is_zero() {
        let ev = sd_topic_event(Some(header(4242, 1_700_000_500, 0, "last reply")), None);

        let discussion = topic_discussion_from_api(sd_topic_from_event(&ev));
        assert_eq!(discussion.last_sender_id, "3");
    }

    #[test]
    fn sd_topic_event_with_zero_ids_and_empty_content_maps_without_panicking() {
        let ev = realtime::SdTopicEvent {
            id: 0,
            clan_id: 0,
            channel_id: 0,
            message_id: 0,
            user_id: 0,
            last_sent_message: Some(header(0, 0, 0, "")),
            message: Some(channel_message("", 0)),
        };

        let discussion = topic_discussion_from_api(sd_topic_from_event(&ev));
        assert_eq!(discussion.id, "0");
        assert_eq!(discussion.message_id, "0");
        assert_eq!(discussion.creator_id, "0");
        assert_eq!(discussion.last_sender_id, "0");
        assert!(discussion.content.is_empty());
        assert_eq!(discussion.last_message_timestamp, 0);
    }

    #[test]
    fn sd_topic_event_last_sent_seconds_uses_the_header_timestamp_when_present() {
        let ev = sd_topic_event(Some(header(4242, 1_700_000_500, 8, "last reply")), None);
        assert_eq!(
            sd_topic_event_last_sent_seconds(&ev, 1_800_000_000),
            1_700_000_500
        );
    }

    #[test]
    fn sd_topic_event_last_sent_seconds_falls_back_to_now_when_the_header_is_absent() {
        let ev = sd_topic_event(None, None);
        assert_eq!(
            sd_topic_event_last_sent_seconds(&ev, 1_800_000_000),
            1_800_000_000
        );
    }

    #[test]
    fn sd_topic_event_last_sent_seconds_falls_back_to_now_when_the_header_timestamp_is_zero() {
        let ev = sd_topic_event(Some(header(4242, 0, 8, "last reply")), None);
        assert_eq!(
            sd_topic_event_last_sent_seconds(&ev, 1_800_000_000),
            1_800_000_000
        );
    }

    #[test]
    fn upsert_topic_does_not_clobber_stored_values_with_zero_ids_or_empty_content() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 100, "8", "hello", 500));
        data.upsert_topic(topic(10, 0, "0", "", 0));

        let stored = data.topic_by_id("10").expect("topic 10");
        assert_eq!(stored.message_id, "100");
        assert_eq!(stored.last_sender_id, "8");
        assert_eq!(stored.content, "hello");
        assert_eq!(stored.last_message_timestamp, 500);
        assert_eq!(data.topics().len(), 1);
    }

    #[test]
    fn upsert_topic_does_not_clobber_stored_values_with_empty_ids() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 100, "8", "hello", 500));
        data.upsert_topic(TopicDiscussion {
            message_id: String::new(),
            last_sender_id: String::new(),
            ..topic(10, 0, "", "", 0)
        });

        let stored = data.topic_by_id("10").expect("topic 10");
        assert_eq!(stored.message_id, "100");
        assert_eq!(stored.last_sender_id, "8");
    }

    #[test]
    fn upsert_topic_merges_newer_values_and_keeps_the_newest_timestamp() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 100, "8", "hello", 500));
        data.upsert_topic(topic(10, 100, "9", "newer", 900));

        let stored = data.topic_by_id("10").expect("topic 10");
        assert_eq!(stored.content, "newer");
        assert_eq!(stored.last_sender_id, "9");
        assert_eq!(stored.last_message_timestamp, 900);

        data.upsert_topic(topic(10, 100, "9", "stale", 100));
        assert_eq!(
            data.topic_by_id("10")
                .expect("topic 10")
                .last_message_timestamp,
            900
        );
    }

    #[test]
    fn topics_are_sorted_newest_first_with_a_consistent_index_after_out_of_order_upserts() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topic(topic(20, 2, "8", "b", 300));
        data.upsert_topic(topic(30, 3, "8", "c", 200));

        assert_eq!(topic_ids(&data), vec!["20", "30", "10"]);
        assert_sorted_by_timestamp_desc(&data);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn set_topics_sorts_and_indexes_a_fetch_response() {
        let mut data = TopicsData::default();
        data.set_topics(vec![
            topic(10, 1, "8", "a", 100),
            topic(20, 2, "8", "b", 300),
            topic(30, 3, "8", "c", 200),
        ]);

        assert_eq!(topic_ids(&data), vec!["20", "30", "10"]);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn realtime_topic_message_updates_preview_and_moves_topic_to_front() {
        let mut data = TopicsData::default();
        data.set_topics(vec![
            topic(10, 1, "3", "older", 100),
            topic(20, 2, "4", "newer", 200),
        ]);
        let message = api::ChannelMessage {
            message_id: 500,
            topic_id: 10,
            sender_id: 99,
            content: "latest reply".into(),
            create_time_seconds: 300,
            ..Default::default()
        };

        assert!(data.note_topic_message(&message));
        assert_eq!(data.topics()[0].id, "10");
        assert_eq!(data.topics()[0].content, "older");
        assert_eq!(data.topics()[0].last_message_content, "latest reply");
        assert_eq!(data.topics()[0].last_sender_id, "99");
        assert_eq!(data.topics()[0].last_message_id, "500");
        assert_eq!(
            inbox_message_key(&data.topics()[0]),
            Some((ChannelId(10), MessageId(500)))
        );
        assert_eq!(data.topics()[0].last_message_timestamp, 300);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn upsert_topics_merges_a_page_then_resorts_once() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topics(vec![
            topic(10, 1, "9", "a2", 150),
            topic(20, 2, "8", "b", 300),
            topic(30, 3, "8", "c", 200),
        ]);

        assert_eq!(topic_ids(&data), vec!["20", "30", "10"]);
        assert_eq!(data.topic_by_id("10").expect("topic 10").content, "a2");
        assert_eq!(
            data.topic_by_id("10").expect("topic 10").last_sender_id,
            "9"
        );
        assert_eq!(
            data.topic_by_id("10")
                .expect("topic 10")
                .last_message_timestamp,
            150
        );
        assert_sorted_by_timestamp_desc(&data);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn topic_index_is_rebuilt_when_a_touch_resorts_the_list() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topic(topic(20, 2, "8", "b", 300));
        data.upsert_topic(topic(30, 3, "8", "c", 200));
        assert_index_matches_topics(&data);

        assert!(data.touch_topic_last_sent(10, 900));

        assert_eq!(topic_ids(&data), vec!["10", "20", "30"]);
        assert_sorted_by_timestamp_desc(&data);
        assert_index_matches_topics(&data);
        assert_eq!(data.topic_by_id("30").expect("topic 30").content, "c");
        assert_eq!(data.topic_by_id("20").expect("topic 20").content, "b");
        assert_eq!(
            data.topic_by_id("10")
                .expect("topic 10")
                .last_message_timestamp,
            900
        );
    }

    #[test]
    fn topic_index_is_rebuilt_when_a_meta_upsert_resorts_the_list() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topic(topic(20, 2, "8", "b", 300));
        data.upsert_topic(topic(30, 3, "8", "c", 200));

        data.upsert_topic_meta("30".to_string(), 4, 900);

        assert_eq!(topic_ids(&data), vec!["30", "20", "10"]);
        assert_eq!(
            data.topic_by_id("30")
                .expect("topic 30")
                .last_message_timestamp,
            900
        );
        assert_sorted_by_timestamp_desc(&data);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn advance_topic_timestamp_never_moves_a_topic_backward() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 500));

        data.advance_topic_timestamp("10", 100);

        assert_eq!(
            data.topic_by_id("10")
                .expect("topic 10")
                .last_message_timestamp,
            500
        );
        assert_index_matches_topics(&data);
    }

    #[test]
    fn upsert_topic_meta_overwrites_the_reply_count_verbatim() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 5, 500);
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 5);

        data.upsert_topic_meta("10".to_string(), 2, 600);
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 2);

        data.upsert_topic_meta("10".to_string(), 0, 700);
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 0);
    }

    #[test]
    fn upsert_topic_meta_only_moves_lsnt_forward() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 1, 500);

        data.upsert_topic_meta("10".to_string(), 2, 100);
        assert_eq!(data.topic_meta("10").expect("meta 10").lsnt, 500);

        data.upsert_topic_meta("10".to_string(), 3, 900);
        assert_eq!(data.topic_meta("10").expect("meta 10").lsnt, 900);
    }

    #[test]
    fn upsert_topic_meta_normalizes_millisecond_timestamps() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 1, 1_700_000_000_000);

        assert_eq!(data.topic_meta("10").expect("meta 10").lsnt, 1_700_000_000);
    }

    #[test]
    fn upsert_topic_meta_seeds_a_missing_entry_with_its_topic_id() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 7, 500);

        let meta = data.topic_meta("10").expect("meta 10");
        assert_eq!(meta.rpl, 7);
        assert_eq!(meta.lsnt, 500);
        assert_eq!(meta.tp_id, "10");
    }

    #[test]
    fn increment_topic_reply_count_adds_one_to_the_stored_count() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 2, 500);

        assert!(data.increment_topic_reply_count(10, 600));

        let meta = data.topic_meta("10").expect("meta 10");
        assert_eq!(meta.rpl, 3);
        assert_eq!(meta.lsnt, 600);
    }

    #[test]
    fn increment_topic_reply_count_seeds_a_missing_meta_with_one_reply() {
        let mut data = TopicsData::default();

        assert!(data.increment_topic_reply_count(10, 600));

        let meta = data.topic_meta("10").expect("meta 10");
        assert_eq!(meta.rpl, 1);
        assert_eq!(meta.lsnt, 600);
        assert_eq!(meta.tp_id, "10");
    }

    #[test]
    fn increment_topic_reply_count_advances_the_topic_sort_key_and_reindexes() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topic(topic(20, 2, "8", "b", 300));

        assert!(data.increment_topic_reply_count(10, 900));

        assert_eq!(topic_ids(&data), vec!["10", "20"]);
        assert_eq!(
            data.topic_by_id("10")
                .expect("topic 10")
                .last_message_timestamp,
            900
        );
        assert_sorted_by_timestamp_desc(&data);
        assert_index_matches_topics(&data);
    }

    #[test]
    fn increment_topic_reply_count_never_moves_lsnt_backward() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 2, 900);

        assert!(data.increment_topic_reply_count(10, 100));

        assert_eq!(data.topic_meta("10").expect("meta 10").lsnt, 900);
    }

    #[test]
    fn increment_topic_reply_count_ignores_a_zero_topic_id() {
        let mut data = TopicsData::default();

        assert!(!data.increment_topic_reply_count(0, 600));

        assert!(data.topic_meta("0").is_none());
    }

    #[test]
    fn decrement_topic_reply_count_never_goes_negative() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 1, 500);

        assert!(data.decrement_topic_reply_count(10));
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 0);

        assert!(data.decrement_topic_reply_count(10));
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 0);
    }

    #[test]
    fn decrement_topic_reply_count_ignores_unknown_and_zero_topics() {
        let mut data = TopicsData::default();

        assert!(!data.decrement_topic_reply_count(10));
        assert!(!data.decrement_topic_reply_count(0));
        assert!(data.topic_meta("10").is_none());
    }

    #[test]
    fn touch_topic_last_sent_moves_lsnt_forward_and_keeps_the_reply_count() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 4, 500);

        assert!(data.touch_topic_last_sent(10, 900));
        let meta = data.topic_meta("10").expect("meta 10");
        assert_eq!(meta.lsnt, 900);
        assert_eq!(meta.rpl, 4);

        assert!(data.touch_topic_last_sent(10, 100));
        let meta = data.topic_meta("10").expect("meta 10");
        assert_eq!(meta.lsnt, 900);
        assert_eq!(meta.rpl, 4);
    }

    #[test]
    fn touch_topic_last_sent_ignores_a_zero_topic_id_or_a_zero_timestamp() {
        let mut data = TopicsData::default();

        assert!(!data.touch_topic_last_sent(0, 900));
        assert!(!data.touch_topic_last_sent(10, 0));

        assert!(data.topic_meta("0").is_none());
        assert!(data.topic_meta("10").is_none());
    }

    #[test]
    fn topic_reply_summary_reports_nothing_for_an_unknown_topic() {
        let data = TopicsData::default();

        assert_eq!(data.topic_reply_summary("10"), (0, None));
    }

    #[test]
    fn topic_reply_summary_hides_a_zero_last_sent_timestamp() {
        let mut data = TopicsData::default();
        data.upsert_topic_meta("10".to_string(), 3, 0);
        assert_eq!(data.topic_reply_summary("10"), (3, None));

        data.upsert_topic_meta("10".to_string(), 3, 700);
        assert_eq!(data.topic_reply_summary("10"), (3, Some(700)));
    }

    #[test]
    fn clear_topics_keeps_the_reply_meta_of_other_clans() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 1, "8", "a", 100));
        data.upsert_topic_meta("10".to_string(), 3, 500);

        data.clear_topics();

        assert!(data.topics().is_empty());
        assert!(data.topic_by_id("10").is_none());
        assert_eq!(data.topic_meta("10").expect("meta 10").rpl, 3);

        data.clear();
        assert!(data.topic_meta("10").is_none());
    }

    #[test]
    fn topic_id_for_origin_message_finds_the_topic_of_an_origin_message() {
        let mut data = TopicsData::default();
        data.upsert_topic(topic(10, 100, "8", "a", 100));
        data.upsert_topic(topic(20, 200, "8", "b", 300));

        assert_eq!(
            data.topic_id_for_origin_message("200"),
            Some("20".to_string())
        );
        assert_eq!(data.topic_id_for_origin_message("999"), None);
    }

    #[test]
    fn deleting_a_reply_inside_the_active_topic_does_not_close_the_panel() {
        let data = data_with_active_topic();
        let compose = compose_for(100, 555);

        assert!(!panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(555),
            MessageId(900),
            Some(ChannelId(555)),
        ));
    }

    #[test]
    fn deleting_a_reply_that_shares_the_origin_id_does_not_close_the_panel() {
        let data = data_with_active_topic();
        let compose = compose_for(100, 555);

        assert!(!panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(555),
            MessageId(100),
            Some(ChannelId(555)),
        ));
    }

    #[test]
    fn deleting_the_origin_message_closes_the_panel() {
        let data = data_with_active_topic();
        let compose = compose_for(100, 555);

        assert!(panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(10),
            MessageId(100),
            None,
        ));
        assert!(panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(10),
            MessageId(100),
            Some(ChannelId(555)),
        ));
    }

    #[test]
    fn deleting_the_topic_origin_closes_the_panel_even_without_a_compose_origin() {
        let data = data_with_active_topic();
        let compose = TopicCompose {
            active_topic_id: Some(555),
            ..Default::default()
        };

        assert!(panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(10),
            MessageId(100),
            Some(ChannelId(555)),
        ));
    }

    #[test]
    fn deleting_a_message_from_another_topic_does_not_close_the_panel() {
        let data = data_with_active_topic();
        let compose = compose_for(100, 555);

        assert!(!panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
            ChannelId(10),
            MessageId(700),
            Some(ChannelId(777)),
        ));
    }

    #[test]
    fn a_closed_panel_never_closes_on_a_message_deletion() {
        let data = data_with_active_topic();
        let compose = compose_for(100, 555);

        assert!(!panel_should_close_on_message_deleted(
            false,
            &compose,
            &data,
            ChannelId(10),
            MessageId(100),
            None,
        ));
    }

    #[test]
    fn message_allows_topic_discussion_rejects_topic_and_poll() {
        assert!(!TopicsStore::message_allows_topic_discussion(
            &sample_message(MessageCode::Topic)
        ));
        assert!(!TopicsStore::message_allows_topic_discussion(
            &sample_message(MessageCode::Poll)
        ));
    }

    #[test]
    fn message_allows_topic_discussion_rejects_message_buzz_which_is_not_a_system_code() {
        assert!(!MessageCode::MessageBuzz.is_system());
        assert!(!TopicsStore::message_allows_topic_discussion(
            &sample_message(MessageCode::MessageBuzz)
        ));
    }

    #[test]
    fn message_allows_topic_discussion_rejects_every_system_code() {
        for code in [
            MessageCode::Welcome,
            MessageCode::UpcomingEvent,
            MessageCode::CreateThread,
            MessageCode::CreatePin,
            MessageCode::AuditLog,
            MessageCode::DeleteThread,
        ] {
            assert!(code.is_system(), "{code:?} is expected to be a system code");
            assert!(
                !TopicsStore::message_allows_topic_discussion(&sample_message(code)),
                "{code:?} must not allow a topic discussion"
            );
        }
    }

    #[test]
    fn message_allows_topic_discussion_allows_regular_message_codes() {
        for code in [
            MessageCode::Chat,
            MessageCode::SendToken,
            MessageCode::Ephemeral,
            MessageCode::ShareContact,
            MessageCode::Location,
            MessageCode::Unknown(99),
        ] {
            assert!(
                TopicsStore::message_allows_topic_discussion(&sample_message(code)),
                "{code:?} must allow a topic discussion"
            );
        }
    }
}
