mod audio;
mod camera;
pub mod compose;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux_session;
mod record;
mod runtime;
mod screen;
mod screen_picker;
mod screen_previews;
mod screen_targets;
mod stream_playback;
mod video;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use mezon_codec::{
    AudioFrame, I420Frame, OpusDecoder, OpusEncoder, SvcConfig, VpxCodec, VpxDecoder, VpxEncoder,
};
use mezon_rtc::codecs::{PT_OPUS, PT_VP8, PT_VP9};
use mezon_rtc::{
    LocalAudio, LocalVideo, PeerConnectionOpts, RemoteAudio, RemoteVideo, RemoteVideoKind,
    RtcSession,
};
use mezon_sfu_client::{SfuClient, SfuClientEvent, SfuConfig, TungsteniteTransport};

pub use audio::{AudioFormat, AudioIo, DeviceResetKind, PlaybackMixer};
pub use mezon_record::{RecordError, RecordStats};
pub use record::{
    RECORD_FPS, RECORD_HEIGHT, RECORD_WIDTH, RecordSession, RecordStarter, RecordTaps,
};

pub fn record_supported() -> bool {
    mezon_record::is_supported()
}

pub fn record_file_extension() -> &'static str {
    mezon_record::file_extension()
}
pub use camera::{
    CameraController, CameraDeviceInfo, camera_denied, enumerate_cameras, start_camera_into,
};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use linux_session::record_wayland_session;
pub use stream_playback::StreamAudioOutput;

pub fn microphone_denied() -> bool {
    audio::microphone_denied()
}
pub use screen_picker::{PickedScreen, system_screen_share_pick};
pub use screen_previews::{ScreenSharePreview, capture_screen_share_preview};
pub use screen_targets::{
    ScreenShareKind, ScreenShareListError, ScreenShareOption, list_screen_share_options,
    peek_screen_share_options,
};
#[cfg(target_os = "macos")]
pub use video::VideoSurface;
pub use video::{VideoFrameData, VideoFrameStore, i420_to_bgra_into, local_camera_key};

use crate::screen::ScreenStopper;
use crate::video::{local_screen_key, remote_camera_key, remote_screen_key, track_frame_key};

const MIC_SSRC: u32 = 0x1111_1111;
const CAMERA_SSRC: u32 = 0x2222_2222;
const SCREEN_SSRC: u32 = 0x3333_3333;

const OPUS_BITRATE_BPS: i32 = 32_000;
const CAMERA_BITRATE_KBPS: u32 = 1_200;
const SCREEN_BITRATE_KBPS: u32 = 2_500;

const OPUS_FRAME_SAMPLES: usize = 960;
const OPUS_FRAME_DURATION: Duration = Duration::from_millis(20);
const OPUS_SAMPLE_RATE: u32 = 48_000;

const VIDEO_FRAME_DURATION: Duration = Duration::from_millis(33);
const VIDEO_PTS_STEP: i64 = 3_000;

const I16_TO_F32: f32 = 1.0 / 32_768.0;

fn remote_audio_key(user_id: &str) -> u64 {
    track_frame_key(user_id, "__mezon_remote_audio__")
}

#[derive(Clone, Debug, Default)]
pub struct IceServerConfig {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetworkQuality {
    Excellent,
    Good,
    Poor,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceParticipant {
    pub identity: String,
    pub name: String,
    pub is_local: bool,
    pub is_agent: bool,
    pub speaking: bool,
    pub muted: bool,
    pub camera: Option<u64>,
    pub screenshare: Option<u64>,
    pub quality: NetworkQuality,
}

#[derive(Clone, Debug)]
pub enum VoiceEvent {
    Connected { room_name: String },
    Reconnecting,
    Reconnected,
    NetworkWeak,
    NetworkRecovered,
    DeviceResetToDefault { input: bool },
    Disconnected { reason: String },
    Participants(Vec<VoiceParticipant>),
    Error(String),
}

enum Command {
    SetMicEnabled(bool),
    SetRole(String),
    PushToTalk,
    SetCameraEnabled(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetCameraDevice(Option<String>),
    SetNoiseSuppression(bool, u8),
    StartScreenShare(PickedScreen, bool),
    StopScreenShare,
    Disconnect,
}

#[derive(Clone, Debug)]
pub struct VoiceConnectConfig {
    pub role: String,
    pub video_codec: String,
    pub svc_mode: String,
    pub local_identity: String,
}

impl Default for VoiceConnectConfig {
    fn default() -> Self {
        Self {
            role: "speaker".to_string(),
            video_codec: "vp9".to_string(),
            svc_mode: "l3t3".to_string(),
            local_identity: String::new(),
        }
    }
}

pub struct VoiceSession {
    cmd_tx: flume::Sender<Command>,
    events: flume::Receiver<VoiceEvent>,
    frame_store: Arc<VideoFrameStore>,
    screen_full_res: Arc<AtomicBool>,
    record_taps: record::RecordTaps,
    record_scene: compose::Scene,
    record: Arc<parking_lot::RwLock<Option<record::RecordSession>>>,
}

impl VoiceSession {
    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        url: String,
        token: String,
        room: String,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        camera_device_id: Option<String>,
        ice_servers: Vec<IceServerConfig>,
        config: VoiceConnectConfig,
    ) -> Self {
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (evt_tx, evt_rx) = flume::unbounded();
        let frame_store = Arc::new(VideoFrameStore::default());
        let screen_full_res = Arc::new(AtomicBool::new(false));

        let store = frame_store.clone();
        let screen_full_res_task = screen_full_res.clone();
        let record_taps = record::RecordTaps::default();
        let record_taps_task = record_taps.clone();
        runtime::runtime().spawn(async move {
            if let Err(e) = session_main(
                url,
                token,
                room,
                input_device_id,
                output_device_id,
                camera_device_id,
                ice_servers,
                config,
                cmd_rx,
                &evt_tx,
                store,
                screen_full_res_task,
                record_taps_task,
            )
            .await
            {
                tracing::error!("voice session ended with error: {e:#}");
                let _ = evt_tx.send(VoiceEvent::Error(e.to_string()));
                let _ = evt_tx.send(VoiceEvent::Disconnected {
                    reason: e.to_string(),
                });
            }
        });

        Self {
            cmd_tx,
            events: evt_rx,
            frame_store,
            screen_full_res,
            record_taps,
            record_scene: compose::Scene::default(),
            record: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    pub fn record_starter(&self) -> record::RecordStarter {
        record::RecordStarter::new(self.record_taps.clone(), self.record.clone())
    }

    pub fn record_scene(&self) -> compose::Scene {
        self.record_scene.clone()
    }

    pub fn record_frame_source(&self) -> (compose::Scene, Arc<VideoFrameStore>) {
        (self.record_scene.clone(), self.frame_store.clone())
    }

    pub fn take_recording(&self) -> Option<record::RecordSession> {
        self.record.write().take()
    }

    pub fn recording_stats(&self) -> Option<mezon_record::RecordStats> {
        self.record.read().as_ref().map(|session| session.stats())
    }

    pub fn recording_failed(&self) -> bool {
        self.record
            .read()
            .as_ref()
            .is_some_and(|session| session.failed())
    }

    pub fn recording_video_unavailable(&self) -> bool {
        self.record
            .read()
            .as_ref()
            .is_some_and(|session| session.video_unavailable())
    }

    pub fn events(&self) -> flume::Receiver<VoiceEvent> {
        self.events.clone()
    }

    pub fn frame_store(&self) -> Arc<VideoFrameStore> {
        self.frame_store.clone()
    }

    pub fn set_mic_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(Command::SetMicEnabled(enabled));
    }

    pub fn set_role(&self, role: String) {
        let _ = self.cmd_tx.send(Command::SetRole(role));
    }

    pub fn push_to_talk(&self) {
        let _ = self.cmd_tx.send(Command::PushToTalk);
    }

    pub fn set_camera_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.send(Command::SetCameraEnabled(enabled));
    }

    pub fn set_input_device(&self, device_id: Option<String>) {
        let _ = self.cmd_tx.send(Command::SetInputDevice(device_id));
    }

    pub fn set_output_device(&self, device_id: Option<String>) {
        let _ = self.cmd_tx.send(Command::SetOutputDevice(device_id));
    }

    pub fn set_camera_device(&self, device_id: Option<String>) {
        let _ = self.cmd_tx.send(Command::SetCameraDevice(device_id));
    }

    pub fn set_noise_suppression(&self, enabled: bool, level: u8) {
        let _ = self
            .cmd_tx
            .send(Command::SetNoiseSuppression(enabled, level));
    }

    pub fn start_screen_share(&self, pick: PickedScreen, share_audio: bool) {
        let _ = self
            .cmd_tx
            .send(Command::StartScreenShare(pick, share_audio));
    }

    pub fn stop_screen_share(&self) {
        let _ = self.cmd_tx.send(Command::StopScreenShare);
    }

    pub fn set_screen_full_res(&self, full_res: bool) {
        self.screen_full_res.store(full_res, Ordering::Relaxed);
    }
}

impl Drop for VoiceSession {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Disconnect);
    }
}

struct CameraSession {
    controller: CameraController,
    encode_task: tokio::task::JoinHandle<()>,
}

impl CameraSession {
    fn stop(self) {
        self.controller.stop();
        self.encode_task.abort();
    }
}

struct ScreenSession {
    stopper: ScreenStopper,
    encode_task: tokio::task::JoinHandle<()>,
}

impl ScreenSession {
    fn stop(self) {
        self.stopper.stop();
        self.encode_task.abort();
    }
}

#[derive(Default)]
struct RemoteRoster {
    peers: BTreeSet<String>,
}

#[derive(Default, Clone, Copy)]
struct RemoteTimes {
    audio_ms: u64,
    speaking_ms: u64,
    camera_ms: u64,
    screen_ms: u64,
}

#[derive(Default)]
struct RemoteMediaState {
    inner: parking_lot::Mutex<HashMap<String, RemoteTimes>>,
}

impl RemoteMediaState {
    fn mark_audio(&self, user: &str, now: u64, speaking: bool) {
        let mut m = self.inner.lock();
        let t = m.entry(user.to_string()).or_default();
        t.audio_ms = now;
        if speaking {
            t.speaking_ms = now;
        }
    }

    fn mark_video(&self, user: &str, kind: RemoteVideoKind, now: u64) {
        let mut m = self.inner.lock();
        let t = m.entry(user.to_string()).or_default();
        match kind {
            RemoteVideoKind::Camera => t.camera_ms = now,
            RemoteVideoKind::Screen => t.screen_ms = now,
        }
    }

    fn snapshot(&self) -> HashMap<String, RemoteTimes> {
        self.inner.lock().clone()
    }
}

fn lifecycle_event(event: &SfuClientEvent, room: &str) -> Option<VoiceEvent> {
    match event {
        SfuClientEvent::Connected => Some(VoiceEvent::Connected {
            room_name: room.to_string(),
        }),
        SfuClientEvent::Disconnected => Some(VoiceEvent::Disconnected {
            reason: "disconnected".to_string(),
        }),
        SfuClientEvent::Error(message) => Some(VoiceEvent::Error(message.clone())),
        _ => None,
    }
}

fn apply_roster_event(roster: &mut RemoteRoster, event: &SfuClientEvent, self_id: &str) -> bool {
    match event {
        SfuClientEvent::RoleChanged { user_id, .. } | SfuClientEvent::PeerJoined { user_id }
            if user_id == self_id =>
        {
            false
        }
        SfuClientEvent::RoleChanged { user_id, .. } | SfuClientEvent::PeerJoined { user_id } => {
            roster.peers.insert(user_id.clone())
        }
        SfuClientEvent::PeerLeft { user_id } => roster.peers.remove(user_id),
        SfuClientEvent::RoomSnapshot { user_ids } => {
            let mut changed = false;
            for uid in user_ids {
                if uid != self_id {
                    changed |= roster.peers.insert(uid.clone());
                }
            }
            changed
        }
        _ => false,
    }
}

const MEDIA_HANGOVER_MS: u64 = 1_000;

fn media_recent(last_ms: u64, now_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) < MEDIA_HANGOVER_MS
}

const SPEAK_TICK_MS: u64 = 120;
const SPEAK_HANGOVER_MS: u64 = 250;
const MIC_SPEAK_RMS: f32 = 600.0;
const REMOTE_SPEAK_RMS: f32 = 0.02;

fn recent_voice(last_ms: u64, now_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) < SPEAK_HANGOVER_MS
}

fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[allow(clippy::too_many_arguments)]
fn build_participants(
    local_identity: &str,
    mic_on: bool,
    camera_on: bool,
    screen_on: bool,
    connected: bool,
    local_speaking: bool,
    peers: &BTreeSet<String>,
    media: &HashMap<String, RemoteTimes>,
    now_ms: u64,
) -> Vec<VoiceParticipant> {
    let quality = if connected {
        NetworkQuality::Good
    } else {
        NetworkQuality::Unknown
    };

    let mut participants = vec![VoiceParticipant {
        identity: local_identity.to_string(),
        name: local_identity.to_string(),
        is_local: true,
        is_agent: false,
        speaking: local_speaking && mic_on,
        muted: !mic_on,
        camera: camera_on.then(|| local_camera_key(local_identity)),
        screenshare: screen_on.then(|| local_screen_key(local_identity)),
        quality,
    }];

    let mut remotes: BTreeSet<&String> = peers.iter().collect();
    remotes.extend(media.keys());
    for user in remotes {
        if user == local_identity {
            continue;
        }
        let times = media.get(user).copied().unwrap_or_default();
        participants.push(VoiceParticipant {
            camera: media_recent(times.camera_ms, now_ms).then(|| remote_camera_key(user)),
            screenshare: media_recent(times.screen_ms, now_ms).then(|| remote_screen_key(user)),
            name: user.clone(),
            identity: user.clone(),
            is_local: false,
            is_agent: false,
            speaking: recent_voice(times.speaking_ms, now_ms),
            muted: !media_recent(times.audio_ms, now_ms),
            quality,
        });
    }
    participants
}

#[allow(clippy::too_many_arguments)]
async fn session_main(
    url: String,
    token: String,
    room: String,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    ice_servers: Vec<IceServerConfig>,
    config: VoiceConnectConfig,
    cmd_rx: flume::Receiver<Command>,
    evt_tx: &flume::Sender<VoiceEvent>,
    frame_store: Arc<VideoFrameStore>,
    screen_full_res: Arc<AtomicBool>,
    session_record_taps: record::RecordTaps,
) -> Result<()> {
    let _ = ice_servers;

    let (video_codec, video_pt) = match config.video_codec.as_str() {
        "vp8" => (VpxCodec::Vp8, PT_VP8),
        _ => (VpxCodec::Vp9, PT_VP9),
    };
    let svc = if video_codec == VpxCodec::Vp9 {
        match config.svc_mode.as_str() {
            "l3t3" => Some(SvcConfig {
                spatial_layers: 3,
                temporal_layers: 3,
                ksvc: false,
            }),
            "l3t3_key" => Some(SvcConfig {
                spatial_layers: 3,
                temporal_layers: 3,
                ksvc: true,
            }),
            "l2t2" => Some(SvcConfig {
                spatial_layers: 2,
                temporal_layers: 2,
                ksvc: false,
            }),
            "l1t3" => Some(SvcConfig {
                spatial_layers: 1,
                temporal_layers: 3,
                ksvc: false,
            }),
            "l1t2" => Some(SvcConfig {
                spatial_layers: 1,
                temporal_layers: 2,
                ksvc: false,
            }),
            _ => None,
        }
    } else {
        None
    };
    tracing::info!(
        "voice video codec={:?} pt={} svc={:?}",
        video_codec,
        video_pt,
        svc
    );

    let rtc = RtcSession::new(PeerConnectionOpts { loopback: false }).await?;
    let audio_pub = rtc.publish_audio(MIC_SSRC, PT_OPUS).await?;
    let camera_pub = rtc
        .publish_video(CAMERA_SSRC, video_pt, video_codec)
        .await?;
    let screen_pub = rtc
        .publish_video(SCREEN_SSRC, video_pt, video_codec)
        .await?;
    let audio_packets = rtc.subscribe_audio();
    let video_frames = rtc.subscribe_video();

    let local_identity = if config.local_identity.is_empty() {
        "local".to_string()
    } else {
        config.local_identity.clone()
    };
    let sfu_user_id = (!config.local_identity.is_empty()).then(|| config.local_identity.clone());
    let self_sfu_id = sfu_user_id.clone().unwrap_or_else(|| "0".to_string());

    let audio = tokio::task::spawn_blocking(move || {
        audio::AudioIo::start(input_device_id, output_device_id, session_record_taps)
    })
    .await
    .map_err(|e| anyhow!("audio init task failed: {e}"))?;

    let mic_enabled = Arc::new(AtomicBool::new(false));
    let out_channels = Arc::new(AtomicU32::new(2));
    let mut audio_io: Option<audio::AudioIo> = None;
    let mut out_change_rx: Option<flume::Receiver<AudioFormat>> = None;
    let mut device_reset_rx: Option<flume::Receiver<audio::DeviceResetKind>> = None;
    let mut mic_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut audio_decode_task: Option<tokio::task::JoinHandle<()>> = None;

    let local_voice_ms = Arc::new(AtomicU64::new(0));
    let speak_epoch = Instant::now();
    let remote_media = Arc::new(RemoteMediaState::default());

    match audio {
        Ok(io) => {
            out_channels.store(io.output_format.channels.max(1), Ordering::Relaxed);
            out_change_rx = Some(io.output_format_rx.clone());
            device_reset_rx = Some(io.device_reset_rx.clone());

            mic_task = Some(runtime::runtime().spawn(publish_microphone(
                io.mic_rx.clone(),
                io.input_format_rx.clone(),
                mic_enabled.clone(),
                audio_pub.clone(),
                local_voice_ms.clone(),
                speak_epoch,
            )));

            audio_decode_task = Some(runtime::runtime().spawn(play_remote_audio(
                audio_packets,
                io.mixer.clone(),
                out_channels.clone(),
                remote_media.clone(),
                speak_epoch,
            )));

            audio_io = Some(io);
        }
        Err(e) => {
            tracing::error!("voice audio unavailable: {e:#}");
            let _ = evt_tx.send(VoiceEvent::Error(format!(
                "audio unavailable (no microphone or playback): {e}"
            )));
        }
    }

    let video_decode = spawn_remote_video(
        video_frames,
        frame_store.clone(),
        remote_media.clone(),
        speak_epoch,
    );

    let connect_url = if url.contains('?') {
        format!("{url}&access_token={token}")
    } else {
        format!("{url}?access_token={token}")
    };
    let transport = TungsteniteTransport::connect(&connect_url).await?;
    let config = SfuConfig {
        ws_url: url.clone(),
        room: room.clone(),
        role: config.role.clone(),
        token: Some(token),
        user_id: sfu_user_id,
    };
    let client = SfuClient::start(config, transport, rtc);
    let sfu_events = client.events();

    let mut roster = RemoteRoster::default();
    let mut mic_on = false;
    let mut camera_on = false;
    let mut screen_on = false;
    let mut connected = false;
    let mut local_speaking = false;
    let mut camera_device_id = camera_device_id;
    let mut camera_session: Option<CameraSession> = None;
    let mut screen_session: Option<ScreenSession> = None;
    let mut camera_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut screen_task: Option<tokio::task::JoinHandle<()>> = None;
    let (cam_tx, cam_rx) = flume::bounded::<Result<CameraSession>>(1);
    let (screen_tx, screen_rx) = flume::bounded::<Result<ScreenSession>>(1);
    let mut last_participants: Vec<VoiceParticipant> = Vec::new();

    let emit = |mic: bool,
                camera: bool,
                screen: bool,
                connected: bool,
                local_speaking: bool,
                peers: &BTreeSet<String>,
                last: &mut Vec<VoiceParticipant>| {
        let now = speak_epoch.elapsed().as_millis() as u64;
        let media = remote_media.snapshot();
        let current = build_participants(
            &local_identity,
            mic,
            camera,
            screen,
            connected,
            local_speaking,
            peers,
            &media,
            now,
        );
        if *last != current {
            *last = current.clone();
            let _ = evt_tx.send(VoiceEvent::Participants(current));
        }
    };
    emit(
        mic_on,
        camera_on,
        screen_on,
        connected,
        local_speaking,
        &roster.peers,
        &mut last_participants,
    );

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let trim_task = runtime::runtime().spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            unsafe { libc::malloc_trim(0) };
        }
    });

    let mut speak_tick = tokio::time::interval(Duration::from_millis(SPEAK_TICK_MS));
    speak_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = speak_tick.tick() => {
                let now = speak_epoch.elapsed().as_millis() as u64;
                local_speaking = mic_on && recent_voice(local_voice_ms.load(Ordering::Relaxed), now);
                emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
            }
            event = sfu_events.recv_async() => {
                let Ok(event) = event else { break };
                if let Some(voice_event) = lifecycle_event(&event, &room) {
                    match event {
                        SfuClientEvent::Connected => connected = true,
                        SfuClientEvent::Disconnected => connected = false,
                        _ => {}
                    }
                    let is_disconnect = matches!(event, SfuClientEvent::Disconnected);
                    let _ = evt_tx.send(voice_event);
                    emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    if is_disconnect {
                        break;
                    }
                } else if apply_roster_event(&mut roster, &event, &self_sfu_id) {
                    emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                }
            }
            command = cmd_rx.recv_async() => {
                match command {
                    Ok(Command::SetMicEnabled(enabled)) => {
                        mic_on = enabled;
                        mic_enabled.store(enabled, Ordering::Relaxed);
                        if let Some(io) = &audio_io {
                            io.set_input_active(enabled);
                        }
                        emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    }
                    Ok(Command::SetRole(role)) => {
                        if let Err(e) = client.set_role(role) {
                            tracing::warn!("voice set_role failed: {e}");
                        }
                    }
                    Ok(Command::PushToTalk) => {
                        if let Err(e) = client.push_to_talk() {
                            tracing::warn!("voice push_to_talk failed: {e}");
                        }
                    }
                    Ok(Command::SetNoiseSuppression(enabled, level)) => {
                        if let Some(io) = &audio_io {
                            io.set_noise_suppression(enabled, level);
                        }
                    }
                    Ok(Command::SetInputDevice(id)) => {
                        if let Some(io) = &audio_io {
                            io.set_input_device(id);
                        }
                    }
                    Ok(Command::SetOutputDevice(id)) => {
                        if let Some(io) = &audio_io {
                            io.set_output_device(id);
                        }
                    }
                    Ok(Command::SetCameraDevice(id)) => {
                        if camera_device_id != id {
                            camera_device_id = id;
                            if let Some(session) = &camera_session {
                                session.controller.switch(camera_device_id.clone());
                            }
                        }
                    }
                    Ok(Command::SetCameraEnabled(true)) => {
                        if camera_session.is_none() && camera_task.is_none() {
                            let identity = local_identity.clone();
                            let store = frame_store.clone();
                            let publisher = camera_pub.clone();
                            let device = camera_device_id.clone();
                            let tx = cam_tx.clone();
                            camera_task = Some(runtime::runtime().spawn(async move {
                                let result = start_camera_track(
                                    publisher,
                                    identity,
                                    store,
                                    device,
                                    video_codec,
                                    svc,
                                )
                                .await;
                                let _ = tx.send_async(result).await;
                            }));
                        }
                    }
                    Ok(Command::SetCameraEnabled(false)) => {
                        if let Some(task) = camera_task.take() {
                            task.abort();
                        }
                        if let Some(session) = camera_session.take() {
                            session.stop();
                            frame_store.remove(local_camera_key(&local_identity));
                        }
                        camera_on = false;
                        emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    }
                    Ok(Command::StartScreenShare(pick, share_audio)) => {
                        if screen_session.is_none() && screen_task.is_none() {
                            screen_full_res.store(false, Ordering::Relaxed);
                            if share_audio {
                                tracing::warn!(
                                    "voice screen-audio sharing is not yet supported by the SFU engine"
                                );
                                let _ = evt_tx.send(VoiceEvent::Error(
                                    "screen audio: sharing system audio is not supported yet"
                                        .to_owned(),
                                ));
                            }
                            let identity = local_identity.clone();
                            let store = frame_store.clone();
                            let publisher = screen_pub.clone();
                            let full_res = screen_full_res.clone();
                            let tx = screen_tx.clone();
                            screen_task = Some(runtime::runtime().spawn(async move {
                                let result = start_screen_track(
                                    publisher,
                                    identity,
                                    store,
                                    full_res,
                                    pick,
                                    video_codec,
                                    svc,
                                )
                                .await;
                                let _ = tx.send_async(result).await;
                            }));
                        }
                    }
                    Ok(Command::StopScreenShare) => {
                        if let Some(task) = screen_task.take() {
                            task.abort();
                        }
                        if let Some(session) = screen_session.take() {
                            session.stop();
                            frame_store.remove(local_screen_key(&local_identity));
                        }
                        screen_on = false;
                        emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    }
                    Ok(Command::Disconnect) | Err(_) => {
                        let _ = evt_tx.send(VoiceEvent::Disconnected {
                            reason: "left".into(),
                        });
                        break;
                    }
                }
            }
            result = cam_rx.recv_async() => {
                match result {
                    Ok(Ok(session)) if camera_task.is_some() => {
                        camera_task = None;
                        camera_session = Some(session);
                        camera_on = true;
                        emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    }
                    Ok(Ok(session)) => {
                        session.stop();
                        frame_store.remove(local_camera_key(&local_identity));
                    }
                    Ok(Err(e)) => {
                        camera_task = None;
                        tracing::warn!("camera enable failed: {e:#}");
                        let _ = evt_tx.send(VoiceEvent::Error(format!("camera: {e}")));
                    }
                    Err(_) => {}
                }
            }
            result = screen_rx.recv_async() => {
                match result {
                    Ok(Ok(session)) if screen_task.is_some() => {
                        screen_task = None;
                        screen_session = Some(session);
                        screen_on = true;
                        emit(mic_on, camera_on, screen_on, connected, local_speaking, &roster.peers, &mut last_participants);
                    }
                    Ok(Ok(session)) => {
                        session.stop();
                        frame_store.remove(local_screen_key(&local_identity));
                    }
                    Ok(Err(e)) => {
                        screen_task = None;
                        tracing::warn!("screen share enable failed: {e:#}");
                        let _ = evt_tx.send(VoiceEvent::Error(format!("screen: {e}")));
                    }
                    Err(_) => {}
                }
            }
            reset = recv_device_reset(&device_reset_rx) => {
                if let Some(kind) = reset {
                    let _ = evt_tx.send(VoiceEvent::DeviceResetToDefault {
                        input: matches!(kind, audio::DeviceResetKind::Input),
                    });
                }
            }
            change = recv_output_change(&out_change_rx) => {
                if let Some(fmt) = change {
                    out_channels.store(fmt.channels.max(1), Ordering::Relaxed);
                }
            }
        }
    }

    if let Some(task) = camera_task.take() {
        task.abort();
    }
    if let Some(task) = screen_task.take() {
        task.abort();
    }
    if let Some(session) = camera_session.take() {
        session.stop();
    }
    if let Some(session) = screen_session.take() {
        session.stop();
    }
    if let Some(task) = mic_task.take() {
        task.abort();
    }
    if let Some(task) = audio_decode_task.take() {
        task.abort();
    }
    video_decode.abort();
    client.close();
    shutdown_audio_io(&mut audio_io).await;

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    trim_task.abort();

    Ok(())
}

async fn publish_microphone(
    mic_rx: flume::Receiver<Vec<i16>>,
    input_format_rx: flume::Receiver<AudioFormat>,
    mic_enabled: Arc<AtomicBool>,
    publisher: Arc<LocalAudio>,
    voice_ms: Arc<AtomicU64>,
    speak_epoch: Instant,
) {
    let mut encoder = match OpusEncoder::new(1, OPUS_BITRATE_BPS) {
        Ok(encoder) => encoder,
        Err(e) => {
            tracing::error!("voice opus encoder unavailable: {e}");
            return;
        }
    };
    let mut resampler = MicResampler::new();
    loop {
        tokio::select! {
            biased;
            fmt = input_format_rx.recv_async() => {
                let Ok(fmt) = fmt else { break };
                resampler.set_source(fmt.sample_rate, fmt.channels);
            }
            captured = mic_rx.recv_async() => {
                let Ok(samples) = captured else { break };
                if !mic_enabled.load(Ordering::Relaxed) {
                    resampler.reset();
                    continue;
                }
                if rms_i16(&samples) > MIC_SPEAK_RMS {
                    voice_ms.store(speak_epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
                resampler.push(&samples);
                while let Some(frame) = resampler.take_frame() {
                    match encoder.encode(AudioFrame { samples: &frame, channels: 1 }) {
                        Ok(packet) => {
                            if publisher
                                .write_encoded(&packet, OPUS_FRAME_DURATION)
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => tracing::warn!("voice opus encode failed: {e}"),
                    }
                }
            }
        }
    }
}

async fn play_remote_audio(
    audio_packets: flume::Receiver<RemoteAudio>,
    mixer: Arc<audio::PlaybackMixer>,
    out_channels: Arc<AtomicU32>,
    remote_media: Arc<RemoteMediaState>,
    speak_epoch: Instant,
) {
    let mut decoders: HashMap<String, OpusDecoder> = HashMap::new();
    let mut interleaved: Vec<i16> = Vec::new();
    while let Ok(RemoteAudio { user_id, opus }) = audio_packets.recv_async().await {
        let decoder = if let Some(decoder) = decoders.get_mut(&user_id) {
            decoder
        } else {
            match OpusDecoder::new(1) {
                Ok(decoder) => decoders.entry(user_id.clone()).or_insert(decoder),
                Err(e) => {
                    tracing::error!("voice opus decoder unavailable: {e}");
                    continue;
                }
            }
        };
        match decoder.decode(&opus) {
            Ok(mono) => {
                let now = speak_epoch.elapsed().as_millis() as u64;
                remote_media.mark_audio(&user_id, now, rms_f32(&mono) > REMOTE_SPEAK_RMS);
                let channels = out_channels.load(Ordering::Relaxed).max(1) as usize;
                interleaved.clear();
                interleaved.reserve(mono.len() * channels);
                for sample in mono {
                    let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    for _ in 0..channels {
                        interleaved.push(value);
                    }
                }
                mixer.push(remote_audio_key(&user_id), &interleaved);
            }
            Err(e) => tracing::warn!("voice opus decode failed: {e}"),
        }
    }
    for user_id in decoders.keys() {
        mixer.remove(remote_audio_key(user_id));
    }
}

fn spawn_remote_video(
    video_frames: flume::Receiver<RemoteVideo>,
    frame_store: Arc<VideoFrameStore>,
    remote_media: Arc<RemoteMediaState>,
    speak_epoch: Instant,
) -> tokio::task::JoinHandle<()> {
    runtime::runtime().spawn(async move {
        let mut decoders: HashMap<(String, RemoteVideoKind), VpxDecoder> = HashMap::new();
        let mut bgra: Vec<u8> = Vec::new();
        while let Ok(RemoteVideo {
            user_id,
            kind,
            codec,
            frame,
        }) = video_frames.recv_async().await
        {
            let decoder = if let Some(decoder) = decoders.get_mut(&(user_id.clone(), kind)) {
                decoder
            } else {
                match VpxDecoder::new(codec) {
                    Ok(decoder) => decoders.entry((user_id.clone(), kind)).or_insert(decoder),
                    Err(e) => {
                        tracing::error!("voice vpx decoder unavailable: {e}");
                        continue;
                    }
                }
            };
            let images = match decoder.decode(&frame.data) {
                Ok(images) => images,
                Err(e) => {
                    tracing::warn!("voice vpx decode failed: {e}");
                    continue;
                }
            };
            let now = speak_epoch.elapsed().as_millis() as u64;
            remote_media.mark_video(&user_id, kind, now);
            let key = match kind {
                RemoteVideoKind::Camera => remote_camera_key(&user_id),
                RemoteVideoKind::Screen => remote_screen_key(&user_id),
            };
            for image in images {
                let w = image.width as usize;
                let h = image.height as usize;
                bgra.resize(w * h * 4, 0);
                i420_to_bgra_into(
                    &mut bgra,
                    &image.y,
                    &image.u,
                    &image.v,
                    image.y_stride as usize,
                    image.u_stride as usize,
                    image.v_stride as usize,
                    w,
                    h,
                );
                if let Some(recycled) =
                    frame_store.publish(key, image.width, image.height, std::mem::take(&mut bgra))
                {
                    bgra = recycled;
                }
            }
        }
    })
}

fn svc_layer_kbps(total_kbps: u32, spatial_layers: u8) -> Vec<u32> {
    match spatial_layers {
        0 | 1 => vec![total_kbps],
        2 => vec![total_kbps * 3 / 8, total_kbps - total_kbps * 3 / 8],
        _ => {
            let base = total_kbps / 4;
            vec![base, base, total_kbps - 2 * base]
        }
    }
}

async fn encode_video_loop(
    frame_rx: flume::Receiver<I420Frame>,
    publisher: Arc<LocalVideo>,
    bitrate_kbps: u32,
    codec: VpxCodec,
    svc: Option<SvcConfig>,
) {
    let mut encoder: Option<VpxEncoder> = None;
    let mut dims: (u32, u32) = (0, 0);
    let mut pts: i64 = 0;
    while let Ok(frame) = frame_rx.recv_async().await {
        let mut force_keyframe = false;
        if encoder.is_none() || dims != (frame.width, frame.height) {
            match VpxEncoder::new(codec, frame.width, frame.height, bitrate_kbps) {
                Ok(mut new_encoder) => {
                    if let Some(svc_cfg) = svc
                        && (svc_cfg.spatial_layers >= 2 || svc_cfg.temporal_layers >= 2)
                    {
                        let per_layer = svc_layer_kbps(bitrate_kbps, svc_cfg.spatial_layers);
                        if let Err(e) = new_encoder.enable_vp9_svc(svc_cfg, &per_layer) {
                            tracing::warn!("voice vp9 svc enable failed: {e}");
                        }
                    }
                    encoder = Some(new_encoder);
                    dims = (frame.width, frame.height);
                    force_keyframe = true;
                }
                Err(e) => {
                    tracing::warn!("voice video encoder init failed: {e}");
                    continue;
                }
            }
        }
        let Some(encoder) = encoder.as_mut() else {
            continue;
        };
        let encoded = match encoder.encode(&frame, force_keyframe, pts) {
            Ok(encoded) => encoded,
            Err(e) => {
                tracing::warn!("voice vp9 encode failed: {e}");
                continue;
            }
        };
        pts += VIDEO_PTS_STEP;
        for encoded_frame in &encoded {
            if publisher
                .write_encoded(encoded_frame, VIDEO_FRAME_DURATION)
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_camera_track(
    publisher: Arc<LocalVideo>,
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
    codec: VpxCodec,
    svc: Option<SvcConfig>,
) -> Result<CameraSession> {
    let (controller, ready_rx, frame_rx) = camera::start_camera(identity, frame_store, device_id);
    ready_rx
        .recv_async()
        .await
        .map_err(|_| anyhow!("camera thread exited"))?
        .map_err(|e| anyhow!(e))?;
    let encode_task = runtime::runtime().spawn(encode_video_loop(
        frame_rx,
        publisher,
        CAMERA_BITRATE_KBPS,
        codec,
        svc,
    ));
    Ok(CameraSession {
        controller,
        encode_task,
    })
}

#[allow(clippy::too_many_arguments)]
async fn start_screen_track(
    publisher: Arc<LocalVideo>,
    identity: String,
    frame_store: Arc<VideoFrameStore>,
    full_res: Arc<AtomicBool>,
    pick: PickedScreen,
    codec: VpxCodec,
    svc: Option<SvcConfig>,
) -> Result<ScreenSession> {
    let (stopper, ready_rx, frame_rx) = screen::start_screen(identity, frame_store, full_res, pick);
    ready_rx
        .recv_async()
        .await
        .map_err(|_| anyhow!("screen thread exited"))?
        .map_err(|e| anyhow!(e))?;
    let encode_task = runtime::runtime().spawn(encode_video_loop(
        frame_rx,
        publisher,
        SCREEN_BITRATE_KBPS,
        codec,
        svc,
    ));
    Ok(ScreenSession {
        stopper,
        encode_task,
    })
}

async fn recv_device_reset(
    rx: &Option<flume::Receiver<audio::DeviceResetKind>>,
) -> Option<audio::DeviceResetKind> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

async fn recv_output_change(rx: &Option<flume::Receiver<AudioFormat>>) -> Option<AudioFormat> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

async fn shutdown_audio_io(audio_io: &mut Option<audio::AudioIo>) {
    let Some(audio_io) = audio_io.take() else {
        return;
    };
    if let Err(error) = tokio::task::spawn_blocking(move || drop(audio_io)).await {
        tracing::warn!("voice audio shutdown task failed: {error}");
    }
}

struct MicResampler {
    source_rate: u32,
    channels: usize,
    cursor: f64,
    src: Vec<f32>,
    frame: Vec<f32>,
}

impl MicResampler {
    fn new() -> Self {
        Self {
            source_rate: OPUS_SAMPLE_RATE,
            channels: 1,
            cursor: 0.0,
            src: Vec::new(),
            frame: Vec::new(),
        }
    }

    fn set_source(&mut self, source_rate: u32, channels: u32) {
        let channels = channels.max(1) as usize;
        if source_rate == self.source_rate && channels == self.channels {
            return;
        }
        self.source_rate = source_rate.max(1);
        self.channels = channels;
        self.reset();
    }

    fn reset(&mut self) {
        self.cursor = 0.0;
        self.src.clear();
        self.frame.clear();
    }

    fn push(&mut self, interleaved: &[i16]) {
        for chunk in interleaved.chunks(self.channels) {
            let sum: i32 = chunk.iter().map(|&s| s as i32).sum();
            self.src
                .push(sum as f32 * I16_TO_F32 / self.channels as f32);
        }
        self.resample();
    }

    fn resample(&mut self) {
        if self.source_rate == OPUS_SAMPLE_RATE {
            self.frame.append(&mut self.src);
            return;
        }
        let step = self.source_rate as f64 / OPUS_SAMPLE_RATE as f64;
        while self.cursor + 1.0 < self.src.len() as f64 {
            let index = self.cursor as usize;
            let fraction = (self.cursor - index as f64) as f32;
            let sample = self.src[index] * (1.0 - fraction) + self.src[index + 1] * fraction;
            self.frame.push(sample);
            self.cursor += step;
        }
        let consumed = self.cursor as usize;
        if consumed > 0 {
            self.src.drain(..consumed);
            self.cursor -= consumed as f64;
        }
    }

    fn take_frame(&mut self) -> Option<Vec<f32>> {
        if self.frame.len() < OPUS_FRAME_SAMPLES {
            return None;
        }
        let rest = self.frame.split_off(OPUS_FRAME_SAMPLES);
        Some(std::mem::replace(&mut self.frame, rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers_of(users: &[&str]) -> BTreeSet<String> {
        users.iter().map(|u| u.to_string()).collect()
    }

    #[test]
    fn local_participant_reflects_local_state() {
        let list = build_participants(
            "me",
            true,
            true,
            false,
            true,
            false,
            &BTreeSet::new(),
            &HashMap::new(),
            1000,
        );
        let local = &list[0];
        assert!(local.is_local);
        assert!(!local.muted, "mic on means not muted");
        assert_eq!(local.camera, Some(local_camera_key("me")));
        assert_eq!(local.screenshare, None);
        assert_eq!(local.quality, NetworkQuality::Good);
        assert_eq!(list.len(), 1, "no remotes when roster empty and no media");
    }

    #[test]
    fn muted_and_disconnected_local() {
        let list = build_participants(
            "me",
            false,
            false,
            false,
            false,
            false,
            &BTreeSet::new(),
            &HashMap::new(),
            1000,
        );
        assert!(list[0].muted);
        assert_eq!(list[0].quality, NetworkQuality::Unknown);
    }

    #[test]
    fn remote_camera_attaches_to_the_sending_user() {
        let peers = peers_of(&["alice", "bob"]);
        let mut media = HashMap::new();
        media.insert(
            "alice".to_string(),
            RemoteTimes {
                audio_ms: 1000,
                speaking_ms: 0,
                camera_ms: 1000,
                screen_ms: 0,
            },
        );
        let list = build_participants("me", false, false, false, true, false, &peers, &media, 1000);
        assert_eq!(list.len(), 3);
        let alice = list.iter().find(|p| p.identity == "alice").unwrap();
        assert_eq!(alice.camera, Some(remote_camera_key("alice")));
        assert!(!alice.muted, "recent audio means not muted");
        let bob = list.iter().find(|p| p.identity == "bob").unwrap();
        assert_eq!(bob.camera, None);
        assert!(bob.muted, "no audio means muted");
    }

    #[test]
    fn remote_appears_from_media_without_roster_membership() {
        let mut media = HashMap::new();
        media.insert(
            "77".to_string(),
            RemoteTimes {
                audio_ms: 0,
                speaking_ms: 0,
                camera_ms: 0,
                screen_ms: 1000,
            },
        );
        let list = build_participants(
            "me",
            false,
            false,
            false,
            true,
            false,
            &BTreeSet::new(),
            &media,
            1000,
        );
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].identity, "77");
        assert_eq!(list[1].screenshare, Some(remote_screen_key("77")));
        assert_eq!(list[1].camera, None);
    }

    #[test]
    fn lifecycle_maps_connect_disconnect_error() {
        assert!(matches!(
            lifecycle_event(&SfuClientEvent::Connected, "42"),
            Some(VoiceEvent::Connected { room_name }) if room_name == "42"
        ));
        assert!(matches!(
            lifecycle_event(&SfuClientEvent::Disconnected, "42"),
            Some(VoiceEvent::Disconnected { .. })
        ));
        assert!(matches!(
            lifecycle_event(&SfuClientEvent::Error("boom".into()), "42"),
            Some(VoiceEvent::Error(msg)) if msg == "boom"
        ));
        assert!(lifecycle_event(&SfuClientEvent::RemoteAudio, "42").is_none());
        assert!(lifecycle_event(&SfuClientEvent::Joined, "42").is_none());
    }

    #[test]
    fn roster_events_fold_and_report_change() {
        let mut roster = RemoteRoster::default();
        let me = "me";
        assert!(apply_roster_event(
            &mut roster,
            &SfuClientEvent::RoleChanged {
                user_id: "alice".into(),
                role: "speaker".into(),
            },
            me
        ));
        assert!(roster.peers.contains("alice"));
        assert!(!apply_roster_event(
            &mut roster,
            &SfuClientEvent::RoleChanged {
                user_id: "alice".into(),
                role: "audience".into(),
            },
            me
        ));
        assert!(apply_roster_event(
            &mut roster,
            &SfuClientEvent::PeerLeft {
                user_id: "alice".into(),
            },
            me
        ));
        assert!(!roster.peers.contains("alice"));
    }

    #[test]
    fn roster_drops_our_own_role_echo() {
        let mut roster = RemoteRoster::default();
        assert!(!apply_roster_event(
            &mut roster,
            &SfuClientEvent::RoleChanged {
                user_id: "42".into(),
                role: "speaker".into(),
            },
            "42"
        ));
        assert!(roster.peers.is_empty());
    }

    #[test]
    fn mic_resampler_passthrough_yields_20ms_frames() {
        let mut resampler = MicResampler::new();
        resampler.set_source(48_000, 1);
        let input: Vec<i16> = (0..OPUS_FRAME_SAMPLES as i16 * 2).collect();
        resampler.push(&input);
        assert!(resampler.take_frame().is_some());
        assert!(resampler.take_frame().is_some());
        assert!(resampler.take_frame().is_none());
    }

    #[test]
    fn mic_resampler_downmixes_stereo() {
        let mut resampler = MicResampler::new();
        resampler.set_source(48_000, 2);
        let input: Vec<i16> = (0..OPUS_FRAME_SAMPLES)
            .flat_map(|_| [16_384, -16_384])
            .collect();
        resampler.push(&input);
        let frame = resampler.take_frame().expect("one frame");
        assert_eq!(frame.len(), OPUS_FRAME_SAMPLES);
        assert!(
            frame.iter().all(|s| s.abs() < 0.01),
            "L/R cancel to silence"
        );
    }

    #[test]
    fn mic_resampler_downsamples_96k_to_48k() {
        let mut resampler = MicResampler::new();
        resampler.set_source(96_000, 1);
        let input: Vec<i16> = vec![1000; OPUS_FRAME_SAMPLES * 4];
        resampler.push(&input);
        assert!(resampler.take_frame().is_some());
        assert!(resampler.take_frame().is_some());
    }
}
