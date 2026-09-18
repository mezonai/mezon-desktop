use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use mezon_client::AppApi;
use mezon_client::ConnectionStatus;
use mezon_client::RealtimeEvent;
use mezon_client::transport::{ApiPinMessage, parse_message_content_tokens};
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
    pub poll: Option<Box<PollData>>,
    pub create_time: i64,
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
    loading: bool,
    pin_badges: HashSet<String>,
    api: Arc<AppApi>,
    _messages_sub: Subscription,
    _conn_watch: Task<()>,
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
        self.loading = false;
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
            loading: false,
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
        self.loading
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
        cx.notify();
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_from_messages(&MessagesStore::global(cx), cx);
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        if self.loading || self.loaded_channel.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.fetch(cx);
    }

    pub fn request_open_popover(&mut self, cx: &mut Context<Self>) {
        cx.emit(PinnedEvent::OpenPopoverRequested);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loaded_channel = None;
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self.channel_id.clone() else {
            return;
        };
        let Some(clan_id) = self.clan_id.clone() else {
            return;
        };
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.get_pin_messages_list(&channel_id, &clan_id).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                if this.channel_id.as_deref() != Some(channel_id.as_str()) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(list) => {
                        let cfg = AppConfig::try_global(cx);
                        this.messages = list.into_iter().map(|m| pinned_from_api(m, cfg)).collect();
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
            return;
        }
        let cfg = AppConfig::try_global(cx);
        self.messages
            .insert(0, pinned_from_last_pin_event(pin, cfg));
        cx.notify();
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
            .messages()
            .iter()
            .find(|m| m.id.get() == message_id_i64)
            .or_else(|| {
                messages.message_in_channel(ChannelId(channel_id_i64), MessageId(message_id_i64))
            })
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
            let pin_attachments: Vec<_> = msg
                .attachments
                .iter()
                .filter(|att| pin_attachment_url_valid(&att.url))
                .cloned()
                .collect();
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
        let attachment = if body.attachments.is_empty() {
            "[]".to_string()
        } else {
            serde_json::to_string(
                &body
                    .attachments
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "url": a.url,
                            "filename": a.filename,
                            "filetype": a.filetype,
                            "width": a.width,
                            "height": a.height,
                            "thumbnail": a.thumbnail,
                            "duration": a.duration,
                            "size": a.size,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into())
        };
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
                poll: body.poll,
                create_time,
            },
        );
        self.set_pin_badge(&channel_id_str, cx);
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if let Err(e) = api
                .create_pin_message(message_id_i64, channel_id_i64, clan_id_i64)
                .await
            {
                tracing::error!("create_pin_message failed: {e}");
                let _ = this.update(cx, |this, cx| this.refresh(cx));
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
                let _ = this.update(cx, |this, cx| this.refresh(cx));
            }
        })
        .detach();
    }
}

fn pin_attachment_url_valid(url: &str) -> bool {
    let url = url.trim();
    !url.is_empty()
        && url.len() < 512
        && (url.starts_with("http://") || url.starts_with("https://"))
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
            } => {
                let start = pin_utf16_len(&t);
                t.push_str(display);
                let mut item = serde_json::Map::new();
                item.insert("s".into(), start.into());
                item.insert("e".into(), pin_utf16_len(&t).into());
                if let Some(channel_id) = channel_id.as_ref().filter(|id| !id.is_empty()) {
                    item.insert("channelId".into(), channel_id.clone().into());
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
