use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use futures::StreamExt;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, RenderImage, SharedString,
    Subscription, Task, Window,
};
use mezon_audio::{AudioPlayer, DecodedPcm};
use mezon_client::{
    AppApi, ChannelAppLaunchParams, RealtimeEvent, api_status_from_error, build_channel_app_url,
};
use mezon_voice::{
    IceServerConfig, MEET_TOKEN_RETRY_LIMIT, TokenRefresher, VoiceConnectOptions, VoiceEvent,
    VoiceSession,
};
use parking_lot::Mutex;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use mezon_voice::record_wayland_session;
pub use mezon_voice::{
    CameraDeviceInfo, NetworkQuality, PickedScreen, RemovalCause, ScreenShareKind,
    ScreenShareListError, ScreenShareMode, ScreenShareOption, ScreenSharePreview, SfuRole,
    VideoFrameData, VideoFrameStore, VoiceParticipant, capture_screen_share_preview,
    list_screen_share_options, peek_screen_share_options, system_screen_share_pick,
};

use crate::AppConfig;
use crate::Settings;
use crate::account::AccountStore;
use crate::clan_members::ClanMembersStore;
use crate::direct::DirectMessageStore;
use crate::gifts::{
    FLOWER_DEDUP_WINDOW, FLOWER_RATE_LIMIT, FLOWER_SCENE_TTL, GiveFlowerDeny, VoiceInteractiveApp,
    VoiceInteractiveEventType, app_reaction_token, build_flower_transfer, can_give_flower,
    flower_event_from_payload, flower_pair_key, flower_price, flower_reaction_token,
    format_flower_amount, is_uncertain_transfer_error, parse_app_reaction_token,
    parse_flower_reaction_token, serialize_flower_interactive_params,
};
use crate::ids::{ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::users_by_user::UsersByUserStore;
use crate::wallet::{WalletEvent, WalletStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    AudioInput,
    AudioOutput,
    VideoInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMenuKind {
    Microphone,
    Camera,
    ScreenShare,
}

const MEET_TOKEN_CACHE_TTL: Duration = Duration::from_secs(45);
const MAX_SFU_RECONNECT_ATTEMPTS: u32 = 4;
const SFU_RECONNECT_HEALTHY_SESSION: Duration = Duration::from_secs(30);
const RAISE_HAND_TTL: Duration = Duration::from_secs(10);
const RECORDING_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(20);
const RECORDING_INDICATOR_TTL: Duration = Duration::from_secs(50);
const REACTION_THROTTLE: Duration = Duration::from_millis(150);
const RECORDING_TICK_INTERVAL: Duration = Duration::from_secs(1);
const SOUND_REACTION_VOLUME: f32 = 0.3;
const SOUND_REACTION_TAIL: Duration = Duration::from_millis(300);
const SOUND_REACTION_THROTTLE: Duration = Duration::from_millis(500);
const SOUND_CACHE_CAP: usize = 8;
const EMOJI_REACTION_RATE_LIMIT: Duration = Duration::from_millis(150);
const INTERACTIVE_LAUNCH_DEDUP_TTL: Duration = Duration::from_secs(10);

fn interactive_app_fingerprint(sender_id: i64, clan_id: i64) -> u64 {
    let mut hasher = DefaultHasher::new();
    (sender_id, clan_id).hash(&mut hasher);
    hasher.finish()
}

fn release_interactive_window(
    opened: &mut HashMap<VoiceInteractiveApp, u64>,
    app: VoiceInteractiveApp,
    window_token: u64,
) -> bool {
    if opened.get(&app) != Some(&window_token) {
        return false;
    }
    opened.remove(&app);
    true
}

fn is_duplicate_interactive_launch(
    launches: &mut HashMap<(VoiceInteractiveApp, u64), Instant>,
    key: (VoiceInteractiveApp, u64),
    now: Instant,
) -> bool {
    launches
        .retain(|_, launched_at| now.duration_since(*launched_at) < INTERACTIVE_LAUNCH_DEDUP_TTL);
    launches.contains_key(&key)
}

fn is_duplicate_flower(shown: &mut HashMap<String, Instant>, pair: &str, now: Instant) -> bool {
    shown.retain(|_, shown_at| now.duration_since(*shown_at) < FLOWER_DEDUP_WINDOW);
    shown.contains_key(pair)
}

fn redact_interactive_app_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return "<invalid interactive app URL>".to_string();
    };
    let pairs = parsed
        .query_pairs()
        .map(|(key, value)| {
            let value = if key == "data" && !value.is_empty() {
                "REDACTED".to_string()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect::<Vec<_>>();
    parsed.query_pairs_mut().clear().extend_pairs(pairs);
    parsed.to_string()
}
const EMOJI_REACTION_TAIL: Duration = Duration::from_millis(500);
const MAX_DISPLAYED_REACTIONS: usize = 20;
const DEFAULT_NOISE_SUPPRESSION_LEVEL: u8 = 20;
pub const MAX_SOUND_BYTES: u64 = 1024 * 1024;
pub const SOUND_ALLOWED_EXTENSIONS: &[&str] = &["mp3", "wav", "mpeg"];
const KICK_SUPPRESS_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_STALL_TIMEOUT: Duration = Duration::from_secs(40);
const RECONNECT_RETRY_DELAY: Duration = Duration::from_secs(5);
static RAISE_HAND_SOUND: &[u8] = include_bytes!("../assets/audio/raising-hand.mp3");
static JOIN_VOICE_SOUND: &[u8] = include_bytes!("../assets/audio/joincallsound.mp3");
static GIVE_FLOWER_SOUND: &[u8] = include_bytes!("../assets/audio/give-flower.mp3");

fn parse_raise_token(token: &str) -> Option<bool> {
    if token.starts_with("raising-up:") {
        Some(true)
    } else if token.starts_with("raising-down:") {
        Some(false)
    } else {
        None
    }
}

#[derive(Clone, PartialEq)]
struct RecordingAnnouncement {
    clan_id: i64,
    channel_id: i64,
    user_id: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordingParams {
    is_recording: bool,
}

fn recording_params(recording: bool) -> String {
    serde_json::to_string(&RecordingParams {
        is_recording: recording,
    })
    .unwrap_or_default()
}

fn recording_from_params(params: &str) -> Option<bool> {
    serde_json::from_str::<RecordingParams>(params)
        .ok()
        .map(|params| params.is_recording)
}

async fn write_recording_state(
    api: &AppApi,
    announcement: &RecordingAnnouncement,
    recording: bool,
) {
    let Ok(sender_id) = announcement.user_id.parse::<i64>() else {
        return;
    };
    if let Err(e) = api
        .write_voice_interactive_event(
            announcement.clan_id,
            announcement.channel_id,
            sender_id,
            sender_id,
            VoiceInteractiveEventType::Recording as i32,
            recording_params(recording),
        )
        .await
    {
        tracing::warn!("recording state broadcast failed: {e:#}");
    }
}

fn reaction_scatter(seq: u64, salt: u64) -> f32 {
    let h = seq.wrapping_add(salt).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn current_locale(cx: &App) -> String {
    Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_default()
}

fn voice_join_error_message(err: &anyhow::Error, locale: &str) -> String {
    match api_status_from_error(err) {
        Some(status) => mezon_i18n::api_error(locale, status.code).to_string(),
        None => mezon_i18n::t(locale, "channelVoice.joinFailed").to_string(),
    }
}

fn meet_token_metadata_from_candidates(names: &[&str], avatars: &[&str]) -> String {
    let username = names
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let avatar = avatars
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    serde_json::json!({ "username": username, "avatar": avatar }).to_string()
}

struct CachedMeetToken {
    channel_id: String,
    metadata: String,
    token: String,
    fetched_at: Instant,
}

#[derive(Clone)]
pub enum VoiceRenderFrame {
    Image(Arc<RenderImage>),
    #[cfg(target_os = "macos")]
    Surface(mezon_voice::VideoSurface),
}

struct CachedRenderFrame {
    seq: u64,
    frame: VoiceRenderFrame,
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
    AgentFailed,
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

    pub fn active_channel(&self) -> Option<(&str, &str)> {
        match self {
            VoiceConnection::Connecting {
                channel_id,
                clan_id,
            }
            | VoiceConnection::Connected {
                channel_id,
                clan_id,
            } => Some((channel_id, clan_id)),
            _ => None,
        }
    }

    pub fn is_connecting(&self) -> bool {
        matches!(self, VoiceConnection::Connecting { .. })
    }

    fn mark_connected(&mut self) -> bool {
        let VoiceConnection::Connecting {
            channel_id,
            clan_id,
        } = self
        else {
            return false;
        };
        let channel_id = std::mem::take(channel_id);
        let clan_id = std::mem::take(clan_id);
        *self = VoiceConnection::Connected {
            channel_id,
            clan_id,
        };
        true
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
    auto_focused_screen: Option<String>,
    fullscreen_screen: Option<u64>,
    pip: Option<PipWindow>,
    room_name: String,
    participant_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    pending_kick: Option<(String, String)>,
    pending_removals: HashMap<String, Instant>,
    moderation_error: Option<VoiceModerationError>,
    muted_by_moderator: bool,
    agent_pending: bool,
    participants: Vec<VoiceParticipant>,
    join_ranks: Vec<String>,
    speak_ranks: HashMap<String, u64>,
    speak_seq: u64,
    raised_hands: Vec<String>,
    raised_hand_timers: HashMap<String, Task<()>>,
    recording_users: Vec<String>,
    recording_user_timers: HashMap<String, Task<()>>,
    recording_announced: Option<RecordingAnnouncement>,
    _recording_announce: Option<Task<()>>,
    raising_hand_player: Option<AudioPlayer>,
    raising_hand_sound_loading: bool,
    join_voice_player: Option<AudioPlayer>,
    join_voice_sound_loading: bool,
    give_flower_player: Option<AudioPlayer>,
    give_flower_sound_loading: bool,
    join_sound_baseline_set: bool,
    awaiting_room_snapshot: bool,
    last_reaction_send: Option<Instant>,
    last_flower_send: Option<Instant>,
    last_flower_effect_at: Option<Instant>,
    shown_flowers: HashMap<String, Instant>,
    flower_seq: u64,
    active_sounds: HashMap<String, ActiveSound>,
    sound_throttle: HashMap<String, Instant>,
    sound_cache: Vec<(String, Arc<DecodedPcm>)>,
    sound_preview: Option<SoundPreview>,
    displayed_reactions: Vec<DisplayedReaction>,
    displayed_flowers: Vec<DisplayedFlower>,
    reaction_seq: u64,
    last_emoji_at: Option<Instant>,
    session: Option<VoiceSession>,
    session_generation: u64,
    reconnect_generation: u64,
    reconnect_token_fetches: u32,
    reconnect_attempts: u32,
    reconnect_healthy_since: Option<Instant>,
    frame_store: Option<Arc<VideoFrameStore>>,
    camera_devices: Vec<CameraDeviceInfo>,
    device_menu: Option<DeviceMenuKind>,
    interactive_launches: HashMap<(VoiceInteractiveApp, u64), Instant>,
    active_interactive_apps: HashMap<VoiceInteractiveApp, (i64, i64)>,
    opened_interactive_apps: HashMap<VoiceInteractiveApp, u64>,
    interactive_window_seq: u64,
    device_submenu: Option<DeviceKind>,
    _camera_enum_task: Option<Task<()>>,
    render_cache: Mutex<HashMap<u64, CachedRenderFrame>>,
    pending_texture_drops: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_replaces: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_work: AtomicBool,
    cached_meet_token: Option<CachedMeetToken>,
    role: SfuRole,
    ptt_active: bool,
    ptt_held: bool,
    hold_to_talk: bool,
    ptt_hint_dismissed: bool,
    pending_join_role: SfuRole,
    join_role_menu_open: bool,
    meet_token_prefetching: Option<String>,
    last_screen_share: Option<(PickedScreen, bool)>,
    screen_share_mode: ScreenShareMode,
    link_copied: bool,
    recording: RecordingState,
    recording_elapsed: Duration,
    recording_stalled: bool,
    member_strip_visible: bool,
    recording_avatars: HashMap<String, RecordingAvatar>,
    #[cfg(debug_assertions)]
    simulated_participants: Vec<VoiceParticipant>,
    _recording_tick: Option<Task<()>>,
    _recording_start: Option<Task<()>>,
    _events_task: Option<Task<()>>,
    _reconnect_watch_task: Option<Task<()>>,
    _link_copied_reset: Option<Task<()>>,
    _app_quit_subscription: Subscription,
    _channel_list_subscription: Option<Subscription>,
    _recording_announcement: Subscription,
}

#[derive(Clone)]
struct VoiceReconnectSnapshot {
    channel_id: String,
    metadata: String,
    clan_id: String,
    ws_url: String,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    mic_enabled: bool,
    camera_enabled: bool,
    screen_share: Option<(PickedScreen, bool)>,
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
    _tick_task: Option<Task<()>>,
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

pub struct DisplayedFlower {
    pub key: String,
    pub started_at: Instant,
    pub caption: SharedString,
    _remove_timer: Task<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingState {
    #[default]
    Idle,
    Starting,
    Recording,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingToast {
    Saved(std::path::PathBuf),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceStoreEvent {
    RecordingFinished(RecordingToast),
    RecordingVideoUnavailable,
    RemovedFromChannel(RemovalCause),
}

impl EventEmitter<VoiceStoreEvent> for VoiceStore {}

struct GlobalVoiceStore(Entity<VoiceStore>);
impl Global for GlobalVoiceStore {}

impl VoiceStore {
    fn display_name_for(&self, participant: &VoiceParticipant, cx: &App) -> String {
        let resolved = self.resolve_reaction_name(&participant.identity, cx);
        if !resolved.is_empty() {
            return resolved;
        }
        if participant.name.chars().all(|c| c.is_ascii_digit()) {
            return String::new();
        }
        participant.name.clone()
    }
}

#[cfg(debug_assertions)]
const SIMULATED_SCREEN_KEY: u64 = u64::MAX - 1;

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy)]
pub struct SimulatedCall {
    pub screenshare: bool,
    pub focus: bool,
    pub fullscreen: bool,
    pub member_strip: bool,
}

#[cfg(debug_assertions)]
impl Default for SimulatedCall {
    fn default() -> Self {
        Self {
            screenshare: false,
            focus: false,
            fullscreen: false,
            member_strip: true,
        }
    }
}

const RECORDING_AVATAR_PX: u32 = 256;

const RECORDING_AVATAR_MAX_ATTEMPTS: u8 = 3;

const RECORDING_AVATAR_WARMUP: Duration = Duration::from_millis(1_500);

const RECORDING_AVATAR_WARMUP_STEP: Duration = Duration::from_millis(50);

#[derive(Default)]
struct RecordingAvatar {
    url: String,
    image: Option<Arc<mezon_voice::compose::AvatarImage>>,
    loading: bool,
    attempts: u8,
}

impl RecordingAvatar {
    fn claim(&mut self, url: &str) -> bool {
        if self.url != url {
            *self = Self {
                url: url.to_string(),
                ..Self::default()
            };
        }
        if self.loading || self.image.is_some() || self.attempts >= RECORDING_AVATAR_MAX_ATTEMPTS {
            return false;
        }
        self.loading = true;
        true
    }

    fn settle(&mut self, url: &str, image: Option<Arc<mezon_voice::compose::AvatarImage>>) {
        if self.url != url {
            return;
        }
        self.loading = false;
        match image {
            Some(image) => {
                self.image = Some(image);
                self.attempts = 0;
            }
            None => self.attempts += 1,
        }
    }
}

async fn load_avatar(url: &str) -> Option<Arc<mezon_voice::compose::AvatarImage>> {
    let (bytes, _) = mezon_client::transport_runtime::fetch_bytes(url)
        .await
        .map_err(|error| tracing::warn!("could not fetch a recording avatar: {error}"))
        .ok()?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|error| tracing::warn!("could not decode a recording avatar: {error}"))
        .ok()?
        .thumbnail(RECORDING_AVATAR_PX, RECORDING_AVATAR_PX)
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut bgra = decoded.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(mezon_voice::compose::AvatarImage {
        width,
        height,
        bgra,
    }))
}

fn solo_tile_for(
    fullscreen_screen: Option<u64>,
    member_strip_visible: bool,
    focused_tile: Option<&str>,
    participants: &[VoiceParticipant],
) -> Option<String> {
    if let Some(key) = fullscreen_screen {
        return participants
            .iter()
            .find(|p| p.screenshare == Some(key))
            .map(|p| screen_tile_id(&p.session_id));
    }
    if !member_strip_visible {
        return focused_tile.map(str::to_string);
    }
    None
}

fn initial_of(name: &str) -> String {
    name.chars()
        .next()
        .and_then(|c| c.to_uppercase().next())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string())
}

pub fn screen_tile_id(identity: &str) -> String {
    format!("{identity}\u{1}screen")
}

pub fn camera_tile_id(identity: &str) -> String {
    format!("{identity}\u{1}camera")
}

#[derive(Debug, PartialEq)]
enum ScreenAutoFocus {
    Keep,
    Clear,
    Focus(String),
}

fn default_focus_tile_for(participants: &[VoiceParticipant]) -> Option<String> {
    if let Some(p) = participants.iter().find(|p| p.screenshare.is_some()) {
        return Some(screen_tile_id(&p.session_id));
    }
    if let Some(p) = participants.iter().find(|p| p.camera.is_some()) {
        return Some(camera_tile_id(&p.session_id));
    }
    participants.first().map(|p| camera_tile_id(&p.session_id))
}

fn screen_auto_focus_transition(
    participants: &[VoiceParticipant],
    auto_focused: Option<&str>,
) -> ScreenAutoFocus {
    let auto_still_live = auto_focused.is_some_and(|id| {
        participants
            .iter()
            .any(|p| p.screenshare.is_some() && screen_tile_id(&p.session_id) == id)
    });
    if auto_still_live {
        return ScreenAutoFocus::Keep;
    }
    let next = participants
        .iter()
        .find(|p| p.screenshare.is_some())
        .map(|p| screen_tile_id(&p.session_id));
    match next {
        Some(id) => ScreenAutoFocus::Focus(id),
        None if auto_focused.is_some() => ScreenAutoFocus::Clear,
        None => ScreenAutoFocus::Keep,
    }
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
        let app_quit_subscription = cx.on_app_quit(|this, cx| {
            tracing::info!("tearing down voice session before app quit");
            this.teardown(None, cx);
            async {}
        });
        // The channel store announces a channel the server stopped listing
        // for us (refetch after a private flip, removal while we were away).
        let recording_announcement =
            cx.observe_self(|this, cx| this.sync_recording_announcement(cx));
        let channel_list_subscription = crate::ChannelList::try_global(cx).map(|channels| {
            cx.subscribe(&channels, |this, _, event, cx| {
                if let crate::ChannelEvent::AccessLost(channel_id) = event {
                    this.leave_lost_channel(&channel_id.to_string(), cx);
                }
            })
        });
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
            auto_focused_screen: None,
            fullscreen_screen: None,
            pip: None,
            room_name: String::new(),
            participant_menu: None,
            pending_kick: None,
            pending_removals: HashMap::new(),
            moderation_error: None,
            muted_by_moderator: false,
            agent_pending: false,
            participants: Vec::new(),
            join_ranks: Vec::new(),
            speak_ranks: HashMap::new(),
            speak_seq: 0,
            raised_hands: Vec::new(),
            raised_hand_timers: HashMap::new(),
            recording_users: Vec::new(),
            recording_user_timers: HashMap::new(),
            recording_announced: None,
            _recording_announce: None,
            raising_hand_player: None,
            raising_hand_sound_loading: false,
            join_voice_player: None,
            join_voice_sound_loading: false,
            give_flower_player: None,
            give_flower_sound_loading: false,
            join_sound_baseline_set: false,
            awaiting_room_snapshot: false,
            last_reaction_send: None,
            last_flower_send: None,
            last_flower_effect_at: None,
            shown_flowers: HashMap::new(),
            flower_seq: 0,
            active_sounds: HashMap::new(),
            sound_throttle: HashMap::new(),
            sound_cache: Vec::new(),
            sound_preview: None,
            displayed_reactions: Vec::new(),
            displayed_flowers: Vec::new(),
            reaction_seq: 0,
            last_emoji_at: None,
            session: None,
            session_generation: 0,
            reconnect_generation: 0,
            reconnect_token_fetches: 0,
            reconnect_attempts: 0,
            reconnect_healthy_since: None,
            frame_store: None,
            camera_devices: Vec::new(),
            device_menu: None,
            screen_share_mode: ScreenShareMode::default(),
            interactive_launches: HashMap::new(),
            active_interactive_apps: HashMap::new(),
            opened_interactive_apps: HashMap::new(),
            interactive_window_seq: 0,
            device_submenu: None,
            _camera_enum_task: None,
            render_cache: Mutex::new(HashMap::new()),
            pending_texture_drops: Mutex::new(Vec::new()),
            pending_texture_replaces: Mutex::new(Vec::new()),
            pending_texture_work: AtomicBool::new(false),
            cached_meet_token: None,
            role: SfuRole::Speaker,
            ptt_active: false,
            ptt_held: false,
            hold_to_talk: false,
            ptt_hint_dismissed: false,
            pending_join_role: SfuRole::Speaker,
            join_role_menu_open: false,
            meet_token_prefetching: None,
            last_screen_share: None,
            link_copied: false,
            recording: RecordingState::Idle,
            recording_elapsed: Duration::ZERO,
            recording_stalled: false,
            member_strip_visible: true,
            recording_avatars: HashMap::new(),
            #[cfg(debug_assertions)]
            simulated_participants: Vec::new(),
            _recording_tick: None,
            _recording_start: None,
            _events_task: None,
            _reconnect_watch_task: None,
            _link_copied_reset: None,
            _app_quit_subscription: app_quit_subscription,
            _channel_list_subscription: channel_list_subscription,
            _recording_announcement: recording_announcement,
        }
    }

    fn meet_token_metadata(clan_id: &str, cx: &App) -> String {
        let Some(account_store) = AccountStore::try_global(cx) else {
            return meet_token_metadata_from_candidates(&[], &[]);
        };
        let account = account_store.read(cx);
        let me = account.account.as_ref();
        let clan_id = clan_id.parse::<i64>().ok().map(ClanId);
        let profile = account
            .clan_profile
            .as_ref()
            .filter(|p| Some(p.clan_id) == clan_id);
        let member = clan_id.zip(me).and_then(|(clan_id, me)| {
            ClanMembersStore::try_global(cx)
                .and_then(|store| store.read(cx).member(clan_id, UserId(me.user_id)).cloned())
        });
        meet_token_metadata_from_candidates(
            &[
                profile.map(|p| p.nick_name.as_str()).unwrap_or_default(),
                member
                    .as_ref()
                    .map(|m| m.clan_nick.as_str())
                    .unwrap_or_default(),
                member
                    .as_ref()
                    .map(|m| m.user.display_name.as_str())
                    .unwrap_or_default(),
                me.map(|m| m.display_name.as_str()).unwrap_or_default(),
                member
                    .as_ref()
                    .map(|m| m.user.username.as_str())
                    .unwrap_or_default(),
                me.map(|m| m.username.as_str()).unwrap_or_default(),
            ],
            &[
                profile
                    .and_then(|p| p.avatar_url.as_deref())
                    .unwrap_or_default(),
                member
                    .as_ref()
                    .map(|m| m.clan_avatar.as_str())
                    .unwrap_or_default(),
                member
                    .as_ref()
                    .map(|m| m.user.avatar_url.as_str())
                    .unwrap_or_default(),
                me.and_then(|m| m.avatar_url.as_deref()).unwrap_or_default(),
            ],
        )
    }

    fn cached_token_for(&self, channel_id: &str, metadata: &str) -> Option<String> {
        let cached = self.cached_meet_token.as_ref()?;
        if cached.channel_id == channel_id
            && cached.metadata == metadata
            && cached.fetched_at.elapsed() < MEET_TOKEN_CACHE_TTL
        {
            return Some(cached.token.clone());
        }
        None
    }

    pub fn prefetch_meet_token(
        &mut self,
        channel_id: String,
        clan_id: String,
        cx: &mut Context<Self>,
    ) {
        let metadata = Self::meet_token_metadata(&clan_id, cx);
        if self.cached_token_for(&channel_id, &metadata).is_some() {
            return;
        }
        if self.meet_token_prefetching.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.meet_token_prefetching = Some(channel_id.clone());
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let token = api
                .generate_meet_token(&channel_id, &channel_id, &metadata)
                .await;
            let _ = this.update(cx, |this, _| {
                if this.meet_token_prefetching.as_deref() == Some(channel_id.as_str()) {
                    this.meet_token_prefetching = None;
                }
                if let Ok(token) = token {
                    this.cached_meet_token = Some(CachedMeetToken {
                        channel_id: channel_id.clone(),
                        metadata: metadata.clone(),
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

    pub fn join_rank(&self, session_id: &str) -> usize {
        self.join_ranks
            .iter()
            .position(|id| id == session_id)
            .unwrap_or(usize::MAX)
    }

    pub fn last_spoke_rank(&self, session_id: &str) -> u64 {
        self.speak_ranks.get(session_id).copied().unwrap_or(0)
    }

    fn track_visual_ranks(&mut self, list: &[VoiceParticipant]) {
        self.join_ranks
            .retain(|id| list.iter().any(|p| p.session_id == *id));
        for p in list {
            if !self.join_ranks.contains(&p.session_id) {
                self.join_ranks.push(p.session_id.clone());
            }
            let was_speaking = self
                .participants
                .iter()
                .any(|old| old.session_id == p.session_id && old.speaking);
            if p.speaking && !was_speaking {
                self.speak_seq += 1;
                self.speak_ranks
                    .insert(p.session_id.clone(), self.speak_seq);
            }
        }
        self.speak_ranks
            .retain(|id, _| list.iter().any(|p| p.session_id == *id));
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

    pub fn screen_share_mode(&self) -> ScreenShareMode {
        self.screen_share_mode
    }

    pub fn set_screen_share_mode(&mut self, mode: ScreenShareMode, cx: &mut Context<Self>) {
        self.device_menu = None;
        self.device_submenu = None;
        if self.screen_share_mode != mode {
            self.screen_share_mode = mode;
            if let Some(session) = &self.session {
                session.set_screen_share_mode(mode);
            }
        }
        cx.notify();
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
        cx.notify();
    }

    pub fn set_noise_suppression_level(&mut self, level: u8, cx: &mut Context<Self>) {
        let level = level.min(100);
        if self.noise_suppression_level == level {
            return;
        }
        self.noise_suppression_level = level;
        cx.notify();
    }

    pub fn frame_store(&self) -> Option<Arc<VideoFrameStore>> {
        self.frame_store.clone()
    }

    pub fn render_frame(&self, key: u64) -> Option<VoiceRenderFrame> {
        let store = self.frame_store.as_ref()?;
        let cached_seq = self.render_cache.lock().get(&key).map(|entry| entry.seq);
        let Some(frame) = store.take_new(key, cached_seq) else {
            return self
                .render_cache
                .lock()
                .get(&key)
                .map(|entry| entry.frame.clone());
        };
        let seq = frame.seq;
        #[cfg(target_os = "macos")]
        if let Some(surface) = frame.surface {
            let rendered = VoiceRenderFrame::Surface(surface);
            let previous = self.render_cache.lock().insert(
                key,
                CachedRenderFrame {
                    seq,
                    frame: rendered.clone(),
                },
            );
            if let Some(CachedRenderFrame {
                frame: VoiceRenderFrame::Image(image),
                ..
            }) = previous
            {
                self.pending_texture_drops.lock().push(image);
                self.pending_texture_work.store(true, Ordering::Release);
            }
            return Some(rendered);
        }
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
        let weak_store = Arc::downgrade(store);
        let recycler = Arc::new(move |buffer| {
            if let Some(store) = weak_store.upgrade() {
                store.recycle(key, buffer);
            }
        });
        let mut render_image = RenderImage::new_recyclable(image::Frame::new(buffer), recycler);
        #[cfg_attr(not(target_os = "macos"), allow(clippy::bind_instead_of_map))]
        let previous_id = self
            .render_cache
            .lock()
            .get(&key)
            .and_then(|entry| match &entry.frame {
                VoiceRenderFrame::Image(image) => Some(image.id),
                #[cfg(target_os = "macos")]
                VoiceRenderFrame::Surface(_) => None,
            });
        if let Some(id) = previous_id {
            render_image = render_image.with_id(id);
        }
        let image = Arc::new(render_image);
        let rendered = VoiceRenderFrame::Image(image.clone());
        let previous = self.render_cache.lock().insert(
            key,
            CachedRenderFrame {
                seq,
                frame: rendered.clone(),
            },
        );
        if previous_id.is_some() {
            let mut replaces = self.pending_texture_replaces.lock();
            if let Some(existing) = replaces.iter_mut().find(|queued| queued.id == image.id) {
                *existing = image.clone();
            } else {
                replaces.push(image.clone());
            }
            self.pending_texture_work.store(true, Ordering::Release);
        } else if let Some(CachedRenderFrame {
            frame: VoiceRenderFrame::Image(previous),
            ..
        }) = previous
        {
            self.pending_texture_drops.lock().push(previous);
            self.pending_texture_work.store(true, Ordering::Release);
        }
        Some(rendered)
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
        let previous_drop_count = drops.len();
        cache.retain(|key, entry| {
            if live_keys.contains(key) {
                true
            } else {
                match &entry.frame {
                    VoiceRenderFrame::Image(image) => drops.push(image.clone()),
                    #[cfg(target_os = "macos")]
                    VoiceRenderFrame::Surface(_) => {}
                }
                false
            }
        });
        if drops.len() != previous_drop_count {
            self.pending_texture_work.store(true, Ordering::Release);
        }
    }

    pub fn flush_texture_drops(&self, mut window: Option<&mut Window>, cx: &mut App) {
        if !self.pending_texture_work.swap(false, Ordering::AcqRel) {
            return;
        }
        let drops: Vec<Arc<RenderImage>> = std::mem::take(&mut *self.pending_texture_drops.lock());
        let replaces: Vec<Arc<RenderImage>> =
            std::mem::take(&mut *self.pending_texture_replaces.lock());
        let dropped: HashSet<_> = drops.iter().map(|image| image.id).collect();
        for image in drops {
            cx.drop_image(image, window.as_deref_mut());
        }
        for image in replaces {
            if dropped.contains(&image.id) {
                continue;
            }
            cx.update_render_image(&image, window.as_deref_mut());
        }
    }

    pub fn focused_tile(&self) -> Option<&str> {
        self.focused_tile.as_deref()
    }

    pub fn member_strip_visible(&self) -> bool {
        self.member_strip_visible
    }

    pub fn set_member_strip_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.member_strip_visible == visible {
            return;
        }
        self.member_strip_visible = visible;
        self.publish_recording_scene(cx);
        cx.notify();
    }

    pub fn toggle_member_strip(&mut self, cx: &mut Context<Self>) {
        self.set_member_strip_visible(!self.member_strip_visible, cx);
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
            dispatch.on(
                RealtimeKind::VoiceInteractive,
                &entity,
                |this, event, cx| this.handle_voice_interactive(event, cx),
            );
            dispatch.on(
                RealtimeKind::UserChannelRemoved,
                &entity,
                |this, event, cx| this.handle_access_lost(event, cx),
            );
            dispatch.on(RealtimeKind::ChannelDeleted, &entity, |this, event, cx| {
                this.handle_access_lost(event, cx)
            });
            dispatch.on(RealtimeKind::UserClanRemoved, &entity, |this, event, cx| {
                this.handle_clan_access_lost(event, cx)
            });
        });
    }

    /// Kicked out of (or having left) the clan the call runs in ends the
    /// call too — the room went with the clan.
    fn handle_clan_access_lost(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::UserClanRemoved(e) = event else {
            return;
        };
        let me = crate::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx));
        if !crate::event_targets_user(&e.user_ids, me) {
            return;
        }
        let Some((channel_id, clan_id)) = self.active_connection_ids() else {
            return;
        };
        if clan_id != e.clan_id.to_string() {
            return;
        }
        self.leave_lost_channel(&channel_id, cx);
    }

    /// Drop the call when the channel it runs in stops being ours: an admin
    /// removed us from a private voice room, or the room was deleted. The
    /// server only checks membership when it mints the meet token (a one
    /// minute JWT), so a session already inside the room would otherwise
    /// outlive the permission that let it in.
    fn handle_access_lost(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let channel_id = match event {
            RealtimeEvent::UserChannelRemoved(e) => {
                let me = crate::BadgeService::try_global(cx)
                    .and_then(|badges| badges.read(cx).current_user_id(cx));
                if !crate::event_targets_user(&e.user_ids, me) {
                    return;
                }
                e.channel_id
            }
            RealtimeEvent::ChannelDeleted(e) => e.channel_id,
            _ => return,
        };
        self.leave_lost_channel(&channel_id.to_string(), cx);
    }

    /// Tear the session down if it is connecting to or connected in
    /// `channel_id`; a call in any other room is left alone.
    pub fn leave_lost_channel(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        if self.connection.active_channel_id() != Some(channel_id) {
            return;
        }
        tracing::info!(channel_id, "leaving voice: access to the channel was lost");
        self.teardown(None, cx);
        cx.notify();
    }

    fn handle_voice_interactive(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::VoiceInteractive(event) = event else {
            return;
        };
        if event.event_type == VoiceInteractiveEventType::Gift as i32 {
            let Some((channel_id, _)) = self.connection.connected_channel() else {
                return;
            };
            let Ok(joined_channel) = channel_id.parse::<i64>() else {
                return;
            };
            let Some((giver_id, receiver_id, _, _)) = flower_event_from_payload(
                event.event_type,
                event.sender_id,
                event.receiver_id,
                event.voice_channel_id,
                &event.params,
                joined_channel,
            ) else {
                return;
            };
            self.show_flower_effect(giver_id, receiver_id, cx);
            return;
        }
        if event.event_type == VoiceInteractiveEventType::Recording as i32 {
            self.handle_recording_signal(event, cx);
            return;
        }
        let Some(app) = VoiceInteractiveApp::from_event_type(event.event_type) else {
            return;
        };
        if self
            .connection
            .active_channel_id()
            .and_then(|id| id.parse::<i64>().ok())
            != Some(event.voice_channel_id)
        {
            return;
        }
        self.handle_interactive_app_signal(app, event.sender_id, event.clan_id, cx);
    }

    fn handle_interactive_app_signal(
        &mut self,
        app: VoiceInteractiveApp,
        sender_id: i64,
        clan_id: i64,
        cx: &mut Context<Self>,
    ) {
        if self.opened_interactive_apps.contains_key(&app) {
            return;
        }
        let current_user_id = crate::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
            .map(|user_id| user_id.0);
        self.active_interactive_apps
            .insert(app, (sender_id, clan_id));
        if current_user_id == Some(sender_id) {
            self.open_interactive_app(app, sender_id, clan_id, cx);
        }
        cx.notify();
    }

    fn finish_interactive_launch(
        &mut self,
        app: VoiceInteractiveApp,
        launch_key: (VoiceInteractiveApp, u64),
        window_token: u64,
        cx: &mut Context<Self>,
    ) {
        self.interactive_launches.remove(&launch_key);
        release_interactive_window(&mut self.opened_interactive_apps, app, window_token);
        cx.notify();
    }

    fn open_interactive_app(
        &mut self,
        app: VoiceInteractiveApp,
        sender_id: i64,
        clan_id: i64,
        cx: &mut Context<Self>,
    ) {
        let fingerprint = interactive_app_fingerprint(sender_id, clan_id);
        let now = Instant::now();
        let launch_key = (app, fingerprint);
        if is_duplicate_interactive_launch(&mut self.interactive_launches, launch_key, now) {
            return;
        }
        self.interactive_launches.insert(launch_key, now);
        self.interactive_window_seq += 1;
        let window_token = self.interactive_window_seq;
        self.opened_interactive_apps.insert(app, window_token);
        let api = self.api.clone();
        let app_id = app.app_id();
        let config = AppConfig::global(cx);
        let base_url = match app {
            VoiceInteractiveApp::Quiz => config.quiz_url.clone(),
            VoiceInteractiveApp::Blackboard => config.blackboard_url.clone(),
            VoiceInteractiveApp::Interactive => config.interactive_url.clone(),
        };
        let clan_id_string = clan_id.to_string();
        let clan_list = crate::ClanList::try_global(cx);
        let clan_name = clan_list.as_ref().and_then(|clans| {
            clans
                .read(cx)
                .clan(ClanId(clan_id))
                .map(|clan| clan.name.clone())
        });
        cx.spawn(async move |this, cx| {
            let hash = match api.generate_hash_channel_apps(app_id).await {
                Ok(hash) if !hash.web_app_data.is_empty() => hash,
                Ok(_) => {
                    tracing::warn!(app_id, "voice app hash returned empty web_app_data");
                    let _ = this.update(cx, |store, cx| {
                        store.finish_interactive_launch(app, launch_key, window_token, cx);
                    });
                    return;
                }
                Err(error) => {
                    tracing::warn!(app_id, "generate voice app hash failed: {error:#}");
                    let _ = this.update(cx, |store, cx| {
                        store.finish_interactive_launch(app, launch_key, window_token, cx);
                    });
                    return;
                }
            };
            let mut url = build_channel_app_url(
                &base_url,
                ChannelAppLaunchParams {
                    web_app_data: &hash.web_app_data,
                    clan_id: &clan_id_string,
                    clan_name: clan_name.as_deref(),
                },
            );
            if app == VoiceInteractiveApp::Blackboard
                && let Ok(mut parsed) = url::Url::parse(&url)
            {
                parsed
                    .query_pairs_mut()
                    .append_pair("userId", &sender_id.to_string());
                url = parsed.to_string();
            }
            tracing::info!(
                url = %redact_interactive_app_url(&url),
                event_type = app.event_type() as i32,
                app_id,
                "opening built-in voice interactive app"
            );
            let opener = cx.update(|cx| {
                crate::PlatformStore::try_global(cx)
                    .and_then(|platform| platform.read(cx).managed_app_window_opener())
            });
            let Some(opener) = opener else {
                tracing::warn!("managed voice app launch skipped: no opener is registered");
                let _ = this.update(cx, |store, cx| {
                    store.finish_interactive_launch(app, launch_key, window_token, cx);
                });
                return;
            };
            let window_key = format!("voice-{}", app.app_id());
            let (closed_tx, closed_rx) = futures::channel::oneshot::channel();
            let waiter = std::thread::Builder::new()
                .name(format!("voice-app-window-{app_id}"))
                .spawn(move || {
                    let _ = closed_tx.send(opener(&url, &window_key));
                });
            let result = match waiter {
                Ok(_) => closed_rx.await.unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "voice app window thread ended without a result"
                    ))
                }),
                Err(error) => Err(error.into()),
            };
            if let Err(error) = result {
                tracing::warn!("managed voice app window failed: {error:#}");
            }
            let _ = this.update(cx, |store, cx| {
                store.finish_interactive_launch(app, launch_key, window_token, cx);
            });
        })
        .detach();
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
        if let Some(receiver_id) = parse_flower_reaction_token(token) {
            let receiver_id = receiver_id.to_string();
            self.show_flower_effect(msg.sender_id.to_string(), receiver_id, cx);
            return;
        }
        if let Some(app) = parse_app_reaction_token(token) {
            let clan_id = self
                .connection
                .connected_channel()
                .and_then(|(_, clan)| clan.parse::<i64>().ok());
            if let Some(clan_id) = clan_id {
                self.handle_interactive_app_signal(app, msg.sender_id, clan_id, cx);
            }
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

    pub fn recording_users(&self) -> &[String] {
        &self.recording_users
    }

    fn add_recording_user(&mut self, user_id: String, cx: &mut Context<Self>) {
        let inserted = if self.recording_users.contains(&user_id) {
            false
        } else {
            self.recording_users.push(user_id.clone());
            true
        };
        let key = user_id.clone();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(RECORDING_INDICATOR_TTL)
                .await;
            this.update(cx, |this, cx| this.remove_recording_user(&key, cx))
                .ok();
        });
        self.recording_user_timers.insert(user_id, timer);
        if inserted {
            cx.notify();
        }
    }

    fn remove_recording_user(&mut self, user_id: &str, cx: &mut Context<Self>) {
        let before = self.recording_users.len();
        self.recording_users.retain(|id| id != user_id);
        self.recording_user_timers.remove(user_id);
        if self.recording_users.len() != before {
            cx.notify();
        }
    }

    fn prune_recording_users(&mut self, cx: &mut Context<Self>) {
        if self.awaiting_room_snapshot || self.participants.is_empty() {
            return;
        }
        let gone: Vec<String> = self
            .recording_users
            .iter()
            .filter(|id| !self.participants.iter().any(|p| &p.identity == *id))
            .cloned()
            .collect();
        for user_id in gone {
            self.remove_recording_user(&user_id, cx);
        }
    }

    fn handle_recording_signal(
        &mut self,
        event: &mezon_proto::realtime::VoiceInteractiveEvent,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection
            .active_channel_id()
            .and_then(|id| id.parse::<i64>().ok())
            != Some(event.voice_channel_id)
        {
            return;
        }
        let Some(recording) = recording_from_params(&event.params) else {
            return;
        };
        tracing::debug!(
            sender_id = event.sender_id,
            recording,
            "voice recording signal"
        );
        let sender_id = event.sender_id.to_string();
        if !recording {
            self.remove_recording_user(&sender_id, cx);
        } else if self.local_user_id().as_deref() != Some(sender_id.as_str())
            || self.recording_announced.is_some()
        {
            self.add_recording_user(sender_id, cx);
        }
    }

    fn sync_recording_announcement(&mut self, cx: &mut Context<Self>) {
        let wanted = if self.recording == RecordingState::Recording {
            self.connection
                .connected_channel()
                .and_then(|(channel_id, clan_id)| {
                    Some((
                        clan_id.parse::<i64>().ok()?,
                        channel_id.parse::<i64>().ok()?,
                    ))
                })
                .zip(self.local_user_id())
                .map(|((clan_id, channel_id), user_id)| RecordingAnnouncement {
                    clan_id,
                    channel_id,
                    user_id,
                })
        } else {
            None
        };
        if wanted == self.recording_announced {
            return;
        }
        if let Some(stopped) = self.recording_announced.take() {
            self._recording_announce = None;
            self.remove_recording_user(&stopped.user_id, cx);
            let api = self.api.clone();
            cx.spawn(async move |_this, _cx| {
                write_recording_state(&api, &stopped, false).await;
            })
            .detach();
        }
        let Some(started) = wanted else {
            return;
        };
        self.recording_announced = Some(started.clone());
        let api = self.api.clone();
        self._recording_announce = Some(cx.spawn(async move |this, cx| {
            loop {
                let refreshed = this.update(cx, |this, cx| {
                    this.add_recording_user(started.user_id.clone(), cx);
                });
                if refreshed.is_err() {
                    break;
                }
                write_recording_state(&api, &started, true).await;
                cx.background_executor()
                    .timer(RECORDING_ANNOUNCE_INTERVAL)
                    .await;
            }
        }));
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
                .spawn(async move { mezon_audio::decode_audio(RAISE_HAND_SOUND.to_vec()) })
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

    fn play_flower_sound(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.give_flower_player {
            player.play();
            return;
        }
        if self.give_flower_sound_loading {
            return;
        }
        self.give_flower_sound_loading = true;
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(GIVE_FLOWER_SOUND.to_vec()) })
                .await;
            this.update(cx, |this, _| {
                this.give_flower_sound_loading = false;
                let Ok(pcm) = decoded else {
                    return;
                };
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                player.play();
                this.give_flower_player = Some(player);
            })
            .ok();
        })
        .detach();
    }

    fn play_join_sound(&mut self, cx: &mut Context<Self>) {
        tracing::info!(
            cached = self.join_voice_player.is_some(),
            "join sound requested"
        );
        if let Some(player) = &self.join_voice_player {
            player.play();
            return;
        }
        if self.join_voice_sound_loading {
            return;
        }
        self.join_voice_sound_loading = true;
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(JOIN_VOICE_SOUND.to_vec()) })
                .await;
            this.update(cx, |this, _| {
                this.join_voice_sound_loading = false;
                let Ok(pcm) = decoded else {
                    return;
                };
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                player.play();
                this.join_voice_player = Some(player);
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
                .spawn(async move { mezon_audio::decode_audio(bytes) })
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

    pub fn displayed_flowers(&self) -> &[DisplayedFlower] {
        &self.displayed_flowers
    }

    pub fn flower_caption(&self) -> Option<&SharedString> {
        self.displayed_flowers.last().map(|flower| &flower.caption)
    }

    fn show_flower_effect(
        &mut self,
        giver_id: String,
        receiver_id: String,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let pair = flower_pair_key(&giver_id, &receiver_id);
        if is_duplicate_flower(&mut self.shown_flowers, &pair, now) {
            return;
        }
        self.shown_flowers.insert(pair.clone(), now);
        self.flower_seq = self.flower_seq.wrapping_add(1);
        let key = format!("{pair}:{}", self.flower_seq);
        let play_sound = self
            .last_flower_effect_at
            .is_none_or(|last| now.duration_since(last) >= FLOWER_RATE_LIMIT);
        if play_sound {
            self.last_flower_effect_at = Some(now);
        }
        self.displayed_flowers.clear();
        let giver_name = self.resolve_flower_name(&giver_id, cx);
        let receiver_name = self.resolve_flower_name(&receiver_id, cx);
        let locale = current_locale(cx);
        let local_is_receiver = self.local_user_id().as_deref() == Some(receiver_id.as_str());
        let label = SharedString::from(if local_is_receiver {
            mezon_i18n::t(&locale, "channelVoice.giveFlowerReceived")
                .replace("{{giver}}", &giver_name)
        } else {
            mezon_i18n::t(&locale, "channelVoice.giveFlowerGiven")
                .replace("{{giver}}", &giver_name)
                .replace("{{receiver}}", &receiver_name)
        });
        let expire_key = key.clone();
        let remove_timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLOWER_SCENE_TTL).await;
            this.update(cx, |this, cx| {
                this.displayed_flowers
                    .retain(|flower| flower.key != expire_key);
                cx.notify();
            })
            .ok();
        });
        self.displayed_flowers.push(DisplayedFlower {
            key,
            started_at: Instant::now(),
            caption: label,
            _remove_timer: remove_timer,
        });
        if play_sound {
            self.play_flower_sound(cx);
        }
        cx.notify();
    }

    fn resolve_flower_name(&self, user_id: &str, cx: &App) -> String {
        let clan_id = self
            .connection
            .connected_channel()
            .and_then(|(_, clan)| clan.parse::<i64>().ok())
            .map(ClanId);
        let uid = user_id.parse::<i64>().ok().map(UserId);
        if let (Some(clan_id), Some(uid)) = (clan_id, uid)
            && let Some(store) = ClanMembersStore::try_global(cx)
            && let Some(member) = store.read(cx).member(clan_id, uid)
        {
            let name = member.name();
            if !name.is_empty() {
                return name.to_string();
            }
        }
        if let Some(uid) = uid
            && let Some(store) = UsersByUserStore::try_global(cx)
            && let Some(user) = store.read(cx).user(uid)
        {
            if !user.display_name.is_empty() {
                return user.display_name.clone();
            }
            if !user.username.is_empty() {
                return user.username.clone();
            }
        }
        if let Some(uid) = uid
            && let Some(account) = AccountStore::try_global(cx)
        {
            let account = account.read(cx);
            if account
                .account
                .as_ref()
                .is_some_and(|me| me.user_id == uid.get())
            {
                if let Some(clan_id) = clan_id
                    && let Some(profile) = account.clan_profile.as_ref()
                    && profile.clan_id == clan_id
                    && !profile.nick_name.is_empty()
                {
                    return profile.nick_name.clone();
                }
                if let Some(me) = account.account.as_ref() {
                    if !me.display_name.is_empty() {
                        return me.display_name.clone();
                    }
                    if !me.username.is_empty() {
                        return me.username.clone();
                    }
                }
            }
        }
        self.participants
            .iter()
            .find(|p| p.identity == user_id)
            .map(|p| p.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| user_id.to_string())
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

    pub fn cached_sound_duration(&self, url: &str) -> Option<f64> {
        self.sound_cache
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, pcm)| pcm.duration_secs())
    }

    pub fn sound_preview_timeline(&self, url: &str) -> Option<(f64, f64)> {
        let preview = self.sound_preview.as_ref().filter(|p| p.url == url)?;
        let player = preview._player.as_ref()?;
        Some((player.position_secs(), player.duration_secs()))
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
                _tick_task: None,
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
                .spawn(async move { mezon_audio::decode_audio(bytes) })
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
            _tick_task: None,
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
        let end_timer = cx.spawn({
            let key = key.clone();
            async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| this.clear_sound_preview(&key, cx))
                    .ok();
            }
        });
        if let Some(preview) = self.sound_preview.as_mut().filter(|p| p.url == url) {
            preview._player = player;
            preview._end_timer = Some(end_timer);
            preview._tick_task = Some(Self::spawn_sound_preview_tick(key, cx));
        }
    }

    fn spawn_sound_preview_tick(url: String, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let still_previewing = this
                    .update(cx, |this, cx| {
                        let active = this.previewing_sound() == Some(url.as_str());
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .ok()
                    .unwrap_or(false);
                if !still_previewing {
                    break;
                }
            }
        })
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
            .is_some_and(|id| id == screen_tile_id(&local.session_id));
        let fullscreen = self.fullscreen_screen == Some(screen_key);
        let pip = self.pip.as_ref().is_some_and(|p| p.key == screen_key);
        focused || fullscreen || pip
    }

    fn sync_screen_full_res(&self) {
        if let Some(session) = &self.session {
            session.set_screen_full_res(self.desired_screen_full_res());
        }
    }

    fn sync_screen_auto_focus(&mut self) {
        match screen_auto_focus_transition(&self.participants, self.auto_focused_screen.as_deref())
        {
            ScreenAutoFocus::Keep => {}
            ScreenAutoFocus::Clear => {
                self.focused_tile = None;
                self.auto_focused_screen = None;
            }
            ScreenAutoFocus::Focus(id) => {
                self.focused_tile = Some(id.clone());
                self.auto_focused_screen = Some(id);
            }
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

    pub fn toggle_layout_view(&mut self, cx: &mut Context<Self>) {
        if self.focused_tile.is_some() {
            self.clear_focus(cx);
        } else if let Some(id) = default_focus_tile_for(&self.participants) {
            self.set_focus(id, cx);
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
                .find(|p| p.screenshare.is_some() && screen_tile_id(&p.session_id) == focused)
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

    pub fn take_muted_by_moderator(&mut self) -> bool {
        std::mem::take(&mut self.muted_by_moderator)
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

    pub fn give_flower(&mut self, identity: String, cx: &mut Context<Self>) {
        self.close_participant_menu(cx);
        let Some(local_id) = self.local_user_id() else {
            return;
        };
        let Some((channel_id, clan_id)) = self
            .connection
            .connected_channel()
            .map(|(channel, clan)| (channel.to_string(), clan.to_string()))
        else {
            return;
        };
        let receiver_name = self
            .participants
            .iter()
            .find(|p| p.identity == identity)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| identity.clone());
        let Ok(receiver_id) = identity.parse::<UserId>() else {
            return;
        };
        let Ok(receiver_i64) = identity.parse::<i64>() else {
            return;
        };
        let Ok(giver_i64) = local_id.parse::<i64>() else {
            return;
        };
        let Ok(channel_i64) = channel_id.parse::<i64>() else {
            return;
        };
        let Ok(clan_i64) = clan_id.parse::<i64>() else {
            return;
        };

        let wallet = WalletStore::try_global(cx);
        let wallet_available = wallet
            .as_ref()
            .map(|store| store.read(cx).is_available())
            .unwrap_or(false);
        let pending = wallet
            .as_ref()
            .map(|store| store.read(cx).pending_give_flower())
            .unwrap_or(false);
        let balance = wallet
            .as_ref()
            .and_then(|store| store.read(cx).balance().map(str::to_string));
        let locale = current_locale(cx);

        match can_give_flower(
            identity == local_id,
            wallet_available,
            pending,
            self.last_flower_send,
            Instant::now(),
            balance.as_deref(),
        ) {
            Err(
                GiveFlowerDeny::SelfTarget
                | GiveFlowerDeny::Pending
                | GiveFlowerDeny::WalletUnavailable,
            ) => return,
            Err(GiveFlowerDeny::RateLimited) => {
                if let Some(wallet) = wallet {
                    let message =
                        mezon_i18n::t(&locale, "channelVoice.giveFlowerRateLimited").to_string();
                    wallet.update(cx, |_wallet, cx| {
                        cx.emit(WalletEvent::SendFailed { message });
                    });
                }
                return;
            }
            Err(GiveFlowerDeny::Insufficient) => {
                if let Some(wallet) = wallet {
                    let message =
                        mezon_i18n::t(&locale, "channelVoice.giveFlowerInsufficient").to_string();
                    wallet.update(cx, |_wallet, cx| {
                        cx.emit(WalletEvent::SendFailed { message });
                    });
                }
                return;
            }
            Ok(()) => {}
        }

        let Some(wallet) = wallet else {
            return;
        };
        let sender_username = AccountStore::global(cx)
            .read(cx)
            .account
            .as_ref()
            .map(|account| account.username.clone())
            .unwrap_or_default();
        let timestamp = mezon_client::server_now_secs() as i64 * 1000;
        let params = serialize_flower_interactive_params(&identity, timestamp);
        let reaction_token = flower_reaction_token(&identity);
        let gift_channel = channel_id.clone();
        let request = build_flower_transfer(
            local_id.clone(),
            sender_username,
            identity.clone(),
            clan_id,
            channel_id,
        );
        let card_text = format!(
            "{} {}₫ | {}",
            mezon_i18n::t(&locale, "token.tokensSent"),
            format_flower_amount(flower_price()),
            mezon_i18n::t(&locale, "token.giveFlowerAction"),
        );
        let flower_generation = wallet.read(cx).reset_generation();
        self.last_flower_send = Some(Instant::now());
        wallet.update(cx, |store, _| store.set_pending_give_flower(true));
        let task = wallet.update(cx, |store, cx| store.send_transaction(request, cx));
        let wallet_weak = wallet.downgrade();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            wallet_weak
                .update(cx, |_store, cx| match &result {
                    Ok(_) => cx.emit(WalletEvent::FlowerSent),
                    Err(message) if is_uncertain_transfer_error(message) => {
                        cx.emit(WalletEvent::FlowerUncertain);
                    }
                    Err(message) => cx.emit(WalletEvent::SendFailed {
                        message: message.clone(),
                    }),
                })
                .ok();
            if result.is_ok() {
                this.update(cx, |this, cx| {
                    let current_generation = wallet_weak
                        .upgrade()
                        .map(|store| store.read(cx).reset_generation());
                    if current_generation != Some(flower_generation) {
                        return;
                    }
                    let card = DirectMessageStore::global(cx).update(cx, |store, cx| {
                        store.create_dm_and_send_token_card(
                            receiver_id,
                            receiver_name,
                            card_text,
                            cx,
                        )
                    });
                    card.detach();
                    let still_in_room = this
                        .connection
                        .connected_channel()
                        .is_some_and(|(channel, _)| channel == gift_channel);
                    if still_in_room {
                        this.show_flower_effect(local_id, identity, cx);
                    }
                })
                .ok();
                if let Err(error) = api
                    .write_voice_interactive_event(
                        clan_i64,
                        channel_i64,
                        giver_i64,
                        receiver_i64,
                        VoiceInteractiveEventType::Gift as i32,
                        params,
                    )
                    .await
                {
                    tracing::warn!("write_voice_interactive_event failed: {error}");
                }
                if let Err(error) = api
                    .write_voice_reaction(vec![reaction_token], channel_i64)
                    .await
                {
                    tracing::warn!("flower voice reaction failed: {error}");
                }
            }
            wallet_weak
                .update(cx, |store, _| store.set_pending_give_flower(false))
                .ok();
        })
        .detach();
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
        self.pending_removals
            .insert(identity.clone(), Instant::now());
        self.participants.retain(|p| p.identity != identity);
        cx.notify();
        self.moderate_participant(identity, ModerationAction::Kick, cx);
    }

    pub fn agent_active(&self) -> bool {
        if self.connection.connected_channel().is_none() {
            return false;
        }
        self.participants.iter().any(|p| p.is_agent && !p.is_local)
    }

    pub fn toggle_agent(&mut self, cx: &mut Context<Self>) {
        if self.agent_pending {
            return;
        }
        let Some((channel_key, _clan_id)) = self.connection.connected_channel() else {
            return;
        };
        let Ok(channel_id) = channel_key.parse::<i64>() else {
            return;
        };
        if self.room_name.is_empty() {
            return;
        }
        let room_name = self.room_name.clone();
        let on_agent = self.agent_active();
        let api = self.api.clone();
        self.agent_pending = true;
        cx.spawn(async move |this, cx| {
            let result = if on_agent {
                api.disconnect_agent(channel_id, &room_name).await
            } else {
                api.add_agent_to_channel(channel_id, &room_name).await
            };
            if let Err(e) = &result {
                tracing::warn!("toggle agent failed: {e:#}");
            }
            let _ = this.update(cx, |this, cx| {
                this.agent_pending = false;
                if result.is_err() {
                    this.moderation_error = Some(VoiceModerationError::AgentFailed);
                }
                cx.notify();
            });
        })
        .detach();
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
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = match action {
                ModerationAction::Mute => {
                    api.mute_participant_mezon_meet(&channel_id, &clan_id, &identity)
                        .await
                }
                ModerationAction::Kick => {
                    api.remove_participant_mezon_meet(&channel_id, &clan_id, &identity)
                        .await
                }
            };
            let _ = this.update(cx, |this, cx| {
                let delivered = match result {
                    Ok(token) => this.deliver_participant_action(token),
                    Err(e) => {
                        tracing::warn!("participant moderation failed: {e:#}");
                        false
                    }
                };
                if delivered {
                    return;
                }
                if matches!(action, ModerationAction::Kick) {
                    this.pending_removals.remove(&identity);
                }
                this.moderation_error = Some(action.error());
                cx.notify();
            });
        })
        .detach();
    }

    fn deliver_participant_action(&self, token: String) -> bool {
        let Some(session) = &self.session else {
            tracing::warn!("participant moderation has no voice session to reach the sfu");
            return false;
        };
        if matches!(self.call_status, VoiceCallStatus::Reconnecting) {
            tracing::warn!("participant moderation skipped while the sfu link is reconnecting");
            return false;
        }
        session.participant_action(token);
        true
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
        role: SfuRole,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        camera_device_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_active_in(&channel_id) {
            return;
        }

        self.teardown(Some(window), cx);
        self.channel_label = channel_label;
        self.role = role;
        self.ptt_held = false;
        self.hold_to_talk = false;
        self.ptt_hint_dismissed = false;
        self.pending_join_role = role;
        self.join_role_menu_open = false;

        let ws_url = AppConfig::global(cx).sfu_ws_url.clone();
        if ws_url.is_empty() {
            let locale = current_locale(cx);
            self.connection = VoiceConnection::Failed {
                channel_id,
                message: mezon_i18n::t(&locale, "channelVoice.sfuNotConfigured").to_string(),
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
        self.join_ranks.clear();
        self.speak_ranks.clear();
        cx.notify();

        let api = self.api.clone();
        let metadata = Self::meet_token_metadata(&clan_id, cx);
        let cached_token = self.cached_token_for(&channel_id, &metadata);
        if let Some(token) = cached_token {
            self.start_session(
                ws_url,
                token,
                channel_id,
                false,
                input_device_id,
                output_device_id,
                camera_device_id,
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let token = api
                .generate_meet_token(&channel_id, &channel_id, &metadata)
                .await;
            let _ = this.update(cx, |this, cx| match token {
                Ok(token) => {
                    this.cached_meet_token = Some(CachedMeetToken {
                        channel_id: channel_id.clone(),
                        metadata: metadata.clone(),
                        token: token.clone(),
                        fetched_at: Instant::now(),
                    });
                    this.start_session(
                        ws_url,
                        token,
                        channel_id,
                        false,
                        input_device_id,
                        output_device_id,
                        camera_device_id,
                        cx,
                    );
                }
                Err(e) => {
                    tracing::error!("failed to generate meet token: {e:#}");
                    if this.connection.active_channel_id() != Some(channel_id.as_str()) {
                        return;
                    }
                    let locale = current_locale(cx);
                    this.connection = VoiceConnection::Failed {
                        channel_id,
                        message: voice_join_error_message(&e, &locale),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn start_session(
        &mut self,
        ws_url: String,
        token: String,
        channel_id: String,
        mic_enabled: bool,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        camera_device_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.connection.active_channel_id() != Some(channel_id.as_str()) {
            return;
        }

        self.flush_recording(cx);
        self.recording = RecordingState::Idle;
        self.recording_elapsed = Duration::ZERO;
        self.recording_stalled = false;
        self._recording_tick = None;
        self.session_generation = self.session_generation.wrapping_add(1);
        let session_generation = self.session_generation;
        self._events_task = None;
        self.session = None;
        self.awaiting_room_snapshot = true;
        let ice_servers = Self::ice_servers(cx);
        let local_user_id = AccountStore::try_global(cx)
            .and_then(|account| {
                account
                    .read(cx)
                    .account
                    .as_ref()
                    .map(|me| me.user_id.to_string())
            })
            .unwrap_or_default();
        let refresh_clan = self
            .active_connection_ids()
            .map(|(_, clan)| clan)
            .unwrap_or_default();
        let (metadata_tx, mut metadata_rx) =
            futures::channel::mpsc::unbounded::<futures::channel::oneshot::Sender<String>>();
        cx.spawn(async move |this, cx| {
            while let Some(reply) = metadata_rx.next().await {
                let Ok(metadata) =
                    this.update(cx, |_, cx| Self::meet_token_metadata(&refresh_clan, cx))
                else {
                    break;
                };
                let _ = reply.send(metadata);
            }
        })
        .detach();
        let refresh_api = self.api.clone();
        let refresh_channel = channel_id.clone();
        let session = VoiceSession::connect(VoiceConnectOptions {
            url: ws_url,
            token,
            room: channel_id.clone(),
            role: self.role,
            local_user_id,
            mic_enabled,
            input_device_id,
            output_device_id,
            camera_device_id,
            ice_servers,
            refresh_token: Some(TokenRefresher::new(move || {
                let api = refresh_api.clone();
                let channel_id = refresh_channel.clone();
                let metadata_tx = metadata_tx.clone();
                async move {
                    let (reply, receiver) = futures::channel::oneshot::channel();
                    metadata_tx.unbounded_send(reply).ok()?;
                    let metadata = receiver.await.ok()?;
                    match api
                        .generate_meet_token(&channel_id, &channel_id, &metadata)
                        .await
                    {
                        Ok(token) => Some(token),
                        Err(e) => {
                            tracing::warn!("voice token refresh failed: {e:#}");
                            None
                        }
                    }
                }
            })),
        });
        let events = session.events();
        self.frame_store = Some(session.frame_store());
        self.session = Some(session);

        let task = cx.spawn(async move |this, cx| {
            let mut pending: Option<VoiceEvent> = None;
            loop {
                let event = match pending.take() {
                    Some(event) => event,
                    None => match events.recv_async().await {
                        Ok(event) => event,
                        Err(_) => break,
                    },
                };
                let event = if matches!(event, VoiceEvent::Participants(_)) {
                    let mut latest = event;
                    loop {
                        match events.try_recv() {
                            Ok(VoiceEvent::Participants(participants)) => {
                                latest = VoiceEvent::Participants(participants);
                            }
                            Ok(other) => {
                                pending = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    latest
                } else {
                    event
                };
                if this
                    .update(cx, |this, cx| {
                        if this.session_generation == session_generation {
                            this.handle_engine_event(event, cx);
                        }
                    })
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

    fn active_connection_ids(&self) -> Option<(String, String)> {
        match &self.connection {
            VoiceConnection::Connecting {
                channel_id,
                clan_id,
            }
            | VoiceConnection::Connected {
                channel_id,
                clan_id,
            } => Some((channel_id.clone(), clan_id.clone())),
            VoiceConnection::Idle | VoiceConnection::Failed { .. } => None,
        }
    }

    fn configured_voice_devices(cx: &App) -> (Option<String>, Option<String>, Option<String>) {
        crate::Settings::try_global(cx)
            .map(|settings| {
                let settings = settings.read(cx);
                (
                    settings.input_device_id.clone(),
                    settings.output_device_id.clone(),
                    settings.camera_device_id.clone(),
                )
            })
            .unwrap_or_default()
    }

    fn reconnect_snapshot(&self, cx: &App) -> Option<VoiceReconnectSnapshot> {
        let (channel_id, clan_id) = self.active_connection_ids()?;
        let ws_url = AppConfig::global(cx).sfu_ws_url.clone();
        if ws_url.is_empty() {
            return None;
        }
        let (input_device_id, output_device_id, camera_device_id) =
            Self::configured_voice_devices(cx);
        let screen_share = self
            .screen_share_enabled
            .then(|| self.last_screen_share.clone())
            .flatten();
        if self.screen_share_enabled && screen_share.is_none() {
            tracing::warn!("voice reconnect cannot restore screen share without a saved target");
        }
        Some(VoiceReconnectSnapshot {
            metadata: Self::meet_token_metadata(&clan_id, cx),
            channel_id,
            clan_id,
            ws_url,
            input_device_id,
            output_device_id,
            camera_device_id,
            mic_enabled: self.mic_enabled,
            camera_enabled: self.camera_enabled,
            screen_share,
        })
    }

    fn reconnect_still_pending(&self, generation: u64) -> bool {
        self.reconnect_generation == generation
            && matches!(self.call_status, VoiceCallStatus::Reconnecting)
            && self.connection.active_channel_id().is_some()
    }

    fn arm_reconnect_watchdog(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.reconnect_generation = self.reconnect_generation.wrapping_add(1);
        let generation = self.reconnect_generation;
        self.schedule_reconnect_recovery(generation, delay, cx);
    }

    fn cancel_reconnect_watchdog(&mut self) {
        self.reconnect_generation = self.reconnect_generation.wrapping_add(1);
        self.reconnect_token_fetches = 0;
        self._reconnect_watch_task = None;
    }

    fn schedule_reconnect_recovery(
        &mut self,
        generation: u64,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let api = self.api.clone();
        self._reconnect_watch_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;

            let snapshot = match this.update(cx, |this, cx| {
                if !this.reconnect_still_pending(generation) {
                    return None;
                }
                if this.reconnect_token_fetches >= MEET_TOKEN_RETRY_LIMIT {
                    tracing::warn!(
                        fetches = this.reconnect_token_fetches,
                        "voice reconnect token retry limit reached; ending the call"
                    );
                    this.teardown(None, cx);
                    cx.notify();
                    return None;
                }
                let snapshot = this.reconnect_snapshot(cx)?;
                this.reconnect_token_fetches += 1;
                Some(snapshot)
            }) {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            let Some(snapshot) = snapshot else {
                return;
            };

            let token = api
                .generate_meet_token(
                    &snapshot.channel_id,
                    &snapshot.channel_id,
                    &snapshot.metadata,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.reconnect_still_pending(generation) {
                    return;
                }
                match token {
                    Ok(token) => {
                        tracing::info!(
                            "voice reconnect watchdog rebuilding SFU session for channel {}",
                            snapshot.channel_id
                        );
                        this.cached_meet_token = Some(CachedMeetToken {
                            channel_id: snapshot.channel_id.clone(),
                            metadata: snapshot.metadata.clone(),
                            token: token.clone(),
                            fetched_at: Instant::now(),
                        });
                        this.restart_sfu_session(generation, snapshot, token, cx);
                    }
                    Err(e) => {
                        tracing::warn!("voice reconnect token refresh failed: {e:#}");
                        this.schedule_reconnect_recovery(generation, RECONNECT_RETRY_DELAY, cx);
                    }
                }
            });
        }));
    }

    fn clear_session_handles(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        tracing::info!(
            status = ?self.call_status,
            generation = self.session_generation,
            had_session = self.session.is_some(),
            "voice session handles cleared"
        );
        self.flush_recording(cx);
        self.recording = RecordingState::Idle;
        self.recording_elapsed = Duration::ZERO;
        self.recording_stalled = false;
        self._recording_tick = None;
        self.session_generation = self.session_generation.wrapping_add(1);
        self._events_task = None;
        self.session = None;
        self.frame_store = None;
        #[cfg_attr(not(target_os = "macos"), allow(clippy::unnecessary_filter_map))]
        let stale: Vec<Arc<RenderImage>> = {
            let mut cache = self.render_cache.lock();
            cache
                .drain()
                .filter_map(|(_, entry)| match entry.frame {
                    VoiceRenderFrame::Image(image) => Some(image),
                    #[cfg(target_os = "macos")]
                    VoiceRenderFrame::Surface(_) => None,
                })
                .collect()
        };
        for image in stale {
            cx.drop_image(image, window.as_deref_mut());
        }
        self.flush_texture_drops(window, cx);
    }

    fn restart_sfu_session(
        &mut self,
        generation: u64,
        snapshot: VoiceReconnectSnapshot,
        token: String,
        cx: &mut Context<Self>,
    ) {
        if self.active_connection_ids()
            != Some((snapshot.channel_id.clone(), snapshot.clan_id.clone()))
        {
            return;
        }

        let mic_enabled = snapshot.mic_enabled && !mezon_voice::microphone_denied();
        self.mic_permission_denied = snapshot.mic_enabled && !mic_enabled;
        self.close_pip(cx);
        self.fullscreen_screen = None;
        self.clear_session_handles(None, cx);
        self.call_status = VoiceCallStatus::Reconnecting;

        self.start_session(
            snapshot.ws_url,
            token,
            snapshot.channel_id.clone(),
            mic_enabled,
            snapshot.input_device_id,
            snapshot.output_device_id,
            snapshot.camera_device_id,
            cx,
        );
        self.restore_media_after_reconnect(
            mic_enabled,
            snapshot.camera_enabled,
            snapshot.screen_share,
        );
        if self.is_audience()
            && self.ptt_held
            && !mezon_voice::microphone_denied()
            && let Some(session) = &self.session
        {
            session.set_push_to_talk(true);
        }
        if self.reconnect_still_pending(generation) {
            self.schedule_reconnect_recovery(generation, RECONNECT_STALL_TIMEOUT, cx);
        }
        cx.notify();
    }

    fn restore_media_after_reconnect(
        &mut self,
        mic_enabled: bool,
        camera_enabled: bool,
        screen_share: Option<(PickedScreen, bool)>,
    ) {
        self.mic_enabled = mic_enabled;
        self.camera_enabled = camera_enabled;
        self.screen_share_enabled = screen_share.is_some();
        self.last_screen_share = screen_share.clone();

        if let Some(session) = &self.session {
            session.set_mic_enabled(mic_enabled);
            session.set_camera_enabled(camera_enabled);
            if let Some((pick, share_audio)) = screen_share {
                session.start_screen_share(pick, share_audio, self.screen_share_mode);
            }
        }
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
                self.cancel_reconnect_watchdog();
                self.reconnect_healthy_since = Some(Instant::now());
                if self.connection.mark_connected() {
                    self.play_join_sound(cx);
                }
                self.call_status = VoiceCallStatus::Stable;
            }
            VoiceEvent::RoomSnapshot => {
                self.awaiting_room_snapshot = false;
                self.join_sound_baseline_set = true;
            }
            VoiceEvent::Reconnecting => {
                if self
                    .reconnect_healthy_since
                    .is_some_and(|since| since.elapsed() >= SFU_RECONNECT_HEALTHY_SESSION)
                {
                    self.reconnect_attempts = 0;
                }
                self.reconnect_healthy_since = None;
                if self.reconnect_attempts >= MAX_SFU_RECONNECT_ATTEMPTS {
                    self.handle_engine_event(
                        VoiceEvent::Disconnected {
                            reason: "reconnect attempts exhausted".into(),
                        },
                        cx,
                    );
                    return;
                }
                self.reconnect_attempts += 1;
                self.call_status = VoiceCallStatus::Reconnecting;
                self.awaiting_room_snapshot = true;
                self.arm_reconnect_watchdog(RECONNECT_STALL_TIMEOUT, cx);
            }
            VoiceEvent::Reconnected => {
                self.call_status = VoiceCallStatus::Stable;
                self.cancel_reconnect_watchdog();
                self.reconnect_healthy_since = Some(Instant::now());
                if self.connection.mark_connected() {
                    self.play_join_sound(cx);
                }
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
            VoiceEvent::RemovedFromChannel { cause, reason } => {
                tracing::info!(?cause, "removed from voice channel: {reason}");
                self.call_status = VoiceCallStatus::Stable;
                self.teardown(None, cx);
                cx.emit(VoiceStoreEvent::RemovedFromChannel(cause));
            }
            VoiceEvent::PushToTalkActive(active) => {
                self.mic_enabled = active;
                self.ptt_active = active;
                cx.notify();
            }
            VoiceEvent::MutedByModerator => {
                self.mic_enabled = false;
                self.muted_by_moderator = true;
            }
            VoiceEvent::DeviceResetToDefault { input } => {
                let kind = if input {
                    DeviceKind::AudioInput
                } else {
                    DeviceKind::AudioOutput
                };
                Self::persist_device(kind, None, cx);
                cx.notify();
            }
            VoiceEvent::InputDeviceChangeFailed {
                requested,
                retained,
                error,
            } => {
                tracing::warn!(?requested, ?retained, "microphone change failed: {error}");
                let selected = crate::Settings::try_global(cx)
                    .map(|settings| settings.read(cx).input_device_id.clone());
                if selected == Some(requested) {
                    Self::persist_device(DeviceKind::AudioInput, retained, cx);
                }
            }
            VoiceEvent::Participants(mut list) => {
                if let Some(config) = AppConfig::try_global(cx) {
                    for participant in &mut list {
                        participant.is_agent = config.is_voice_agent(&participant.identity);
                    }
                }
                let refresh_scene = self.recording == RecordingState::Recording;
                if !self.pending_removals.is_empty() {
                    let now = Instant::now();
                    self.pending_removals.retain(|identity, issued_at| {
                        list.iter().any(|p| &p.identity == identity)
                            && now.duration_since(*issued_at) < KICK_SUPPRESS_TIMEOUT
                    });
                    list.retain(|p| !self.pending_removals.contains_key(&p.identity));
                }
                self.apply_simulated_participants(&mut list);
                if self.participants == list {
                    return;
                }
                let settling = self.awaiting_room_snapshot;
                if settling {
                    self.awaiting_room_snapshot = !list.iter().any(|p| !p.is_local);
                }
                let remote_arrived = list.iter().any(|p| {
                    !p.is_local
                        && !p.is_agent
                        && !self
                            .participants
                            .iter()
                            .any(|old| old.identity == p.identity)
                });
                let remote_joined = !settling && self.join_sound_baseline_set && remote_arrived;
                if remote_arrived && !remote_joined {
                    tracing::info!(
                        settling,
                        baseline = self.join_sound_baseline_set,
                        "remote participant arrived; join sound suppressed"
                    );
                }
                self.join_sound_baseline_set = true;
                self.track_visual_ranks(&list);
                self.participants = list;
                self.prune_recording_users(cx);
                if refresh_scene {
                    self.publish_recording_scene(cx);
                }
                if remote_joined {
                    self.play_join_sound(cx);
                }
                if let Some(local) = self.participants.iter().find(|p| p.is_local) {
                    self.mic_enabled = !local.muted;
                    self.camera_enabled = local.camera.is_some();
                    self.screen_share_enabled = local.screenshare.is_some();
                }
                self.sync_screen_auto_focus();
                self.evict_stale_render_cache();
                self.flush_texture_drops(None, cx);
                self.prune_screen_targets(cx);
                self.sync_screen_full_res();
            }
            VoiceEvent::Disconnected { reason } => {
                tracing::info!("voice disconnected: {reason}");
                let was_connected = matches!(&self.connection, VoiceConnection::Connected { .. });
                let unjoined_channel = match &self.connection {
                    VoiceConnection::Connecting { channel_id, .. }
                    | VoiceConnection::Failed { channel_id, .. } => Some(channel_id.clone()),
                    _ => None,
                };
                self.teardown(None, cx);
                if was_connected && reason != "left" {
                    cx.emit(VoiceStoreEvent::RemovedFromChannel(
                        RemovalCause::Disconnected,
                    ));
                }
                if let Some(channel_id) = unjoined_channel {
                    let locale = current_locale(cx);
                    self.connection = VoiceConnection::Failed {
                        channel_id,
                        message: mezon_i18n::t(&locale, "channelVoice.joinFailed").to_string(),
                    };
                }
            }
            VoiceEvent::Error(message) => {
                tracing::warn!("voice error: {message}");
                if message.starts_with("camera:") {
                    self.camera_enabled = false;
                } else if message.starts_with("screen:") {
                    self.screen_share_enabled = false;
                    self.last_screen_share = None;
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
        if self.is_audience() {
            return;
        }
        self.hold_to_talk = false;
        self.set_mic_enabled(!self.mic_enabled, cx);
    }

    pub fn role(&self) -> SfuRole {
        self.role
    }

    pub fn pending_join_role(&self) -> SfuRole {
        self.pending_join_role
    }

    pub fn join_role_menu_open(&self) -> bool {
        self.join_role_menu_open
    }

    pub fn set_pending_join_role(&mut self, role: SfuRole, cx: &mut Context<Self>) {
        self.pending_join_role = role;
        self.join_role_menu_open = false;
        cx.notify();
    }

    pub fn toggle_join_role_menu(&mut self, cx: &mut Context<Self>) {
        self.join_role_menu_open = !self.join_role_menu_open;
        cx.notify();
    }

    pub fn close_join_role_menu(&mut self, cx: &mut Context<Self>) {
        if self.join_role_menu_open {
            self.join_role_menu_open = false;
            cx.notify();
        }
    }

    pub fn is_audience(&self) -> bool {
        self.role.is_audience()
    }

    pub fn push_to_talk_active(&self) -> bool {
        self.ptt_active
    }

    pub fn ptt_hint_dismissed(&self) -> bool {
        self.ptt_hint_dismissed
    }

    pub fn dismiss_ptt_hint(&mut self, cx: &mut Context<Self>) {
        if !self.ptt_hint_dismissed {
            self.ptt_hint_dismissed = true;
            cx.notify();
        }
    }

    pub fn set_push_to_talk(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.ptt_held == active {
            return;
        }
        if !self.is_audience() {
            self.set_hold_to_talk(active, cx);
            return;
        }
        if active && mezon_voice::microphone_denied() {
            self.mic_permission_denied = true;
            cx.notify();
            return;
        }
        self.mic_permission_denied = false;
        self.ptt_held = active;
        if let Some(session) = &self.session {
            session.set_push_to_talk(active);
        }
        if !active {
            self.ptt_active = false;
            self.mic_enabled = false;
        }
        cx.notify();
    }

    fn set_hold_to_talk(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.session.is_none() {
            return;
        }
        self.ptt_held = active;
        if active {
            if self.mic_enabled {
                return;
            }
            self.set_mic_enabled(true, cx);
            self.hold_to_talk = self.mic_enabled;
        } else if self.hold_to_talk {
            self.hold_to_talk = false;
            self.set_mic_enabled(false, cx);
        }
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

    pub fn set_input_device(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        Self::persist_device(DeviceKind::AudioInput, device_id.clone(), cx);
        if let Some(session) = &self.session {
            session.set_input_device(device_id);
        }
        self.device_menu = None;
        self.device_submenu = None;
        cx.notify();
    }

    pub fn set_output_device(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        Self::persist_device(DeviceKind::AudioOutput, device_id.clone(), cx);
        if let Some(session) = &self.session {
            session.set_output_device(device_id);
        }
        self.device_menu = None;
        self.device_submenu = None;
        cx.notify();
    }

    pub fn set_camera_device(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        Self::persist_device(DeviceKind::VideoInput, device_id.clone(), cx);
        if let Some(session) = &self.session {
            session.set_camera_device(device_id);
        }
        self.device_menu = None;
        self.device_submenu = None;
        cx.notify();
    }

    fn persist_device(kind: DeviceKind, device_id: Option<String>, cx: &mut Context<Self>) {
        let Some(settings) = crate::Settings::try_global(cx) else {
            return;
        };
        settings.update(cx, |s, _| match kind {
            DeviceKind::AudioInput => s.input_device_id = device_id,
            DeviceKind::AudioOutput => s.output_device_id = device_id,
            DeviceKind::VideoInput => s.camera_device_id = device_id,
        });
        crate::schedule_settings_save(&settings, cx);
    }

    pub fn device_menu(&self) -> Option<DeviceMenuKind> {
        self.device_menu
    }

    pub fn request_interactive_app(&mut self, app: VoiceInteractiveApp, cx: &mut Context<Self>) {
        if self.opened_interactive_apps.contains_key(&app) {
            return;
        }
        let Some((voice_channel_id, clan_id)) = self.connection.connected_channel() else {
            return;
        };
        let (Ok(voice_channel_id), Ok(clan_id)) =
            (voice_channel_id.parse::<i64>(), clan_id.parse::<i64>())
        else {
            return;
        };
        let Some(user_id) = crate::BadgeService::try_global(cx)
            .and_then(|badges| badges.read(cx).current_user_id(cx))
            .map(|user_id| user_id.0)
        else {
            tracing::warn!("VoiceInteractiveEvent skipped: current user id is unavailable");
            return;
        };
        let fingerprint = interactive_app_fingerprint(user_id, clan_id);
        self.interactive_launches.remove(&(app, fingerprint));
        let api = self.api.clone();
        let params = if app == VoiceInteractiveApp::Blackboard {
            format!("userId={user_id}")
        } else {
            String::new()
        };
        cx.spawn(async move |_this, _cx| {
            match api
                .write_voice_interactive_event(
                    clan_id,
                    voice_channel_id,
                    user_id,
                    user_id,
                    app.event_type() as i32,
                    params,
                )
                .await
            {
                Ok(Some(_event)) => {
                    tracing::info!(
                        clan_id,
                        voice_channel_id,
                        sender_id = user_id,
                        receiver_id = user_id,
                        event_type = app.event_type() as i32,
                        "VoiceInteractiveEvent acknowledged with CID payload"
                    );
                }
                Ok(None) => tracing::info!(
                    clan_id,
                    voice_channel_id,
                    event_type = app.event_type() as i32,
                    "VoiceInteractiveEvent acknowledged; waiting for realtime event"
                ),
                Err(error) => {
                    tracing::warn!("write_voice_interactive_event failed: {error:#}");
                }
            }
            if let Err(error) = api
                .write_voice_reaction(vec![app_reaction_token(app)], voice_channel_id)
                .await
            {
                tracing::warn!("interactive app voice reaction failed: {error:#}");
            }
        })
        .detach();
    }

    pub fn has_active_interactive_apps(&self) -> bool {
        !self.active_interactive_apps.is_empty()
    }

    pub fn has_opened_interactive_apps(&self) -> bool {
        !self.opened_interactive_apps.is_empty()
    }

    pub fn is_interactive_app_active(&self, app: VoiceInteractiveApp) -> bool {
        self.active_interactive_apps.contains_key(&app)
    }

    pub fn is_interactive_app_opened(&self, app: VoiceInteractiveApp) -> bool {
        self.opened_interactive_apps.contains_key(&app)
    }

    pub fn join_interactive_app(&mut self, app: VoiceInteractiveApp, cx: &mut Context<Self>) {
        if self.opened_interactive_apps.contains_key(&app) {
            return;
        }
        let Some(&(sender_id, clan_id)) = self.active_interactive_apps.get(&app) else {
            return;
        };
        let fingerprint = interactive_app_fingerprint(sender_id, clan_id);
        self.interactive_launches.remove(&(app, fingerprint));
        self.open_interactive_app(app, sender_id, clan_id, cx);
        cx.notify();
    }

    pub fn device_submenu(&self) -> Option<DeviceKind> {
        self.device_submenu
    }

    pub fn camera_devices(&self) -> &[CameraDeviceInfo] {
        &self.camera_devices
    }

    pub fn toggle_device_menu(&mut self, kind: DeviceMenuKind, cx: &mut Context<Self>) {
        if self.device_menu == Some(kind) {
            self.device_menu = None;
            self.device_submenu = None;
        } else {
            self.device_menu = Some(kind);
            self.device_submenu = None;
            if kind != DeviceMenuKind::ScreenShare {
                self.refresh_devices(cx);
            }
        }
        cx.notify();
    }

    pub fn close_device_menu(&mut self, cx: &mut Context<Self>) {
        if self.device_menu.is_some() || self.device_submenu.is_some() {
            self.device_menu = None;
            self.device_submenu = None;
            cx.notify();
        }
    }

    pub fn set_device_submenu(&mut self, submenu: Option<DeviceKind>, cx: &mut Context<Self>) {
        if self.device_submenu != submenu {
            self.device_submenu = submenu;
            cx.notify();
        }
    }

    fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        if let Some(audio_store) = crate::AudioStore::try_global(cx) {
            crate::AudioStore::refresh_devices(&audio_store, cx);
        }
        self.refresh_cameras(cx);
    }

    fn refresh_cameras(&mut self, cx: &mut Context<Self>) {
        self._camera_enum_task = Some(cx.spawn(async move |this, cx| {
            let devices = cx
                .background_executor()
                .spawn(async move { mezon_voice::enumerate_cameras() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.camera_devices = devices;
                cx.notify();
            });
        }));
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
        self.last_screen_share = Some((pick.clone(), share_audio));
        if let Some(session) = &self.session {
            session.start_screen_share(pick, share_audio, self.screen_share_mode);
        }
        cx.notify();
    }

    pub fn recording_state(&self) -> RecordingState {
        self.recording
    }

    pub fn recording_elapsed(&self) -> Duration {
        self.recording_elapsed
    }

    pub fn recording_stalled(&self) -> bool {
        self.recording_stalled
    }

    pub fn can_record(&self) -> bool {
        self.session.is_some() && mezon_voice::record_supported()
    }

    pub fn toggle_recording(&mut self, window_id: Option<u64>, cx: &mut Context<Self>) {
        match self.recording {
            RecordingState::Idle => self.start_recording(window_id, cx),
            RecordingState::Recording => self.stop_recording(cx),
            // Nothing to do, but a button that looks dead is worth a line in the
            // log — a finalize that never returns lands here every time.
            state => tracing::warn!("ignoring the record button while the recorder is {state:?}"),
        }
    }

    pub fn suggested_recording_path(&self) -> std::path::PathBuf {
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        directory.join(format!(
            "mezon-call-{}.{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            mezon_voice::record_file_extension()
        ))
    }

    pub fn start_recording_at(
        &mut self,
        path: std::path::PathBuf,
        window_id: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.recording != RecordingState::Idle {
            return Err("a recording is already running".into());
        }
        let Some((scene, starter)) = self.session.as_ref().map(|session| {
            (
                Some(session.record_frame_source()),
                session.record_starter(),
            )
        }) else {
            return Err("not in a call".into());
        };
        let _ = window_id;
        self.fetch_missing_avatars(cx);
        self.publish_recording_scene(cx);
        let generation = self.session_generation;
        self.recording = RecordingState::Starting;
        cx.notify();

        self._recording_start = Some(cx.spawn(async move |this, cx| {
            let deadline = Instant::now() + RECORDING_AVATAR_WARMUP;
            while Instant::now() < deadline {
                let loading = this
                    .update(cx, |this, _| this.recording_avatars_loading())
                    .unwrap_or(false);
                if !loading {
                    break;
                }
                cx.background_executor()
                    .timer(RECORDING_AVATAR_WARMUP_STEP)
                    .await;
            }
            let still_starting = this
                .update(cx, |this, cx| {
                    if this.session_generation != generation
                        || this.recording != RecordingState::Starting
                    {
                        return false;
                    }
                    this.publish_recording_scene(cx);
                    true
                })
                .unwrap_or(false);
            if !still_starting {
                return;
            }

            let started = cx
                .background_executor()
                .spawn(async move { starter.start(path, scene) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.session_generation != generation {
                    return;
                }
                match started {
                    Ok(()) => {
                        this.recording = RecordingState::Recording;
                        this.recording_elapsed = Duration::ZERO;
                        this.recording_stalled = false;
                        this.start_recording_tick(cx);
                    }
                    Err(error) => {
                        tracing::error!("could not start the call recording: {error}");
                        this.recording = RecordingState::Idle;
                        cx.emit(VoiceStoreEvent::RecordingFinished(RecordingToast::Failed(
                            error,
                        )));
                    }
                }
                cx.notify();
            });
        }));
        Ok(())
    }

    pub fn request_stop_recording(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.recording != RecordingState::Recording {
            return Err("no recording is running".into());
        }
        self.stop_recording(cx);
        Ok(())
    }

    fn start_recording(&mut self, window_id: Option<u64>, cx: &mut Context<Self>) {
        if self.recording != RecordingState::Idle || self.session.is_none() {
            return;
        }
        let default_path = self.suggested_recording_path();
        let directory = default_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let suggested = default_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let receiver = cx.prompt_for_new_path(&directory, Some(suggested.as_str()));
        let fallback_directory = directory.clone();
        let fallback_name = suggested.clone();

        self.recording = RecordingState::Starting;
        let generation = self.session_generation;
        cx.notify();

        self._recording_start = Some(cx.spawn(async move |this, cx| {
            let path = match crate::dialog::classify_dialog(receiver.await) {
                crate::dialog::DialogOutcome::Picked(path) => path,
                crate::dialog::DialogOutcome::Cancelled => {
                    tracing::info!("the recording save dialog was cancelled");
                    let _ = this.update(cx, |this, cx| {
                        this.recording = RecordingState::Idle;
                        cx.notify();
                    });
                    return;
                }
                crate::dialog::DialogOutcome::Lost => {
                    tracing::warn!("the recording save dialog went away before answering");
                    let _ = this.update(cx, |this, cx| {
                        this.recording = RecordingState::Idle;
                        cx.notify();
                    });
                    return;
                }
                crate::dialog::DialogOutcome::Unavailable(failure) => {
                    tracing::warn!(
                        "could not ask where to save the recording: {}",
                        failure.reason
                    );
                    // Being asked is the convenience; the recording is the point. Keep
                    // it under the name we suggested so a machine with no working file
                    // dialog can still record.
                    let reserved = cx
                        .background_executor()
                        .spawn(async move {
                            mezon_client::reserve_path_in(&fallback_directory, &fallback_name)
                        })
                        .await;
                    match reserved {
                        Ok(path) => path,
                        Err(error) => {
                            tracing::error!("could not reserve a path for the recording: {error}");
                            let _ = this.update(cx, |this, cx| {
                                let locale = current_locale(cx);
                                this.recording = RecordingState::Idle;
                                cx.emit(VoiceStoreEvent::RecordingFinished(
                                    RecordingToast::Failed(
                                        mezon_i18n::t(&locale, "file.dialogUnavailable")
                                            .to_string(),
                                    ),
                                ));
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
            };
            let _ = this.update(cx, |this, cx| {
                if this.session_generation != generation {
                    tracing::info!("dropping a save dialog that outlived its call");
                    return;
                }
                this.recording = RecordingState::Idle;
                let _ = this.start_recording_at(path, window_id, cx);
            });
        }));
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording != RecordingState::Recording {
            return;
        }
        let Some(session) = self.session.as_ref().and_then(|s| s.take_recording()) else {
            self.recording = RecordingState::Idle;
            self._recording_tick = None;
            cx.notify();
            return;
        };
        self.recording = RecordingState::Stopping;
        self._recording_tick = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { session.finish() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.recording = RecordingState::Idle;
                this.recording_elapsed = Duration::ZERO;
                this.recording_stalled = false;
                cx.emit(VoiceStoreEvent::RecordingFinished(match result {
                    Ok(path) => RecordingToast::Saved(path),
                    Err(error) => {
                        tracing::error!("could not finish the call recording: {error}");
                        RecordingToast::Failed(error)
                    }
                }));
                cx.notify();
            });
        })
        .detach();
    }

    fn recording_avatars_loading(&self) -> bool {
        self.recording_avatars.values().any(|entry| entry.loading)
    }

    #[cfg(not(debug_assertions))]
    fn apply_simulated_participants(&self, _list: &mut Vec<VoiceParticipant>) {}

    #[cfg(debug_assertions)]
    fn apply_simulated_participants(&self, list: &mut Vec<VoiceParticipant>) {
        for fake in &self.simulated_participants {
            if !list.iter().any(|p| p.identity == fake.identity) {
                list.push(fake.clone());
            }
        }
    }

    #[cfg(debug_assertions)]
    pub fn simulate_participants(&mut self, count: usize, cx: &mut Context<Self>) -> usize {
        self.simulate_call(count, &SimulatedCall::default(), cx)
    }

    #[cfg(debug_assertions)]
    pub fn simulate_call(
        &mut self,
        count: usize,
        options: &SimulatedCall,
        cx: &mut Context<Self>,
    ) -> usize {
        let previous: Vec<String> = self
            .simulated_participants
            .iter()
            .map(|p| p.identity.clone())
            .collect();
        self.simulated_participants.clear();

        if count > 0 {
            let taken: HashSet<String> = self
                .participants
                .iter()
                .filter(|p| !previous.contains(&p.identity))
                .map(|p| p.identity.clone())
                .collect();
            let clan_id = self
                .connection
                .connected_channel()
                .and_then(|(_, clan)| clan.parse::<i64>().ok());
            let mut pool: Vec<(String, String)> = Vec::new();
            if let Some(clan_id) = clan_id
                && let Some(store) = ClanMembersStore::try_global(cx)
            {
                for member in store.read(cx).members(ClanId(clan_id)) {
                    let identity = member.id().0.to_string();
                    if taken.contains(&identity) {
                        continue;
                    }
                    pool.push((identity, member.name().to_string()));
                    if pool.len() == count {
                        break;
                    }
                }
            }
            for index in pool.len()..count {
                pool.push((
                    format!("9000000000000000{index:03}"),
                    format!("test.user{index}"),
                ));
            }

            self.simulated_participants = pool
                .into_iter()
                .enumerate()
                .map(|(index, (identity, name))| VoiceParticipant {
                    session_id: identity.clone(),
                    screenshare: (index == 0 && options.screenshare)
                        .then_some(SIMULATED_SCREEN_KEY),
                    identity,
                    name,
                    is_local: false,
                    is_agent: false,
                    is_audience: false,
                    speaking: index == 0,
                    muted: index % 3 == 2,
                    camera: None,
                    quality: NetworkQuality::Excellent,
                })
                .collect();
        }

        let mut list = self.participants.clone();
        list.retain(|p| !previous.contains(&p.identity));
        self.apply_simulated_participants(&mut list);
        self.track_visual_ranks(&list);
        self.participants = list;

        let screen_owner = self
            .simulated_participants
            .first()
            .filter(|_| options.screenshare)
            .map(|p| p.session_id.clone());
        self.focused_tile = match (&screen_owner, options.focus) {
            (Some(identity), true) => Some(screen_tile_id(identity)),
            _ => None,
        };
        self.fullscreen_screen =
            (options.fullscreen && screen_owner.is_some()).then_some(SIMULATED_SCREEN_KEY);
        self.member_strip_visible = options.member_strip;

        self.fetch_missing_avatars(cx);
        self.publish_recording_scene(cx);
        cx.notify();
        self.simulated_participants.len()
    }

    fn fetch_missing_avatars(&mut self, cx: &mut Context<Self>) {
        let Some((_, clan)) = self.connection.connected_channel() else {
            return;
        };
        let Ok(clan_id) = clan.parse::<i64>() else {
            return;
        };
        let members = ClanMembersStore::try_global(cx);
        let mut wanted: Vec<(String, String)> = Vec::new();
        for participant in &self.participants {
            let Ok(uid) = participant.identity.parse::<i64>() else {
                continue;
            };
            let Some(source) = members.as_ref().and_then(|store| {
                store
                    .read(cx)
                    .member(ClanId(clan_id), UserId(uid))
                    .map(|m| m.avatar().to_string())
            }) else {
                continue;
            };
            if source.is_empty() {
                continue;
            }
            let url = AppConfig::try_global(cx)
                .map(|cfg| {
                    cfg.imgproxy_url(&source, RECORDING_AVATAR_PX, RECORDING_AVATAR_PX, "fit")
                })
                .unwrap_or(source);
            if self
                .recording_avatars
                .entry(participant.identity.clone())
                .or_default()
                .claim(&url)
            {
                wanted.push((participant.identity.clone(), url));
            }
        }

        for (identity, url) in wanted {
            cx.spawn(async move |this, cx| {
                let fetch_url = url.clone();
                let decoded = cx
                    .background_executor()
                    .spawn(async move { load_avatar(&fetch_url).await })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let Some(entry) = this.recording_avatars.get_mut(&identity) else {
                        return;
                    };
                    entry.settle(&url, decoded);
                    this.publish_recording_scene(cx);
                });
            })
            .detach();
        }
    }

    fn solo_recording_tile(&self) -> Option<String> {
        solo_tile_for(
            self.fullscreen_screen,
            self.member_strip_visible,
            self.focused_tile.as_deref(),
            &self.participants,
        )
    }

    fn fullscreen_recording_tile(&self) -> Option<String> {
        let key = self.fullscreen_screen?;
        self.participants
            .iter()
            .find(|p| p.screenshare == Some(key))
            .map(|p| screen_tile_id(&p.session_id))
    }

    fn publish_recording_scene(&self, cx: &App) {
        let Some(session) = &self.session else {
            return;
        };
        let focused = self.focused_tile();
        let solo = self.solo_recording_tile();
        let fullscreen = self.fullscreen_recording_tile();
        let mut tiles = Vec::new();
        for participant in &self.participants {
            let label = self.display_name_for(participant, cx);
            let avatar = self
                .recording_avatars
                .get(&participant.identity)
                .and_then(|entry| entry.image.clone());
            if let Some(key) = participant.screenshare {
                let id = screen_tile_id(&participant.session_id);
                let solo_tile = solo.as_deref() == Some(id.as_str());
                if solo.is_none() || solo_tile {
                    let is_fullscreen = fullscreen.as_deref() == Some(id.as_str());
                    let label = if is_fullscreen {
                        String::new()
                    } else {
                        label.clone()
                    };
                    tiles.push(mezon_voice::compose::SceneTile {
                        focused: solo_tile || focused == Some(id.as_str()),
                        fullscreen: is_fullscreen,
                        key: id,
                        initial: initial_of(&label),
                        label,
                        avatar: avatar.clone(),
                        frame_key: Some(key),
                        is_screen_share: true,
                        speaking: false,
                        muted: participant.muted,
                    });
                }
            }
            let id = camera_tile_id(&participant.session_id);
            let solo_tile = solo.as_deref() == Some(id.as_str());
            if solo.is_some() && !solo_tile {
                continue;
            }
            tiles.push(mezon_voice::compose::SceneTile {
                focused: solo_tile || focused == Some(id.as_str()),
                fullscreen: false,
                key: id,
                initial: initial_of(&label),
                label,
                avatar,
                frame_key: participant.camera,
                is_screen_share: false,
                speaking: participant.speaking,
                muted: participant.muted,
            });
        }
        session.record_scene().set(tiles);
    }

    fn start_recording_tick(&mut self, cx: &mut Context<Self>) {
        self._recording_tick = Some(cx.spawn(async move |this, cx| {
            let mut reported_video_gap = false;
            loop {
                cx.background_executor()
                    .timer(RECORDING_TICK_INTERVAL)
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let Some(stats) = this.session.as_ref().and_then(|s| s.recording_stats())
                        else {
                            return false;
                        };
                        if this.recording != RecordingState::Recording {
                            return false;
                        }
                        if !reported_video_gap
                            && this
                                .session
                                .as_ref()
                                .is_some_and(|s| s.recording_video_unavailable())
                        {
                            reported_video_gap = true;
                            cx.emit(VoiceStoreEvent::RecordingVideoUnavailable);
                        }
                        if this
                            .session
                            .as_ref()
                            .is_some_and(|session| session.recording_failed())
                        {
                            tracing::error!("the call recorder failed mid-recording; stopping");
                            this.stop_recording(cx);
                            return false;
                        }
                        this.fetch_missing_avatars(cx);
                        this.publish_recording_scene(cx);
                        if this.recording_elapsed != stats.elapsed
                            || this.recording_stalled != stats.video_stalled
                        {
                            this.recording_elapsed = stats.elapsed;
                            this.recording_stalled = stats.video_stalled;
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        }));
    }

    fn flush_recording(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.as_ref().and_then(|s| s.take_recording()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { session.finish() })
                .await;
            let _ = this.update(cx, |_this, cx| {
                cx.emit(VoiceStoreEvent::RecordingFinished(match result {
                    Ok(path) => RecordingToast::Saved(path),
                    Err(error) => {
                        tracing::error!("could not finish the call recording on leave: {error}");
                        RecordingToast::Failed(error)
                    }
                }));
                cx.notify();
            });
        })
        .detach();
    }

    pub fn stop_screen_share(&mut self, cx: &mut Context<Self>) {
        if !self.screen_share_enabled {
            return;
        }
        self.last_screen_share = None;
        if let Some(session) = &self.session {
            session.stop_screen_share();
        }
        cx.notify();
    }

    fn teardown(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.cancel_reconnect_watchdog();
        self.reconnect_attempts = 0;
        self.reconnect_healthy_since = None;
        self.close_pip(cx);
        self.fullscreen_screen = None;
        self.member_strip_visible = true;
        self.clear_session_handles(window, cx);
        self.connection = VoiceConnection::Idle;
        self.call_status = VoiceCallStatus::Stable;
        self.channel_label.clear();
        self.mic_enabled = false;
        self.hold_to_talk = false;
        self.ptt_held = false;
        self.mic_permission_denied = false;
        self.camera_enabled = false;
        self.screen_share_enabled = false;
        self.noise_suppression_enabled = false;
        self.noise_suppression_level = DEFAULT_NOISE_SUPPRESSION_LEVEL;
        self.focused_tile = None;
        self.auto_focused_screen = None;
        self.room_name.clear();
        self.device_menu = None;
        self.device_submenu = None;
        self.participant_menu = None;
        self.interactive_launches.clear();
        self.active_interactive_apps.clear();
        self.opened_interactive_apps.clear();
        self.pending_kick = None;
        self.pending_removals.clear();
        self.moderation_error = None;
        self.agent_pending = false;
        self.participants.clear();
        self.join_ranks.clear();
        self.speak_ranks.clear();
        self.raised_hands.clear();
        self.raised_hand_timers.clear();
        self.recording_users.clear();
        self.recording_user_timers.clear();
        self.active_sounds.clear();
        self.sound_throttle.clear();
        self.sound_cache.clear();
        self.sound_preview = None;
        self.raising_hand_player = None;
        self.raising_hand_sound_loading = false;
        self.join_voice_player = None;
        self.join_voice_sound_loading = false;
        self.give_flower_player = None;
        self.give_flower_sound_loading = false;
        self.join_sound_baseline_set = false;
        self.displayed_reactions.clear();
        self.displayed_flowers.clear();
        self.last_emoji_at = None;
        self.last_flower_send = None;
        self.last_flower_effect_at = None;
        self.shown_flowers.clear();
        self.flower_seq = 0;
        self.meet_token_prefetching = None;
        self.last_screen_share = None;
        self.link_copied = false;
        self.recording = RecordingState::Idle;
        self.recording_elapsed = Duration::ZERO;
        self.recording_stalled = false;
        self._recording_tick = None;
        self._link_copied_reset = None;
    }
}

pub fn validate_sound_file(path: &Path, max_bytes: u64) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !SOUND_ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err("unsupported_type".into());
    }
    let len = std::fs::metadata(path)
        .map_err(|_| "invalid_file".to_string())?
        .len();
    if len == 0 {
        return Err("empty".into());
    }
    if len > max_bytes {
        return Err("size_limit".into());
    }
    let data = std::fs::read(path).map_err(|_| "invalid_file".to_string())?;
    let mime =
        mezon_audio::sniff_sound_mime(&data).ok_or_else(|| "unsupported_type".to_string())?;
    let ext_matches = match ext.as_str() {
        "wav" => mime == "audio/wav",
        "mp3" | "mpeg" => mime == "audio/mpeg",
        _ => false,
    };
    if !ext_matches {
        return Err("unsupported_type".into());
    }
    Ok(())
}

pub async fn upload_sound_file(
    api: &AppApi,
    path: &Path,
    max_bytes: u64,
) -> Result<(i64, String), String> {
    let path_buf = path.to_path_buf();
    let max = max_bytes;
    let data = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || {
            validate_sound_file(&path_buf, max)?;
            std::fs::read(&path_buf).map_err(|_| "invalid_file".to_string())
        })
        .await
        .map_err(|e| e.to_string())??;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3")
        .to_ascii_lowercase();
    let filetype = match ext.as_str() {
        "wav" => "audio/wav",
        "mp3" | "mpeg" => "audio/mpeg",
        _ => return Err("unsupported_type".into()),
    };
    let id = crate::emoji::generate_snowflake_id();
    api.upload_emoticon("sounds", id, &ext, filetype, data)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::parse_raise_token;
    use super::{
        FLOWER_DEDUP_WINDOW, INTERACTIVE_LAUNCH_DEDUP_TTL, MAX_SOUND_BYTES,
        RECORDING_AVATAR_MAX_ATTEMPTS, RecordingAvatar, VoiceConnection, flower_pair_key,
        is_duplicate_flower, is_duplicate_interactive_launch, redact_interactive_app_url,
        release_interactive_window, solo_tile_for, validate_sound_file, voice_join_error_message,
    };
    use crate::{VoiceInteractiveApp, VoiceInteractiveEventType};
    use gpui::RenderImage;
    use parking_lot::Mutex;

    #[test]
    fn a_closed_window_from_an_earlier_launch_does_not_release_the_current_one() {
        let app = VoiceInteractiveApp::Quiz;
        let mut opened = HashMap::from([(app, 2u64)]);
        assert!(!release_interactive_window(&mut opened, app, 1));
        assert!(
            opened.contains_key(&app),
            "after reset() and a relaunch the first window's exit must leave the new window \
             marked open, or Launch/Join reappear while it is still on screen"
        );
        assert!(release_interactive_window(&mut opened, app, 2));
        assert!(opened.is_empty());
        assert!(!release_interactive_window(&mut opened, app, 2));
    }

    #[test]
    fn an_open_member_strip_records_everyone() {
        let people = [
            voice_participant("1", Some(7)),
            voice_participant("2", None),
        ];
        assert_eq!(
            solo_tile_for(None, true, Some(&screen_tile_id("1")), &people),
            None
        );
    }

    const ME: i64 = 77;

    fn init_voice_store(cx: &mut gpui::App) -> gpui::Entity<super::VoiceStore> {
        use gpui::AppContext as _;
        let api = Arc::new(mezon_client::AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: ME.to_string(),
                ..Default::default()
            })
        });
        crate::badge::BadgeService::init(auth_state, cx);
        super::VoiceStore::init(api, cx)
    }

    fn connected(channel_id: &str) -> VoiceConnection {
        VoiceConnection::Connected {
            channel_id: channel_id.into(),
            clan_id: "1".into(),
        }
    }

    fn removed(channel_id: i64, user_ids: Vec<i64>) -> mezon_client::RealtimeEvent {
        mezon_client::RealtimeEvent::UserChannelRemoved(mezon_proto::realtime::UserChannelRemoved {
            channel_id,
            user_ids,
            channel_type: 10,
            ..Default::default()
        })
    }

    fn deleted(channel_id: i64) -> mezon_client::RealtimeEvent {
        mezon_client::RealtimeEvent::ChannelDeleted(mezon_proto::realtime::ChannelDeletedEvent {
            clan_id: 1,
            channel_id,
            ..Default::default()
        })
    }

    fn recording_event(
        channel_id: i64,
        sender_id: i64,
        recording: bool,
    ) -> mezon_client::RealtimeEvent {
        mezon_client::RealtimeEvent::VoiceInteractive(
            mezon_proto::realtime::VoiceInteractiveEvent {
                clan_id: 1,
                voice_channel_id: channel_id,
                sender_id,
                receiver_id: sender_id,
                event_type: crate::gifts::VoiceInteractiveEventType::Recording as i32,
                params: super::recording_params(recording),
            },
        )
    }

    #[test]
    fn recording_params_round_trip() {
        assert_eq!(super::recording_params(true), r#"{"isRecording":true}"#);
        assert_eq!(
            super::recording_from_params(&super::recording_params(true)),
            Some(true)
        );
        assert_eq!(
            super::recording_from_params(&super::recording_params(false)),
            Some(false)
        );
        assert_eq!(super::recording_from_params(""), None);
        assert_eq!(super::recording_from_params("userId=8"), None);
    }

    #[gpui::test]
    fn someone_recording_the_room_shows_until_they_stop(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let voice = init_voice_store(cx);
            voice.update(cx, |voice, cx| {
                voice.connection = connected("5");
                let mut me = voice_participant(&ME.to_string(), None);
                me.is_local = true;
                voice.participants = vec![me, voice_participant("8", None)];

                voice.handle_voice_interactive(&recording_event(5, 8, true), cx);
                assert_eq!(voice.recording_users(), ["8".to_string()]);

                voice.handle_voice_interactive(&recording_event(6, 9, true), cx);
                assert_eq!(voice.recording_users(), ["8".to_string()], "another room");

                voice.handle_voice_interactive(&recording_event(5, ME, true), cx);
                assert_eq!(
                    voice.recording_users(),
                    ["8".to_string()],
                    "a late echo of our own recording, which has stopped"
                );

                voice.handle_voice_interactive(&recording_event(5, 8, false), cx);
                assert!(voice.recording_users().is_empty());
            });
        });
    }

    #[gpui::test]
    fn a_recorder_who_leaves_the_room_stops_showing(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let voice = init_voice_store(cx);
            voice.update(cx, |voice, cx| {
                voice.connection = connected("5");
                voice.participants = vec![voice_participant("8", None)];
                voice.handle_voice_interactive(&recording_event(5, 8, true), cx);
                assert_eq!(voice.recording_users(), ["8".to_string()]);

                voice.participants = vec![voice_participant("9", None)];
                voice.prune_recording_users(cx);
                assert!(voice.recording_users().is_empty());
            });
        });
    }

    /// Losing the channel the call runs in ends the call; anything about
    /// another channel, or about another user, leaves it alone.
    #[gpui::test]
    fn losing_the_channel_of_the_active_call_hangs_up(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let voice = init_voice_store(cx);
            voice.update(cx, |voice, cx| {
                voice.connection = connected("5");

                voice.handle_access_lost(&removed(5, vec![8, 9]), cx);
                assert_eq!(voice.connection, connected("5"), "someone else was removed");

                voice.handle_access_lost(&removed(6, vec![ME]), cx);
                assert_eq!(
                    voice.connection,
                    connected("5"),
                    "removed from another channel"
                );

                voice.handle_access_lost(&deleted(6), cx);
                assert_eq!(
                    voice.connection,
                    connected("5"),
                    "another channel was deleted"
                );

                voice.handle_access_lost(&removed(5, vec![8, ME]), cx);
                assert_eq!(
                    voice.connection,
                    VoiceConnection::Idle,
                    "removed from our room"
                );

                voice.connection = connected("5");
                voice.handle_access_lost(&deleted(5), cx);
                assert_eq!(
                    voice.connection,
                    VoiceConnection::Idle,
                    "our room was deleted"
                );

                voice.leave_lost_channel("5", cx);
                assert_eq!(voice.connection, VoiceConnection::Idle, "idle stays idle");

                let kicked = |clan_id: i64, user_ids: Vec<i64>| {
                    mezon_client::RealtimeEvent::UserClanRemoved(
                        mezon_proto::realtime::UserClanRemoved { clan_id, user_ids },
                    )
                };
                voice.connection = connected("5");
                voice.handle_clan_access_lost(&kicked(2, vec![ME]), cx);
                assert_eq!(voice.connection, connected("5"), "kicked from another clan");
                voice.handle_clan_access_lost(&kicked(1, vec![8]), cx);
                assert_eq!(voice.connection, connected("5"), "someone else was kicked");
                voice.handle_clan_access_lost(&kicked(1, vec![ME]), cx);
                assert_eq!(
                    voice.connection,
                    VoiceConnection::Idle,
                    "kicked from our clan"
                );
            });
        });
    }

    /// The channel list announcing a lost channel must reach the voice store
    /// through its subscription, not only through the realtime handlers.
    #[gpui::test]
    fn a_channel_dropped_by_a_refetch_hangs_up_through_the_subscription(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext as _;
        let (voice, channels) = cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            crate::realtime::RealtimeDispatch::init(api.clone(), cx);
            let auth_state = cx.new(|_| {
                crate::AuthState::Authenticated(mezon_client::Session {
                    user_id: ME.to_string(),
                    ..Default::default()
                })
            });
            crate::badge::BadgeService::init(auth_state, cx);
            crate::clan::ClanList::init(api.clone(), cx);
            let channels = crate::ChannelList::init(api.clone(), cx);
            let voice = super::VoiceStore::init(api, cx);
            voice.update(cx, |voice, _| voice.connection = connected("2"));
            channels.update(cx, |channels, cx| {
                channels.apply_clan_structure(
                    crate::ids::ClanId(1),
                    crate::channel::test_support::two_channels(),
                    None,
                    cx,
                );
                channels.apply_clan_structure(
                    crate::ids::ClanId(1),
                    crate::channel::test_support::one_channel(),
                    None,
                    cx,
                );
            });
            (voice, channels)
        });
        cx.update(|cx| {
            assert_eq!(
                voice.read(cx).connection,
                VoiceConnection::Idle,
                "channel 2 vanished from the listing while we were in it"
            );
        });
        drop(channels);
    }

    #[test]
    fn a_closed_member_strip_records_only_the_focused_tile() {
        let people = [
            voice_participant("1", Some(7)),
            voice_participant("2", None),
        ];
        assert_eq!(
            solo_tile_for(None, false, Some(&screen_tile_id("1")), &people),
            Some(screen_tile_id("1"))
        );
        assert_eq!(
            solo_tile_for(None, false, Some(&camera_tile_id("2")), &people),
            Some(camera_tile_id("2"))
        );
    }

    #[test]
    fn a_closed_strip_with_nothing_focused_still_records_the_grid() {
        let people = [voice_participant("1", None)];
        assert_eq!(solo_tile_for(None, false, None, &people), None);
    }

    #[test]
    fn a_fullscreen_share_records_only_that_share() {
        let people = [
            voice_participant("1", Some(7)),
            voice_participant("2", None),
        ];
        assert_eq!(
            solo_tile_for(Some(7), true, Some(&camera_tile_id("2")), &people),
            Some(screen_tile_id("1"))
        );
    }

    #[test]
    fn a_fullscreen_key_whose_sharer_left_records_nothing_extra() {
        let people = [voice_participant("2", None)];
        assert_eq!(solo_tile_for(Some(7), true, None, &people), None);
    }

    fn avatar_pixel() -> Arc<mezon_voice::compose::AvatarImage> {
        Arc::new(mezon_voice::compose::AvatarImage {
            width: 1,
            height: 1,
            bgra: vec![0, 0, 0, 255],
        })
    }

    #[test]
    fn a_loaded_avatar_is_not_fetched_again() {
        let mut slot = RecordingAvatar::default();
        assert!(slot.claim("https://cdn/a.png"));
        slot.settle("https://cdn/a.png", Some(avatar_pixel()));
        assert!(!slot.claim("https://cdn/a.png"));
    }

    #[test]
    fn a_changed_avatar_drops_the_picture_it_replaces() {
        let mut slot = RecordingAvatar::default();
        assert!(slot.claim("https://cdn/old.png"));
        slot.settle("https://cdn/old.png", Some(avatar_pixel()));

        assert!(slot.claim("https://cdn/new.png"));
        assert!(
            slot.image.is_none(),
            "the recording must not keep drawing the old avatar"
        );
    }

    #[test]
    fn a_fetch_that_lands_after_the_avatar_changed_is_ignored() {
        let mut slot = RecordingAvatar::default();
        assert!(slot.claim("https://cdn/old.png"));
        assert!(slot.claim("https://cdn/new.png"));

        slot.settle("https://cdn/old.png", Some(avatar_pixel()));
        assert!(slot.image.is_none());
        assert!(slot.loading, "the newer fetch is still in flight");
    }

    #[test]
    fn a_failed_fetch_retries_a_bounded_number_of_times() {
        let mut slot = RecordingAvatar::default();
        for _ in 0..RECORDING_AVATAR_MAX_ATTEMPTS {
            assert!(slot.claim("https://cdn/a.png"));
            slot.settle("https://cdn/a.png", None);
        }
        assert!(
            !slot.claim("https://cdn/a.png"),
            "a URL that keeps failing must stop hitting the CDN every tick"
        );
        assert!(
            slot.claim("https://cdn/b.png"),
            "a new URL starts over from a clean slot"
        );
    }

    #[test]
    fn a_recovered_fetch_clears_the_failure_count() {
        let mut slot = RecordingAvatar::default();
        assert!(slot.claim("https://cdn/a.png"));
        slot.settle("https://cdn/a.png", None);
        assert!(slot.claim("https://cdn/a.png"));
        slot.settle("https://cdn/a.png", Some(avatar_pixel()));
        assert_eq!(slot.attempts, 0);
    }

    #[test]
    fn interactive_app_type_maps_only_app_events() {
        assert_eq!(
            VoiceInteractiveApp::from_event_type(VoiceInteractiveEventType::AppQuiz as i32),
            Some(VoiceInteractiveApp::Quiz)
        );
        assert_eq!(
            VoiceInteractiveApp::from_event_type(VoiceInteractiveEventType::AppBlackboard as i32),
            Some(VoiceInteractiveApp::Blackboard)
        );
        assert_eq!(
            VoiceInteractiveApp::from_event_type(VoiceInteractiveEventType::AppInteractive as i32),
            Some(VoiceInteractiveApp::Interactive)
        );
        assert_eq!(
            VoiceInteractiveApp::from_event_type(VoiceInteractiveEventType::Gift as i32),
            None
        );
        assert_eq!(VoiceInteractiveApp::from_event_type(999), None);
    }

    #[test]
    fn interactive_app_log_url_redacts_signed_data() {
        assert_eq!(
            redact_interactive_app_url(
                "https://quiz.mezon.ai/?data=signed-secret&clanId=42&clanName=Class"
            ),
            "https://quiz.mezon.ai/?data=REDACTED&clanId=42&clanName=Class"
        );
        assert_eq!(
            redact_interactive_app_url(
                "https://quiz.mezon.ai/?appdata=keep-data%3D&data=signed-secret&clanId=42"
            ),
            "https://quiz.mezon.ai/?appdata=keep-data%3D&data=REDACTED&clanId=42"
        );
        assert_eq!(
            redact_interactive_app_url("not a URL?data=signed-secret"),
            "<invalid interactive app URL>"
        );
    }

    #[test]
    fn interactive_launch_dedup_is_keyed_by_app_and_initiator() {
        let now = std::time::Instant::now();
        let mut launches = std::collections::HashMap::new();
        let quiz = (VoiceInteractiveApp::Quiz, 1);
        let blackboard = (VoiceInteractiveApp::Blackboard, 2);
        assert!(!is_duplicate_interactive_launch(&mut launches, quiz, now));
        launches.insert(quiz, now);
        assert!(!is_duplicate_interactive_launch(
            &mut launches,
            blackboard,
            now
        ));
        launches.insert(blackboard, now);
        assert!(is_duplicate_interactive_launch(&mut launches, quiz, now));
        assert!(!is_duplicate_interactive_launch(
            &mut launches,
            (VoiceInteractiveApp::Quiz, 3),
            now
        ));
        assert!(!is_duplicate_interactive_launch(
            &mut launches,
            quiz,
            now + INTERACTIVE_LAUNCH_DEDUP_TTL
        ));
    }

    #[test]
    fn validate_sound_file_rejects_mismatched_extension() {
        let dir = std::env::temp_dir().join(format!("mezon-sound-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let wav_path = dir.join("fake.mp3");
        let mut wav = vec![0u8; 44];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        std::fs::write(&wav_path, wav).unwrap();
        assert_eq!(
            validate_sound_file(&wav_path, MAX_SOUND_BYTES).unwrap_err(),
            "unsupported_type"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_sound_file_accepts_wav() {
        let dir = std::env::temp_dir().join(format!("mezon-sound-wav-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let wav_path = dir.join("tone.wav");
        let mut wav = vec![0u8; 44];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        std::fs::write(&wav_path, wav).unwrap();
        assert!(validate_sound_file(&wav_path, MAX_SOUND_BYTES).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_sound_file_accepts_mp3_with_id3_tag() {
        let dir = std::env::temp_dir().join(format!("mezon-sound-id3-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mp3_path = dir.join("tagged.mp3");
        let mut bytes = vec![0u8; 128];
        bytes[0..3].copy_from_slice(b"ID3");
        bytes[6..10].copy_from_slice(&[0, 0, 0, 100]);
        bytes[110..112].copy_from_slice(&[0xFF, 0xFB]);
        std::fs::write(&mp3_path, bytes).unwrap();
        assert!(validate_sound_file(&mp3_path, MAX_SOUND_BYTES).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::{
        ScreenAutoFocus, camera_tile_id, default_focus_tile_for, screen_auto_focus_transition,
        screen_tile_id,
    };
    use crate::{NetworkQuality, VoiceParticipant};

    fn voice_participant(identity: &str, screenshare: Option<u64>) -> VoiceParticipant {
        VoiceParticipant {
            session_id: identity.to_string(),
            identity: identity.to_string(),
            name: identity.to_string(),
            is_local: false,
            is_agent: false,
            is_audience: false,
            speaking: false,
            muted: false,
            camera: None,
            screenshare,
            quality: NetworkQuality::Unknown,
        }
    }

    #[test]
    fn focus_tracks_the_device_when_one_account_shares_twice() {
        let mut first = voice_participant("same-account", Some(11));
        first.session_id = "device-1".into();
        let mut second = voice_participant("same-account", Some(22));
        second.session_id = "device-2".into();
        let focused = screen_tile_id(&first.session_id);
        let participants = vec![second.clone(), first];
        assert_eq!(
            screen_auto_focus_transition(&participants, Some(&focused)),
            ScreenAutoFocus::Keep
        );
        assert_eq!(
            super::solo_tile_for(Some(22), true, None, &participants),
            Some(screen_tile_id(&second.session_id))
        );
        assert_eq!(
            screen_auto_focus_transition(&[second.clone()], Some(&focused)),
            ScreenAutoFocus::Focus(screen_tile_id(&second.session_id))
        );
    }

    #[test]
    fn auto_focuses_first_screen_share() {
        let participants = vec![
            voice_participant("a", None),
            voice_participant("b", Some(1)),
            voice_participant("c", Some(2)),
        ];
        assert_eq!(
            screen_auto_focus_transition(&participants, None),
            ScreenAutoFocus::Focus(screen_tile_id("b"))
        );
    }

    #[test]
    fn keeps_state_while_auto_focused_share_lives() {
        let participants = vec![voice_participant("b", Some(1))];
        let auto = screen_tile_id("b");
        assert_eq!(
            screen_auto_focus_transition(&participants, Some(&auto)),
            ScreenAutoFocus::Keep
        );
    }

    #[test]
    fn does_not_steal_focus_for_second_share() {
        let participants = vec![
            voice_participant("b", Some(1)),
            voice_participant("c", Some(2)),
        ];
        let auto = screen_tile_id("b");
        assert_eq!(
            screen_auto_focus_transition(&participants, Some(&auto)),
            ScreenAutoFocus::Keep
        );
    }

    #[test]
    fn clears_focus_when_auto_focused_share_ends() {
        let participants = vec![voice_participant("b", None)];
        let auto = screen_tile_id("b");
        assert_eq!(
            screen_auto_focus_transition(&participants, Some(&auto)),
            ScreenAutoFocus::Clear
        );
    }

    #[test]
    fn moves_focus_to_remaining_share_when_auto_focused_share_ends() {
        let participants = vec![
            voice_participant("b", None),
            voice_participant("c", Some(2)),
        ];
        let auto = screen_tile_id("b");
        assert_eq!(
            screen_auto_focus_transition(&participants, Some(&auto)),
            ScreenAutoFocus::Focus(screen_tile_id("c"))
        );
    }

    #[test]
    fn stays_idle_without_screen_shares() {
        let participants = vec![voice_participant("a", None)];
        assert_eq!(
            screen_auto_focus_transition(&participants, None),
            ScreenAutoFocus::Keep
        );
    }

    #[test]
    fn layout_toggle_prefers_screen_share() {
        let participants = vec![
            voice_participant("a", None),
            voice_participant("b", Some(1)),
        ];
        assert_eq!(
            default_focus_tile_for(&participants),
            Some(screen_tile_id("b"))
        );
    }

    #[test]
    fn layout_toggle_prefers_camera_with_video() {
        let mut a = voice_participant("a", None);
        a.camera = Some(1);
        let participants = vec![a, voice_participant("b", None)];
        assert_eq!(
            default_focus_tile_for(&participants),
            Some(camera_tile_id("a"))
        );
    }

    #[test]
    fn layout_toggle_falls_back_to_first_participant() {
        let participants = vec![voice_participant("a", None), voice_participant("b", None)];
        assert_eq!(
            default_focus_tile_for(&participants),
            Some(camera_tile_id("a"))
        );
    }

    #[test]
    fn active_channel_id_only_tracks_connecting_and_connected() {
        assert_eq!(
            VoiceConnection::Connecting {
                channel_id: "a".into(),
                clan_id: "1".into(),
            }
            .active_channel_id(),
            Some("a")
        );
        assert_eq!(
            VoiceConnection::Connected {
                channel_id: "a".into(),
                clan_id: "1".into(),
            }
            .active_channel_id(),
            Some("a")
        );
        assert_eq!(
            VoiceConnection::Failed {
                channel_id: "a".into(),
                message: "boom".into(),
            }
            .active_channel_id(),
            None
        );
        assert_eq!(VoiceConnection::Idle.active_channel_id(), None);
    }

    #[test]
    fn mark_connected_only_promotes_a_pending_connection() {
        let mut connection = VoiceConnection::Connecting {
            channel_id: "a".into(),
            clan_id: "1".into(),
        };
        assert!(connection.mark_connected());
        assert_eq!(
            connection,
            VoiceConnection::Connected {
                channel_id: "a".into(),
                clan_id: "1".into(),
            }
        );
        assert!(!connection.mark_connected());
        assert_eq!(connection.connected_channel(), Some(("a", "1")));

        let mut idle = VoiceConnection::Idle;
        assert!(!idle.mark_connected());
        assert_eq!(idle, VoiceConnection::Idle);

        let mut failed = VoiceConnection::Failed {
            channel_id: "a".into(),
            message: "boom".into(),
        };
        assert!(!failed.mark_connected());
        assert_eq!(failed.active_channel_id(), None);
    }

    #[test]
    fn a_call_still_connecting_already_has_its_channel() {
        let mut connection = VoiceConnection::Connecting {
            channel_id: "a".into(),
            clan_id: "1".into(),
        };
        assert_eq!(connection.active_channel(), Some(("a", "1")));
        assert!(connection.is_connecting());
        assert_eq!(connection.connected_channel(), None);

        assert!(connection.mark_connected());
        assert_eq!(connection.active_channel(), Some(("a", "1")));
        assert!(!connection.is_connecting());

        let failed = VoiceConnection::Failed {
            channel_id: "a".into(),
            message: "boom".into(),
        };
        assert_eq!(failed.active_channel(), None);
        assert!(!failed.is_connecting());
        assert_eq!(VoiceConnection::Idle.active_channel(), None);
    }

    #[test]
    fn voice_join_error_maps_typed_api_status_and_generic() {
        let permission_denied: anyhow::Error = mezon_client::ApiStatusError { code: 7 }.into();
        assert_eq!(
            voice_join_error_message(&permission_denied, "en"),
            mezon_i18n::api_error("en", 7)
        );
        assert_eq!(
            voice_join_error_message(&anyhow::anyhow!("empty body (code=0)"), "en"),
            mezon_i18n::t("en", "channelVoice.joinFailed")
        );
        assert_eq!(
            voice_join_error_message(&anyhow::anyhow!("invalid_token"), "en"),
            mezon_i18n::t("en", "channelVoice.joinFailed")
        );
    }

    #[test]
    fn parse_raise_token_classifies_prefixes() {
        assert_eq!(parse_raise_token("raising-up:123"), Some(true));
        assert_eq!(parse_raise_token("raising-down:123"), Some(false));
        assert_eq!(parse_raise_token("sound:https://x.mp3"), None);
        assert_eq!(parse_raise_token(":smile:"), None);
        assert_eq!(parse_raise_token(""), None);
    }

    #[test]
    fn the_second_copy_of_one_flower_is_dropped_but_a_later_one_is_not() {
        let mut shown = HashMap::new();
        let now = Instant::now();
        let pair = flower_pair_key("10", "20");
        assert!(!is_duplicate_flower(&mut shown, &pair, now));
        shown.insert(pair.clone(), now);

        let reaction_arrives = now + Duration::from_millis(30);
        assert!(is_duplicate_flower(&mut shown, &pair, reaction_arrives));
        assert!(!is_duplicate_flower(
            &mut shown,
            &flower_pair_key("10", "21"),
            reaction_arrives
        ));

        let window_passed = now + FLOWER_DEDUP_WINDOW + Duration::from_millis(1);
        assert!(!is_duplicate_flower(&mut shown, &pair, window_passed));
        assert!(shown.is_empty());
    }

    #[test]
    fn recyclable_render_image_returns_owned_pixels() {
        let returned = Arc::new(Mutex::new(Vec::new()));
        let sink = returned.clone();
        let recycler = Arc::new(move |pixels| sink.lock().push(pixels));
        let buffer = image::RgbaImage::from_raw(2, 2, vec![7; 16]).expect("rgba");
        let image = RenderImage::new_recyclable(image::Frame::new(buffer), recycler);
        drop(image);
        assert_eq!(returned.lock().as_slice(), &[vec![7; 16]]);
    }
}
