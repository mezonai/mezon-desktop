use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, Global, RenderImage, Task, Window};
use parking_lot::Mutex;

use crate::voice::VoiceRenderFrame;

use mezon_audio::AudioPlayer;
use mezon_call::{
    AnswerPayload, CallConfig, CallEngine, EngineCommand, EngineEvent, IcePayload, OfferPayload,
    WEBRTC_CLEAR_CALL, WEBRTC_ICE_CANDIDATE, WEBRTC_SDP_ANSWER, WEBRTC_SDP_INIT,
    WEBRTC_SDP_JOINED_OTHER_CALL, WEBRTC_SDP_OFFER, WEBRTC_SDP_QUIT,
    WEBRTC_SDP_STATUS_REMOTE_MEDIA, WEBRTC_SDP_TIMEOUT, compress_sdp, decompress_sdp,
};
use mezon_client::{AppApi, RealtimeEvent};
use mezon_voice::{IceServerConfig, VideoFrameStore};

use crate::AppConfig;
use crate::account::AccountStore;
use crate::message::CallLogType;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const NO_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
const ICE_DISCONNECT_GRACE: Duration = Duration::from_secs(12);
const MAX_PENDING_ICE: usize = 128;
const DM_STREAM_MODE: i32 = 4;

static DIALTONE_SOUND: &[u8] = include_bytes!("../assets/audio/dialtone.mp3");
static RINGING_SOUND: &[u8] = include_bytes!("../assets/audio/ringing.mp3");
static ENDCALL_SOUND: &[u8] = include_bytes!("../assets/audio/endcall.mp3");
static BUSYTONE_SOUND: &[u8] = include_bytes!("../assets/audio/busytone.mp3");

#[derive(Clone, Debug)]
pub struct CallPeer {
    pub user_id: i64,
    pub channel_id: i64,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPhase {
    Idle,
    Outgoing,
    Incoming,
    Connecting,
    Connected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaFlags {
    pub mic_on: bool,
    pub cam_on: bool,
}

#[derive(Clone, Copy)]
enum ToneSlot {
    Dial,
    Ring,
    End,
    Busy,
}

#[derive(Clone, Copy)]
enum EndReason {
    LocalHangup,
    RemoteQuit,
    Timeout,
    Failed,
    Busy,
}

#[derive(Default)]
struct CallTones {
    dial: Option<AudioPlayer>,
    ring: Option<AudioPlayer>,
    end: Option<AudioPlayer>,
    busy: Option<AudioPlayer>,
}

impl CallTones {
    fn stop_dial_ring(&mut self) {
        self.dial = None;
        self.ring = None;
    }
}

struct CachedRenderFrame {
    seq: u64,
    frame: VoiceRenderFrame,
}

struct GlobalCallStore(Entity<CallStore>);
impl Global for GlobalCallStore {}

pub struct CallStore {
    api: Arc<AppApi>,
    phase: CallPhase,
    peer: Option<CallPeer>,
    is_caller: bool,
    media: MediaKind,
    local: MediaFlags,
    remote: MediaFlags,
    mic_prompt: bool,
    camera_prompt: bool,
    selected_input: Option<String>,
    selected_output: Option<String>,
    incoming_offer: Option<String>,
    engine: Option<CallEngine>,
    frame_store: Option<Arc<VideoFrameStore>>,
    connected_at: Option<Instant>,
    self_id: i64,
    self_name: String,
    self_avatar: String,
    call_message_id: Option<i64>,
    call_create_time: u32,
    tones: CallTones,
    generation: u64,
    pending_remote_ice: Vec<IcePayload>,
    pending_local_ice: Vec<IcePayload>,
    render_cache: Mutex<HashMap<u64, CachedRenderFrame>>,
    pending_texture_drops: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_replaces: Mutex<Vec<Arc<RenderImage>>>,
    pending_texture_work: AtomicBool,
    _events_task: Option<Task<()>>,
    _timeout_task: Option<Task<()>>,
    _ice_grace_task: Option<Task<()>>,
}

impl CallStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalCallStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCallStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalCallStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        Self {
            api,
            phase: CallPhase::Idle,
            peer: None,
            is_caller: false,
            media: MediaKind::Audio,
            local: MediaFlags::default(),
            remote: MediaFlags::default(),
            mic_prompt: false,
            camera_prompt: false,
            selected_input: None,
            selected_output: None,
            incoming_offer: None,
            engine: None,
            frame_store: None,
            connected_at: None,
            self_id: 0,
            self_name: String::new(),
            self_avatar: String::new(),
            call_message_id: None,
            call_create_time: 0,
            tones: CallTones::default(),
            pending_remote_ice: Vec::new(),
            pending_local_ice: Vec::new(),
            generation: 0,
            render_cache: Mutex::new(HashMap::new()),
            pending_texture_drops: Mutex::new(Vec::new()),
            pending_texture_replaces: Mutex::new(Vec::new()),
            pending_texture_work: AtomicBool::new(false),
            _events_task: None,
            _timeout_task: None,
            _ice_grace_task: None,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::WebrtcSignaling, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on(
                RealtimeKind::IncomingCallPush,
                &entity,
                |this, event, cx| this.handle_event(event, cx),
            );
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
        });
    }

    pub fn phase(&self) -> CallPhase {
        self.phase
    }

    pub fn peer(&self) -> Option<&CallPeer> {
        self.peer.as_ref()
    }

    pub fn is_caller(&self) -> bool {
        self.is_caller
    }

    pub fn is_video(&self) -> bool {
        self.media == MediaKind::Video || self.local.cam_on
    }

    pub fn local_flags(&self) -> MediaFlags {
        self.local
    }

    pub fn remote_flags(&self) -> MediaFlags {
        self.remote
    }

    pub fn selected_input(&self) -> Option<&str> {
        self.selected_input.as_deref()
    }

    pub fn selected_output(&self) -> Option<&str> {
        self.selected_output.as_deref()
    }

    pub fn self_name(&self) -> &str {
        &self.self_name
    }

    pub fn self_avatar(&self) -> &str {
        &self.self_avatar
    }

    pub fn self_id(&self) -> i64 {
        self.self_id
    }

    pub fn connected_at(&self) -> Option<Instant> {
        self.connected_at
    }

    pub fn frame_store(&self) -> Option<Arc<VideoFrameStore>> {
        self.frame_store.clone()
    }

    pub fn remote_frame_key(&self) -> u64 {
        mezon_call::REMOTE_FRAME_KEY
    }

    pub fn mic_prompt(&self) -> bool {
        self.mic_prompt
    }

    pub fn dismiss_mic_prompt(&mut self, cx: &mut Context<Self>) {
        if self.mic_prompt {
            self.mic_prompt = false;
            cx.notify();
        }
    }

    pub fn camera_prompt(&self) -> bool {
        self.camera_prompt
    }

    pub fn dismiss_camera_prompt(&mut self, cx: &mut Context<Self>) {
        if self.camera_prompt {
            self.camera_prompt = false;
            cx.notify();
        }
    }

    pub fn has_remote_video(&self) -> bool {
        self.frame_store
            .as_ref()
            .and_then(|store| store.get(mezon_call::REMOTE_FRAME_KEY))
            .is_some()
    }

    pub fn self_frame_key(&self) -> u64 {
        mezon_voice::local_camera_key(&self.self_id.to_string())
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

    pub fn flush_texture_drops(&self, mut window: Option<&mut Window>, cx: &mut App) {
        if !self.pending_texture_work.swap(false, Ordering::AcqRel) {
            return;
        }
        let drops: Vec<Arc<RenderImage>> = std::mem::take(&mut *self.pending_texture_drops.lock());
        let replaces: Vec<Arc<RenderImage>> =
            std::mem::take(&mut *self.pending_texture_replaces.lock());
        let dropped: std::collections::HashSet<_> = drops.iter().map(|image| image.id).collect();
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

    pub fn start_call(&mut self, peer: CallPeer, video: bool, cx: &mut Context<Self>) {
        if !matches!(self.phase, CallPhase::Idle) {
            return;
        }
        if mezon_voice::microphone_denied() {
            self.mic_prompt = true;
            cx.notify();
            return;
        }
        if video && mezon_voice::camera_denied() {
            self.camera_prompt = true;
            cx.notify();
            return;
        }
        let Some((self_id, self_name, self_avatar)) = self_identity(cx) else {
            tracing::warn!("cannot start call: no account");
            return;
        };
        self.generation += 1;
        self.self_id = self_id;
        self.self_name = self_name;
        self.self_avatar = self_avatar;
        self.peer = Some(peer);
        self.is_caller = true;
        self.media = if video {
            MediaKind::Video
        } else {
            MediaKind::Audio
        };
        self.local = MediaFlags {
            mic_on: true,
            cam_on: video,
        };
        self.remote = MediaFlags::default();
        self.connected_at = None;
        self.phase = CallPhase::Outgoing;

        let (input_device, output_device, camera_device) = self.call_devices(cx);
        let config = CallConfig {
            ice_servers: ice_servers(cx),
            is_caller: true,
            self_identity: self_id.to_string(),
            input_device,
            output_device,
            camera_device,
            initial_camera_on: video,
        };
        self.spawn_engine(config, cx);
        self.write_start_call_log(video, cx);
        self.play_tone(ToneSlot::Dial, DIALTONE_SOUND, true, cx);
        self.start_timeout(cx);
        cx.notify();
    }

    pub fn accept(&mut self, video: bool, cx: &mut Context<Self>) {
        if !matches!(self.phase, CallPhase::Incoming) {
            return;
        }
        let Some(offer_sdp) = self.incoming_offer.take() else {
            return;
        };
        let Some((self_id, self_name, self_avatar)) = self_identity(cx) else {
            return;
        };
        if mezon_voice::microphone_denied() {
            self.mic_prompt = true;
        }
        self.self_id = self_id;
        self.self_name = self_name;
        self.self_avatar = self_avatar;
        self.is_caller = false;
        self.media = if video {
            MediaKind::Video
        } else {
            MediaKind::Audio
        };
        self.local = MediaFlags {
            mic_on: true,
            cam_on: video,
        };
        self.phase = CallPhase::Connecting;
        self.tones.stop_dial_ring();

        let (input_device, output_device, camera_device) = self.call_devices(cx);
        let config = CallConfig {
            ice_servers: ice_servers(cx),
            is_caller: false,
            self_identity: self_id.to_string(),
            input_device,
            output_device,
            camera_device,
            initial_camera_on: video,
        };
        self.spawn_engine(config, cx);
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::ApplyRemoteOffer(offer_sdp));
        }
        self.start_timeout(cx);
        cx.notify();
    }

    pub fn decline(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, CallPhase::Incoming) {
            return;
        }
        self.end_call(EndReason::LocalHangup, cx);
    }

    pub fn hangup(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, CallPhase::Idle) {
            return;
        }
        self.end_call(EndReason::LocalHangup, cx);
    }

    pub fn logout_teardown(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, CallPhase::Idle) {
            return;
        }
        self.end_call(EndReason::LocalHangup, cx);
    }

    pub fn toggle_mic(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            return;
        }
        self.local.mic_on = !self.local.mic_on;
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::SetMicEnabled(self.local.mic_on));
        }
        let status = serde_json::json!({ "micEnabled": self.local.mic_on }).to_string();
        self.send_to_peer(WEBRTC_SDP_STATUS_REMOTE_MEDIA, status, cx);
        cx.notify();
    }

    pub fn toggle_camera(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            return;
        }
        self.local.cam_on = !self.local.cam_on;
        if self.local.cam_on {
            self.media = MediaKind::Video;
        }
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::SetCameraEnabled(self.local.cam_on));
        }
        let status = serde_json::json!({ "cameraEnabled": self.local.cam_on }).to_string();
        self.send_to_peer(WEBRTC_SDP_STATUS_REMOTE_MEDIA, status, cx);
        cx.notify();
    }

    pub fn set_input_device(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        self.selected_input = device_id.clone();
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::SetInputDevice(device_id));
        }
        cx.notify();
    }

    pub fn set_output_device(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        self.selected_output = device_id.clone();
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::SetOutputDevice(device_id));
        }
        cx.notify();
    }

    pub fn set_camera_device(&self, device_id: Option<String>) {
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::SetCameraDevice(device_id));
        }
    }

    fn spawn_engine(&mut self, config: CallConfig, cx: &mut Context<Self>) {
        self.selected_input = config.input_device.clone();
        self.selected_output = config.output_device.clone();
        let engine = CallEngine::start(config);
        let events = engine.events().clone();
        self.frame_store = Some(engine.frame_store());
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let alive = this
                    .update(cx, |this, cx| {
                        if this.generation == generation {
                            this.on_engine_event(event, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        self._events_task = Some(task);
        self.engine = Some(engine);
        self.flush_remote_ice();
    }

    fn call_devices(&self, cx: &App) -> (Option<String>, Option<String>, Option<String>) {
        let (input, output, camera) = crate::Settings::try_global(cx)
            .map(|settings| {
                let settings = settings.read(cx);
                (
                    settings.input_device_id.clone(),
                    settings.output_device_id.clone(),
                    settings.camera_device_id.clone(),
                )
            })
            .unwrap_or_default();
        (
            self.selected_input.clone().or(input),
            self.selected_output.clone().or(output),
            camera,
        )
    }

    fn flush_remote_ice(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        let pending = std::mem::take(&mut self.pending_remote_ice);
        if pending.is_empty() {
            return;
        }
        tracing::debug!("call: flushing {} buffered remote ice", pending.len());
        for ice in pending {
            engine.send(EngineCommand::AddRemoteIce(ice));
        }
    }

    fn flush_local_ice(&mut self, cx: &Context<Self>) {
        let pending = std::mem::take(&mut self.pending_local_ice);
        if pending.is_empty() {
            return;
        }
        tracing::debug!("call: flushing {} buffered local ice", pending.len());
        for ice in pending {
            self.send_local_ice(ice, cx);
        }
    }

    fn send_local_ice(&self, ice: IcePayload, cx: &Context<Self>) {
        if let Ok(json) = serde_json::to_string(&ice) {
            self.send_to_peer(WEBRTC_ICE_CANDIDATE, json, cx);
        }
    }

    fn on_engine_event(&mut self, event: EngineEvent, cx: &mut Context<Self>) {
        match event {
            EngineEvent::LocalOffer(sdp) => {
                let payload = OfferPayload {
                    kind: "offer".to_string(),
                    sdp,
                    caller_name: self.self_name.clone(),
                    caller_avatar: self.self_avatar.clone(),
                };
                if let Some(compressed) = serde_json::to_string(&payload)
                    .ok()
                    .and_then(|json| compress_sdp(&json).ok())
                {
                    self.send_to_peer(WEBRTC_SDP_OFFER, compressed.clone(), cx);
                    self.send_call_push_offer(compressed, cx);
                }
            }
            EngineEvent::LocalAnswer(sdp) => {
                let payload = AnswerPayload {
                    kind: "answer".to_string(),
                    sdp,
                };
                if let Some(compressed) = serde_json::to_string(&payload)
                    .ok()
                    .and_then(|json| compress_sdp(&json).ok())
                {
                    self.send_to_peer(WEBRTC_SDP_ANSWER, compressed, cx);
                }
            }
            EngineEvent::LocalIce(ice) => {
                if matches!(self.phase, CallPhase::Outgoing) {
                    if self.pending_local_ice.len() < MAX_PENDING_ICE {
                        self.pending_local_ice.push(ice);
                    }
                    return;
                }
                self.send_local_ice(ice, cx);
            }
            EngineEvent::Connected => {
                self._ice_grace_task = None;
                if matches!(self.phase, CallPhase::Connected) {
                    return;
                }
                self.phase = CallPhase::Connected;
                self.connected_at = Some(Instant::now());
                self.cancel_timeout();
                self.tones.stop_dial_ring();
                self.send_to_peer(WEBRTC_SDP_INIT, String::new(), cx);
                let status = serde_json::json!({
                    "cameraEnabled": self.local.cam_on,
                    "micEnabled": self.local.mic_on,
                })
                .to_string();
                self.send_to_peer(WEBRTC_SDP_STATUS_REMOTE_MEDIA, status, cx);
                if self.is_caller {
                    self.send_call_push_cancel(true, cx);
                }
                cx.notify();
            }
            EngineEvent::Disconnected => {
                self.start_ice_grace(cx);
            }
            EngineEvent::Failed | EngineEvent::Closed => {
                self.end_call(EndReason::Failed, cx);
            }
            EngineEvent::MicUnavailable => {
                self.mic_prompt = true;
                cx.notify();
            }
            EngineEvent::CameraUnavailable => {
                self.camera_prompt = true;
                if self.local.cam_on {
                    self.local.cam_on = false;
                    let status = serde_json::json!({ "cameraEnabled": false }).to_string();
                    self.send_to_peer(WEBRTC_SDP_STATUS_REMOTE_MEDIA, status, cx);
                }
                cx.notify();
            }
        }
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let fwd = match event {
            RealtimeEvent::WebrtcSignaling(fwd) => fwd,
            RealtimeEvent::IncomingCallPush(push) => {
                self.on_call_push(push.channel_id, &push.json_data, cx);
                return;
            }
            RealtimeEvent::ChannelMessage(m) => {
                self.on_call_log_end(m.channel_id, &m.content, cx);
                return;
            }
            _ => return,
        };
        let caller_id = fwd.caller_id;
        let channel_id = fwd.channel_id;
        let from_peer = self.peer.as_ref().map(|p| p.user_id) == Some(caller_id);
        if matches!(self.phase, CallPhase::Incoming)
            && from_peer
            && fwd.data_type == WEBRTC_SDP_INIT
        {
            tracing::info!("call: caller connected elsewhere (INIT while incoming) -> dismiss");
            self.reset_state();
            cx.notify();
            return;
        }
        match fwd.data_type {
            WEBRTC_SDP_OFFER => self.on_remote_offer(caller_id, channel_id, &fwd.json_data, cx),
            WEBRTC_SDP_ANSWER => self.on_remote_answer(caller_id, &fwd.json_data, cx),
            WEBRTC_ICE_CANDIDATE => self.on_remote_ice(caller_id, &fwd.json_data),
            WEBRTC_SDP_QUIT => {
                if !matches!(self.phase, CallPhase::Idle) {
                    self.forward(caller_id, WEBRTC_CLEAR_CALL, String::new(), channel_id, cx);
                }
                self.on_remote_end(caller_id, EndReason::RemoteQuit, cx);
            }
            WEBRTC_CLEAR_CALL => self.on_remote_end(caller_id, EndReason::RemoteQuit, cx),
            WEBRTC_SDP_TIMEOUT => self.on_remote_end(caller_id, EndReason::Timeout, cx),
            WEBRTC_SDP_JOINED_OTHER_CALL => self.on_remote_end(caller_id, EndReason::Busy, cx),
            WEBRTC_SDP_STATUS_REMOTE_MEDIA => self.on_remote_status(caller_id, &fwd.json_data, cx),
            _ => {}
        }
    }

    fn on_call_log_end(&mut self, channel_id: i64, content: &str, cx: &mut Context<Self>) {
        if !matches!(self.phase, CallPhase::Incoming) {
            return;
        }
        if self.peer.as_ref().map(|p| p.channel_id) != Some(channel_id) {
            return;
        }
        let terminal = serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|v| v.get("callLog")?.get("callLogType")?.as_i64())
            .is_some_and(|t| t != CallLogType::StartCall.raw() as i64);
        if terminal {
            tracing::info!("call: incoming ended elsewhere (call-log terminal) -> dismiss");
            self.reset_state();
            cx.notify();
        }
    }

    fn on_call_push(&mut self, channel_id: i64, json: &str, cx: &mut Context<Self>) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        if value.get("offer").and_then(|v| v.as_str()) != Some("CANCEL_CALL") {
            return;
        }
        if matches!(self.phase, CallPhase::Incoming)
            && self.peer.as_ref().map(|p| p.channel_id) == Some(channel_id)
        {
            tracing::info!("call: incoming cancelled/answered elsewhere -> dismiss");
            self.reset_state();
            cx.notify();
        }
    }

    fn on_remote_offer(
        &mut self,
        caller_id: i64,
        channel_id: i64,
        json: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(offer) = decompress_sdp(json)
            .ok()
            .and_then(|data| serde_json::from_str::<OfferPayload>(&data).ok())
        else {
            tracing::warn!("call: failed to decompress/parse remote offer");
            return;
        };
        if matches!(self.phase, CallPhase::Idle) {
            self.generation += 1;
            if let Some((self_id, self_name, self_avatar)) = self_identity(cx) {
                self.self_id = self_id;
                self.self_name = self_name;
                self.self_avatar = self_avatar;
            }
            self.peer = Some(CallPeer {
                user_id: caller_id,
                channel_id,
                name: offer.caller_name,
                avatar: (!offer.caller_avatar.is_empty()).then_some(offer.caller_avatar),
            });
            self.is_caller = false;
            self.incoming_offer = Some(offer.sdp);
            self.media = MediaKind::Audio;
            self.local = MediaFlags::default();
            self.remote = MediaFlags {
                mic_on: true,
                cam_on: false,
            };
            self.connected_at = None;
            self.phase = CallPhase::Incoming;
            self.play_tone(ToneSlot::Ring, RINGING_SOUND, true, cx);
            self.start_timeout(cx);
            cx.notify();
        } else if self.peer.as_ref().map(|p| p.user_id) == Some(caller_id) {
            if let Some(engine) = &self.engine {
                engine.send(EngineCommand::ApplyRemoteOffer(offer.sdp));
            } else {
                tracing::warn!("call: renegotiation offer but no engine");
            }
        } else {
            tracing::info!("call: offer from a different peer -> reply JOINED_OTHER_CALL");
            self.forward(
                caller_id,
                WEBRTC_SDP_JOINED_OTHER_CALL,
                String::new(),
                channel_id,
                cx,
            );
        }
    }

    fn on_remote_answer(&mut self, caller_id: i64, json: &str, cx: &mut Context<Self>) {
        if self.peer.as_ref().map(|p| p.user_id) != Some(caller_id) {
            tracing::warn!("call: answer from unexpected peer {caller_id}; ignored");
            return;
        }
        if !matches!(self.phase, CallPhase::Outgoing | CallPhase::Connecting) {
            tracing::warn!("call: answer ignored (phase={:?})", self.phase);
            return;
        }
        let Some(answer) = decompress_sdp(json)
            .ok()
            .and_then(|data| serde_json::from_str::<AnswerPayload>(&data).ok())
        else {
            tracing::warn!("call: failed to decompress/parse remote answer");
            return;
        };
        if let Some(engine) = &self.engine {
            tracing::info!("call: remote answer -> engine");
            engine.send(EngineCommand::ApplyRemoteAnswer(answer.sdp));
        }
        if matches!(self.phase, CallPhase::Outgoing) {
            self.phase = CallPhase::Connecting;
        }
        self.flush_local_ice(cx);
        self.start_timeout(cx);
        cx.notify();
    }

    fn on_remote_ice(&mut self, caller_id: i64, json: &str) {
        if self.peer.as_ref().map(|p| p.user_id) != Some(caller_id) {
            return;
        }
        let Ok(ice) = serde_json::from_str::<IcePayload>(json) else {
            tracing::warn!("call: unparsable remote ice candidate");
            return;
        };
        match &self.engine {
            Some(engine) => engine.send(EngineCommand::AddRemoteIce(ice)),
            None if self.pending_remote_ice.len() < MAX_PENDING_ICE => {
                self.pending_remote_ice.push(ice)
            }
            None => {}
        }
    }

    fn on_remote_end(&mut self, caller_id: i64, reason: EndReason, cx: &mut Context<Self>) {
        if self.peer.as_ref().map(|p| p.user_id) != Some(caller_id) {
            return;
        }
        self.terminate(reason, false, cx);
    }

    fn on_remote_status(&mut self, caller_id: i64, json: &str, cx: &mut Context<Self>) {
        if self.peer.as_ref().map(|p| p.user_id) != Some(caller_id) {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        if let Some(cam) = value.get("cameraEnabled").and_then(|v| v.as_bool()) {
            self.remote.cam_on = cam;
            if !cam {
                if let Some(store) = &self.frame_store {
                    store.remove(mezon_call::REMOTE_FRAME_KEY);
                }
                self.render_cache
                    .lock()
                    .remove(&mezon_call::REMOTE_FRAME_KEY);
            }
        }
        if let Some(mic) = value.get("micEnabled").and_then(|v| v.as_bool()) {
            self.remote.mic_on = mic;
        }
        cx.notify();
    }

    fn end_call(&mut self, reason: EndReason, cx: &mut Context<Self>) {
        self.terminate(reason, true, cx);
    }

    fn terminate(&mut self, reason: EndReason, notify_peer: bool, cx: &mut Context<Self>) {
        if matches!(self.phase, CallPhase::Idle) {
            return;
        }
        if notify_peer {
            self.send_to_peer(WEBRTC_SDP_QUIT, String::new(), cx);
        }
        if let Some(engine) = &self.engine {
            engine.send(EngineCommand::Hangup);
        }
        self.write_terminal_log(reason, cx);
        if self.is_caller {
            self.send_call_push_cancel(self.connected_at.is_some(), cx);
        }
        match reason {
            EndReason::Busy => self.play_tone(ToneSlot::Busy, BUSYTONE_SOUND, false, cx),
            _ => self.play_tone(ToneSlot::End, ENDCALL_SOUND, false, cx),
        }
        self.reset_state();
        cx.notify();
    }

    fn reset_state(&mut self) {
        self.generation += 1;
        self.phase = CallPhase::Idle;
        self.peer = None;
        self.is_caller = false;
        self.media = MediaKind::Audio;
        self.local = MediaFlags::default();
        self.remote = MediaFlags::default();
        self.mic_prompt = false;
        self.camera_prompt = false;
        self.incoming_offer = None;
        self.pending_remote_ice.clear();
        self.pending_local_ice.clear();
        self.engine = None;
        self.frame_store = None;
        self.connected_at = None;
        self.call_message_id = None;
        self.call_create_time = 0;
        self._events_task = None;
        self._timeout_task = None;
        self._ice_grace_task = None;
        self.tones.stop_dial_ring();
        let mut cache = self.render_cache.lock();
        if !cache.is_empty() {
            let mut drops = self.pending_texture_drops.lock();
            for (_, entry) in cache.drain() {
                match entry.frame {
                    VoiceRenderFrame::Image(image) => drops.push(image),
                    #[cfg(target_os = "macos")]
                    VoiceRenderFrame::Surface(_) => {}
                }
            }
            self.pending_texture_work.store(true, Ordering::Release);
        }
    }

    fn start_timeout(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        self._timeout_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NO_ANSWER_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation
                    && matches!(
                        this.phase,
                        CallPhase::Outgoing | CallPhase::Connecting | CallPhase::Incoming
                    )
                {
                    this.end_call(EndReason::Timeout, cx);
                }
            });
        }));
    }

    fn cancel_timeout(&mut self) {
        self._timeout_task = None;
    }

    fn start_ice_grace(&mut self, cx: &mut Context<Self>) {
        if self._ice_grace_task.is_some() {
            return;
        }
        let generation = self.generation;
        self._ice_grace_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ICE_DISCONNECT_GRACE).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation && this._ice_grace_task.is_some() {
                    this.end_call(EndReason::Failed, cx);
                }
            });
        }));
    }

    fn write_start_call_log(&mut self, is_video: bool, cx: &mut Context<Self>) {
        let Some(peer) = self.peer.clone() else {
            return;
        };
        let api = self.api.clone();
        let content = serde_json::json!({
            "t": format!("Started {} call", if is_video { "video" } else { "voice" }),
            "callLog": { "isVideo": is_video, "callLogType": 1, "showCallBack": false },
        })
        .to_string();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .send_channel_message_structured(peer.channel_id, &content, DM_STREAM_MODE)
                .await;
            let _ = this.update(cx, |this, _| {
                if this.generation != generation {
                    return;
                }
                if let Ok(message) = result {
                    this.call_message_id = Some(message.message_id);
                    this.call_create_time = to_seconds_u32(message.create_time);
                }
            });
        })
        .detach();
    }

    fn write_terminal_log(&self, reason: EndReason, cx: &Context<Self>) {
        if !self.is_caller {
            return;
        }
        let is_video = self.media == MediaKind::Video;
        let connected = self.connected_at.is_some();
        let entry = if connected {
            match reason {
                EndReason::Busy => None,
                _ => {
                    let elapsed = self.connected_at.map(|at| at.elapsed()).unwrap_or_default();
                    let mins = elapsed.as_secs() / 60;
                    let secs = elapsed.as_secs() % 60;
                    Some((
                        CallLogType::FinishCall,
                        format!("Call duration: {mins} mins {secs} secs"),
                        true,
                    ))
                }
            }
        } else {
            match reason {
                EndReason::Timeout | EndReason::Failed => Some((
                    CallLogType::TimeoutCall,
                    format!(
                        "{} call timed out",
                        if is_video { "Video" } else { "Voice" }
                    ),
                    true,
                )),
                EndReason::RemoteQuit => Some((
                    CallLogType::RejectCall,
                    format!("Declined {} call", if is_video { "video" } else { "voice" }),
                    false,
                )),
                EndReason::LocalHangup => Some((CallLogType::CancelCall, String::new(), false)),
                EndReason::Busy => None,
            }
        };
        let Some((log_type, text, show_call_back)) = entry else {
            return;
        };
        let (Some(message_id), Some(peer)) = (self.call_message_id, self.peer.as_ref()) else {
            return;
        };
        let content = serde_json::json!({
            "t": text,
            "callLog": {
                "isVideo": is_video,
                "callLogType": log_type.raw(),
                "showCallBack": show_call_back,
            },
        })
        .to_string();
        let api = self.api.clone();
        let channel_id = peer.channel_id;
        let create_time = self.call_create_time;
        cx.background_executor()
            .spawn(async move {
                let _ = api
                    .update_channel_message_structured(
                        0,
                        channel_id,
                        message_id,
                        content,
                        DM_STREAM_MODE,
                        create_time,
                    )
                    .await;
            })
            .detach();
    }

    fn send_to_peer(&self, data_type: i32, json_data: String, cx: &Context<Self>) {
        let Some(peer) = self.peer.clone() else {
            return;
        };
        self.forward(peer.user_id, data_type, json_data, peer.channel_id, cx);
    }

    fn forward(
        &self,
        receiver_id: i64,
        data_type: i32,
        json_data: String,
        channel_id: i64,
        cx: &Context<Self>,
    ) {
        let api = self.api.clone();
        let caller_id = self.self_id;
        cx.background_executor()
            .spawn(async move {
                let _ = api
                    .forward_webrtc_signaling(
                        receiver_id,
                        data_type,
                        json_data,
                        channel_id,
                        caller_id,
                    )
                    .await;
            })
            .detach();
    }

    fn send_call_push_offer(&self, compressed_offer: String, cx: &Context<Self>) {
        let Some(peer) = self.peer.clone() else {
            return;
        };
        let body = serde_json::json!({
            "offer": compressed_offer,
            "callerName": self.self_name,
            "callerAvatar": self.self_avatar,
            "callerId": self.self_id.to_string(),
            "isVideoCall": self.media == MediaKind::Video,
            "channelId": peer.channel_id.to_string(),
            "sentAt": now_ms().to_string(),
        })
        .to_string();
        let api = self.api.clone();
        let caller_id = self.self_id;
        cx.background_executor()
            .spawn(async move {
                let _ = api
                    .make_call_push(peer.user_id, body, peer.channel_id, caller_id)
                    .await;
            })
            .detach();
    }

    fn send_call_push_cancel(&self, is_connected: bool, cx: &Context<Self>) {
        let Some(peer) = self.peer.clone() else {
            return;
        };
        let original_caller = if self.is_caller {
            self.self_id
        } else {
            peer.user_id
        };
        let (caller_name, caller_avatar) = if self.is_caller {
            (self.self_name.clone(), self.self_avatar.clone())
        } else {
            (peer.name.clone(), peer.avatar.clone().unwrap_or_default())
        };
        let body = serde_json::json!({
            "offer": "CANCEL_CALL",
            "isConnected": is_connected,
            "isVideo": self.media == MediaKind::Video,
            "callerName": caller_name,
            "callerAvatar": caller_avatar,
            "callerId": original_caller.to_string(),
            "channelId": peer.channel_id.to_string(),
            "sentAt": now_ms().to_string(),
        })
        .to_string();
        let api = self.api.clone();
        let channel_id = peer.channel_id;
        cx.background_executor()
            .spawn(async move {
                let _ = api
                    .make_call_push(peer.user_id, body, channel_id, original_caller)
                    .await;
            })
            .detach();
    }

    fn play_tone(
        &mut self,
        slot: ToneSlot,
        bytes: &'static [u8],
        looping: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn(async move { mezon_audio::decode_audio(bytes.to_vec()) })
                .await;
            let _ = this.update(cx, |this, _| {
                let Ok(pcm) = decoded else {
                    return;
                };
                let allowed = match slot {
                    ToneSlot::Dial => matches!(this.phase, CallPhase::Outgoing),
                    ToneSlot::Ring => matches!(this.phase, CallPhase::Incoming),
                    ToneSlot::End | ToneSlot::Busy => true,
                };
                if !allowed {
                    return;
                }
                let Ok(player) = AudioPlayer::new() else {
                    return;
                };
                player.set_data(pcm);
                if looping {
                    player.play_looping();
                } else {
                    player.play();
                }
                match slot {
                    ToneSlot::Dial => this.tones.dial = Some(player),
                    ToneSlot::Ring => this.tones.ring = Some(player),
                    ToneSlot::End => this.tones.end = Some(player),
                    ToneSlot::Busy => this.tones.busy = Some(player),
                }
            });
        })
        .detach();
    }
}

fn self_identity(cx: &App) -> Option<(i64, String, String)> {
    let entity = AccountStore::try_global(cx)?;
    let store = entity.read(cx);
    let account = store.account.as_ref()?;
    let name = if account.display_name.is_empty() {
        account.username.clone()
    } else {
        account.display_name.clone()
    };
    let avatar = account.avatar_url.clone().unwrap_or_default();
    Some((account.user_id, name, avatar))
}

fn ice_servers(cx: &App) -> Vec<IceServerConfig> {
    let config = AppConfig::global(cx);
    let mut servers = vec![IceServerConfig {
        urls: vec!["stun:stun.l.google.com:19302".into()],
        username: String::new(),
        credential: String::new(),
    }];
    if !config.webrtc_ice_servers_url.is_empty() && !config.webrtc_ice_servers_credential.is_empty()
    {
        servers.push(IceServerConfig {
            urls: vec![config.webrtc_ice_servers_url.clone()],
            username: config.webrtc_ice_servers_username.clone(),
            credential: config.webrtc_ice_servers_credential.clone(),
        });
    }
    servers
}

fn to_seconds_u32(create_time: i64) -> u32 {
    let seconds = if create_time > 4_000_000_000 {
        create_time / 1000
    } else {
        create_time
    };
    seconds.clamp(0, u32::MAX as i64) as u32
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}
