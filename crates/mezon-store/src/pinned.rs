use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use mezon_client::AppApi;
use mezon_client::ConnectionStatus;
use mezon_client::RealtimeEvent;
use mezon_client::transport::{ApiMessage, ApiPinMessage, parse_message_content_tokens};
use mezon_proto::realtime::LastPinMessageEvent;

use crate::AppConfig;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::{
    Embed, Message, MessageAttachment, MessageSpan, OgpPreview, PollData, RichLayout,
    build_rich_layout, link_marker_from_kind, parse_spans,
};
use crate::messages::{
    MessagesEvent, MessagesStore, build_embeds, build_ogp_preview, build_poll_data,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::user_profile::{ProfileContext, resolve_avatar_url};

#[derive(Debug, Clone)]
pub struct PinnedMessage {
    pub id: String,
    pub message_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub avatar_url: String,
    pub avatar_proxied: SharedString,
    pub content: String,
    pub raw_content: String,
    pub spans: Arc<[MessageSpan]>,
    pub rich_layout: Option<Arc<RichLayout>>,
    pub ogp: Option<Box<OgpPreview>>,
    pub embeds: Arc<[Embed]>,
    pub attachments: Vec<MessageAttachment>,
    /// Verified against the original message API; a display cache must not override it.
    pub attachments_from_source: bool,
    pub poll: Option<Box<PollData>>,
    pub create_time: i64,
}

impl PinnedMessage {
    pub fn preview_attachments<'a>(
        &'a self,
        cached: Option<&'a Message>,
    ) -> &'a [MessageAttachment] {
        if !self.attachments_from_source
            && let Some(message) = cached
            && !message.attachments.is_empty()
        {
            return &message.attachments;
        }
        &self.attachments
    }
}

#[derive(Debug, Clone)]
pub enum PinnedEvent {
    OpenPopoverRequested,
}

pub struct PinnedMessagesStore {
    channel_id: Option<String>,
    clan_id: Option<String>,
    messages: Vec<PinnedMessage>,
    loaded_channel: Option<String>,
    fetch_state: PinFetchState,
    pin_badges: HashSet<String>,
    api: Arc<AppApi>,
    _messages_sub: Subscription,
    _conn_watch: Task<()>,
}

#[derive(Default)]
struct PinFetchState {
    generation: u64,
    active: Option<u64>,
    dirty: bool,
}

#[derive(Debug, PartialEq)]
enum PinFetchCompletion {
    Ignore,
    Retry,
    Apply,
}

impl PinFetchState {
    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn reset(&mut self) {
        self.generation += 1;
        self.active = None;
        self.dirty = false;
    }

    fn start(&mut self) -> Option<u64> {
        if self.active.is_some() {
            return None;
        }
        self.generation += 1;
        self.active = Some(self.generation);
        self.dirty = false;
        self.active
    }

    fn finish(&mut self, generation: u64) -> PinFetchCompletion {
        if self.active != Some(generation) {
            return PinFetchCompletion::Ignore;
        }
        self.active = None;
        if self.dirty {
            PinFetchCompletion::Retry
        } else {
            PinFetchCompletion::Apply
        }
    }
}

struct GlobalPinnedMessagesStore(Entity<PinnedMessagesStore>);
impl Global for GlobalPinnedMessagesStore {}

impl EventEmitter<PinnedEvent> for PinnedMessagesStore {}

impl PinnedMessagesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalPinnedMessagesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPinnedMessagesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPinnedMessagesStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.channel_id = None;
        self.clan_id = None;
        self.messages.clear();
        self.loaded_channel = None;
        self.fetch_state.reset();
        self.pin_badges.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let messages_sub = cx.subscribe(&MessagesStore::global(cx), |this, store, event, cx| {
            if matches!(event, MessagesEvent::Reset { .. }) {
                this.sync_from_messages(&store, cx);
            }
        });
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let mut store = Self {
            channel_id: None,
            clan_id: None,
            messages: Vec::new(),
            loaded_channel: None,
            fetch_state: PinFetchState::default(),
            pin_badges: HashSet::new(),
            api,
            _messages_sub: messages_sub,
            _conn_watch: conn_watch,
        };
        store.sync_from_messages(&MessagesStore::global(cx), cx);
        store
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::LastPinMessage, &entity, |this, event, cx| {
                this.handle_last_pin(event, cx);
            });
            dispatch.on(RealtimeKind::UnpinMessage, &entity, |this, event, cx| {
                this.handle_unpin(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| {
                if this.loaded_channel.is_some() {
                    this.refresh(cx);
                }
            });
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
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn pinned(&self) -> &[PinnedMessage] {
        &self.messages
    }

    pub fn is_loading(&self) -> bool {
        self.fetch_state.active.is_some()
    }

    pub fn clan_id(&self) -> Option<ClanId> {
        self.clan_id
            .as_ref()
            .and_then(|id| id.parse::<ClanId>().ok())
            .filter(|id| !id.is_zero())
    }

    pub fn channel_id(&self) -> Option<ChannelId> {
        self.channel_id
            .as_ref()
            .and_then(|id| id.parse::<ChannelId>().ok())
    }

    fn sync_from_messages(&mut self, store: &Entity<MessagesStore>, cx: &mut Context<Self>) {
        let (channel_id, clan_id) = {
            let messages = store.read(cx);
            context_from_active(messages.active_channel_id(), messages.active_clan_id())
        };
        if self.channel_id == channel_id && self.clan_id == clan_id {
            return;
        }
        self.channel_id = channel_id;
        self.clan_id = clan_id;
        self.messages.clear();
        self.loaded_channel = None;
        self.fetch_state.invalidate();
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_from_messages(&MessagesStore::global(cx), cx);
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        if self.is_loading() || self.loaded_channel.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.fetch(cx);
    }

    pub fn request_open_popover(&mut self, cx: &mut Context<Self>) {
        cx.emit(PinnedEvent::OpenPopoverRequested);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.fetch_state.invalidate();
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        let Some(generation) = self.fetch_state.start() else {
            return;
        };
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.get_pin_messages_list(&channel_id, &clan_id).await;
            let result = match result {
                Ok(mut pins) => {
                    if let (Ok(clan), Ok(channel)) = (clan_id.parse(), channel_id.parse()) {
                        let verified =
                            hydrate_pin_attachments(&api, clan, channel, &mut pins).await;
                        Ok((pins, verified))
                    } else {
                        Ok((pins, HashSet::new()))
                    }
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                match this.fetch_state.finish(generation) {
                    PinFetchCompletion::Ignore => return,
                    PinFetchCompletion::Retry => {
                        this.fetch(cx);
                        cx.notify();
                        return;
                    }
                    PinFetchCompletion::Apply => {}
                }
                if this.channel_id.as_deref() != Some(channel_id.as_str())
                    || this.clan_id.as_deref() != Some(clan_id.as_str())
                {
                    cx.notify();
                    this.fetch(cx);
                    return;
                }
                match result {
                    Ok((list, verified)) => {
                        let cfg = AppConfig::try_global(cx);
                        this.messages = list
                            .into_iter()
                            .map(|m| {
                                let from_source = m
                                    .message_id
                                    .parse()
                                    .ok()
                                    .is_some_and(|id| verified.contains(&id));
                                let mut pin = pinned_from_api(m, cfg);
                                pin.attachments_from_source = from_source;
                                pin
                            })
                            .collect();
                        this.loaded_channel = Some(channel_id);
                    }
                    Err(e) => tracing::error!("get_pin_messages_list failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn is_pinned(&self, message_id: &str) -> bool {
        self.messages.iter().any(|m| m.message_id == message_id)
    }

    pub fn active_has_pin_badge(&self) -> bool {
        self.channel_id
            .as_ref()
            .is_some_and(|id| self.pin_badges.contains(id))
    }

    pub fn clear_pin_badge(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        if self.pin_badges.remove(channel_id) {
            cx.notify();
        }
    }

    pub fn clear_active_pin_badge(&mut self, cx: &mut Context<Self>) {
        if let Some(channel_id) = self.channel_id.clone() {
            self.clear_pin_badge(&channel_id, cx);
        }
    }

    fn set_pin_badge(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        if channel_id.is_empty() {
            return;
        }
        if self.pin_badges.insert(channel_id.to_string()) {
            cx.notify();
        }
    }

    fn handle_last_pin(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::LastPinMessage(pin) = event else {
            return;
        };
        if pin.operation != 1 || pin.channel_id == 0 {
            return;
        }
        let channel_id = pin.channel_id.to_string();
        self.set_pin_badge(&channel_id, cx);
        if self.channel_id.as_deref() != Some(channel_id.as_str()) {
            return;
        }
        let message_id = pin.message_id.to_string();
        if self.messages.iter().any(|m| m.message_id == message_id) {
            self.refresh(cx);
            return;
        }
        let cfg = AppConfig::try_global(cx);
        self.messages
            .insert(0, pinned_from_last_pin_event(pin, cfg));
        cx.notify();
        self.refresh(cx);
    }

    fn handle_unpin(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::UnpinMessage(ev) = event else {
            return;
        };
        if ev.channel_id == 0 {
            return;
        }
        let channel_id = ev.channel_id.to_string();
        if self.channel_id.as_deref() != Some(channel_id.as_str()) {
            return;
        }
        let message_id = ev.message_id.to_string();
        let pin_id = ev.id.to_string();
        let before = self.messages.len();
        self.messages
            .retain(|m| m.message_id != message_id && m.id != pin_id && m.id != message_id);
        if self.messages.len() != before {
            cx.notify();
        }
        self.refresh(cx);
    }

    /// Pin a message. Mirrors React `setChannelPinMessage` + `joinPinMessage`:
    /// create via API, broadcast `LastPinMessageEvent`, and optimistically insert locally.
    pub fn pin(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let Some(channel_id_str) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id_str) = self.clan_id.clone() else {
            return;
        };
        let (Ok(message_id_i64), Ok(channel_id_i64), Ok(clan_id_i64)) = (
            message_id.parse::<i64>(),
            channel_id_str.parse::<i64>(),
            clan_id_str.parse::<i64>(),
        ) else {
            return;
        };
        if self.is_pinned(message_id) {
            return;
        }

        let messages = MessagesStore::global(cx).read(cx);
        let mode = messages.mode();
        let is_public = messages.is_public();
        let clan_id_opt = self.clan_id();
        let channel_id_opt = self.channel_id();
        let msg = messages
            .message_in_channel(ChannelId(channel_id_i64), MessageId(message_id_i64))
            .cloned();

        let (
            sender_id,
            sender_name,
            avatar_url,
            _content_plain,
            content_wire,
            create_time,
            created_time_iso,
            pin_attachments,
        ) = if let Some(msg) = msg.as_ref() {
            let sender_id = msg.sender_id.clone();
            let sender_name = msg.sender_name.to_string();
            let mut avatar = msg.avatar_url.to_string();
            if let Ok(user_id) = sender_id.parse::<UserId>()
                && !user_id.is_zero()
            {
                if let Some(clan_id) = clan_id_opt
                    && let Some(url) =
                        resolve_avatar_url(user_id, ProfileContext::Clan(clan_id), cx)
                            .filter(|url| !url.is_empty())
                {
                    avatar = url;
                } else if let Some(channel_id) = channel_id_opt
                    && let Some(url) =
                        resolve_avatar_url(user_id, ProfileContext::Direct(channel_id), cx)
                            .filter(|url| !url.is_empty())
                {
                    avatar = url;
                }
            }
            let content_wire = pin_content_wire(msg);
            let create_time = msg.create_time;
            let created_time_iso = chrono::DateTime::from_timestamp(create_time, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
            let pin_attachments = pin_message_attachments(msg);
            (
                sender_id,
                sender_name,
                avatar,
                msg.content.clone(),
                content_wire,
                create_time,
                created_time_iso,
                pin_attachments,
            )
        } else {
            return;
        };

        let cfg = AppConfig::try_global(cx);
        let avatar_proxied = cfg
            .map(|c| c.avatar_proxy(&avatar_url))
            .unwrap_or_else(|| avatar_url.clone());
        let body = enrich_pin_body(&content_wire, pin_attachments, cfg);
        self.messages.insert(
            0,
            PinnedMessage {
                id: message_id.to_string(),
                message_id: message_id.to_string(),
                sender_id: sender_id.clone(),
                sender_name: sender_name.clone(),
                avatar_url: avatar_url.clone(),
                avatar_proxied: avatar_proxied.into(),
                content: body.text,
                raw_content: content_wire.clone(),
                spans: body.spans,
                rich_layout: body.rich_layout,
                ogp: body.ogp,
                embeds: body.embeds,
                attachments: body.attachments,
                attachments_from_source: false,
                poll: body.poll,
                create_time,
            },
        );
        self.fetch_state.invalidate();
        self.set_pin_badge(&channel_id_str, cx);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let source = match api
                .list_channel_messages(clan_id_i64, channel_id_i64, message_id_i64, 2, 3)
                .await
            {
                Ok(page) => page
                    .messages
                    .into_iter()
                    .find(|m| m.message_id == message_id_i64),
                Err(error) => {
                    tracing::error!("read original message before pin failed: {error}");
                    None
                }
            };
            let Some(source) = source else {
                tracing::error!("original message unavailable; pin was not written");
                let _ = this.update(cx, |this, cx| {
                    if this.channel_id.as_deref() == Some(channel_id_str.as_str())
                        && this.clan_id.as_deref() == Some(clan_id_str.as_str())
                    {
                        this.messages
                            .retain(|pin| pin.message_id != message_id_i64.to_string());
                        this.refresh(cx);
                    }
                });
                return;
            };
            let attachment = pin_attachments_wire(
                &source
                    .attachments
                    .into_iter()
                    .filter(|a| pin_attachment_url_valid(&a.url))
                    .map(|a| MessageAttachment::from_api(a, None))
                    .collect::<Vec<_>>(),
            );
            if let Err(e) = api
                .create_pin_message(message_id_i64, channel_id_i64, clan_id_i64)
                .await
            {
                tracing::error!("create_pin_message failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.channel_id.as_deref() == Some(channel_id_str.as_str())
                        && this.clan_id.as_deref() == Some(clan_id_str.as_str())
                    {
                        this.refresh(cx);
                    }
                });
                return;
            }
            if let Err(e) = api
                .write_last_pin_message(
                    clan_id_i64,
                    channel_id_i64,
                    message_id_i64,
                    mode,
                    is_public,
                    chrono::Utc::now().timestamp() as u32,
                    1,
                    &avatar_url,
                    &sender_id,
                    &sender_name,
                    &content_wire,
                    &attachment,
                    &created_time_iso,
                )
                .await
            {
                tracing::error!("write_last_pin_message failed: {e}");
            }
            let _ = this.update(cx, |this, cx| {
                if this.channel_id.as_deref() == Some(channel_id_str.as_str())
                    && this.clan_id.as_deref() == Some(clan_id_str.as_str())
                {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }

    pub fn unpin(&mut self, pin_id: &str, message_id: &str, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        self.messages.retain(|m| m.id != pin_id);
        self.fetch_state.invalidate();
        cx.notify();

        let api = self.api.clone();
        let pin_id = pin_id.to_string();
        let message_id = message_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = api
                .delete_pin_message(&pin_id, &message_id, &channel_id, &clan_id)
                .await;
            if let Err(e) = result {
                tracing::error!("delete_pin_message failed: {e}");
            }
            let _ = this.update(cx, |this, cx| {
                if this.channel_id.as_deref() == Some(channel_id.as_str())
                    && this.clan_id.as_deref() == Some(clan_id.as_str())
                {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }
}

fn pin_attachment_url_valid(url: &str) -> bool {
    let url = url.trim();
    !url.is_empty() && (url.starts_with("http://") || url.starts_with("https://"))
}

fn apply_source_attachments(pins: &mut [ApiPinMessage], sources: &[ApiMessage]) {
    for pin in pins {
        if let Some(source) = sources
            .iter()
            .find(|m| m.message_id.to_string() == pin.message_id)
        {
            pin.attachments = source.attachments.clone();
        }
    }
}

async fn hydrate_pin_attachments(
    api: &AppApi,
    clan_id: i64,
    channel_id: i64,
    pins: &mut [ApiPinMessage],
) -> HashSet<i64> {
    let mut verified = HashSet::new();
    let mut pending: HashSet<i64> = pins
        .iter()
        .filter_map(|p| p.message_id.parse().ok())
        .collect();
    while let Some(anchor) = pending.iter().copied().next() {
        pending.remove(&anchor);
        match api
            .list_channel_messages(clan_id, channel_id, anchor, 2, 100)
            .await
        {
            Ok(page) => {
                for source in &page.messages {
                    pending.remove(&source.message_id);
                    verified.insert(source.message_id);
                }
                apply_source_attachments(pins, &page.messages);
            }
            Err(error) => tracing::warn!("read original pinned messages failed: {error}"),
        }
    }
    verified
}

fn pin_message_attachments(message: &Message) -> Vec<MessageAttachment> {
    message
        .attachments
        .iter()
        .filter(|attachment| pin_attachment_url_valid(&attachment.url))
        .cloned()
        .collect()
}

fn pin_attachments_wire(attachments: &[MessageAttachment]) -> String {
    serde_json::Value::Array(
        attachments
            .iter()
            .map(|attachment| {
                serde_json::json!({
                    "url": attachment.url,
                    "filename": attachment.filename,
                    "filetype": attachment.filetype,
                    "width": attachment.width,
                    "height": attachment.height,
                    "thumbnail": attachment.thumbnail,
                    "duration": attachment.duration,
                    "size": attachment.size,
                })
            })
            .collect(),
    )
    .to_string()
}

fn pin_content_wire(msg: &Message) -> String {
    if let Some(raw) = msg.raw_content.as_deref().filter(|raw| !raw.is_empty()) {
        return raw.to_string();
    }
    rebuild_pin_content_json(msg)
}

fn rebuild_pin_content_json(msg: &Message) -> String {
    let mut obj = serde_json::Map::new();
    let mut t = String::new();
    let mut mk = Vec::new();
    let mut hg = Vec::new();
    let mut ej = Vec::new();
    let mut lk = Vec::new();

    for span in &msg.spans {
        match span {
            MessageSpan::Text(text) => t.push_str(text),
            MessageSpan::Bold(text) => {
                let start = pin_utf16_len(&t);
                t.push_str(text);
                mk.push(serde_json::json!({
                    "s": start,
                    "e": pin_utf16_len(&t),
                    "type": "b",
                }));
            }
            MessageSpan::Code(text) => {
                let start = pin_utf16_len(&t);
                t.push_str(text);
                mk.push(serde_json::json!({
                    "s": start,
                    "e": pin_utf16_len(&t),
                    "type": "c",
                }));
            }
            MessageSpan::CodeBlock { text, .. } => {
                let start = pin_utf16_len(&t);
                t.push_str(text);
                mk.push(serde_json::json!({
                    "s": start,
                    "e": pin_utf16_len(&t),
                    "type": "pre",
                }));
            }
            MessageSpan::Heading { text, .. } => t.push_str(text),
            MessageSpan::Mention { display, .. } => t.push_str(display),
            MessageSpan::Hashtag {
                display,
                channel_id,
                meta,
            } => {
                let start = pin_utf16_len(&t);
                t.push_str(display);
                let mut item = serde_json::Map::new();
                item.insert("s".into(), start.into());
                item.insert("e".into(), pin_utf16_len(&t).into());
                if let Some(channel_id) = channel_id.as_ref().filter(|id| !id.is_empty()) {
                    item.insert("channelId".into(), channel_id.clone().into());
                }
                if let Some(meta) = meta {
                    item.insert("clanId".into(), meta.clan_id.get().to_string().into());
                    item.insert("channelLabel".into(), meta.label.to_string().into());
                    item.insert("channelType".into(), meta.channel_type.as_raw().into());
                    if let Some(parent_id) = meta.parent_id {
                        item.insert("parentId".into(), parent_id.get().to_string().into());
                    }
                }
                hg.push(serde_json::Value::Object(item));
            }
            MessageSpan::Emoji { name, emoji_id, .. } => {
                let start = pin_utf16_len(&t);
                t.push_str(name);
                let mut item = serde_json::Map::new();
                item.insert("s".into(), start.into());
                item.insert("e".into(), pin_utf16_len(&t).into());
                if !emoji_id.is_empty() {
                    item.insert("emojiid".into(), emoji_id.clone().into());
                }
                ej.push(serde_json::Value::Object(item));
            }
            MessageSpan::Link { text, url, kind } => {
                let start = pin_utf16_len(&t);
                t.push_str(text);
                let mut item = serde_json::Map::new();
                item.insert("s".into(), start.into());
                item.insert("e".into(), pin_utf16_len(&t).into());
                if !url.is_empty() {
                    item.insert("url".into(), url.clone().into());
                }
                // `parse_spans` only classifies a bare `lk` token when `mk` is empty (mirroring
                // React's `patchLinkTokens`), so the kind rides in `mk` instead. Dropping it into
                // `lk` would downgrade the card to a plain link whenever the pinned message also
                // carries bold or code.
                match link_marker_from_kind(*kind) {
                    Some(marker) => {
                        item.insert("type".into(), marker.into());
                        mk.push(serde_json::Value::Object(item));
                    }
                    None => lk.push(serde_json::Value::Object(item)),
                }
            }
            MessageSpan::Canvas { title, .. } => t.push_str(title),
        }
    }

    if t.is_empty() {
        t = msg.content.clone();
    }
    obj.insert("t".into(), t.into());

    if !msg.mention_targets.is_empty() {
        let mentions: Vec<serde_json::Value> = msg
            .mention_targets
            .iter()
            .map(|m| {
                let mut item = serde_json::Map::new();
                if let Some(user_id) = m.user_id.as_ref().filter(|id| !id.is_empty()) {
                    item.insert("user_id".into(), user_id.clone().into());
                }
                if let Some(role_id) = m.role_id.as_ref().filter(|id| !id.is_empty()) {
                    item.insert("role_id".into(), role_id.clone().into());
                }
                if !m.username.is_empty() {
                    item.insert("username".into(), m.username.clone().into());
                }
                item.insert("s".into(), m.s.into());
                item.insert("e".into(), m.e.into());
                serde_json::Value::Object(item)
            })
            .collect();
        obj.insert("mentions".into(), mentions.into());
    }
    if !mk.is_empty() {
        obj.insert("mk".into(), mk.into());
    }
    if !hg.is_empty() {
        obj.insert("hg".into(), hg.into());
    }
    if !ej.is_empty() {
        obj.insert("ej".into(), ej.into());
    }
    if !lk.is_empty() {
        obj.insert("lk".into(), lk.into());
    }

    serde_json::Value::Object(obj).to_string()
}

fn pin_utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

fn enrich_pin_body(
    raw_content: &str,
    attachments: Vec<MessageAttachment>,
    cfg: Option<&AppConfig>,
) -> EnrichedPinBody {
    let trimmed = raw_content.trim();
    let tokens = parse_message_content_tokens(trimmed);
    let spans = parse_spans(&tokens);
    let rich_layout = build_rich_layout(&spans);
    let ogp = build_ogp_preview(&tokens, cfg);
    let embeds = build_embeds(&tokens, cfg);
    let text = if tokens.t.is_empty() {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(serde_json::Value::Object(fields)) => fields
                .get("t")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            _ => trimmed.to_string(),
        }
    } else {
        tokens.t.clone()
    };
    let poll = build_poll_data(&tokens, &text, cfg);
    let text = if poll.is_some() { String::new() } else { text };
    EnrichedPinBody {
        text,
        spans: spans.into(),
        rich_layout,
        ogp,
        embeds,
        attachments,
        poll,
    }
}

struct EnrichedPinBody {
    text: String,
    spans: Arc<[MessageSpan]>,
    rich_layout: Option<Arc<RichLayout>>,
    ogp: Option<Box<OgpPreview>>,
    embeds: Arc<[Embed]>,
    attachments: Vec<MessageAttachment>,
    poll: Option<Box<PollData>>,
}

fn pinned_from_api(m: ApiPinMessage, cfg: Option<&AppConfig>) -> PinnedMessage {
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&m.avatar))
        .unwrap_or_else(|| m.avatar.clone());
    let attachments = m
        .attachments
        .into_iter()
        .map(|a| MessageAttachment::from_api(a, cfg))
        .collect::<Vec<_>>();
    let body = enrich_pin_body(&m.content, attachments, cfg);
    PinnedMessage {
        id: m.id,
        message_id: m.message_id,
        sender_id: m.sender_id,
        sender_name: m.sender_name,
        avatar_url: m.avatar,
        avatar_proxied: avatar_proxied.into(),
        content: if body.poll.is_some() {
            String::new()
        } else if body.text.is_empty() {
            m.content_text
        } else {
            body.text
        },
        raw_content: m.content,
        spans: body.spans,
        rich_layout: body.rich_layout,
        ogp: body.ogp,
        embeds: body.embeds,
        attachments: body.attachments,
        attachments_from_source: false,
        poll: body.poll,
        create_time: m.create_time,
    }
}

fn pinned_from_last_pin_event(pin: &LastPinMessageEvent, cfg: Option<&AppConfig>) -> PinnedMessage {
    let message_id = pin.message_id.to_string();
    let avatar = pin.message_sender_avatar.clone();
    let avatar_proxied = cfg
        .map(|c| c.avatar_proxy(&avatar))
        .unwrap_or_else(|| avatar.clone());
    let create_time = if pin.timestamp_seconds > 0 {
        i64::from(pin.timestamp_seconds)
    } else {
        parse_pin_create_time(&pin.message_created_time)
    };
    let attachments = mezon_client::parse_search_attachment_field(&pin.message_attachment)
        .into_iter()
        .map(|a| MessageAttachment::from_api(a, cfg))
        .collect::<Vec<_>>();
    let body = enrich_pin_body(&pin.message_content, attachments, cfg);
    PinnedMessage {
        id: message_id.clone(),
        message_id,
        sender_id: pin.message_sender_id.clone(),
        sender_name: pin.message_sender_username.clone(),
        avatar_url: avatar,
        avatar_proxied: avatar_proxied.into(),
        content: body.text,
        raw_content: pin.message_content.clone(),
        spans: body.spans,
        rich_layout: body.rich_layout,
        ogp: body.ogp,
        embeds: body.embeds,
        attachments: body.attachments,
        attachments_from_source: false,
        poll: body.poll,
        create_time,
    }
}

fn parse_pin_create_time(raw: &str) -> i64 {
    if raw.is_empty() {
        return chrono::Utc::now().timestamp();
    }
    if let Ok(ts) = raw.parse::<i64>() {
        return ts;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|_| chrono::Utc::now().timestamp())
}

fn context_from_active(
    channel_id: Option<ChannelId>,
    clan_id: Option<ClanId>,
) -> (Option<String>, Option<String>) {
    (
        channel_id.map(|id| id.to_string()),
        clan_id.map(|id| id.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_source_image_and_empty_source_cannot_be_overridden_by_stale_cache() {
        let mut cached = Message::new(MessageId(42), "", "7", "user", 0);
        cached.attachments = vec![MessageAttachment {
            url: "https://cdn.example/wrong-godzilla.jpg".into(),
            ..Default::default()
        }];
        let mut pin = pinned_from_last_pin_event(
            &LastPinMessageEvent {
                message_id: 42,
                message_attachment:
                    r#"[{"url":"https://cdn.example/correct-portrait.png","filetype":"image"}]"#
                        .into(),
                ..Default::default()
            },
            None,
        );
        pin.attachments_from_source = true;
        assert_eq!(
            pin.preview_attachments(Some(&cached))[0].url,
            "https://cdn.example/correct-portrait.png"
        );
        pin.attachments.clear();
        assert!(pin.preview_attachments(Some(&cached)).is_empty());
        pin.attachments_from_source = false;
        assert_eq!(
            pin.preview_attachments(Some(&cached))[0].url,
            "https://cdn.example/wrong-godzilla.jpg"
        );
    }

    #[test]
    fn refresh_during_fetch_discards_old_result_and_coalesces_requests() {
        let mut state = PinFetchState::default();
        let old = state.start().unwrap();
        state.invalidate();
        state.invalidate();
        assert_eq!(state.start(), None);
        assert_eq!(state.finish(old), PinFetchCompletion::Retry);
        let fresh = state.start().unwrap();
        assert_eq!(state.finish(old), PinFetchCompletion::Ignore);
        assert_eq!(state.active, Some(fresh));
        assert_eq!(state.finish(fresh), PinFetchCompletion::Apply);
        assert!(state.active.is_none());
    }

    #[test]
    fn reset_and_reload_same_channel_rejects_previous_session_result() {
        let mut state = PinFetchState::default();
        let old = state.start().unwrap();
        state.reset();
        let current = state.start().unwrap();
        assert_eq!(state.finish(old), PinFetchCompletion::Ignore);
        assert_eq!(state.active, Some(current));
        assert_eq!(state.finish(current), PinFetchCompletion::Apply);
    }

    #[test]
    fn another_mutation_during_retry_requires_another_fresh_snapshot() {
        let mut state = PinFetchState::default();
        let first = state.start().unwrap();
        state.invalidate();
        assert_eq!(state.finish(first), PinFetchCompletion::Retry);
        let second = state.start().unwrap();
        state.invalidate();
        assert_eq!(state.finish(second), PinFetchCompletion::Retry);
        let third = state.start().unwrap();
        assert_eq!(state.finish(third), PinFetchCompletion::Apply);
    }

    #[test]
    fn original_messages_repair_empty_and_wrong_pin_images_without_cache() {
        let record = |id: i64, url: &str| ApiPinMessage {
            id: (id + 100).to_string(),
            message_id: id.to_string(),
            content: String::new(),
            content_text: String::new(),
            sender_id: "7".into(),
            sender_name: "user".into(),
            avatar: String::new(),
            create_time: 0,
            attachments: if url.is_empty() {
                vec![]
            } else {
                vec![mezon_client::transport::ApiAttachment {
                    url: url.into(),
                    filetype: "image".into(),
                    ..Default::default()
                }]
            },
        };
        let mut pins = vec![
            record(42, ""),
            record(43, "https://cdn.example/wrong.jpg"),
            record(44, "https://cdn.example/removed.jpg"),
            record(45, "https://cdn.example/unavailable.jpg"),
        ];
        let source = |id, url: &str| ApiMessage {
            message_id: id,
            content: String::new(),
            content_tokens: Default::default(),
            content_raw: String::new(),
            code: 0,
            sender_id: 7,
            sender_name: "user".into(),
            avatar: String::new(),
            create_time: 0,
            update_time: 0,
            hide_editted: false,
            attachments: record(id, url).attachments,
            references: vec![],
            reactions: vec![],
            entity_mentions: vec![],
            topic_id: 0,
        };
        apply_source_attachments(
            &mut pins,
            &[
                source(43, "https://cdn.example/godzilla.jpg"),
                source(44, ""),
                source(42, "https://cdn.example/portrait.png"),
                source(999, "https://cdn.example/unrelated.jpg"),
            ],
        );
        let pins: Vec<_> = pins.into_iter().map(|p| pinned_from_api(p, None)).collect();
        assert_eq!(
            pins[0].attachments[0].url,
            "https://cdn.example/portrait.png"
        );
        assert_eq!(
            pins[1].attachments[0].url,
            "https://cdn.example/godzilla.jpg"
        );
        assert!(pins[2].attachments.is_empty());
        assert_eq!(
            pins[3].attachments[0].url,
            "https://cdn.example/unavailable.jpg"
        );
    }

    #[test]
    fn pin_attachment_write_round_trip_preserves_distinct_images_and_long_urls() {
        let mut message = Message::new(MessageId(42), "", "7", "user", 0);
        let long_url = format!("https://cdn.example/{}.png", "a".repeat(512));
        message.attachments = vec![
            MessageAttachment {
                url: long_url.clone(),
                filename: "ChatGPT Image.png".into(),
                filetype: "image/png".into(),
                width: 1200,
                height: 800,
                size: 1762267,
                ..Default::default()
            },
            MessageAttachment {
                url: "https://cdn.example/godzulaThum.jpg".into(),
                filename: "godzulaThum.jpg".into(),
                filetype: "image/jpeg".into(),
                ..Default::default()
            },
            MessageAttachment {
                url: "file:///local-preview.png".into(),
                ..Default::default()
            },
        ];
        let attachment = pin_attachments_wire(&pin_message_attachments(&message));
        let pin = pinned_from_last_pin_event(
            &LastPinMessageEvent {
                message_id: 42,
                message_attachment: attachment,
                ..Default::default()
            },
            None,
        );
        assert_eq!(pin.message_id, "42");
        assert_eq!(pin.attachments.len(), 2);
        assert_eq!(pin.attachments[0].url, long_url);
        assert_eq!(pin.attachments[0].filename, "ChatGPT Image.png");
        assert_eq!(pin.attachments[0].width, 1200);
        assert_eq!(pin.attachments[0].height, 800);
        assert_eq!(pin.attachments[0].size, 1762267);
        assert_eq!(
            pin.attachments[1].url,
            "https://cdn.example/godzulaThum.jpg"
        );
        assert!(pin.attachments.iter().all(MessageAttachment::is_image));
    }

    #[test]
    fn pin_attachment_url_validation_keeps_remote_urls_only() {
        assert!(pin_attachment_url_valid("http://cdn.example/image.jpg"));
        for url in [
            "",
            " ",
            "file:///image.png",
            "blob:local",
            "data:image/png;base64,abc",
        ] {
            assert!(!pin_attachment_url_valid(url), "{url}");
        }
    }

    #[test]
    fn pin_write_and_api_reload_preserve_separate_messages_without_cache() {
        let urls = [
            format!("https://cdn.example/{}.png", "a".repeat(512)),
            "https://cdn.example/godzulaThum.jpg".into(),
        ];
        let api_records: Vec<_> = urls
            .iter()
            .enumerate()
            .map(|(index, url)| {
                let id = MessageId(42 + index as i64);
                let mut message = Message::new(id, "", "7", "user", 0);
                message.attachments = vec![MessageAttachment {
                    url: url.clone(),
                    filetype: "image".into(),
                    ..Default::default()
                }];
                let wire = pin_attachments_wire(&pin_message_attachments(&message));
                ApiPinMessage {
                    id: (100 + index).to_string(),
                    message_id: id.to_string(),
                    content: "{\"t\":\"\"}".into(),
                    content_text: String::new(),
                    sender_id: "7".into(),
                    sender_name: "user".into(),
                    avatar: String::new(),
                    create_time: 0,
                    attachments: mezon_client::parse_search_attachment_field(&wire),
                }
            })
            .collect();
        // Only the API records survive; no message cache is available on reload.
        let pins: Vec<_> = api_records
            .into_iter()
            .map(|record| pinned_from_api(record, None))
            .collect();
        for (index, pin) in pins.iter().enumerate() {
            assert_eq!(pin.id, (100 + index).to_string());
            assert_eq!(pin.message_id, (42 + index).to_string());
            assert_eq!(pin.attachments.len(), 1);
            assert_eq!(pin.attachments[0].url, urls[index]);
            assert!(pin.attachments[0].is_image());
        }
    }

    #[test]
    fn parse_pin_create_time_unix() {
        assert_eq!(parse_pin_create_time("1710000000"), 1710000000);
    }

    #[test]
    fn context_from_active_dm_keeps_clan_id_zero() {
        assert_eq!(
            context_from_active(Some(ChannelId(9)), Some(ClanId(0))),
            (Some("9".into()), Some("0".into())),
        );
    }

    #[test]
    fn context_from_active_clan_channel() {
        assert_eq!(
            context_from_active(Some(ChannelId(3)), Some(ClanId(7))),
            (Some("3".into()), Some("7".into())),
        );
    }

    #[test]
    fn context_from_active_cleared() {
        assert_eq!(context_from_active(None, None), (None, None));
    }

    #[test]
    fn rebuild_pin_content_json_round_trips_a_social_link_beside_markdown() {
        let mut msg = Message::new(MessageId(1), "", "1", "user", 0);
        msg.spans = vec![
            crate::message::MessageSpan::Bold("hi".into()),
            crate::message::MessageSpan::Text(" ".into()),
            crate::message::MessageSpan::Link {
                text: "https://youtu.be/abc".into(),
                url: "https://youtu.be/abc".into(),
                kind: crate::message::LinkKind::YouTube,
            },
        ];
        let json = rebuild_pin_content_json(&msg);
        let content: mezon_client::transport::ApiMessageContent =
            serde_json::from_str(&json).expect("valid content json");
        assert!(
            parse_spans(&content).iter().any(|span| matches!(
                span,
                crate::message::MessageSpan::Link {
                    kind: crate::message::LinkKind::YouTube,
                    ..
                }
            )),
            "a pinned social link must stay classified when the message also carries markdown"
        );
    }

    #[test]
    fn poll_pin_body_parses_poll_and_drops_raw_json() {
        let raw = r#"{"poll_id":123,"question":"gg","answers":[{"index":0,"label":"1"},{"index":1,"label":"2"}],"answer_counts":[1,1],"total_votes":2,"type":1,"expire_at":0}"#;

        let body = enrich_pin_body(raw, Vec::new(), None);

        assert!(body.text.is_empty());
        let poll = body.poll.expect("poll parsed from pin content");
        assert_eq!(poll.question.as_ref(), "gg");
        assert_eq!(poll.answers.len(), 2);
        assert_eq!(poll.total_votes, 2);
        assert!(poll.allow_multiple);
    }

    #[test]
    fn embed_only_pin_body_never_shows_raw_json() {
        let raw = r#"{"embed":[{"title":"hi","description":"there"}]}"#;

        let body = enrich_pin_body(raw, Vec::new(), None);

        assert!(body.text.is_empty());
        assert_eq!(body.embeds.len(), 1);
    }

    #[test]
    fn plain_text_pin_body_keeps_non_json_content() {
        let body = enrich_pin_body("hello", Vec::new(), None);

        assert_eq!(body.text, "hello");
    }

    #[test]
    fn text_pin_body_keeps_text_and_has_no_poll() {
        let body = enrich_pin_body(r#"{"t":"hello"}"#, Vec::new(), None);

        assert_eq!(body.text, "hello");
        assert!(body.poll.is_none());
    }
}
