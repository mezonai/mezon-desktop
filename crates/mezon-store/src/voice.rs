use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, Global, RenderImage, Task, Window};
use mezon_audio::{AudioPlayer, DecodedPcm};
use mezon_client::{AppApi, RealtimeEvent};
use mezon_voice::{IceServerConfig, VoiceEvent, VoiceSession};
use parking_lot::Mutex;

pub use mezon_voice::{
    NetworkQuality, PickedScreen, ScreenShareKind, ScreenShareListError, ScreenShareOption,
    ScreenSharePreview, VideoFrameData, VideoFrameStore, VoiceParticipant,
    capture_screen_share_preview, list_screen_share_options, peek_screen_share_options,
};

use crate::AppConfig;
use crate::clan_members::ClanMembersStore;
use crate::ids::{ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MEET_TOKEN_CACHE_TTL: Duration = Duration::from_secs(45);
const RAISE_HAND_TTL: Duration = Duration::from_secs(10);
const REACTION_THROTTLE: Duration = Duration::from_millis(150);
const SOUND_REACTION_VOLUME: f32 = 0.3;
const SOUND_REACTION_TAIL: Duration = Duration::from_millis(300);
const SOUND_REACTION_THROTTLE: Duration = Duration::from_millis(500);
const SOUND_CACHE_CAP: usize = 8;
const EMOJI_REACTION_RATE_LIMIT: Duration = Duration::from_millis(150);
const EMOJI_REACTION_TAIL: Duration = Duration::from_millis(500);
const MAX_DISPLAYED_REACTIONS: usize = 20;
const DEFAULT_NOISE_SUPPRESSION_LEVEL: u8 = 20;
static RAISE_HAND_SOUND: &[u8] = include_bytes!("../assets/audio/raising-hand.mp3");

fn parse_raise_token(token: &str) -> Option<bool> {
    if token.starts_with("raising-up:") {
        Some(true)
    } else if token.starts_with("raising-down:") {
        Some(false)
    } else {
        None
    }
}

fn reaction_scatter(seq: u64, salt: u64) -> f32 {
    let h = seq.wrapping_add(salt).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h >> 40) as f32 / (1u64 << 24) as f32
}

struct CachedMeetToken {
    channel_id: String,
    token: String,
    fetched_at: Instant,
}

struct CachedRenderImage {
    seq: u64,
    image: Arc<RenderImage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceConnection {
    Idle,
    Connecting { channel_id: String, clan_id: String },
    Connected { channel_id: String, clan_id: String },
    Failed { channel_id: String, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCallStatus {
    Stable,
    WeakNetwork,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceModerationError {
    MuteFailed,
    KickFailed,
}

#[derive(Clone, Copy)]
enum ModerationAction {
    Mute,
    Kick,
}

impl ModerationAction {
    fn error(self) -> VoiceModerationError {
        match self {
            ModerationAction::Mute => VoiceModerationError::MuteFailed,
            ModerationAction::Kick => VoiceModerationError::KickFailed,
        }
    }
}

impl VoiceConnection {
    pub fn active_channel_id(&self) -> Option<&str> {
        match self {
            VoiceConnection::Connecting { channel_id, .. }
            | VoiceConnection::Connected { channel_id, .. } => Some(channel_id),
            _ => None,
        }
    }

    pub fn connected_channel(&self) -> Option<(&str, &str)> {
        match self {
            VoiceConnection::Connected {
                channel_id,
                clan_id,
            } => Some((channel_id, clan_id)),
            _ => None,
        }
    }
}

pub struct VoiceStore {
    api: Arc<AppApi>,
    connection: VoiceConnection,
    call_status: VoiceCallStatus,
    channel_label: String,
    mic_enabled: bool,
    mic_permission_denied: bool,
    camera_enabled: bool,
    screen_share_enabled: bool,
    noise_suppression_enabled: bool,
    noise_suppression_level: u8,
    focused_tile: Option<String>,
    fullscreen_screen: Option<u64>,
    pip: Option<PipWindow>,
    room_name: String,
    participant_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    pending_kick: Option<(String, String)>,
    moderation_error: Option<VoiceModerationError>,
    participants: Vec<VoiceParticipant>,
    raised_hands: Vec<String>,
    raised_hand_timers: HashMap<String, Task<()>>,
    raising_hand_player: Option<AudioPlayer>,
    raising_hand_sound_loading: bool,
    last_reaction_send: Option<Instant>,
    active_sounds: HashMap<String, ActiveSound>,
    sound_throttle: HashMap<String, Instant>,
    sound_cache: Vec<(String, Arc<DecodedPcm>)>,
    sound_preview: Option<SoundPreview>,
    displayed_reactions: Vec<DisplayedReaction>,
    reaction_seq: u64,
    last_emoji_at: Option<Instant>,
    session: Option<VoiceSession>,
    frame_store: Option<Arc<VideoFrameStore>>,
    render_cache: Mutex<HashMap<u64, CachedRenderImage>>,
    pending_texture_drops: Mutex<Vec<Arc<RenderImage>>>,
    cached_meet_token: Option<CachedMeetToken>,
    meet_token_prefetching: Option<String>,
    link_copied: bool,
    _events_task: Option<Task<()>>,
    _link_copied_reset: Option<Task<()>>,
}

struct PipWindow {
    key: u64,
    handle: gpui::AnyWindowHandle,
}

struct ActiveSound {
    _player: Option<AudioPlayer>,
    _remove_timer: Option<Task<()>>,
    _fetch_task: Option<Task<()>>,
}

struct SoundPreview {
    url: String,
    _player: Option<AudioPlayer>,
    _end_timer: Option<Task<()>>,
    _fetch_task: Option<Task<()>>,
}

pub struct DisplayedReaction {
    pub seq: u64,
    pub emoji_src: String,
    pub display_name: String,
    pub left: f32,
    pub drift: f32,
    pub duration: Duration,
    _remove_timer: Task<()>,
}

struct GlobalVoiceStore(Entity<VoiceStore>);
impl Global for GlobalVoiceStore {}

pub fn screen_tile_id(identity: &str) -> String {
    format!("{identity}\u{1}screen")
}

pub fn camera_tile_id(identity: &str) -> String {
    format!("{identity}\u{1}camera")
}

impl VoiceStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalVoiceStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalVoiceStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalVoiceStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        Self {
            api,
            connection: VoiceConnection::Idle,
            call_status: VoiceCallStatus::Stable,
            channel_label: String::new(),
            mic_enabled: false,
            mic_permission_denied: false,
            camera_enabled: false,
            screen_share_enabled: false,
            noise_suppression_enabled: false,
            noise_suppression_level: DEFAULT_NOISE_SUPPRESSION_LEVEL,
            focused_tile: None,
            fullscreen_screen: None,
            pip: None,
            room_name: String::new(),
            participant_menu: None,
            pending_kick: None,
            moderation_error: None,
            participants: Vec::new(),
            raised_hands: Vec::new(),
            raised_hand_timers: HashMap::new(),
            raising_hand_player: None,
            raising_hand_sound_loading: false,
            last_reaction_send: None,
            active_sounds: HashMap::new(),
            sound_throttle: HashMap::new(),
            sound_cache: Vec::new(),
            sound_preview: None,
            displayed_reactions: Vec::new(),
            reaction_seq: 0,
            last_emoji_at: None,
            session: None,
            frame_store: None,
            render_cache: Mutex::new(HashMap::new()),
            pending_texture_drops: Mutex::new(Vec::new()),
            cached_meet_token: None,
            meet_token_prefetching: None,
            link_copied: false,
            _events_task: None,
            _link_copied_reset: None,
        }
    }

    fn cached_token_for(&self, channel_id: &str) -> Option<String> {
        let cached = self.cached_meet_token.as_ref()?;
        if cached.channel_id == channel_id && cached.fetched_at.elapsed() < MEET_TOKEN_CACHE_TTL {
            return Some(cached.token.clone());
        }
        None
    }

    pub fn prefetch_meet_token(&mut self, channel_id: String, cx: &mut Context<Self>) {
        if self.cached_token_for(&channel_id).is_some() {
            return;
        }
        if self.meet_token_prefetching.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.meet_token_prefetching = Some(channel_id.clone());
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let token = api.generate_meet_token(&channel_id, "").await;
            let _ = this.update(cx, |this, _| {
                if this.meet_token_prefetching.as_deref() == Some(channel_id.as_str()) {
                    this.meet_token_prefetching = None;
                }
                if let Ok(token) = token {
                    this.cached_meet_token = Some(CachedMeetToken {
                        channel_id: channel_id.clone(),
                        token,
                        fetched_at: Instant::now(),
                    });
                }
            });
        })
        .detach();
    }

    pub fn connection(&self) -> &VoiceConnection {
        &self.connection
    }

    pub fn call_status(&self) -> VoiceCallStatus {
        self.call_status
    }

    pub fn channel_label(&self) -> &str {
        &self.channel_label
    }

    pub fn participants(&self) -> &[VoiceParticipant] {
        &self.participants
    }

    pub fn mic_enabled(&self) -> bool {
        self.mic_enabled
    }

    pub fn mic_permission_denied(&self) -> bool {
        self.mic_permission_denied
    }

    pub fn dismiss_mic_permission_prompt(&mut self, cx: &mut Context<Self>) {
        if self.mic_permission_denied {
            self.mic_permission_denied = false;
            cx.notify();
        }
    }

    pub fn camera_enabled(&self) -> bool {
        self.camera_enabled
    }

    pub fn screen_share_enabled(&self) -> bool {
        self.screen_share_enabled
    }

    pub fn noise_suppression_enabled(&self) -> bool {
        self.noise_suppression_enabled
    }

    pub fn noise_suppression_level(&self) -> u8 {
        self.noise_suppression_level
    }

    pub fn toggle_noise_suppression(&mut self, cx: &mut Context<Self>) {
        self.noise_suppression_enabled = !self.noise_suppression_enabled;
        self.sync_noise_suppression();
        cx.notify();
    }

    pub fn set_noise_suppression_level(&mut self, level: u8, cx: &mut Context<Self>) {
        let level = level.min(100);
        if self.noise_suppression_level == level {
            return;
        }
        self.noise_suppression_level = level;
        self.sync_noise_suppression();
        cx.notify();
    }

    fn sync_noise_suppression(&self) {
        if let Some(session) = &self.session {
            session.set_noise_suppression(
                self.noise_suppression_enabled,
                self.noise_suppression_level,
            );
        }
    }

    pub fn frame_store(&self) -> Option<Arc<VideoFrameStore>> {
        self.frame_store.clone()
    }

    pub fn render_image(&self, key: u64) -> Option<Arc<RenderImage>> {
        let store = self.frame_store.as_ref()?;
        let cached_seq = self.render_cache.lock().get(&key).map(|entry| entry.seq);
        let Some(frame) = store.take_new(key, cached_seq) else {
            return self
                .render_cache
                .lock()
                .get(&key)
                .map(|entry| entry.image.clone());
        };
        let seq = frame.seq;
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            buffer,
        )]));
        let previous = self.render_cache.lock().insert(
            key,
            CachedRenderImage {
                seq,
                image: image.clone(),
            },
        );
        if let Some(previous) = previous {
            self.pending_texture_drops.lock().push(previous.image);
        }
        Some(image)
    }

    fn evict_stale_render_cache(&self) {
        let mut cache = self.render_cache.lock();
        if cache.is_empty() {
            return;
        }
        let mut live_keys: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for participant in &self.participants {
            if let Some(key) = participant.camera {
                live_keys.insert(key);
            }
            if let Some(key) = participant.screenshare {
                live_keys.insert(key);
            }
        }
        let mut drops = self.pending_texture_drops.lock();
        cache.retain(|key, entry| {
            if live_keys.contains(key) {
                true
            } else {
                drops.push(entry.image.clone());
                false
            }
        });
    }

    pub fn flush_texture_drops(&self, mut window: Option<&mut Window>, cx: &mut App) {
        let drops: Vec<Arc<RenderImage>> = std::mem::take(&mut *self.pending_texture_drops.lock());
        for image in drops {
            cx.drop_image(image, window.as_deref_mut());
        }
    }

    pub fn focused_tile(&self) -> Option<&str> {
        self.focused_tile.as_deref()
    }

    pub fn link_copied(&self) -> bool {
        self.link_copied
    }

    pub fn mark_link_copied(&mut self, cx: &mut Context<Self>) {
        self.link_copied = true;
        cx.notify();
        self._link_copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            this.update(cx, |this, cx| {
                this.link_copied = false;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn raised_hands(&self) -> &[String] {
        &self.raised_hands
    }

    fn local_user_id(&self) -> Option<String> {
        self.participants
            .iter()
            .find(|p| p.is_local)
            .map(|p| p.identity.clone())
    }

    pub fn is_local_hand_raised(&self) -> bool {
        self.local_user_id()
            .is_some_and(|id| self.raised_hands.contains(&id))
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::VoiceReaction, &entity, |this, event, cx| {
                this.handle_voice_reaction(event, cx)
            });
        });
    }

    fn handle_voice_reaction(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::VoiceReaction(msg) = event else {
            return;
        };
        if self
            .connection
            .active_channel_id()
            .and_then(|c| c.parse::<i64>().ok())
            != Some(msg.channel_id)
        {
            return;
        }
        let Some(token) = msg.emojis.first() else {
            return;
        };
        if let Some(sound_url) = token.strip_prefix("sound:") {
            self.handle_sound_reaction(msg.sender_id.to_string(), sound_url.to_string(), cx);
            return;
        }
        let Some(raise) = parse_raise_token(token) else {
            self.handle_emoji_reaction(msg.sender_id.to_string(), token.clone(), cx);
            return;
        };
        let sender_id = msg.sender_id.to_string();
        if raise {
            self.add_raised_hand(sender_id, cx);
            self.play_raise_sound(cx);
        } else {
            self.remove_raised_hand(&sender_id, cx);
        }
    }

    fn add_raised_hand(&mut self, user_id: String, cx: &mut Context<Self>) {
        let inserted = if self.raised_hands.contains(&user_id) {
            false
        } else {
            self.raised_hands.push(user_id.clone());
            true
        };
        let key = user_id.clone();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RAISE_HAND_TTL).await;
            this.update(cx, |this, cx| this.remove_raised_hand(&key, cx))
                .ok();
        });
        self.raised_hand_timers.insert(user_id, timer);
        if inserted {
            cx.notify();
        }
    }

    fn remove_raised_hand(&mut self, user_id: &str, cx: &mut Context<Self>) {
        let before = self.raised_hands.len();
        self.raised_hands.retain(|h| h != user_id);
        self.raised_hand_timers.remove(user_id);
        if self.raised_hands.len() != before {
            cx.notify();
        }
    }

    pub fn send_raising_hand(&mut self, cx: &mut Context<Self>) {
        let Some(channel_id) = self
            .connection
            .active_channel_id()
            .and_then(|c| c.parse::<i64>().ok())
        else {
            return;
        };
        let Some(user_id) = self.local_user_id() else {
            return;
        };
        if !self.can_send_reaction() {
            return;
        }
        let raise = !self.raised_hands.contains(&user_id);
        let token = if raise {
            format!("raising-up:{user_id}")
        } else {
            format!("raising-down:{user_id}")
        };
        if raise {
            self.add_raised_hand(user_id, cx);
        } else {
            self.remove_raised_hand(&user_id, cx);
        }
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.write_voice_reaction(vec![token], channel_id).await {
                tracing::warn!("write_voice_reaction failed: {e}");
            }
        })
        .detach();
    }

    fn can_send_reaction(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_reaction_send
            && now.duration_since(last) < REACTION_THROTTLE
        {
            return false;
        }
        self.last_reaction_send = Some(now);
        true
    }

    fn play_raise_sound(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.raising_hand_player {
            player.play();
            return;
        }
        if self.raising_hand_sound_loading {
            return;
        }
        self.raising_hand_sound_loading = true;
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(RAISE_HAND_SOUND) })
                .await;
            this.update(cx, |this, _| {
                this.raising_hand_sound_loading = false;
                let Ok(pcm) = decoded else {
                    return;
                };
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                player.play();
                this.raising_hand_player = Some(player);
            })
            .ok();
        })
        .detach();
    }

    pub fn is_sound_active(&self, user_id: &str) -> bool {
        self.active_sounds.contains_key(user_id)
    }

    fn handle_sound_reaction(
        &mut self,
        user_id: String,
        sound_url: String,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        if self
            .sound_throttle
            .get(&user_id)
            .is_some_and(|last| now.duration_since(*last) < SOUND_REACTION_THROTTLE)
        {
            return;
        }
        self.sound_throttle
            .retain(|_, last| now.duration_since(*last) < SOUND_REACTION_THROTTLE);
        self.sound_throttle.insert(user_id.clone(), now);

        if let Some(pcm) = self
            .sound_cache
            .iter()
            .find(|(url, _)| *url == sound_url)
            .map(|(_, pcm)| pcm.clone())
        {
            self.active_sounds.insert(
                user_id.clone(),
                ActiveSound {
                    _player: None,
                    _remove_timer: None,
                    _fetch_task: None,
                },
            );
            self.attach_sound_playback(&user_id, &pcm, cx);
            cx.notify();
            return;
        }

        let key = user_id.clone();
        let fetch_task = cx.spawn(async move |this, cx| {
            let bytes = match mezon_client::transport_runtime::fetch_bytes(&sound_url).await {
                Ok((bytes, _)) => bytes,
                Err(e) => {
                    tracing::warn!("sound reaction fetch failed: {e}");
                    this.update(cx, |this, cx| this.remove_active_sound(&key, cx))
                        .ok();
                    return;
                }
            };
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(&bytes) })
                .await;
            this.update(cx, |this, cx| {
                if !this.active_sounds.contains_key(&key) {
                    return;
                }
                match decoded {
                    Ok(pcm) => {
                        let pcm = Arc::new(pcm);
                        this.cache_sound_pcm(sound_url, pcm.clone());
                        this.attach_sound_playback(&key, &pcm, cx);
                    }
                    Err(e) => {
                        tracing::warn!("sound reaction decode failed: {e}");
                        this.remove_active_sound(&key, cx);
                    }
                }
            })
            .ok();
        });
        self.active_sounds.insert(
            user_id,
            ActiveSound {
                _player: None,
                _remove_timer: None,
                _fetch_task: Some(fetch_task),
            },
        );
        cx.notify();
    }

    fn cache_sound_pcm(&mut self, url: String, pcm: Arc<DecodedPcm>) {
        if self.sound_cache.iter().any(|(u, _)| *u == url) {
            return;
        }
        if self.sound_cache.len() >= SOUND_CACHE_CAP {
            self.sound_cache.remove(0);
        }
        self.sound_cache.push((url, pcm));
    }

    fn attach_sound_playback(
        &mut self,
        user_id: &str,
        pcm: &Arc<DecodedPcm>,
        cx: &mut Context<Self>,
    ) {
        let delay = Duration::try_from_secs_f64(pcm.duration_secs()).unwrap_or(Duration::ZERO)
            + SOUND_REACTION_TAIL;
        let player = AudioPlayer::new().ok().inspect(|player| {
            player.set_volume(SOUND_REACTION_VOLUME);
            player.set_data(DecodedPcm {
                samples: pcm.samples.clone(),
                channels: pcm.channels,
                sample_rate: pcm.sample_rate,
            });
            player.play();
        });
        let key = user_id.to_string();
        let remove_timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.remove_active_sound(&key, cx))
                .ok();
        });
        if let Some(active) = self.active_sounds.get_mut(user_id) {
            active._player = player;
            active._remove_timer = Some(remove_timer);
        }
    }

    fn remove_active_sound(&mut self, user_id: &str, cx: &mut Context<Self>) {
        if self.active_sounds.remove(user_id).is_some() {
            cx.notify();
        }
    }

    pub fn displayed_reactions(&self) -> &[DisplayedReaction] {
        &self.displayed_reactions
    }

    fn handle_emoji_reaction(
        &mut self,
        sender_id: String,
        emoji_id: String,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        if let Some(last) = self.last_emoji_at
            && now.duration_since(last) < EMOJI_REACTION_RATE_LIMIT
        {
            return;
        }
        self.last_emoji_at = Some(now);

        let display_name = self.resolve_reaction_name(&sender_id, cx);
        let emoji_src = AppConfig::try_global(cx)
            .map(|cfg| cfg.emoji_src(&emoji_id))
            .unwrap_or_default();
        self.reaction_seq = self.reaction_seq.wrapping_add(1);
        let seq = self.reaction_seq;
        let left = 0.3 + reaction_scatter(seq, 1) * 0.4;
        let drift = (reaction_scatter(seq, 2) - 0.5) * 0.12;
        let duration = Duration::from_secs_f32(2.5 + reaction_scatter(seq, 3) * 3.5);

        let remove_timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(duration + EMOJI_REACTION_TAIL)
                .await;
            this.update(cx, |this, cx| {
                this.displayed_reactions.retain(|r| r.seq != seq);
                cx.notify();
            })
            .ok();
        });

        self.displayed_reactions.push(DisplayedReaction {
            seq,
            emoji_src,
            display_name,
            left,
            drift,
            duration,
            _remove_timer: remove_timer,
        });
        if self.displayed_reactions.len() > MAX_DISPLAYED_REACTIONS {
            self.displayed_reactions.remove(0);
        }
        cx.notify();
    }

    fn resolve_reaction_name(&self, sender_id: &str, cx: &App) -> String {
        let Some((_, clan)) = self.connection.connected_channel() else {
            return String::new();
        };
        let (Ok(clan_id), Ok(uid)) = (clan.parse::<i64>(), sender_id.parse::<i64>()) else {
            return String::new();
        };
        ClanMembersStore::try_global(cx)
            .and_then(|store| {
                store
                    .read(cx)
                    .member(ClanId(clan_id), UserId(uid))
                    .map(|m| m.name().to_string())
            })
            .unwrap_or_default()
    }

    pub fn send_emoji_reaction(&mut self, emoji_id: String, cx: &mut Context<Self>) {
        let Some(channel_id) = self
            .connection
            .active_channel_id()
            .and_then(|c| c.parse::<i64>().ok())
        else {
            return;
        };
        if !self.can_send_reaction() {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api.write_voice_reaction(vec![emoji_id], channel_id).await {
                tracing::warn!("send_emoji_reaction failed: {e}");
            }
        })
        .detach();
    }

    pub fn send_sound_reaction(&mut self, sound_url: String, cx: &mut Context<Self>) {
        let Some(channel_id) = self
            .connection
            .active_channel_id()
            .and_then(|c| c.parse::<i64>().ok())
        else {
            return;
        };
        if !self.can_send_reaction() {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |_this, _cx| {
            if let Err(e) = api
                .write_voice_reaction(vec![format!("sound:{sound_url}")], channel_id)
                .await
            {
                tracing::warn!("send_sound_reaction failed: {e}");
            }
        })
        .detach();
    }

    pub fn previewing_sound(&self) -> Option<&str> {
        self.sound_preview.as_ref().map(|p| p.url.as_str())
    }

    pub fn stop_sound_preview(&mut self, cx: &mut Context<Self>) {
        if self.sound_preview.take().is_some() {
            cx.notify();
        }
    }

    pub fn toggle_sound_preview(&mut self, url: String, cx: &mut Context<Self>) {
        if self.previewing_sound() == Some(url.as_str()) {
            self.stop_sound_preview(cx);
            return;
        }
        if let Some(pcm) = self
            .sound_cache
            .iter()
            .find(|(u, _)| *u == url)
            .map(|(_, pcm)| pcm.clone())
        {
            self.sound_preview = Some(SoundPreview {
                url: url.clone(),
                _player: None,
                _end_timer: None,
                _fetch_task: None,
            });
            self.start_sound_preview(&url, &pcm, cx);
            cx.notify();
            return;
        }
        let fetch_url = url.clone();
        let fetch_task = cx.spawn(async move |this, cx| {
            let bytes = match mezon_client::transport_runtime::fetch_bytes(&fetch_url).await {
                Ok((bytes, _)) => bytes,
                Err(e) => {
                    tracing::warn!("sound preview fetch failed: {e}");
                    this.update(cx, |this, cx| this.clear_sound_preview(&fetch_url, cx))
                        .ok();
                    return;
                }
            };
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(&bytes) })
                .await;
            this.update(cx, |this, cx| {
                if this.previewing_sound() != Some(fetch_url.as_str()) {
                    return;
                }
                match decoded {
                    Ok(pcm) => {
                        let pcm = Arc::new(pcm);
                        this.cache_sound_pcm(fetch_url.clone(), pcm.clone());
                        this.start_sound_preview(&fetch_url, &pcm, cx);
                    }
                    Err(e) => {
                        tracing::warn!("sound preview decode failed: {e}");
                        this.clear_sound_preview(&fetch_url, cx);
                    }
                }
            })
            .ok();
        });
        self.sound_preview = Some(SoundPreview {
            url,
            _player: None,
            _end_timer: None,
            _fetch_task: Some(fetch_task),
        });
        cx.notify();
    }

    fn start_sound_preview(&mut self, url: &str, pcm: &Arc<DecodedPcm>, cx: &mut Context<Self>) {
        let delay = Duration::try_from_secs_f64(pcm.duration_secs()).unwrap_or(Duration::ZERO);
        let player = AudioPlayer::new().ok().inspect(|player| {
            player.set_data(DecodedPcm {
                samples: pcm.samples.clone(),
                channels: pcm.channels,
                sample_rate: pcm.sample_rate,
            });
            player.play();
        });
        let key = url.to_string();
        let end_timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.clear_sound_preview(&key, cx))
                .ok();
        });
        if let Some(preview) = self.sound_preview.as_mut().filter(|p| p.url == url) {
            preview._player = player;
            preview._end_timer = Some(end_timer);
        }
    }

    fn clear_sound_preview(&mut self, url: &str, cx: &mut Context<Self>) {
        if self.previewing_sound() == Some(url) {
            self.sound_preview = None;
            cx.notify();
        }
    }

    fn desired_screen_full_res(&self) -> bool {
        let Some(local) = self.participants.iter().find(|p| p.is_local) else {
            return false;
        };
        let Some(screen_key) = local.screenshare else {
            return false;
        };
        let focused = self
            .focused_tile
            .as_deref()
            .is_some_and(|id| id == screen_tile_id(&local.identity));
        let fullscreen = self.fullscreen_screen == Some(screen_key);
        let pip = self.pip.as_ref().is_some_and(|p| p.key == screen_key);
        focused || fullscreen || pip
    }

    fn sync_screen_full_res(&self) {
        if let Some(session) = &self.session {
            session.set_screen_full_res(self.desired_screen_full_res());
        }
    }

    pub fn toggle_focus(&mut self, id: String, cx: &mut Context<Self>) {
        if self.focused_tile.as_deref() == Some(id.as_str()) {
            self.focused_tile = None;
        } else {
            self.focused_tile = Some(id);
        }
        self.sync_screen_full_res();
        cx.notify();
    }

    pub fn set_focus(&mut self, id: String, cx: &mut Context<Self>) {
        if self.focused_tile.as_deref() != Some(id.as_str()) {
            self.focused_tile = Some(id);
            self.sync_screen_full_res();
            cx.notify();
        }
    }

    pub fn clear_focus(&mut self, cx: &mut Context<Self>) {
        if self.focused_tile.take().is_some() {
            self.sync_screen_full_res();
            cx.notify();
        }
    }

    pub fn fullscreen_screen(&self) -> Option<u64> {
        self.fullscreen_screen
    }

    pub fn pip_key(&self) -> Option<u64> {
        self.pip.as_ref().map(|p| p.key)
    }

    pub fn primary_screen_key(&self) -> Option<u64> {
        if let Some(key) = self.fullscreen_screen {
            return Some(key);
        }
        if let Some(focused) = self.focused_tile.as_deref()
            && let Some(key) = self
                .participants
                .iter()
                .find(|p| p.screenshare.is_some() && screen_tile_id(&p.identity) == focused)
                .and_then(|p| p.screenshare)
        {
            return Some(key);
        }
        self.participants.iter().find_map(|p| p.screenshare)
    }

    pub fn toggle_fullscreen_screen(&mut self, key: u64, cx: &mut Context<Self>) {
        self.fullscreen_screen = if self.fullscreen_screen == Some(key) {
            None
        } else {
            Some(key)
        };
        self.sync_screen_full_res();
        cx.notify();
    }

    pub fn clear_fullscreen_screen(&mut self, cx: &mut Context<Self>) {
        if self.fullscreen_screen.take().is_some() {
            self.sync_screen_full_res();
            cx.notify();
        }
    }

    pub fn set_pip(&mut self, key: u64, handle: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        if let Some(prev) = self.pip.take() {
            let _ = prev
                .handle
                .update(cx, |_, window, _| window.remove_window());
        }
        self.pip = Some(PipWindow { key, handle });
        self.sync_screen_full_res();
        cx.notify();
    }

    pub fn close_pip(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.pip.take() {
            let _ = prev
                .handle
                .update(cx, |_, window, _| window.remove_window());
            self.sync_screen_full_res();
            cx.notify();
        }
    }

    pub fn on_pip_closed(&mut self, key: u64, cx: &mut Context<Self>) {
        if self.pip.as_ref().is_some_and(|p| p.key == key) {
            self.pip = None;
            self.sync_screen_full_res();
            cx.notify();
        }
    }

    pub fn participant_menu(&self) -> Option<(&str, gpui::Point<gpui::Pixels>)> {
        self.participant_menu
            .as_ref()
            .map(|(identity, position)| (identity.as_str(), *position))
    }

    pub fn open_participant_menu(
        &mut self,
        identity: String,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.participant_menu = Some((identity, position));
        cx.notify();
    }

    pub fn close_participant_menu(&mut self, cx: &mut Context<Self>) {
        if self.participant_menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn take_moderation_error(&mut self) -> Option<VoiceModerationError> {
        self.moderation_error.take()
    }

    pub fn mute_participant(&mut self, identity: String, cx: &mut Context<Self>) {
        self.moderate_participant(identity, ModerationAction::Mute, cx);
    }

    pub fn pending_kick(&self) -> Option<(&str, &str)> {
        self.pending_kick
            .as_ref()
            .map(|(identity, name)| (identity.as_str(), name.as_str()))
    }

    pub fn request_kick(&mut self, identity: String, name: String, cx: &mut Context<Self>) {
        self.pending_kick = Some((identity, name));
        cx.notify();
    }

    pub fn cancel_kick(&mut self, cx: &mut Context<Self>) {
        if self.pending_kick.take().is_some() {
            cx.notify();
        }
    }

    pub fn confirm_kick(&mut self, cx: &mut Context<Self>) {
        let Some((identity, _)) = self.pending_kick.take() else {
            return;
        };
        cx.notify();
        self.moderate_participant(identity, ModerationAction::Kick, cx);
    }

    fn moderate_participant(
        &mut self,
        identity: String,
        action: ModerationAction,
        cx: &mut Context<Self>,
    ) {
        let Some((channel_id, clan_id)) = self.connection.connected_channel() else {
            return;
        };
        if self.room_name.is_empty() {
            return;
        }
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();
        let room_name = self.room_name.clone();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = match action {
                ModerationAction::Mute => {
                    api.mute_participant_mezon_meet(&channel_id, &clan_id, &room_name, &identity)
                        .await
                }
                ModerationAction::Kick => {
                    api.remove_participant_mezon_meet(&channel_id, &clan_id, &room_name, &identity)
                        .await
                }
            };
            if let Err(e) = result {
                tracing::warn!("participant moderation failed: {e:#}");
                let _ = this.update(cx, |this, cx| {
                    this.moderation_error = Some(action.error());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn prune_screen_targets(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.fullscreen_screen
            && !self.participants.iter().any(|p| p.screenshare == Some(key))
        {
            self.fullscreen_screen = None;
        }
        if let Some(key) = self.pip.as_ref().map(|p| p.key)
            && !self.participants.iter().any(|p| p.screenshare == Some(key))
        {
            self.close_pip(cx);
        }
    }

    pub fn is_connected_to(&self, channel_id: &str) -> bool {
        matches!(self.connection.connected_channel(), Some((id, _)) if id == channel_id)
    }

    pub fn is_active_in(&self, channel_id: &str) -> bool {
        self.connection.active_channel_id() == Some(channel_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn join(
        &mut self,
        channel_id: String,
        clan_id: String,
        channel_label: String,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_active_in(&channel_id) {
            return;
        }

        self.teardown(Some(window), cx);
        self.channel_label = channel_label;

        let ws_url = AppConfig::global(cx).meet_ws_url.clone();
        if ws_url.is_empty() {
            self.connection = VoiceConnection::Failed {
                channel_id,
                message: "meet server URL is not configured".into(),
            };
            cx.notify();
            return;
        }

        self.connection = VoiceConnection::Connecting {
            channel_id: channel_id.clone(),
            clan_id: clan_id.clone(),
        };
        self.call_status = VoiceCallStatus::Stable;
        self.mic_enabled = false;
        self.mic_permission_denied = false;
        self.participants.clear();
        cx.notify();

        let api = self.api.clone();
        let cached_token = self.cached_token_for(&channel_id);
        if let Some(token) = cached_token {
            self.start_session(
                ws_url,
                token,
                channel_id,
                input_device_id,
                output_device_id,
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let token = api.generate_meet_token(&channel_id, "").await;
            let _ = this.update(cx, |this, cx| match token {
                Ok(token) => {
                    this.cached_meet_token = Some(CachedMeetToken {
                        channel_id: channel_id.clone(),
                        token: token.clone(),
                        fetched_at: Instant::now(),
                    });
                    this.start_session(
                        ws_url,
                        token,
                        channel_id,
                        input_device_id,
                        output_device_id,
                        cx,
                    );
                }
                Err(e) => {
                    tracing::error!("failed to generate meet token: {e:#}");
                    this.connection = VoiceConnection::Failed {
                        channel_id,
                        message: e.to_string(),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn start_session(
        &mut self,
        ws_url: String,
        token: String,
        channel_id: String,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.connection.active_channel_id() != Some(channel_id.as_str()) {
            return;
        }

        let ice_servers = Self::ice_servers(cx);
        let session = VoiceSession::connect(
            ws_url,
            token,
            input_device_id,
            output_device_id,
            ice_servers,
        );
        let events = session.events();
        self.frame_store = Some(session.frame_store());
        self.session = Some(session);
        self.sync_noise_suppression();

        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                if this
                    .update(cx, |this, cx| this.handle_engine_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self._events_task = Some(task);
        cx.notify();
    }

    fn ice_servers(cx: &App) -> Vec<IceServerConfig> {
        let config = AppConfig::global(cx);
        if config.webrtc_ice_servers_url.is_empty() {
            return Vec::new();
        }
        if config.webrtc_ice_servers_url.starts_with("turn")
            && config.webrtc_ice_servers_credential.is_empty()
        {
            tracing::warn!(
                "skipping TURN server {}: missing credential",
                config.webrtc_ice_servers_url
            );
            return Vec::new();
        }
        vec![IceServerConfig {
            urls: vec![config.webrtc_ice_servers_url.clone()],
            username: config.webrtc_ice_servers_username.clone(),
            credential: config.webrtc_ice_servers_credential.clone(),
        }]
    }

    pub fn has_active_video(&self) -> bool {
        self.camera_enabled
            || self.screen_share_enabled
            || self
                .participants
                .iter()
                .any(|p| p.camera.is_some() || p.screenshare.is_some())
    }

    fn handle_engine_event(&mut self, event: VoiceEvent, cx: &mut Context<Self>) {
        match event {
            VoiceEvent::Connected { room_name } => {
                self.room_name = room_name;
                if let VoiceConnection::Connecting {
                    channel_id,
                    clan_id,
                } = &self.connection
                {
                    self.connection = VoiceConnection::Connected {
                        channel_id: channel_id.clone(),
                        clan_id: clan_id.clone(),
                    };
                }
                self.call_status = VoiceCallStatus::Stable;
            }
            VoiceEvent::Reconnecting => {
                self.call_status = VoiceCallStatus::Reconnecting;
            }
            VoiceEvent::Reconnected => {
                self.call_status = VoiceCallStatus::Stable;
            }
            VoiceEvent::NetworkWeak => {
                if !matches!(self.call_status, VoiceCallStatus::Reconnecting) {
                    self.call_status = VoiceCallStatus::WeakNetwork;
                }
            }
            VoiceEvent::NetworkRecovered => {
                if matches!(self.call_status, VoiceCallStatus::WeakNetwork) {
                    self.call_status = VoiceCallStatus::Stable;
                }
            }
            VoiceEvent::Participants(list) => {
                self.participants = list;
                if let Some(local) = self.participants.iter().find(|p| p.is_local) {
                    self.mic_enabled = !local.muted;
                    self.camera_enabled = local.camera.is_some();
                    self.screen_share_enabled = local.screenshare.is_some();
                }
                self.evict_stale_render_cache();
                self.flush_texture_drops(None, cx);
                self.prune_screen_targets(cx);
                self.sync_screen_full_res();
            }
            VoiceEvent::Disconnected { reason } => {
                tracing::info!("voice disconnected: {reason}");
                self.teardown(None, cx);
            }
            VoiceEvent::Error(message) => {
                tracing::warn!("voice error: {message}");
                if message.starts_with("camera:") {
                    self.camera_enabled = false;
                } else if message.starts_with("screen:") {
                    self.screen_share_enabled = false;
                } else if let VoiceConnection::Connecting { channel_id, .. } = &self.connection {
                    self.connection = VoiceConnection::Failed {
                        channel_id: channel_id.clone(),
                        message,
                    };
                }
            }
        }
        cx.notify();
    }

    pub fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.teardown(Some(window), cx);
        cx.notify();
    }

    pub fn logout_teardown(&mut self, cx: &mut Context<Self>) {
        self.teardown(None, cx);
        cx.notify();
    }

    pub fn toggle_mic(&mut self, cx: &mut Context<Self>) {
        self.set_mic_enabled(!self.mic_enabled, cx);
    }

    pub fn set_mic_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled && mezon_voice::microphone_denied() {
            self.mic_enabled = false;
            self.mic_permission_denied = true;
            cx.notify();
            return;
        }
        self.mic_permission_denied = false;
        self.mic_enabled = enabled;
        if let Some(session) = &self.session {
            session.set_mic_enabled(enabled);
        }
        cx.notify();
    }

    pub fn toggle_camera(&mut self, cx: &mut Context<Self>) {
        self.set_camera_enabled(!self.camera_enabled, cx);
    }

    pub fn set_camera_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.set_camera_enabled(enabled);
        }
        cx.notify();
    }

    pub fn start_screen_share(
        &mut self,
        pick: PickedScreen,
        share_audio: bool,
        cx: &mut Context<Self>,
    ) {
        if self.screen_share_enabled {
            return;
        }
        if let Some(session) = &self.session {
            session.start_screen_share(pick, share_audio);
        }
        cx.notify();
    }

    pub fn stop_screen_share(&mut self, cx: &mut Context<Self>) {
        if !self.screen_share_enabled {
            return;
        }
        if let Some(session) = &self.session {
            session.stop_screen_share();
        }
        cx.notify();
    }

    fn teardown(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.close_pip(cx);
        self.fullscreen_screen = None;
        self.session = None;
        self.frame_store = None;
        let stale: Vec<Arc<RenderImage>> = {
            let mut cache = self.render_cache.lock();
            cache.drain().map(|(_, entry)| entry.image).collect()
        };
        for image in stale {
            cx.drop_image(image, window.as_deref_mut());
        }
        self.flush_texture_drops(window, cx);
        self._events_task = None;
        self.connection = VoiceConnection::Idle;
        self.call_status = VoiceCallStatus::Stable;
        self.channel_label.clear();
        self.mic_enabled = false;
        self.mic_permission_denied = false;
        self.camera_enabled = false;
        self.screen_share_enabled = false;
        self.noise_suppression_enabled = false;
        self.noise_suppression_level = DEFAULT_NOISE_SUPPRESSION_LEVEL;
        self.focused_tile = None;
        self.room_name.clear();
        self.participant_menu = None;
        self.pending_kick = None;
        self.moderation_error = None;
        self.participants.clear();
        self.raised_hands.clear();
        self.raised_hand_timers.clear();
        self.active_sounds.clear();
        self.sound_throttle.clear();
        self.sound_cache.clear();
        self.sound_preview = None;
        self.raising_hand_player = None;
        self.raising_hand_sound_loading = false;
        self.displayed_reactions.clear();
        self.last_emoji_at = None;
        self.meet_token_prefetching = None;
        self.link_copied = false;
        self._link_copied_reset = None;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_raise_token;

    #[test]
    fn parse_raise_token_classifies_prefixes() {
        assert_eq!(parse_raise_token("raising-up:123"), Some(true));
        assert_eq!(parse_raise_token("raising-down:123"), Some(false));
        assert_eq!(parse_raise_token("sound:https://x.mp3"), None);
        assert_eq!(parse_raise_token(":smile:"), None);
        assert_eq!(parse_raise_token(""), None);
    }
}
