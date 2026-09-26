use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use flume::{Receiver, Sender};
use futures::{SinkExt, StreamExt};
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{AnswerOptions, PeerConnection, PeerConnectionState};
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::rtp_transceiver::RtpTransceiverDirection;
use libwebrtc::session_description::{SdpType, SessionDescription};
use mezon_voice::{StreamAudioOutput, stabilize_inactive_video_sections};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

const HEALTHY_CONNECTION: Duration = Duration::from_secs(30);
const MAX_RECONNECT_ATTEMPTS: u32 = 40;
const MAX_TOKEN_REFRESH_ATTEMPTS: u32 = 3;

pub type StreamTokenProvider =
    Arc<dyn Fn() -> futures::future::BoxFuture<'static, Result<String>> + Send + Sync + 'static>;

#[derive(Clone)]
pub struct StreamSessionConfig {
    pub ws_url: String,
    pub token: String,
    pub room: String,
    pub token_provider: StreamTokenProvider,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Live,
    NoBroadcast,
    RemoteAudio(bool),
    PlaybackBlocked,
    Error(String),
    Disconnected,
}

pub struct StreamSession {
    stop_tx: Sender<()>,
    event_rx: Receiver<StreamEvent>,
    audio: Arc<Mutex<Option<Arc<StreamAudioOutput>>>>,
}

impl StreamSession {
    pub fn start(
        config: StreamSessionConfig,
        output_device_id: Option<String>,
        volume: f32,
        muted: bool,
    ) -> Self {
        let (stop_tx, stop_rx) = flume::bounded(1);
        let (event_tx, event_rx) = flume::unbounded();
        let audio = Arc::new(Mutex::new(None));
        let audio_for_thread = audio.clone();
        std::thread::spawn(move || {
            let audio_output = match StreamAudioOutput::start(output_device_id, volume, muted) {
                Ok(output) => Arc::new(output),
                Err(_) => {
                    let _ = event_tx.send(StreamEvent::Error("Stream audio failed".into()));
                    let _ = event_tx.send(StreamEvent::Disconnected);
                    return;
                }
            };
            *audio_for_thread.lock() = Some(audio_output.clone());

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let _ = event_tx.send(StreamEvent::Error("stream runtime failed".into()));
                let _ = event_tx.send(StreamEvent::Disconnected);
                return;
            };
            runtime.block_on(run_session(config, audio_output, stop_rx, event_tx));
        });
        Self {
            stop_tx,
            event_rx,
            audio,
        }
    }

    pub fn audio(&self) -> Option<Arc<StreamAudioOutput>> {
        self.audio.lock().clone()
    }

    pub fn events(&self) -> &Receiver<StreamEvent> {
        &self.event_rx
    }

    pub fn disconnect(&self) {
        let _ = self.stop_tx.try_send(());
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.disconnect();
    }
}

async fn run_session(
    config: StreamSessionConfig,
    audio: Arc<StreamAudioOutput>,
    stop_rx: Receiver<()>,
    event_tx: Sender<StreamEvent>,
) {
    let mut token = config.token.clone();
    let mut reconnect_attempt = 0u32;
    let mut token_refresh_attempt = 0u32;
    let factory = PeerConnectionFactory::default();

    loop {
        match run_session_once(
            &config,
            &token,
            audio.clone(),
            &stop_rx,
            &event_tx,
            &factory,
            &mut reconnect_attempt,
        )
        .await
        {
            Ok(()) => break,
            Err(SessionFailure::Fatal(reason)) => {
                let _ = event_tx.send(StreamEvent::Error(reason));
                break;
            }
            Err(SessionFailure::RefreshToken(reason)) => {
                token_refresh_attempt = token_refresh_attempt.saturating_add(1);
                if token_refresh_attempt > MAX_TOKEN_REFRESH_ATTEMPTS {
                    let _ = event_tx.send(StreamEvent::Error(format!(
                        "SFU token refresh failed after {MAX_TOKEN_REFRESH_ATTEMPTS} attempts: {reason}"
                    )));
                    break;
                }
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                tracing::warn!(
                    attempt = reconnect_attempt,
                    "SFU stream session refreshing token"
                );
                match refresh_token(&config, &stop_rx, reconnect_attempt).await {
                    Some(next_token) => {
                        token = next_token;
                        token_refresh_attempt = 0;
                    }
                    None if token_refresh_attempt < MAX_TOKEN_REFRESH_ATTEMPTS => {
                        if !wait_before_retry(&stop_rx, reconnect_attempt).await {
                            break;
                        }
                    }
                    None => break,
                }
            }
            Err(SessionFailure::Retry(reason)) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    let _ = event_tx.send(StreamEvent::Error(format!(
                        "SFU stream reconnect limit reached: {reason}"
                    )));
                    break;
                }
                tracing::warn!(
                    attempt = reconnect_attempt,
                    %reason,
                    "SFU stream session reconnecting"
                );
                if !wait_before_retry(&stop_rx, reconnect_attempt).await {
                    break;
                }
            }
        }
    }
    let _ = event_tx.send(StreamEvent::Disconnected);
}

async fn refresh_token(
    config: &StreamSessionConfig,
    stop_rx: &Receiver<()>,
    reconnect_attempt: u32,
) -> Option<String> {
    if !wait_before_retry(stop_rx, reconnect_attempt).await {
        return None;
    }
    let refresh = (config.token_provider)();
    let result = tokio::select! {
        _ = stop_rx.recv_async() => return None,
        result = refresh => result,
    };
    match result {
        Ok(token) if !token.is_empty() => Some(token),
        Ok(_) | Err(_) => None,
    }
}

async fn wait_before_retry(stop_rx: &Receiver<()>, reconnect_attempt: u32) -> bool {
    tokio::select! {
        _ = stop_rx.recv_async() => false,
        _ = tokio::time::sleep(reconnect_delay(reconnect_attempt)) => true,
    }
}

fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_millis(1_000u64.saturating_mul(2u64.saturating_pow(attempt.min(4))))
        .min(Duration::from_secs(15))
}

#[derive(Debug)]
enum SessionFailure {
    Retry(String),
    RefreshToken(String),
    Fatal(String),
}

async fn run_session_once(
    config: &StreamSessionConfig,
    token: &str,
    audio: Arc<StreamAudioOutput>,
    stop_rx: &Receiver<()>,
    event_tx: &Sender<StreamEvent>,
    factory: &PeerConnectionFactory,
    reconnect_attempt: &mut u32,
) -> std::result::Result<(), SessionFailure> {
    let url = build_ws_url(&config.ws_url, token)
        .map_err(|error| SessionFailure::Fatal(format!("invalid SFU URL: {error}")))?;
    let (ws_stream, _) = tokio::select! {
        _ = stop_rx.recv_async() => return Ok(()),
        result = connect_async(url.as_str()) => result
            .map_err(|error| SessionFailure::Retry(format!("SFU websocket connect failed: {error}")))?,
    };
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    send_json(
        &mut ws_tx,
        serde_json::json!({
            "type": "join",
            "room": config.room,
            "token": token,
            "role": "audience"
        }),
    )
    .await
    .map_err(|error| SessionFailure::Retry(error.to_string()))?;

    let (connection_state_tx, connection_state_rx) = flume::unbounded::<PeerConnectionState>();
    let active_audio_tracks = Arc::new(AtomicUsize::new(0));
    let pump_registry = Arc::new(AudioPumpRegistry::default());
    let mut pc = PeerConnectionGuard::new(pump_registry.clone());
    let mut connected = false;
    let mut healthy_since = None;

    loop {
        let healthy_deadline = healthy_since.map(|since| since + HEALTHY_CONNECTION);
        tokio::select! {
            _ = async {
                match healthy_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                *reconnect_attempt = 0;
                healthy_since = None;
            },
            _ = stop_rx.recv_async() => {
                let _ = ws_tx.send(Message::Close(None)).await;
                break;
            },
            state = connection_state_rx.recv_async() => {
                match state {
                    Ok(PeerConnectionState::Connected) => {
                        connected = true;
                        healthy_since.get_or_insert_with(tokio::time::Instant::now);
                        if active_audio_tracks.load(Ordering::Acquire) == 0 {
                            let _ = event_tx.send(StreamEvent::NoBroadcast);
                        }
                    }
                    Ok(PeerConnectionState::Failed) => {
                        return Err(SessionFailure::Retry("SFU peer connection failed".into()));
                    }
                    Ok(PeerConnectionState::Closed) => {
                        return Err(SessionFailure::Retry("SFU peer connection closed".into()));
                    }
                    Ok(PeerConnectionState::New | PeerConnectionState::Connecting | PeerConnectionState::Disconnected) => {}
                    Err(_) => {}
                }
            }
            incoming = ws_rx.next() => {
                let Some(frame) = incoming else {
                    return Err(SessionFailure::Retry("SFU websocket closed".into()));
                };
                let frame = frame.map_err(|error| {
                    SessionFailure::Retry(format!("SFU websocket read failed: {error}"))
                })?;
                let text = match frame {
                    Message::Text(text) => text,
                    Message::Ping(payload) => {
                        ws_tx.send(Message::Pong(payload)).await.map_err(|error| {
                            SessionFailure::Retry(format!("SFU pong failed: {error}"))
                        })?;
                        continue;
                    }
                    Message::Close(frame) => {
                        let code = frame.as_ref().map(|frame| frame.code);
                        let reason = close_reason(code);
                        return Err(match classify_close(code) {
                            CloseVerdict::Retry => SessionFailure::Retry(reason),
                            CloseVerdict::RefreshToken => SessionFailure::RefreshToken(reason),
                            CloseVerdict::Fatal => SessionFailure::Fatal(reason),
                        });
                    }
                    _ => continue,
                };
                let message: Value = match serde_json::from_str(&text) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!(%error, "SFU sent an unparsable stream frame");
                        continue;
                    }
                };
                match message.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "ping" => send_json(&mut ws_tx, serde_json::json!({"type":"pong"}))
                        .await
                        .map_err(|error| SessionFailure::Retry(error.to_string()))?,
                    "pong" => {}
                    "joined" if pc.peer_connection.is_none() => {
                        pc.peer_connection = Some(create_peer_connection(
                            factory,
                            &message,
                            &connection_state_tx,
                            audio.clone(),
                            event_tx,
                            active_audio_tracks.clone(),
                            pump_registry.clone(),
                            Handle::current(),
                        )
                        .map_err(|error| {
                            SessionFailure::Fatal(format!("peer connection setup failed: {error:#}"))
                        })?);
                    }
                    "joined" => {}
                    "offer" => {
                        let Some(generation) = message.get("offer_generation").and_then(Value::as_u64) else {
                            tracing::warn!("SFU offer is missing offer_generation");
                            continue;
                        };
                        let Some(sdp) = message.get("sdp").and_then(Value::as_str) else {
                            tracing::warn!(generation, "SFU offer is missing sdp");
                            continue;
                        };
                        let Some(peer_connection) = pc.peer_connection.as_ref() else {
                            tracing::warn!(generation, "SFU offer arrived before joined");
                            continue;
                        };
                        negotiate(peer_connection, generation, sdp, &mut ws_tx)
                            .await
                            .map_err(|error| {
                                SessionFailure::Retry(format!("SFU offer negotiation failed: {error:#}"))
                            })?;
                    }
                    "error" => {
                        let reason = message.get("message").and_then(Value::as_str).unwrap_or("SFU error");
                        if matches!(reason, "stale_offer_generation" | "future_offer_generation") {
                            tracing::warn!(%reason, connected, "SFU rejected a stale or future offer generation");
                            continue;
                        }
                        return Err(match classify_server_error(reason) {
                            ServerErrorVerdict::Retry => SessionFailure::Retry(format!("SFU error: {reason}")),
                            ServerErrorVerdict::RefreshToken => {
                                SessionFailure::RefreshToken(format!("SFU error: {reason}"))
                            }
                            ServerErrorVerdict::Fatal => SessionFailure::Fatal(format!("SFU error: {reason}")),
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseVerdict {
    Retry,
    RefreshToken,
    Fatal,
}

fn classify_close(code: Option<CloseCode>) -> CloseVerdict {
    match code.map(u16::from) {
        Some(4004 | 4005) => CloseVerdict::RefreshToken,
        Some(4006 | 4011) => CloseVerdict::Fatal,
        _ => CloseVerdict::Retry,
    }
}

fn close_reason(code: Option<CloseCode>) -> String {
    match code.map(u16::from) {
        Some(4004) => "SFU closed the session because the token is missing".into(),
        Some(4005) => "SFU closed the session because the token is invalid".into(),
        Some(4006) => "SFU removed the stream audience session".into(),
        Some(4011) => "SFU closed the session because no publisher was present".into(),
        Some(code) => format!("SFU closed the stream session (code {code})"),
        None => "SFU closed the stream session".into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerErrorVerdict {
    Retry,
    RefreshToken,
    Fatal,
}

fn classify_server_error(reason: &str) -> ServerErrorVerdict {
    match reason {
        "invalid_token" | "missing_token" => ServerErrorVerdict::RefreshToken,
        "room_not_found"
        | "token_room_mismatch"
        | "not_member"
        | "kicked"
        | "removed"
        | "forbidden"
        | "unauthorized"
        | "auth_not_configured" => ServerErrorVerdict::Fatal,
        _ => ServerErrorVerdict::Retry,
    }
}

#[derive(Default)]
struct AudioPumpRegistry {
    next_key: AtomicU64,
    pumps: Mutex<std::collections::HashMap<u64, JoinHandle<()>>>,
}

impl AudioPumpRegistry {
    fn spawn(
        self: &Arc<Self>,
        track: libwebrtc::audio_track::RtcAudioTrack,
        audio: Arc<StreamAudioOutput>,
        event_tx: Sender<StreamEvent>,
        active_audio_tracks: Arc<AtomicUsize>,
        runtime: Handle,
    ) {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        let weak_registry = Arc::downgrade(self);
        let registry = Arc::clone(self);
        let handle = runtime.spawn(async move {
            let format = audio.format();
            let mut stream =
                NativeAudioStream::new(track, format.sample_rate as i32, format.channels as i32);
            while let Some(frame) = stream.next().await {
                audio.push_track(key, &frame.data);
            }
            audio.clear_track(key);
            let _ = event_tx.send(StreamEvent::RemoteAudio(false));
            if active_audio_tracks.fetch_sub(1, Ordering::AcqRel) == 1 {
                let _ = event_tx.send(StreamEvent::NoBroadcast);
            }
            if let Some(registry) = weak_registry.upgrade() {
                registry.pumps.lock().remove(&key);
            }
        });
        registry.pumps.lock().insert(key, handle);
    }

    fn abort_all(&self) {
        for (_, pump) in self.pumps.lock().drain() {
            pump.abort();
        }
    }
}

struct PeerConnectionGuard {
    peer_connection: Option<PeerConnection>,
    pumps: Arc<AudioPumpRegistry>,
}

impl PeerConnectionGuard {
    fn new(pumps: Arc<AudioPumpRegistry>) -> Self {
        Self {
            peer_connection: None,
            pumps,
        }
    }
}

impl Drop for PeerConnectionGuard {
    fn drop(&mut self) {
        self.pumps.abort_all();
        if let Some(peer_connection) = self.peer_connection.take() {
            peer_connection.close();
        }
    }
}

fn create_peer_connection(
    factory: &PeerConnectionFactory,
    joined: &Value,
    connection_state_tx: &Sender<PeerConnectionState>,
    audio: Arc<StreamAudioOutput>,
    event_tx: &Sender<StreamEvent>,
    active_audio_tracks: Arc<AtomicUsize>,
    pump_registry: Arc<AudioPumpRegistry>,
    runtime: Handle,
) -> Result<libwebrtc::peer_connection::PeerConnection> {
    let ice_servers = joined
        .get("iceServers")
        .map(parse_ice_servers)
        .unwrap_or_default();
    let pc = factory
        .create_peer_connection(RtcConfiguration {
            ice_servers,
            continual_gathering_policy: ContinualGatheringPolicy::GatherContinually,
            ice_transport_type: IceTransportsType::All,
        })
        .context("create SFU peer connection")?;

    let connection_state_tx = connection_state_tx.clone();
    pc.on_connection_state_change(Some(Box::new(move |state| {
        let _ = connection_state_tx.send(state);
    })));

    let events = event_tx.clone();
    pc.on_track(Some(Box::new(move |track_event| match track_event.track {
        MediaStreamTrack::Audio(audio_track) => {
            if active_audio_tracks.fetch_add(1, Ordering::AcqRel) == 0 {
                let _ = events.send(StreamEvent::Live);
            }
            let _ = events.send(StreamEvent::RemoteAudio(true));
            pump_registry.spawn(
                audio_track,
                audio.clone(),
                events.clone(),
                active_audio_tracks.clone(),
                runtime.clone(),
            );
        }
        MediaStreamTrack::Video(_) => {}
    })));

    Ok(pc)
}

async fn negotiate(
    pc: &libwebrtc::peer_connection::PeerConnection,
    generation: u64,
    offer_sdp: &str,
    ws_tx: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
) -> Result<()> {
    let previous = pc
        .current_remote_description()
        .map(|description| description.to_string());
    let stabilized = stabilize_inactive_video_sections(offer_sdp, previous.as_deref());
    let offer = SessionDescription::parse(&stabilized, SdpType::Offer)
        .map_err(|e| anyhow!("parse SFU offer: {} {}", e.line, e.description))?;
    pc.set_remote_description(offer)
        .await
        .context("set SFU remote offer")?;

    let offered_sections = media_sections(&stabilized);
    let transceivers = pc.transceivers();
    if transceivers.len() != offered_sections.len() {
        return Err(anyhow!("SFU transceiver count does not match offer"));
    }
    for (index, transceiver) in transceivers.iter().enumerate() {
        let offered = &offered_sections[index];
        if transceiver.mid().as_deref() != Some(offered.mid.as_str()) {
            return Err(anyhow!("SFU transceiver mid order changed"));
        }
        let direction = if offered.kind == "video" {
            RtpTransceiverDirection::Inactive
        } else {
            RtpTransceiverDirection::RecvOnly
        };
        transceiver
            .set_direction(direction)
            .map_err(|error| anyhow!("set SFU transceiver direction: {error}"))?;
    }

    let answer = pc
        .create_answer(AnswerOptions::default())
        .await
        .context("create SFU answer")?;
    pc.set_local_description(answer)
        .await
        .context("set SFU local answer")?;
    let local_sdp = pc
        .current_local_description()
        .map(|description| description.to_string())
        .context("SFU local answer missing")?;
    validate_full_sdp_layout(&stabilized, &local_sdp)?;
    send_json(
        ws_tx,
        serde_json::json!({
            "type": "answer",
            "sdp": local_sdp,
            "offer_generation": generation
        }),
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
struct MediaSection {
    kind: String,
    mid: String,
    direction: Option<String>,
}

fn media_sections(sdp: &str) -> Vec<MediaSection> {
    let mut sections = Vec::new();
    let mut current: Option<MediaSection> = None;
    for line in sdp.lines().map(|line| line.trim_end_matches('\r')) {
        if let Some(media) = line.strip_prefix("m=") {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            let kind = media
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            current = Some(MediaSection {
                kind,
                mid: String::new(),
                direction: None,
            });
        } else if let Some(mid) = line.strip_prefix("a=mid:") {
            if let Some(section) = current.as_mut() {
                section.mid = mid.to_owned();
            }
        } else if matches!(
            line,
            "a=sendrecv" | "a=sendonly" | "a=recvonly" | "a=inactive"
        ) && let Some(section) = current.as_mut()
        {
            section.direction = Some(line.to_owned());
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    sections
}

fn validate_full_sdp_layout(offer_sdp: &str, answer_sdp: &str) -> Result<()> {
    let offer = media_sections(offer_sdp);
    let answer = media_sections(answer_sdp);
    if offer.len() != answer.len() {
        return Err(anyhow!("SFU answer changed m-line count"));
    }
    for (index, offered) in offer.iter().enumerate() {
        let actual = &answer[index];
        if offered.mid.is_empty()
            || actual.mid.is_empty()
            || offered.kind != actual.kind
            || offered.mid != actual.mid
        {
            return Err(anyhow!("SFU answer changed m-line order or mid"));
        }
        match offered.kind.as_str() {
            "video" if actual.direction.as_deref() != Some("a=inactive") => {
                return Err(anyhow!("SFU answer activated a video m-line"));
            }
            "audio"
                if !matches!(
                    actual.direction.as_deref(),
                    Some("a=recvonly") | Some("a=inactive")
                ) =>
            {
                return Err(anyhow!("SFU answer activated an audio sender"));
            }
            _ => {}
        }
    }
    Ok(())
}
fn parse_ice_servers(value: &Value) -> Vec<IceServer> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let urls = match item.get("urls") {
                Some(Value::String(url)) => vec![url.clone()],
                Some(Value::Array(urls)) => urls
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            };
            (!urls.is_empty()).then(|| IceServer {
                urls,
                username: item
                    .get("username")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                password: item
                    .get("credential")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect()
}

async fn send_json<S>(sink: &mut S, value: Value) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    sink.send(Message::Text(value.to_string().into()))
        .await
        .map_err(|e| anyhow!("SFU websocket send failed: {e}"))
}

fn build_ws_url(base: &str, token: &str) -> Result<url::Url> {
    let mut url = url::Url::parse(base.trim())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("access_token", token);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &str = concat!(
        "v=0\r\n",
        "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
        "a=mid:0\r\n",
        "a=sendrecv\r\n",
        "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
        "a=mid:1\r\n",
        "a=sendrecv\r\n",
        "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
        "a=mid:2\r\n",
        "a=sendrecv\r\n",
    );

    const ANSWER: &str = concat!(
        "v=0\r\n",
        "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
        "a=mid:0\r\n",
        "a=recvonly\r\n",
        "m=video 0 UDP/TLS/RTP/SAVPF 96\r\n",
        "a=mid:1\r\n",
        "a=inactive\r\n",
        "m=video 0 UDP/TLS/RTP/SAVPF 96\r\n",
        "a=mid:2\r\n",
        "a=inactive\r\n",
    );

    #[test]
    fn accepts_full_sdp_with_audio_only_answer_directions() {
        assert!(validate_full_sdp_layout(OFFER, ANSWER).is_ok());
        let sections = media_sections(ANSWER);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].direction.as_deref(), Some("a=recvonly"));
        assert_eq!(sections[1].direction.as_deref(), Some("a=inactive"));
        assert_eq!(sections[2].direction.as_deref(), Some("a=inactive"));
    }

    #[test]
    fn rejects_changed_m_line_layout_or_active_video() {
        let wrong_order = ANSWER.replace("a=mid:1", "a=mid:9");
        assert!(validate_full_sdp_layout(OFFER, &wrong_order).is_err());

        let missing_mid = ANSWER.replace("a=mid:1", "a=mid:");
        assert!(validate_full_sdp_layout(OFFER, &missing_mid).is_err());

        let active_video = ANSWER.replace("a=inactive", "a=recvonly");
        assert!(validate_full_sdp_layout(OFFER, &active_video).is_err());

        let active_audio = ANSWER.replace("a=recvonly", "a=sendrecv");
        assert!(validate_full_sdp_layout(OFFER, &active_audio).is_err());
    }

    #[test]
    fn appends_access_token_without_replacing_existing_query() {
        let url = build_ws_url("wss://sfu.example/ws?transport=websocket", "opaque-token").unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("transport").map(String::as_str),
            Some("websocket")
        );
        assert_eq!(
            query.get("access_token").map(String::as_str),
            Some("opaque-token")
        );
    }

    #[test]
    fn token_close_codes_refresh_once_but_removal_codes_stop() {
        assert_eq!(
            classify_close(Some(CloseCode::Library(4004))),
            CloseVerdict::RefreshToken
        );
        assert_eq!(
            classify_close(Some(CloseCode::Library(4005))),
            CloseVerdict::RefreshToken
        );
        assert_eq!(
            classify_close(Some(CloseCode::Library(4006))),
            CloseVerdict::Fatal
        );
        assert_eq!(
            classify_close(Some(CloseCode::Library(4011))),
            CloseVerdict::Fatal
        );
    }

    #[test]
    fn server_errors_do_not_refresh_for_room_or_membership_failures() {
        assert_eq!(
            classify_server_error("invalid_token"),
            ServerErrorVerdict::RefreshToken
        );
        assert_eq!(
            classify_server_error("room_not_found"),
            ServerErrorVerdict::Fatal
        );
        assert_eq!(
            classify_server_error("not_member"),
            ServerErrorVerdict::Fatal
        );
        assert_eq!(
            classify_server_error("transport_error"),
            ServerErrorVerdict::Retry
        );
    }
}
