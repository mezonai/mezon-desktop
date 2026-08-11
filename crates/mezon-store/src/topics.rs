use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::OutgoingMessageFlags;
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
    MessagesStore, OutgoingAttachment, OutgoingContent, OutgoingEmoji, OutgoingHashtag,
    OutgoingMention, ReplyDraft, viewer_user_id,
};
use crate::presign;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{CACHE_TTL, ChannelId, ClanId, Message, MessageId, UserId};

const TOPICS_LIMIT: i32 = 50;
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

    fn upsert_topic(&mut self, topic: TopicDiscussion) {
        let topic_id = topic.id.clone();
        if let Some(idx) = self.topic_index.get(&topic_id).copied() {
            let existing = &mut self.topics[idx];
            if !topic.content.is_empty() {
                existing.content = topic.content;
            }
            if topic.last_message_timestamp > 0 {
                existing.last_message_timestamp = existing
                    .last_message_timestamp
                    .max(topic.last_message_timestamp);
            }
            if !topic.last_sender_id.is_empty() && topic.last_sender_id != "0" {
                existing.last_sender_id = topic.last_sender_id;
            }
            if !topic.message_id.is_empty() && topic.message_id != "0" {
                existing.message_id = topic.message_id;
            }
        } else {
            self.topics.push(topic);
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

pub struct TopicsStore {
    data: TopicsData,
    clan_id: Option<String>,
    loading: bool,
    fetch_generation: u64,
    fetched_at: Option<Instant>,
    panel_open: bool,
    init_topic_message_id: Option<MessageId>,
    reply_target: Option<ReplyDraft>,
    compose: TopicCompose,
    compose_generation: u64,
    creating_topic_for: Option<MessageId>,
    api: Arc<AppApi>,
    updated_notify_task: Option<Task<()>>,
    _conn_watch: Task<()>,
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
        Self {
            data: TopicsData::default(),
            clan_id: None,
            loading: false,
            fetch_generation: 0,
            fetched_at: None,
            panel_open: false,
            init_topic_message_id: None,
            reply_target: None,
            compose: TopicCompose::default(),
            compose_generation: 0,
            creating_topic_for: None,
            api,
            updated_notify_task: None,
            _conn_watch: conn_watch,
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
        self.data.clear();
        self.clan_id = None;
        self.loading = false;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        self.fetched_at = None;
        self.init_topic_message_id = None;
        self.updated_notify_task = None;
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.creating_topic_for = None;
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

    pub fn set_reply_to(&mut self, message_id: MessageId, cx: &mut Context<Self>) {
        let Some(draft) = MessagesStore::global(cx)
            .read(cx)
            .reply_draft_for(message_id)
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

    /// Open the discussion panel for `origin` (mezon-react `handleCreateTopic`).
    /// Reuses `origin.topic_id` when already set (topic button or card footer);
    /// otherwise the topic is created on first reply.
    pub fn start_create(&mut self, origin: Message, cx: &mut Context<Self>) {
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

        let existing_topic_id = origin.topic_id.map(|t| t.get()).filter(|id| *id != 0);

        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.init_topic_message_id = Some(origin.id);
        self.compose = TopicCompose {
            origin_message_id: Some(origin.id),
            origin_message: Some(origin),
            parent_channel_id: Some(parent_channel_id),
            clan_id: Some(clan_id),
            mode,
            is_public,
            active_topic_id: existing_topic_id,
            submitting: false,
            creating: false,
            error: None,
        };
        self.panel_open = true;

        messages.update(cx, |store, cx| {
            store.set_active_topic(existing_topic_id, cx);
        });
        if let Some(topic_id) = existing_topic_id {
            messages.update(cx, |store, cx| {
                store.fetch_topic_messages(clan_id, parent_channel_id, topic_id, cx);
            });
        }

        cx.emit(TopicsEvent::Opened);
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
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
    ) -> bool {
        panel_should_close_on_message_deleted(
            self.panel_open,
            &self.compose,
            &self.data,
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
        message_id: MessageId,
        message_topic_id: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) {
        if self.should_close_on_message_deleted(message_id, message_topic_id) {
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
            message_code: 0,
        };
        let has_attachments = !attachments.is_empty();
        let reply_ref =
            self.reply_target
                .take()
                .map(|draft| mezon_client::transport::OutgoingReply {
                    message_ref_id: draft.message_ref_id.get(),
                    content: draft.content_preview,
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
        let transport_hashtags: Vec<mezon_client::transport::OutgoingHashtag> = content_tokens
            .hashtags
            .into_iter()
            .map(OutgoingHashtag::into_transport)
            .collect();
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
            let ack = if has_attachments {
                let files: Vec<UploadFile> = attachments
                    .into_iter()
                    .map(OutgoingAttachment::into_upload)
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
                let update_mentions = if anonymous {
                    Vec::new()
                } else {
                    transport_mentions.clone()
                };
                let update_hashtags = transport_hashtags.clone();
                let update_emojis = transport_emojis.clone();
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
                        Vec::new(),
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
                upload_ctx = Some((
                    presigned,
                    keys,
                    update_mentions,
                    update_hashtags,
                    update_emojis,
                    sent.message_id,
                    sent.create_time.max(0) as u32,
                ));
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
                    cx,
                );
            });

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
            let (on_complete, mut completions) =
                tokio::sync::mpsc::unbounded_channel::<AttachmentUploadOutcome>();
            let drain_this = this.clone();
            cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                while let Some(outcome) = completions.recv().await {
                    if drain_this.upgrade().is_none() {
                        return;
                    }
                    cx.update(|cx| {
                        MessagesStore::global(cx).update(cx, |store, cx| {
                            store.apply_topic_attachment_outcome(
                                topic_id,
                                MessageId(real_message_id),
                                outcome,
                                cx,
                            );
                        });
                    });
                }
            })
            .detach();
            api.upload_presigned_and_patch(
                clan_id,
                topic_id,
                real_message_id,
                &content,
                update_mentions,
                update_hashtags,
                update_emojis,
                create_time_seconds,
                presigned,
                keys,
                mode,
                is_public,
                topic_id,
                true,
                on_complete,
            )
            .await;
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
            let append = store.append_topic_message(topic_id, ack, anonymous, cx);
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
                    content: draft.content_preview,
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

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    fn is_fresh(&self, clan_id: &str) -> bool {
        self.clan_id.as_deref() == Some(clan_id)
            && self.fetched_at.is_some_and(|t| t.elapsed() < CACHE_TTL)
    }

    pub fn fetch_if_needed(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.is_fresh(clan_id) {
            return;
        }
        self.fetch(clan_id, cx);
    }

    pub fn fetch(&mut self, clan_id: &str, cx: &mut Context<Self>) {
        if self.loading && self.clan_id.as_deref() == Some(clan_id) {
            return;
        }
        if self.clan_id.as_deref() != Some(clan_id) {
            self.data.clear_topics();
            self.clan_id = Some(clan_id.to_string());
            self.fetched_at = None;
        }
        self.loading = true;
        self.fetch_generation = self.fetch_generation.wrapping_add(1);
        let generation = self.fetch_generation;
        cx.notify();

        let api = self.api.clone();
        let clan_id = clan_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = api.list_sd_topics(&clan_id, TOPICS_LIMIT).await;
            let _ = this.update(cx, |this, cx| {
                this.apply_fetch_result(&clan_id, generation, result, cx);
            });
        })
        .detach();
    }

    fn apply_fetch_result(
        &mut self,
        clan_id: &str,
        generation: u64,
        result: Result<Vec<TopicDiscussion>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.fetch_generation != generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(topics) => {
                self.data.set_topics(topics);
                self.clan_id = Some(clan_id.to_string());
                self.fetched_at = Some(Instant::now());
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
            Err(e) => {
                tracing::error!("list_sd_topics failed: {e}");
                cx.emit(TopicsEvent::Updated);
                cx.notify();
            }
        }
    }
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
    message_id: MessageId,
    message_topic_id: Option<ChannelId>,
) -> bool {
    if !panel_open {
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
            content: content.to_string(),
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
            MessageId(900),
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
            MessageId(100),
            None,
        ));
        assert!(panel_should_close_on_message_deleted(
            true,
            &compose,
            &data,
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
