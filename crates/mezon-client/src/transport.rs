/// Main Mezon transport client with WebSocket/TCP support and REST API methods.
///
/// Handles connection management, message routing, and provides typed API methods
/// for interacting with the Mezon backend.
pub use crate::transport_adapter::TransportAdapter;
use anyhow::{Context, Result};
use mezon_proto::{api, realtime};
use parking_lot::Mutex;
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

const DEFAULT_SEND_TIMEOUT_MS: u64 = 10000;
const DEFAULT_CONNECT_GATE_MS: u64 = 5000;
const DEFAULT_PING_TIMEOUT_MS: u64 = 5000;
const MULTIPART_OP_TIMEOUT_MS: u64 = 120000;

fn parse_id<T>(value: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|e| anyhow::anyhow!("invalid id {value:?}: {e}"))
}

/// Promise executor for matching responses to requests.
struct PromiseExecutor {
    sender: oneshot::Sender<(u32, Vec<u8>)>,
}

/// Represents real-time events pushed from the server.
#[derive(Debug, Clone)]
pub enum RealtimeEvent {
    ChannelMessage(api::ChannelMessage),
    MessageTyping(realtime::MessageTypingEvent),
    ChannelPresence(realtime::ChannelPresenceEvent),
    StatusPresence(realtime::StatusPresenceEvent),
    CustomStatus(realtime::CustomStatusEvent),
    MessageReaction(api::MessageReaction),
    MarkAsRead(realtime::MarkAsRead),
    LastSeenUpdated(realtime::LastSeenMessageEvent),
    Notifications(realtime::Notifications),
    ChannelCreated(realtime::ChannelCreatedEvent),
    ChannelUpdated(realtime::ChannelUpdatedEvent),
    ChannelDeleted(realtime::ChannelDeletedEvent),
    ChannelArchive(realtime::ChannelArchiveEvent),
    CategoryEvent(realtime::CategoryEvent),
    Unmute(realtime::UnmuteEvent),
    StreamingJoined(realtime::StreamingJoinedEvent),
    StreamingLeaved(realtime::StreamingLeavedEvent),
    VoiceStarted(realtime::VoiceStartedEvent),
    VoiceEnded(realtime::VoiceEndedEvent),
    VoiceJoined(realtime::VoiceJoinedEvent),
    VoiceLeaved(realtime::VoiceLeavedEvent),
    VoiceReaction(realtime::VoiceReactionSend),
    UserChannelAdded(realtime::UserChannelAdded),
    UserChannelRemoved(realtime::UserChannelRemoved),
    AddClanUser(realtime::AddClanUserEvent),
    UserClanRemoved(realtime::UserClanRemoved),
    ClanUpdated(realtime::ClanUpdatedEvent),
    ClanProfileUpdated(realtime::ClanProfileUpdatedEvent),
    ClanDeleted(realtime::ClanDeletedEvent),
    ClanEmoji(realtime::EventEmoji),
    AddFriend(realtime::AddFriend),
    RemoveFriend(realtime::RemoveFriend),
    /// Server-pushed session refresh over the socket (`refresh_session_event`, field 96).
    /// The native equivalent of mezon-js `client.onrefreshsession`.
    SessionRefreshed(api::Session),
    LastPinMessage(realtime::LastPinMessageEvent),
    UnpinMessage(realtime::UnpinMessageEvent),
    Unhandled(realtime::envelope::Message),
}

impl TryFrom<realtime::envelope::Message> for RealtimeEvent {
    type Error = &'static str;

    fn try_from(msg: realtime::envelope::Message) -> Result<Self, Self::Error> {
        match msg {
            realtime::envelope::Message::ChannelMessage(m) => Ok(Self::ChannelMessage(m)),
            realtime::envelope::Message::MessageTypingEvent(m) => Ok(Self::MessageTyping(m)),
            realtime::envelope::Message::ChannelPresenceEvent(m) => Ok(Self::ChannelPresence(m)),
            realtime::envelope::Message::StatusPresenceEvent(m) => Ok(Self::StatusPresence(m)),
            realtime::envelope::Message::CustomStatusEvent(m) => Ok(Self::CustomStatus(m)),
            realtime::envelope::Message::MessageReactionEvent(m) => Ok(Self::MessageReaction(m)),
            realtime::envelope::Message::MarkAsRead(m) => Ok(Self::MarkAsRead(m)),
            realtime::envelope::Message::LastSeenMessageEvent(m) => Ok(Self::LastSeenUpdated(m)),
            realtime::envelope::Message::Notifications(m) => Ok(Self::Notifications(m)),
            realtime::envelope::Message::ChannelCreatedEvent(m) => Ok(Self::ChannelCreated(m)),
            realtime::envelope::Message::ChannelUpdatedEvent(m) => Ok(Self::ChannelUpdated(m)),
            realtime::envelope::Message::ChannelDeletedEvent(m) => Ok(Self::ChannelDeleted(m)),
            realtime::envelope::Message::ChannelArchiveEvent(m) => Ok(Self::ChannelArchive(m)),
            realtime::envelope::Message::CategoryEvent(m) => Ok(Self::CategoryEvent(m)),
            realtime::envelope::Message::UnmuteEvent(m) => Ok(Self::Unmute(m)),
            realtime::envelope::Message::StreamingJoinedEvent(m) => Ok(Self::StreamingJoined(m)),
            realtime::envelope::Message::StreamingLeavedEvent(m) => Ok(Self::StreamingLeaved(m)),
            realtime::envelope::Message::VoiceStartedEvent(m) => Ok(Self::VoiceStarted(m)),
            realtime::envelope::Message::VoiceEndedEvent(m) => Ok(Self::VoiceEnded(m)),
            realtime::envelope::Message::VoiceJoinedEvent(m) => Ok(Self::VoiceJoined(m)),
            realtime::envelope::Message::VoiceLeavedEvent(m) => Ok(Self::VoiceLeaved(m)),
            realtime::envelope::Message::VoiceReactionSend(m) => Ok(Self::VoiceReaction(m)),
            realtime::envelope::Message::UserChannelAddedEvent(m) => Ok(Self::UserChannelAdded(m)),
            realtime::envelope::Message::UserChannelRemovedEvent(m) => {
                Ok(Self::UserChannelRemoved(m))
            }
            realtime::envelope::Message::AddClanUserEvent(m) => Ok(Self::AddClanUser(m)),
            realtime::envelope::Message::UserClanRemovedEvent(m) => Ok(Self::UserClanRemoved(m)),
            realtime::envelope::Message::ClanUpdatedEvent(m) => Ok(Self::ClanUpdated(m)),
            realtime::envelope::Message::ClanProfileUpdatedEvent(m) => {
                Ok(Self::ClanProfileUpdated(m))
            }
            realtime::envelope::Message::ClanDeletedEvent(m) => Ok(Self::ClanDeleted(m)),
            realtime::envelope::Message::EventEmoji(m) => Ok(Self::ClanEmoji(m)),
            realtime::envelope::Message::AddFriend(m) => Ok(Self::AddFriend(m)),
            realtime::envelope::Message::RemoveFriend(m) => Ok(Self::RemoveFriend(m)),
            realtime::envelope::Message::RefreshSessionEvent(s) => Ok(Self::SessionRefreshed(s)),
            realtime::envelope::Message::LastPinMessageEvent(m) => Ok(Self::LastPinMessage(m)),
            realtime::envelope::Message::UnpinMessageEvent(m) => Ok(Self::UnpinMessage(m)),
            other => Ok(Self::Unhandled(other)),
        }
    }
}

fn dispatch_realtime_push(
    cid: u16,
    payload: &[u8],
    on_event: &(dyn Fn(RealtimeEvent) + Send + Sync),
) {
    match realtime::Envelope::decode(payload) {
        Ok(envelope) => match envelope.message {
            Some(msg) => {
                tracing::debug!("server push (cid={cid}) -> publishing realtime event");
                if let Ok(event) = RealtimeEvent::try_from(msg) {
                    on_event(event);
                }
            }
            None => tracing::warn!("server push (cid={cid}): envelope has no message"),
        },
        Err(e) => tracing::warn!(
            "server push (cid={cid}) decode failed (len={}): {e}",
            payload.len()
        ),
    }
}

/// Main transport client.
pub struct MezonTransport {
    adapter: Arc<dyn TransportAdapter>,
    cid_counter: Arc<AtomicU16>,
    pending_requests: Arc<Mutex<HashMap<u16, PromiseExecutor>>>,
    send_timeout_ms: Duration,
    connect_gate: Duration,
    connected_tx: watch::Sender<bool>,
    connected_rx: watch::Receiver<bool>,
    #[allow(dead_code)]
    base_path: String,
}

impl MezonTransport {
    /// Create a new transport with the given adapter.
    pub fn new(adapter: Box<dyn TransportAdapter>, base_path: String) -> Self {
        let (connected_tx, connected_rx) = watch::channel(false);
        Self {
            adapter: Arc::from(adapter),
            cid_counter: Arc::new(AtomicU16::new(1)),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            send_timeout_ms: Duration::from_millis(DEFAULT_SEND_TIMEOUT_MS),
            connect_gate: Duration::from_millis(DEFAULT_CONNECT_GATE_MS),
            connected_tx,
            connected_rx,
            base_path,
        }
    }

    /// Set request timeout.
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.send_timeout_ms = Duration::from_millis(timeout_ms);
    }

    /// Generate a unique correlation ID.
    fn generate_cid(&self) -> u16 {
        loop {
            let cid = self.cid_counter.fetch_add(1, Ordering::SeqCst);
            if cid != 0 {
                return cid;
            }
        }
    }

    async fn wait_connected(&self, deadline: Duration) -> Result<()> {
        let mut rx = self.connected_rx.clone();
        if *rx.borrow_and_update() {
            return Ok(());
        }
        match tokio::time::timeout(deadline, async move {
            while rx.changed().await.is_ok() {
                if *rx.borrow_and_update() {
                    return true;
                }
            }
            false
        })
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(anyhow::anyhow!("connection signal closed")),
            Err(_) => Err(anyhow::anyhow!("not connected (gate timed out)")),
        }
    }

    /// Connect to the Mezon backend.
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        token: &str,
        on_event: impl Fn(RealtimeEvent) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::debug!("MezonTransport::connect() starting");
        tracing::debug!("  Target: {host}:{port}");

        // Set up message handler
        tracing::debug!("Setting up message handler...");
        let pending_requests = self.pending_requests.clone();
        let on_event: Arc<dyn Fn(RealtimeEvent) + Send + Sync> = Arc::new(on_event);
        self.adapter
            .set_on_message(Arc::new(move |cid, code, message| {
                tracing::trace!("on_message: cid={cid} code={code} len={}", message.len());

                if cid != 0 {
                    let executor = pending_requests.lock().remove(&cid);
                    match executor {
                        Some(executor) => {
                            let _ = executor.sender.send((code, message));
                        }
                        None => dispatch_realtime_push(cid, &message, on_event.as_ref()),
                    }
                } else {
                    dispatch_realtime_push(cid, &message, on_event.as_ref());
                }
            }))
            .await;
        tracing::debug!("  Message handler set");

        let connected_for_open = self.connected_tx.clone();
        self.adapter
            .set_on_open(Arc::new(move || {
                let _ = connected_for_open.send(true);
            }))
            .await;

        // Set up close handler
        tracing::debug!("Setting up close handler...");
        let pending_for_close = self.pending_requests.clone();
        let connected_for_close = self.connected_tx.clone();
        self.adapter
            .set_on_close(Arc::new(move |was_clean| {
                let _ = connected_for_close.send(false);
                pending_for_close.lock().clear();
                on_disconnected(was_clean);
            }))
            .await;
        tracing::debug!("  Close handler set");

        self.adapter
            .set_on_error(Arc::new(|err| {
                tracing::warn!("realtime transport error: {err}");
            }))
            .await;

        // Connect
        tracing::debug!("Calling adapter.connect()...");
        self.adapter
            .connect(host, port, token)
            .await
            .with_context(|| format!("Failed to connect adapter to {host}:{port}"))?;

        tracing::debug!("MezonTransport::connect() completed successfully");
        Ok(())
    }

    /// Send a raw message and wait for response.
    pub async fn send(&self, cid: u16, message: Vec<u8>) -> Result<(u32, Vec<u8>)> {
        self.send_with_timeout(cid, message, self.send_timeout_ms)
            .await
    }

    /// Send a raw message and wait for response, using an explicit timeout.
    pub async fn send_with_timeout(
        &self,
        cid: u16,
        message: Vec<u8>,
        timeout: Duration,
    ) -> Result<(u32, Vec<u8>)> {
        self.wait_connected(self.connect_gate).await?;
        let (tx, rx) = oneshot::channel();
        self.pending_requests
            .lock()
            .insert(cid, PromiseExecutor { sender: tx });
        if let Err(e) = self.adapter.send(message).await {
            self.pending_requests.lock().remove(&cid);
            return Err(e);
        }
        let result = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                self.pending_requests.lock().remove(&cid);
                anyhow::anyhow!("Request timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?;
        Ok(result)
    }

    /// Check if the adapter is connected.
    pub async fn is_open(&self) -> bool {
        self.adapter.is_open()
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<()> {
        let _ = self.connected_tx.send(false);
        self.pending_requests.lock().clear();
        self.adapter.close().await
    }

    /// Send a ping.
    pub async fn ping(&self, cid: u16) -> Result<()> {
        self.adapter.send_ping(cid).await
    }

    /// Send a ping and wait for matching pong.
    pub async fn ping_roundtrip(&self) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!("MezonTransport::ping_roundtrip() cid={}", cid);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock();
            pending.insert(cid, PromiseExecutor { sender: tx });
            tracing::debug!(
                "  Registered ping pending request. Total pending: {}",
                pending.len()
            );
        }

        tracing::debug!("Sending ping cid={}", cid);
        if let Err(e) = self.adapter.send_ping(cid).await {
            self.pending_requests.lock().remove(&cid);
            return Err(e);
        }

        tokio::time::timeout(Duration::from_millis(DEFAULT_PING_TIMEOUT_MS), rx)
            .await
            .map_err(|_| {
                tracing::error!("Ping timed out after {} ms", DEFAULT_PING_TIMEOUT_MS);
                self.pending_requests.lock().remove(&cid);
                anyhow::anyhow!("Ping timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Ping response channel closed"))?;

        tracing::debug!("Pong received for cid={}", cid);
        Ok(())
    }
}

// ============================================================================
// API Methods - Hot Path (frequently called)
// ============================================================================

/// API response types (simplified - expand as needed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiAccount {
    pub user_id: i64,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub about_me: Option<String>,
    pub phone_number: Option<String>,
    pub password_setted: bool,
    pub logo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSession {
    pub token: String,
    pub refresh_token: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiChannelDesc {
    pub channel_id: i64,
    pub channel_label: String,
    pub channel_type: u32,
    pub clan_id: i64,
    pub category_name: String,
    pub category_id: i64,
    pub channel_private: i32,
    pub count_mess_unread: i32,
    pub member_count: i32,
    pub parent_id: i64,
    pub is_mute: bool,
    pub last_seen_message_id: i64,
    pub last_seen_timestamp: i64,
    pub last_sent_message_id: i64,
    pub last_sent_timestamp: i64,
    pub badge_count: i32,
    #[serde(default)]
    pub creator_id: i64,
}

/// A direct-message / group conversation descriptor (clan_id = 0 namespace). Unlike
/// [`ApiChannelDesc`] this carries the DM participant arrays the UI needs to render rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDirectChannel {
    pub channel_id: i64,
    pub channel_label: String,
    /// Raw channel type: 2 = group, 3 = 1-1 DM.
    pub channel_type: u32,
    pub channel_avatar: String,
    pub avatars: Vec<String>,
    pub usernames: Vec<String>,
    pub display_names: Vec<String>,
    pub user_ids: Vec<i64>,
    pub onlines: Vec<bool>,
    pub member_count: i32,
    pub count_mess_unread: i32,
    pub last_sent_timestamp: i64,
    pub last_seen_timestamp: i64,
    #[serde(default)]
    pub creator_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCategoryDesc {
    pub category_id: i64,
    pub category_name: String,
    pub clan_id: i64,
    pub category_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiVoiceChannelUser {
    pub channel_id: i64,
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiChannelApp {
    pub app_id: String,
    pub app_name: String,
    pub app_logo: Option<String>,
    pub app_url: String,
    pub channel_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClanDesc {
    pub clan_id: i64,
    pub clan_name: String,
    pub creator_id: i64,
    pub logo: String,
    pub banner: String,
    pub welcome_channel_id: i64,
    pub status: i32,
    pub is_onboarding: bool,
    pub is_community: bool,
    pub prevent_anonymous: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: i32,
    pub height: i32,
    pub thumbnail: String,
    pub duration: i32,
    pub size: i32,
}

/// A channel attachment record returned by `ListChannelAttachment` (gallery /
/// image viewer). Richer than [`ApiAttachment`] — it carries the uploader, the
/// originating message, and the creation timestamp used for date grouping and
/// pagination cursors. Mirrors React's `ApiChannelAttachment` / `IAttachmentEntity`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiChannelAttachment {
    pub id: i64,
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub filesize: String,
    pub width: i32,
    pub height: i32,
    pub uploader: i64,
    pub message_id: i64,
    pub create_time_seconds: u32,
}

impl ApiChannelAttachment {
    fn from_proto(a: api::ChannelAttachment) -> Self {
        Self {
            id: a.id,
            url: a.url,
            filename: a.filename,
            filetype: a.filetype,
            filesize: a.filesize,
            width: a.width,
            height: a.height,
            uploader: a.uploader,
            message_id: a.message_id,
            create_time_seconds: a.create_time_seconds,
        }
    }
}

fn parse_message_attachments(bytes: &[u8]) -> Vec<ApiAttachment> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::MessageAttachmentList::decode(bytes) {
        Ok(list) => list
            .attachments
            .into_iter()
            .filter(|a| !a.url.is_empty())
            .map(api_attachment_from_proto)
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode message attachments ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

fn api_attachment_from_proto(a: api::MessageAttachment) -> ApiAttachment {
    ApiAttachment {
        url: a.url,
        filename: a.filename,
        filetype: a.filetype,
        width: a.width,
        height: a.height,
        thumbnail: a.thumbnail,
        duration: a.duration,
        size: a.size,
    }
}

pub fn parse_search_attachment_field(raw: &str) -> Vec<ApiAttachment> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        let parsed = parse_search_attachment_json_value(&value);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, trimmed) {
        let decoded = parse_message_attachments(&bytes);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    parse_message_attachments(trimmed.as_bytes())
}

pub fn parse_search_mentions_field(raw: &str) -> Vec<ApiEntityMention> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        let parsed = parse_search_mentions_json_value(&value);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, trimmed) {
        let decoded = parse_message_mentions(&bytes);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    parse_message_mentions(trimmed.as_bytes())
}

fn parse_search_mentions_json_value(value: &serde_json::Value) -> Vec<ApiEntityMention> {
    let items = if let Some(array) = value.as_array() {
        array.clone()
    } else if let Some(array) = value.get("mentions").and_then(|v| v.as_array()) {
        array.clone()
    } else if value.is_object() {
        vec![value.clone()]
    } else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(parse_search_mention_json_item)
        .collect()
}

fn parse_search_mention_json_item(item: &serde_json::Value) -> Option<ApiEntityMention> {
    let s = item.get("s").and_then(json_to_i32).unwrap_or(0);
    let e = item.get("e").and_then(json_to_i32)?;
    if e <= s {
        return None;
    }
    let user_id = item
        .get("user_id")
        .or_else(|| item.get("userId"))
        .and_then(json_to_i64)
        .unwrap_or_default();
    let role_id = item
        .get("role_id")
        .or_else(|| item.get("roleId"))
        .and_then(json_to_i64)
        .unwrap_or_default();
    let username = item
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Some(ApiEntityMention {
        user_id,
        role_id,
        username,
        s,
        e,
    })
}

fn json_to_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|v| v as i64))
        .or_else(|| value.as_f64().map(|v| v as i64))
        .or_else(|| value.as_str()?.parse().ok())
}

fn parse_search_attachment_json_value(value: &serde_json::Value) -> Vec<ApiAttachment> {
    let items = if let Some(array) = value.as_array() {
        array.clone()
    } else if let Some(array) = value.get("attachments").and_then(|v| v.as_array()) {
        array.clone()
    } else if value.is_object() {
        vec![value.clone()]
    } else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(parse_search_attachment_json_item)
        .collect()
}

fn parse_search_attachment_json_item(item: &serde_json::Value) -> Option<ApiAttachment> {
    let url = item.get("url").and_then(|v| v.as_str())?;
    if url.is_empty() {
        return None;
    }
    Some(ApiAttachment {
        url: url.to_string(),
        filename: item
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        filetype: item
            .get("filetype")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        width: item.get("width").and_then(json_to_i32).unwrap_or_default(),
        height: item.get("height").and_then(json_to_i32).unwrap_or_default(),
        thumbnail: item
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        duration: item
            .get("duration")
            .and_then(json_to_i32)
            .unwrap_or_default(),
        size: item.get("size").and_then(json_to_i32).unwrap_or_default(),
    })
}

fn json_to_i32(value: &serde_json::Value) -> Option<i32> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .and_then(|n| i32::try_from(n).ok())
}

fn parse_message_text(content: &str) -> String {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("t").and_then(|t| t.as_str().map(|s| s.to_string())))
        .unwrap_or_else(|| content.to_string())
}

pub fn prioritize_avatar(clan_avatar: &str, user_avatar: &str) -> String {
    if !clan_avatar.is_empty() {
        clan_avatar.to_string()
    } else {
        user_avatar.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEntityMention {
    pub user_id: i64,
    pub role_id: i64,
    pub username: String,
    pub s: i32,
    pub e: i32,
}

pub fn parse_message_mentions(bytes: &[u8]) -> Vec<ApiEntityMention> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::MessageMentionList::decode(bytes) {
        Ok(list) => list
            .mentions
            .into_iter()
            .map(|m| ApiEntityMention {
                user_id: m.user_id,
                role_id: m.role_id,
                username: m.username,
                s: m.s,
                e: m.e,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode message mentions ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

fn entity_mention_targets_user(m: &ApiEntityMention, user_id: i64, role_ids: &[i64]) -> bool {
    if m.user_id != 0 {
        let uid = m.user_id.to_string();
        if is_here_user_id(&uid) || m.user_id == user_id {
            return true;
        }
    }
    m.role_id != 0 && role_ids.contains(&m.role_id)
}

pub fn enrich_content_tokens(tokens: &mut ApiMessageContent, entity_mentions: &[ApiEntityMention]) {
    for m in entity_mentions {
        if m.e <= m.s {
            continue;
        }
        let user_id = (m.user_id != 0).then(|| m.user_id.to_string());
        let role_id = (m.role_id != 0).then(|| m.role_id.to_string());
        let duplicate = tokens.mentions.iter().any(|tok| {
            tok.s == Some(i64::from(m.s))
                && tok.e == Some(i64::from(m.e))
                && tok.user_id == user_id
                && tok.role_id == role_id
        });
        if duplicate {
            continue;
        }
        tokens.mentions.push(ContentToken {
            s: Some(i64::from(m.s)),
            e: Some(i64::from(m.e)),
            user_id,
            role_id,
            username: (!m.username.is_empty()).then(|| m.username.clone()),
            ..Default::default()
        });
    }
    tokens.mentions.sort_by_key(|tok| tok.s.unwrap_or(i64::MAX));
}

fn parse_message_references(bytes: &[u8]) -> Vec<ApiMessageRef> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::MessageRefList::decode(bytes) {
        Ok(list) => list
            .refs
            .into_iter()
            .map(|r| ApiMessageRef {
                message_ref_id: r.message_ref_id,
                content: r.content,
                has_attachment: r.has_attachment,
                message_sender_id: r.message_sender_id,
                message_sender_username: r.message_sender_username,
                message_sender_avatar: r.message_sender_avatar,
                message_sender_clan_nick: r.message_sender_clan_nick,
                message_sender_display_name: r.message_sender_display_name,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode message references ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

fn json_field_i64(value: &serde_json::Value, key: &str) -> i64 {
    match value.get(key) {
        Some(serde_json::Value::String(raw)) => raw.parse::<i64>().unwrap_or(0),
        Some(serde_json::Value::Number(num)) => num
            .as_i64()
            .or_else(|| num.as_f64().map(|seconds| seconds as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

pub fn parse_notification_content(content: &[u8]) -> (i64, i64) {
    if content.is_empty() {
        return (0, 0);
    }
    if !matches!(content.first().copied(), Some(b'{') | Some(b'['))
        && let Ok(fcm) = api::DirectFcmProto::decode(content)
    {
        return (fcm.message_id, i64::from(fcm.create_time_seconds));
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(content) else {
        return (0, 0);
    };
    (
        json_field_i64(&value, "message_id"),
        json_field_i64(&value, "create_time_seconds"),
    )
}

pub fn is_mention_or_reply(
    content: &str,
    references: &[u8],
    mention_bytes: &[u8],
    user_id: i64,
    role_ids: &[i64],
) -> bool {
    if parse_message_mentions(mention_bytes)
        .iter()
        .any(|m| entity_mention_targets_user(m, user_id, role_ids))
    {
        return true;
    }
    if let Ok(parsed) = serde_json::from_str::<ApiMessageContent>(content)
        && parsed
            .mentions
            .iter()
            .any(|token| mention_targets_user(token, user_id, role_ids))
    {
        return true;
    }
    parse_message_references(references)
        .iter()
        .any(|reference| reference.message_sender_id == user_id)
}

fn mention_targets_user(token: &ContentToken, user_id: i64, role_ids: &[i64]) -> bool {
    if let Some(uid) = token.user_id.as_deref()
        && (is_here_user_id(uid) || uid.parse::<i64>().is_ok_and(|id| id == user_id))
    {
        return true;
    }
    token
        .role_id
        .as_deref()
        .and_then(|rid| rid.parse::<i64>().ok())
        .is_some_and(|rid| role_ids.contains(&rid))
}

fn parse_message_reactions(bytes: &[u8]) -> Vec<ApiMessageReaction> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::MessageReactionList::decode(bytes) {
        Ok(list) => list
            .reactions
            .into_iter()
            .map(|r| ApiMessageReaction {
                emoji_id: r.emoji_id.to_string(),
                emoji: r.emoji,
                count: r.count.max(0) as u32,
                sender_id: r.sender_id.to_string(),
                action: r.action,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode message reactions ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

/// A single inline content token from a message's JSON `content` payload.
/// Mirrors the React `IExtendedMessage` token shape (`s`/`e` index ranges plus
/// kind-specific fields). All fields are optional because the wire payload only
/// includes what is relevant for that token kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentToken {
    #[serde(default, deserialize_with = "opt_i64_flex::deserialize")]
    pub s: Option<i64>,
    #[serde(default, deserialize_with = "opt_i64_flex::deserialize")]
    pub e: Option<i64>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub user_id: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub role_id: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub username: Option<String>,
    #[serde(
        default,
        rename = "channelId",
        deserialize_with = "string_or_number::deserialize"
    )]
    pub channel_id: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub emojiid: Option<String>,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "string_or_number::deserialize"
    )]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub image: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub banner: Option<String>,
    #[serde(default, deserialize_with = "opt_i64_flex::deserialize")]
    pub member_count: Option<i64>,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub is_community: bool,
    #[serde(
        default,
        rename = "clanId",
        deserialize_with = "string_or_number::deserialize"
    )]
    pub clan_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiPollAnswer {
    #[serde(default)]
    pub index: Option<i64>,
    #[serde(default)]
    pub label: String,
}

mod opt_i64_flex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::Number(n)) => Ok(n.as_i64()),
            Some(serde_json::Value::String(s)) => {
                if s.is_empty() {
                    Ok(None)
                } else {
                    Ok(s.parse::<i64>().ok())
                }
            }
            Some(_) => Ok(None),
        }
    }
}

mod opt_i32_flex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Number(n)) => Ok(n.as_i64().map(|v| v as i32)),
            Some(serde_json::Value::String(s)) if !s.is_empty() => Ok(s.parse::<i32>().ok()),
            _ => Ok(None),
        }
    }
}

mod i32_flex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i32, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Number(n)) => Ok(n.as_i64().map(|v| v as i32).unwrap_or(0)),
            Some(serde_json::Value::String(s)) => Ok(s.parse::<i32>().unwrap_or(0)),
            _ => Ok(0),
        }
    }
}

mod bool_flex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Bool(b)) => Ok(b),
            Some(serde_json::Value::String(s)) => Ok(s == "true" || s == "1"),
            Some(serde_json::Value::Number(n)) => Ok(n.as_i64() == Some(1)),
            _ => Ok(false),
        }
    }
}

mod vec_i32_flex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<i32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Array(arr)) => Ok(arr
                .into_iter()
                .filter_map(|v| match v {
                    serde_json::Value::Number(n) => n.as_i64().map(|x| x as i32),
                    serde_json::Value::String(s) => s.parse::<i32>().ok(),
                    _ => None,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }
}

mod vec_null_as_empty {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: DeserializeOwned,
    {
        Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
    }
}

mod vec_content_token_lenient {
    use super::ContentToken;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<ContentToken>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Option::<Vec<serde_json::Value>>::deserialize(deserializer)?.unwrap_or_default();
        Ok(raw
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect())
    }
}

mod map_null_as_empty {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer};
    use std::collections::HashMap;
    use std::hash::Hash;

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: DeserializeOwned + Eq + Hash,
        V: DeserializeOwned,
    {
        Ok(Option::<HashMap<K, V>>::deserialize(deserializer)?.unwrap_or_default())
    }
}

mod poll_answers {
    use super::ApiPollAnswer;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<ApiPollAnswer>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => return Ok(Vec::new()),
        };
        Ok(raw
            .into_iter()
            .map(|value| match value {
                serde_json::Value::String(label) => ApiPollAnswer { index: None, label },
                serde_json::Value::Object(map) => ApiPollAnswer {
                    index: map.get("index").and_then(serde_json::Value::as_i64),
                    label: map
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
                _ => ApiPollAnswer::default(),
            })
            .collect())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbedAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbedThumbnail {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbedImage {
    #[serde(default)]
    pub url: String,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub width: Option<i32>,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbedFooter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbedField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub inline: bool,
    #[serde(default)]
    pub inputs: Option<serde_json::Value>,
    #[serde(default)]
    pub shape: Option<serde_json::Value>,
    #[serde(default)]
    pub button: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiEmbed {
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub author: Option<ApiEmbedAuthor>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<ApiEmbedThumbnail>,
    #[serde(default, deserialize_with = "vec_null_as_empty::deserialize")]
    pub fields: Vec<ApiEmbedField>,
    #[serde(default)]
    pub image: Option<ApiEmbedImage>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub footer: Option<ApiEmbedFooter>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiSelectOption {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiButtonComponent {
    #[serde(default)]
    pub label: String,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub disable: bool,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub style: Option<i32>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiSelectComponent {
    #[serde(
        default,
        rename = "type",
        deserialize_with = "opt_i32_flex::deserialize"
    )]
    pub select_type: Option<i32>,
    #[serde(default)]
    pub options: Vec<ApiSelectOption>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub min_options: Option<i32>,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub max_options: Option<i32>,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub disabled: bool,
    #[serde(default, rename = "valueSelected")]
    pub value_selected: Option<ApiSelectOption>,
}

#[derive(Debug, Clone, Serialize)]
pub enum ApiComponentPayload {
    Button(ApiButtonComponent),
    Select(ApiSelectComponent),
    Other(serde_json::Value),
}

impl Default for ApiComponentPayload {
    fn default() -> Self {
        ApiComponentPayload::Other(serde_json::Value::Null)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ApiMessageComponent {
    pub component_type: i32,
    pub id: Option<String>,
    pub max_options: Option<i32>,
    pub component: ApiComponentPayload,
}

impl<'de> Deserialize<'de> for ApiMessageComponent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, rename = "type", deserialize_with = "i32_flex::deserialize")]
            component_type: i32,
            #[serde(default)]
            id: Option<String>,
            #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
            max_options: Option<i32>,
            #[serde(default)]
            component: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        let component = match raw.component_type {
            1 => ApiComponentPayload::Button(
                serde_json::from_value(raw.component).unwrap_or_default(),
            ),
            2 => ApiComponentPayload::Select(
                serde_json::from_value(raw.component).unwrap_or_default(),
            ),
            _ => ApiComponentPayload::Other(raw.component),
        };
        Ok(ApiMessageComponent {
            component_type: raw.component_type,
            id: raw.id,
            max_options: raw.max_options,
            component,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiActionRow {
    #[serde(default, deserialize_with = "vec_null_as_empty::deserialize")]
    pub components: Vec<ApiMessageComponent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiCallLog {
    #[serde(
        default,
        rename = "isVideo",
        deserialize_with = "bool_flex::deserialize"
    )]
    pub is_video: bool,
    #[serde(
        default,
        rename = "callLogType",
        deserialize_with = "i32_flex::deserialize"
    )]
    pub call_log_type: i32,
    #[serde(
        default,
        rename = "showCallBack",
        deserialize_with = "bool_flex::deserialize"
    )]
    pub show_call_back: bool,
}

/// Parsed `content` JSON of a message (the mezon `IExtendedMessage` shape).
/// `t` is the plain text; the token arrays carry inline rich-text ranges.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiMessageContent {
    #[serde(default)]
    pub t: String,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub mentions: Vec<ContentToken>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub hg: Vec<ContentToken>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub ej: Vec<ContentToken>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub mk: Vec<ContentToken>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub lk: Vec<ContentToken>,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub fwd: bool,
    #[serde(default, alias = "id", deserialize_with = "opt_i64_flex::deserialize")]
    pub poll_id: Option<i64>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default, deserialize_with = "poll_answers::deserialize")]
    pub answers: Vec<ApiPollAnswer>,
    #[serde(default, deserialize_with = "vec_i32_flex::deserialize")]
    pub answer_counts: Vec<i32>,
    #[serde(default, deserialize_with = "opt_i64_flex::deserialize")]
    pub expire_at: Option<i64>,
    #[serde(default, deserialize_with = "bool_flex::deserialize")]
    pub is_closed: bool,
    #[serde(default, deserialize_with = "opt_i32_flex::deserialize")]
    pub total_votes: Option<i32>,
    #[serde(default, rename = "type", deserialize_with = "opt_i64_flex::deserialize")]
    pub poll_type: Option<i64>,
    #[serde(default, deserialize_with = "vec_null_as_empty::deserialize")]
    pub embed: Vec<ApiEmbed>,
    #[serde(default, deserialize_with = "vec_null_as_empty::deserialize")]
    pub components: Vec<ApiActionRow>,
    #[serde(default, rename = "callLog")]
    pub call_log: Option<ApiCallLog>,
    #[serde(default, alias = "isCard", deserialize_with = "bool_flex::deserialize")]
    pub is_card: bool,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub tp: Option<String>,
    #[serde(default, deserialize_with = "string_or_number::deserialize")]
    pub cid: Option<String>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub vk: Vec<ContentToken>,
    #[serde(default, deserialize_with = "map_null_as_empty::deserialize")]
    pub cvtt: HashMap<String, String>,
    #[serde(default, deserialize_with = "vec_content_token_lenient::deserialize")]
    pub lky: Vec<ContentToken>,
    #[serde(default, skip_serializing)]
    pub presign_finish: Option<Vec<String>>,
}

/// A reply/reference attached to a message (mezon `MessageRef`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiMessageRef {
    pub message_ref_id: i64,
    pub content: String,
    pub has_attachment: bool,
    pub message_sender_id: i64,
    pub message_sender_username: String,
    pub message_sender_avatar: String,
    pub message_sender_clan_nick: String,
    pub message_sender_display_name: String,
}

/// A single emoji reaction entry (mezon `MessageReaction`). For list/realtime
/// payloads each entry is one user's reaction; the store aggregates by emoji.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiMessageReaction {
    pub emoji_id: String,
    pub emoji: String,
    pub count: u32,
    pub sender_id: String,
    /// Proto `action`: true when this entry represents a removal.
    pub action: bool,
}

/// Plain reply descriptor passed from the store to the transport so the store
/// does not depend on the protobuf types. Converted to `api::MessageRef` here.
#[derive(Debug, Clone, Default)]
pub struct OutgoingReply {
    pub message_ref_id: i64,
    pub content: String,
    pub has_attachment: bool,
    pub message_sender_id: i64,
    pub message_sender_username: String,
    pub message_sender_avatar: String,
    pub message_sender_clan_nick: String,
    pub message_sender_display_name: String,
}

pub const MENTION_HERE_ID: &str = "here";
pub const MENTION_HERE_USER_ID: &str = "1775731111020111321";

pub fn is_here_user_id(user_id: &str) -> bool {
    user_id == MENTION_HERE_ID || user_id == MENTION_HERE_USER_ID
}

mod string_or_number {
    use serde::de::{self, Visitor};
    use std::fmt;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct OptVisitor;

        impl<'de> Visitor<'de> for OptVisitor {
            type Value = Option<String>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, number, or null")
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok((!v.is_empty()).then(|| v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok((!v.is_empty()).then_some(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok((v != 0).then(|| v.to_string()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok((v != 0).then(|| v.to_string()))
            }
        }

        deserializer.deserialize_any(OptVisitor)
    }
}

#[derive(Debug, Clone, Default)]
pub struct OutgoingMention {
    pub user_id: String,
    pub role_id: String,
    pub username: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingMention {
    fn is_here(&self) -> bool {
        self.user_id == MENTION_HERE_ID
    }

    fn to_content_token(&self) -> ContentToken {
        ContentToken {
            s: Some(i64::from(self.s)),
            e: Some(i64::from(self.e)),
            user_id: (!self.user_id.is_empty()).then(|| self.user_id.clone()),
            role_id: (!self.role_id.is_empty()).then(|| self.role_id.clone()),
            username: (!self.username.is_empty()).then(|| self.username.clone()),
            ..Default::default()
        }
    }

    fn to_proto(&self) -> Option<api::MessageMention> {
        if self.is_here() {
            return None;
        }
        let user_id = match self.user_id.as_str() {
            "" => 0,
            raw => match raw.parse::<i64>() {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!("dropping mention with non-numeric user_id");
                    return None;
                }
            },
        };
        let role_id = match self.role_id.as_str() {
            "" => 0,
            raw => match raw.parse::<i64>() {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!("dropping mention with non-numeric role_id");
                    return None;
                }
            },
        };
        Some(api::MessageMention {
            user_id,
            role_id,
            username: self.username.clone(),
            s: self.s,
            e: self.e,
            ..Default::default()
        })
    }
}

pub fn mention_content_tokens(mentions: &[OutgoingMention]) -> Vec<ContentToken> {
    mentions
        .iter()
        .map(OutgoingMention::to_content_token)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct OutgoingHashtag {
    pub channel_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingHashtag {
    fn to_content_token(&self) -> ContentToken {
        ContentToken {
            s: Some(i64::from(self.s)),
            e: Some(i64::from(self.e)),
            channel_id: (!self.channel_id.is_empty()).then(|| self.channel_id.clone()),
            ..Default::default()
        }
    }
}

pub fn hashtag_content_tokens(hashtags: &[OutgoingHashtag]) -> Vec<ContentToken> {
    hashtags
        .iter()
        .map(OutgoingHashtag::to_content_token)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct OutgoingEmoji {
    pub emoji_id: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingEmoji {
    fn to_content_token(&self) -> ContentToken {
        ContentToken {
            s: Some(i64::from(self.s)),
            e: Some(i64::from(self.e)),
            emojiid: (!self.emoji_id.is_empty()).then(|| self.emoji_id.clone()),
            ..Default::default()
        }
    }
}

pub fn emoji_content_tokens(emojis: &[OutgoingEmoji]) -> Vec<ContentToken> {
    emojis.iter().map(OutgoingEmoji::to_content_token).collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingMarkdown {
    pub kind: String,
    pub s: i32,
    pub e: i32,
}

impl OutgoingMarkdown {
    fn to_content_token(&self) -> ContentToken {
        ContentToken {
            s: Some(i64::from(self.s)),
            e: Some(i64::from(self.e)),
            kind: (!self.kind.is_empty()).then(|| self.kind.clone()),
            ..Default::default()
        }
    }
}

pub fn markdown_content_tokens(markdowns: &[OutgoingMarkdown]) -> Vec<ContentToken> {
    markdowns
        .iter()
        .map(OutgoingMarkdown::to_content_token)
        .collect()
}

pub fn detect_markdown(text: &str) -> Vec<OutgoingMarkdown> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut u16_at = Vec::with_capacity(n + 1);
    let mut acc = 0i32;
    for ch in &chars {
        u16_at.push(acc);
        acc += ch.len_utf16() as i32;
    }
    u16_at.push(acc);

    let is_triple =
        |k: usize| k + 2 < n && chars[k] == '`' && chars[k + 1] == '`' && chars[k + 2] == '`';
    let starts_with = |k: usize, pat: &str| -> bool {
        for (d, pc) in pat.chars().enumerate() {
            if k + d >= n || chars[k + d] != pc {
                return false;
            }
        }
        true
    };
    let non_blank = |a: usize, b: usize| chars[a..b].iter().any(|c| !c.is_whitespace());

    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if is_triple(i) {
            let mut j = i + 3;
            while j < n && !is_triple(j) {
                j += 1;
            }
            if is_triple(j) && non_blank(i + 3, j) {
                out.push(OutgoingMarkdown {
                    kind: "pre".into(),
                    s: u16_at[i],
                    e: u16_at[j + 3],
                });
                i = j + 3;
                continue;
            }
        }

        if starts_with(i, "http://") || starts_with(i, "https://") {
            let scheme = if starts_with(i, "https://") { 8 } else { 7 };
            let mut j = i + scheme;
            while j < n && !matches!(chars[j], ' ' | '\n' | '\r' | '\t') {
                j += 1;
            }
            if j > i + scheme {
                out.push(OutgoingMarkdown {
                    kind: "lk".into(),
                    s: u16_at[i],
                    e: u16_at[j],
                });
                i = j;
                continue;
            }
        }

        if chars[i] == '`' && !is_triple(i) {
            let mut j = i + 1;
            while j < n && chars[j] != '`' {
                j += 1;
            }
            if j < n && chars[j] == '`' && non_blank(i + 1, j) {
                out.push(OutgoingMarkdown {
                    kind: "c".into(),
                    s: u16_at[i],
                    e: u16_at[j + 1],
                });
                i = j + 1;
                continue;
            }
        }

        if i + 1 < n && chars[i] == '*' && chars[i + 1] == '*' {
            let mut j = i + 2;
            while j + 1 < n && !(chars[j] == '*' && chars[j + 1] == '*') {
                j += 1;
            }
            if j + 1 < n && chars[j] == '*' && chars[j + 1] == '*' && non_blank(i + 2, j) {
                out.push(OutgoingMarkdown {
                    kind: "b".into(),
                    s: u16_at[i],
                    e: u16_at[j + 2],
                });
                i = j + 2;
                continue;
            }
        }

        i += 1;
    }
    out
}

fn with_presign_finish(content_json: String, keys: &[String]) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(&content_json).unwrap_or_else(|_| serde_json::json!({}));
    let Some(obj) = value.as_object_mut() else {
        return content_json;
    };
    obj.insert(
        "presign_finish".into(),
        serde_json::Value::Array(
            keys.iter()
                .map(|k| serde_json::Value::String(k.clone()))
                .collect(),
        ),
    );
    serde_json::to_string(&value).unwrap_or(content_json)
}

fn with_create_time_seconds(content_json: String, create_time_seconds: u32) -> String {
    if create_time_seconds == 0 {
        return content_json;
    }
    let mut value: serde_json::Value =
        serde_json::from_str(&content_json).unwrap_or_else(|_| serde_json::json!({}));
    let Some(obj) = value.as_object_mut() else {
        return content_json;
    };
    obj.insert(
        "create_time_seconds".into(),
        serde_json::Value::Number(create_time_seconds.into()),
    );
    serde_json::to_string(&value).unwrap_or(content_json)
}

fn build_message_content_json(
    text: &str,
    mentions: &[OutgoingMention],
    hashtags: &[OutgoingHashtag],
    emojis: &[OutgoingEmoji],
    markdowns: &[OutgoingMarkdown],
) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("t".into(), text.into());
    if !mentions.is_empty() {
        let arr: Vec<serde_json::Value> = mentions
            .iter()
            .map(|m| {
                let mut o = serde_json::Map::new();
                if !m.user_id.is_empty() {
                    o.insert("user_id".into(), m.user_id.clone().into());
                }
                if !m.role_id.is_empty() {
                    o.insert("role_id".into(), m.role_id.clone().into());
                }
                if !m.username.is_empty() {
                    o.insert("username".into(), m.username.clone().into());
                }
                o.insert("s".into(), m.s.into());
                o.insert("e".into(), m.e.into());
                serde_json::Value::Object(o)
            })
            .collect();
        obj.insert("mentions".into(), arr.into());
    }
    if !hashtags.is_empty() {
        let arr: Vec<serde_json::Value> = hashtags
            .iter()
            .map(|h| {
                let mut o = serde_json::Map::new();
                if !h.channel_id.is_empty() {
                    o.insert("channelId".into(), h.channel_id.clone().into());
                }
                o.insert("s".into(), h.s.into());
                o.insert("e".into(), h.e.into());
                serde_json::Value::Object(o)
            })
            .collect();
        obj.insert("hg".into(), arr.into());
    }
    if !emojis.is_empty() {
        let arr: Vec<serde_json::Value> = emojis
            .iter()
            .map(|e| {
                let mut o = serde_json::Map::new();
                if !e.emoji_id.is_empty() {
                    o.insert("emojiid".into(), e.emoji_id.clone().into());
                }
                o.insert("s".into(), e.s.into());
                o.insert("e".into(), e.e.into());
                serde_json::Value::Object(o)
            })
            .collect();
        obj.insert("ej".into(), arr.into());
    }
    if !markdowns.is_empty() {
        let arr: Vec<serde_json::Value> = markdowns
            .iter()
            .map(|m| {
                let mut o = serde_json::Map::new();
                if !m.kind.is_empty() {
                    o.insert("type".into(), m.kind.clone().into());
                }
                o.insert("s".into(), m.s.into());
                o.insert("e".into(), m.e.into());
                serde_json::Value::Object(o)
            })
            .collect();
        obj.insert("mk".into(), arr.into());
    }
    serde_json::Value::Object(obj).to_string()
}

/// One page from `ListChannelMessages`, including read-position metadata.
#[derive(Debug, Clone)]
pub struct ListChannelMessagesResult {
    pub messages: Vec<ApiMessage>,
    pub last_seen_message_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    pub message_id: i64,
    /// Plain message text (the `t` field of the content JSON).
    pub content: String,
    /// Full parsed content tokens for rich-text rendering.
    pub content_tokens: ApiMessageContent,
    /// Message type/category (`TypeMessage` in React). 0 = normal chat.
    pub code: i32,
    pub sender_id: i64,
    pub sender_name: String,
    pub avatar: String,
    pub create_time: i64,
    pub update_time: i64,
    /// When true, the "(edited)" suffix is hidden even if the message was edited.
    pub hide_editted: bool,
    pub attachments: Vec<ApiAttachment>,
    pub references: Vec<ApiMessageRef>,
    pub reactions: Vec<ApiMessageReaction>,
    pub entity_mentions: Vec<ApiEntityMention>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiThreadDesc {
    pub channel_id: String,
    pub channel_label: String,
    pub clan_id: String,
    pub parent_id: String,
    pub category_id: String,
    pub channel_private: i32,
    pub member_count: i32,
    pub active: i32,
    pub creator_id: String,
    pub last_message_content: String,
    pub last_message_sender_id: String,
    pub last_message_sender_name: String,
    pub last_message_sender_avatar: String,
    pub last_sent_timestamp: i64,
}

pub const THREAD_LIST_LIMIT: i32 = 50;

pub const CHECK_NAME_TYPE_THREAD: i32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiPinMessage {
    pub id: String,
    pub message_id: String,
    pub content: String,
    pub sender_id: String,
    pub sender_name: String,
    pub avatar: String,
    pub create_time: i64,
}

impl MezonTransport {
    /// Build a protobuf-encoded API request envelope.
    ///
    /// Wire format: Envelope { cid: uint32, api_request_event: ApiRequestEvent }
    fn build_api_request(&self, cid: u16, api_name: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let api_index = self
            .get_api_index(api_name)
            .ok_or_else(|| anyhow::anyhow!("unknown API name: {api_name}"))?;
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ApiRequestEvent(
                realtime::ApiRequestEvent {
                    api_index: api_index as i32,
                    api_name: api_name.to_string(),
                    body,
                },
            )),
        };
        Ok(envelope.encode_to_vec())
    }

    async fn send_api_request(
        &self,
        cid: u16,
        api_name: &str,
        body: Vec<u8>,
    ) -> Result<(u32, Vec<u8>)> {
        tracing::debug!(target: "socket", "api_send: action={api_name} cid={cid}");
        self.send(cid, self.build_api_request(cid, api_name, body)?)
            .await
    }

    async fn send_api_request_with_timeout(
        &self,
        cid: u16,
        api_name: &str,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<(u32, Vec<u8>)> {
        tracing::debug!(target: "socket", "api_send: action={api_name} cid={cid}");
        self.send_with_timeout(cid, self.build_api_request(cid, api_name, body)?, timeout)
            .await
    }

    fn account_from_user(
        user: api::User,
        email: Option<String>,
        password_setted: bool,
        logo: Option<String>,
    ) -> ApiAccount {
        ApiAccount {
            user_id: user.id,
            username: user.username,
            email,
            display_name: (!user.display_name.is_empty()).then_some(user.display_name),
            avatar_url: (!user.avatar_url.is_empty()).then_some(user.avatar_url),
            about_me: (!user.about_me.is_empty()).then_some(user.about_me),
            phone_number: (!user.phone_number.is_empty()).then_some(user.phone_number),
            password_setted,
            logo,
        }
    }

    fn decode_channel_desc_list(response: &[u8]) -> Result<api::ChannelDescList> {
        match api::ChannelDescList::decode(response) {
            Ok(list) => Ok(list),
            Err(decode_err) => {
                if let Ok(error) = realtime::Error::decode(response)
                    && (error.code != 0 || !error.message.is_empty())
                {
                    return Err(anyhow::anyhow!(
                        "API error: code={} {}",
                        error.code,
                        error.message.trim()
                    ));
                }
                Err(decode_err.into())
            }
        }
    }

    fn channel_desc_from_proto(channel: api::ChannelDescription) -> ApiChannelDesc {
        let last_seen_message_id = channel
            .last_seen_message
            .as_ref()
            .map(|m| m.id)
            .unwrap_or_default();
        let last_seen_timestamp = channel
            .last_seen_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        let last_sent_message_id = channel
            .last_sent_message
            .as_ref()
            .map(|m| m.id)
            .unwrap_or_default();
        let last_sent_timestamp = channel
            .last_sent_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        ApiChannelDesc {
            channel_id: channel.channel_id,
            channel_label: channel.channel_label,
            channel_type: channel.r#type as u32,
            clan_id: channel.clan_id,
            category_name: channel.category_name,
            category_id: channel.category_id,
            channel_private: channel.channel_private,
            count_mess_unread: channel.count_mess_unread,
            member_count: channel.member_count,
            parent_id: channel.parent_id,
            is_mute: channel.is_mute,
            last_seen_message_id,
            last_seen_timestamp,
            last_sent_message_id,
            last_sent_timestamp,
            badge_count: channel.count_mess_unread,
            creator_id: channel.creator_id,
        }
    }

    fn direct_channel_from_proto(channel: api::ChannelDescription) -> ApiDirectChannel {
        let last_seen_timestamp = channel
            .last_seen_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        let last_sent_timestamp = channel
            .last_sent_message
            .as_ref()
            .map(|m| i64::from(m.timestamp_seconds))
            .unwrap_or(0);
        ApiDirectChannel {
            channel_id: channel.channel_id,
            channel_label: channel.channel_label,
            channel_type: channel.r#type as u32,
            channel_avatar: channel.channel_avatar,
            avatars: channel.avatars,
            usernames: channel.usernames,
            display_names: channel.display_names,
            user_ids: channel.user_ids,
            onlines: channel.onlines,
            member_count: channel.member_count,
            count_mess_unread: channel.count_mess_unread,
            last_sent_timestamp,
            last_seen_timestamp,
            creator_id: channel.creator_id,
        }
    }

    fn category_desc_from_proto(cat: api::CategoryDesc) -> ApiCategoryDesc {
        ApiCategoryDesc {
            category_id: cat.category_id,
            category_name: cat.category_name,
            clan_id: cat.clan_id,
            category_order: cat.category_order,
        }
    }

    fn clan_desc_from_proto(clan: api::ClanDesc) -> ApiClanDesc {
        ApiClanDesc {
            clan_id: clan.clan_id,
            clan_name: clan.clan_name,
            creator_id: clan.creator_id,
            logo: clan.logo,
            banner: clan.banner,
            welcome_channel_id: clan.welcome_channel_id,
            status: clan.status,
            is_onboarding: clan.is_onboarding,
            is_community: clan.is_community,
            prevent_anonymous: clan.prevent_anonymous,
        }
    }

    pub fn message_from_proto(message: &api::ChannelMessage) -> ApiMessage {
        let content_tokens = serde_json::from_str::<ApiMessageContent>(&message.content)
            .unwrap_or_else(|_| ApiMessageContent {
                t: message.content.clone(),
                ..Default::default()
            });
        let entity_mentions = parse_message_mentions(&message.mentions);
        let mut content_tokens = content_tokens;
        enrich_content_tokens(&mut content_tokens, &entity_mentions);
        let content = content_tokens.t.clone();

        let sender_name = if !message.clan_nick.is_empty() {
            message.clan_nick.clone()
        } else if !message.display_name.is_empty() {
            message.display_name.clone()
        } else {
            message.username.clone()
        };

        let attachments = parse_message_attachments(&message.attachments);
        let references = parse_message_references(&message.references);
        let reactions = parse_message_reactions(&message.reactions);

        ApiMessage {
            message_id: message.message_id,
            content,
            content_tokens,
            code: message.code,
            sender_id: message.sender_id,
            sender_name,
            avatar: prioritize_avatar(&message.clan_avatar, &message.avatar),
            create_time: i64::from(message.create_time_seconds),
            update_time: i64::from(message.update_time_seconds),
            hide_editted: message.hide_editted,
            attachments,
            references,
            reactions,
            entity_mentions,
        }
    }

    fn thread_desc_from_proto(channel: api::ChannelDescription) -> ApiThreadDesc {
        let (last_message_content, last_message_sender_id, last_sent_timestamp) =
            match channel.last_sent_message.as_ref() {
                Some(msg) => (
                    parse_message_text(&msg.content),
                    msg.sender_id.to_string(),
                    i64::from(msg.timestamp_seconds),
                ),
                None => (String::new(), String::new(), 0),
            };

        ApiThreadDesc {
            channel_id: channel.channel_id.to_string(),
            channel_label: channel.channel_label,
            clan_id: channel.clan_id.to_string(),
            parent_id: channel.parent_id.to_string(),
            category_id: channel.category_id.to_string(),
            channel_private: channel.channel_private,
            member_count: channel.member_count,
            active: channel.active,
            creator_id: channel.creator_id.to_string(),
            last_message_content,
            last_message_sender_id,
            last_message_sender_name: String::new(),
            last_message_sender_avatar: String::new(),
            last_sent_timestamp,
        }
    }

    fn pin_message_from_proto(pin: api::PinMessage) -> ApiPinMessage {
        let content = serde_json::from_str::<serde_json::Value>(&pin.content)
            .ok()
            .and_then(|v| v.get("t").and_then(|t| t.as_str().map(|s| s.to_string())))
            .unwrap_or_else(|| pin.content.clone());

        ApiPinMessage {
            id: pin.id.to_string(),
            message_id: pin.message_id.to_string(),
            content,
            sender_id: pin.sender_id.to_string(),
            sender_name: pin.username,
            avatar: pin.avatar,
            create_time: i64::from(pin.create_time_seconds),
        }
    }

    /// Get API index from API name (matches TypeScript ApiNameEnum order)
    fn get_api_index(&self, api_name: &str) -> Option<u32> {
        let index = match api_name {
            // HOT PATH
            "ListChannelDescs" => 0,
            "GetAccount" => 1,
            "ListClanDescs" => 2,
            "ListClanUsers" => 3,
            "ListRoles" => 4,
            "ListEvents" => 5,
            "GetRoleOfUserInTheClan" => 6,
            "GetListPermission" => 7,
            "ListUserPermissionInChannel" => 8,
            "GetNotificationClan" => 9,
            "ListMutedChannel" => 10,
            "ListStreamingChannelUsers" => 11,
            "ListQuickMenuAccess" => 12,
            "GetNotificationChannel" => 13,
            "ListFriends" => 14,
            "EmojiRecentList" => 15,
            "GetListEmojisByUserId" => 16,
            "ListClanBadgeCount" => 17,
            "ListChannelBadgeCount" => 18,
            "ListLogedDevice" => 19,
            "ListClanUsersStatus" => 20,
            "ListChannelApps" => 21,
            "GetListFavoriteChannel" => 22,
            "ListCategoryDescs" => 23,
            "ListOnboarding" => 24,
            "GetListStickersByUserId" => 25,
            "GetSystemMessageByClanId" => 26,
            "GetPinMessagesList" => 27,
            "GetChannelCanvasList" => 28,
            "ListChannelTimeline" => 29,
            "ListChannelMessages" => 30,
            "ListActivity" => 31,
            "ListChannelByUserId" => 32,
            "ListUserClansByUserId" => 33,
            "GetUserProfileOnClan" => 34,
            "RegistFCMDeviceToken" => 35,
            "IsBanned" => 36,
            "ListThreadDescs" => 37,
            "ListArchivedChannelDescs" => 38,
            "ListChannelDetail" => 39,
            "GetChannelCategoryNotiSettingsList" => 40,
            "ListRoleUsers" => 41,
            "ListChannelUsers" => 42,
            "ListChannelAttachment" => 43,
            "ListChannelVoiceUsers" => 44,
            "ListUserOnline" => 45,
            "ListNotifications" => 46,
            "ListChannelUsersUC" => 47,
            "ListWebhookByChannelId" => 48,
            "GetPermissionByRoleIdChannelId" => 49,
            "ListChannelSetting" => 50,
            "ListApps" => 51,
            "GetApp" => 52,
            "ListForSaleItems" => 53,
            "ListClanWebhook" => 54,
            "GetUserStatus" => 55,
            "ListSdTopic" => 56,
            // COLD PATH
            "AddFriends" => 57,
            "AddChannelUsers" => 58,
            "RegistrationEmail" => 59,
            "BlockFriends" => 60,
            "UnblockFriends" => 61,
            "UploadAttachmentFile" => 62,
            "UploadOauthFile" => 63,
            "AddRolesChannelDesc" => 64,
            "CreateCategoryDesc" => 65,
            "CreateChannelDesc" => 66,
            "CreateRole" => 67,
            "CreateEvent" => 68,
            "DeleteRole" => 69,
            "DeleteEvent" => 70,
            "DeleteRoleChannelDesc" => 71,
            "DeleteChannelDesc" => 72,
            "CloseDMByChannelId" => 73,
            "OpenDMByChannelId" => 74,
            "DeleteAccount" => 75,
            "DeleteFriends" => 76,
            "DeleteCategoryDesc" => 77,
            "DeleteNotifications" => 78,
            "DeleteClanDesc" => 79,
            "UpdateUser" => 80,
            "UpdateUserProfileByClan" => 81,
            "UpdateClanOrder" => 82,
            "RemoveChannelUsers" => 83,
            "LeaveThread" => 84,
            "ArchiveChannel" => 85,
            "LinkSMS" => 86,
            "ConfirmLinkMezonOTP" => 87,
            "LinkEmail" => 88,
            "CreateClanDesc" => 89,
            "RemoveClanUsers" => 90,
            "BanClanUsers" => 91,
            "CreateLinkInviteUser" => 92,
            "InviteUser" => 93,
            "SetRoleChannelPermission" => 94,
            "SetNotificationChannelSetting" => 95,
            "SetMuteChannel" => 96,
            "SetMuteCategory" => 97,
            "SetNotificationClanSetting" => 98,
            "SetNotificationCategorySetting" => 99,
            "DeleteNotificationCategorySetting" => 100,
            "DeleteNotificationChannel" => 101,
            "CreatePinMessage" => 102,
            "CreateMessage2Inbox" => 103,
            "UnlinkMezon" => 104,
            "UnlinkEmail" => 105,
            "UpdateAccount" => 106,
            "UpdateUsername" => 107,
            "UpdateCategory" => 108,
            "UpdateCategoryOrder" => 109,
            "UpdateRoleOrder" => 110,
            "UpdateClanDesc" => 111,
            "UpdateChannelDesc" => 112,
            "UpdateChannelPrivate" => 113,
            "UpdateRole" => 114,
            "UpdateEvent" => 115,
            "SearchMessage" => 116,
            "CreateClanEmoji" => 117,
            "DeleteByIdClanEmoji" => 118,
            "UpdateClanEmojiById" => 119,
            "GenerateWebhook" => 120,
            "HandleWebhook" => 121,
            "UpdateWebhookById" => 122,
            "DeleteWebhookById" => 123,
            "AddClanSticker" => 124,
            "UpdateClanStickerById" => 125,
            "DeleteClanStickerById" => 126,
            "ChangeChannelCategory" => 127,
            "CheckDuplicateName" => 128,
            "AddApp" => 129,
            "DeleteApp" => 130,
            "UpdateApp" => 131,
            "AddAppToClan" => 132,
            "CreateSystemMessage" => 133,
            "UpdateSystemMessage" => 134,
            "DeleteSystemMessage" => 135,
            "StreamingServerCallback" => 136,
            "EditChannelCanvases" => 137,
            "GetChannelCanvasDetail" => 138,
            "DeleteChannelCanvas" => 139,
            "AddChannelFavorite" => 140,
            "RemoveChannelFavorite" => 141,
            "CreateActiviy" => 142,
            "GetPubKeys" => 143,
            "PushPubKey" => 144,
            "GetChanEncryptionMethod" => 145,
            "SetChanEncryptionMethod" => 146,
            "GetKeyServer" => 147,
            "ListAuditLog" => 148,
            "GetOnboardingDetail" => 149,
            "CreateOnboarding" => 150,
            "UpdateOnboarding" => 151,
            "DeleteOnboarding" => 152,
            "ListOnboardingStep" => 153,
            "UpdateOnboardingStep" => 154,
            "GenerateClanWebhook" => 155,
            "UpdateClanWebhookById" => 156,
            "DeleteClanWebhookById" => 157,
            "HandleClanWebhook" => 158,
            "UpdateUserStatus" => 159,
            "UpdateUserCustomStatus" => 160,
            "GetTopicDetail" => 161,
            "CreateSdTopic" => 162,
            "DeleteSdTopic" => 163,
            "CreateExternalMezonMeet" => 164,
            "GenerateMeetToken" => 165,
            "RemoveParticipantMezonMeet" => 166,
            "MuteParticipantMezonMeet" => 167,
            "CreateRoomChannelApps" => 168,
            "GetMezonOauthClient" => 169,
            "DeleteMezonOauthClient" => 170,
            "UpdateMezonOauthClient" => 171,
            "SearchThread" => 172,
            "GenerateHashChannelApps" => 173,
            "DeleteUserEvent" => 174,
            "AddUserEvent" => 175,
            "DeleteQuickMenuAccess" => 176,
            "AddQuickMenuAccess" => 177,
            "UpdateQuickMenuAccess" => 178,
            "TransferOwnership" => 179,
            "SendChannelMessage" => 180,
            "UpdateChannelMessage" => 181,
            "DeleteChannelMessage" => 182,
            "ReportMessageAbuse" => 183,
            "MessageButtonClick" => 184,
            "DropdownBoxSelected" => 185,
            "ActiveArchivedThread" => 186,
            "UpdateChannelTimeline" => 187,
            "AddAgentToChannel" => 188,
            "DisconnectAgent" => 189,
            "CreateChannelTimeline" => 190,
            "DetailChannelTimeline" => 191,
            "CreatePoll" => 192,
            "VotePoll" => 193,
            "ClosePoll" => 194,
            "GetPoll" => 195,
            "ReactChannelMessage" => 196,
            "MultipartUploadAttachmentFileStart" => 197,
            "MultipartUploadAttachmentFileFinish" => 198,
            "SessionRefresh" => 199,
            "SessionLogout" => 200,
            "Healthcheck" => 201,
            "UnbanClanUsers" => 202,
            "ListBannedUsers" => 203,
            "GetNotificationCategory" => 204,
            "ListRolePermissions" => 205,
            "IsFollower" => 206,
            "DeletePinMessage" => 207,
            "MarkAsRead" => 208,
            "UploadBatchAttachmentFile" => 209,
            _ => {
                tracing::warn!("unknown API name: {api_name}");
                return None;
            }
        };
        Some(index)
    }

    /// Get the current user's account.
    pub async fn get_account(&self) -> Result<ApiAccount> {
        tracing::debug!("MezonTransport::get_account() called");

        let cid = self.generate_cid();
        tracing::debug!("  Generated CID: {}", cid);

        // Build API request envelope
        let api_name = "GetAccount";
        let body = Vec::new();

        tracing::debug!("  Building API request envelope...");
        tracing::debug!("    API name: {}", api_name);
        tracing::debug!("    API index: {:?}", self.get_api_index(api_name));
        tracing::debug!("    Body len: {}", body.len());

        let request_bytes = self.build_api_request(cid, api_name, body)?;
        tracing::debug!("  Request envelope size: {} bytes", request_bytes.len());

        tracing::debug!("  Calling self.send() with cid={}...", cid);
        let send_result = self.send(cid, request_bytes).await;

        match send_result {
            Ok((code, response)) => {
                tracing::debug!(
                    "Received response: code={}, len={} bytes",
                    code,
                    response.len()
                );

                if code != 0 {
                    tracing::error!("API error: code={}", code);
                    return Err(anyhow::anyhow!("API error: code={}", code));
                }

                if let Ok(envelope) = realtime::Envelope::decode(response.as_slice())
                    && let Some(realtime::envelope::Message::Error(error)) = envelope.message
                {
                    return Err(anyhow::anyhow!(
                        "GetAccount API error: code={} error={}",
                        error.code,
                        error.message
                    ));
                }

                let account = api::Account::decode(response.as_slice())?;
                let user = account
                    .user
                    .ok_or_else(|| anyhow::anyhow!("GetAccount response missing user"))?;
                let account = Self::account_from_user(
                    user,
                    (!account.email.is_empty()).then_some(account.email),
                    account.password_setted,
                    (!account.logo.is_empty()).then_some(account.logo),
                );
                tracing::debug!("Decoded account response: {} bytes", response.len());
                Ok(account)
            }
            Err(e) => {
                tracing::error!("self.send() failed: {}", e);
                Err(e)
            }
        }
    }

    /// List users in a clan.
    pub async fn list_clan_users(&self, clan_id: i64) -> Result<Vec<api::ClanUserList>> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ClanUserList::decode(response.as_slice())?;
        Ok(vec![list])
    }

    /// List channels in a clan.
    pub async fn list_channel_descs(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelDescs";
        let body = api::ListChannelDescsRequest {
            clan_id,
            limit: 500,
            state: 1,
            channel_type: 1,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!(
                "API error: code={} {}",
                code,
                String::from_utf8_lossy(&response).trim()
            ));
        }

        let channels = Self::decode_channel_desc_list(&response)?;
        Ok(channels
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    /// List the user's direct-message / group conversations (clan_id = 0). Mirrors mezon-react's
    /// `fetchDirectMessage` → `ListChannelDescs(clan_id="0", channel_type=GROUP)`, which returns
    /// both 1-1 DMs (type 3) and groups (type 2).
    pub async fn list_dm_channel_descs(&self, page: i32) -> Result<Vec<ApiDirectChannel>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelDescs";
        let body = api::ListChannelDescsRequest {
            clan_id: 0,
            limit: 500,
            state: 1,
            channel_type: 2,
            page,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!(
                "API error: code={} {}",
                code,
                String::from_utf8_lossy(&response).trim()
            ));
        }

        let channels = Self::decode_channel_desc_list(&response)?;
        Ok(channels
            .channeldesc
            .into_iter()
            .map(Self::direct_channel_from_proto)
            .collect())
    }

    /// List roles in a clan.
    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<api::RoleListEventResponse> {
        let cid = self.generate_cid();
        let body = api::RoleListEventRequest {
            clan_id,
            limit,
            state: 0,
            cursor: cursor.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoles", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleListEventResponse::decode(response.as_slice())?)
    }

    /// List user's clans.
    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        let cid = self.generate_cid();

        let body = api::ListClanDescRequest {
            limit: 50,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanDescs", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let clans = api::ClanDescList::decode(response.as_slice())?;
        Ok(clans
            .clandesc
            .into_iter()
            .map(Self::clan_desc_from_proto)
            .collect())
    }

    /// List users in a channel.
    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<api::ChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            channel_id,
            channel_type,
            limit: 2000,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelUserList::decode(response.as_slice())?)
    }

    /// List messages in a channel.
    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<ListChannelMessagesResult> {
        let cid = self.generate_cid();

        let api_name = "ListChannelMessages";
        let body = api::ListChannelMessagesRequest {
            clan_id,
            channel_id,
            message_id,
            direction,
            limit: limit as i32,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let page = api::ChannelMessageList::decode(response.as_slice())?;
        let last_seen_message_id = page.last_seen_message.as_ref().map(|h| h.id).unwrap_or(0);
        let parsed: Vec<ApiMessage> = page
            .messages
            .into_iter()
            // Skip pure store-mutation / transient codes that are not renderable
            // list rows: ChatUpdate(1), ChatRemove(2), Typing(3),
            // UpdateEphemeralMsg(14), DeleteEphemeralMsg(15). All other codes
            // (normal chat + system rows like Welcome/CreatePin/CreateThread)
            // are kept and routed by the UI dispatcher.
            .map(|m| Self::message_from_proto(&m))
            .collect();
        tracing::debug!(
            "list_channel_messages: channel_id={channel_id} count={} response_bytes={}",
            parsed.len(),
            response.len(),
        );
        Ok(ListChannelMessagesResult {
            messages: parsed,
            last_seen_message_id,
        })
    }

    /// Send a message to a channel.
    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!(target: "socket", "realtime_send: action=ChannelJoin cid={} clan_id={clan_id} channel_id={channel_id}", i32::from(cid));
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ChannelJoin(
                realtime::ChannelJoin {
                    clan_id,
                    channel_id,
                    channel_type,
                    is_public,
                },
            )),
        };
        let (code, _response) = self.send(cid, envelope.encode_to_vec()).await?;
        if code != 0 {
            anyhow::bail!("join_chat error: code={code}");
        }
        Ok(())
    }

    pub async fn write_voice_reaction(&self, emojis: Vec<String>, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!(target: "socket", "realtime_send: action=VoiceReactionSend cid={} channel_id={channel_id}", i32::from(cid));
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::VoiceReactionSend(
                realtime::VoiceReactionSend {
                    emojis,
                    channel_id,
                    sender_id: 0,
                    media_type: 0,
                },
            )),
        };
        let (code, _response) = self.send(cid, envelope.encode_to_vec()).await?;
        if code != 0 {
            anyhow::bail!("write_voice_reaction error: code={code}");
        }
        Ok(())
    }

    /// Report the user's read position (cf. React `writeLastSeenMessage`).
    pub async fn write_last_seen_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        mode: i32,
        timestamp_seconds: u32,
        badge_count: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!(
            target: "socket",
            "realtime_send: action=LastSeenMessageEvent cid={} clan_id={clan_id} channel_id={channel_id} message_id={message_id}",
            i32::from(cid)
        );
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::LastSeenMessageEvent(
                realtime::LastSeenMessageEvent {
                    clan_id,
                    channel_id,
                    message_id,
                    mode,
                    timestamp_seconds,
                    badge_count,
                },
            )),
        };
        let (code, _response) = self.send(cid, envelope.encode_to_vec()).await?;
        if code != 0 {
            anyhow::bail!("write_last_seen_message error: code={code}");
        }
        Ok(())
    }

    pub async fn join_clan_chat(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        tracing::debug!(target: "socket", "realtime_send: action=ClanJoin cid={} clan_id={clan_id}", i32::from(cid));
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ClanJoin(realtime::ClanJoin {
                clan_id,
            })),
        };
        let (code, _response) = self.send(cid, envelope.encode_to_vec()).await?;
        if code != 0 {
            anyhow::bail!("join_clan_chat error: code={code}");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        mentions: Vec<OutgoingMention>,
        hashtags: Vec<OutgoingHashtag>,
        emojis: Vec<OutgoingEmoji>,
    ) -> Result<ApiMessage> {
        self.send_channel_message_inner(
            clan_id,
            channel_id,
            content,
            is_public,
            mode,
            Vec::new(),
            Vec::new(),
            mentions,
            hashtags,
            emojis,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<api::MessageAttachment>,
        reply: Option<OutgoingReply>,
        mentions: Vec<OutgoingMention>,
        hashtags: Vec<OutgoingHashtag>,
        emojis: Vec<OutgoingEmoji>,
        presign_finish: Option<Vec<String>>,
    ) -> Result<ApiMessage> {
        let references = reply
            .map(|reply| api::MessageRef {
                message_ref_id: reply.message_ref_id,
                content: reply.content,
                has_attachment: reply.has_attachment,
                ref_type: 0,
                message_sender_id: reply.message_sender_id,
                message_sender_username: reply.message_sender_username,
                message_sender_avatar: reply.message_sender_avatar,
                message_sender_clan_nick: reply.message_sender_clan_nick,
                message_sender_display_name: reply.message_sender_display_name,
                ..Default::default()
            })
            .into_iter()
            .collect();
        self.send_channel_message_inner(
            clan_id,
            channel_id,
            content,
            is_public,
            mode,
            attachments,
            references,
            mentions,
            hashtags,
            emojis,
            presign_finish,
        )
        .await
    }

    /// Send a message as a reply to `reply` (mezon `references[0]`).
    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_reply(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        reply: OutgoingReply,
        mentions: Vec<OutgoingMention>,
        hashtags: Vec<OutgoingHashtag>,
        emojis: Vec<OutgoingEmoji>,
    ) -> Result<ApiMessage> {
        let reference = api::MessageRef {
            message_ref_id: reply.message_ref_id,
            content: reply.content,
            has_attachment: reply.has_attachment,
            ref_type: 0,
            message_sender_id: reply.message_sender_id,
            message_sender_username: reply.message_sender_username,
            message_sender_avatar: reply.message_sender_avatar,
            message_sender_clan_nick: reply.message_sender_clan_nick,
            message_sender_display_name: reply.message_sender_display_name,
            ..Default::default()
        };
        self.send_channel_message_inner(
            clan_id,
            channel_id,
            content,
            is_public,
            mode,
            Vec::new(),
            vec![reference],
            mentions,
            hashtags,
            emojis,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_channel_message_inner(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<api::MessageAttachment>,
        references: Vec<api::MessageRef>,
        mentions: Vec<OutgoingMention>,
        hashtags: Vec<OutgoingHashtag>,
        emojis: Vec<OutgoingEmoji>,
        presign_finish: Option<Vec<String>>,
    ) -> Result<ApiMessage> {
        let cid = self.generate_cid();

        let api_name = "SendChannelMessage";
        let parsed_clan_id: i64 = clan_id;
        let parsed_channel_id: i64 = channel_id;
        tracing::debug!(
            "send_channel_message: clan_id={} channel_id={} is_public={} content_len={} attachments={}",
            parsed_clan_id,
            parsed_channel_id,
            is_public,
            content.len(),
            attachments.len()
        );
        // mezon stores message content as JSON `{ "t": <text> }` (matches mezon-js), not raw text.
        let markdowns = detect_markdown(content);
        let content_json =
            build_message_content_json(content, &mentions, &hashtags, &emojis, &markdowns);
        let content_json = match &presign_finish {
            Some(keys) => with_presign_finish(content_json, keys),
            None => content_json,
        };
        let mention_everyone = mentions.iter().any(OutgoingMention::is_here);
        let proto_mentions: Vec<api::MessageMention> = mentions
            .iter()
            .filter_map(OutgoingMention::to_proto)
            .collect();
        // No client `id`: the server generates the message Snowflake (mezon-js omits it).
        // Sending a client-side id made the server reject with code 13 (INTERNAL).
        let body = realtime::ChannelMessageSend {
            clan_id: parsed_clan_id,
            channel_id: parsed_channel_id,
            content: content_json,
            mentions: proto_mentions,
            attachments,
            references,
            mode,
            is_public,
            mention_everyone,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let ack = realtime::ChannelMessageAck::decode(response.as_slice())?;
        tracing::debug!(
            "send_channel_message ack: message_id={} channel_id={} code={}",
            ack.message_id,
            ack.channel_id,
            ack.code
        );
        Ok(ApiMessage {
            message_id: ack.message_id,
            content: content.to_string(),
            content_tokens: ApiMessageContent {
                t: content.to_string(),
                mentions: mentions
                    .iter()
                    .map(OutgoingMention::to_content_token)
                    .collect(),
                hg: hashtags
                    .iter()
                    .map(OutgoingHashtag::to_content_token)
                    .collect(),
                ej: emojis.iter().map(OutgoingEmoji::to_content_token).collect(),
                mk: markdown_content_tokens(&markdowns),
                ..Default::default()
            },
            code: 0,
            sender_id: 0,
            sender_name: ack.username,
            avatar: String::new(),
            create_time: i64::from(ack.create_time_seconds),
            update_time: 0,
            hide_editted: false,
            attachments: Vec::new(),
            references: Vec::new(),
            reactions: Vec::new(),
            entity_mentions: Vec::new(),
        })
    }

    /// List user's friends.
    pub async fn list_friends(&self) -> Result<Vec<ApiAccount>> {
        let cid = self.generate_cid();

        let api_name = "ListFriends";
        let body = api::ListFriendsRequest::default().encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let friends = api::FriendList::decode(response.as_slice())?;
        Ok(friends
            .friends
            .into_iter()
            .filter_map(|friend| {
                friend
                    .user
                    .map(|user| Self::account_from_user(user, None, false, None))
            })
            .collect())
    }

    /// List clan badge counts.
    pub async fn list_clan_badge_count(&self) -> Result<api::ListClanBadgeCountResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListClanBadgeCount", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    pub async fn list_clan_badge_count_typed(&self) -> Result<Vec<(String, i32, bool)>> {
        let raw = self.list_clan_badge_count().await?;
        Ok(raw
            .list_badge
            .into_iter()
            .map(|b| (b.clan_id.to_string(), b.badge, b.has_unread))
            .collect())
    }

    /// List channel badge counts.
    pub async fn list_channel_badge_count(
        &self,
        clan_id: i64,
    ) -> Result<api::ListChannelBadgeCountResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelBadgeCountRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelBadgeCount", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List notifications.
    pub async fn list_notifications(
        &self,
        clan_id: i64,
        limit: i32,
        notification_id: i64,
        category: i32,
        direction: i32,
    ) -> Result<api::NotificationList> {
        let cid = self.generate_cid();
        let body = api::ListNotificationsRequest {
            clan_id,
            limit,
            notification_id,
            category,
            direction,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationList::decode(response.as_slice())?)
    }

    /// Get user profile on a clan.
    pub async fn get_user_profile_on_clan(&self, clan_id: i64) -> Result<api::ClanProfile> {
        let cid = self.generate_cid();
        let body = api::ClanProfileRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetUserProfileOnClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanProfile::decode(response.as_slice())?)
    }

    /// List category descriptions in a clan.
    pub async fn list_category_descs(&self, clan_id: i64) -> Result<api::CategoryDescList> {
        let cid = self.generate_cid();
        let body = api::CategoryDesc {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListCategoryDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDescList::decode(response.as_slice())?)
    }

    pub async fn list_categories_typed(&self, clan_id: i64) -> Result<Vec<ApiCategoryDesc>> {
        let raw = self.list_category_descs(clan_id).await?;
        Ok(raw
            .categorydesc
            .into_iter()
            .map(Self::category_desc_from_proto)
            .collect())
    }

    pub async fn list_channel_badge_counts(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        let raw = self.list_channel_badge_count(clan_id).await?;
        Ok(raw
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    pub async fn list_voice_channel_users(&self, clan_id: i64) -> Result<Vec<ApiVoiceChannelUser>> {
        let raw = self.list_channel_voice_users(clan_id).await?;
        Ok(raw
            .voice_channel_users
            .into_iter()
            .map(|u| ApiVoiceChannelUser {
                channel_id: u.channel_id,
                user_ids: u
                    .user_ids
                    .iter()
                    .filter_map(|s| s.parse::<i64>().ok())
                    .collect(),
            })
            .collect())
    }

    /// List channel description detail.
    pub async fn list_channel_detail(&self, channel_id: i64) -> Result<api::ChannelDescription> {
        let cid = self.generate_cid();
        let body = api::ListChannelDetailRequest { channel_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescription::decode(response.as_slice())?)
    }

    /// List thread descriptions for a parent channel.
    pub async fn list_thread_descs(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
        state: i32,
        thread_id: Option<&str>,
    ) -> Result<Vec<ApiThreadDesc>> {
        let cid = self.generate_cid();
        let thread_id = match thread_id {
            Some(id) => parse_id(id)?,
            None => 0,
        };
        let body = api::ListThreadRequest {
            channel_id,
            clan_id,
            limit,
            page,
            state,
            thread_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListThreadDescs", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ChannelDescList::decode(response.as_slice())?;
        Ok(list
            .channeldesc
            .into_iter()
            .map(Self::thread_desc_from_proto)
            .collect())
    }

    /// List channels by user ID.
    pub async fn list_channel_by_user_id(&self) -> Result<Vec<ApiChannelDesc>> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListChannelByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let channel_list = api::ChannelDescList::decode(response.as_slice())?;
        Ok(channel_list
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    /// Get notification settings for a clan.
    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        let cid = self.generate_cid();

        let body = api::NotificationClan { clan_id }.encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "GetNotificationClan", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let setting = api::NotificationSetting::decode(response.as_slice())?;
        Ok(setting.notification_setting_type)
    }

    /// List events in a clan.
    pub async fn list_events(&self, clan_id: i64) -> Result<api::EventList> {
        let cid = self.generate_cid();
        let body = api::ListEventsRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListEvents", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventList::decode(response.as_slice())?)
    }

    /// List activity.
    pub async fn list_activity(&self) -> Result<api::ListUserActivity> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListActivity", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserActivity::decode(response.as_slice())?)
    }

    pub async fn list_channel_apps(&self, clan_id: i64) -> Result<Vec<ApiChannelApp>> {
        let cid = self.generate_cid();
        let body = api::ListChannelAppsRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("ListChannelApps code={}", code));
        }
        let resp = api::ListChannelAppsResponse::decode(response.as_slice())?;
        Ok(resp
            .channel_apps
            .into_iter()
            .map(|a| ApiChannelApp {
                app_id: a.app_id.to_string(),
                app_name: a.app_name,
                app_logo: (!a.app_logo.is_empty()).then_some(a.app_logo),
                app_url: a.app_url,
                channel_id: a.channel_id,
            })
            .collect())
    }

    /// List emoji recent by user ID.
    pub async fn emoji_recent_list(&self) -> Result<api::EmojiRecentList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "EmojiRecentList", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiRecentList::decode(response.as_slice())?)
    }

    /// List emojis by user ID.
    pub async fn list_emojis_by_user_id(&self) -> Result<api::EmojiListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListEmojisByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiListedResponse::decode(response.as_slice())?)
    }

    /// List stickers by user ID.
    pub async fn list_stickers_by_user_id(&self) -> Result<api::StickerListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListStickersByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StickerListedResponse::decode(response.as_slice())?)
    }

    /// Get system message by clan ID.
    pub async fn get_system_message_by_clan_id(&self, clan_id: i64) -> Result<api::SystemMessage> {
        let cid = self.generate_cid();
        let body = api::GetSystemMessage { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetSystemMessageByClanId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SystemMessage::decode(response.as_slice())?)
    }

    /// Get pin messages list.
    pub async fn get_pin_messages_list(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<Vec<ApiPinMessage>> {
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id {channel_id:?}: {e}"))?;
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id {clan_id:?}: {e}"))?;
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            channel_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPinMessagesList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::PinMessagesList::decode(response.as_slice())?;
        Ok(list
            .pin_messages_list
            .into_iter()
            .map(Self::pin_message_from_proto)
            .collect())
    }

    /// List channel timeline.
    pub async fn list_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        year: i32,
        limit: i32,
    ) -> Result<api::ListChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelTimelineRequest {
            clan_id,
            channel_id,
            year,
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get role of user in clan.
    pub async fn get_role_of_user_in_clan(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::RoleList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionOfUsersRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetRoleOfUserInTheClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleList::decode(response.as_slice())?)
    }

    /// Get list permission.
    pub async fn get_list_permission(&self) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListPermission", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// List user permission in channel.
    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::UserPermissionInChannelListResponse> {
        let cid = self.generate_cid();
        let body = api::UserPermissionInChannelListRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListUserPermissionInChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserPermissionInChannelListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get user status.
    pub async fn get_user_status(&self) -> Result<api::UserStatus> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetUserStatus", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserStatus::decode(response.as_slice())?)
    }

    /// List online users.
    pub async fn list_user_online(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ListUserOnlineResponse> {
        let cid = self.generate_cid();
        let body = api::ListUserOnlineRequest {
            clan_id,
            limit,
            page,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListUserOnline", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserOnlineResponse::decode(response.as_slice())?)
    }

    /// List streaming channel users.
    pub async fn list_streaming_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::StreamingChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListStreamingChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StreamingChannelUserList::decode(response.as_slice())?)
    }

    /// List quick menu access.
    pub async fn list_quick_menu_access(
        &self,
        bot_id: i64,
        channel_id: i64,
        menu_type: i32,
    ) -> Result<api::QuickMenuAccessList> {
        let cid = self.generate_cid();
        let body = api::ListQuickMenuAccessRequest {
            bot_id,
            channel_id,
            menu_type,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::QuickMenuAccessList::decode(response.as_slice())?)
    }

    /// Get notification channel.
    pub async fn get_notification_channel(
        &self,
        channel_id: i64,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel { channel_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// Get notification category.
    pub async fn get_notification_category(
        &self,
        category_id: i64,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory { category_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// List clan users status.
    pub async fn list_clan_users_status(&self, clan_id: i64) -> Result<api::ClanUserStatusList> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersStatusRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListClanUsersStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanUserStatusList::decode(response.as_slice())?)
    }

    /// Get list favorite channels.
    pub async fn get_list_favorite_channel(
        &self,
        clan_id: i64,
    ) -> Result<api::ListFavoriteChannelResponse> {
        let cid = self.generate_cid();
        let body = api::ListFavoriteChannelRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetListFavoriteChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListFavoriteChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List logged devices.
    pub async fn list_loged_device(&self) -> Result<api::LogedDeviceList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListLogedDevice", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let devices = api::LogedDeviceList::decode(response.as_slice())?;
        Ok(devices)
    }

    /// List channel users (UC variant).
    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<api::AllUsersAddChannelResponse> {
        let cid = self.generate_cid();
        let body = api::AllUsersAddChannelRequest { channel_id, limit }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelUsersUC", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUsersAddChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List webhook by channel ID.
    pub async fn list_webhook_by_channel_id(
        &self,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<api::WebhookListResponse> {
        let cid = self.generate_cid();
        let body = api::WebhookListRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListWebhookByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::WebhookListResponse::decode(response.as_slice())?)
    }

    /// Get permission by role ID and channel ID.
    pub async fn get_permission_by_role_id_channel_id(
        &self,
        role_id: i64,
        channel_id: i64,
    ) -> Result<api::PermissionRoleChannelListEventResponse> {
        let cid = self.generate_cid();
        let body = api::PermissionRoleChannelListEventRequest {
            role_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPermissionByRoleIdChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionRoleChannelListEventResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List channel setting.
    pub async fn list_channel_setting(
        &self,
        clan_id: i64,
    ) -> Result<api::ChannelSettingListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelSettingListRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelSettingListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List apps.
    pub async fn list_apps(&self, filter: &str) -> Result<api::AppList> {
        let cid = self.generate_cid();
        let body = api::ListAppsRequest {
            filter: filter.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AppList::decode(response.as_slice())?)
    }

    /// Get app by ID.
    pub async fn get_app(&self, id: i64) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::App {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// List for sale items.
    pub async fn list_for_sale_items(&self, page: i32) -> Result<api::ForSaleItemList> {
        let cid = self.generate_cid();
        let body = api::ListForSaleItemsRequest { page }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListForSaleItems", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ForSaleItemList::decode(response.as_slice())?)
    }

    /// List clan webhook.
    pub async fn list_clan_webhook(&self, clan_id: i64) -> Result<api::ListClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::ListClanWebhookRequest { clan_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanWebhookResponse::decode(response.as_slice())?)
    }

    /// List Sd Topics.
    pub async fn list_sd_topic(&self, clan_id: i64, limit: i32) -> Result<api::SdTopicList> {
        let cid = self.generate_cid();
        let body = api::ListSdTopicRequest { clan_id, limit }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopicList::decode(response.as_slice())?)
    }

    /// Get topic detail.
    pub async fn get_topic_detail(&self, topic_id: i64) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicDetailRequest { topic_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetTopicDetail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// List channel attachment. Parameter order and fields mirror mezon-js
    /// `listChannelAttachments(clanId, channelId, fileType, state, limit, before, after)`
    /// so the server cache key matches the web client (see `CACHE_FLOW.md`).
    #[allow(clippy::too_many_arguments)]
    pub async fn list_channel_attachment(
        &self,
        clan_id: i64,
        channel_id: i64,
        file_type: String,
        state: i32,
        limit: i32,
        before: u32,
        after: u32,
    ) -> Result<Vec<ApiChannelAttachment>> {
        let cid = self.generate_cid();
        let body = api::ListChannelAttachmentRequest {
            clan_id,
            channel_id,
            file_type,
            limit,
            state,
            before,
            after,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelAttachment", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ChannelAttachmentList::decode(response.as_slice())?;
        Ok(list
            .attachments
            .into_iter()
            .filter(|a| !a.url.is_empty())
            .map(ApiChannelAttachment::from_proto)
            .collect())
    }

    pub async fn forward_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<api::MessageAttachment>,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let mut obj = serde_json::Map::new();
        obj.insert("t".into(), content.into());
        obj.insert("fwd".into(), serde_json::Value::Bool(true));
        let content_json = serde_json::Value::Object(obj).to_string();
        let body = realtime::ChannelMessageSend {
            clan_id,
            channel_id,
            content: content_json,
            attachments,
            mode,
            is_public,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _response) = self
            .send_api_request(cid, "SendChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        attachments: Vec<api::MessageAttachment>,
        mode: i32,
        is_public: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let markdowns = detect_markdown(content);
        let content_json = build_message_content_json(content, &[], &[], &[], &markdowns);
        let body = realtime::ChannelMessageUpdate {
            clan_id,
            channel_id,
            message_id,
            content: content_json,
            attachments,
            mode,
            is_public,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn patch_message_presign_finish(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        mentions: Vec<OutgoingMention>,
        presign_finish: Vec<String>,
        create_time_seconds: u32,
        mode: i32,
        is_public: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let markdowns = detect_markdown(content);
        let content_json = build_message_content_json(content, &mentions, &[], &[], &markdowns);
        let content_json = with_presign_finish(content_json, &presign_finish);
        let content_json = with_create_time_seconds(content_json, create_time_seconds);
        let proto_mentions: Vec<api::MessageMention> = mentions
            .iter()
            .filter_map(OutgoingMention::to_proto)
            .collect();
        let body = realtime::ChannelMessageUpdate {
            clan_id,
            channel_id,
            message_id,
            content: content_json,
            mentions: proto_mentions,
            mode,
            is_public,
            hide_editted: true,
            create_time_seconds,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// List voice channel users.
    pub async fn list_channel_voice_users(
        &self,
        clan_id: i64,
    ) -> Result<api::VoiceChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id,
            limit: 100,
            state: 1,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelVoiceUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VoiceChannelUserList::decode(response.as_slice())?)
    }

    /// List archived channel descriptions.
    pub async fn list_archived_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<api::ListArchivedChannelDescsResponse> {
        let cid = self.generate_cid();
        let body = api::ListArchivedChannelDescsRequest { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListArchivedChannelDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListArchivedChannelDescsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List user clans by user ID.
    pub async fn list_user_clans_by_user_id(&self) -> Result<api::AllUserClans> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListUserClansByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUserClans::decode(response.as_slice())?)
    }

    /// Check if user is banned.
    pub async fn is_banned(&self, channel_id: i64) -> Result<api::IsBannedResponse> {
        let cid = self.generate_cid();
        let body = api::IsBannedRequest { channel_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsBanned", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsBannedResponse::decode(response.as_slice())?)
    }

    /// List banned users.
    pub async fn list_banned_users(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::BannedUserList> {
        let cid = self.generate_cid();
        let body = api::BannedUserListRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListBannedUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::BannedUserList::decode(response.as_slice())?)
    }

    /// Get channel canvas list.
    pub async fn get_channel_canvas_list(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ChannelCanvasListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasListRequest {
            channel_id,
            clan_id,
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasListResponse::decode(response.as_slice())?)
    }

    /// Get channel canvas detail.
    pub async fn get_channel_canvas_detail(
        &self,
        id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::ChannelCanvasDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasDetailRequest {
            id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List onboarding.
    pub async fn list_onboarding(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingRequest {
            clan_id,
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Get onboarding detail.
    pub async fn get_onboarding_detail(
        &self,
        id: i64,
        clan_id: i64,
    ) -> Result<api::OnboardingItem> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest { id, clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetOnboardingDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::OnboardingItem::decode(response.as_slice())?)
    }

    /// List onboarding steps.
    pub async fn list_onboarding_step(
        &self,
        clan_id: i64,
    ) -> Result<api::ListOnboardingStepResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingStepRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingStepResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List role users.
    pub async fn list_role_users(&self, role_id: i64) -> Result<api::RoleUserList> {
        let cid = self.generate_cid();
        let body = api::ListRoleUsersRequest {
            role_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoleUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleUserList::decode(response.as_slice())?)
    }

    /// List role permissions.
    pub async fn list_role_permissions(&self, role_id: i64) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionsRequest { role_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListRolePermissions", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// Check if user is a follower.
    pub async fn is_follower(&self, follow_id: i64) -> Result<api::IsFollowerResponse> {
        let cid = self.generate_cid();
        let body = api::IsFollowerRequest { follow_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsFollower", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsFollowerResponse::decode(response.as_slice())?)
    }

    /// Get channel encryption method.
    pub async fn get_chan_encryption_method(
        &self,
        channel_id: i64,
    ) -> Result<api::ChanEncryptionMethod> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChanEncryptionMethod::decode(response.as_slice())?)
    }

    /// Get key server.
    pub async fn get_key_server(&self) -> Result<api::GetKeyServerResp> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetKeyServer", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetKeyServerResp::decode(response.as_slice())?)
    }

    /// Get pub keys.
    pub async fn get_pub_keys(&self, user_ids: &[&str]) -> Result<api::GetPubKeysResponse> {
        let cid = self.generate_cid();
        let body = api::GetPubKeysRequest {
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPubKeys", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPubKeysResponse::decode(response.as_slice())?)
    }

    /// List audit log.
    pub async fn list_audit_log(&self, clan_id: i64) -> Result<api::ListAuditLog> {
        let cid = self.generate_cid();
        let body = api::ListAuditLogRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListAuditLog", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListAuditLog::decode(response.as_slice())?)
    }

    /// Search messages via Elasticsearch (server RPC `SearchMessage`).
    pub async fn search_message(
        &self,
        filters: Vec<api::FilterParam>,
        from: i32,
        size: i32,
        sorts: Vec<api::SortParam>,
    ) -> Result<api::SearchMessageResponse> {
        let cid = self.generate_cid();
        let body = api::SearchMessageRequest {
            filters,
            from,
            size,
            sorts,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SearchMessageResponse::decode(response.as_slice())?)
    }

    /// Search threads by label within a parent channel.
    pub async fn search_thread(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<Vec<ApiThreadDesc>> {
        let cid = self.generate_cid();
        let body = api::SearchThreadRequest {
            clan_id: parse_id(clan_id)?,
            channel_id: parse_id(channel_id)?,
            label: label.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ChannelDescList::decode(response.as_slice())?;
        Ok(list
            .channeldesc
            .into_iter()
            .map(Self::thread_desc_from_proto)
            .collect())
    }

    /// List Mezon OAuth client.
    pub async fn list_mezon_oauth_client(&self) -> Result<api::MezonOauthClientList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListMezonOauthClient", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClientList::decode(response.as_slice())?)
    }

    /// Get Mezon OAuth client.
    pub async fn get_mezon_oauth_client(&self, client_id: &str) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::GetMezonOauthClientRequest {
            client_id: client_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Generate hash channel apps.
    pub async fn generate_hash_channel_apps(
        &self,
        app_id: i64,
    ) -> Result<api::GenerateHashChannelAppsResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateHashChannelAppsRequest { app_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateHashChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateHashChannelAppsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get notification category settings list.
    pub async fn get_channel_category_noti_settings_list(
        &self,
        clan_id: i64,
    ) -> Result<api::NotificationChannelCategorySettingList> {
        let cid = self.generate_cid();
        let body = api::NotificationClan { clan_id }.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCategoryNotiSettingsList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationChannelCategorySettingList::decode(
            response.as_slice(),
        )?)
    }

    /// List muted channels.
    pub async fn list_muted_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        let cid = self.generate_cid();

        let body = api::ListMutedChannelRequest { clan_id }.encode_to_vec();

        let (code, response) = self.send_api_request(cid, "ListMutedChannel", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let muted = api::MutedChannelList::decode(response.as_slice())?;
        Ok(muted
            .muted_list
            .into_iter()
            .map(|channel_id| channel_id.to_string())
            .collect())
    }
}

// ============================================================================
// API Methods - Cold Path (infrequent operations)
// ============================================================================

impl MezonTransport {
    /// Create a new channel.
    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
        channel_private: i32,
    ) -> Result<ApiChannelDesc> {
        let cid = self.generate_cid();

        let category_id = category_id.unwrap_or(0);
        let parent_id = parent_id.unwrap_or(0);
        let body = api::CreateChannelDescRequest {
            clan_id,
            channel_label: channel_label.to_string(),
            r#type: channel_type as i32,
            category_id,
            parent_id,
            channel_private,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "CreateChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let channel = api::ChannelDescription::decode(response.as_slice())?;
        Ok(Self::channel_desc_from_proto(channel))
    }

    pub async fn create_direct_channel(&self, user_ids: &[i64]) -> Result<ApiChannelDesc> {
        let cid = self.generate_cid();

        let body = api::CreateChannelDescRequest {
            clan_id: 0,
            r#type: 3,
            channel_private: 1,
            user_ids: user_ids.to_vec(),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "CreateChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let channel = api::ChannelDescription::decode(response.as_slice())?;
        Ok(Self::channel_desc_from_proto(channel))
    }

    /// Delete a channel.
    pub async fn delete_channel(&self, _channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteChannelDescRequest {
            channel_id: _channel_id,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self
            .send_api_request(cid, "DeleteChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Add a friend.
    pub async fn add_friend(&self, _user_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::AddFriendsRequest {
            ids: vec![_user_id],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "AddFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Delete a friend.
    pub async fn delete_friend(&self, _user_id: i64) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteFriendsRequest {
            ids: vec![_user_id],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "DeleteFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Update channel description.
    pub async fn update_channel_desc(
        &self,
        clan_id: i64,
        channel_id: i64,
        label: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelDescRequest {
            clan_id,
            channel_id,
            channel_label: Some(label.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel private.
    pub async fn update_channel_private(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_private: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelPrivateRequest {
            clan_id,
            channel_id,
            channel_private,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelPrivate", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Change channel category.
    pub async fn change_channel_category(
        &self,
        clan_id: i64,
        channel_id: i64,
        new_category_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelCategoryRequest {
            clan_id,
            channel_id,
            new_category_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ChangeChannelCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel users.
    pub async fn add_channel_users(&self, channel_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddChannelUsersRequest {
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel users.
    pub async fn remove_channel_users(&self, channel_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveChannelUsersRequest {
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Leave thread.
    pub async fn leave_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::LeaveThreadRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "LeaveThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Archive channel.
    pub async fn archive_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ArchiveChannelRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ArchiveChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Reactivate archived thread.
    pub async fn active_archived_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ActiveArchivedThread {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ActiveArchivedThread", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Close DM.
    pub async fn close_dm_by_channel_id(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CloseDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Open DM.
    pub async fn open_dm_by_channel_id(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "OpenDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<ApiClanDesc> {
        let cid = self.generate_cid();
        let body = api::CreateClanDescRequest {
            clan_name: clan_name.to_string(),
            logo: logo.to_string(),
            banner: banner.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let clan = api::ClanDesc::decode(response.as_slice())?;
        Ok(Self::clan_desc_from_proto(clan))
    }

    /// Update clan description and settings.
    pub async fn update_clan_desc(&self, request: api::UpdateClanDescRequest) -> Result<()> {
        let cid = self.generate_cid();
        let body = request.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan.
    pub async fn delete_clan_desc(&self, clan_desc_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteClanDescRequest { clan_desc_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove clan users.
    pub async fn remove_clan_users(&self, clan_id: i64, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveClanUsersRequest {
            clan_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "RemoveClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Ban clan users.
    pub async fn ban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: &[&str],
        ban_time: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id,
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ban_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unban clan users.
    pub async fn unban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: &[&str],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id,
            channel_id,
            user_ids: user_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnbanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create category.
    pub async fn create_category_desc(
        &self,
        category_name: &str,
        clan_id: i64,
    ) -> Result<api::CategoryDesc> {
        let cid = self.generate_cid();
        let body = api::CreateCategoryDescRequest {
            category_name: category_name.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDesc::decode(response.as_slice())?)
    }

    /// Delete category.
    pub async fn delete_category_desc(&self, category_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteCategoryDescRequest {
            category_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category.
    pub async fn update_category(
        &self,
        category_id: i64,
        category_name: &str,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryDescRequest {
            category_id,
            category_name: category_name.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Block friends.
    pub async fn block_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BlockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unblock friends.
    pub async fn unblock_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnblockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update username.
    pub async fn update_username(&self, username: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsernameRequest {
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUsername", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user profile.
    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsersRequest {
            display_name: display_name.to_string(),
            avatar_url: avatar_url.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UpdateUser", body).await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!("UpdateUser error: code={}, response={}", code, msg);
            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(())
    }

    /// Update user profile by clan.
    pub async fn update_user_profile_by_clan(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanProfileRequest {
            clan_id,
            nick_name: Some(nick_name.to_string()),
            avatar: avatar_url.map(|s| s.to_string()),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateUserProfileByClan", body)
            .await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!(
                "UpdateUserProfileByClan error: code={}, response={}",
                code,
                msg
            );
            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(())
    }

    /// Mark as read.
    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MarkAsReadRequest {
            channel_id,
            category_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "MarkAsRead", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user status.
    pub async fn update_user_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUserStatus", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user custom status.
    pub async fn update_user_custom_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateUserCustomStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Session logout.
    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SessionLogoutRequest {
            token: token.to_string(),
            refresh_token: refresh_token.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SessionLogout", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Log out / remove a specific device by device_id.
    /// Uses the current session credentials + target device_id.
    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SessionLogoutRequest {
            token: token.to_string(),
            refresh_token: refresh_token.to_string(),
            device_id: device_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SessionLogout", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification channel setting.
    pub async fn set_notification_channel_setting(
        &self,
        channel_category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id,
            notification_type,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification clan setting.
    pub async fn set_notification_clan_setting(
        &self,
        clan_id: i64,
        notification_type: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetDefaultNotificationRequest {
            clan_id,
            notification_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationClanSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification category setting.
    pub async fn set_notification_category_setting(
        &self,
        channel_category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id,
            notification_type,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute channel.
    pub async fn set_mute_channel(&self, id: i64, mute_time: i32, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id,
            mute_time,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute category.
    pub async fn set_mute_category(&self, id: i64, mute_time: i32, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id,
            mute_time,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notifications.
    pub async fn delete_notifications(&self, ids: &[&str], category: i32) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteNotificationsRequest {
            ids: ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            category,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification category setting.
    pub async fn delete_notification_category_setting(&self, category_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory { category_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification channel.
    pub async fn delete_notification_channel(&self, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel { channel_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set role channel permission.
    pub async fn set_role_channel_permission(&self, role_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleChannelRequest {
            role_id,
            channel_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetRoleChannelPermission", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Check duplicate thread name within a parent channel.
    pub async fn check_duplicate_thread_name(
        &self,
        name: &str,
        parent_channel_id: &str,
    ) -> Result<bool> {
        let resp = self
            .check_duplicate_name(name, CHECK_NAME_TYPE_THREAD, parse_id(parent_channel_id)?)
            .await?;
        Ok(resp.is_duplicate)
    }

    /// Check duplicate name.
    pub async fn check_duplicate_name(
        &self,
        name: &str,
        r#type: i32,
        condition_id: i64,
    ) -> Result<api::CheckDuplicateNameResponse> {
        let cid = self.generate_cid();
        let body = api::CheckDuplicateNameRequest {
            name: name.to_string(),
            r#type,
            condition_id,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CheckDuplicateName", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CheckDuplicateNameResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Upload attachment file.
    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            width,
            height,
            part_count: 0,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UploadAttachmentFile", body)
            .await?;
        if code != 0 {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!(
                "UploadAttachmentFile error: code={}, response={}",
                code,
                msg
            );

            if let Ok(envelope) = realtime::Envelope::decode(response.as_slice())
                && let Some(realtime::envelope::Message::Error(error)) = envelope.message
            {
                return Err(anyhow::anyhow!(
                    "UploadAttachmentFile API error: code={} error={}",
                    error.code,
                    error.message
                ));
            }

            return Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Upload OAuth file.
    pub async fn upload_oauth_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UploadOauthFile", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Push pub key.
    pub async fn push_pub_key(&self, encr: &[u8], sign: &[u8]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::PushPubKeyRequest {
            pk: Some(api::PubKey {
                encr: encr.to_vec(),
                sign: sign.to_vec(),
            }),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "PushPubKey", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set channel encryption method.
    pub async fn set_chan_encryption_method(&self, channel_id: i64, method: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id,
            method: method.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Transfer ownership.
    pub async fn transfer_ownership(&self, clan_id: i64, new_owner_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::TransferOwnershipRequest {
            clan_id,
            new_owner_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "TransferOwnership", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Report message abuse.
    pub async fn report_message_abuse(&self, message_id: i64, abuse_type: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ReportMessageAbuseReqest {
            message_id,
            abuse_type: abuse_type.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReportMessageAbuse", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Message button click.
    pub async fn message_button_click(
        &self,
        message_id: i64,
        channel_id: i64,
        button_id: &str,
        sender_id: i64,
        user_id: i64,
        extra_data: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::MessageButtonClicked {
            message_id,
            channel_id,
            button_id: button_id.to_string(),
            sender_id,
            user_id,
            extra_data: extra_data.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MessageButtonClick", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Dropdown box selected.
    pub async fn dropdown_box_selected(
        &self,
        message_id: i64,
        channel_id: i64,
        selectbox_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::DropdownBoxSelected {
            message_id,
            channel_id,
            selectbox_id: selectbox_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DropdownBoxSelected", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add agent to channel.
    pub async fn add_agent_to_channel(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddAgentToChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Disconnect agent.
    pub async fn disconnect_agent(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DisconnectAgent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Regist FCM device token.
    pub async fn regist_fcm_device_token(
        &self,
        token: &str,
        device_id: &str,
        platform: &str,
    ) -> Result<api::RegistFcmDeviceTokenResponse> {
        let cid = self.generate_cid();
        let body = api::RegistFcmDeviceTokenRequest {
            token: token.to_string(),
            device_id: device_id.to_string(),
            platform: platform.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "RegistFCMDeviceToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RegistFcmDeviceTokenResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Create link invite user.
    pub async fn create_link_invite_user(
        &self,
        clan_id: i64,
        channel_id: i64,
        expiry_time: i32,
    ) -> Result<api::LinkInviteUser> {
        let cid = self.generate_cid();
        let body = api::LinkInviteUserRequest {
            clan_id,
            channel_id,
            expiry_time,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateLinkInviteUser", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::LinkInviteUser::decode(response.as_slice())?)
    }

    /// Invite user.
    pub async fn invite_user(&self, invite_id: i64) -> Result<api::InviteUserRes> {
        let cid = self.generate_cid();
        let body = api::InviteUserRequest { invite_id }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "InviteUser", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::InviteUserRes::decode(response.as_slice())?)
    }

    /// Create activity.
    pub async fn create_activiy(
        &self,
        activity_name: &str,
        activity_type: i32,
    ) -> Result<api::UserActivity> {
        let cid = self.generate_cid();
        let body = api::CreateActivityRequest {
            activity_name: activity_name.to_string(),
            activity_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateActiviy", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserActivity::decode(response.as_slice())?)
    }

    /// Create message to inbox.
    pub async fn create_message_2_inbox(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
        content: &str,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::Message2InboxRequest {
            message_id,
            channel_id,
            clan_id,
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateMessage2Inbox", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Create pin message.
    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            message_id,
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Delete pin message.
    pub async fn delete_pin_message(
        &self,
        id: &str,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        let id = id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid pin id {id:?}: {e}"))?;
        let message_id = message_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid message_id {message_id:?}: {e}"))?;
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id {channel_id:?}: {e}"))?;
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id {clan_id:?}: {e}"))?;
        let cid = self.generate_cid();
        let body = api::DeletePinMessage {
            id,
            message_id,
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeletePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan order.
    pub async fn update_clan_order(&self, clans_order: &[(i32, i64)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanOrderRequest {
            clans_order: clans_order
                .iter()
                .map(|(order, clan_id)| {
                    Ok(api::update_clan_order_request::ClanOrder {
                        order: *order,
                        clan_id: *clan_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category order.
    pub async fn update_category_order(
        &self,
        clan_id: i64,
        categories: &[(i32, i64)],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryOrderRequest {
            clan_id,
            categories: categories
                .iter()
                .map(|(order, category_id)| {
                    Ok(api::CategoryOrderUpdate {
                        order: *order,
                        category_id: *category_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateCategoryOrder", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role order.
    pub async fn update_role_order(&self, clan_id: i64, roles: &[(i32, i64)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleOrderRequest {
            clan_id,
            roles: roles
                .iter()
                .map(|(order, role_id)| {
                    Ok(api::RoleOrderUpdate {
                        order: *order,
                        role_id: *role_id,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRoleOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create event.
    pub async fn create_event(
        &self,
        title: &str,
        clan_id: i64,
        channel_id: i64,
        start_time: u32,
        end_time: u32,
    ) -> Result<api::EventManagement> {
        let cid = self.generate_cid();
        let body = api::CreateEventRequest {
            title: title.to_string(),
            clan_id,
            channel_id,
            start_time_seconds: start_time,
            end_time_seconds: end_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventManagement::decode(response.as_slice())?)
    }

    /// Delete event.
    pub async fn delete_event(&self, event_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteEventRequest {
            event_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update event.
    pub async fn update_event(&self, event_id: i64, clan_id: i64, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateEventRequest {
            event_id,
            clan_id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add user event.
    pub async fn add_user_event(&self, clan_id: i64, event_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest { clan_id, event_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete user event.
    pub async fn delete_user_event(&self, clan_id: i64, event_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest { clan_id, event_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create role.
    pub async fn create_role(&self, title: &str, clan_id: i64) -> Result<api::Role> {
        let cid = self.generate_cid();
        let body = api::CreateRoleRequest {
            title: title.to_string(),
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::Role::decode(response.as_slice())?)
    }

    /// Delete role.
    pub async fn delete_role(&self, role_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role.
    pub async fn update_role(&self, role_id: i64, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleRequest {
            role_id,
            title: Some(title.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete role channel desc.
    pub async fn delete_role_channel_desc(&self, role_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteRoleChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add roles channel desc.
    pub async fn add_roles_channel_desc(&self, role_ids: &[&str], channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddRoleChannelDescRequest {
            role_ids: role_ids
                .iter()
                .map(|s| parse_id(s))
                .collect::<Result<Vec<_>>>()?,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddRolesChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan emoji.
    pub async fn create_clan_emoji(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiCreateRequest {
            clan_id,
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "CreateClanEmoji", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan emoji by ID.
    pub async fn delete_clan_emoji_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiDeleteRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteByIdClanEmoji", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan emoji by ID.
    pub async fn update_clan_emoji_by_id(
        &self,
        id: i64,
        shortname: &str,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiUpdateRequest {
            id,
            shortname: shortname.to_string(),
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanEmojiById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add clan sticker.
    pub async fn add_clan_sticker(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerAddRequest {
            clan_id,
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddClanSticker", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan sticker by ID.
    pub async fn update_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerUpdateByIdRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan sticker by ID.
    pub async fn delete_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerDeleteRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate webhook.
    pub async fn generate_webhook(
        &self,
        webhook_name: &str,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookCreateRequest {
            webhook_name: webhook_name.to_string(),
            channel_id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "GenerateWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update webhook by ID.
    pub async fn update_webhook_by_id(&self, id: i64, webhook_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookUpdateRequestById {
            id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete webhook by ID.
    pub async fn delete_webhook_by_id(&self, id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookDeleteRequestById {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate clan webhook.
    pub async fn generate_clan_webhook(
        &self,
        clan_id: i64,
        webhook_name: &str,
    ) -> Result<api::GenerateClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateClanWebhookRequest {
            clan_id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateClanWebhook", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateClanWebhookResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update clan webhook by ID.
    pub async fn update_clan_webhook_by_id(
        &self,
        id: i64,
        clan_id: i64,
        webhook_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanWebhookRequest {
            id,
            clan_id,
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan webhook by ID.
    pub async fn delete_clan_webhook_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanWebhookRequest { id, clan_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add app.
    pub async fn add_app(&self, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::AddAppRequest {
            appname: appname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "AddApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Delete app.
    pub async fn delete_app(&self, id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::App {
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update app.
    pub async fn update_app(&self, id: i64, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::UpdateAppRequest {
            id,
            appname: Some(appname.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UpdateApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Add app to clan.
    pub async fn add_app_to_clan(&self, app_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AppClan { app_id, clan_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddAppToClan", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create system message.
    pub async fn create_system_message(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SystemMessageRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CreateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update system message.
    pub async fn update_system_message(&self, request: api::SystemMessageRequest) -> Result<()> {
        let cid = self.generate_cid();
        let body = request.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete system message.
    pub async fn delete_system_message(&self, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteSystemMessage { clan_id }.encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Edit channel canvases.
    pub async fn edit_channel_canvases(
        &self,
        channel_id: i64,
        clan_id: i64,
        title: &str,
        content: &str,
    ) -> Result<api::EditChannelCanvasResponse> {
        let cid = self.generate_cid();
        let body = api::EditChannelCanvasRequest {
            channel_id,
            clan_id,
            title: title.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "EditChannelCanvases", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EditChannelCanvasResponse::decode(response.as_slice())?)
    }

    /// Delete channel canvas.
    pub async fn delete_channel_canvas(
        &self,
        canvas_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelCanvasRequest {
            canvas_id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelCanvas", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel favorite.
    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddFavoriteChannelRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel favorite.
    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveFavoriteChannelRequest {
            channel_id,
            clan_id,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create onboarding.
    pub async fn create_onboarding(&self, clan_id: i64) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::CreateOnboardingRequest {
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Update onboarding.
    pub async fn update_onboarding(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingRequest {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete onboarding.
    pub async fn delete_onboarding(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest { id, clan_id }.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update onboarding step.
    pub async fn update_onboarding_step(&self, clan_id: i64, onboarding_step: i32) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingStepRequest {
            clan_id,
            onboarding_step,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create Sd topic.
    pub async fn create_sd_topic(
        &self,
        message_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicRequest {
            message_id,
            clan_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// Create external Mezon meet.
    pub async fn create_external_mezon_meet(&self) -> Result<api::GenerateMezonMeetResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "CreateExternalMezonMeet", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMezonMeetResponse::decode(response.as_slice())?)
    }

    /// Generate meet token.
    pub async fn generate_meet_token(
        &self,
        channel_id: i64,
        room_name: &str,
    ) -> Result<api::GenerateMeetTokenResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateMeetTokenRequest {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateMeetToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMeetTokenResponse::decode(response.as_slice())?)
    }

    /// Remove participant Mezon meet.
    pub async fn remove_participant_mezon_meet(
        &self,
        channel_id: i64,
        clan_id: i64,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id,
            clan_id,
            room_name: room_name.to_string(),
            username: username.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Mute participant Mezon meet.
    pub async fn mute_participant_mezon_meet(
        &self,
        channel_id: i64,
        clan_id: i64,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id,
            clan_id,
            room_name: room_name.to_string(),
            username: username.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MuteParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create room channel apps.
    pub async fn create_room_channel_apps(
        &self,
        channel_id: i64,
        room_name: &str,
    ) -> Result<api::CreateRoomChannelApps> {
        let cid = self.generate_cid();
        let body = api::CreateRoomChannelApps {
            channel_id,
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateRoomChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateRoomChannelApps::decode(response.as_slice())?)
    }

    /// Update Mezon OAuth client.
    pub async fn update_mezon_oauth_client(
        &self,
        client_id: &str,
        client_name: &str,
    ) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::MezonOauthClient {
            client_id: client_id.to_string(),
            client_name: client_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Add quick menu access.
    pub async fn add_quick_menu_access(
        &self,
        bot_id: i64,
        clan_id: i64,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id,
            clan_id,
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update quick menu access.
    pub async fn update_quick_menu_access(
        &self,
        bot_id: i64,
        clan_id: i64,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id,
            clan_id,
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete quick menu access.
    pub async fn delete_quick_menu_access(&self, id: i64, clan_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            id,
            clan_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel message.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        mentions: Vec<OutgoingMention>,
        hashtags: Vec<OutgoingHashtag>,
        emojis: Vec<OutgoingEmoji>,
        mode: i32,
        is_public: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let markdowns = detect_markdown(content);
        let content_json =
            build_message_content_json(content, &mentions, &hashtags, &emojis, &markdowns);
        let proto_mentions: Vec<api::MessageMention> = mentions
            .iter()
            .filter_map(OutgoingMention::to_proto)
            .collect();
        let body = realtime::ChannelMessageUpdate {
            clan_id,
            channel_id,
            message_id,
            content: content_json,
            mentions: proto_mentions,
            mode,
            is_public,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete channel message.
    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ChannelMessageRemove {
            clan_id,
            channel_id,
            message_id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// React channel message.
    #[allow(clippy::too_many_arguments)]
    pub async fn react_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        emoji_id: i64,
        emoji: &str,
        count: i32,
        message_sender_id: i64,
        mode: i32,
        is_public: bool,
        remove: bool,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MessageReaction {
            clan_id,
            channel_id,
            message_id,
            emoji_id,
            emoji: emoji.to_string(),
            count,
            message_sender_id,
            mode,
            is_public,
            action: remove,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReactChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create poll.
    pub async fn create_poll(
        &self,
        channel_id: i64,
        clan_id: i64,
        question: &str,
    ) -> Result<api::CreatePollResponse> {
        let cid = self.generate_cid();
        let body = api::CreatePollRequest {
            channel_id,
            clan_id,
            question: question.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreatePollResponse::decode(response.as_slice())?)
    }

    /// Vote poll.
    pub async fn vote_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
        answer_indices: Vec<i32>,
    ) -> Result<api::VotePollResponse> {
        let cid = self.generate_cid();
        let body = api::VotePollRequest {
            poll_id,
            message_id,
            channel_id,
            answer_indices,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "VotePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VotePollResponse::decode(response.as_slice())?)
    }

    /// Close poll.
    pub async fn close_poll(&self, poll_id: i64, message_id: i64, channel_id: i64) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClosePollRequest {
            poll_id,
            message_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ClosePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Get poll.
    pub async fn get_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<api::GetPollResponse> {
        let cid = self.generate_cid();
        let body = api::GetPollRequest {
            poll_id,
            message_id,
            channel_id,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPollResponse::decode(response.as_slice())?)
    }

    /// Create channel timeline.
    pub async fn create_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        title: &str,
    ) -> Result<api::CreateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::CreateChannelTimelineRequest {
            clan_id,
            channel_id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update channel timeline.
    pub async fn update_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
        title: &str,
    ) -> Result<api::UpdateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelTimelineRequest {
            clan_id,
            channel_id,
            id,
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UpdateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Detail channel timeline.
    pub async fn detail_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
    ) -> Result<api::ChannelTimelineDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelTimelineDetailRequest {
            clan_id,
            channel_id,
            id,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "DetailChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelTimelineDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update user account.
    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::UpdateAccountRequest {
            display_name: display_name
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            avatar_url: avatar_url.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            about_me: about_me.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, "UpdateAccount", body).await?;

        tracing::debug!(
            "UpdateAccount response: code={}, response={:?}",
            code,
            response
        );

        if code == 0 {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&response);
            tracing::error!("UpdateAccount error: code={}, response={}", code, msg);
            Err(anyhow::anyhow!(
                "API error: code={}, response={}",
                code,
                msg
            ))
        }
    }

    /// Delete user account.
    pub async fn delete_account(&self) -> Result<()> {
        let cid = self.generate_cid();

        let (code, _response) = self
            .send_api_request(cid, "DeleteAccount", Vec::new())
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Authenticate against the server with a refresh token.
    pub async fn session_refresh(&self, token: &str, is_remember: bool) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = api::SessionRefreshRequest {
            token: token.to_string(),
            is_remember,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SessionRefresh", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Register email.
    pub async fn registration_email(
        &self,
        req: api::RegistrationEmailRequest,
    ) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "RegistrationEmail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Link email.
    pub async fn link_email(&self, req: api::AccountEmail) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "LinkEmail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Unlink email.
    pub async fn unlink_email(&self, req: api::AccountEmail) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnlinkEmail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Link SMS.
    pub async fn link_sms(&self, req: api::AccountMezon) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "LinkSMS", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Unlink Mezon (SMS).
    pub async fn unlink_mezon(&self, req: api::AccountMezon) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnlinkMezon", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Confirm link Mezon OTP.
    pub async fn confirm_link_mezon_otp(
        &self,
        req: api::LinkAccountConfirmRequest,
    ) -> Result<ApiSession> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ConfirmLinkMezonOTP", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let session = api::Session::decode(response.as_slice())?;
        Ok(ApiSession {
            token: session.token,
            refresh_token: session.refresh_token,
            user_id: session.user_id,
        })
    }

    /// Multipart upload attachment file start.
    pub async fn multipart_upload_attachment_file_start(
        &self,
        req: api::UploadAttachmentRequest,
    ) -> Result<api::MultipartUploadAttachment> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request_with_timeout(
                cid,
                "MultipartUploadAttachmentFileStart",
                body,
                Duration::from_millis(MULTIPART_OP_TIMEOUT_MS),
            )
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MultipartUploadAttachment::decode(response.as_slice())?)
    }

    /// Multipart upload attachment file finish.
    pub async fn multipart_upload_attachment_file_finish(
        &self,
        req: api::MultipartUploadAttachmentFinishRequest,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request_with_timeout(
                cid,
                "MultipartUploadAttachmentFileFinish",
                body,
                Duration::from_millis(MULTIPART_OP_TIMEOUT_MS),
            )
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Upload batch attachment file.
    pub async fn upload_batch_attachment_file(
        &self,
        req: api::UploadBatchAttachmentRequest,
    ) -> Result<api::UploadAttachmentBatch> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UploadBatchAttachmentFile", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachmentBatch::decode(response.as_slice())?)
    }

    /// Delete SD topic.
    pub async fn delete_sd_topic(&self, req: api::DeleteSdTopicRequest) -> Result<()> {
        let cid = self.generate_cid();
        let body = req.encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_content_deserializes_flexible_scalar_and_answer_types() {
        let json = r#"{
            "t": "",
            "poll_id": "123",
            "question": "Q?",
            "answers": ["A", {"index": 1, "label": "B"}],
            "answer_counts": ["3", 1, "x"],
            "total_votes": "4",
            "is_closed": "true",
            "expire_at": "1700000000",
            "type": 1
        }"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.poll_id, Some(123));
        assert_eq!(c.answers.len(), 2);
        assert_eq!(c.answers[0].label, "A");
        assert_eq!(c.answers[0].index, None);
        assert_eq!(c.answers[1].index, Some(1));
        assert_eq!(c.answers[1].label, "B");
        assert_eq!(c.answer_counts, vec![3, 1]);
        assert_eq!(c.total_votes, Some(4));
        assert!(c.is_closed);
        assert_eq!(c.expire_at, Some(1_700_000_000));
        assert_eq!(c.poll_type, Some(1));
    }

    #[test]
    fn poll_id_falls_back_to_id_alias_and_bool_flex_accepts_numeric() {
        let c: ApiMessageContent =
            serde_json::from_str(r#"{"t":"","id":456,"is_closed":1}"#).expect("content");
        assert_eq!(c.poll_id, Some(456));
        assert!(c.is_closed);
        let c2: ApiMessageContent =
            serde_json::from_str(r#"{"t":"","is_closed":false}"#).expect("content");
        assert!(!c2.is_closed);
    }

    fn user_mention(user_id: &str, s: i32, e: i32) -> OutgoingMention {
        OutgoingMention {
            user_id: user_id.into(),
            role_id: String::new(),
            username: "@bob".into(),
            s,
            e,
        }
    }

    fn here_mention(s: i32, e: i32) -> OutgoingMention {
        OutgoingMention {
            user_id: MENTION_HERE_ID.into(),
            role_id: String::new(),
            username: "@here".into(),
            s,
            e,
        }
    }

    #[test]
    fn here_mention_sets_everyone_and_is_excluded_from_proto() {
        let mentions = [user_mention("42", 0, 4), here_mention(5, 10)];
        let everyone = mentions.iter().any(OutgoingMention::is_here);
        let proto: Vec<_> = mentions
            .iter()
            .filter_map(OutgoingMention::to_proto)
            .collect();
        assert!(everyone);
        assert_eq!(proto.len(), 1);
        assert_eq!(proto[0].user_id, 42);
        assert_eq!(proto[0].s, 0);
        assert_eq!(proto[0].e, 4);
        assert_eq!(proto[0].username, "@bob");
    }

    #[test]
    fn no_here_mention_leaves_everyone_unset() {
        let mentions = [user_mention("7", 0, 4)];
        assert!(!mentions.iter().any(OutgoingMention::is_here));
    }

    #[test]
    fn non_numeric_user_id_is_dropped_not_zeroed() {
        let bad = user_mention("not-a-number", 0, 4);
        assert!(bad.to_proto().is_none());
    }

    #[test]
    fn message_from_proto_prefers_clan_avatar() {
        let msg = api::ChannelMessage {
            message_id: 1,
            sender_id: 42,
            avatar: "user.png".into(),
            clan_avatar: "clan.png".into(),
            content: r#"{"t":"hi"}"#.into(),
            ..Default::default()
        };
        let parsed = MezonTransport::message_from_proto(&msg);
        assert_eq!(parsed.avatar, "clan.png");
        assert_eq!(parsed.content, "hi");
    }

    #[test]
    fn message_from_proto_falls_back_to_user_avatar() {
        let msg = api::ChannelMessage {
            message_id: 1,
            avatar: "user.png".into(),
            content: r#"{"t":"hi"}"#.into(),
            ..Default::default()
        };
        let parsed = MezonTransport::message_from_proto(&msg);
        assert_eq!(parsed.avatar, "user.png");
    }

    #[test]
    fn message_from_proto_captures_presign_finish_from_content() {
        let msg = api::ChannelMessage {
            message_id: 1,
            content: r#"{"t":"hi","presign_finish":["a/b/photo.png"]}"#.into(),
            ..Default::default()
        };
        let parsed = MezonTransport::message_from_proto(&msg);
        assert_eq!(parsed.content, "hi");
        assert_eq!(
            parsed.content_tokens.presign_finish,
            Some(vec!["a/b/photo.png".to_string()])
        );
    }

    #[test]
    fn message_from_proto_presign_finish_absent_is_none() {
        let msg = api::ChannelMessage {
            message_id: 1,
            content: r#"{"t":"hi"}"#.into(),
            ..Default::default()
        };
        let parsed = MezonTransport::message_from_proto(&msg);
        assert_eq!(parsed.content_tokens.presign_finish, None);
    }

    #[test]
    fn content_json_plain_has_no_mentions_key() {
        let json = build_message_content_json("hello", &[], &[], &[], &[]);
        assert_eq!(json, r#"{"t":"hello"}"#);
    }

    #[test]
    fn content_json_includes_mentions_with_utf16_offsets() {
        let json =
            build_message_content_json("@bob hi", &[user_mention("42", 0, 4)], &[], &[], &[]);
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.t, "@bob hi");
        assert_eq!(parsed.mentions.len(), 1);
        assert_eq!(parsed.mentions[0].user_id.as_deref(), Some("42"));
        assert_eq!(parsed.mentions[0].s, Some(0));
        assert_eq!(parsed.mentions[0].e, Some(4));
    }

    #[test]
    fn content_json_keeps_here_mention_for_receive_colouring() {
        let json = build_message_content_json("@here", &[here_mention(0, 5)], &[], &[], &[]);
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mentions.len(), 1);
        assert_eq!(parsed.mentions[0].user_id.as_deref(), Some("here"));
    }

    fn role_mention(role_id: &str, s: i32, e: i32) -> OutgoingMention {
        OutgoingMention {
            user_id: String::new(),
            role_id: role_id.into(),
            username: "@mods".into(),
            s,
            e,
        }
    }

    #[test]
    fn content_json_includes_hashtags_with_channel_id() {
        let hashtags = [OutgoingHashtag {
            channel_id: "777".into(),
            s: 0,
            e: 8,
        }];
        let json = build_message_content_json("#general hi", &[], &hashtags, &[], &[]);
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert!(parsed.mentions.is_empty());
        assert_eq!(parsed.hg.len(), 1);
        assert_eq!(parsed.hg[0].channel_id.as_deref(), Some("777"));
        assert_eq!(parsed.hg[0].s, Some(0));
        assert_eq!(parsed.hg[0].e, Some(8));
    }

    #[test]
    fn content_json_includes_emojis_with_emojiid() {
        let emojis = [OutgoingEmoji {
            emoji_id: "55".into(),
            s: 0,
            e: 7,
        }];
        let json = build_message_content_json(":smile:", &[], &[], &emojis, &[]);
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ej.len(), 1);
        assert_eq!(parsed.ej[0].emojiid.as_deref(), Some("55"));
        assert_eq!(parsed.ej[0].s, Some(0));
        assert_eq!(parsed.ej[0].e, Some(7));
    }

    fn md(kind: &str, s: i32, e: i32) -> OutgoingMarkdown {
        OutgoingMarkdown {
            kind: kind.into(),
            s,
            e,
        }
    }

    #[test]
    fn detect_markdown_bold_keeps_markers_in_offsets() {
        assert_eq!(detect_markdown("**hi**"), vec![md("b", 0, 6)]);
    }

    #[test]
    fn detect_markdown_inline_code() {
        assert_eq!(detect_markdown("`x`"), vec![md("c", 0, 3)]);
    }

    #[test]
    fn detect_markdown_code_block_takes_precedence_over_inline() {
        assert_eq!(detect_markdown("```code```"), vec![md("pre", 0, 10)]);
        assert_eq!(detect_markdown("```a```"), vec![md("pre", 0, 7)]);
    }

    #[test]
    fn detect_markdown_link_has_no_trailing_punctuation_trim() {
        assert_eq!(detect_markdown("see https://a.com."), vec![md("lk", 4, 18)]);
    }

    #[test]
    fn detect_markdown_link_inside_code_block_is_not_separately_detected() {
        assert_eq!(
            detect_markdown("```http://x.com```"),
            vec![md("pre", 0, 18)]
        );
    }

    #[test]
    fn detect_markdown_uses_utf16_offsets() {
        assert_eq!(detect_markdown("😀 **b**"), vec![md("b", 3, 8)]);
    }

    #[test]
    fn detect_markdown_ignores_unclosed_and_blank() {
        assert!(detect_markdown("**hi").is_empty());
        assert!(detect_markdown("``").is_empty());
        assert!(detect_markdown("** **").is_empty());
    }

    #[test]
    fn content_json_includes_markdown_mk() {
        let json = build_message_content_json("**hey**", &[], &[], &[], &[md("b", 0, 7)]);
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.t, "**hey**");
        assert_eq!(parsed.mk.len(), 1);
        assert_eq!(parsed.mk[0].kind.as_deref(), Some("b"));
        assert_eq!(parsed.mk[0].s, Some(0));
        assert_eq!(parsed.mk[0].e, Some(7));
    }

    #[test]
    fn content_json_markdown_keeps_other_token_offsets_unshifted() {
        let json = build_message_content_json(
            "**hi** @bob",
            &[user_mention("42", 7, 11)],
            &[],
            &[],
            &[md("b", 0, 6)],
        );
        let parsed: ApiMessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.t, "**hi** @bob");
        assert_eq!(parsed.mentions[0].s, Some(7));
        assert_eq!(parsed.mentions[0].e, Some(11));
        assert_eq!(parsed.mk[0].s, Some(0));
        assert_eq!(parsed.mk[0].e, Some(6));
    }

    fn reply_reference_bytes(message_sender_id: i64) -> Vec<u8> {
        api::MessageRefList {
            refs: vec![api::MessageRef {
                message_sender_id,
                ..Default::default()
            }],
        }
        .encode_to_vec()
    }

    #[test]
    fn mention_or_reply_plain_message_is_false() {
        let content = build_message_content_json("hello world", &[], &[], &[], &[]);
        assert!(!is_mention_or_reply(&content, &[], &[], 42, &[]));
    }

    #[test]
    fn mention_or_reply_detects_here() {
        let content = build_message_content_json("@here", &[here_mention(0, 5)], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &[], 42, &[]));
    }

    #[test]
    fn mention_or_reply_detects_current_user_only() {
        let content =
            build_message_content_json("@bob", &[user_mention("42", 0, 4)], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &[], 42, &[]));
        assert!(!is_mention_or_reply(&content, &[], &[], 7, &[]));
    }

    #[test]
    fn mention_or_reply_detects_matching_role_only() {
        let content =
            build_message_content_json("@mods", &[role_mention("99", 0, 5)], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &[], 42, &[99]));
        assert!(!is_mention_or_reply(&content, &[], &[], 42, &[100]));
    }

    #[test]
    fn mention_or_reply_detects_reply_to_user() {
        let refs = reply_reference_bytes(42);
        let content = build_message_content_json("re", &[], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &refs, &[], 42, &[]));
        assert!(!is_mention_or_reply(&content, &refs, &[], 7, &[]));
    }

    #[test]
    fn mention_or_reply_malformed_content_is_false() {
        assert!(!is_mention_or_reply("not json", &[], &[], 42, &[]));
    }

    #[test]
    fn mention_or_reply_detects_proto_entity_mentions() {
        let list = api::MessageMentionList {
            mentions: vec![api::MessageMention {
                user_id: 42,
                username: "@bob".into(),
                s: 0,
                e: 4,
                ..Default::default()
            }],
        };
        let bytes = list.encode_to_vec();
        let content = build_message_content_json("hello", &[], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &bytes, 42, &[]));
        assert!(!is_mention_or_reply(&content, &[], &bytes, 7, &[]));
    }

    #[test]
    fn mention_or_reply_detects_proto_role_mention() {
        let bytes = api::MessageMentionList {
            mentions: vec![api::MessageMention {
                role_id: 99,
                rolename: "@mods".into(),
                s: 0,
                e: 5,
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let content = build_message_content_json("hello", &[], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &bytes, 42, &[99]));
        assert!(!is_mention_or_reply(&content, &[], &bytes, 42, &[100]));
    }

    #[test]
    fn mention_or_reply_detects_proto_here_mention() {
        let here_user_id = MENTION_HERE_USER_ID.parse::<i64>().unwrap();
        let bytes = api::MessageMentionList {
            mentions: vec![api::MessageMention {
                user_id: here_user_id,
                username: "@here".into(),
                s: 0,
                e: 5,
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let content = build_message_content_json("@here", &[], &[], &[], &[]);
        assert!(is_mention_or_reply(&content, &[], &bytes, 7, &[]));
    }

    #[test]
    fn mention_or_reply_malformed_proto_mentions_is_false() {
        let content = build_message_content_json("hello", &[], &[], &[], &[]);
        assert!(!is_mention_or_reply(
            &content,
            &[],
            &[0xff, 0xff, 0xff, 0xff],
            42,
            &[]
        ));
    }

    #[test]
    fn notification_content_extracts_message_id_and_time() {
        let json = br#"{"message_id":"12345","create_time_seconds":1700000000}"#;
        assert_eq!(parse_notification_content(json), (12345, 1700000000));
    }

    #[test]
    fn notification_content_missing_fields_is_zero() {
        let json = br#"{"t":"hi @bob"}"#;
        assert_eq!(parse_notification_content(json), (0, 0));
    }

    #[test]
    fn notification_content_malformed_is_zero() {
        assert_eq!(parse_notification_content(&[0xff, 0x00, 0x12]), (0, 0));
        assert_eq!(parse_notification_content(&[]), (0, 0));
    }

    #[test]
    fn notification_content_decodes_direct_fcm_protobuf() {
        let fcm = api::DirectFcmProto {
            message_id: 987654321,
            create_time_seconds: 1700000000,
            content: "hi @bob".into(),
            ..Default::default()
        };
        let bytes = fcm.encode_to_vec();
        assert_eq!(parse_notification_content(&bytes), (987654321, 1700000000));
    }

    #[test]
    fn notification_content_accepts_numeric_message_id() {
        let json = br#"{"message_id":12345,"create_time_seconds":1700000000}"#;
        assert_eq!(parse_notification_content(json), (12345, 1700000000));
    }

    #[test]
    fn notification_content_keeps_message_id_when_time_is_unexpected_type() {
        let json = br#"{"message_id":"12345","create_time_seconds":"1700000000"}"#;
        assert_eq!(parse_notification_content(json), (12345, 1700000000));
    }

    #[test]
    fn notification_content_keeps_message_id_when_time_missing() {
        let json = br#"{"message_id":"98765","content":"{\"t\":\"hi\"}"}"#;
        assert_eq!(parse_notification_content(json), (98765, 0));
    }

    struct MockAdapter {
        send_ok: bool,
    }

    #[async_trait::async_trait]
    impl TransportAdapter for MockAdapter {
        async fn connect(&self, _host: &str, _port: u16, _token: &str) -> Result<()> {
            Ok(())
        }
        async fn send(&self, _message: Vec<u8>) -> Result<()> {
            if self.send_ok {
                Ok(())
            } else {
                Err(anyhow::anyhow!("mock send failed"))
            }
        }
        async fn send_ping(&self, _cid: u16) -> Result<()> {
            Ok(())
        }
        fn is_open(&self) -> bool {
            true
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
        async fn set_on_message(&self, _handler: crate::transport_adapter::MessageHandler) {}
        async fn set_on_open(&self, _handler: crate::transport_adapter::OpenHandler) {}
        async fn set_on_close(&self, _handler: crate::transport_adapter::CloseHandler) {}
        async fn set_on_error(&self, _handler: crate::transport_adapter::ErrorHandler) {}
    }

    fn transport(send_ok: bool) -> MezonTransport {
        MezonTransport::new(Box::new(MockAdapter { send_ok }), String::new())
    }

    #[tokio::test]
    async fn gate_passes_immediately_when_connected() {
        let t = transport(true);
        t.connected_tx.send(true).unwrap();
        t.wait_connected(Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn gate_times_out_when_never_connected() {
        let t = transport(true);
        let err = t
            .wait_connected(Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn gate_resolves_when_connection_arrives_mid_wait() {
        let t = transport(true);
        let tx = t.connected_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(true);
        });
        t.wait_connected(Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn send_gate_times_out_and_inserts_no_pending_when_disconnected() {
        let mut t = transport(true);
        t.connect_gate = Duration::from_millis(50);
        let cid = t.generate_cid();
        let err = t.send(cid, vec![1, 2, 3, 4]).await.unwrap_err();
        assert!(err.to_string().contains("not connected"));
        assert!(t.pending_requests.lock().is_empty());
    }

    #[tokio::test]
    async fn send_removes_pending_when_adapter_send_fails() {
        let mut t = transport(false);
        t.connect_gate = Duration::from_millis(50);
        t.connected_tx.send(true).unwrap();
        let cid = t.generate_cid();
        let err = t.send(cid, vec![1, 2, 3, 4]).await.unwrap_err();
        assert!(err.to_string().contains("mock send failed"));
        assert!(t.pending_requests.lock().is_empty());
    }

    #[test]
    fn api_index_pins_known_names_and_rejects_unknown() {
        let t = transport(true);
        assert_eq!(t.get_api_index("ListChannelDescs"), Some(0));
        assert_eq!(t.get_api_index("GetAccount"), Some(1));
        assert_eq!(t.get_api_index("ListClanDescs"), Some(2));
        assert_eq!(t.get_api_index("ListChannelMessages"), Some(30));
        assert_eq!(t.get_api_index("UploadBatchAttachmentFile"), Some(209));
        assert_eq!(t.get_api_index("DefinitelyNotAnApi"), None);
    }

    #[test]
    fn embed_content_deserializes_full_tree() {
        let json = r##"{
            "t": "",
            "embed": [{
                "color": "#5865F2",
                "title": "Release notes",
                "url": "https://mezon.ai/blog",
                "author": {"name": "Mezon Bot", "icon_url": "https://cdn.mezon.ai/a.png", "url": "https://mezon.ai"},
                "description": "**Bold** body",
                "thumbnail": {"url": "https://cdn.mezon.ai/thumb.png"},
                "fields": [
                    {"name": "Version", "value": "1.4.69", "inline": true},
                    {"name": "Notes", "value": "line1\nline2"}
                ],
                "image": {"url": "https://cdn.mezon.ai/img.png", "width": 640, "height": 360},
                "timestamp": "2026-07-04T00:00:00Z",
                "footer": {"text": "Mezon", "icon_url": "https://cdn.mezon.ai/f.png"}
            }]
        }"##;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.embed.len(), 1);
        let e = &c.embed[0];
        assert_eq!(e.color.as_deref(), Some("#5865F2"));
        assert_eq!(e.title.as_deref(), Some("Release notes"));
        assert_eq!(e.url.as_deref(), Some("https://mezon.ai/blog"));
        let author = e.author.as_ref().expect("author");
        assert_eq!(author.name, "Mezon Bot");
        assert_eq!(
            author.icon_url.as_deref(),
            Some("https://cdn.mezon.ai/a.png")
        );
        assert_eq!(e.description.as_deref(), Some("**Bold** body"));
        assert_eq!(
            e.thumbnail.as_ref().map(|t| t.url.as_str()),
            Some("https://cdn.mezon.ai/thumb.png")
        );
        assert_eq!(e.fields.len(), 2);
        assert_eq!(e.fields[0].name, "Version");
        assert_eq!(e.fields[0].value, "1.4.69");
        assert!(e.fields[0].inline);
        assert!(!e.fields[1].inline);
        let img = e.image.as_ref().expect("image");
        assert_eq!(img.url, "https://cdn.mezon.ai/img.png");
        assert_eq!(img.width, Some(640));
        assert_eq!(img.height, Some(360));
        assert_eq!(e.timestamp.as_deref(), Some("2026-07-04T00:00:00Z"));
        let footer = e.footer.as_ref().expect("footer");
        assert_eq!(footer.text, "Mezon");
    }

    #[test]
    fn components_deserialize_button_and_select_rows() {
        let json = r#"{
            "t": "",
            "components": [
                {"components": [
                    {"type": 1, "id": "btn1", "component": {"label": "Play", "style": 3, "url": "https://mezon.ai", "icon": "PLAY"}},
                    {"type": 1, "id": "btn2", "component": {"label": "Cancel", "style": 4, "disable": true}}
                ]},
                {"components": [
                    {"type": 2, "id": "sel1", "max_options": 2, "component": {
                        "type": 1,
                        "placeholder": "Pick",
                        "min_options": 1,
                        "max_options": 2,
                        "options": [
                            {"label": "One", "value": "1", "default": true},
                            {"label": "Two", "value": "2", "description": "second"}
                        ]
                    }}
                ]}
            ]
        }"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.components.len(), 2);

        let button_row = &c.components[0];
        assert_eq!(button_row.components.len(), 2);
        let first = &button_row.components[0];
        assert_eq!(first.component_type, 1);
        assert_eq!(first.id.as_deref(), Some("btn1"));
        match &first.component {
            ApiComponentPayload::Button(b) => {
                assert_eq!(b.label, "Play");
                assert_eq!(b.style, Some(3));
                assert_eq!(b.url.as_deref(), Some("https://mezon.ai"));
                assert_eq!(b.icon.as_deref(), Some("PLAY"));
                assert!(!b.disable);
            }
            other => panic!("expected button, got {other:?}"),
        }
        match &button_row.components[1].component {
            ApiComponentPayload::Button(b) => {
                assert_eq!(b.label, "Cancel");
                assert!(b.disable);
                assert_eq!(b.style, Some(4));
            }
            other => panic!("expected button, got {other:?}"),
        }

        let select_row = &c.components[1];
        assert_eq!(select_row.components.len(), 1);
        let select = &select_row.components[0];
        assert_eq!(select.component_type, 2);
        assert_eq!(select.max_options, Some(2));
        match &select.component {
            ApiComponentPayload::Select(s) => {
                assert_eq!(s.select_type, Some(1));
                assert_eq!(s.placeholder.as_deref(), Some("Pick"));
                assert_eq!(s.min_options, Some(1));
                assert_eq!(s.max_options, Some(2));
                assert_eq!(s.options.len(), 2);
                assert_eq!(s.options[0].label, "One");
                assert_eq!(s.options[0].value, "1");
                assert!(s.options[0].default);
                assert_eq!(s.options[1].description.as_deref(), Some("second"));
                assert!(!s.options[1].default);
            }
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn unknown_component_type_is_preserved_not_dropped() {
        let json = r#"{
            "t": "",
            "components": [
                {"components": [
                    {"type": 4, "id": "dp", "component": {"value": "2026-07-04"}}
                ]}
            ]
        }"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.components.len(), 1);
        assert_eq!(c.components[0].components.len(), 1);
        let comp = &c.components[0].components[0];
        assert_eq!(comp.component_type, 4);
        match &comp.component {
            ApiComponentPayload::Other(v) => {
                assert_eq!(v.get("value").and_then(|x| x.as_str()), Some("2026-07-04"));
            }
            other => panic!("expected other, got {other:?}"),
        }
    }

    #[test]
    fn call_log_content_deserializes_camel_case_keys() {
        let json =
            r#"{"t": "", "callLog": {"isVideo": true, "callLogType": 2, "showCallBack": true}}"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        let call_log = c.call_log.expect("call_log");
        assert!(call_log.is_video);
        assert_eq!(call_log.call_log_type, 2);
        assert!(call_log.show_call_back);
    }

    #[test]
    fn scalar_content_fields_and_tokens_deserialize() {
        let json = r#"{
            "t": "hi",
            "isCard": true,
            "tp": "1775731111020111321",
            "cid": 42,
            "vk": [{"s": 0, "e": 4, "url": "https://mezon.ai/voice"}],
            "cvtt": {"1234567890123456789": "My Canvas"},
            "lky": [{"s": 0, "e": 4, "url": "https://youtu.be/x"}]
        }"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert!(c.is_card);
        assert_eq!(c.tp.as_deref(), Some("1775731111020111321"));
        assert_eq!(c.cid.as_deref(), Some("42"));
        assert_eq!(c.vk.len(), 1);
        assert_eq!(c.vk[0].url.as_deref(), Some("https://mezon.ai/voice"));
        assert_eq!(
            c.cvtt.get("1234567890123456789").map(String::as_str),
            Some("My Canvas")
        );
        assert_eq!(c.lky.len(), 1);
        assert_eq!(c.lky[0].url.as_deref(), Some("https://youtu.be/x"));
    }

    #[test]
    fn ogp_invite_token_exposes_extended_fields() {
        let json = r#"{
            "t": "join",
            "mk": [{
                "s": 0,
                "e": 4,
                "type": "ogp",
                "title": "Cool Clan",
                "description": "come in",
                "image": "https://cdn.mezon.ai/i.png",
                "url": "https://mezon.ai/invite/abc",
                "banner": "https://cdn.mezon.ai/b.png",
                "member_count": 128,
                "is_community": true,
                "clanId": "1775731111020111321"
            }]
        }"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.mk.len(), 1);
        let tok = &c.mk[0];
        assert_eq!(tok.banner.as_deref(), Some("https://cdn.mezon.ai/b.png"));
        assert_eq!(tok.member_count, Some(128));
        assert!(tok.is_community);
        assert_eq!(tok.clan_id.as_deref(), Some("1775731111020111321"));
    }

    #[test]
    fn attachment_size_round_trips() {
        let att = ApiAttachment {
            url: "https://cdn.mezon.ai/f.pdf".into(),
            filename: "f.pdf".into(),
            filetype: "application/pdf".into(),
            width: 0,
            height: 0,
            thumbnail: String::new(),
            duration: 0,
            size: 204_800,
        };
        let json = serde_json::to_string(&att).expect("serialize");
        let back: ApiAttachment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.size, 204_800);
    }

    #[test]
    fn extra_content_keys_are_tolerated() {
        let json = r#"{"t": "hi", "callLog": {"isVideo": false, "callLogType": 1}, "brand_new_key": {"x": 1}}"#;
        let c: ApiMessageContent = serde_json::from_str(json).expect("content");
        assert_eq!(c.t, "hi");
        let call_log = c.call_log.expect("call_log");
        assert!(!call_log.is_video);
        assert_eq!(call_log.call_log_type, 1);
        assert!(!call_log.show_call_back);
    }

    #[test]
    fn parse_search_attachment_reads_json_object_wrapper() {
        let raw = r#"{"attachments":[{"url":"https://cdn/a.jpg","filetype":"image/jpeg","width":400,"height":300}]}"#;
        let parsed = parse_search_attachment_field(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://cdn/a.jpg");
    }

    #[test]
    fn parse_search_attachment_reads_json_array() {
        let raw = r#"[{"url":"https://cdn/b.png","filetype":"image/png"}]"#;
        let parsed = parse_search_attachment_field(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://cdn/b.png");
    }
}
