mod audio;
mod camera;
mod runtime;
mod screen;
mod screen_audio;
mod screen_picker;
mod screen_previews;
mod screen_targets;
mod video;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use futures::StreamExt;
use livekit::options::{TrackPublishOptions, VideoEncoding};
use livekit::prelude::*;
use livekit::track::{
    LocalAudioTrack, LocalTrack, LocalVideoTrack, RemoteVideoTrack, TrackKind, TrackSource,
};
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::peer_connection_factory::IceServer;
use livekit::webrtc::prelude::{AudioFrame, AudioSourceOptions, RtcAudioSource, VideoBuffer};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use parking_lot::Mutex;

pub use audio::AudioFormat;

pub fn microphone_denied() -> bool {
    audio::microphone_denied()
}
pub use screen_picker::PickedScreen;
pub use screen_previews::{ScreenSharePreview, capture_screen_share_preview};
pub use screen_targets::{
    ScreenShareKind, ScreenShareListError, ScreenShareOption, list_screen_share_options,
    peek_screen_share_options,
};
pub use video::{VideoFrameData, VideoFrameStore};

use crate::camera::CameraStopper;
use crate::screen::ScreenStopper;
use crate::video::{i420_to_bgra_into, local_camera_key, local_screen_key, track_frame_key};

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
    Disconnected { reason: String },
    Participants(Vec<VoiceParticipant>),
    Error(String),
}

enum Command {
    SetMicEnabled(bool),
    SetCameraEnabled(bool),
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
}

impl VoiceSession {
    pub fn connect(
        url: String,
        token: String,
        input_device_id: Option<String>,
        output_device_id: Option<String>,
        ice_servers: Vec<IceServerConfig>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (evt_tx, evt_rx) = flume::unbounded();
        let frame_store = Arc::new(VideoFrameStore::default());
        let screen_full_res = Arc::new(AtomicBool::new(false));

        let store = frame_store.clone();
        let screen_full_res_task = screen_full_res.clone();
        runtime::runtime().spawn(async move {
            if let Err(e) = session_main(
                url,
                token,
                input_device_id,
                output_device_id,
                ice_servers,
                cmd_rx,
                &evt_tx,
                store,
                screen_full_res_task,
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
        }
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
    stopper: CameraStopper,
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
    options.rtc_config.ice_servers = ice_servers;
    options
}

#[allow(clippy::too_many_arguments)]
async fn session_main(
    url: String,
    token: String,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    ice_servers: Vec<IceServerConfig>,
    cmd_rx: flume::Receiver<Command>,
    evt_tx: &flume::Sender<VoiceEvent>,
    frame_store: Arc<VideoFrameStore>,
    screen_full_res: Arc<AtomicBool>,
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
    let mut out_fmt = None;
    let mut audio_io: Option<audio::AudioIo> = None;

    let audio = tokio::task::spawn_blocking(move || {
        audio::AudioIo::start(input_device_id, output_device_id)
    })
    .await
    .map_err(|e| anyhow::anyhow!("audio init task failed: {e}"))?;

    match audio {
        Ok(audio) => {
            audio_mixer = Some(audio.mixer.clone());
            out_fmt = Some(audio.output_format);

            let mic_enabled = mic_enabled.clone();
            let mic_publication_task = mic_publication.clone();
            let mic_rx = audio.mic_rx.clone();
            let input_format_rx = audio.input_format_rx.clone();
            let room_for_mic = room.clone();
            runtime::runtime().spawn(async move {
                let Ok(in_fmt) = input_format_rx.recv_async().await else {
                    return;
                };
                let source = NativeAudioSource::new(
                    AudioSourceOptions::default(),
                    in_fmt.sample_rate,
                    in_fmt.channels,
                    1000,
                );
                let mic_track = LocalAudioTrack::create_audio_track(
                    "microphone",
                    RtcAudioSource::Native(source.clone()),
                );
                let publication = match room_for_mic
                    .local_participant()
                    .publish_track(
                        LocalTrack::Audio(mic_track),
                        TrackPublishOptions {
                            source: TrackSource::Microphone,
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(publication) => publication,
                    Err(e) => {
                        tracing::warn!("failed to publish mic track: {e}");
                        return;
                    }
                };
                if !mic_enabled.load(Ordering::Relaxed) {
                    publication.mute();
                }
                *mic_publication_task.lock() = Some(publication);

                let channels = in_fmt.channels.max(1);
                let sample_rate = in_fmt.sample_rate;
                while let Ok(samples) = mic_rx.recv_async().await {
                    if !mic_enabled.load(Ordering::Relaxed) {
                        continue;
                    }
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
                    let _ = source.capture_frame(&frame).await;
                }
            });

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
    let mut camera_session: Option<CameraSession> = None;
    let mut screen_session: Option<ScreenSession> = None;
    let (cam_tx, cam_rx) = flume::bounded::<(u64, Result<CameraSession>)>(1);
    let (screen_tx, screen_rx) = flume::bounded::<(u64, Result<ScreenSession>)>(1);
    let mut camera_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut screen_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut camera_gen: u64 = 0;
    let mut screen_gen: u64 = 0;

    let emit =
        |room: &Room, mic: bool, camera: &Option<CameraSession>, screen: &Option<ScreenSession>| {
            emit_participants(
                room,
                evt_tx,
                &local_identity,
                mic,
                camera.is_some(),
                screen.is_some(),
            );
        };
    emit(&room, mic_on, &camera_session, &screen_session);

    loop {
        tokio::select! {
            event = room_events.recv() => {
                let Some(event) = event else { break };
                match event {
                    RoomEvent::TrackSubscribed { track, participant, .. } => {
                        match track {
                            RemoteTrack::Audio(audio_track) => {
                                if let (Some(mixer), Some(out_fmt)) = (&audio_mixer, out_fmt) {
                                    let key = track_frame_key(participant.identity().as_str(), audio_track.sid().as_str());
                                    spawn_playback(audio_track, key, mixer.clone(), out_fmt);
                                }
                            }
                            RemoteTrack::Video(video_track) => {
                                let key = track_frame_key(participant.identity().as_str(), video_track.sid().as_str());
                                spawn_video(video_track, key, frame_store.clone());
                            }
                        }
                        emit(&room, mic_on, &camera_session, &screen_session);
                    }
                    RoomEvent::TrackUnsubscribed { track, participant, .. } => {
                        let key = track_frame_key(participant.identity().as_str(), track.sid().as_str());
                        if let Some(mixer) = &audio_mixer {
                            mixer.remove(key);
                        }
                        frame_store.remove(key);
                        emit(&room, mic_on, &camera_session, &screen_session);
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
                        emit(&room, mic_on, &camera_session, &screen_session);
                    }
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
                        emit(&room, mic_on, &camera_session, &screen_session);
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
                        emit(&room, mic_on, &camera_session, &screen_session);
                    }
                    Ok(Command::SetNoiseSuppression(enabled, level)) => {
                        if let Some(io) = &audio_io {
                            io.set_noise_suppression(enabled, level);
                        }
                    }
                    Ok(Command::SetCameraEnabled(true)) => {
                        if camera_session.is_none() && camera_task.is_none() {
                            let room = room.clone();
                            let identity = local_identity.clone();
                            let store = frame_store.clone();
                            let tx = cam_tx.clone();
                            let generation = camera_gen;
                            camera_task = Some(runtime::runtime().spawn(async move {
                                let result = start_camera_track(&room, &identity, store).await;
                                let _ = tx.send_async((generation, result)).await;
                            }));
                        }
                    }
                    Ok(Command::SetCameraEnabled(false)) => {
                        let mut changed = false;
                        camera_gen = camera_gen.wrapping_add(1);
                        if camera_task.take().is_some() {
                            changed = true;
                        }
                        if let Some(session) = camera_session.take() {
                            session.stopper.stop();
                            let _ = room.local_participant().unpublish_track(&session.track.sid()).await;
                            frame_store.remove(local_camera_key(&local_identity));
                            changed = true;
                        }
                        if changed {
                            emit(&room, mic_on, &camera_session, &screen_session);
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
                            screen_task = Some(runtime::runtime().spawn(async move {
                                let result =
                                    start_screen_track(&room, &identity, store, full_res, pick, share_audio).await;
                                let _ = tx.send_async((generation, result)).await;
                            }));
                        }
                    }
                    Ok(Command::StopScreenShare) => {
                        let mut changed = false;
                        screen_gen = screen_gen.wrapping_add(1);
                        if screen_task.take().is_some() {
                            changed = true;
                        }
                        if let Some(session) = screen_session.take() {
                            session.stop(&room).await;
                            frame_store.remove(local_screen_key(&local_identity));
                            changed = true;
                        }
                        if changed {
                            emit(&room, mic_on, &camera_session, &screen_session);
                        }
                    }
                    Ok(Command::Disconnect) | Err(_) => {
                        if let Some(task) = camera_task.take() {
                            task.abort();
                        }
                        if let Some(task) = screen_task.take() {
                            task.abort();
                        }
                        if let Some(session) = camera_session.take() {
                            session.stopper.stop();
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
                        camera_session = Some(session);
                        emit(&room, mic_on, &camera_session, &screen_session);
                    }
                    Ok((_, Ok(session))) => {
                        session.stopper.stop();
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
                    Ok((_, Err(e))) => {
                        tracing::debug!("stale camera result dropped: {e:#}");
                    }
                    Err(_) => {}
                }
            }
            result = screen_rx.recv_async() => {
                match result {
                    Ok((generation, Ok(session))) if generation == screen_gen => {
                        screen_task = None;
                        screen_session = Some(session);
                        emit(&room, mic_on, &camera_session, &screen_session);
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
                    Ok((_, Err(e))) => {
                        tracing::debug!("stale screen result dropped: {e:#}");
                    }
                    Err(_) => {}
                }
            }
        }
    }

    Ok(())
}

async fn start_camera_track(
    room: &Room,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
) -> Result<CameraSession> {
    let (stopper, track_rx) = camera::start_camera(identity.to_string(), frame_store);
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
                simulcast: false,
                video_encoding: Some(VideoEncoding {
                    max_bitrate: 1_200_000,
                    max_framerate: 30.0,
                }),
                ..Default::default()
            },
        )
        .await?;
    Ok(CameraSession { track, stopper })
}

async fn start_screen_track(
    room: &Room,
    identity: &str,
    frame_store: Arc<VideoFrameStore>,
    full_res: Arc<AtomicBool>,
    pick: PickedScreen,
    share_audio: bool,
) -> Result<ScreenSession> {
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
                    max_bitrate: 8_000_000,
                    max_framerate: 30.0,
                }),
                ..Default::default()
            },
        )
        .await?;
    let audio = if share_audio {
        match start_screen_audio_track(room).await {
            Ok(audio) => Some(audio),
            Err(e) => {
                tracing::warn!("system audio share unavailable: {e:#}");
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

async fn start_screen_audio_track(room: &Room) -> Result<ScreenAudioSession> {
    let capture = tokio::task::spawn_blocking(screen_audio::start_screen_audio)
        .await
        .map_err(|e| anyhow::anyhow!("screen audio init task failed: {e}"))?
        .map_err(|e| anyhow::anyhow!(e))?;

    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        screen_audio::SCREEN_AUDIO_SAMPLE_RATE,
        screen_audio::SCREEN_AUDIO_CHANNELS,
        1000,
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
) {
    let rtc_track = track.rtc_track();
    runtime::runtime().spawn(async move {
        let mut stream = NativeAudioStream::new(
            rtc_track,
            out_fmt.sample_rate as i32,
            out_fmt.channels as i32,
        );
        while let Some(frame) = stream.next().await {
            mixer.push(key, &frame.data);
        }
        mixer.remove(key);
    });
}

fn spawn_video(track: RemoteVideoTrack, key: u64, frame_store: Arc<VideoFrameStore>) {
    let rtc_track = track.rtc_track();
    runtime::runtime().spawn(async move {
        let mut stream = NativeVideoStream::new(rtc_track);
        let mut bgra: Vec<u8> = Vec::new();
        while let Some(frame) = stream.next().await {
            let buffer = frame.buffer.to_i420();
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
            frame_store.publish(key, width, height, std::mem::take(&mut bgra));
        }
        frame_store.remove(key);
    });
}

fn emit_participants(
    room: &Room,
    evt_tx: &flume::Sender<VoiceEvent>,
    local_identity: &str,
    local_mic_enabled: bool,
    local_camera_on: bool,
    local_screen_on: bool,
) {
    let mut participants = Vec::new();

    let local = room.local_participant();
    participants.push(VoiceParticipant {
        identity: local.identity().as_str().to_string(),
        name: display_name(&local.name(), local.identity().as_str()),
        is_local: true,
        speaking: local.is_speaking(),
        muted: !local_mic_enabled || local_mic_muted(&local),
        camera: local_camera_on.then(|| local_camera_key(local_identity)),
        screenshare: local_screen_on.then(|| local_screen_key(local_identity)),
        quality: network_quality(local.connection_quality()),
    });

    for participant in room.remote_participants().values() {
        let identity = participant.identity().as_str().to_string();
        let (camera, screenshare) = remote_video_keys(participant, &identity);
        participants.push(VoiceParticipant {
            name: display_name(&participant.name(), &identity),
            is_local: false,
            speaking: participant.is_speaking(),
            muted: remote_mic_muted(participant),
            camera,
            screenshare,
            quality: network_quality(participant.connection_quality()),
            identity,
        });
    }

    let _ = evt_tx.send(VoiceEvent::Participants(participants));
}

fn remote_video_keys(
    participant: &RemoteParticipant,
    identity: &str,
) -> (Option<u64>, Option<u64>) {
    let mut camera = None;
    let mut screenshare = None;
    for publication in participant.track_publications().values() {
        if publication.kind() != TrackKind::Video || !publication.is_subscribed() {
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
    let publications = participant.track_publications();
    let Some(publication) = publications
        .values()
        .find(|publication| publication.source() == TrackSource::Microphone)
    else {
        return true;
    };
    let enabled = match publication.track() {
        Some(track) => !track.is_muted() && !publication.is_muted(),
        None => !publication.is_muted() && publication.is_subscribed(),
    };
    !enabled
}

fn display_name(name: &str, identity: &str) -> String {
    if name.trim().is_empty() {
        identity.to_string()
    } else {
        name.to_string()
    }
}
