mod audio;
mod camera;
pub mod compose;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux_session;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod pipewire_init;
mod record;
mod runtime;
mod screen;
mod screen_audio;
mod screen_picker;
mod screen_previews;
mod screen_targets;
mod sfu;
mod stream_playback;
mod video;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::StreamExt;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::audio_track::RtcAudioTrack;
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt as _;
use libwebrtc::prelude::{AudioFrame, AudioSourceOptions, VideoBuffer};
use libwebrtc::video_frame::I420Buffer;
use libwebrtc::video_stream::native::NativeVideoStream;
use libwebrtc::video_track::{ContentHint, RtcVideoTrack};
use parking_lot::{Condvar, Mutex};

use sfu::{SfuConfig, SfuEngine, SfuEvent, SfuPeer};

pub use audio::{AudioFormat, AudioIo, DeviceResetKind, MicResampler, PlaybackMixer, SpeakingLevels};
pub use camera::{
    CameraController, CameraDeviceInfo, camera_denied, enumerate_cameras, start_camera,
    start_camera_into,
};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use linux_session::record_wayland_session;
pub use mezon_record::{RecordError, RecordStats};
pub use record::{
    RECORD_FPS, RECORD_HEIGHT, RECORD_WIDTH, RecordSession, RecordStarter, RecordTaps,
};
pub use sfu::SfuRole;
pub use stream_playback::StreamAudioOutput;

pub fn microphone_denied() -> bool {
    audio::microphone_denied()
}

pub fn record_supported() -> bool {
    mezon_record::is_supported()
}

pub fn record_file_extension() -> &'static str {
    mezon_record::file_extension()
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
use crate::video::local_screen_key;

const MAX_REMOTE_VIDEO_WIDTH: u32 = 1920;
const MAX_REMOTE_VIDEO_HEIGHT: u32 = 1080;

const AUDIO_SOURCE_QUEUE_SIZE_MS: u32 = 100;
const PLAYBACK_RESTART_DELAY: Duration = Duration::from_millis(500);
const MAX_PLAYBACK_RESTARTS: u32 = 3;

#[derive(Clone, Debug, Default)]
pub struct IceServerConfig {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

/// Mints a replacement join token.
///
/// The engine needs one *between* sessions, after the SFU has rejected the
/// current token and before the next connect, so this is a callback rather than
/// a value: `mezon-voice` does not depend on the API client that issues them.
#[derive(Clone)]
pub struct TokenRefresher(
    Arc<dyn Fn() -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>,
);

impl TokenRefresher {
    pub fn new<F, Fut>(mint: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<String>> + Send + 'static,
    {
        Self(Arc::new(move || {
            Box::pin(mint()) as futures::future::BoxFuture<'static, Option<String>>
        }))
    }

    pub(crate) async fn mint(&self) -> Option<String> {
        (self.0)().await
    }
}

impl std::fmt::Debug for TokenRefresher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenRefresher(..)")
    }
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
    pub is_audience: bool,
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
    /// The SFU granted or withdrew an audience member's push-to-talk turn. The
    /// UI holds the button down optimistically, so this is what confirms the
    /// mic is actually live.
    PushToTalkActive(bool),
    /// A moderator removed this peer from the channel. Always followed by
    /// [`VoiceEvent::Disconnected`]; this arrives first so the UI can explain
    /// why the call ended.
    RemovedFromChannel { reason: String },
    Error(String),
}

enum Command {
    SetMicEnabled(bool),
    SetCameraEnabled(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetCameraDevice(Option<String>),
    SetNoiseSuppression(bool, u8),
    StartScreenShare(PickedScreen, bool),
    StopScreenShare,
    PushToTalk(bool),
    Disconnect,
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

#[derive(Clone, Debug)]
pub struct VoiceConnectOptions {
    pub url: String,
    pub token: String,
    pub room: String,
    pub role: SfuRole,
    pub local_user_id: String,
    pub input_device_id: Option<String>,
    pub output_device_id: Option<String>,
    pub camera_device_id: Option<String>,
    pub ice_servers: Vec<IceServerConfig>,
    pub refresh_token: Option<TokenRefresher>,
}

impl VoiceSession {
    pub fn connect(options: VoiceConnectOptions) -> Self {
        let VoiceConnectOptions {
            url,
            token,
            room,
            role,
            local_user_id,
            input_device_id,
            output_device_id,
            camera_device_id,
            ice_servers,
            refresh_token,
        } = options;
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (evt_tx, evt_rx) = flume::unbounded();
        let frame_store = Arc::new(VideoFrameStore::default());
        let screen_full_res = Arc::new(AtomicBool::new(false));

        let record_taps = record::RecordTaps::default();

        let store = frame_store.clone();
        let screen_full_res_task = screen_full_res.clone();
        let record_taps_task = record_taps.clone();
        runtime::runtime().spawn(async move {
            if let Err(e) = session_main(
                SfuConfig {
                    ws_url: url,
                    token,
                    room,
                    role,
                    fallback_ice_servers: ice_servers,
                    refresh_token,
                },
                local_user_id,
                input_device_id,
                output_device_id,
                camera_device_id,
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

    pub fn recording_video_unavailable(&self) -> bool {
        self.record
            .read()
            .as_ref()
            .is_some_and(|session| session.video_unavailable())
    }

    pub fn recording_failed(&self) -> bool {
        self.record
            .read()
            .as_ref()
            .is_some_and(|session| session.failed())
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

    /// Press and release the audience push-to-talk turn. A speaker ignores
    /// these; the SFU rejects the request and the engine backs the state out.
    pub fn set_push_to_talk(&self, active: bool) {
        let _ = self.cmd_tx.send(Command::PushToTalk(active));
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
    track: RtcVideoTrack,
    controller: CameraController,
}

struct ScreenSession {
    track: sfu::ScreenTrack,
    stopper: ScreenStopper,
    audio: Option<ScreenAudioSession>,
}

struct ScreenAudioSession {
    _capture: screen_audio::ScreenAudioCapture,
    pump: tokio::task::JoinHandle<()>,
}

impl ScreenSession {
    fn stop(self) {
        self.stopper.stop();
        if let Some(audio) = self.audio {
            audio.pump.abort();
        }
    }
}

/// Everything published upstream goes through one opus stream at this format,
/// fixed for the life of the session: the SFU negotiates mid 0 once and there
/// is no way to republish it when the capture device changes underneath.
const UPLINK_SAMPLE_RATE: u32 = 48_000;
const UPLINK_CHANNELS: u32 = 1;

/// Mono 48 kHz screen audio waiting to be summed into the next uplink frame.
/// Capped so a stalled uplink cannot grow it without bound.
const SCREEN_AUDIO_BACKLOG_LIMIT: usize = UPLINK_SAMPLE_RATE as usize;
const SCREEN_AUDIO_LOG_INTERVAL: Duration = Duration::from_secs(5);
const UPLINK_TICK: Duration = Duration::from_millis(10);
const UPLINK_TICK_SAMPLES: usize = (UPLINK_SAMPLE_RATE / 100) as usize;
const MICROPHONE_SILENCE_GRACE: Duration = Duration::from_millis(30);

type ScreenAudioBus = Arc<Mutex<std::collections::VecDeque<i16>>>;

/// There is only ever one local microphone, so its speaking level lives under a
/// fixed key rather than one derived from a mid.
const LOCAL_AUDIO_KEY: u64 = 0x10CA_1_A0D_10;
/// How often the participant list is re-sent so a tile's speaking ring can turn
/// on and off. Cheap: the list is deduplicated, so an idle room sends nothing.
const SPEAKING_POLL_INTERVAL: Duration = Duration::from_millis(150);

#[allow(clippy::too_many_arguments)]
async fn session_main(
    sfu_config: SfuConfig,
    local_user_id: String,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    cmd_rx: flume::Receiver<Command>,
    evt_tx: &flume::Sender<VoiceEvent>,
    frame_store: Arc<VideoFrameStore>,
    screen_full_res: Arc<AtomicBool>,
    session_record_taps: record::RecordTaps,
) -> Result<()> {
    let role = sfu_config.role;
    let factory = PeerConnectionFactory::default();

    let (sfu_tx, sfu_rx) = flume::unbounded::<SfuEvent>();
    let (engine, engine_task) = SfuEngine::spawn(sfu_config, factory.clone(), sfu_tx);

    let mic_enabled = Arc::new(AtomicBool::new(false));
    let speaking = Arc::new(audio::SpeakingLevels::default());
    let screen_audio_bus: ScreenAudioBus = Arc::new(Mutex::new(std::collections::VecDeque::new()));
    let mut audio_mixer = None;
    let mut record_taps: Option<record::RecordTaps> = None;
    let mut out_fmt = None;
    let mut audio_io: Option<audio::AudioIo> = None;
    let mut out_change_rx: Option<flume::Receiver<AudioFormat>> = None;
    let mut device_reset_rx: Option<flume::Receiver<audio::DeviceResetKind>> = None;
    let mut microphone_task: Option<tokio::task::JoinHandle<()>> = None;

    let audio = tokio::task::spawn_blocking(move || {
        audio::AudioIo::start(input_device_id, output_device_id, session_record_taps)
    })
    .await
    .map_err(|e| anyhow::anyhow!("audio init task failed: {e}"))?;

    match audio {
        Ok(audio) => {
            audio_mixer = Some(audio.mixer.clone());
            out_fmt = Some(audio.output_format);
            out_change_rx = Some(audio.output_format_rx.clone());
            device_reset_rx = Some(audio.device_reset_rx.clone());
            record_taps = Some(audio.mixer.record_taps());

            let uplink_source = NativeAudioSource::new(
                AudioSourceOptions::default(),
                UPLINK_SAMPLE_RATE,
                UPLINK_CHANNELS,
                AUDIO_SOURCE_QUEUE_SIZE_MS,
            );
            let uplink_track =
                factory.create_audio_track("microphone", uplink_source.clone());
            engine.set_local_audio(Some(uplink_track));

            microphone_task = Some(runtime::runtime().spawn(uplink_pump(
                audio.mic_rx.clone(),
                audio.input_format_rx.clone(),
                uplink_source,
                mic_enabled.clone(),
                audio.mixer.record_taps(),
                screen_audio_bus.clone(),
                speaking.clone(),
            )));

            audio_io = Some(audio);
        }
        Err(e) => {
            tracing::error!("voice audio unavailable: {e:#}");
            let _ = evt_tx.send(VoiceEvent::Error(format!(
                "audio unavailable (no microphone or playback): {e}"
            )));
        }
    }

    let mut mic_on = false;
    let mut ptt_on = false;
    let mut camera_device_id = camera_device_id;
    let mut camera_switch_pending = false;
    let mut camera_session: Option<CameraSession> = None;
    let mut screen_session: Option<ScreenSession> = None;
    let (cam_tx, cam_rx) = flume::bounded::<(u64, Result<CameraSession>)>(1);
    let (screen_tx, screen_rx) = flume::bounded::<(u64, Result<ScreenSession>)>(1);
    let mut camera_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut screen_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut camera_gen: u64 = 0;
    let mut screen_gen: u64 = 0;
    let mut audio_tracks: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut video_tracks: HashMap<u64, VideoTrackHandle> = HashMap::new();
    // Kept so a playback-device change can rebuild every reader at the new
    // output format without asking the SFU to re-send anything.
    let mut remote_audio: HashMap<u64, RtcAudioTrack> = HashMap::new();

    let mut peers: Vec<SfuPeer> = Vec::new();
    let mut last_participants: Vec<VoiceParticipant> = Vec::new();
    let mut speaking_tick = tokio::time::interval(SPEAKING_POLL_INTERVAL);
    speaking_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    macro_rules! emit {
        () => {
            emit_participants(
                evt_tx,
                &local_user_id,
                role,
                &peers,
                &speaking,
                mic_on,
                camera_session.is_some(),
                screen_session.is_some(),
                &mut last_participants,
            )
        };
    }

    emit!();

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let trim_task = runtime::runtime().spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            unsafe { libc::malloc_trim(0) };
        }
    });

    loop {
        tokio::select! {
            event = sfu_rx.recv_async() => {
                let Ok(event) = event else { break };
                match event {
                    SfuEvent::Connected { room } => {
                        let _ = evt_tx.send(VoiceEvent::Connected { room_name: room });
                    }
                    SfuEvent::Peers(next) => {
                        peers = next;
                        emit!();
                    }
                    SfuEvent::RemoteAudio { key, track } => {
                        remote_audio.insert(key, track.clone());
                        if let (Some(mixer), Some(out_fmt)) = (&audio_mixer, out_fmt) {
                            if let Some(handle) = audio_tracks.remove(&key) {
                                handle.abort();
                            }
                            let handle =
                                spawn_playback(track, key, mixer.clone(), out_fmt, speaking.clone());
                            audio_tracks.insert(key, handle);
                        }
                    }
                    SfuEvent::RemoteVideo { key, track } => {
                        if let Some(handle) = video_tracks.remove(&key) {
                            handle.stop();
                        }
                        let handle = spawn_video(track, key, frame_store.clone());
                        video_tracks.insert(key, handle);
                    }
                    SfuEvent::RemoteGone { key } => {
                        remote_audio.remove(&key);
                        speaking.forget(key);
                        if let Some(handle) = audio_tracks.remove(&key) {
                            handle.abort();
                        }
                        if let Some(handle) = video_tracks.remove(&key) {
                            handle.stop();
                        }
                        if let Some(mixer) = &audio_mixer {
                            mixer.remove(key);
                        }
                        frame_store.remove(key);
                    }
                    SfuEvent::PttActive(active) => {
                        ptt_on = active;
                        mic_on = active;
                        mic_enabled.store(active, Ordering::Relaxed);
                        if let Some(io) = &audio_io {
                            io.set_input_active(active);
                        }
                        let _ = evt_tx.send(VoiceEvent::PushToTalkActive(active));
                        emit!();
                    }
                    SfuEvent::Reconnecting => {
                        let _ = evt_tx.send(VoiceEvent::Reconnecting);
                    }
                    SfuEvent::Reconnected => {
                        let _ = evt_tx.send(VoiceEvent::Reconnected);
                    }
                    SfuEvent::Removed { reason } => {
                        let _ = evt_tx.send(VoiceEvent::RemovedFromChannel { reason });
                    }
                    SfuEvent::Error(message) => {
                        let _ = evt_tx.send(VoiceEvent::Error(message));
                    }
                    SfuEvent::Disconnected { reason } => {
                        let _ = evt_tx.send(VoiceEvent::Disconnected { reason });
                        break;
                    }
                }
            }
            command = cmd_rx.recv_async() => {
                match command {
                    Ok(Command::SetMicEnabled(enabled)) => {
                        // An audience member has no mic switch of their own; the
                        // turn is granted by the SFU and arrives as PttActive.
                        if role.is_audience() {
                            continue;
                        }
                        mic_on = enabled;
                        mic_enabled.store(enabled, Ordering::Relaxed);
                        if let Some(io) = &audio_io {
                            io.set_input_active(enabled);
                        }
                        engine.set_mute(!enabled);
                        emit!();
                    }
                    Ok(Command::PushToTalk(active)) => {
                        if !role.is_audience() {
                            continue;
                        }
                        // Held optimistically so the button feels immediate; the
                        // SFU's push_to_talk_changed is what actually opens the
                        // mic, and a rejection closes it again.
                        engine.push_to_talk(active);
                        if !active && ptt_on {
                            ptt_on = false;
                            mic_on = false;
                            mic_enabled.store(false, Ordering::Relaxed);
                            if let Some(io) = &audio_io {
                                io.set_input_active(false);
                            }
                            emit!();
                        }
                    }
                    Ok(Command::SetNoiseSuppression(enabled, level)) => {
                        if let Some(io) = &audio_io {
                            io.set_noise_suppression(enabled, level);
                        }
                    }
                    Ok(Command::SetCameraEnabled(true)) => {
                        if role.is_audience() {
                            continue;
                        }
                        if camera_session.is_none() && camera_task.is_none() {
                            camera_switch_pending = false;
                            let factory = factory.clone();
                            let identity = local_user_id.clone();
                            let store = frame_store.clone();
                            let tx = cam_tx.clone();
                            let generation = camera_gen;
                            let device = camera_device_id.clone();
                            camera_task = Some(runtime::runtime().spawn(async move {
                                let result = start_camera_track(&factory, &identity, store, device).await;
                                let _ = tx.send_async((generation, result)).await;
                            }));
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
                            } else if camera_task.is_some() {
                                camera_switch_pending = true;
                            }
                        }
                    }
                    Ok(Command::SetCameraEnabled(false)) => {
                        let mut changed = false;
                        camera_gen = camera_gen.wrapping_add(1);
                        camera_switch_pending = false;
                        if let Some(task) = camera_task.take() {
                            task.abort();
                            changed = true;
                        }
                        if let Some(session) = camera_session.take() {
                            session.controller.stop();
                            engine.set_local_camera(None);
                            engine.set_camera_active(false);
                            frame_store.remove(local_camera_key(&local_user_id));
                            changed = true;
                        }
                        if changed {
                            emit!();
                        }
                    }
                    Ok(Command::StartScreenShare(pick, share_audio)) => {
                        if role.is_audience() {
                            continue;
                        }
                        if screen_session.is_none() && screen_task.is_none() {
                            screen_full_res.store(false, Ordering::Relaxed);
                            let factory = factory.clone();
                            let identity = local_user_id.clone();
                            let store = frame_store.clone();
                            let full_res = screen_full_res.clone();
                            let tx = screen_tx.clone();
                            let generation = screen_gen;
                            let taps = record_taps.clone();
                            let events = evt_tx.clone();
                            let bus = screen_audio_bus.clone();
                            screen_task = Some(runtime::runtime().spawn(async move {
                                let result = start_screen_track(
                                    &factory, &identity, store, full_res, pick, share_audio, taps, events, bus,
                                )
                                .await;
                                let _ = tx.send_async((generation, result)).await;
                            }));
                        }
                    }
                    Ok(Command::StopScreenShare) => {
                        let mut changed = false;
                        screen_gen = screen_gen.wrapping_add(1);
                        if let Some(task) = screen_task.take() {
                            task.abort();
                            changed = true;
                        }
                        if let Some(session) = screen_session.take() {
                            session.stop();
                            engine.set_local_screen(None);
                            engine.set_screen_audio(false);
                            engine.set_screen_active(false);
                            screen_audio_bus.lock().clear();
                            frame_store.remove(local_screen_key(&local_user_id));
                            changed = true;
                        }
                        if changed {
                            emit!();
                        }
                    }
                    Ok(Command::Disconnect) | Err(_) => {
                        engine.close();
                        abort_task(&mut microphone_task).await;
                        shutdown_audio_io(&mut audio_io).await;
                        if let Some(task) = camera_task.take() {
                            task.abort();
                        }
                        if let Some(task) = screen_task.take() {
                            task.abort();
                        }
                        if let Some(session) = camera_session.take() {
                            session.controller.stop();
                        }
                        if let Some(session) = screen_session.take() {
                            session.stop();
                        }
                        let _ = evt_tx.send(VoiceEvent::Disconnected { reason: "left".into() });
                        break;
                    }
                }
            }
            result = cam_rx.recv_async() => {
                match result {
                    Ok((generation, Ok(session))) if generation == camera_gen => {
                        camera_task = None;
                        if camera_switch_pending {
                            camera_switch_pending = false;
                            session.controller.switch(camera_device_id.clone());
                        }
                        engine.set_local_camera(Some(session.track.clone()));
                        engine.set_camera_active(true);
                        camera_session = Some(session);
                        emit!();
                    }
                    Ok((_, Ok(session))) => {
                        session.controller.stop();
                        if camera_session.is_none() {
                            frame_store.remove(local_camera_key(&local_user_id));
                        }
                    }
                    Ok((generation, Err(e))) if generation == camera_gen => {
                        camera_task = None;
                        tracing::warn!("camera enable failed: {e:#}");
                        let _ = evt_tx.send(VoiceEvent::Error(format!("camera: {e}")));
                    }
                    Ok((_, Err(_))) => {}
                    Err(_) => {}
                }
            }
            result = screen_rx.recv_async() => {
                match result {
                    Ok((generation, Ok(session))) if generation == screen_gen => {
                        screen_task = None;
                        engine.set_local_screen(Some(session.track.clone()));
                        engine.set_screen_audio(session.audio.is_some());
                        engine.set_screen_active(true);
                        screen_session = Some(session);
                        emit!();
                    }
                    Ok((_, Ok(session))) => {
                        session.stop();
                        if screen_session.is_none() {
                            frame_store.remove(local_screen_key(&local_user_id));
                        }
                    }
                    Ok((generation, Err(e))) if generation == screen_gen => {
                        screen_task = None;
                        tracing::warn!("screen share enable failed: {e:#}");
                        let _ = evt_tx.send(VoiceEvent::Error(format!("screen: {e}")));
                    }
                    Ok((_, Err(_))) => {}
                    Err(_) => {}
                }
            }
            change = recv_output_change(&out_change_rx) => {
                if let (Some(new_fmt), Some(mixer)) = (change, &audio_mixer) {
                    out_fmt = Some(new_fmt);
                    respawn_audio_playback(
                        &remote_audio,
                        mixer,
                        new_fmt,
                        &mut audio_tracks,
                        &speaking,
                    );
                }
            }
            _ = speaking_tick.tick() => {
                // Speaking is measured on the audio threads, so nothing else
                // would prompt a re-send. emit! deduplicates, so a room where
                // nobody is talking sends nothing at all.
                emit!();
            }
            reset = recv_device_reset(&device_reset_rx) => {
                if let Some(kind) = reset {
                    let _ = evt_tx.send(VoiceEvent::DeviceResetToDefault {
                        input: matches!(kind, audio::DeviceResetKind::Input),
                    });
                }
            }
        }
    }

    for handle in audio_tracks.into_values() {
        handle.abort();
    }
    for handle in video_tracks.into_values() {
        handle.stop();
    }
    abort_task(&mut microphone_task).await;
    shutdown_audio_io(&mut audio_io).await;
    engine.close();
    engine_task.abort();

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    trim_task.abort();

    Ok(())
}

async fn uplink_pump(
    mic_rx: flume::Receiver<Vec<i16>>,
    input_format_rx: flume::Receiver<AudioFormat>,
    source: NativeAudioSource,
    mic_enabled: Arc<AtomicBool>,
    record_taps: record::RecordTaps,
    screen_audio: ScreenAudioBus,
    speaking: Arc<audio::SpeakingLevels>,
) {
    let mut resampler = audio::MicResampler::new(UPLINK_SAMPLE_RATE);
    let mut current_in_fmt: Option<AudioFormat> = None;
    let mut mic_out: Vec<i16> = Vec::new();
    let mut meter = ScreenAudioMeter::default();
    let mut last_mic_frame = Instant::now();
    let mut tick = tokio::time::interval(UPLINK_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            reconfigure = input_format_rx.recv_async() => {
                let Ok(in_fmt) = reconfigure else { break };
                current_in_fmt = Some(in_fmt);
            }
            captured = mic_rx.recv_async() => {
                let Ok(samples) = captured else { break };
                last_mic_frame = Instant::now();
                let Some(in_fmt) = current_in_fmt else { continue };

                let live = mic_enabled.load(Ordering::Relaxed);
                if live {
                    record_taps.push(
                        mezon_record::AudioSource::Mic,
                        &samples,
                        in_fmt.sample_rate,
                        in_fmt.channels,
                    );
                }

                mic_out.clear();
                resampler.process(&samples, in_fmt.sample_rate, in_fmt.channels, &mut mic_out);
                if mic_out.is_empty() {
                    continue;
                }
                if live {
                    speaking.observe(LOCAL_AUDIO_KEY, &mic_out);
                } else {
                    mic_out.iter_mut().for_each(|s| *s = 0);
                }
                if let Some(peak) = mix_screen_audio(&screen_audio, &mut mic_out) {
                    meter.observe(peak, live);
                }
                push_uplink_frame(&source, &mic_out).await;
            }
            _ = tick.tick() => {
                if last_mic_frame.elapsed() < MICROPHONE_SILENCE_GRACE {
                    continue;
                }
                let shared = take_screen_audio(&screen_audio, UPLINK_TICK_SAMPLES * 3);
                if shared.is_empty() {
                    continue;
                }
                let peak = shared.iter().map(|s| s.saturating_abs()).max().unwrap_or(0);
                meter.observe(peak, false);
                push_uplink_frame(&source, &shared).await;
            }
        }
    }
}

async fn push_uplink_frame(source: &NativeAudioSource, samples: &[i16]) {
    let frame = AudioFrame {
        data: std::borrow::Cow::Borrowed(samples),
        num_channels: UPLINK_CHANNELS,
        sample_rate: UPLINK_SAMPLE_RATE,
        samples_per_channel: samples.len() as u32,
    };
    if let Err(e) = source.capture_frame(&frame).await {
        tracing::warn!("uplink capture_frame failed: {e}");
    }
}

struct ScreenAudioMeter {
    peak: Option<i16>,
    logged_at: Instant,
}

impl Default for ScreenAudioMeter {
    fn default() -> Self {
        Self {
            peak: None,
            logged_at: Instant::now(),
        }
    }
}

impl ScreenAudioMeter {
    fn observe(&mut self, peak: i16, mic_live: bool) {
        let peak = self.peak.unwrap_or(0).max(peak);
        self.peak = Some(peak);
        if self.logged_at.elapsed() < SCREEN_AUDIO_LOG_INTERVAL {
            return;
        }
        tracing::info!(peak, mic_live, "screen audio mixed into the uplink");
        self.peak = None;
        self.logged_at = Instant::now();
    }
}

fn take_screen_audio(bus: &ScreenAudioBus, limit: usize) -> Vec<i16> {
    let mut queue = bus.lock();
    let count = queue.len().min(limit);
    queue.drain(..count).collect()
}

fn mix_screen_audio(bus: &ScreenAudioBus, out: &mut [i16]) -> Option<i16> {
    let mut queue = bus.lock();
    if queue.is_empty() {
        return None;
    }
    let mut peak = 0i16;
    for sample in out.iter_mut() {
        let Some(shared) = queue.pop_front() else { break };
        peak = peak.max(shared.saturating_abs());
        *sample = audio::clamp_i16(*sample as f32 + shared as f32);
    }
    Some(peak)
}

async fn abort_task(task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
}

async fn shutdown_audio_io(audio_io: &mut Option<audio::AudioIo>) {
    if let Some(io) = audio_io.take() {
        let _ = tokio::task::spawn_blocking(move || drop(io)).await;
    }
}

async fn start_camera_track(
    factory: &PeerConnectionFactory,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
) -> Result<CameraSession> {
    let (controller, source_rx) =
        camera::start_camera(identity.to_string(), frame_store, device_id);
    let source = source_rx
        .recv_async()
        .await
        .map_err(|_| anyhow::anyhow!("camera thread exited"))?
        .map_err(|e| anyhow::anyhow!(e))?;
    let track = factory.create_video_track("camera", source);
    track.set_content_hint(ContentHint::Fluid);
    Ok(CameraSession { track, controller })
}

#[allow(clippy::too_many_arguments)]
async fn start_screen_track(
    factory: &PeerConnectionFactory,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
    full_res: Arc<AtomicBool>,
    pick: PickedScreen,
    share_audio: bool,
    record_taps: Option<record::RecordTaps>,
    evt_tx: flume::Sender<VoiceEvent>,
    screen_audio_bus: ScreenAudioBus,
) -> Result<ScreenSession> {
    // Whether the switch in the picker was on is the first thing to know when a
    // recording comes out silent, and it left no trace anywhere before.
    tracing::info!("starting screen share (share system audio: {share_audio})");
    let (stopper, source_rx) =
        screen::start_screen(identity.to_string(), frame_store, full_res, pick);
    let (source, width, height) = source_rx
        .recv_async()
        .await
        .map_err(|_| anyhow::anyhow!("screen thread exited"))?
        .map_err(|e| anyhow::anyhow!(e))?;
    let screen_track = factory.create_video_track("screen", source);
    // What the web client sets on its screen track. Paired with
    // `MaintainResolution`, it is what keeps shared text legible when the
    // encoder has to choose between detail and frame rate.
    screen_track.set_content_hint(ContentHint::Detailed);
    let track = sfu::ScreenTrack {
        track: screen_track,
        width,
        height,
    };

    let audio = if share_audio {
        match start_screen_audio(record_taps, screen_audio_bus).await {
            Ok(audio) => {
                tracing::info!("sharing this machine's system audio with the call");
                Some(audio)
            }
            Err(e) => {
                // The screen keeps sharing without it, so a log line was the
                // only sign — nobody in the call hears the shared sound and the
                // recording has none either, with nothing to explain why.
                tracing::warn!("system audio share unavailable: {e:#}");
                let _ = evt_tx.send(VoiceEvent::Error(format!("screen audio: {e}")));
                None
            }
        }
    } else {
        None
    };

    Ok(ScreenSession {
        track,
        stopper,
        audio,
    })
}

async fn start_screen_audio(
    record_taps: Option<record::RecordTaps>,
    bus: ScreenAudioBus,
) -> Result<ScreenAudioSession> {
    let capture = tokio::task::spawn_blocking(screen_audio::start_screen_audio)
        .await
        .map_err(|e| anyhow::anyhow!("screen audio init task failed: {e}"))?
        .map_err(|e| anyhow::anyhow!(e))?;

    let rx = capture.rx.clone();
    let pump = runtime::runtime().spawn(async move {
        let channels = screen_audio::SCREEN_AUDIO_CHANNELS.max(1) as usize;
        while let Ok(samples) = rx.recv_async().await {
            if samples.is_empty() {
                continue;
            }
            if let Some(taps) = &record_taps {
                taps.push(
                    mezon_record::AudioSource::Screen,
                    &samples,
                    screen_audio::SCREEN_AUDIO_SAMPLE_RATE,
                    screen_audio::SCREEN_AUDIO_CHANNELS,
                );
            }
            let mut queue = bus.lock();
            for frame in samples.chunks_exact(channels) {
                let mono = frame.iter().map(|&s| s as f32).sum::<f32>() / channels as f32;
                queue.push_back(audio::clamp_i16(mono));
            }
            while queue.len() > SCREEN_AUDIO_BACKLOG_LIMIT {
                queue.pop_front();
            }
        }
    });

    Ok(ScreenAudioSession {
        _capture: capture,
        pump,
    })
}

fn spawn_playback(
    track: RtcAudioTrack,
    key: u64,
    mixer: Arc<audio::PlaybackMixer>,
    out_fmt: AudioFormat,
    speaking: Arc<audio::SpeakingLevels>,
) -> tokio::task::JoinHandle<()> {
    runtime::runtime().spawn(async move {
        let mut restart_attempts = 0;
        loop {
            let mut stream = NativeAudioStream::new(
                track.clone(),
                out_fmt.sample_rate as i32,
                out_fmt.channels as i32,
            );
            let mut saw_frame = false;
            while let Some(frame) = stream.next().await {
                saw_frame = true;
                speaking.observe(key, &frame.data);
                mixer.push(key, &frame.data);
            }
            mixer.remove(key);

            if restart_attempts >= MAX_PLAYBACK_RESTARTS {
                tracing::warn!(
                    "remote audio stream ended after {MAX_PLAYBACK_RESTARTS} restart attempts; stopping playback reader"
                );
                break;
            }

            let delay = PLAYBACK_RESTART_DELAY * 2u32.pow(restart_attempts);
            restart_attempts += 1;
            tracing::warn!(
                "remote audio stream ended (received_frames={saw_frame}); restart attempt {restart_attempts}/{MAX_PLAYBACK_RESTARTS} in {}ms",
                delay.as_millis()
            );
            tokio::time::sleep(delay).await;
        }
    })
}

async fn recv_output_change(rx: &Option<flume::Receiver<AudioFormat>>) -> Option<AudioFormat> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

async fn recv_device_reset(
    rx: &Option<flume::Receiver<audio::DeviceResetKind>>,
) -> Option<audio::DeviceResetKind> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

fn respawn_audio_playback(
    remote_audio: &HashMap<u64, RtcAudioTrack>,
    mixer: &Arc<audio::PlaybackMixer>,
    out_fmt: AudioFormat,
    audio_tracks: &mut HashMap<u64, tokio::task::JoinHandle<()>>,
    speaking: &Arc<audio::SpeakingLevels>,
) {
    for (_, handle) in audio_tracks.drain() {
        handle.abort();
    }
    for (&key, track) in remote_audio {
        mixer.remove(key);
        let handle = spawn_playback(track.clone(), key, mixer.clone(), out_fmt, speaking.clone());
        audio_tracks.insert(key, handle);
    }
}

#[derive(Default)]
struct VideoConvertState {
    latest: Option<I420Buffer>,
    closed: bool,
}

#[derive(Default)]
struct VideoConvertSlot {
    state: Mutex<VideoConvertState>,
    cond: Condvar,
}

impl VideoConvertSlot {
    fn put(&self, buffer: I420Buffer) {
        self.state.lock().latest = Some(buffer);
        self.cond.notify_one();
    }

    fn close(&self) {
        self.state.lock().closed = true;
        self.cond.notify_one();
    }

    fn take_latest(&self) -> Option<I420Buffer> {
        let mut state = self.state.lock();
        loop {
            if let Some(buffer) = state.latest.take() {
                return Some(buffer);
            }
            if state.closed {
                return None;
            }
            self.cond.wait(&mut state);
        }
    }
}

struct VideoTrackHandle {
    task: tokio::task::JoinHandle<()>,
    slot: Arc<VideoConvertSlot>,
}

impl VideoTrackHandle {
    fn stop(self) {
        self.task.abort();
        self.slot.close();
    }
}

fn spawn_video(
    track: RtcVideoTrack,
    key: u64,
    frame_store: Arc<VideoFrameStore>,
) -> VideoTrackHandle {
    let rtc_track = track;
    let slot = Arc::new(VideoConvertSlot::default());

    let convert_slot = slot.clone();
    let convert_store = frame_store;
    if let Err(e) = std::thread::Builder::new()
        .name("mezon-video-convert".into())
        .spawn(move || {
            let mut bgra: Vec<u8> = Vec::new();
            while let Some(buffer) = convert_slot.take_latest() {
                let width = buffer.width();
                let height = buffer.height();
                let (sy, su, sv) = buffer.strides();
                let (y, u, v) = buffer.data();
                bgra.clear();
                bgra.resize(width as usize * height as usize * 4, 0);
                i420_to_bgra_into(
                    &mut bgra,
                    y,
                    u,
                    v,
                    sy as usize,
                    su as usize,
                    sv as usize,
                    width as usize,
                    height as usize,
                );
                if let Some(recycled) =
                    convert_store.publish(key, width, height, std::mem::take(&mut bgra))
                {
                    bgra = recycled;
                }
            }
            convert_store.remove(key);
        })
    {
        tracing::error!("failed to spawn video convert thread: {e}");
    }

    let task_slot = slot.clone();
    let task = runtime::runtime().spawn(async move {
        let mut stream = NativeVideoStream::new(rtc_track);
        while let Some(frame) = stream.next().await {
            let mut buffer = frame.buffer.to_i420();
            let (width, height) = bounded_dimensions(
                buffer.width(),
                buffer.height(),
                MAX_REMOTE_VIDEO_WIDTH,
                MAX_REMOTE_VIDEO_HEIGHT,
            );
            if width != buffer.width() || height != buffer.height() {
                buffer = buffer.scale(width as i32, height as i32);
            }
            task_slot.put(buffer);
        }
        task_slot.close();
    });

    VideoTrackHandle { task, slot }
}

fn bounded_dimensions(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    let scale =
        (max_width as f64 / width.max(1) as f64).min(max_height as f64 / height.max(1) as f64);
    let width = ((width as f64 * scale).floor() as u32 & !1).max(2);
    let height = ((height as f64 * scale).floor() as u32 & !1).max(2);
    (width, height)
}

/// Builds the participant list the store renders from.
///
/// The SFU only reports remote peers, so the local tile is synthesised here
/// from what this session knows about itself. `name` is left empty on purpose:
/// the store resolves display names and avatars from the clan member list by
/// user id, and inventing one here would shadow it.
#[allow(clippy::too_many_arguments)]
fn emit_participants(
    evt_tx: &flume::Sender<VoiceEvent>,
    local_user_id: &str,
    local_role: SfuRole,
    peers: &[SfuPeer],
    speaking: &audio::SpeakingLevels,
    local_mic_enabled: bool,
    local_camera_on: bool,
    local_screen_on: bool,
    last: &mut Vec<VoiceParticipant>,
) {
    let mut participants = Vec::with_capacity(peers.len() + 1);

    participants.push(VoiceParticipant {
        identity: local_user_id.to_string(),
        name: String::new(),
        is_local: true,
        is_agent: false,
        is_audience: local_role.is_audience(),
        speaking: local_mic_enabled && speaking.is_speaking(LOCAL_AUDIO_KEY),
        muted: !local_mic_enabled,
        camera: local_camera_on.then(|| local_camera_key(local_user_id)),
        screenshare: local_screen_on.then(|| local_screen_key(local_user_id)),
        quality: NetworkQuality::Unknown,
    });

    for peer in peers {
        if peer.user_id == local_user_id {
            continue;
        }
        participants.push(VoiceParticipant {
            identity: peer.user_id.clone(),
            name: String::new(),
            is_local: false,
            is_agent: false,
            is_audience: peer.is_audience,
            speaking: !peer.muted
                && peer.audio.is_some_and(|key| speaking.is_speaking(key)),
            muted: peer.muted,
            camera: peer.camera,
            screenshare: peer.screenshare,
            quality: NetworkQuality::Unknown,
        });
    }

    if *last == participants {
        return;
    }
    last.clone_from(&participants);
    let _ = evt_tx.send(VoiceEvent::Participants(participants));
}


#[cfg(test)]
mod remote_video_tests {
    use super::bounded_dimensions;

    #[test]
    fn bounded_dimensions_preserve_small_frames() {
        assert_eq!(bounded_dimensions(1280, 720, 1920, 1080), (1280, 720));
    }

    #[test]
    fn bounded_dimensions_cap_large_frames_evenly() {
        assert_eq!(bounded_dimensions(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(bounded_dimensions(2560, 1600, 1920, 1080), (1728, 1080));
    }
}
