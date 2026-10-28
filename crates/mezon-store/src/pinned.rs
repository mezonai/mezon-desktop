use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use mezon_client::AppApi;
use mezon_client::ConnectionStatus;
use mezon_client::RealtimeEvent;
use mezon_client::transport::{ApiMessage, ApiPinMessage, parse_message_content_tokens};
use mezon_proto::realtime::LastPinMessageEvent;

use crate::AppConfig;
use crate::CACHE_TTL;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::message::{
    Embed, Message, MessageAttachment, MessageCode, MessageSpan, OgpPreview, PollData, RichLayout,
    build_rich_layout, link_marker_from_kind, parse_spans,
};
use crate::messages::{
    MessagesEvent, MessagesStore, build_embeds, build_ogp_preview, build_poll_data,
    message_from_channel_proto,
};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::user_profile::{ProfileContext, resolve_avatar_url};

const PIN_SOURCE_PAGE_LIMIT: u32 = 100;
const PIN_SOURCE_DIRECTION_AROUND: i32 = 2;

pub fn resolve_pin_avatar_url(
    sender_id: &str,
    fallback_avatar: &str,
    clan_id: Option<ClanId>,
    channel_id: Option<ChannelId>,
    cx: &App,
) -> String {
    if crate::messages::is_anonymous_sender_id(sender_id, cx) {
        return fallback_avatar.to_string();
    }
    if let Ok(user_id) = sender_id.parse::<UserId>()
        && !user_id.is_zero()
    {
        if let Some(clan_id) = clan_id
            && let Some(url) = resolve_avatar_url(user_id, ProfileContext::Clan(clan_id), cx)
                .filter(|url| !url.is_empty())
        {
            return url;
        }
        if let Some(channel_id) = channel_id
            && let Some(url) = resolve_avatar_url(user_id, ProfileContext::Direct(channel_id), cx)
                .filter(|url| !url.is_empty())
        {
            return url;
        }
    }
    fallback_avatar.to_string()
}

#[derive(Debug, Clone)]
pub struct PinnedMessage {
    pub id: String,
    pub message_id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub avatar_url: String,
    pub avatar_proxied: SharedString,
    pub avatar_from_source: bool,
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
    pub fn has_authoritative_empty_avatar(&self, cx: &App) -> bool {
        self.avatar_from_source
            && self.avatar_url.is_empty()
            && crate::messages::is_anonymous_sender_id(&self.sender_id, cx)
    }

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

    pub fn compact_preview_text(&self) -> Option<String> {
        let content = self.content.trim();
        if !content.is_empty() {
            return Some(content.to_string());
        }

        if let Some(layout) = self.rich_layout.as_ref() {
            let text = layout.text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }

        for embed in self.embeds.iter() {
            let title = embed.title.trim();
            let description = build_rich_layout(&embed.description_spans)
                .map(|layout| layout.text.trim().to_string())
                .filter(|description| !description.is_empty());
            match ((!title.is_empty()).then_some(title), description) {
                (Some(title), Some(description)) => {
                    return Some(format!("{title} {description}"));
                }
                (Some(title), None) => return Some(title.to_string()),
                (None, Some(description)) => return Some(description),
                (None, None) => {}
            }
        }

        if let Some(ogp) = self.ogp.as_ref() {
            let title = ogp.title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
            let description = ogp.description.trim();
            if !description.is_empty() {
                return Some(description.to_string());
            }
        }

        None
    }
}

#[derive(Debug, Clone)]
pub enum PinnedEvent {
    OpenPopoverRequested,
    Updated,
}

pub struct PinnedMessagesStore {
    channel_id: Option<String>,
    clan_id: Option<String>,
    messages: Vec<PinnedMessage>,
    channel_cache: HashMap<(String, String), Vec<PinnedMessage>>,
    channel_fetched_at: HashMap<(String, String), Instant>,
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
    confirmed: bool,
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
        self.confirmed = false;
    }

    fn reset(&mut self) {
        self.generation += 1;
        self.active = None;
        self.dirty = false;
        self.confirmed = false;
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

    fn confirm(&mut self) {
        self.confirmed = true;
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
        self.channel_cache.clear();
        self.channel_fetched_at.clear();
        self.loaded_channel = None;
        self.fetch_state.reset();
        self.pin_badges.clear();
        cx.emit(PinnedEvent::Updated);
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let messages_sub =
            cx.subscribe(
                &MessagesStore::global(cx),
                |this, store, event, cx| match event {
                    MessagesEvent::Reset { .. } => this.sync_from_messages(&store, cx),
                    MessagesEvent::Updated {
                        message_id: Some(message_id),
                    } => {
                        let message = this.channel_id().and_then(|channel_id| {
                            store
                                .read(cx)
                                .message_in_channel(channel_id, *message_id)
                                .cloned()
                        });
                        if let Some(message) = message {
                            this.apply_message_update(&message, cx);
                        }
                    }
                    _ => {}
                },
            );
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let mut store = Self {
            channel_id: None,
            clan_id: None,
            messages: Vec::new(),
            channel_cache: HashMap::new(),
            channel_fetched_at: HashMap::new(),
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
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_message_update(event, cx);
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

    pub fn is_loaded_for(&self, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.clan_id() == Some(clan_id)
            && self.channel_id() == Some(channel_id)
            && self.loaded_channel.as_deref() == self.channel_id.as_deref()
            && !self.is_loading()
            && !self.fetch_state.dirty
            && self.fetch_state.confirmed
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
        let cache_key = self.active_cache_key();
        let cached = cache_key
            .as_ref()
            .and_then(|key| self.channel_cache.get(key).cloned());
        let cache_fresh = cache_key.as_ref().is_some_and(|key| {
            self.channel_fetched_at
                .get(key)
                .is_some_and(|fetched_at| fetched_at.elapsed() < CACHE_TTL)
        });
        self.messages = cached.unwrap_or_default();
        // Render cached pins immediately, but still fetch the channel so upstream's freshness and
        // attachment verification guarantees are preserved.
        self.loaded_channel = cache_fresh.then(|| self.channel_id.clone()).flatten();
        // A request for the previous channel must neither block nor overwrite the new one. Keep
        // the per-channel cache visible while the latest state is fetched to avoid UI flicker.
        self.fetch_state.reset();
        if cache_fresh {
            self.fetch_state.confirm();
        }
        cx.emit(PinnedEvent::Updated);
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
        self.invalidate_active_snapshot();
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
        let anonymous_user_id = AppConfig::try_global(cx)
            .map(|cfg| cfg.anonymous_user_id.clone())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let result = api.get_pin_messages_list(&channel_id, &clan_id).await;
            let result = match result {
                Ok(mut pins) => {
                    if let (Ok(clan), Ok(channel)) = (clan_id.parse(), channel_id.parse()) {
                        let clan_key = clan_id.as_str();
                        let channel_key = channel_id.as_str();
                        let verified =
                            hydrate_pin_sources(&mut pins, &anonymous_user_id, |anchor| {
                                let api = api.clone();
                                let owner = this.clone();
                                let source_cx = cx.clone();
                                async move {
                                    let current = owner
                                        .read_with(&source_cx, |store, _| {
                                            store.clan_id.as_deref() == Some(clan_key)
                                                && store.channel_id.as_deref() == Some(channel_key)
                                                && store.fetch_state.active == Some(generation)
                                                && !store.fetch_state.dirty
                                        })
                                        .unwrap_or(false);
                                    if !current {
                                        return Ok(None);
                                    }
                                    api.list_channel_messages(
                                        clan,
                                        channel,
                                        anchor,
                                        PIN_SOURCE_DIRECTION_AROUND,
                                        PIN_SOURCE_PAGE_LIMIT,
                                    )
                                    .await
                                    .map(|page| Some(page.messages))
                                }
                            })
                            .await;
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
                        let cached: HashMap<_, _> = this
                            .messages
                            .iter()
                            .map(|pin| (pin.message_id.as_str(), pin))
                            .collect();
                        this.messages = list
                            .into_iter()
                            .map(|m| {
                                let from_source = m
                                    .message_id
                                    .parse()
                                    .ok()
                                    .is_some_and(|id| verified.contains(&id));
                                let previous = cached.get(m.message_id.as_str()).copied();
                                let mut pin = pinned_from_verified_api(
                                    m,
                                    from_source,
                                    previous,
                                    cfg,
                                    &anonymous_user_id,
                                );
                                pin.attachments_from_source = from_source;
                                pin
                            })
                            .collect();
                        this.cache_active_messages();
                        if let Some(key) = this.active_cache_key() {
                            this.channel_fetched_at.insert(key, Instant::now());
                        }
                        this.loaded_channel = Some(channel_id);
                        this.fetch_state.confirm();
                    }
                    Err(e) => {
                        this.loaded_channel = Some(channel_id);
                        tracing::error!("get_pin_messages_list failed: {e}");
                    }
                }
                cx.emit(PinnedEvent::Updated);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn is_pinned(&self, message_id: &str) -> bool {
        self.messages.iter().any(|m| m.message_id == message_id)
    }

    fn active_cache_key(&self) -> Option<(String, String)> {
        Some((self.clan_id.clone()?, self.channel_id.clone()?))
    }

    fn cache_active_messages(&mut self) {
        if let Some(key) = self.active_cache_key() {
            self.channel_cache.insert(key, self.messages.clone());
        }
    }

    fn apply_message_update(&mut self, message: &Message, cx: &mut Context<Self>) {
        let message_id = message.id.to_string();
        let Some(index) = self
            .messages
            .iter()
            .position(|pin| pin.message_id == message_id)
        else {
            return;
        };
        let mut updated = self.messages[index].clone();
        merge_pinned_message_update(&mut updated, message);
        if format!("{updated:?}") == format!("{:?}", self.messages[index]) {
            return;
        }
        self.messages[index] = updated;
        if self.is_loading() {
            self.invalidate_active_snapshot();
        }
        self.cache_active_messages();
        cx.emit(PinnedEvent::Updated);
        cx.notify();
    }

    fn handle_message_update(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ChannelMessage(message) = event else {
            return;
        };
        if MessageCode::from_raw(message.code) != MessageCode::ChatUpdate || message.message_id <= 0
        {
            return;
        }
        let channel_id = message.channel_id.to_string();
        if self.channel_id.as_deref() != Some(channel_id.as_str()) {
            self.invalidate_channel_cache(&channel_id);
            return;
        }
        let parsed = message_from_channel_proto(
            message,
            message.message_id,
            AppConfig::try_global(cx),
            None,
        );
        self.apply_message_update(&parsed, cx);
    }

    fn invalidate_active_snapshot(&mut self) {
        self.fetch_state.invalidate();
        if let Some(key) = self.active_cache_key() {
            self.channel_fetched_at.remove(&key);
        }
    }

    fn invalidate_channel_cache(&mut self, channel_id: &str) {
        self.channel_cache
            .retain(|(_, cached_channel_id), _| cached_channel_id != channel_id);
        self.channel_fetched_at
            .retain(|(_, cached_channel_id), _| cached_channel_id != channel_id);
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
            self.invalidate_channel_cache(&channel_id);
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
        self.cache_active_messages();
        self.invalidate_active_snapshot();
        cx.emit(PinnedEvent::Updated);
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
            self.invalidate_channel_cache(&channel_id);
            return;
        }
        let message_id = ev.message_id.to_string();
        let pin_id = ev.id.to_string();
        let before = self.messages.len();
        self.messages
            .retain(|m| m.message_id != message_id && m.id != pin_id && m.id != message_id);
        if self.messages.len() != before {
            self.cache_active_messages();
            self.invalidate_active_snapshot();
            cx.emit(PinnedEvent::Updated);
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
            let avatar = resolve_pin_avatar_url(
                &sender_id,
                &msg.avatar_url,
                clan_id_opt,
                channel_id_opt,
                cx,
            );
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
                avatar_from_source: true,
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
        self.cache_active_messages();
        self.invalidate_active_snapshot();
        cx.emit(PinnedEvent::Updated);
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
        self.cache_active_messages();
        self.invalidate_active_snapshot();
        cx.emit(PinnedEvent::Updated);
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

fn merge_pinned_message_update(pin: &mut PinnedMessage, message: &Message) {
    pin.sender_id = message.sender_id.clone();
    pin.sender_name = message.sender_name.to_string();
    pin.avatar_url = message.avatar_url.to_string();
    pin.avatar_proxied = message.avatar_proxied.clone();
    pin.avatar_from_source = true;
    pin.content = message.content.clone();
    pin.raw_content = message.raw_content.as_deref().unwrap_or("").to_string();
    pin.spans = message.spans.clone().into();
    pin.rich_layout = message.rich_layout.clone();
    pin.ogp = message.ogp.clone();
    pin.embeds = message.embeds.clone();
    if !pin.attachments_from_source && !message.attachments.is_empty() {
        pin.attachments = message.attachments.clone();
    }
    if message.poll.is_some() {
        pin.poll = message.poll.clone();
    }
}

fn pin_attachment_url_valid(url: &str) -> bool {
    let url = url.trim();
    !url.is_empty() && (url.starts_with("http://") || url.starts_with("https://"))
}

fn apply_pin_sources(pins: &mut [ApiPinMessage], sources: &[ApiMessage], anonymous_user_id: &str) {
    for pin in pins {
        if let Some(source) = sources
            .iter()
            .find(|m| m.message_id.to_string() == pin.message_id)
        {
            pin.attachments = source.attachments.clone();
            if !anonymous_user_id.is_empty()
                && (source.sender_id.to_string() == anonymous_user_id
                    || pin.sender_id == anonymous_user_id)
            {
                pin.sender_id = source.sender_id.to_string();
                if !source.sender_name.is_empty() {
                    pin.sender_name = source.sender_name.clone();
                }
                pin.avatar = source.avatar.clone();
            }
        }
    }
}

async fn hydrate_pin_sources<F, Fut>(
    pins: &mut [ApiPinMessage],
    anonymous_user_id: &str,
    mut fetch: F,
) -> HashSet<i64>
where
    F: FnMut(i64) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<Vec<ApiMessage>>>>,
{
    let mut verified = HashSet::new();
    let latest = pins
        .iter()
        .max_by_key(|pin| pin.create_time)
        .and_then(|pin| pin.message_id.parse::<i64>().ok());
    let mut seen = HashSet::new();
    let anchors: Vec<_> = latest
        .into_iter()
        .chain(
            pins.iter()
                .filter(|pin| !anonymous_user_id.is_empty() && pin.sender_id == anonymous_user_id)
                .filter_map(|pin| pin.message_id.parse::<i64>().ok()),
        )
        .filter(|id| *id > 0 && seen.insert(*id))
        .collect();
    for anchor in anchors {
        if verified.contains(&anchor) {
            continue;
        }
        match fetch(anchor).await {
            Ok(Some(sources)) => {
                verified.extend(sources.iter().map(|source| source.message_id));
                apply_pin_sources(pins, &sources, anonymous_user_id);
            }
            Ok(None) => break,
            Err(error) => tracing::warn!("read original pinned message failed: {error}"),
        }
    }
    verified
}

fn pinned_from_verified_api(
    record: ApiPinMessage,
    verified: bool,
    cached: Option<&PinnedMessage>,
    cfg: Option<&AppConfig>,
    anonymous_user_id: &str,
) -> PinnedMessage {
    let mut pin = pinned_from_api(record, cfg);
    pin.avatar_from_source = verified;
    if !verified && !anonymous_user_id.is_empty() && pin.sender_id == anonymous_user_id {
        if let Some(cached) = cached.filter(|cached| {
            cached.avatar_from_source
                && cached.message_id == pin.message_id
                && cached.sender_id == pin.sender_id
        }) {
            pin.sender_name = cached.sender_name.clone();
            pin.avatar_url = cached.avatar_url.clone();
            pin.avatar_proxied = cached.avatar_proxied.clone();
            pin.avatar_from_source = true;
        } else {
            pin.avatar_url.clear();
            pin.avatar_proxied = SharedString::default();
        }
    }
    pin
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
                    if meta.private {
                        item.insert("channelPrivate".into(), 1.into());
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
        avatar_from_source: false,
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
        avatar_from_source: true,
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

    const ANONYMOUS_ID: &str = "9876";

    fn anonymous_config(cx: &mut App) {
        AppConfig::init_global(
            Arc::new(AppConfig {
                anonymous_user_id: ANONYMOUS_ID.into(),
                ..Default::default()
            }),
            cx,
        );
    }

    fn persona_source(id: i64, sender_id: i64, avatar: &str) -> ApiMessage {
        ApiMessage {
            message_id: id,
            content: "hello".into(),
            content_tokens: Default::default(),
            content_raw: String::new(),
            code: 0,
            sender_id,
            sender_name: "persona".into(),
            avatar: avatar.into(),
            create_time: 1,
            update_time: 0,
            hide_editted: false,
            attachments: vec![],
            references: vec![],
            reactions: vec![],
            entity_mentions: vec![],
            topic_id: 0,
        }
    }

    fn persona_pin(id: i64, sender_id: &str, avatar: &str) -> ApiPinMessage {
        ApiPinMessage {
            id: id.to_string(),
            message_id: id.to_string(),
            content: "hello".into(),
            content_text: "hello".into(),
            sender_id: sender_id.into(),
            sender_name: "persona".into(),
            avatar: avatar.into(),
            create_time: 1,
            attachments: vec![],
        }
    }

    #[test]
    fn anonymous_pins_outside_latest_page_are_fetched_once_and_shared_pages_are_reused() {
        futures::executor::block_on(async {
            let mut pins = vec![
                persona_pin(1, ANONYMOUS_ID, "account.webp"),
                persona_pin(2, ANONYMOUS_ID, "account.webp"),
                persona_pin(5000, ANONYMOUS_ID, "account.webp"),
                persona_pin(1, ANONYMOUS_ID, "account.webp"),
                persona_pin(0, ANONYMOUS_ID, "account.webp"),
            ];
            pins[2].create_time = 5000;
            let mut calls = Vec::new();
            let verified = hydrate_pin_sources(&mut pins, ANONYMOUS_ID, |anchor| {
                calls.push(anchor);
                let sources = match anchor {
                    5000 => vec![persona_source(5000, 9876, "new.webp")],
                    1 => vec![
                        persona_source(1, 9876, ""),
                        persona_source(2, 9876, "old.webp"),
                    ],
                    _ => panic!("unexpected source request"),
                };
                std::future::ready(Ok(Some(sources)))
            })
            .await;
            assert_eq!(calls, [5000, 1]);
            let mapped: Vec<_> = pins
                .into_iter()
                .map(|record| {
                    let known = record
                        .message_id
                        .parse::<i64>()
                        .ok()
                        .is_some_and(|id| verified.contains(&id));
                    pinned_from_verified_api(record, known, None, None, ANONYMOUS_ID)
                })
                .collect();
            assert_eq!(
                mapped
                    .iter()
                    .map(|pin| pin.avatar_url.as_str())
                    .collect::<Vec<_>>(),
                ["", "old.webp", "new.webp", "", ""]
            );
            assert!(mapped[0].avatar_proxied.is_empty());
            assert!(mapped[0].avatar_from_source);
            assert!(!mapped[4].avatar_from_source);
        });
    }

    #[test]
    fn failed_and_missing_sources_do_not_reuse_unverified_account_avatars() {
        futures::executor::block_on(async {
            let mut pins = vec![
                persona_pin(1, ANONYMOUS_ID, "account.webp"),
                persona_pin(2, ANONYMOUS_ID, "account.webp"),
                persona_pin(3, ANONYMOUS_ID, "account.webp"),
            ];
            pins[2].create_time = 3;
            let mut calls = Vec::new();
            let verified = hydrate_pin_sources(&mut pins, ANONYMOUS_ID, |anchor| {
                calls.push(anchor);
                std::future::ready(match anchor {
                    3 => Err(anyhow::anyhow!("source unavailable")),
                    1 => Ok(Some(Vec::new())),
                    2 => Ok(Some(vec![persona_source(2, 9876, "persona.webp")])),
                    _ => panic!("unexpected source request"),
                })
            })
            .await;
            assert_eq!(calls, [3, 1, 2]);
            assert_eq!(verified, HashSet::from([2]));
            for record in pins {
                let known = verified.contains(&record.message_id.parse::<i64>().unwrap());
                let pin = pinned_from_verified_api(record, known, None, None, ANONYMOUS_ID);
                if pin.message_id == "2" {
                    assert_eq!(pin.avatar_url, "persona.webp");
                } else {
                    assert!(pin.avatar_url.is_empty());
                    assert!(pin.avatar_proxied.is_empty());
                    assert!(!pin.avatar_from_source);
                }
            }
        });
    }

    #[test]
    fn obsolete_pin_fetch_stops_requesting_more_source_pages() {
        futures::executor::block_on(async {
            let mut pins = vec![
                persona_pin(1, ANONYMOUS_ID, "account.webp"),
                persona_pin(2, ANONYMOUS_ID, "account.webp"),
            ];
            pins[1].create_time = 2;
            let mut calls = Vec::new();
            let verified = hydrate_pin_sources(&mut pins, ANONYMOUS_ID, |anchor| {
                calls.push(anchor);
                std::future::ready(Ok(None))
            })
            .await;
            assert_eq!(calls, [2]);
            assert!(verified.is_empty());
        });
    }

    #[test]
    fn regular_pins_keep_the_existing_single_latest_source_request() {
        futures::executor::block_on(async {
            for anonymous_id in [ANONYMOUS_ID, ""] {
                let mut pins = vec![
                    persona_pin(1, "42", "member.webp"),
                    persona_pin(2, "42", "member.webp"),
                ];
                pins[1].create_time = 2;
                let mut calls = Vec::new();
                let verified = hydrate_pin_sources(&mut pins, anonymous_id, |anchor| {
                    calls.push(anchor);
                    std::future::ready(Ok(Some(vec![persona_source(2, 42, "source.webp")])))
                })
                .await;
                assert_eq!(calls, [2]);
                assert_eq!(verified, HashSet::from([2]));
                assert_eq!(pins[0].avatar, "member.webp");
                assert_eq!(pins[1].avatar, "member.webp");
            }
        });
    }

    #[test]
    fn source_failure_preserves_only_trusted_matching_personas_and_empty_source_overrides_them() {
        let record = persona_pin(42, ANONYMOUS_ID, "account.webp");
        let untrusted = pinned_from_api(record.clone(), None);
        let pin =
            pinned_from_verified_api(record.clone(), false, Some(&untrusted), None, ANONYMOUS_ID);
        assert!(pin.avatar_url.is_empty());
        let mut trusted = pinned_from_last_pin_event(
            &LastPinMessageEvent {
                message_id: 42,
                message_sender_id: ANONYMOUS_ID.into(),
                message_sender_avatar: "trusted.webp".into(),
                ..Default::default()
            },
            None,
        );
        let pin =
            pinned_from_verified_api(record.clone(), false, Some(&trusted), None, ANONYMOUS_ID);
        assert_eq!(pin.avatar_url, "trusted.webp");
        assert!(pin.avatar_from_source);
        let empty = persona_pin(42, ANONYMOUS_ID, "");
        let pin = pinned_from_verified_api(empty, true, Some(&trusted), None, ANONYMOUS_ID);
        assert!(pin.avatar_url.is_empty());
        assert!(pin.avatar_proxied.is_empty());
        assert!(pin.avatar_from_source);
        trusted.message_id = "other".into();
        assert!(
            pinned_from_verified_api(record, false, Some(&trusted), None, ANONYMOUS_ID)
                .avatar_url
                .is_empty()
        );
        let normal = pinned_from_verified_api(
            persona_pin(5, "42", "member.webp"),
            false,
            None,
            None,
            ANONYMOUS_ID,
        );
        assert_eq!(normal.avatar_url, "member.webp");
    }

    #[gpui::test]
    fn anonymous_pin_avatar_never_resolves_a_shared_profile(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            anonymous_config(cx);
            for avatar in [
                "https://cdn.example/persona-a.webp",
                "https://cdn.example/persona-b.webp",
                "",
            ] {
                for (clan_id, channel_id) in [
                    (Some(ClanId(1)), Some(ChannelId(2))),
                    (Some(ClanId(3)), Some(ChannelId(4))),
                    (None, Some(ChannelId(5))),
                    (None, None),
                ] {
                    assert_eq!(
                        resolve_pin_avatar_url(ANONYMOUS_ID, avatar, clan_id, channel_id, cx),
                        avatar
                    );
                }
            }
            for sender_id in ["", "0", "invalid", "42"] {
                assert_eq!(
                    resolve_pin_avatar_url(sender_id, "message.webp", None, None, cx),
                    "message.webp"
                );
            }
        });
    }

    #[gpui::test]
    fn anonymous_pin_persona_survives_event_mapping_message_merge_and_api_mapping(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            anonymous_config(cx);
            let mut pin = pinned_from_last_pin_event(
                &LastPinMessageEvent {
                    message_id: 42,
                    message_sender_id: ANONYMOUS_ID.into(),
                    message_sender_username: "persona".into(),
                    message_sender_avatar: "event.webp".into(),
                    ..Default::default()
                },
                None,
            );
            assert_eq!(
                resolve_pin_avatar_url(&pin.sender_id, &pin.avatar_url, Some(ClanId(1)), None, cx),
                "event.webp"
            );
            let edited = Message::new(MessageId(42), "edited", ANONYMOUS_ID, "persona", 1)
                .with_avatar("updated.webp");
            merge_pinned_message_update(&mut pin, &edited);
            let cached = pin.clone();
            assert_eq!(cached.avatar_url, "updated.webp");
            assert_eq!(cached.content, "edited");
            let mut records = vec![persona_pin(42, ANONYMOUS_ID, "wrong-profile.webp")];
            let source = persona_source(42, ANONYMOUS_ID.parse().unwrap(), "updated.webp");
            apply_pin_sources(&mut records, std::slice::from_ref(&source), ANONYMOUS_ID);
            apply_pin_sources(&mut records, &[source], ANONYMOUS_ID);
            let restored = pinned_from_api(records.remove(0), None);
            assert_eq!(restored.sender_name, "persona");
            assert_eq!(
                resolve_pin_avatar_url(
                    &restored.sender_id,
                    &restored.avatar_url,
                    Some(ClanId(3)),
                    None,
                    cx
                ),
                cached.avatar_url
            );
        });
    }

    #[test]
    fn pin_source_repair_distinguishes_absent_messages_from_empty_avatars() {
        let mut pins = vec![
            persona_pin(1, ANONYMOUS_ID, "old.webp"),
            persona_pin(2, "42", "member.webp"),
            persona_pin(3, ANONYMOUS_ID, "shared-account.webp"),
            persona_pin(4, ANONYMOUS_ID, "unavailable.webp"),
        ];
        let sources = vec![
            persona_source(1, ANONYMOUS_ID.parse().unwrap(), "persona.webp"),
            persona_source(2, 42, "account.webp"),
            persona_source(3, ANONYMOUS_ID.parse().unwrap(), ""),
            persona_source(999, ANONYMOUS_ID.parse().unwrap(), "unrelated.webp"),
        ];
        apply_pin_sources(&mut pins, &sources, "");
        assert_eq!(pins[0].avatar, "old.webp");
        apply_pin_sources(&mut pins, &sources, ANONYMOUS_ID);
        let avatars: Vec<_> = pins
            .into_iter()
            .map(|pin| pinned_from_api(pin, None).avatar_url)
            .collect();
        assert_eq!(
            avatars,
            ["persona.webp", "member.webp", "", "unavailable.webp"]
        );
        let mut pins = vec![persona_pin(1, ANONYMOUS_ID, "")];
        let mut source = persona_source(1, ANONYMOUS_ID.parse().unwrap(), "late.webp");
        source.sender_name.clear();
        apply_pin_sources(&mut pins, &[source], ANONYMOUS_ID);
        let pin = pinned_from_api(pins.remove(0), None);
        assert_eq!(pin.avatar_url, "late.webp");
        assert_eq!(pin.sender_name, "persona");
    }

    #[gpui::test]
    fn anonymous_pin_source_empty_clears_raw_and_proxied_avatar(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            anonymous_config(cx);
            let mut records = vec![persona_pin(42, ANONYMOUS_ID, "shared-account.webp")];
            let previous = pinned_from_api(records[0].clone(), AppConfig::try_global(cx));
            assert!(!previous.avatar_url.is_empty());
            assert!(!previous.avatar_proxied.is_empty());
            let source = persona_source(42, ANONYMOUS_ID.parse().unwrap(), "");
            apply_pin_sources(&mut records, std::slice::from_ref(&source), ANONYMOUS_ID);
            apply_pin_sources(&mut records, &[source], ANONYMOUS_ID);
            let pin = pinned_from_verified_api(
                records[0].clone(),
                true,
                None,
                AppConfig::try_global(cx),
                ANONYMOUS_ID,
            );
            assert!(pin.avatar_url.is_empty());
            assert!(pin.avatar_proxied.is_empty());
            assert!(pin.has_authoritative_empty_avatar(cx));
            assert!(
                resolve_pin_avatar_url(
                    &pin.sender_id,
                    &pin.avatar_url,
                    Some(ClanId(1)),
                    Some(ChannelId(2)),
                    cx,
                )
                .is_empty()
            );
            let cached = pin.clone();
            let remapped = pinned_from_verified_api(
                records.remove(0),
                true,
                None,
                AppConfig::try_global(cx),
                ANONYMOUS_ID,
            );
            assert_eq!(remapped.avatar_url, cached.avatar_url);
            assert_eq!(remapped.avatar_proxied, cached.avatar_proxied);
            assert_eq!(remapped.sender_id, ANONYMOUS_ID);
        });
    }

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
    fn failed_refresh_never_confirms_an_optimistic_snapshot() {
        let mut state = PinFetchState::default();
        state.confirm();
        state.invalidate();
        let failed = state.start().unwrap();
        assert_eq!(state.finish(failed), PinFetchCompletion::Apply);
        assert!(!state.confirmed);

        state.invalidate();
        let successful = state.start().unwrap();
        assert_eq!(state.finish(successful), PinFetchCompletion::Apply);
        state.confirm();
        assert!(state.confirmed);
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
        apply_pin_sources(
            &mut pins,
            &[
                source(43, "https://cdn.example/godzilla.jpg"),
                source(44, ""),
                source(42, "https://cdn.example/portrait.png"),
                source(999, "https://cdn.example/unrelated.jpg"),
            ],
            "",
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
    fn embed_only_pin_provides_a_compact_preview() {
        let pin = pinned_from_api(
            ApiPinMessage {
                id: "1".into(),
                message_id: "42".into(),
                content: r#"{"embed":[{"title":"BẢN TÓM TẮT CUỘC HỘI THOẠI","description":"Thursday, Oct 8, 2026"}]}"#.into(),
                content_text: String::new(),
                sender_id: "7".into(),
                sender_name: "pm-assistant-bot".into(),
                avatar: String::new(),
                create_time: 0,
                attachments: Vec::new(),
            },
            None,
        );
        assert_eq!(
            pin.compact_preview_text().as_deref(),
            Some("BẢN TÓM TẮT CUỘC HỘI THOẠI Thursday, Oct 8, 2026")
        );
    }

    #[test]
    fn compact_pin_preview_keeps_plain_text_behavior() {
        let pin = pinned_from_api(
            ApiPinMessage {
                id: "1".into(),
                message_id: "42".into(),
                content: r#"{"t":"hello"}"#.into(),
                content_text: String::new(),
                sender_id: "7".into(),
                sender_name: "user".into(),
                avatar: String::new(),
                create_time: 0,
                attachments: Vec::new(),
            },
            None,
        );
        assert_eq!(pin.compact_preview_text().as_deref(), Some("hello"));
    }

    #[test]
    fn compact_pin_preview_uses_embed_description_when_title_is_empty() {
        let pin = pinned_from_api(
            ApiPinMessage {
                id: "1".into(),
                message_id: "42".into(),
                content: r#"{"embed":[{"title":"","description":"Meeting summary"}]}"#.into(),
                content_text: String::new(),
                sender_id: "7".into(),
                sender_name: "pm-assistant-bot".into(),
                avatar: String::new(),
                create_time: 0,
                attachments: Vec::new(),
            },
            None,
        );
        assert_eq!(
            pin.compact_preview_text().as_deref(),
            Some("Meeting summary")
        );
    }

    #[test]
    fn compact_pin_preview_stays_empty_for_attachment_only_pin() {
        let pin = pinned_from_api(
            ApiPinMessage {
                id: "1".into(),
                message_id: "42".into(),
                content: r#"{"t":""}"#.into(),
                content_text: String::new(),
                sender_id: "7".into(),
                sender_name: "user".into(),
                avatar: String::new(),
                create_time: 0,
                attachments: vec![mezon_client::transport::ApiAttachment {
                    url: "https://cdn.example/image.png".into(),
                    filetype: "image/png".into(),
                    ..Default::default()
                }],
            },
            None,
        );
        assert_eq!(pin.compact_preview_text(), None);
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

    fn cached_pin(content: &str) -> PinnedMessage {
        let mut pin = pinned_from_api(
            ApiPinMessage {
                id: "pin-9".into(),
                message_id: "42".into(),
                content: format!(r#"{{"t":"{content}"}}"#),
                content_text: content.into(),
                sender_id: "7".into(),
                sender_name: "user".into(),
                avatar: String::new(),
                create_time: 5,
                attachments: Vec::new(),
            },
            None,
        );
        pin.raw_content = content.into();
        pin
    }

    #[test]
    fn message_update_replaces_content_without_changing_pin_identity() {
        let mut pin = cached_pin("before");
        let mut edited = Message::new(MessageId(42), "after", "7", "user", 10);
        edited.raw_content = Some(Arc::from("after-wire"));

        merge_pinned_message_update(&mut pin, &edited);
        assert_eq!(pin.id, "pin-9");
        assert_eq!(pin.message_id, "42");
        assert_eq!(pin.create_time, 5);
        assert_eq!(pin.content, "after");
        assert_eq!(pin.raw_content, "after-wire");
    }

    #[test]
    fn sparse_update_preserves_pin_attachments() {
        let mut pin = cached_pin("before");
        pin.attachments = vec![MessageAttachment {
            url: "https://cdn.example/cached.png".into(),
            ..Default::default()
        }];
        let edited = Message::new(MessageId(42), "after", "7", "user", 10);

        merge_pinned_message_update(&mut pin, &edited);
        assert_eq!(pin.attachments[0].url, "https://cdn.example/cached.png");
    }

    #[test]
    fn source_verified_attachments_are_not_replaced_by_live_cache() {
        let mut pin = cached_pin("before");
        pin.attachments_from_source = true;
        pin.attachments = vec![MessageAttachment {
            url: "https://cdn.example/verified.png".into(),
            ..Default::default()
        }];
        let mut edited = Message::new(MessageId(42), "after", "7", "user", 10);
        edited.attachments = vec![MessageAttachment {
            url: "https://cdn.example/live.png".into(),
            ..Default::default()
        }];

        merge_pinned_message_update(&mut pin, &edited);
        assert_eq!(pin.attachments[0].url, "https://cdn.example/verified.png");
    }
}
