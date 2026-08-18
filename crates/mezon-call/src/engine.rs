use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use flume::{Receiver, Sender};
use futures::StreamExt as _;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::audio_track::RtcAudioTrack;
use libwebrtc::ice_candidate::IceCandidate;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{
    AnswerOptions, OfferOptions, PeerConnection, PeerConnectionState, SignalingState,
};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt as _;
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::prelude::{AudioFrame, AudioSourceOptions, MediaType, VideoBuffer};
use libwebrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
use libwebrtc::session_description::{SdpType, SessionDescription};
use libwebrtc::stats::RtcStats;
use libwebrtc::video_source::VideoResolution;
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_stream::native::NativeVideoStream;
use libwebrtc::video_track::RtcVideoTrack;
use mezon_voice::{
    AudioFormat, AudioIo, CameraController, IceServerConfig, PlaybackMixer, RecordTaps,
    VideoFrameStore, i420_to_bgra_into, local_camera_key, start_camera_into,
};
use parking_lot::Mutex;

use crate::REMOTE_FRAME_KEY;
use crate::codec::IcePayload;

const REMOTE_AUDIO_KEY: u64 = 0xCA11_0002;
const AUDIO_SOURCE_QUEUE_SIZE_MS: u32 = 100;
const AUDIO_FORMAT_WAIT: Duration = Duration::from_secs(3);
const CALL_STREAM_ID: &str = "call-stream";
const VIDEO_SOURCE_WIDTH: u32 = 1280;
const VIDEO_SOURCE_HEIGHT: u32 = 720;
const CALL_AUDIO_RATE: u32 = 48_000;
const CALL_AUDIO_CHANNELS: u32 = 1;
const MEDIA_STATS_INTERVAL: Duration = Duration::from_secs(5);
const LEVEL_LOG_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(not(target_os = "macos"))]
const MIC_OPEN_DEADLINE: Duration = Duration::from_secs(6);

type InputFmt = Arc<Mutex<(u32, u32)>>;

pub struct CallConfig {
    pub ice_servers: Vec<IceServerConfig>,
    pub is_caller: bool,
    pub self_identity: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub camera_device: Option<String>,
    pub initial_camera_on: bool,
}

pub enum EngineCommand {
    ApplyRemoteAnswer(String),
    ApplyRemoteOffer(String),
    AddRemoteIce(IcePayload),
    SetMicEnabled(bool),
    SetCameraEnabled(bool),
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetCameraDevice(Option<String>),
    Hangup,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    LocalOffer(String),
    LocalAnswer(String),
    LocalIce(IcePayload),
    Connected,
    Disconnected,
    Failed,
    Closed,
    MicUnavailable,
    CameraUnavailable,
}

pub struct CallEngine {
    cmd_tx: Sender<EngineCommand>,
    event_rx: Receiver<EngineEvent>,
    frame_store: Arc<VideoFrameStore>,
    stop_tx: Sender<()>,
}

impl CallEngine {
    pub fn start(config: CallConfig) -> Self {
        let frame_store = Arc::new(VideoFrameStore::default());
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (event_tx, event_rx) = flume::unbounded();
        let (stop_tx, stop_rx) = flume::bounded(1);
        let frame_store_thread = frame_store.clone();
        let spawned = std::thread::Builder::new()
            .name("mezon-call".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = event_tx.send(EngineEvent::Failed);
                    return;
                };
                let handle = runtime.handle().clone();
                runtime.block_on(async move {
                    if let Err(e) = run_engine(
                        config,
                        frame_store_thread,
                        cmd_rx,
                        event_tx.clone(),
                        stop_rx,
                        handle,
                    )
                    .await
                    {
                        tracing::warn!("call engine ended with error: {e:#}");
                        let _ = event_tx.send(EngineEvent::Failed);
                    }
                });
            });
        if let Err(e) = spawned {
            tracing::error!("failed to spawn call engine thread: {e}");
        }
        Self {
            cmd_tx,
            event_rx,
            frame_store,
            stop_tx,
        }
    }

    pub fn send(&self, cmd: EngineCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn events(&self) -> &Receiver<EngineEvent> {
        &self.event_rx
    }

    pub fn frame_store(&self) -> Arc<VideoFrameStore> {
        self.frame_store.clone()
    }
}

impl Drop for CallEngine {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

async fn run_engine(
    mut config: CallConfig,
    frame_store: Arc<VideoFrameStore>,
    cmd_rx: Receiver<EngineCommand>,
    event_tx: Sender<EngineEvent>,
    stop_rx: Receiver<()>,
    handle: tokio::runtime::Handle,
) -> Result<()> {
    tracing::info!(
        "call: engine start (is_caller={}, initial_camera_on={}, ice_servers={})",
        config.is_caller,
        config.initial_camera_on,
        config.ice_servers.len()
    );
    let audio_io = tokio::task::spawn_blocking({
        let input = config.input_device.clone();
        let output = config.output_device.clone();
        move || AudioIo::start(input, output, RecordTaps::default())
    })
    .await
    .map_err(|e| anyhow!("audio init task failed: {e}"))?
    .context("audio io start failed")?;
    audio_io.set_input_active(true);
    let mixer = audio_io.mixer.clone();
    let out_fmt = audio_io.output_format;
    let mic_rx = audio_io.mic_rx.clone();
    let input_format_rx = audio_io.input_format_rx.clone();

    let factory = PeerConnectionFactory::default();
    let pc = factory
        .create_peer_connection(build_rtc_config(&config.ice_servers))
        .context("create peer connection failed")?;

    let ice_tx = event_tx.clone();
    pc.on_ice_candidate(Some(Box::new(move |candidate| {
        let sdp = candidate.candidate();
        if sdp.is_empty() {
            return;
        }
        tracing::debug!("call: local ice candidate typ={}", candidate_kind(&sdp));
        let _ = ice_tx.send(EngineEvent::LocalIce(IcePayload {
            candidate: sdp,
            sdp_mid: Some(candidate.sdp_mid()),
            sdp_mline_index: Some(candidate.sdp_mline_index()),
        }));
    })));

    pc.on_ice_candidate_error(Some(Box::new(|error| {
        tracing::warn!(
            "call: ice candidate error url={} code={} {}",
            error.url,
            error.error_code,
            error.error_text
        );
    })));

    pc.on_ice_connection_state_change(Some(Box::new(|state| {
        tracing::info!("call: ice connection state = {state:?}");
    })));

    pc.on_ice_gathering_state_change(Some(Box::new(|state| {
        tracing::debug!("call: ice gathering state = {state:?}");
    })));

    let pump_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let track_mixer = mixer.clone();
    let track_frame_store = frame_store.clone();
    let track_handle = handle.clone();
    let track_pump_tasks = pump_tasks.clone();
    pc.on_track(Some(Box::new(move |event| match event.track {
        MediaStreamTrack::Video(video_track) => {
            tracing::info!("call: remote track added (video)");
            let store = track_frame_store.clone();
            let task = track_handle.spawn(pump_video(video_track, store));
            track_pump_tasks.lock().push(task);
        }
        MediaStreamTrack::Audio(audio_track) => {
            tracing::info!("call: remote track added (audio)");
            let mixer = track_mixer.clone();
            let task = track_handle.spawn(pump_audio(audio_track, mixer, out_fmt));
            track_pump_tasks.lock().push(task);
        }
    })));

    let state_tx = event_tx.clone();
    pc.on_connection_state_change(Some(Box::new(move |state| {
        tracing::info!("call: connection state = {state:?}");
        let event = match state {
            PeerConnectionState::Connected => Some(EngineEvent::Connected),
            PeerConnectionState::Disconnected => Some(EngineEvent::Disconnected),
            PeerConnectionState::Failed => Some(EngineEvent::Failed),
            PeerConnectionState::Closed => Some(EngineEvent::Closed),
            _ => None,
        };
        if let Some(event) = event {
            let _ = state_tx.send(event);
        }
    })));

    let first_fmt = tokio::time::timeout(AUDIO_FORMAT_WAIT, input_format_rx.recv_async())
        .await
        .ok()
        .and_then(|result| result.ok());
    let mic_opened = Arc::new(AtomicBool::new(first_fmt.is_some()));
    #[cfg(not(target_os = "macos"))]
    {
        let mic_opened = mic_opened.clone();
        let event_tx = event_tx.clone();
        handle.spawn(async move {
            tokio::time::sleep(MIC_OPEN_DEADLINE).await;
            if !mic_opened.load(Ordering::Relaxed) {
                tracing::warn!("call: microphone did not open (permission/device)");
                let _ = event_tx.send(EngineEvent::MicUnavailable);
            }
        });
    }
    let input_fmt: InputFmt = Arc::new(Mutex::new(match first_fmt {
        Some(format) => (format.sample_rate, format.channels.max(1)),
        None => (CALL_AUDIO_RATE, CALL_AUDIO_CHANNELS),
    }));
    let mic_source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        CALL_AUDIO_RATE,
        CALL_AUDIO_CHANNELS,
        AUDIO_SOURCE_QUEUE_SIZE_MS,
    );
    let audio_track = factory.create_audio_track("call-audio", mic_source.clone());
    let audio_sender = pc
        .add_track(
            MediaStreamTrack::from(audio_track),
            &[CALL_STREAM_ID.to_string()],
        )
        .context("add audio track failed")?;
    tracing::info!(
        "call: audio track added (fixed {}Hz/{}ch, input {:?})",
        CALL_AUDIO_RATE,
        CALL_AUDIO_CHANNELS,
        *input_fmt.lock()
    );

    let video_transceiver = pc
        .add_transceiver_for_media(MediaType::Video, sendrecv_init())
        .context("add video transceiver failed")?;
    let video_source = NativeVideoSource::new(
        VideoResolution {
            width: VIDEO_SOURCE_WIDTH,
            height: VIDEO_SOURCE_HEIGHT,
        },
        false,
    );
    let video_track = factory.create_video_track("call-camera", video_source.clone());
    video_transceiver
        .sender()
        .set_track(Some(MediaStreamTrack::from(video_track.clone())))
        .context("attach video track failed")?;

    let mic_enabled = Arc::new(AtomicBool::new(true));
    let mic_task = handle.spawn(mic_capture(
        mic_rx,
        mic_source,
        input_fmt.clone(),
        mic_enabled.clone(),
    ));
    let mut input_format_rx = Some(input_format_rx);

    let pc = Arc::new(pc);
    let mut camera: Option<CameraController> = None;
    if config.initial_camera_on {
        set_camera(
            true,
            &config,
            &frame_store,
            &video_source,
            &mut camera,
            &handle,
            &event_tx,
        );
    }

    if config.is_caller {
        let offer = pc
            .create_offer(OfferOptions {
                offer_to_receive_audio: true,
                offer_to_receive_video: true,
                ..OfferOptions::default()
            })
            .await
            .context("create offer failed")?;
        pc.set_local_description(offer.clone())
            .await
            .context("set local (offer) failed")?;
        tracing::info!("call: local offer sent");
        let _ = event_tx.send(EngineEvent::LocalOffer(offer.to_string()));
    }

    let mut remote_set = false;
    let mut pending_ice: Vec<IcePayload> = Vec::new();
    let mut stats_timer = tokio::time::interval(MEDIA_STATS_INTERVAL);
    stats_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    stats_timer.tick().await;
    loop {
        tokio::select! {
            _ = stop_rx.recv_async() => break,
            _ = stats_timer.tick() => log_media_stats(&pc).await,
            format = next_input_format(&input_format_rx) => {
                match format {
                    Some(format) => {
                        mic_opened.store(true, Ordering::Relaxed);
                        let next = (format.sample_rate, format.channels.max(1));
                        let mut current = input_fmt.lock();
                        if *current != next {
                            tracing::info!(
                                "call: mic input format {:?} -> {:?} (resampling to {}Hz)",
                                *current,
                                next,
                                CALL_AUDIO_RATE
                            );
                            *current = next;
                        }
                    }
                    None => input_format_rx = None,
                }
            }
            cmd = cmd_rx.recv_async() => {
                let Ok(cmd) = cmd else { break };
                match cmd {
                    EngineCommand::ApplyRemoteOffer(sdp) => {
                        if config.is_caller && pc.signaling_state() != SignalingState::Stable {
                            tracing::warn!("ignoring remote offer during local negotiation (glare)");
                            continue;
                        }
                        let offer = SessionDescription::parse(&sdp, SdpType::Offer)
                            .map_err(|e| anyhow!("offer parse: {} {}", e.line, e.description))?;
                        pc.set_remote_description(offer)
                            .await
                            .context("set remote (offer) failed")?;
                        remote_set = true;
                        drain_ice(&pc, &mut pending_ice).await;
                        let answer = pc
                            .create_answer(AnswerOptions::default())
                            .await
                            .context("create answer failed")?;
                        pc.set_local_description(answer.clone())
                            .await
                            .context("set local (answer) failed")?;
                        let _ = event_tx.send(EngineEvent::LocalAnswer(answer.to_string()));
                    }
                    EngineCommand::ApplyRemoteAnswer(sdp) => {
                        let answer = SessionDescription::parse(&sdp, SdpType::Answer)
                            .map_err(|e| anyhow!("answer parse: {} {}", e.line, e.description))?;
                        pc.set_remote_description(answer)
                            .await
                            .context("set remote (answer) failed")?;
                        remote_set = true;
                        drain_ice(&pc, &mut pending_ice).await;
                    }
                    EngineCommand::AddRemoteIce(ice) => {
                        if remote_set {
                            add_ice(&pc, &ice).await;
                        } else {
                            pending_ice.push(ice);
                        }
                    }
                    EngineCommand::SetMicEnabled(on) => mic_enabled.store(on, Ordering::Relaxed),
                    EngineCommand::SetCameraEnabled(on) => {
                        set_camera(
                            on,
                            &config,
                            &frame_store,
                            &video_source,
                            &mut camera,
                            &handle,
                            &event_tx,
                        );
                    }
                    EngineCommand::SetInputDevice(device) => audio_io.set_input_device(device),
                    EngineCommand::SetOutputDevice(device) => audio_io.set_output_device(device),
                    EngineCommand::SetCameraDevice(device) => {
                        config.camera_device = device.clone();
                        if let Some(controller) = camera.as_ref() {
                            controller.switch(device);
                        }
                    }
                    EngineCommand::Hangup => break,
                }
            }
        }
    }

    mic_enabled.store(false, Ordering::Relaxed);
    mic_task.abort();
    let _ = mic_task.await;
    for task in pump_tasks.lock().drain(..) {
        task.abort();
    }
    if let Some(controller) = camera.take() {
        controller.stop();
        frame_store.remove(local_camera_key(&config.self_identity));
    }
    pc.close();
    drop(audio_sender);
    drop(video_transceiver);
    drop(pc);
    drop(video_track);
    drop(video_source);
    Ok(())
}

fn sendrecv_init() -> RtpTransceiverInit {
    RtpTransceiverInit {
        direction: RtpTransceiverDirection::SendRecv,
        stream_ids: vec![CALL_STREAM_ID.to_string()],
        send_encodings: vec![],
    }
}

fn build_rtc_config(servers: &[IceServerConfig]) -> RtcConfiguration {
    let mut ice_servers: Vec<IceServer> = servers
        .iter()
        .filter(|server| !server.urls.is_empty())
        .map(|server| IceServer {
            urls: server.urls.clone(),
            username: server.username.clone(),
            password: server.credential.clone(),
        })
        .collect();
    if ice_servers.is_empty() {
        ice_servers.push(IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: String::new(),
            password: String::new(),
        });
    }
    RtcConfiguration {
        ice_servers,
        continual_gathering_policy: ContinualGatheringPolicy::GatherContinually,
        ice_transport_type: IceTransportsType::All,
    }
}

async fn drain_ice(pc: &PeerConnection, pending: &mut Vec<IcePayload>) {
    for ice in std::mem::take(pending) {
        add_ice(pc, &ice).await;
    }
}

async fn log_media_stats(pc: &PeerConnection) {
    let Ok(stats) = pc.get_stats().await else {
        return;
    };
    let mut packets_sent = 0u64;
    let mut bytes_sent = 0u64;
    let mut packets_received = 0u64;
    let mut bytes_received = 0u64;
    let mut remote_level = 0.0f64;
    let mut mic_level = 0.0f64;
    let mut mic_samples = 0u64;
    for stat in &stats {
        match stat {
            RtcStats::OutboundRtp(sent) if sent.stream.kind == "audio" => {
                packets_sent += sent.sent.packets_sent;
                bytes_sent += sent.sent.bytes_sent;
            }
            RtcStats::InboundRtp(received) if received.stream.kind == "audio" => {
                packets_received += received.received.packets_received;
                bytes_received += received.inbound.bytes_received;
                remote_level = remote_level.max(received.inbound.audio_level);
            }
            RtcStats::MediaSource(source) if source.source.kind == "audio" => {
                mic_level = mic_level.max(source.audio.audio_level);
                mic_samples = mic_samples.max(source.audio.total_samples_captured);
            }
            _ => {}
        }
    }
    tracing::info!(
        "call: audio stats out(packets={packets_sent} bytes={bytes_sent} mic_level={mic_level:.4} captured={mic_samples}) in(packets={packets_received} bytes={bytes_received} level={remote_level:.4})"
    );
}

fn candidate_kind(candidate: &str) -> &str {
    candidate
        .split_whitespace()
        .skip_while(|token| *token != "typ")
        .nth(1)
        .unwrap_or("unknown")
}

struct LevelMeter {
    label: &'static str,
    energy: f64,
    samples: u64,
    next_log: tokio::time::Instant,
}

impl LevelMeter {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            energy: 0.0,
            samples: 0,
            next_log: tokio::time::Instant::now() + LEVEL_LOG_INTERVAL,
        }
    }

    fn record(&mut self, samples: &[i16]) {
        self.energy += samples
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum::<f64>();
        self.samples += samples.len() as u64;
        let now = tokio::time::Instant::now();
        if now < self.next_log {
            return;
        }
        let rms = if self.samples == 0 {
            0.0
        } else {
            (self.energy / self.samples as f64).sqrt()
        };
        tracing::debug!("call: {} rms={rms:.0} samples={}", self.label, self.samples);
        self.energy = 0.0;
        self.samples = 0;
        self.next_log = now + LEVEL_LOG_INTERVAL;
    }
}

async fn add_ice(pc: &PeerConnection, ice: &IcePayload) {
    match IceCandidate::parse(
        ice.sdp_mid.as_deref().unwrap_or_default(),
        ice.sdp_mline_index.unwrap_or_default(),
        &ice.candidate,
    ) {
        Ok(candidate) => {
            tracing::debug!(
                "call: remote ice candidate typ={}",
                candidate_kind(&ice.candidate)
            );
            if let Err(e) = pc.add_ice_candidate(candidate).await {
                tracing::warn!("add ice candidate failed: {e:?}");
            }
        }
        Err(e) => tracing::warn!("ice candidate parse failed: {} {}", e.line, e.description),
    }
}

async fn next_input_format(rx: &Option<Receiver<AudioFormat>>) -> Option<AudioFormat> {
    match rx {
        Some(rx) => rx.recv_async().await.ok(),
        None => std::future::pending().await,
    }
}

async fn mic_capture(
    mic_rx: Receiver<Vec<i16>>,
    source: NativeAudioSource,
    input_fmt: InputFmt,
    mic_enabled: Arc<AtomicBool>,
) {
    let mut resampler = MicResampler::new();
    let mut out: Vec<i16> = Vec::new();
    let mut meter = LevelMeter::new("mic level");
    while let Ok(samples) = mic_rx.recv_async().await {
        if !mic_enabled.load(Ordering::Relaxed) {
            continue;
        }
        let (in_rate, in_channels) = *input_fmt.lock();
        if in_rate == 0 || in_channels == 0 {
            continue;
        }
        out.clear();
        resampler.process(&samples, in_rate, in_channels, &mut out);
        if out.is_empty() {
            continue;
        }
        meter.record(&out);
        let frame = AudioFrame {
            data: std::borrow::Cow::Borrowed(out.as_slice()),
            num_channels: CALL_AUDIO_CHANNELS,
            sample_rate: CALL_AUDIO_RATE,
            samples_per_channel: out.len() as u32,
        };
        if let Err(e) = source.capture_frame(&frame).await {
            tracing::warn!("mic capture_frame failed: {e}");
        }
    }
}

struct MicResampler {
    in_rate: u32,
    step: f64,
    out_pos: f64,
    consumed: u64,
    prev: f32,
    have_prev: bool,
    started: bool,
}

impl MicResampler {
    fn new() -> Self {
        Self {
            in_rate: 0,
            step: 1.0,
            out_pos: 0.0,
            consumed: 0,
            prev: 0.0,
            have_prev: false,
            started: false,
        }
    }

    fn reset(&mut self, in_rate: u32) {
        self.in_rate = in_rate;
        self.step = in_rate as f64 / CALL_AUDIO_RATE as f64;
        self.out_pos = 0.0;
        self.consumed = 0;
        self.prev = 0.0;
        self.have_prev = false;
        self.started = false;
    }

    fn process(&mut self, samples: &[i16], in_rate: u32, in_channels: u32, out: &mut Vec<i16>) {
        if in_rate != self.in_rate {
            self.reset(in_rate);
        }
        let channels = in_channels.max(1) as usize;
        let mono: Vec<f32> = if channels == 1 {
            samples.iter().map(|&s| s as f32).collect()
        } else {
            samples
                .chunks_exact(channels)
                .map(|c| c.iter().map(|&s| s as f32).sum::<f32>() / channels as f32)
                .collect()
        };
        let Some(&last) = mono.last() else {
            return;
        };
        if (self.step - 1.0).abs() < f64::EPSILON {
            out.extend(mono.iter().map(|&v| clamp_i16(v)));
            self.prev = last;
            self.have_prev = true;
            self.consumed += mono.len() as u64;
            return;
        }
        if !self.started {
            self.out_pos = self.consumed as f64;
            self.started = true;
        }
        let base = self.consumed;
        let n = mono.len() as u64;
        let last_abs = (base + n - 1) as f64;
        while self.out_pos < last_abs {
            let left = self.out_pos.floor();
            let frac = (self.out_pos - left) as f32;
            let li = left as i64;
            let sl = self.sample_at(li, base, &mono);
            let sr = self.sample_at(li + 1, base, &mono);
            out.push(clamp_i16(sl + (sr - sl) * frac));
            self.out_pos += self.step;
        }
        self.prev = last;
        self.have_prev = true;
        self.consumed += n;
    }

    fn sample_at(&self, abs: i64, base: u64, mono: &[f32]) -> f32 {
        if abs < base as i64 {
            if self.have_prev { self.prev } else { mono[0] }
        } else {
            let idx = (abs - base as i64) as usize;
            mono.get(idx).copied().unwrap_or(self.prev)
        }
    }
}

fn clamp_i16(v: f32) -> i16 {
    v.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

fn set_camera(
    enable: bool,
    config: &CallConfig,
    frame_store: &Arc<VideoFrameStore>,
    video_source: &NativeVideoSource,
    camera: &mut Option<CameraController>,
    handle: &tokio::runtime::Handle,
    event_tx: &Sender<EngineEvent>,
) {
    if enable {
        if camera.is_some() {
            return;
        }
        let (err_tx, err_rx) = flume::bounded::<String>(1);
        let controller = start_camera_into(
            config.self_identity.clone(),
            frame_store.clone(),
            config.camera_device.clone(),
            video_source.clone(),
            err_tx,
        );
        let event_tx = event_tx.clone();
        handle.spawn(async move {
            if let Ok(err) = err_rx.recv_async().await {
                tracing::warn!("call: camera unavailable: {err}");
                let _ = event_tx.send(EngineEvent::CameraUnavailable);
            }
        });
        *camera = Some(controller);
    } else if let Some(controller) = camera.take() {
        controller.stop();
        frame_store.remove(local_camera_key(&config.self_identity));
    }
}

async fn pump_video(video_track: RtcVideoTrack, frame_store: Arc<VideoFrameStore>) {
    let mut bgra: Vec<u8> = Vec::new();
    let mut stream = NativeVideoStream::new(video_track);
    let mut logged_first = false;
    while let Some(frame) = stream.next().await {
        let buffer = frame.buffer.to_i420();
        let width = buffer.width();
        let height = buffer.height();
        if !logged_first {
            logged_first = true;
            tracing::info!("call: remote video first frame {width}x{height}");
        }
        let (stride_y, stride_u, stride_v) = buffer.strides();
        let (y, u, v) = buffer.data();
        bgra.clear();
        bgra.resize(width as usize * height as usize * 4, 0);
        i420_to_bgra_into(
            &mut bgra,
            y,
            u,
            v,
            stride_y as usize,
            stride_u as usize,
            stride_v as usize,
            width as usize,
            height as usize,
        );
        if let Some(recycled) =
            frame_store.publish(REMOTE_FRAME_KEY, width, height, std::mem::take(&mut bgra))
        {
            bgra = recycled;
        }
    }
    frame_store.remove(REMOTE_FRAME_KEY);
}

async fn pump_audio(audio_track: RtcAudioTrack, mixer: Arc<PlaybackMixer>, out_fmt: AudioFormat) {
    let mut stream = NativeAudioStream::new(
        audio_track,
        out_fmt.sample_rate as i32,
        out_fmt.channels as i32,
    );
    let mut logged_first = false;
    let mut meter = LevelMeter::new("remote audio level");
    while let Some(frame) = stream.next().await {
        if !logged_first {
            logged_first = true;
            tracing::info!(
                "call: remote audio first frame ({}Hz/{}ch)",
                frame.sample_rate,
                frame.num_channels
            );
        }
        meter.record(&frame.data);
        mixer.push(REMOTE_AUDIO_KEY, &frame.data);
    }
    mixer.remove(REMOTE_AUDIO_KEY);
}
