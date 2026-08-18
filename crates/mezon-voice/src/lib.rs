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
mod stream_playback;
mod video;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::StreamExt;
use livekit::options::{DegradationPreference, TrackPublishOptions, VideoEncoding};
use livekit::participant::ParticipantKind;
use livekit::prelude::*;
use livekit::track::{
    LocalAudioTrack, LocalTrack, LocalVideoTrack, RemoteVideoTrack, TrackKind, TrackSource,
};
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::peer_connection_factory::IceServer;
use livekit::webrtc::prelude::{AudioFrame, AudioSourceOptions, RtcAudioSource, VideoBuffer};
use livekit::webrtc::stats::RtcStats;
use livekit::webrtc::video_frame::I420Buffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use parking_lot::{Condvar, Mutex};

pub use audio::{AudioFormat, AudioIo, DeviceResetKind, PlaybackMixer};
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
use crate::video::{local_screen_key, track_frame_key};

const MAX_REMOTE_VIDEO_WIDTH: u32 = 1920;
const MAX_REMOTE_VIDEO_HEIGHT: u32 = 1080;

const AUDIO_SOURCE_QUEUE_SIZE_MS: u32 = 100;
const MIC_PUBLISH_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const MIC_EGRESS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MIC_EGRESS_PUBLISH_GRACE: Duration = Duration::from_secs(3);
const MIC_EGRESS_STALL_LIMIT: u32 = 3;
const PLAYBACK_RESTART_DELAY: Duration = Duration::from_millis(500);
const MAX_PLAYBACK_RESTARTS: u32 = 3;

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
    SetCameraEnabled(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetCameraDevice(Option<String>),
    SetNoiseSuppression(bool, u8),
    StartScreenShare(PickedScreen, bool),
    StopScreenShare,
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

impl VoiceSession {
    pub fn connect(
        url: String,
        token: String,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        camera_device_id: Option<String>,
        ice_servers: Vec<IceServerConfig>,
    ) -> Self {
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
                url,
                token,
                input_device_id,
                output_device_id,
                camera_device_id,
                ice_servers,
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
    track: LocalVideoTrack,
    controller: CameraController,
}

struct ScreenSession {
    track: LocalVideoTrack,
    stopper: ScreenStopper,
    audio: Option<ScreenAudioSession>,
}

struct ScreenAudioSession {
    track: LocalAudioTrack,
    _capture: screen_audio::ScreenAudioCapture,
    pump: tokio::task::JoinHandle<()>,
}

impl ScreenSession {
    async fn stop(self, room: &Room) {
        self.stopper.stop();
        let _ = room
            .local_participant()
            .unpublish_track(&self.track.sid())
            .await;
        if let Some(audio) = self.audio {
            audio.pump.abort();
            let _ = room
                .local_participant()
                .unpublish_track(&audio.track.sid())
                .await;
        }
    }
}

fn room_options(ice_servers: Vec<IceServerConfig>) -> RoomOptions {
    let ice_servers: Vec<IceServer> = ice_servers
        .into_iter()
        .filter(|s| !s.urls.is_empty())
        .map(|s| IceServer {
            urls: s.urls,
            username: s.username,
            password: s.credential,
        })
        .collect();

    let mut options = RoomOptions::default();
    options.adaptive_stream = true;
    options.dynacast = true;
    options.rtc_config.ice_servers = ice_servers;
    options
}

#[allow(clippy::too_many_arguments)]
async fn session_main(
    url: String,
    token: String,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    camera_device_id: Option<String>,
    ice_servers: Vec<IceServerConfig>,
    cmd_rx: flume::Receiver<Command>,
    evt_tx: &flume::Sender<VoiceEvent>,
    frame_store: Arc<VideoFrameStore>,
    screen_full_res: Arc<AtomicBool>,
    session_record_taps: record::RecordTaps,
) -> Result<()> {
    let options = room_options(ice_servers);
    let (room, mut room_events) = Room::connect(&url, &token, options).await?;
    let room = Arc::new(room);
    tracing::info!("voice connected to room: {}", room.name());
    let local_identity = room.local_participant().identity().as_str().to_string();
    let _ = evt_tx.send(VoiceEvent::Connected {
        room_name: room.name(),
    });

    let mic_enabled = Arc::new(AtomicBool::new(false));
    let mic_publication: Arc<Mutex<Option<LocalTrackPublication>>> = Arc::new(Mutex::new(None));
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

            let mic_enabled = mic_enabled.clone();
            let mic_publication_task = mic_publication.clone();
            let mic_record_taps = audio.mixer.record_taps();
            let mic_rx = audio.mic_rx.clone();
            let input_format_rx = audio.input_format_rx.clone();
            let room_for_mic = room.clone();
            microphone_task = Some(runtime::runtime().spawn(async move {
                let mut source: Option<NativeAudioSource> = None;
                let mut channels: u32 = 1;
                let mut sample_rate: u32 = 48_000;
                let mut current_in_fmt: Option<AudioFormat> = None;
                let mut last_publish_attempt: Option<Instant> = None;
                let mut egress_timer = tokio::time::interval(MIC_EGRESS_POLL_INTERVAL);
                egress_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_egress_packets: u64 = 0;
                let mut egress_stall_polls: u32 = 0;
                let mut frames_since_poll: u64 = 0;
                let mut egress_recovering = false;
                let mut egress_grace_until: Option<Instant> = None;
                loop {
                    tokio::select! {
                        biased;
                        reconfigure = input_format_rx.recv_async() => {
                            let Ok(in_fmt) = reconfigure else { break };
                            current_in_fmt = Some(in_fmt);
                            last_publish_attempt = Some(Instant::now());
                            match publish_microphone_track(
                                &room_for_mic,
                                &mic_publication_task,
                                &mic_enabled,
                                in_fmt,
                            )
                            .await
                            {
                                Ok(new_source) => {
                                    channels = new_source.channels;
                                    sample_rate = new_source.sample_rate;
                                    source = Some(new_source.source);
                                    last_egress_packets = 0;
                                    egress_stall_polls = 0;
                                    egress_grace_until =
                                        Some(Instant::now() + MIC_EGRESS_PUBLISH_GRACE);
                                }
                                Err(e) => {
                                    tracing::warn!("failed to publish mic track: {e:#}");
                                    source = None;
                                }
                            }
                        }
                        captured = mic_rx.recv_async() => {
                            let Ok(samples) = captured else { break };
                            if !mic_enabled.load(Ordering::Relaxed) {
                                continue;
                            }
                            if let Some(in_fmt) = current_in_fmt {
                                mic_record_taps.push(
                                    mezon_record::AudioSource::Mic,
                                    &samples,
                                    in_fmt.sample_rate,
                                    in_fmt.channels,
                                );
                            }
                            if source.is_none()
                                && last_publish_attempt
                                    .is_none_or(|at| at.elapsed() >= MIC_PUBLISH_RETRY_INTERVAL)
                                && let Some(in_fmt) = current_in_fmt
                            {
                                last_publish_attempt = Some(Instant::now());
                                match publish_microphone_track(
                                    &room_for_mic,
                                    &mic_publication_task,
                                    &mic_enabled,
                                    in_fmt,
                                )
                                .await
                                {
                                    Ok(new_source) => {
                                        channels = new_source.channels;
                                        sample_rate = new_source.sample_rate;
                                        source = Some(new_source.source);
                                        last_egress_packets = 0;
                                        egress_stall_polls = 0;
                                        egress_grace_until =
                                            Some(Instant::now() + MIC_EGRESS_PUBLISH_GRACE);
                                    }
                                    Err(e) => {
                                        tracing::warn!("failed to retry mic track publish: {e:#}");
                                    }
                                }
                            }
                            let Some(mic_source) = source.as_ref() else {
                                continue;
                            };
                            let samples_per_channel = samples.len() as u32 / channels;
                            if samples_per_channel == 0 {
                                continue;
                            }
                            let frame = AudioFrame {
                                data: samples.into(),
                                num_channels: channels,
                                sample_rate,
                                samples_per_channel,
                            };
                            if let Err(e) = mic_source.capture_frame(&frame).await {
                                tracing::warn!("failed to capture mic frame: {e}");
                                source = None;
                            } else {
                                frames_since_poll += 1;
                            }
                        }
                        _ = egress_timer.tick() => {
                            let fed = std::mem::take(&mut frames_since_poll);
                            if !mic_enabled.load(Ordering::Relaxed) || source.is_none() {
                                egress_stall_polls = 0;
                                continue;
                            }
                            if egress_grace_until.is_some_and(|until| Instant::now() < until) {
                                continue;
                            }
                            if fed == 0 {
                                egress_stall_polls = 0;
                                continue;
                            }
                            let track = mic_publication_task
                                .lock()
                                .as_ref()
                                .and_then(|publication| publication.track());
                            let Some(LocalTrack::Audio(track)) = track else {
                                continue;
                            };
                            let packets = match tokio::time::timeout(
                                Duration::from_millis(500),
                                track.get_stats(),
                            )
                            .await
                            {
                                Ok(Ok(stats)) => outbound_audio_packets_sent(&stats),
                                _ => continue,
                            };
                            if packets > last_egress_packets {
                                last_egress_packets = packets;
                                egress_stall_polls = 0;
                                if egress_recovering {
                                    tracing::info!("voice mic egress recovered after republish");
                                    egress_recovering = false;
                                }
                                continue;
                            }
                            egress_stall_polls += 1;
                            if egress_stall_polls < MIC_EGRESS_STALL_LIMIT {
                                continue;
                            }
                            tracing::warn!(
                                packets_sent = packets,
                                "voice mic published but no RTP egress while unmuted; republishing track"
                            );
                            source = None;
                            last_publish_attempt = None;
                            egress_stall_polls = 0;
                            egress_recovering = true;
                            egress_grace_until = Some(Instant::now() + MIC_EGRESS_PUBLISH_GRACE);
                        }
                    }
                }
            }));

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

    let mut last_participants: Vec<VoiceParticipant> = Vec::new();
    let emit = |room: &Room,
                mic: bool,
                camera: &Option<CameraSession>,
                screen: &Option<ScreenSession>,
                last: &mut Vec<VoiceParticipant>| {
        emit_participants(
            room,
            evt_tx,
            &local_identity,
            mic,
            camera.is_some(),
            screen.is_some(),
            last,
        );
    };
    emit(
        &room,
        mic_on,
        &camera_session,
        &screen_session,
        &mut last_participants,
    );

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
            event = room_events.recv() => {
                let Some(event) = event else { break };
                match event {
                    RoomEvent::TrackSubscribed { track, publication, participant } => {
                        match track {
                            RemoteTrack::Audio(audio_track) => {
                                // Screen-share audio and a plain microphone are
                                // the same kind of remote track here, so the
                                // source is the only way to tell from a log
                                // whether the shared sound ever reached the mix
                                // the recorder tees from.
                                tracing::info!(
                                    "playing remote {:?} audio from {}",
                                    publication.source(),
                                    participant.identity().as_str()
                                );
                                if let (Some(mixer), Some(out_fmt)) = (&audio_mixer, out_fmt) {
                                    let key = track_frame_key(participant.identity().as_str(), audio_track.sid().as_str());
                                    if let Some(handle) = audio_tracks.remove(&key) {
                                        handle.abort();
                                    }
                                    let handle = spawn_playback(audio_track, key, mixer.clone(), out_fmt);
                                    audio_tracks.insert(key, handle);
                                }
                            }
                            RemoteTrack::Video(video_track) => {
                                let key = track_frame_key(participant.identity().as_str(), video_track.sid().as_str());
                                if let Some(handle) = video_tracks.remove(&key) {
                                    handle.stop();
                                }
                                let handle = spawn_video(video_track, key, frame_store.clone());
                                video_tracks.insert(key, handle);
                            }
                        }
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    RoomEvent::TrackUnsubscribed { track, participant, .. } => {
                        let key = track_frame_key(participant.identity().as_str(), track.sid().as_str());
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
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    RoomEvent::ConnectionQualityChanged { quality, participant } => {
                        if participant.identity().as_str() == local_identity {
                            match quality {
                                ConnectionQuality::Excellent | ConnectionQuality::Good => {
                                    let _ = evt_tx.send(VoiceEvent::NetworkRecovered);
                                }
                                ConnectionQuality::Poor | ConnectionQuality::Lost => {
                                    let _ = evt_tx.send(VoiceEvent::NetworkWeak);
                                }
                            }
                        }
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    RoomEvent::ConnectionStateChanged(ConnectionState::Reconnecting) => {
                        let _ = evt_tx.send(VoiceEvent::Reconnecting);
                    }
                    RoomEvent::ConnectionStateChanged(ConnectionState::Connected) => {
                        let _ = evt_tx.send(VoiceEvent::Reconnected);
                    }
                    RoomEvent::ConnectionStateChanged(ConnectionState::Disconnected) => {}
                    RoomEvent::Reconnecting => {
                        let _ = evt_tx.send(VoiceEvent::Reconnecting);
                    }
                    RoomEvent::Reconnected => {
                        let _ = evt_tx.send(VoiceEvent::Reconnected);
                    }
                    RoomEvent::Disconnected { reason } => {
                        let _ = evt_tx.send(VoiceEvent::Disconnected { reason: format!("{reason:?}") });
                        break;
                    }
                    RoomEvent::ParticipantConnected(..)
                    | RoomEvent::ParticipantActive(..)
                    | RoomEvent::ParticipantDisconnected(..)
                    | RoomEvent::TrackPublished { .. }
                    | RoomEvent::TrackUnpublished { .. }
                    | RoomEvent::TrackMuted { .. }
                    | RoomEvent::TrackUnmuted { .. }
                    | RoomEvent::ActiveSpeakersChanged { .. }
                    | RoomEvent::ParticipantNameChanged { .. }
                    | RoomEvent::ParticipantsUpdated { .. } => {
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    _ => {}
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
                        if let Some(publication) = mic_publication.lock().as_ref() {
                            if enabled {
                                publication.unmute();
                            } else {
                                publication.mute();
                            }
                        }
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    Ok(Command::SetNoiseSuppression(enabled, level)) => {
                        if let Some(io) = &audio_io {
                            io.set_noise_suppression(enabled, level);
                        }
                    }
                    Ok(Command::SetCameraEnabled(true)) => {
                        if camera_session.is_none() && camera_task.is_none() {
                            camera_switch_pending = false;
                            let room = room.clone();
                            let identity = local_identity.clone();
                            let store = frame_store.clone();
                            let tx = cam_tx.clone();
                            let generation = camera_gen;
                            let device = camera_device_id.clone();
                            camera_task = Some(runtime::runtime().spawn(async move {
                                let result = start_camera_track(&room, &identity, store, device).await;
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
                            let _ = room.local_participant().unpublish_track(&session.track.sid()).await;
                            frame_store.remove(local_camera_key(&local_identity));
                            changed = true;
                        }
                        if changed {
                            emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                        }
                    }
                    Ok(Command::StartScreenShare(pick, share_audio)) => {
                        if screen_session.is_none() && screen_task.is_none() {
                            screen_full_res.store(false, Ordering::Relaxed);
                            let room = room.clone();
                            let identity = local_identity.clone();
                            let store = frame_store.clone();
                            let full_res = screen_full_res.clone();
                            let tx = screen_tx.clone();
                            let generation = screen_gen;
                            let taps = record_taps.clone();
                            let events = evt_tx.clone();
                            screen_task = Some(runtime::runtime().spawn(async move {
                                let result =
                                    start_screen_track(&room, &identity, store, full_res, pick, share_audio, taps, events).await;
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
                            session.stop(&room).await;
                            frame_store.remove(local_screen_key(&local_identity));
                            changed = true;
                        }
                        if changed {
                            emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                        }
                    }
                    Ok(Command::Disconnect) | Err(_) => {
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
                            session.stopper.stop();
                            if let Some(audio) = &session.audio {
                                audio.pump.abort();
                            }
                        }
                        let _ = room.close().await;
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
                        camera_session = Some(session);
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    Ok((_, Ok(session))) => {
                        session.controller.stop();
                        let _ = room.local_participant().unpublish_track(&session.track.sid()).await;
                        if camera_session.is_none() {
                            frame_store.remove(local_camera_key(&local_identity));
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
                        screen_session = Some(session);
                        emit(
                            &room,
                            mic_on,
                            &camera_session,
                            &screen_session,
                            &mut last_participants,
                        );
                    }
                    Ok((_, Ok(session))) => {
                        session.stop(&room).await;
                        if screen_session.is_none() {
                            frame_store.remove(local_screen_key(&local_identity));
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
                    respawn_audio_playback(&room, mixer, new_fmt, &mut audio_tracks);
                }
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

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    trim_task.abort();

    Ok(())
}

async fn abort_task(task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
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

struct PublishedMicSource {
    source: NativeAudioSource,
    channels: u32,
    sample_rate: u32,
}

fn outbound_audio_packets_sent(stats: &[RtcStats]) -> u64 {
    stats
        .iter()
        .filter_map(|stat| match stat {
            RtcStats::OutboundRtp(outbound) => Some(outbound.sent.packets_sent),
            _ => None,
        })
        .sum()
}

async fn publish_microphone_track(
    room: &Room,
    mic_publication: &Arc<Mutex<Option<LocalTrackPublication>>>,
    mic_enabled: &Arc<AtomicBool>,
    in_fmt: AudioFormat,
) -> Result<PublishedMicSource> {
    let previous = mic_publication.lock().take();
    if let Some(previous) = previous {
        let _ = room
            .local_participant()
            .unpublish_track(&previous.sid())
            .await;
    }

    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        in_fmt.sample_rate,
        in_fmt.channels,
        AUDIO_SOURCE_QUEUE_SIZE_MS,
    );
    let mic_track =
        LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source.clone()));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(mic_track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await?;
    if !mic_enabled.load(Ordering::Relaxed) {
        publication.mute();
    }
    *mic_publication.lock() = Some(publication);
    Ok(PublishedMicSource {
        source,
        channels: in_fmt.channels.max(1),
        sample_rate: in_fmt.sample_rate,
    })
}

async fn start_camera_track(
    room: &Room,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
    device_id: Option<String>,
) -> Result<CameraSession> {
    let (controller, track_rx) = camera::start_camera(identity.to_string(), frame_store, device_id);
    let track = track_rx
        .recv_async()
        .await
        .map_err(|_| anyhow::anyhow!("camera thread exited"))?
        .map_err(|e| anyhow::anyhow!(e))?;
    room.local_participant()
        .publish_track(
            LocalTrack::Video(track.clone()),
            TrackPublishOptions {
                source: TrackSource::Camera,
                simulcast: true,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: 1_200_000,
                    max_framerate: 30.0,
                }),
                ..Default::default()
            },
        )
        .await?;
    Ok(CameraSession { track, controller })
}

async fn start_screen_track(
    room: &Room,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
    full_res: Arc<AtomicBool>,
    pick: PickedScreen,
    share_audio: bool,
    record_taps: Option<record::RecordTaps>,
    evt_tx: flume::Sender<VoiceEvent>,
) -> Result<ScreenSession> {
    // Whether the switch in the picker was on is the first thing to know when a
    // recording comes out silent, and it left no trace anywhere before.
    tracing::info!("starting screen share (share system audio: {share_audio})");
    let (stopper, track_rx) =
        screen::start_screen(identity.to_string(), frame_store, full_res, pick);
    let track = track_rx
        .recv_async()
        .await
        .map_err(|_| anyhow::anyhow!("screen thread exited"))?
        .map_err(|e| anyhow::anyhow!(e))?;
    room.local_participant()
        .publish_track(
            LocalTrack::Video(track.clone()),
            TrackPublishOptions {
                source: TrackSource::Screenshare,
                simulcast: false,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: 2_500_000,
                    max_framerate: 15.0,
                }),
                degradation_preference: Some(DegradationPreference::MaintainResolution),
                ..Default::default()
            },
        )
        .await?;
    let audio = if share_audio {
        match start_screen_audio_track(room, record_taps).await {
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

async fn start_screen_audio_track(
    room: &Room,
    record_taps: Option<record::RecordTaps>,
) -> Result<ScreenAudioSession> {
    let capture = tokio::task::spawn_blocking(screen_audio::start_screen_audio)
        .await
        .map_err(|e| anyhow::anyhow!("screen audio init task failed: {e}"))?
        .map_err(|e| anyhow::anyhow!(e))?;

    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        screen_audio::SCREEN_AUDIO_SAMPLE_RATE,
        screen_audio::SCREEN_AUDIO_CHANNELS,
        AUDIO_SOURCE_QUEUE_SIZE_MS,
    );
    let track =
        LocalAudioTrack::create_audio_track("screen-audio", RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track.clone()),
            TrackPublishOptions {
                source: TrackSource::ScreenshareAudio,
                dtx: false,
                audio_encoding: Some(livekit::options::audio::SPEECH.encoding.clone()),
                ..Default::default()
            },
        )
        .await?;

    let rx = capture.rx.clone();
    let pump = runtime::runtime().spawn(async move {
        let channels = screen_audio::SCREEN_AUDIO_CHANNELS;
        while let Ok(samples) = rx.recv_async().await {
            let samples_per_channel = samples.len() as u32 / channels;
            if samples_per_channel == 0 {
                continue;
            }
            if let Some(taps) = &record_taps {
                taps.push(
                    mezon_record::AudioSource::Screen,
                    &samples,
                    screen_audio::SCREEN_AUDIO_SAMPLE_RATE,
                    channels,
                );
            }
            let frame = AudioFrame {
                data: samples.into(),
                num_channels: channels,
                sample_rate: screen_audio::SCREEN_AUDIO_SAMPLE_RATE,
                samples_per_channel,
            };
            let _ = source.capture_frame(&frame).await;
        }
    });

    Ok(ScreenAudioSession {
        track,
        _capture: capture,
        pump,
    })
}

fn spawn_playback(
    track: RemoteAudioTrack,
    key: u64,
    mixer: Arc<audio::PlaybackMixer>,
    out_fmt: AudioFormat,
) -> tokio::task::JoinHandle<()> {
    runtime::runtime().spawn(async move {
        let mut restart_attempts = 0;
        loop {
            let rtc_track = track.rtc_track();
            let mut stream = NativeAudioStream::new(
                rtc_track,
                out_fmt.sample_rate as i32,
                out_fmt.channels as i32,
            );
            let mut saw_frame = false;
            while let Some(frame) = stream.next().await {
                saw_frame = true;
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
    room: &Room,
    mixer: &Arc<audio::PlaybackMixer>,
    out_fmt: AudioFormat,
    audio_tracks: &mut HashMap<u64, tokio::task::JoinHandle<()>>,
) {
    for (_, handle) in audio_tracks.drain() {
        handle.abort();
    }
    for participant in room.remote_participants().values() {
        let identity = participant.identity().as_str().to_string();
        for publication in participant.track_publications().values() {
            if publication.kind() != TrackKind::Audio || !publication.is_subscribed() {
                continue;
            }
            let Some(RemoteTrack::Audio(track)) = publication.track() else {
                continue;
            };
            let key = track_frame_key(&identity, publication.sid().as_str());
            mixer.remove(key);
            let handle = spawn_playback(track, key, mixer.clone(), out_fmt);
            audio_tracks.insert(key, handle);
        }
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
    track: RemoteVideoTrack,
    key: u64,
    frame_store: Arc<VideoFrameStore>,
) -> VideoTrackHandle {
    let rtc_track = track.rtc_track();
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

fn is_agent_participant(
    kind: ParticipantKind,
    permission: Option<livekit_protocol::ParticipantPermission>,
) -> bool {
    #[allow(deprecated)]
    let legacy_agent_permission = permission.is_some_and(|p| p.agent);
    legacy_agent_permission || kind == ParticipantKind::Agent
}

fn emit_participants(
    room: &Room,
    evt_tx: &flume::Sender<VoiceEvent>,
    local_identity: &str,
    local_mic_enabled: bool,
    local_camera_on: bool,
    local_screen_on: bool,
    last: &mut Vec<VoiceParticipant>,
) {
    let mut participants = Vec::new();

    let local = room.local_participant();
    participants.push(VoiceParticipant {
        identity: local.identity().as_str().to_string(),
        name: display_name(&local.name(), local.identity().as_str()),
        is_local: true,
        is_agent: is_agent_participant(local.kind(), local.permission()),
        speaking: local.is_speaking(),
        muted: !local_mic_enabled || local_mic_muted(&local),
        camera: local_camera_on.then(|| local_camera_key(local_identity)),
        screenshare: local_screen_on.then(|| local_screen_key(local_identity)),
        quality: network_quality(local.connection_quality()),
    });

    let mut remotes: Vec<RemoteParticipant> = room.remote_participants().into_values().collect();
    remotes.sort_by(|a, b| a.identity().as_str().cmp(b.identity().as_str()));
    for participant in &remotes {
        let identity = participant.identity().as_str().to_string();
        let (camera, screenshare) = remote_video_keys(participant, &identity);
        participants.push(VoiceParticipant {
            name: display_name(&participant.name(), &identity),
            is_local: false,
            is_agent: is_agent_participant(participant.kind(), participant.permission()),
            speaking: participant.is_speaking(),
            muted: remote_mic_muted(participant),
            camera,
            screenshare,
            quality: network_quality(participant.connection_quality()),
            identity,
        });
    }

    if *last == participants {
        return;
    }
    last.clone_from(&participants);
    let _ = evt_tx.send(VoiceEvent::Participants(participants));
}

fn remote_video_keys(
    participant: &RemoteParticipant,
    identity: &str,
) -> (Option<u64>, Option<u64>) {
    let mut camera = None;
    let mut screenshare = None;
    for publication in participant.track_publications().values() {
        if publication.kind() != TrackKind::Video
            || !publication.is_subscribed()
            || publication.is_muted()
        {
            continue;
        }
        let key = track_frame_key(identity, publication.sid().as_str());
        match publication.source() {
            TrackSource::Screenshare => screenshare = Some(key),
            _ => camera = Some(key),
        }
    }
    (camera, screenshare)
}

fn network_quality(quality: ConnectionQuality) -> NetworkQuality {
    match quality {
        ConnectionQuality::Excellent => NetworkQuality::Excellent,
        ConnectionQuality::Good => NetworkQuality::Good,
        ConnectionQuality::Poor => NetworkQuality::Poor,
        ConnectionQuality::Lost => NetworkQuality::Unknown,
    }
}

fn local_mic_muted(local: &LocalParticipant) -> bool {
    local
        .track_publications()
        .values()
        .find(|publication| publication.source() == TrackSource::Microphone)
        .is_some_and(|publication| publication.is_muted())
}

fn remote_mic_muted(participant: &RemoteParticipant) -> bool {
    participant
        .track_publications()
        .values()
        .find(|publication| publication.source() == TrackSource::Microphone)
        .is_none_or(|publication| publication.is_muted())
}

fn display_name(name: &str, identity: &str) -> String {
    if name.trim().is_empty() {
        identity.to_string()
    } else {
        name.to_string()
    }
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
