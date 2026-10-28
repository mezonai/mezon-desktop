use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use serde::{Deserialize, Serialize};

use crate::AuthState;
use crate::ChannelType;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STREAMING_CHANNEL_TYPE: i32 = 6;
const STREAM_MEMBER_STATE_ACTIVE: i32 = 1;
const STREAM_FETCH_LIMIT: i32 = 100;
const STREAM_FETCH_ATTEMPTS: u32 = 3;
const STREAM_FETCH_RETRY_BASE: Duration = Duration::from_secs(1);
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROLS_HIDE_DELAY: Duration = Duration::from_secs(3);
const WATCH_RENEW_INTERVAL: Duration = Duration::from_secs(30);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const STALL_STEP_DOWN: Duration = Duration::from_secs(4);
const STALL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RECONNECTS: u32 = 3;
const STREAM_LIVE_OFFSET_SECS: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamQuality {
    High,
    Mid,
    Low,
    #[default]
    #[serde(other)]
    Auto,
}

pub const STREAM_QUALITIES: [StreamQuality; 4] = [
    StreamQuality::Auto,
    StreamQuality::High,
    StreamQuality::Mid,
    StreamQuality::Low,
];

impl StreamQuality {
    pub fn i18n_key(self) -> &'static str {
        match self {
            Self::Auto => "channelStream.qualityAuto",
            Self::High => "channelStream.qualityHigh",
            Self::Mid => "channelStream.qualityMid",
            Self::Low => "channelStream.qualityLow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsVariant {
    pub bandwidth: u64,
    pub width: u32,
    pub height: u32,
    pub url: String,
}

fn parse_stream_inf(attrs: &str) -> (u64, u32, u32) {
    let mut bandwidth = 0u64;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut start = 0;
    let mut in_quotes = false;
    let bytes = attrs.as_bytes();
    for i in 0..=bytes.len() {
        if i < bytes.len() && bytes[i] == b'"' {
            in_quotes = !in_quotes;
        }
        if i == bytes.len() || (bytes[i] == b',' && !in_quotes) {
            let part = attrs[start..i].trim();
            start = i + 1;
            let Some((key, val)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim().trim_matches('"');
            if key == "BANDWIDTH" {
                bandwidth = val.parse::<u64>().unwrap_or(0);
            } else if key == "RESOLUTION"
                && let Some((w, h)) = val.split_once('x').or_else(|| val.split_once('X'))
            {
                width = w.parse::<u32>().unwrap_or(0);
                height = h.parse::<u32>().unwrap_or(0);
            }
        }
    }
    (bandwidth, width, height)
}

pub fn parse_hls_variants(master_url: &str, text: &str) -> Vec<HlsVariant> {
    let text = text.trim_start();
    if !text.starts_with("#EXTM3U") {
        return Vec::new();
    }
    let Ok(base) = url::Url::parse(master_url) else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    let mut pending: Option<(u64, u32, u32)> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(parse_stream_inf(attrs));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some((bandwidth, width, height)) = pending.take()
            && let Ok(resolved) = base.join(line)
        {
            variants.push(HlsVariant {
                bandwidth,
                width,
                height,
                url: resolved.into(),
            });
        }
    }
    variants.sort_by_key(|v| v.bandwidth);
    variants
}

pub fn variant_index(
    quality: StreamQuality,
    count: usize,
    auto_cap: Option<usize>,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match quality {
        StreamQuality::Low => 0,
        StreamQuality::Mid => (count - 1) / 2,
        StreamQuality::High => count - 1,
        StreamQuality::Auto => (count - 1).min(auto_cap.unwrap_or(usize::MAX)),
    })
}

pub fn auto_step_down_target(
    quality: StreamQuality,
    failed: bool,
    stalled: Duration,
    current_index: Option<usize>,
) -> Option<usize> {
    if quality != StreamQuality::Auto || failed || stalled < STALL_STEP_DOWN {
        return None;
    }
    current_index.filter(|&idx| idx > 0).map(|idx| idx - 1)
}

pub fn coreaudio_device_uid(device_id: Option<&str>) -> Option<&str> {
    device_id
        .and_then(|id| id.strip_prefix("coreaudio:"))
        .filter(|uid| !uid.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMember {
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub display_name: String,
    pub avatar: String,
    pub is_publisher: bool,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamPhase {
    Idle,
    Joining {
        channel_id: ChannelId,
        clan_id: ClanId,
    },
    Joined {
        channel_id: ChannelId,
        clan_id: ClanId,
    },
    Error(String),
}

struct FetchGuard {
    clan_id: ClanId,
    touched: HashSet<(ChannelId, UserId)>,
    ended: HashSet<ChannelId>,
}

impl FetchGuard {
    fn new(clan_id: ClanId) -> Self {
        Self {
            clan_id,
            touched: HashSet::new(),
            ended: HashSet::new(),
        }
    }
}

pub struct StreamStore {
    api: Arc<AppApi>,
    phase: StreamPhase,
    members: HashMap<(ChannelId, UserId), StreamMember>,
    members_by_channel: HashMap<ChannelId, Arc<[StreamMember]>>,
    empty_members: Arc<[StreamMember]>,
    channel_clans: HashMap<ChannelId, ClanId>,
    members_revision: u64,
    fetch_guard: Option<FetchGuard>,
    pending_unwatch: HashSet<ChannelId>,
    reset_generation: u64,
    next_seq: u64,
    active_clan: Option<ClanId>,
    last_viewed_stream_channel: Option<ChannelId>,
    show_chat: bool,
    show_members: bool,
    controls_visible: bool,
    controls_hovered: bool,
    stage_hovered: bool,
    controls_hide_at: Option<Instant>,
    volume: f32,
    muted: bool,
    fullscreen: bool,
    error_message: Option<String>,
    variants: Vec<HlsVariant>,
    player: Option<Rc<mezon_video::VideoPlayer>>,
    playing_url: Option<String>,
    player_generation: u64,
    quality: StreamQuality,
    auto_cap: Option<usize>,
    reconnects: u32,
    reconnecting: bool,
    session_generation: u64,
    session_channel_label: String,
    session_clan_name: String,
    _fetch_task: Option<Task<()>>,
    _playback_task: Option<Task<()>>,
    _health_task: Option<Task<()>>,
    _watch_task: Option<Task<()>>,
    _join_timeout: Option<Task<()>>,
    _controls_hide_task: Option<Task<()>>,
    _conn_watch: Task<()>,
    _app_quit_subscription: Subscription,
}

struct GlobalStreamStore(Entity<StreamStore>);
impl Global for GlobalStreamStore {}

fn video_error_text(cx: &App) -> String {
    let locale = crate::Settings::try_global(cx)
        .map(|s| s.read(cx).language.clone())
        .unwrap_or_else(|| "en".to_string());
    mezon_i18n::t(&locale, "channelStream.videoError").to_string()
}

impl StreamStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalStreamStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStreamStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalStreamStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let app_quit_subscription = cx.on_app_quit(|this, _cx| {
            let channel_id = this.session_channel_id();
            let api = this.api.clone();
            async move {
                if let Some(channel_id) = channel_id
                    && let Err(err) = api.write_streaming_watch(channel_id.get(), false).await
                {
                    tracing::warn!(
                        "write_streaming_watch({}, false) on quit failed: {err:#}",
                        channel_id.get()
                    );
                }
            }
        });
        let quality = crate::Settings::try_global(cx)
            .map(|s| s.read(cx).stream_default_quality)
            .unwrap_or_default();
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            api,
            phase: StreamPhase::Idle,
            members: HashMap::new(),
            members_by_channel: HashMap::new(),
            empty_members: Arc::from([]),
            channel_clans: HashMap::new(),
            members_revision: 0,
            fetch_guard: None,
            pending_unwatch: HashSet::new(),
            reset_generation: 0,
            next_seq: 0,
            active_clan: None,
            last_viewed_stream_channel: None,
            show_chat: false,
            show_members: true,
            controls_visible: true,
            controls_hovered: false,
            stage_hovered: false,
            controls_hide_at: None,
            volume: 1.0,
            muted: false,
            fullscreen: false,
            error_message: None,
            variants: Vec::new(),
            player: None,
            playing_url: None,
            player_generation: 0,
            quality,
            auto_cap: None,
            reconnects: 0,
            reconnecting: false,
            session_generation: 0,
            session_channel_label: String::new(),
            session_clan_name: String::new(),
            _fetch_task: None,
            _playback_task: None,
            _health_task: None,
            _watch_task: None,
            _join_timeout: None,
            _controls_hide_task: None,
            _conn_watch: conn_watch,
            _app_quit_subscription: app_quit_subscription,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::StreamingJoined, &entity, |this, event, cx| {
                this.handle_streaming_joined(event, cx)
            });
            dispatch.on(RealtimeKind::StreamingLeaved, &entity, |this, event, cx| {
                this.handle_streaming_leaved(event, cx)
            });
            dispatch.on(RealtimeKind::StreamingEnded, &entity, |this, event, cx| {
                this.handle_streaming_ended(event, cx)
            });
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = *status_rx.borrow() == ConnectionStatus::Connected;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected
                    && !was_connected
                    && this.update(cx, |this, cx| this.on_reconnected(cx)).is_err()
                {
                    break;
                }
                was_connected = connected;
            }
        })
    }

    fn on_reconnected(&mut self, cx: &mut Context<Self>) {
        if let StreamPhase::Joined { channel_id, .. } = self.phase {
            self.send_streaming_watch(channel_id, true, cx);
        }
        let left: Vec<ChannelId> = self.pending_unwatch.iter().copied().collect();
        for channel_id in left {
            self.send_streaming_watch(channel_id, false, cx);
        }
        if let Some(clan_id) = self.active_clan {
            self.fetch_members(clan_id, cx);
        }
    }

    fn next_member_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    pub fn phase(&self) -> &StreamPhase {
        &self.phase
    }

    pub fn is_joined(&self) -> bool {
        matches!(self.phase, StreamPhase::Joined { .. })
    }

    pub fn is_joining(&self) -> bool {
        matches!(self.phase, StreamPhase::Joining { .. })
    }

    pub fn session_channel_id(&self) -> Option<ChannelId> {
        match &self.phase {
            StreamPhase::Joining { channel_id, .. } | StreamPhase::Joined { channel_id, .. } => {
                Some(*channel_id)
            }
            _ => None,
        }
    }

    pub fn is_session_channel(&self, channel_id: ChannelId) -> bool {
        self.session_channel_id() == Some(channel_id)
    }

    pub fn sync_chat_for_active_channel(
        &mut self,
        active: Option<(ChannelType, ChannelId)>,
        cx: &mut Context<Self>,
    ) {
        if !self.show_chat {
            return;
        }
        let Some((ChannelType::Stream, channel_id)) = active else {
            return;
        };
        if self.is_session_channel(channel_id) && (self.is_joined() || self.is_joining()) {
            return;
        }
        self.show_chat = false;
        cx.notify();
    }

    pub fn show_chat(&self) -> bool {
        self.show_chat
    }

    pub fn show_members(&self) -> bool {
        self.show_members
    }

    pub fn controls_visible(&self) -> bool {
        self.controls_visible || self.controls_hovered
    }

    pub fn set_controls_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.controls_hovered == hovered {
            return;
        }
        self.controls_hovered = hovered;
        if !hovered {
            self.bump_controls_visible(cx);
        }
        cx.notify();
    }

    pub fn set_stage_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.stage_hovered == hovered {
            return;
        }
        self.stage_hovered = hovered;
        cx.notify();
    }

    pub fn bump_controls_visible(&mut self, cx: &mut Context<Self>) {
        let was_visible = self.controls_visible;
        self.controls_visible = true;
        self.controls_hide_at = Some(Instant::now() + CONTROLS_HIDE_DELAY);
        if self._controls_hide_task.is_none() {
            self._controls_hide_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let wait = this
                        .update(cx, |this, _| {
                            this.controls_hide_at
                                .map(|at| at.saturating_duration_since(Instant::now()))
                        })
                        .ok()
                        .flatten();
                    let Some(wait) = wait else {
                        break;
                    };
                    if !wait.is_zero() {
                        cx.background_executor().timer(wait).await;
                    }
                    let hide = this
                        .update(cx, |this, _| {
                            this.controls_hide_at.is_none_or(|at| Instant::now() >= at)
                        })
                        .unwrap_or(true);
                    if hide {
                        this.update(cx, |this, cx| {
                            this.controls_visible = false;
                            this.controls_hide_at = None;
                            this._controls_hide_task = None;
                            if this.stage_hovered && !this.controls_hovered {
                                cx.hide_cursor_until_mouse_moves();
                            }
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                }
            }));
        }
        if !was_visible {
            cx.notify();
        }
    }

    pub fn clear_error_on_active_channel_change(
        &mut self,
        active: Option<(ChannelType, ChannelId)>,
        cx: &mut Context<Self>,
    ) {
        let viewed = match active {
            Some((ChannelType::Stream, channel_id)) => Some(channel_id),
            _ => None,
        };
        if viewed.is_none() || viewed != self.session_channel_id() {
            self.stage_hovered = false;
            self.controls_hovered = false;
        }
        if self.last_viewed_stream_channel == viewed {
            return;
        }
        self.last_viewed_stream_channel = viewed;
        if self.error_message.take().is_some() {
            if matches!(self.phase, StreamPhase::Error(_)) {
                self.phase = StreamPhase::Idle;
            }
            cx.notify();
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    pub fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn session_channel_label(&self) -> &str {
        &self.session_channel_label
    }

    pub fn session_clan_name(&self) -> &str {
        &self.session_clan_name
    }

    pub fn player(&self) -> Option<Rc<mezon_video::VideoPlayer>> {
        self.player.clone()
    }

    pub fn player_generation(&self) -> u64 {
        self.player_generation
    }

    pub fn quality(&self) -> StreamQuality {
        self.quality
    }

    pub fn set_quality(&mut self, quality: StreamQuality, cx: &mut Context<Self>) {
        if self.quality == quality && self.auto_cap.is_none() {
            return;
        }
        self.quality = quality;
        self.auto_cap = None;
        if self.is_joined()
            && let Some(index) = variant_index(self.quality, self.variants.len(), self.auto_cap)
            && self.playing_url.as_deref() != Some(self.variants[index].url.as_str())
        {
            if self.open_current_variant(cx) {
                self.cancel_reconnect();
            } else if let StreamPhase::Joined {
                channel_id,
                clan_id,
            } = self.phase
            {
                self.reconnect_playback(self.session_generation, clan_id, channel_id, cx);
            }
            return;
        }
        cx.notify();
    }

    fn cancel_reconnect(&mut self) {
        if self.reconnecting {
            self._playback_task = None;
            self.reconnecting = false;
        }
    }

    pub fn set_output_device(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if let Some(player) = &self.player {
            player.set_audio_output_device(coreaudio_device_uid(id.as_deref()));
        }
        cx.notify();
    }

    fn rebuild_channel_members(&mut self, channel_id: ChannelId) {
        self.members_revision = self.members_revision.wrapping_add(1);
        let mut list: Vec<StreamMember> = self
            .members
            .values()
            .filter(|m| m.channel_id == channel_id)
            .cloned()
            .collect();
        if list.is_empty() {
            self.members_by_channel.remove(&channel_id);
        } else {
            list.sort_by_key(|m| (!m.is_publisher, m.seq));
            self.members_by_channel.insert(channel_id, Arc::from(list));
        }
    }

    pub fn members_for_channel(&self, channel_id: ChannelId) -> Arc<[StreamMember]> {
        self.members_by_channel
            .get(&channel_id)
            .cloned()
            .unwrap_or_else(|| self.empty_members.clone())
    }

    pub fn members_revision(&self) -> u64 {
        self.members_revision
    }

    pub fn stream_publisher(&self, channel_id: ChannelId) -> Option<&StreamMember> {
        self.members_by_channel
            .get(&channel_id)?
            .first()
            .filter(|m| m.is_publisher)
    }

    pub fn viewer_count(&self, channel_id: ChannelId) -> u32 {
        self.members_by_channel
            .get(&channel_id)
            .map_or(0, |members| {
                let publishers = usize::from(members.first().is_some_and(|m| m.is_publisher));
                (members.len() - publishers) as u32
            })
    }

    fn clear_channel_members(&mut self, channel_id: ChannelId) {
        self.members.retain(|(cid, _), _| *cid != channel_id);
        if self.members_by_channel.remove(&channel_id).is_some() {
            self.members_revision = self.members_revision.wrapping_add(1);
        }
    }

    fn note_member_event(&mut self, clan_id: ClanId, channel_id: ChannelId, user: Option<UserId>) {
        let Some(guard) = self.fetch_guard.as_mut().filter(|g| g.clan_id == clan_id) else {
            return;
        };
        match user {
            Some(user_id) => {
                guard.touched.insert((channel_id, user_id));
            }
            None => {
                guard.ended.insert(channel_id);
            }
        }
    }

    pub fn set_active_clan(&mut self, clan_id: Option<ClanId>, cx: &mut Context<Self>) {
        if self.active_clan == clan_id {
            return;
        }
        self.active_clan = clan_id;
        if let Some(clan_id) = clan_id {
            self.fetch_members(clan_id, cx);
        }
    }

    pub fn fetch_members(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            for attempt in 0..STREAM_FETCH_ATTEMPTS {
                if attempt > 0 {
                    cx.background_executor()
                        .timer(STREAM_FETCH_RETRY_BASE * (1 << (attempt - 1)))
                        .await;
                }
                if this
                    .update(cx, |this, _| {
                        this.fetch_guard = Some(FetchGuard::new(clan_id));
                    })
                    .is_err()
                {
                    return;
                }
                let result = api
                    .list_streaming_channel_users(
                        clan_id.get(),
                        0,
                        STREAMING_CHANNEL_TYPE,
                        STREAM_MEMBER_STATE_ACTIVE,
                        STREAM_FETCH_LIMIT,
                    )
                    .await;
                let Ok(done) = this.update(cx, |this, cx| {
                    let guard = this
                        .fetch_guard
                        .take()
                        .filter(|g| g.clan_id == clan_id)
                        .unwrap_or_else(|| FetchGuard::new(clan_id));
                    match result {
                        Ok(list) => {
                            this.apply_fetched_members(&guard, list.streaming_channel_users);
                            if let Some(store) = crate::ClanMembersStore::try_global(cx) {
                                store.update(cx, |store, cx| store.ensure_loaded(clan_id, cx));
                            }
                            if let Some(store) = crate::UsersByUserStore::try_global(cx) {
                                store.update(cx, |store, cx| store.ensure_loaded(cx));
                            }
                            cx.notify();
                            true
                        }
                        Err(err) => {
                            tracing::warn!("list_streaming_channel_users failed: {err:#}");
                            false
                        }
                    }
                }) else {
                    return;
                };
                if done {
                    return;
                }
            }
        }));
    }

    fn apply_fetched_members(
        &mut self,
        guard: &FetchGuard,
        users: Vec<mezon_proto::api::StreamingChannelUser>,
    ) {
        let clan_id = guard.clan_id;
        let mut changed_channels = HashSet::new();
        let live_keys: HashSet<(ChannelId, UserId)> = users
            .iter()
            .map(|u| (ChannelId(u.channel_id), UserId(u.user_id)))
            .collect();
        let stale_keys: Vec<(ChannelId, UserId)> = self
            .members
            .keys()
            .copied()
            .filter(|key| {
                self.channel_clans.get(&key.0) == Some(&clan_id)
                    && !live_keys.contains(key)
                    && !guard.touched.contains(key)
            })
            .collect();
        for (ch_id, uid) in stale_keys {
            self.members.remove(&(ch_id, uid));
            changed_channels.insert(ch_id);
        }
        for user in users {
            let user_id = UserId(user.user_id);
            let channel_id = ChannelId(user.channel_id);
            if guard.touched.contains(&(channel_id, user_id)) || guard.ended.contains(&channel_id) {
                continue;
            }
            self.channel_clans.insert(channel_id, clan_id);
            let existing = self.members.get(&(channel_id, user_id));
            let display_name = if !user.participant.is_empty() {
                user.participant
            } else if let Some(known) = existing
                && !known.display_name.is_empty()
            {
                known.display_name.clone()
            } else {
                user_id.to_string()
            };
            let avatar = existing.map(|m| m.avatar.clone()).unwrap_or_default();
            let seq = existing
                .map(|m| m.seq)
                .unwrap_or_else(|| self.next_member_seq());
            let member = StreamMember {
                user_id,
                channel_id,
                display_name,
                avatar,
                is_publisher: user.is_publisher,
                seq,
            };
            if self.members.get(&(channel_id, user_id)) != Some(&member) {
                changed_channels.insert(channel_id);
            }
            self.members.insert((channel_id, user_id), member);
        }
        for ch_id in changed_channels {
            self.rebuild_channel_members(ch_id);
        }
    }

    pub fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        self.show_chat = !self.show_chat;
        cx.notify();
    }

    pub fn toggle_members(&mut self, cx: &mut Context<Self>) {
        self.show_members = !self.show_members;
        cx.notify();
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0.0, 1.0);
        self.muted = self.volume <= 0.0;
        self.sync_audio_playback();
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.sync_audio_playback();
        cx.notify();
    }

    pub fn toggle_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.fullscreen = !self.fullscreen;
        self.controls_hovered = false;
        self.stage_hovered = false;
        self.bump_controls_visible(cx);
        cx.notify();
    }

    pub fn clear_fullscreen(&mut self, cx: &mut Context<Self>) {
        if self.fullscreen {
            self.fullscreen = false;
            self.controls_hovered = false;
            self.stage_hovered = false;
            self.bump_controls_visible(cx);
            cx.notify();
        }
    }

    fn sync_audio_playback(&self) {
        if let Some(player) = &self.player {
            player.set_volume(self.effective_volume());
            player.set_muted(self.muted);
        }
    }

    fn send_streaming_watch(
        &mut self,
        channel_id: ChannelId,
        watching: bool,
        cx: &mut Context<Self>,
    ) {
        if watching {
            self.pending_unwatch.remove(&channel_id);
        }
        let api = self.api.clone();
        let reset_generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.write_streaming_watch(channel_id.get(), watching).await;
            if let Err(err) = &result {
                tracing::warn!(
                    "write_streaming_watch({}, {watching}) failed: {err:#}",
                    channel_id.get()
                );
            }
            if watching {
                return;
            }
            this.update(cx, |this, _| {
                if this.reset_generation != reset_generation {
                    return;
                }
                if result.is_ok() {
                    this.pending_unwatch.remove(&channel_id);
                } else if !this.is_session_channel(channel_id) {
                    this.pending_unwatch.insert(channel_id);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn join_stream(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        channel_label: &str,
        clan_name: &str,
        auth: &AuthState,
        cx: &mut Context<Self>,
    ) {
        let AuthState::Authenticated(_) = auth else {
            self.fail_stream("Not authenticated".into(), cx);
            return;
        };

        if let Some(old_channel_id) = self.session_channel_id().filter(|&old| old != channel_id) {
            self.send_streaming_watch(old_channel_id, false, cx);
        }

        self.disconnect_session();
        self.fullscreen = false;
        self.controls_hovered = false;
        self.stage_hovered = false;
        self.auto_cap = None;
        self.quality = crate::Settings::try_global(cx)
            .map(|s| s.read(cx).stream_default_quality)
            .unwrap_or_default();

        let session_generation = self.session_generation;
        self.session_channel_label = channel_label.to_string();
        self.session_clan_name = clan_name.to_string();
        self.phase = StreamPhase::Joining {
            channel_id,
            clan_id,
        };
        self.error_message = None;
        cx.notify();

        let api = self.api.clone();
        self._playback_task = Some(cx.spawn(async move |this, cx| {
            let playback = api.get_stream_playback(channel_id.get(), false).await;
            let hls_url = this
                .update(cx, |this, cx| {
                    if this.session_generation != session_generation
                        || !this.is_session_channel(channel_id)
                        || !this.is_joining()
                    {
                        return None;
                    }
                    match playback {
                        Ok(pb) if !pb.token.is_empty() && !pb.hls_url.is_empty() => {
                            Some(pb.hls_url)
                        }
                        _ => {
                            let msg = video_error_text(cx);
                            this.fail_stream(msg, cx);
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(hls_url) = hls_url else {
                return;
            };
            let playlist_text = api.fetch_stream_playlist(&hls_url).await;
            let variants = playlist_text
                .as_deref()
                .map(|text| parse_hls_variants(&hls_url, text))
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.session_generation != session_generation
                    || !this.is_session_channel(channel_id)
                    || !this.is_joining()
                {
                    return;
                }
                if variants.is_empty() {
                    this.reconnect_playback(session_generation, clan_id, channel_id, cx);
                    return;
                }
                this.variants = variants;
                if this.open_current_variant(cx) {
                    this.on_playback_opened(session_generation, clan_id, channel_id, cx);
                } else {
                    this.reconnect_playback(session_generation, clan_id, channel_id, cx);
                }
            });
        }));

        self._join_timeout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(JOIN_TIMEOUT).await;
            this.update(cx, |this, cx| {
                if this.session_generation == session_generation && this.is_joining() {
                    this.fail_stream("Join timed out".into(), cx);
                }
            })
            .ok();
        }));
    }

    fn open_current_variant(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(index) = variant_index(self.quality, self.variants.len(), self.auto_cap) else {
            return false;
        };
        let url = self.variants[index].url.clone();
        let had_player = self.player.take().is_some();
        self.playing_url = None;
        let output_device_id =
            crate::Settings::try_global(cx).and_then(|s| s.read(cx).output_device_id.clone());
        match mezon_video::VideoPlayer::open(&url, None) {
            Ok(player) => {
                player.set_live_offset(STREAM_LIVE_OFFSET_SECS);
                player.set_audio_output_device(coreaudio_device_uid(output_device_id.as_deref()));
                player.set_volume(self.effective_volume());
                player.set_muted(self.muted);
                player.play();
                self.player = Some(Rc::new(player));
                self.playing_url = Some(url);
                self.player_generation = self.player_generation.wrapping_add(1);
                cx.notify();
                true
            }
            Err(err) => {
                if had_player {
                    self.player_generation = self.player_generation.wrapping_add(1);
                    cx.notify();
                }
                tracing::warn!("VideoPlayer::open failed for stream: {err}");
                false
            }
        }
    }

    fn on_playback_opened(
        &mut self,
        session_generation: u64,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let was_joining = self.is_joining();
        self._join_timeout = None;
        self.phase = StreamPhase::Joined {
            channel_id,
            clan_id,
        };
        if was_joining {
            self.bump_controls_visible(cx);
        }
        self.send_streaming_watch(channel_id, true, cx);
        self.start_watch_task(session_generation, channel_id, cx);
        self.start_health_task(session_generation, clan_id, channel_id, cx);
        cx.notify();
    }

    fn start_watch_task(
        &mut self,
        session_generation: u64,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if self._watch_task.is_some() {
            return;
        }
        let api = self.api.clone();
        self._watch_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_RENEW_INTERVAL).await;
                let still_joined = this
                    .update(cx, |this, _| {
                        this.session_generation == session_generation
                            && this.is_session_channel(channel_id)
                            && this.is_joined()
                    })
                    .unwrap_or(false);
                if !still_joined {
                    break;
                }
                if let Err(err) = api.write_streaming_watch(channel_id.get(), true).await {
                    tracing::warn!(
                        "write_streaming_watch({}, true) renew failed: {err:#}",
                        channel_id.get()
                    );
                }
            }
        }));
    }

    fn start_health_task(
        &mut self,
        session_generation: u64,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if self._health_task.is_some() {
            return;
        }
        self._health_task = Some(cx.spawn(async move |this, cx| {
            let mut tracked_player_gen: Option<u64> = None;
            let mut last_time: Option<f64> = None;
            let mut last_advance_at = Instant::now();
            let mut started = false;
            loop {
                cx.background_executor().timer(HEALTH_POLL_INTERVAL).await;
                let cont = this
                    .update(cx, |this, cx| {
                        if this.session_generation != session_generation
                            || !this.is_session_channel(channel_id)
                            || !this.is_joined()
                        {
                            return false;
                        }
                        if this.reconnecting {
                            return true;
                        }
                        let Some(player) = this.player.clone() else {
                            this.reconnect_playback(session_generation, clan_id, channel_id, cx);
                            return true;
                        };
                        if tracked_player_gen != Some(this.player_generation) {
                            tracked_player_gen = Some(this.player_generation);
                            last_time = None;
                            last_advance_at = Instant::now();
                            started = false;
                        }
                        let failed = player.failed();
                        let current_time = player.current_time();
                        if let Some(prev) = last_time {
                            if current_time > prev {
                                last_time = Some(current_time);
                                last_advance_at = Instant::now();
                                started = true;
                                this.reconnects = 0;
                            }
                        } else {
                            last_time = Some(current_time);
                            last_advance_at = Instant::now();
                        }
                        let current_index =
                            variant_index(this.quality, this.variants.len(), this.auto_cap);
                        let stalled = if started {
                            last_advance_at.elapsed()
                        } else {
                            Duration::ZERO
                        };
                        if let Some(new_cap) =
                            auto_step_down_target(this.quality, failed, stalled, current_index)
                        {
                            this.auto_cap = Some(new_cap);
                            last_time = None;
                            last_advance_at = Instant::now();
                            if !this.open_current_variant(cx) {
                                this.reconnect_playback(
                                    session_generation,
                                    clan_id,
                                    channel_id,
                                    cx,
                                );
                            }
                            return true;
                        }
                        if failed || last_advance_at.elapsed() >= STALL_TIMEOUT {
                            last_time = None;
                            last_advance_at = Instant::now();
                            this.reconnect_playback(session_generation, clan_id, channel_id, cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
            }
        }));
    }

    fn reconnect_playback(
        &mut self,
        session_generation: u64,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if self.reconnecting {
            return;
        }
        self.reconnecting = true;
        let api = self.api.clone();
        self._playback_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let delay = this
                    .update(cx, |this, cx| {
                        if this.session_generation != session_generation
                            || !this.is_session_channel(channel_id)
                            || (!this.is_joined() && !this.is_joining())
                        {
                            return None;
                        }
                        if this.reconnects >= MAX_RECONNECTS {
                            this.reconnecting = false;
                            this.send_streaming_watch(channel_id, false, cx);
                            let msg = video_error_text(cx);
                            this.fail_stream(msg, cx);
                            return None;
                        }
                        let delay = mezon_voice::sfu_reconnect_delay(this.reconnects);
                        this.reconnects += 1;
                        Some(delay)
                    })
                    .ok()
                    .flatten();
                let Some(delay) = delay else {
                    return;
                };
                cx.background_executor().timer(delay).await;
                let still_valid = this
                    .update(cx, |this, _| {
                        this.session_generation == session_generation
                            && this.is_session_channel(channel_id)
                            && (this.is_joined() || this.is_joining())
                    })
                    .unwrap_or(false);
                if !still_valid {
                    return;
                }
                let Ok(playback) = api.get_stream_playback(channel_id.get(), false).await else {
                    continue;
                };
                if playback.token.is_empty() || playback.hls_url.is_empty() {
                    continue;
                }
                let Some(playlist_text) = api.fetch_stream_playlist(&playback.hls_url).await else {
                    continue;
                };
                let variants = parse_hls_variants(&playback.hls_url, &playlist_text);
                if variants.is_empty() {
                    continue;
                }
                let done = this
                    .update(cx, |this, cx| {
                        if this.session_generation != session_generation
                            || !this.is_session_channel(channel_id)
                            || (!this.is_joined() && !this.is_joining())
                        {
                            return true;
                        }
                        this.variants = variants;
                        if this.open_current_variant(cx) {
                            this.reconnecting = false;
                            this.on_playback_opened(session_generation, clan_id, channel_id, cx);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
        }));
    }

    pub fn leave_stream(&mut self, cx: &mut Context<Self>) {
        if let Some(channel_id) = self.session_channel_id() {
            self.send_streaming_watch(channel_id, false, cx);
        }
        self.leave_stream_locally(cx);
    }

    fn leave_stream_locally(&mut self, cx: &mut Context<Self>) {
        self.disconnect_session();
        self.phase = StreamPhase::Idle;
        self.error_message = None;
        self.fullscreen = false;
        self.controls_visible = false;
        self.controls_hovered = false;
        self.stage_hovered = false;
        self.controls_hide_at = None;
        self._controls_hide_task = None;
        self.show_chat = false;
        self.session_channel_label.clear();
        self.session_clan_name.clear();
        cx.notify();
    }

    fn disconnect_session(&mut self) {
        self.session_generation = self.session_generation.wrapping_add(1);
        self._playback_task = None;
        self._health_task = None;
        self._watch_task = None;
        self._join_timeout = None;
        if self.player.take().is_some() {
            self.player_generation = self.player_generation.wrapping_add(1);
        }
        self.playing_url = None;
        self.variants.clear();
        self.auto_cap = None;
        self.reconnects = 0;
        self.reconnecting = false;
    }

    fn fail_stream(&mut self, message: String, cx: &mut Context<Self>) {
        self.disconnect_session();
        self.fullscreen = false;
        self.controls_hovered = false;
        self.stage_hovered = false;
        self.phase = StreamPhase::Error(message.clone());
        self.error_message = Some(message);
        cx.notify();
    }

    fn handle_streaming_joined(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingJoined(e) = event else {
            return;
        };
        let clan_id = ClanId(e.clan_id);
        let channel_id = ChannelId(e.streaming_channel_id);
        let user_id = UserId(e.user_id);
        self.channel_clans.insert(channel_id, clan_id);
        self.note_member_event(clan_id, channel_id, Some(user_id));

        if let Some(existing) = self.members.get(&(channel_id, user_id))
            && existing.is_publisher
            && !e.is_publisher
        {
            cx.notify();
            return;
        }
        let seq = self
            .members
            .get(&(channel_id, user_id))
            .map(|m| m.seq)
            .unwrap_or_else(|| self.next_member_seq());
        let display_name = if e.participant.is_empty() {
            user_id.to_string()
        } else {
            e.participant.clone()
        };
        self.members.insert(
            (channel_id, user_id),
            StreamMember {
                user_id,
                channel_id,
                display_name,
                avatar: e.streaming_channel_label.clone(),
                is_publisher: e.is_publisher,
                seq,
            },
        );
        self.rebuild_channel_members(channel_id);
        cx.notify();
    }

    fn handle_streaming_leaved(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingLeaved(e) = event else {
            return;
        };
        let Ok(channel_id) = e.streaming_channel_id.parse::<i64>() else {
            return;
        };
        let channel_id = ChannelId(channel_id);
        let Ok(user_id) = e.streaming_user_id.parse::<i64>() else {
            return;
        };
        let user_id = UserId(user_id);
        let clan_id = ClanId(e.clan_id);

        if self
            .stream_publisher(channel_id)
            .is_some_and(|pub_member| pub_member.user_id == user_id)
        {
            self.note_member_event(clan_id, channel_id, None);
            self.clear_channel_members(channel_id);
        } else {
            self.note_member_event(clan_id, channel_id, Some(user_id));
            if self.members.remove(&(channel_id, user_id)).is_some() {
                self.rebuild_channel_members(channel_id);
            }
        }

        if self.is_session_channel(channel_id)
            && (self.is_joined() || self.is_joining())
            && !self.members_by_channel.contains_key(&channel_id)
        {
            self.leave_stream_locally(cx);
            return;
        }
        cx.notify();
    }

    fn handle_streaming_ended(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::StreamingEnded(e) = event else {
            return;
        };
        let channel_id = ChannelId(e.channel_id);
        self.note_member_event(ClanId(e.clan_id), channel_id, None);
        self.clear_channel_members(channel_id);
        cx.notify();
    }

    pub fn on_logout(&mut self, cx: &mut Context<Self>) {
        self._fetch_task = None;
        self.fetch_guard = None;
        self.leave_stream(cx);
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.pending_unwatch.clear();
        self.members.clear();
        self.members_by_channel.clear();
        self.members_revision = self.members_revision.wrapping_add(1);
        self.channel_clans.clear();
        self.next_seq = 0;
        self.active_clan = None;
        self.last_viewed_stream_channel = None;
        self.show_chat = false;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_store(cx: &mut gpui::TestAppContext) -> Entity<StreamStore> {
        cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            cx.new(|cx| StreamStore::new(api, cx))
        })
    }

    fn joined_event(
        clan_id: ClanId,
        channel_id: ChannelId,
        user_id: UserId,
        name: &str,
        avatar: &str,
        is_publisher: bool,
    ) -> RealtimeEvent {
        RealtimeEvent::StreamingJoined(mezon_proto::realtime::StreamingJoinedEvent {
            clan_id: clan_id.get(),
            streaming_channel_id: channel_id.get(),
            user_id: user_id.get(),
            participant: name.into(),
            streaming_channel_label: avatar.into(),
            is_publisher,
            ..Default::default()
        })
    }

    fn leaved_event(clan_id: ClanId, channel_id: ChannelId, user_id: UserId) -> RealtimeEvent {
        RealtimeEvent::StreamingLeaved(mezon_proto::realtime::StreamingLeavedEvent {
            clan_id: clan_id.get(),
            streaming_channel_id: channel_id.to_string(),
            streaming_user_id: user_id.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn parse_hls_variants_resolves_origin_and_cdn_masters_and_sorts_by_bandwidth() {
        let origin_master = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:9\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=2995075,CODECS=\"avc1.42c01f,opus\",RESOLUTION=1280x720\n",
            "stream_0/playlist.m3u8?token=jwt123\n"
        );
        let origin_variants = parse_hls_variants(
            "https://stream.example.com/0123456789abcdef/live/index.m3u8?token=jwt123",
            origin_master,
        );
        assert_eq!(
            origin_variants,
            vec![HlsVariant {
                bandwidth: 2_995_075,
                width: 1280,
                height: 720,
                url: "https://stream.example.com/0123456789abcdef/live/stream_0/playlist.m3u8?token=jwt123".into(),
            }]
        );

        let cdn_master = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:9\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=2500000,CODECS=\"avc1.42c01f,opus\",RESOLUTION=1280x720\n",
            "stream_2/playlist.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=500000,CODECS=\"avc1.42c01f,opus\",RESOLUTION=640x360\n",
            "stream_0/playlist.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1200000,CODECS=\"avc1.42c01f,opus\",RESOLUTION=854x480\n",
            "stream_1/playlist.m3u8\n"
        );
        let cdn_variants = parse_hls_variants(
            "https://cdn-dev.nccsoft.vn/live/1840000000000000000/index.m3u8",
            cdn_master,
        );
        assert_eq!(
            cdn_variants,
            vec![
                HlsVariant {
                    bandwidth: 500_000,
                    width: 640,
                    height: 360,
                    url:
                        "https://cdn-dev.nccsoft.vn/live/1840000000000000000/stream_0/playlist.m3u8"
                            .into(),
                },
                HlsVariant {
                    bandwidth: 1_200_000,
                    width: 854,
                    height: 480,
                    url:
                        "https://cdn-dev.nccsoft.vn/live/1840000000000000000/stream_1/playlist.m3u8"
                            .into(),
                },
                HlsVariant {
                    bandwidth: 2_500_000,
                    width: 1280,
                    height: 720,
                    url:
                        "https://cdn-dev.nccsoft.vn/live/1840000000000000000/stream_2/playlist.m3u8"
                            .into(),
                },
            ]
        );

        assert!(
            parse_hls_variants(
                "https://cdn-dev.nccsoft.vn/live/1/index.m3u8",
                "not a playlist"
            )
            .is_empty()
        );
    }

    #[test]
    fn variant_index_maps_all_qualities_and_counts() {
        for q in STREAM_QUALITIES {
            assert_eq!(variant_index(q, 0, None), None);
            assert_eq!(variant_index(q, 1, None), Some(0));
        }
        assert_eq!(variant_index(StreamQuality::Low, 2, None), Some(0));
        assert_eq!(variant_index(StreamQuality::Mid, 2, None), Some(0));
        assert_eq!(variant_index(StreamQuality::High, 2, None), Some(1));
        assert_eq!(variant_index(StreamQuality::Auto, 2, None), Some(1));

        assert_eq!(variant_index(StreamQuality::Low, 3, None), Some(0));
        assert_eq!(variant_index(StreamQuality::Mid, 3, None), Some(1));
        assert_eq!(variant_index(StreamQuality::High, 3, None), Some(2));
        assert_eq!(variant_index(StreamQuality::Auto, 3, None), Some(2));

        assert_eq!(variant_index(StreamQuality::Auto, 3, Some(1)), Some(1));
        assert_eq!(variant_index(StreamQuality::Auto, 3, Some(0)), Some(0));
        assert_eq!(variant_index(StreamQuality::High, 3, Some(0)), Some(2));
        assert_eq!(variant_index(StreamQuality::Mid, 3, Some(0)), Some(1));
    }

    #[test]
    fn auto_step_down_target_steps_down_only_for_stalled_auto_above_lowest_variant() {
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, false, Duration::from_secs(4), Some(2)),
            Some(1)
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, false, Duration::from_secs(6), Some(1)),
            Some(0)
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, false, Duration::from_secs(6), Some(0)),
            None
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, false, Duration::from_secs(3), Some(2)),
            None
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, true, Duration::from_secs(5), Some(2)),
            None
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::High, false, Duration::from_secs(5), Some(2)),
            None
        );
        assert_eq!(
            auto_step_down_target(StreamQuality::Auto, false, Duration::from_secs(5), None),
            None
        );
    }

    #[test]
    fn coreaudio_device_uid_extracts_non_empty_coreaudio_uid() {
        assert_eq!(
            coreaudio_device_uid(Some("coreaudio:BuiltInSpeakerDevice")),
            Some("BuiltInSpeakerDevice")
        );
        assert_eq!(coreaudio_device_uid(Some("coreaudio:")), None);
        assert_eq!(coreaudio_device_uid(Some("wasapi:123")), None);
        assert_eq!(coreaudio_device_uid(Some("default")), None);
        assert_eq!(coreaudio_device_uid(None), None);
    }

    #[gpui::test]
    fn viewing_another_stream_channel_keeps_the_session(cx: &mut gpui::TestAppContext) {
        let joined = ChannelId(1);
        let other = ChannelId(2);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id: joined,
                    clan_id: ClanId(9),
                };
                store.show_chat = true;
                store.sync_chat_for_active_channel(Some((ChannelType::Stream, other)), cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert!(store.is_joined());
            assert_eq!(store.session_channel_id(), Some(joined));
            assert!(store.is_session_channel(joined));
            assert!(!store.is_session_channel(other));
            assert!(!store.show_chat());
        });
    }

    #[gpui::test]
    fn viewing_the_session_channel_keeps_chat_open(cx: &mut gpui::TestAppContext) {
        let joined = ChannelId(1);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id: joined,
                    clan_id: ClanId(9),
                };
                store.show_chat = true;
                store.sync_chat_for_active_channel(Some((ChannelType::Stream, joined)), cx);
            });
        });

        assert!(cx.update(|cx| store.read(cx).show_chat()));
    }

    #[gpui::test]
    fn publisher_is_not_demoted_by_non_publisher_join_and_members_sort_publisher_first(
        cx: &mut gpui::TestAppContext,
    ) {
        let clan_id = ClanId(9);
        let ch = ChannelId(1);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.handle_streaming_joined(
                    &joined_event(
                        clan_id,
                        ch,
                        UserId(20),
                        "Viewer 1",
                        "https://img/v1.png",
                        false,
                    ),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(
                        clan_id,
                        ch,
                        UserId(10),
                        "Streamer",
                        "https://img/pub.png",
                        true,
                    ),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(
                        clan_id,
                        ch,
                        UserId(30),
                        "Viewer 2",
                        "https://img/v2.png",
                        false,
                    ),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(
                        clan_id,
                        ch,
                        UserId(10),
                        "Stale",
                        "https://img/stale.png",
                        false,
                    ),
                    cx,
                );
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            let members = store.members_for_channel(ch);
            let ids: Vec<UserId> = members.iter().map(|m| m.user_id).collect();
            assert_eq!(ids, vec![UserId(10), UserId(20), UserId(30)]);
            assert!(members[0].is_publisher);
            assert_eq!(members[0].display_name, "Streamer");
            assert_eq!(members[0].avatar, "https://img/pub.png");
            assert_eq!(
                store.stream_publisher(ch).map(|m| m.user_id),
                Some(UserId(10))
            );
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.handle_streaming_leaved(&leaved_event(clan_id, ch, UserId(20)), cx);
                store.handle_streaming_joined(
                    &joined_event(
                        clan_id,
                        ch,
                        UserId(40),
                        "Viewer 3",
                        "https://img/v3.png",
                        false,
                    ),
                    cx,
                );
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            let ids: Vec<UserId> = store
                .members_for_channel(ch)
                .iter()
                .map(|m| m.user_id)
                .collect();
            assert_eq!(ids, vec![UserId(10), UserId(30), UserId(40)]);
        });
    }

    #[gpui::test]
    fn viewer_leave_removes_only_that_viewer_while_publisher_leave_clears_channel_and_ends_session(
        cx: &mut gpui::TestAppContext,
    ) {
        let clan_id = ClanId(9);
        let ch = ChannelId(1);
        let other_ch = ChannelId(2);
        let store = init_store(cx);

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.phase = StreamPhase::Joined {
                    channel_id: ch,
                    clan_id,
                };
                store.show_chat = true;
                store.handle_streaming_joined(
                    &joined_event(clan_id, ch, UserId(10), "Pub", "https://a", true),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(clan_id, ch, UserId(20), "V1", "https://b", false),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(clan_id, ch, UserId(30), "V2", "https://c", false),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(clan_id, other_ch, UserId(40), "OtherPub", "https://d", true),
                    cx,
                );
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert_eq!(store.viewer_count(ch), 2);
            assert_eq!(store.viewer_count(other_ch), 0);
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.handle_streaming_leaved(&leaved_event(clan_id, ch, UserId(20)), cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert!(store.is_joined());
            assert_eq!(
                store
                    .members_for_channel(ch)
                    .iter()
                    .map(|m| m.user_id)
                    .collect::<Vec<_>>(),
                vec![UserId(10), UserId(30)]
            );
            assert_eq!(store.viewer_count(ch), 1);
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.handle_streaming_leaved(&leaved_event(clan_id, ch, UserId(10)), cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert_eq!(store.phase(), &StreamPhase::Idle);
            assert!(!store.show_chat());
            assert!(store.members_for_channel(ch).is_empty());
            assert_eq!(store.viewer_count(ch), 0);
            assert_eq!(store.members_for_channel(other_ch).len(), 1);
        });
    }

    fn fetched_user(
        user_id: i64,
        channel_id: ChannelId,
        name: &str,
        is_publisher: bool,
    ) -> mezon_proto::api::StreamingChannelUser {
        mezon_proto::api::StreamingChannelUser {
            id: user_id,
            user_id,
            channel_id: channel_id.get(),
            participant: name.into(),
            is_publisher,
        }
    }

    #[gpui::test]
    fn fetched_members_replace_the_clan_but_events_during_the_fetch_win(
        cx: &mut gpui::TestAppContext,
    ) {
        let clan_a = ClanId(1);
        let clan_b = ClanId(2);
        let ch_a1 = ChannelId(10);
        let ch_a2 = ChannelId(11);
        let ch_b1 = ChannelId(20);
        let store = init_store(cx);

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                for (clan, channel, user, name) in [
                    (clan_a, ch_a1, 100, "Pub A1"),
                    (clan_a, ch_a2, 101, "Pub A2"),
                    (clan_b, ch_b1, 200, "Pub B1"),
                ] {
                    store.handle_streaming_joined(
                        &joined_event(clan, channel, UserId(user), name, "https://avatar/x", true),
                        cx,
                    );
                }
                store.apply_fetched_members(
                    &FetchGuard::new(clan_a),
                    vec![
                        fetched_user(100, ch_a1, "Pub A1 Updated", true),
                        fetched_user(105, ch_a1, "Viewer A1", false),
                    ],
                );
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            let a1 = store.members_for_channel(ch_a1);
            assert_eq!(a1.len(), 2);
            assert_eq!(a1[0].display_name, "Pub A1 Updated");
            assert_eq!(a1[0].avatar, "https://avatar/x");
            assert_eq!(a1[1].user_id, UserId(105));
            assert!(store.members_for_channel(ch_a2).is_empty());
            assert_eq!(store.viewer_count(ch_a1), 1);
            assert_eq!(store.members_for_channel(ch_b1).len(), 1);
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.fetch_guard = Some(FetchGuard::new(clan_a));
                store.handle_streaming_joined(
                    &joined_event(clan_a, ch_a2, UserId(102), "Joined Mid-Fetch", "", true),
                    cx,
                );
                store.handle_streaming_leaved(&leaved_event(clan_a, ch_a1, UserId(105)), cx);
                let guard = store.fetch_guard.take().unwrap();
                store.apply_fetched_members(
                    &guard,
                    vec![
                        fetched_user(100, ch_a1, "Pub A1 Updated", true),
                        fetched_user(105, ch_a1, "Viewer A1", false),
                        fetched_user(106, ch_a1, "Viewer New", false),
                    ],
                );
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            let a1 = store.members_for_channel(ch_a1);
            assert_eq!(
                a1.iter().map(|m| m.user_id).collect::<Vec<_>>(),
                vec![UserId(100), UserId(106)]
            );
            let a2 = store.members_for_channel(ch_a2);
            assert_eq!(a2.len(), 1);
            assert_eq!(a2[0].user_id, UserId(102));
            assert_eq!(store.members_for_channel(ch_b1).len(), 1);
        });
    }

    #[gpui::test]
    fn a_roster_without_its_publisher_has_no_publisher_and_counts_every_viewer(
        cx: &mut gpui::TestAppContext,
    ) {
        let clan = ClanId(1);
        let channel = ChannelId(10);
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.handle_streaming_joined(
                    &joined_event(clan, channel, UserId(5), "Viewer 5", "", false),
                    cx,
                );
                store.handle_streaming_joined(
                    &joined_event(clan, channel, UserId(6), "Viewer 6", "", false),
                    cx,
                );
                store.handle_streaming_leaved(&leaved_event(clan, channel, UserId(5)), cx);
            });
        });
        cx.update(|cx| {
            let store = store.read(cx);
            assert!(store.stream_publisher(channel).is_none());
            assert_eq!(store.viewer_count(channel), 1);
            assert_eq!(store.members_for_channel(channel)[0].user_id, UserId(6));
        });
    }

    #[gpui::test]
    fn picking_auto_again_clears_the_step_down_cap(cx: &mut gpui::TestAppContext) {
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.quality = StreamQuality::Auto;
                store.auto_cap = Some(0);
                store.set_quality(StreamQuality::Auto, cx);
                assert_eq!(store.auto_cap, None);
                store.auto_cap = Some(1);
                store.set_quality(StreamQuality::Low, cx);
                assert_eq!(store.auto_cap, None);
                assert_eq!(store.quality(), StreamQuality::Low);
            });
        });
    }

    #[test]
    fn unknown_stored_quality_falls_back_to_auto() {
        let quality: StreamQuality = serde_json::from_str("\"source\"").unwrap();
        assert_eq!(quality, StreamQuality::Auto);
        let quality: StreamQuality = serde_json::from_str("\"low\"").unwrap();
        assert_eq!(quality, StreamQuality::Low);
    }

    #[gpui::test]
    fn fail_stream_resets_fullscreen_and_controls_hovered(cx: &mut gpui::TestAppContext) {
        let store = init_store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.fullscreen = true;
                store.controls_visible = false;
                store.controls_hovered = true;
                store.fail_stream("Playback failed".into(), cx);
            });
        });

        cx.update(|cx| {
            let store = store.read(cx);
            assert!(!store.fullscreen());
            assert!(!store.controls_hovered);
            assert!(!store.controls_visible());
            assert_eq!(store.phase(), &StreamPhase::Error("Playback failed".into()));
        });
    }
}
