
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures::{SinkExt as _, StreamExt as _};
use libwebrtc::audio_track::RtcAudioTrack;
use libwebrtc::media_stream_track::{MediaStreamTrack, RtcTrackState};
use libwebrtc::peer_connection::{
    AnswerOptions, IceConnectionState, PeerConnection, PeerConnectionState,
};
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::rtp_parameters::{DegradationPreference, Priority};
use libwebrtc::rtp_transceiver::RtpTransceiverDirection;
use libwebrtc::session_description::{SdpType, SessionDescription};
use libwebrtc::stats::RtcStats;
use libwebrtc::video_track::RtcVideoTrack;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::{IceServerConfig, TokenRefresher};
use crate::video::track_frame_key;

use super::messages::{ClientMessage, IceServerSpec, ServerMessage, SnapshotMember};
use super::mid::{self, MID_AUDIO, MID_CAMERA, MID_SCREEN, RemoteKind};
use super::sdp;

const CAMERA_MAX_BITRATE: u64 = 1_000_000;
const CAMERA_MAX_FRAMERATE: f64 = 30.0;
const SCREEN_MAX_BITRATE: u64 = 3_500_000;
const SCREEN_MAX_FRAMERATE: f64 = crate::screen::CAPTURE_FPS as f64;
const SCREEN_SCALABILITY_MODE: &str = "L1T1";
const SCREEN_CODEC: &str = "vp9";
const SCREEN_MAX_ENCODE_WIDTH: f64 = 1920.0;
const CAMERA_BITRATE_LIMITS: sdp::BitrateLimits = sdp::BitrateLimits {
    min_kbps: 250,
    start_kbps: 500,
    max_kbps: 1_000,
};
const SCREEN_BITRATE_LIMITS: sdp::BitrateLimits = sdp::BitrateLimits {
    min_kbps: 1_000,
    start_kbps: 2_500,
    max_kbps: 3_500,
};
const MEDIA_STATS_INTERVAL: Duration = Duration::from_secs(5);
const TRANSPORT_DISCONNECTED_GRACE: Duration = Duration::from_secs(4);
const NET_PATH_POLL: Duration = Duration::from_secs(1);
const LINK_DOWN_TICKS: u32 = 2;
const HEALTHY_SESSION: Duration = Duration::from_secs(30);
const ROUTE_PROBE_TARGETS: [&str; 2] = ["203.0.113.9:9", "[2001:db8::9]:9"];
const MEDIA_CONNECT_DEADLINE: Duration = Duration::from_secs(15);
const DTLS_CONNECT_DEADLINE: Duration = Duration::from_secs(15);
const CONNECT_POLL: Duration = Duration::from_millis(250);
const RENEGOTIATION_SETTLE: Duration = Duration::from_millis(50);
const RECONNECT_DELAY_FAST: Duration = Duration::from_millis(400);
const RECONNECT_DELAY: Duration = Duration::from_secs(3);
const RECONNECT_FAST_ATTEMPTS: u32 = 2;
const MAX_RECONNECT_ATTEMPTS: u32 = 40;
const RECONNECTING_HEARTBEAT_TICKS: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfuRole {
    Speaker,
    Audience,
}

impl SfuRole {
    pub fn wire(self) -> &'static str {
        match self {
            SfuRole::Speaker => "speaker",
            SfuRole::Audience => "audience",
        }
    }

    pub fn is_audience(self) -> bool {
        matches!(self, SfuRole::Audience)
    }
}

#[derive(Debug, Clone)]
pub struct SfuConfig {
    pub ws_url: String,
    pub token: String,
    pub room: String,
    pub role: SfuRole,
    pub fallback_ice_servers: Vec<IceServerConfig>,
    pub refresh_token: Option<TokenRefresher>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfuPeer {
    pub peer_id: u32,
    pub user_id: String,
    pub muted: bool,
    pub is_audience: bool,
    pub audio: Option<u64>,
    pub camera: Option<u64>,
    pub screenshare: Option<u64>,
}

#[derive(Clone)]
pub struct ScreenTrack {
    pub track: RtcVideoTrack,
    pub width: u32,
    pub height: u32,
}

pub enum SfuEvent {
    Connected { room: String },
    Peers(Vec<SfuPeer>),
    RemoteAudio { key: u64, track: RtcAudioTrack },
    RemoteVideo { key: u64, track: RtcVideoTrack },
    RemoteGone { key: u64 },
    PttActive(bool),
    Reconnecting,
    Reconnected,
    Disconnected { reason: String },
    Removed { reason: String },
    Error(String),
}

enum EngineCommand {
    SetLocalAudio(Option<RtcAudioTrack>),
    SetLocalCamera(Option<RtcVideoTrack>),
    SetLocalScreen(Option<ScreenTrack>),
    SetMute(bool),
    SetCameraActive(bool),
    SetScreenActive(bool),
    SetScreenAudio(bool),
    PushToTalk(bool),
    Close,
}

pub struct SfuEngine {
    cmd_tx: flume::Sender<EngineCommand>,
}

impl SfuEngine {
    pub fn spawn(
        config: SfuConfig,
        factory: PeerConnectionFactory,
        evt_tx: flume::Sender<SfuEvent>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let handle = crate::runtime::runtime().spawn(async move {
            if let Err(e) = engine_main(config, factory, cmd_rx, &evt_tx).await {
                tracing::error!("sfu engine stopped: {e:#}");
                let _ = evt_tx.send(SfuEvent::Disconnected {
                    reason: e.to_string(),
                });
            }
        });
        (Self { cmd_tx }, handle)
    }

    pub fn set_local_audio(&self, track: Option<RtcAudioTrack>) {
        let _ = self.cmd_tx.send(EngineCommand::SetLocalAudio(track));
    }

    pub fn set_local_camera(&self, track: Option<RtcVideoTrack>) {
        let _ = self.cmd_tx.send(EngineCommand::SetLocalCamera(track));
    }

    pub fn set_local_screen(&self, screen: Option<ScreenTrack>) {
        let _ = self.cmd_tx.send(EngineCommand::SetLocalScreen(screen));
    }

    pub fn set_mute(&self, muted: bool) {
        let _ = self.cmd_tx.send(EngineCommand::SetMute(muted));
    }

    pub fn set_camera_active(&self, active: bool) {
        let _ = self.cmd_tx.send(EngineCommand::SetCameraActive(active));
    }

    pub fn set_screen_active(&self, active: bool) {
        let _ = self.cmd_tx.send(EngineCommand::SetScreenActive(active));
    }

    pub fn set_screen_audio(&self, active: bool) {
        let _ = self.cmd_tx.send(EngineCommand::SetScreenAudio(active));
    }

    pub fn push_to_talk(&self, active: bool) {
        let _ = self.cmd_tx.send(EngineCommand::PushToTalk(active));
    }

    pub fn close(&self) {
        let _ = self.cmd_tx.send(EngineCommand::Close);
    }
}

fn remote_frame_key(mid: &str) -> u64 {
    track_frame_key("__sfu_remote__", mid)
}

#[derive(Default)]
struct Membership {
    self_peer_id: u32,
    by_peer: HashMap<u32, SnapshotMember>,
    peer_by_mid: HashMap<String, u32>,
    user_by_mid: HashMap<String, String>,
    live_tracks: HashMap<u64, String>,
}

impl Membership {
    fn apply(&mut self, mut member: SnapshotMember) {
        let [audio, camera, screen] = self.mids_of(member.peer_id);
        if member.mid_audio == 0 {
            member.mid_audio = audio;
        }
        if member.mid_video == 0 {
            member.mid_video = camera;
        }
        if member.mid_screen == 0 {
            member.mid_screen = screen;
        }
        let peer_id = member.peer_id;
        let user_id = member.user_id.clone();
        let mids = member_mids(&member);
        self.by_peer.insert(peer_id, member);
        for (mid, _) in mids {
            self.claim(peer_id, &mid);
            if !user_id.is_empty() {
                self.user_by_mid.insert(mid, user_id.clone());
            }
        }
    }

    fn mids_of(&self, peer_id: u32) -> [u32; 3] {
        let mut mids = [0u32; 3];
        for (mid, owner) in &self.peer_by_mid {
            if *owner != peer_id {
                continue;
            }
            let (Some(remote), Ok(n)) = (mid::classify(mid), mid.parse::<u32>()) else {
                continue;
            };
            mids[kind_index(remote.kind)] = n;
        }
        mids
    }

    fn claim(&mut self, peer_id: u32, mid: &str) {
        let (Some(remote), Ok(n)) = (mid::classify(mid), mid.parse::<u32>()) else {
            return;
        };
        self.peer_by_mid.retain(|other, owner| {
            *owner != peer_id
                || other == mid
                || !matches!(mid::classify(other), Some(r) if r.kind == remote.kind)
        });
        self.peer_by_mid.insert(mid.to_owned(), peer_id);
        for (id, member) in self.by_peer.iter_mut() {
            let slot = match remote.kind {
                RemoteKind::Audio => &mut member.mid_audio,
                RemoteKind::Camera => &mut member.mid_video,
                RemoteKind::Screen => &mut member.mid_screen,
            };
            if *id == peer_id {
                *slot = n;
            } else if *slot == n {
                *slot = 0;
            }
        }
    }

    fn remove_peer(&mut self, peer_id: u32, mids: [u32; 3]) -> Vec<String> {
        self.by_peer.remove(&peer_id);
        let mut released = Vec::new();
        for mid in mids.iter().filter(|m| **m != 0).map(u32::to_string) {
            if self.peer_by_mid.get(&mid).is_some_and(|owner| *owner != peer_id) {
                continue;
            }
            self.peer_by_mid.remove(&mid);
            self.user_by_mid.remove(&mid);
            released.push(mid);
        }
        self.peer_by_mid.retain(|_, id| *id != peer_id);
        released
    }

    fn absorb_msids(&mut self, sdp: &str) {
        for (mid, occupant) in mid::parse_msid_occupants(sdp) {
            if occupant.peer_id != 0 {
                self.claim(occupant.peer_id, &mid);
            }
            self.user_by_mid.insert(mid, occupant.user_id);
        }
    }

    fn peers(&self) -> Vec<SfuPeer> {
        let mut peers: Vec<SfuPeer> = self
            .by_peer
            .values()
            .filter(|m| m.peer_id != self.self_peer_id && !m.user_id.is_empty())
            .map(|m| {
                let camera_mid = m.mid_video.to_string();
                let screen_mid = m.mid_screen.to_string();
                SfuPeer {
                    peer_id: m.peer_id,
                    user_id: m.user_id.clone(),
                    muted: m.is_mute,
                    is_audience: m.is_audience(),
                    audio: (m.mid_audio != 0)
                        .then(|| remote_frame_key(&m.mid_audio.to_string())),
                    camera: (m.camera_active && m.mid_video != 0)
                        .then(|| remote_frame_key(&camera_mid)),
                    screenshare: (m.screen_active && m.mid_screen != 0)
                        .then(|| remote_frame_key(&screen_mid)),
                }
            })
            .collect();
        peers.sort_by(|a, b| a.user_id.cmp(&b.user_id).then(a.peer_id.cmp(&b.peer_id)));
        peers
    }
}

fn member_mids(member: &SnapshotMember) -> Vec<(String, RemoteKind)> {
    [
        (member.mid_audio, RemoteKind::Audio),
        (member.mid_video, RemoteKind::Camera),
        (member.mid_screen, RemoteKind::Screen),
    ]
    .into_iter()
    .filter(|(mid, _)| *mid != 0)
    .map(|(mid, kind)| (mid.to_string(), kind))
    .collect()
}

fn kind_index(kind: RemoteKind) -> usize {
    match kind {
        RemoteKind::Audio => 0,
        RemoteKind::Camera => 1,
        RemoteKind::Screen => 2,
    }
}

struct LocalTracks {
    audio: Option<RtcAudioTrack>,
    camera: Option<RtcVideoTrack>,
    screen: Option<ScreenTrack>,
    screen_audio: bool,
    muted: bool,
    ptt_active: bool,
    ptt_requested: bool,
}

impl LocalTracks {
    fn new() -> Self {
        Self {
            audio: None,
            camera: None,
            screen: None,
            screen_audio: false,
            muted: true,
            ptt_active: false,
            ptt_requested: false,
        }
    }

    fn microphone_open(&self, role: SfuRole) -> bool {
        match role {
            SfuRole::Speaker => !self.muted,
            SfuRole::Audience => self.ptt_active,
        }
    }

    fn uplink_open(&self, role: SfuRole) -> bool {
        self.microphone_open(role) || self.screen_audio
    }

    fn resumes_push_to_talk(&self, role: SfuRole) -> bool {
        role == SfuRole::Audience && self.ptt_requested
    }

    fn apply_audio_gate(&self, pc: Option<&PeerConnection>, role: SfuRole) {
        let flowing = self.uplink_open(role);
        if flowing {
            set_audio_encoding_active(pc, true);
        }
        if let Some(track) = &self.audio {
            track.set_enabled(flowing);
        }
        if !flowing {
            set_audio_encoding_active(pc, false);
        }
    }
}

fn set_audio_encoding_active(pc: Option<&PeerConnection>, active: bool) {
    let Some(pc) = pc else {
        return;
    };
    for transceiver in pc.transceivers() {
        if transceiver.mid().as_deref() != Some(MID_AUDIO) {
            continue;
        }
        let sender = transceiver.sender();
        let mut parameters = sender.parameters();
        if parameters.encodings.is_empty()
            || parameters.encodings.iter().all(|e| e.active == active)
        {
            return;
        }
        for encoding in parameters.encodings.iter_mut() {
            encoding.active = active;
        }
        match sender.set_parameters(parameters) {
            Ok(()) => tracing::info!(active, "audio uplink encoding gated"),
            Err(e) => tracing::warn!("audio uplink encoding active={active} rejected: {e}"),
        }
        return;
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct LocalRoutes(Vec<IpAddr>);

impl LocalRoutes {
    fn probe() -> Self {
        let mut addresses: Vec<IpAddr> = ROUTE_PROBE_TARGETS
            .into_iter()
            .filter_map(route_source_address)
            .map(stable_prefix)
            .collect();
        addresses.sort();
        addresses.dedup();
        Self(addresses)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn describe(&self) -> String {
        if self.0.is_empty() {
            return "-".to_owned();
        }
        self.0
            .iter()
            .map(IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn stable_prefix(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            octets[8..].fill(0);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
    }
}

fn route_source_address(target: &str) -> Option<IpAddr> {
    let target: SocketAddr = target.parse().ok()?;
    let bind = if target.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_unspecified()).then_some(address)
}

async fn engine_main(
    mut config: SfuConfig,
    factory: PeerConnectionFactory,
    cmd_rx: flume::Receiver<EngineCommand>,
    evt_tx: &flume::Sender<SfuEvent>,
) -> Result<()> {
    let mut local = LocalTracks::new();
    let mut attempts: u32 = 0;
    let mut ever_joined = false;
    let mut retiring = RetiredPeerConnection(None);

    loop {
        let (joined, reason, stale_token) =
            match run_session(
                &config,
                &factory,
                &mut local,
                &cmd_rx,
                evt_tx,
                ever_joined,
                &mut attempts,
                &mut retiring.0,
            )
            .await
            {
                SessionOutcome::Closed => {
                    let _ = evt_tx.send(SfuEvent::Disconnected {
                        reason: "left".into(),
                    });
                    return Ok(());
                }
                SessionOutcome::Fatal(reason) => {
                    let _ = evt_tx.send(SfuEvent::Disconnected { reason });
                    return Ok(());
                }
                SessionOutcome::Removed(reason) => {
                    let _ = evt_tx.send(SfuEvent::Removed {
                        reason: reason.clone(),
                    });
                    let _ = evt_tx.send(SfuEvent::Disconnected { reason });
                    return Ok(());
                }
                SessionOutcome::Dropped { joined, reason } => (joined, reason, false),
                SessionOutcome::DroppedStaleToken { joined, reason } => (joined, reason, true),
            };

        if local.ptt_active {
            local.ptt_active = false;
            local.apply_audio_gate(None, config.role);
            let _ = evt_tx.send(SfuEvent::PttActive(false));
        }

        ever_joined |= joined;
        let refreshed = stale_token && refresh_session_token(&mut config).await;
        if !ever_joined && !refreshed {
            let _ = evt_tx.send(SfuEvent::Disconnected { reason });
            return Ok(());
        }
        let _ = evt_tx.send(SfuEvent::Reconnecting);
        if LocalRoutes::probe().is_empty() {
            tracing::warn!("sfu link dropped ({reason}); parking until the machine has a route again");
            match wait_for_local_route(&cmd_rx, &mut local, config.role, evt_tx).await {
                OfflineWait::Closed => {
                    let _ = evt_tx.send(SfuEvent::Disconnected {
                        reason: "left".into(),
                    });
                    return Ok(());
                }
                OfflineWait::Online => continue,
            }
        }
        attempts += 1;
        if attempts > MAX_RECONNECT_ATTEMPTS {
            let _ = evt_tx.send(SfuEvent::Disconnected {
                reason: "reconnect attempts exhausted".into(),
            });
            return Ok(());
        }
        tracing::warn!("sfu link dropped ({reason}); retry {attempts}");
        let backoff = if attempts <= RECONNECT_FAST_ATTEMPTS { RECONNECT_DELAY_FAST } else { RECONNECT_DELAY };
        tokio::time::sleep(backoff).await;
    }
}

enum OfflineWait {
    Online,
    Closed,
}

async fn wait_for_local_route(
    cmd_rx: &flume::Receiver<EngineCommand>,
    local: &mut LocalTracks,
    role: SfuRole,
    evt_tx: &flume::Sender<SfuEvent>,
) -> OfflineWait {
    let mut timer = tokio::time::interval(NET_PATH_POLL);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut quiet_ticks: u32 = 0;
    loop {
        tokio::select! {
            command = cmd_rx.recv_async() => {
                match command {
                    Ok(EngineCommand::Close) | Err(_) => return OfflineWait::Closed,
                    Ok(other) => apply_offline_command(other, local, role, evt_tx),
                }
            }
            _ = timer.tick() => {
                if !LocalRoutes::probe().is_empty() {
                    tracing::info!("machine has a route again; resuming the sfu session");
                    return OfflineWait::Online;
                }
                quiet_ticks = quiet_ticks.saturating_add(1);
                if quiet_ticks >= RECONNECTING_HEARTBEAT_TICKS {
                    quiet_ticks = 0;
                    let _ = evt_tx.send(SfuEvent::Reconnecting);
                }
            }
        }
    }
}

fn apply_offline_command(
    command: EngineCommand,
    local: &mut LocalTracks,
    role: SfuRole,
    evt_tx: &flume::Sender<SfuEvent>,
) {
    match command {
        EngineCommand::SetLocalAudio(track) => local.audio = track,
        EngineCommand::SetLocalCamera(track) => local.camera = track,
        EngineCommand::SetLocalScreen(screen) => local.screen = screen,
        EngineCommand::SetMute(muted) => local.muted = muted,
        EngineCommand::SetScreenAudio(active) => local.screen_audio = active,
        EngineCommand::PushToTalk(active) => {
            local.ptt_requested = active;
            if !active {
                local.ptt_active = false;
                let _ = evt_tx.send(SfuEvent::PttActive(false));
            }
        }
        EngineCommand::SetCameraActive(_)
        | EngineCommand::SetScreenActive(_)
        | EngineCommand::Close => {}
    }
    local.apply_audio_gate(None, role);
}

async fn refresh_session_token(config: &mut SfuConfig) -> bool {
    let Some(refresher) = config.refresh_token.clone() else {
        return false;
    };
    match refresher.mint().await {
        Some(fresh) if fresh != config.token => {
            tracing::info!("sfu join token refreshed after the server rejected it");
            config.token = fresh;
            true
        }
        _ => false,
    }
}

enum SessionOutcome {
    Closed,
    Fatal(String),
    Removed(String),
    Dropped { joined: bool, reason: String },
    DroppedStaleToken { joined: bool, reason: String },
}

struct RetiredPeerConnection(Option<PeerConnection>);

impl Drop for RetiredPeerConnection {
    fn drop(&mut self) {
        if let Some(pc) = self.0.take() {
            pc.close();
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    config: &SfuConfig,
    factory: &PeerConnectionFactory,
    local: &mut LocalTracks,
    cmd_rx: &flume::Receiver<EngineCommand>,
    evt_tx: &flume::Sender<SfuEvent>,
    resuming: bool,
    attempts: &mut u32,
    retiring: &mut Option<PeerConnection>,
) -> SessionOutcome {
    let mut pc: Option<PeerConnection> = None;
    let outcome = session_loop(
        config, factory, local, cmd_rx, evt_tx, resuming, attempts, &mut pc, retiring,
    )
    .await;
    let retryable = matches!(
        outcome,
        SessionOutcome::Dropped { .. } | SessionOutcome::DroppedStaleToken { .. }
    );
    match pc {
        Some(pc) if retryable => {
            if let Some(previous) = retiring.take() {
                previous.close();
            }
            *retiring = Some(pc);
        }
        Some(pc) => {
            if let Some(previous) = retiring.take() {
                previous.close();
            }
            pc.close();
        }
        None if retryable => {}
        None => {
            if let Some(previous) = retiring.take() {
                previous.close();
            }
        }
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn session_loop(
    config: &SfuConfig,
    factory: &PeerConnectionFactory,
    local: &mut LocalTracks,
    cmd_rx: &flume::Receiver<EngineCommand>,
    evt_tx: &flume::Sender<SfuEvent>,
    resuming: bool,
    attempts: &mut u32,
    pc: &mut Option<PeerConnection>,
    retiring: &mut Option<PeerConnection>,
) -> SessionOutcome {
    let url = build_ws_url(&config.ws_url, &config.token);
    let ws = match connect_async(url.as_str()).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            return SessionOutcome::Dropped {
                joined: false,
                reason: format!("websocket connect failed: {e}"),
            };
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    tracing::info!(role = config.role.wire(), room = %config.room, "sfu websocket connected");

    let join = ClientMessage::Join {
        room: config.room.clone(),
        token: config.token.clone(),
        role: config.role.wire(),
        screen_codec: SCREEN_CODEC,
    };
    if let Err(e) = send(&mut ws_tx, &join).await {
        return SessionOutcome::Dropped {
            joined: false,
            reason: format!("join send failed: {e}"),
        };
    }

    let mut membership = Membership::default();
    let (transport_state_tx, transport_state_rx) = flume::unbounded::<PeerConnectionState>();
    let (ice_state_tx, ice_state_rx) = flume::unbounded::<IceConnectionState>();
    let mut stats_timer = tokio::time::interval(MEDIA_STATS_INTERVAL);
    stats_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pending_offer: Option<(u64, String)> = None;
    let mut media_wait_started: Option<Instant> = None;
    let mut ice_up_since: Option<Instant> = None;
    let mut transport_connected = false;
    let mut disconnected_since: Option<Instant> = None;
    let mut media_up_since: Option<Instant> = None;
    let mut budget_reset = false;
    let mut connect_timer = tokio::time::interval(CONNECT_POLL);
    connect_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut routes = LocalRoutes::probe();
    let mut route_candidate: Option<LocalRoutes> = None;
    let mut offline_ticks: u32 = 0;
    let mut link_down = false;
    let mut net_path_timer = tokio::time::interval(NET_PATH_POLL);
    net_path_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut announced_state = false;
    let mut announced_connection = false;
    let mut joined_room = config.room.clone();
    let mut joined = false;

    loop {
        if joined && transport_connected && !announced_connection {
            announced_connection = true;
            if resuming {
                let _ = evt_tx.send(SfuEvent::Reconnected);
            } else {
                let _ = evt_tx.send(SfuEvent::Connected {
                    room: joined_room.clone(),
                });
            }
        }
        tokio::select! {
            biased;
            incoming = ws_rx.next() => {
                let Some(frame) = incoming else {
                    return SessionOutcome::Dropped { joined, reason: "websocket closed".into() };
                };
                let text = match frame {
                    Ok(Message::Text(text)) => text,
                    Ok(Message::Ping(payload)) => {
                        let _ = ws_tx.send(Message::Pong(payload)).await;
                        continue;
                    }
                    Ok(Message::Close(frame)) => {
                        let code = frame.as_ref().map(|f| f.code);
                        let reason = frame
                            .as_ref()
                            .map(|f| f.reason.to_string())
                            .filter(|r| !r.is_empty())
                            .unwrap_or_else(|| "server closed link".to_owned());
                        tracing::info!(?code, %reason, "sfu closed the link");
                        return match classify_close(code) {
                            CloseVerdict::Retry => SessionOutcome::Dropped { joined, reason },
                            CloseVerdict::RetryWithNewToken => {
                                SessionOutcome::DroppedStaleToken { joined, reason }
                            }
                            CloseVerdict::Kicked => SessionOutcome::Removed(reason),
                        };
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        return SessionOutcome::Dropped { joined, reason: format!("websocket error: {e}") };
                    }
                };
                let message: ServerMessage = match serde_json::from_str(&text) {
                    Ok(message) => message,
                    Err(e) => {
                        tracing::warn!("sfu sent an unparsable frame: {e}");
                        continue;
                    }
                };

                match message {
                    ServerMessage::Ping => {
                        if send(&mut ws_tx, &ClientMessage::Pong).await.is_err() {
                            return SessionOutcome::Dropped { joined, reason: "pong send failed".into() };
                        }
                    }
                    ServerMessage::Pong => {}
                    ServerMessage::Joined { room, ice_servers } => {
                        tracing::info!(%room, ice_servers = ice_servers.len(), "sfu accepted join");
                        joined_room = room;
                        match create_peer_connection(factory, &ice_servers, &config.fallback_ice_servers) {
                            Ok(created) => {
                                let state_tx = transport_state_tx.clone();
                                created.on_connection_state_change(Some(Box::new(move |state| {
                                    tracing::info!(?state, "sfu transport state");
                                    let _ = state_tx.send(state);
                                })));
                                let ice_tx = ice_state_tx.clone();
                                created.on_ice_connection_state_change(Some(Box::new(move |state| {
                                    tracing::info!(?state, "sfu ice state");
                                    let _ = ice_tx.send(state);
                                })));
                                created.on_ice_gathering_state_change(Some(Box::new(|state| {
                                    tracing::info!(?state, "sfu ice gathering");
                                })));
                                created.on_ice_candidate_error(Some(Box::new(|error| {
                                    tracing::warn!(
                                        url = %error.url,
                                        code = error.error_code,
                                        text = %error.error_text,
                                        "sfu ice candidate error"
                                    );
                                })));
                                *pc = Some(created);
                                media_wait_started = Some(Instant::now());
                                ice_up_since = None;
                                transport_connected = false;
                            }
                            Err(e) => {
                                return SessionOutcome::Fatal(format!("peer connection setup failed: {e:#}"));
                            }
                        }
                    }
                    ServerMessage::Offer { offer_generation, sdp } => {
                        tracing::debug!(generation = offer_generation, bytes = sdp.len(), "sfu offer");
                        membership.absorb_msids(&sdp);
                        pending_offer = Some((offer_generation, sdp));
                    }
                    ServerMessage::RoomSnapshot { self_peer_id, members } => {
                        tracing::info!(members = members.len(), "sfu room snapshot");
                        membership.self_peer_id = self_peer_id;
                        joined = true;
                        for member in members {
                            membership.apply(member);
                        }
                        if !announced_state {
                            announced_state = true;
                            if announce_initial_state(&mut ws_tx, config, local).await.is_err() {
                                return SessionOutcome::Dropped { joined, reason: "initial state send failed".into() };
                            }
                        }
                        let _ = evt_tx.send(SfuEvent::Peers(membership.peers()));
                    }
                    ServerMessage::PeerJoined { peer } | ServerMessage::PeerUpdated { peer } => {
                        if let Some(peer) = peer {
                            membership.apply(peer);
                            let _ = evt_tx.send(SfuEvent::Peers(membership.peers()));
                        }
                    }
                    ServerMessage::PeerLeft { peer_id, mid_audio, mid_video, mid_screen } => {
                        for mid in membership.remove_peer(peer_id, [mid_audio, mid_video, mid_screen]) {
                            let key = remote_frame_key(&mid);
                            if membership.live_tracks.remove(&key).is_some() {
                                let _ = evt_tx.send(SfuEvent::RemoteGone { key });
                            }
                        }
                        let _ = evt_tx.send(SfuEvent::Peers(membership.peers()));
                    }
                    ServerMessage::PushToTalkChanged { active } => {
                        tracing::info!(active, "sfu push-to-talk grant changed");
                        let active = active && local.ptt_requested;
                        local.ptt_active = active;
                        local.apply_audio_gate(pc.as_ref(), config.role);
                        let _ = evt_tx.send(SfuEvent::PttActive(active));
                    }
                    ServerMessage::MuteChanged { .. }
                    | ServerMessage::VisibilityChanged { .. }
                    | ServerMessage::RoleChanged { .. }
                    | ServerMessage::Unknown => {}
                    ServerMessage::Error { message } => {
                        if matches!(message.as_str(), "invalid_push_to_talk" | "push_to_talk_rejected") {
                            local.ptt_active = false;
                            local.apply_audio_gate(pc.as_ref(), config.role);
                            let _ = evt_tx.send(SfuEvent::PttActive(false));
                            continue;
                        }
                        tracing::warn!(%message, joined, "sfu reported an error");
                        let _ = evt_tx.send(SfuEvent::Error(message.clone()));
                        if matches!(message.as_str(), "invalid_token" | "missing_token") {
                            return SessionOutcome::DroppedStaleToken { joined, reason: message };
                        }
                        if joined || resuming {
                            return SessionOutcome::Dropped { joined, reason: message };
                        }
                        return SessionOutcome::Fatal(message);
                    }
                }
            }
            command = cmd_rx.recv_async() => {
                let Ok(command) = command else {
                    return SessionOutcome::Closed;
                };
                match command {
                    EngineCommand::Close => {
                        if let Some(pc) = &*pc {
                            pc.close();
                        }
                        let _ = ws_tx.send(Message::Close(None)).await;
                        return SessionOutcome::Closed;
                    }
                    EngineCommand::SetLocalAudio(track) => {
                        local.audio = track;
                        local.apply_audio_gate(pc.as_ref(), config.role);
                        attach_local_tracks(pc.as_ref(), local, config.role);
                    }
                    EngineCommand::SetLocalCamera(track) => {
                        local.camera = track;
                        attach_local_tracks(pc.as_ref(), local, config.role);
                        if let Some(peer_connection) = &*pc {
                            tune_uplinks(peer_connection, local);
                        }
                    }
                    EngineCommand::SetLocalScreen(track) => {
                        local.screen = track;
                        attach_local_tracks(pc.as_ref(), local, config.role);
                        if let Some(peer_connection) = &*pc {
                            tune_uplinks(peer_connection, local);
                        }
                    }
                    EngineCommand::SetMute(muted) => {
                        local.muted = muted;
                        local.apply_audio_gate(pc.as_ref(), config.role);
                        if joined && send(&mut ws_tx, &ClientMessage::Mute { is_mute: muted }).await.is_err() {
                            return SessionOutcome::Dropped { joined, reason: "mute send failed".into() };
                        }
                    }
                    EngineCommand::SetCameraActive(active) => {
                        if joined && send(&mut ws_tx, &ClientMessage::Camera { active }).await.is_err() {
                            return SessionOutcome::Dropped { joined, reason: "camera send failed".into() };
                        }
                    }
                    EngineCommand::SetScreenActive(active) => {
                        if joined && send(&mut ws_tx, &ClientMessage::ShareScreen { active }).await.is_err() {
                            return SessionOutcome::Dropped { joined, reason: "share_screen send failed".into() };
                        }
                    }
                    EngineCommand::SetScreenAudio(active) => {
                        local.screen_audio = active;
                        local.apply_audio_gate(pc.as_ref(), config.role);
                    }
                    EngineCommand::PushToTalk(active) => {
                        local.ptt_requested = active;
                        if !active {
                            local.ptt_active = false;
                            local.apply_audio_gate(pc.as_ref(), config.role);
                            let _ = evt_tx.send(SfuEvent::PttActive(false));
                        }
                        if !joined {
                            continue;
                        }
                        let ordered: [ClientMessage; 2] = if active {
                            [ClientMessage::Mute { is_mute: false }, ClientMessage::PushToTalk { active: true }]
                        } else {
                            [ClientMessage::PushToTalk { active: false }, ClientMessage::Mute { is_mute: true }]
                        };
                        for message in &ordered {
                            if send(&mut ws_tx, message).await.is_err() {
                                return SessionOutcome::Dropped { joined, reason: "push_to_talk send failed".into() };
                            }
                        }
                    }
                }
            }
            state = transport_state_rx.recv_async() => {
                match state {
                    Ok(PeerConnectionState::Failed) => {
                        return SessionOutcome::Dropped {
                            joined,
                            reason: "peer connection failed".into(),
                        };
                    }
                    Ok(PeerConnectionState::Connected) => {
                        transport_connected = true;
                        media_wait_started = None;
                        disconnected_since = None;
                        media_up_since.get_or_insert_with(Instant::now);
                    }
                    Ok(PeerConnectionState::Disconnected) => {
                        transport_connected = false;
                        media_wait_started.get_or_insert_with(Instant::now);
                        disconnected_since.get_or_insert_with(Instant::now);
                    }
                    Ok(_) => {
                        if transport_connected {
                            transport_connected = false;
                            media_wait_started = Some(Instant::now());
                        }
                    }
                    Err(_) => {}
                }
            }
            ice = ice_state_rx.recv_async() => {
                if let Ok(state) = ice {
                    let up = matches!(
                        state,
                        IceConnectionState::Connected | IceConnectionState::Completed
                    );
                    let changed = up != ice_up_since.is_some();
                    if up {
                        ice_up_since.get_or_insert_with(Instant::now);
                    } else {
                        ice_up_since = None;
                    }
                    if changed && !transport_connected {
                        media_wait_started = Some(Instant::now());
                    }
                }
            }
            _ = connect_timer.tick() => {
                if let Some(since) = disconnected_since {
                    let waited = since.elapsed();
                    if waited >= TRANSPORT_DISCONNECTED_GRACE {
                        tracing::warn!(
                            waited_ms = waited.as_millis(),
                            "sfu transport stayed disconnected; restarting the session"
                        );
                        return SessionOutcome::Dropped {
                            joined,
                            reason: "peer connection stayed disconnected".into(),
                        };
                    }
                }
                if !budget_reset
                    && transport_connected
                    && media_up_since.is_some_and(|since| since.elapsed() >= HEALTHY_SESSION)
                {
                    budget_reset = true;
                    *attempts = 0;
                }
                if pc.is_some() && !transport_connected {
                    let waited = media_wait_started
                        .get_or_insert_with(Instant::now)
                        .elapsed();
                    let ice_up = ice_up_since.is_some();
                    let limit = if ice_up { DTLS_CONNECT_DEADLINE } else { MEDIA_CONNECT_DEADLINE };
                    if waited >= limit {
                        let reason = if ice_up {
                            "dtls handshake never completed"
                        } else {
                            "media transport never connected"
                        };
                        tracing::warn!(
                            ice_up,
                            waited_ms = waited.as_millis(),
                            "sfu media transport stalled; dropping the session to reconnect"
                        );
                        if let Some(pc) = pc.as_ref() {
                            log_media_stats(pc).await;
                        }
                        return SessionOutcome::Dropped { joined, reason: reason.into() };
                    }
                }
            }
            _ = stats_timer.tick() => {
                if let Some(peer_connection) = pc.clone() {
                    match peer_connection.connection_state() {
                        PeerConnectionState::Failed => {
                            return SessionOutcome::Dropped {
                                joined,
                                reason: "peer connection failed".into(),
                            };
                        }
                        PeerConnectionState::Disconnected => {
                            disconnected_since.get_or_insert_with(Instant::now);
                        }
                        PeerConnectionState::Connected => disconnected_since = None,
                        _ => {}
                    }
                    log_media_stats(&peer_connection).await;
                }
            }
            _ = net_path_timer.tick() => {
                let current = LocalRoutes::probe();
                if current.is_empty() {
                    route_candidate = None;
                    offline_ticks = offline_ticks.saturating_add(1);
                    link_down |= offline_ticks >= LINK_DOWN_TICKS;
                } else if !joined || routes.is_empty() {
                    routes = current;
                    route_candidate = None;
                    offline_ticks = 0;
                    link_down = false;
                } else if link_down {
                    tracing::warn!(
                        routes = %current.describe(),
                        offline_ticks,
                        "local network link came back; restarting the sfu session"
                    );
                    return SessionOutcome::Dropped {
                        joined,
                        reason: "local network link flapped".into(),
                    };
                } else if current == routes {
                    route_candidate = None;
                    offline_ticks = 0;
                } else if route_candidate.as_ref() == Some(&current) {
                    tracing::warn!(
                        was = %routes.describe(),
                        now = %current.describe(),
                        "local network path changed; restarting the sfu session"
                    );
                    return SessionOutcome::Dropped {
                        joined,
                        reason: "local network path changed".into(),
                    };
                } else {
                    route_candidate = Some(current);
                    offline_ticks = 0;
                }
            }
            () = std::future::ready(()), if pending_offer.is_some() => {
                let Some((generation, offer_sdp)) = pending_offer.take() else {
                    continue;
                };
                let Some(peer_connection) = pc.clone() else {
                    tracing::warn!("sfu offered before the joined response; dropping the offer");
                    continue;
                };
                match negotiate(
                    &peer_connection,
                    generation,
                    &offer_sdp,
                    local,
                    config.role,
                    &mut ws_tx,
                )
                .await
                {
                    Ok(()) => {
                        tracing::debug!(generation, "sfu answer sent");
                        sync_remote_media(&peer_connection, &mut membership, evt_tx);
                        if !membership.live_tracks.is_empty()
                            && let Some(previous) = retiring.take()
                        {
                            previous.close();
                        }
                        let _ = evt_tx.send(SfuEvent::Peers(membership.peers()));
                    }
                    Err(e) => {
                        return SessionOutcome::Dropped {
                            joined,
                            reason: format!("sfu negotiation failed: {e:#}"),
                        };
                    }
                }
                tokio::time::sleep(RENEGOTIATION_SETTLE).await;
            }
        }

    }
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn send(ws_tx: &mut WsSink, message: &ClientMessage) -> Result<()> {
    let text = serde_json::to_string(message).context("serialize signaling message")?;
    ws_tx
        .send(Message::Text(text.into()))
        .await
        .context("send signaling message")
}

async fn announce_initial_state(
    ws_tx: &mut WsSink,
    config: &SfuConfig,
    local: &LocalTracks,
) -> Result<()> {
    let resume_push_to_talk = local.resumes_push_to_talk(config.role);
    send(
        ws_tx,
        &ClientMessage::Mute {
            is_mute: local.muted && !resume_push_to_talk,
        },
    )
    .await?;
    if resume_push_to_talk {
        tracing::info!("re-asserting the held push-to-talk turn on the new sfu session");
        send(ws_tx, &ClientMessage::PushToTalk { active: true }).await?;
    }
    if config.role == SfuRole::Speaker {
        send(
            ws_tx,
            &ClientMessage::Camera {
                active: local.camera.is_some(),
            },
        )
        .await?;
        if local.screen.is_some() {
            send(ws_tx, &ClientMessage::ShareScreen { active: true }).await?;
        }
    }
    send(ws_tx, &ClientMessage::Visibility { visible: true }).await
}

fn build_ws_url(base: &str, token: &str) -> String {
    let base = base.trim();
    if base.is_empty() {
        return String::new();
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    let encoded =
        percent_encoding::utf8_percent_encode(token, percent_encoding::NON_ALPHANUMERIC);
    format!("{base}{separator}access_token={encoded}")
}

fn create_peer_connection(
    factory: &PeerConnectionFactory,
    from_server: &[IceServerSpec],
    fallback: &[IceServerConfig],
) -> Result<PeerConnection> {
    let mut ice_servers: Vec<IceServer> = from_server
        .iter()
        .filter(|server| !server.urls.is_empty())
        .map(|server| IceServer {
            urls: server.urls.clone(),
            username: server.username.clone(),
            password: server.credential.clone(),
        })
        .collect();

    if ice_servers.is_empty() {
        ice_servers = fallback
            .iter()
            .filter(|server| !server.urls.is_empty())
            .map(|server| IceServer {
                urls: server.urls.clone(),
                username: server.username.clone(),
                password: server.credential.clone(),
            })
            .collect();
    }

    if ice_servers.is_empty() {
        ice_servers.push(IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: String::new(),
            password: String::new(),
        });
    }

    tracing::info!(
        urls = %ice_servers
            .iter()
            .flat_map(|server| server.urls.iter().cloned())
            .collect::<Vec<_>>()
            .join(","),
        "sfu ice servers"
    );

    factory
        .create_peer_connection(RtcConfiguration {
            ice_servers,
            continual_gathering_policy: ContinualGatheringPolicy::GatherContinually,
            ice_transport_type: IceTransportsType::All,
        })
        .context("create peer connection")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseVerdict {
    Retry,
    RetryWithNewToken,
    Kicked,
}

fn classify_close(code: Option<CloseCode>) -> CloseVerdict {
    let Some(code) = code else {
        return CloseVerdict::Retry;
    };
    match u16::from(code) {
        4004 | 4005 => CloseVerdict::RetryWithNewToken,
        4006 => CloseVerdict::Kicked,
        _ => CloseVerdict::Retry,
    }
}

async fn log_media_stats(pc: &PeerConnection) {
    let Ok(stats) = pc.get_stats().await else {
        return;
    };

    let mut codecs: HashMap<String, String> = HashMap::new();
    for stat in &stats {
        if let RtcStats::Codec(codec) = stat {
            let mime = codec
                .codec
                .mime_type
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            codecs.insert(
                codec.rtc.id.clone(),
                format!("{mime}/{}", codec.codec.payload_type),
            );
        }
    }
    let codec_of = |id: &str| codecs.get(id).cloned().unwrap_or_else(|| "?".to_owned());

    let mut lines: Vec<String> = Vec::new();
    for stat in &stats {
        match stat {
            RtcStats::OutboundRtp(out) => lines.push(format!(
                "out mid={} {} {} pkts={} {}x{} fps={:.0} enc={}",
                out.outbound.mid,
                out.stream.kind,
                codec_of(&out.stream.codec_id),
                out.sent.packets_sent,
                out.outbound.frame_width,
                out.outbound.frame_height,
                out.outbound.frames_per_second,
                out.outbound.frames_encoded,
            )),
            RtcStats::InboundRtp(inb) if inb.stream.kind == "audio" => lines.push(format!(
                "in mid={} audio {} pkts={} disc={} played={:.1}s conceal={} depth={:.0}ms lvl={:.3}",
                inb.inbound.mid,
                codec_of(&inb.stream.codec_id),
                inb.received.packets_received,
                inb.inbound.packets_discarded,
                inb.inbound.total_samples_duration,
                inb.inbound.concealed_samples,
                jitter_depth_ms(&inb.inbound),
                inb.inbound.audio_level,
            )),
            RtcStats::InboundRtp(inb) => lines.push(format!(
                "in mid={} {} {} pkts={} {}x{} dec={} key={} pli={} nack={} disc={}",
                inb.inbound.mid,
                inb.stream.kind,
                codec_of(&inb.stream.codec_id),
                inb.received.packets_received,
                inb.inbound.frame_width,
                inb.inbound.frame_height,
                inb.inbound.frames_decoded,
                inb.inbound.key_frames_decoded,
                inb.inbound.pli_count,
                inb.inbound.nack_count,
                inb.inbound.packets_discarded,
            )),
            _ => {}
        }
    }

    if !lines.is_empty() {
        lines.sort();
        tracing::info!(streams = %lines.join(" | "), "sfu media stats");
    }

    let mut candidates: HashMap<String, String> = HashMap::new();
    for stat in &stats {
        match stat {
            RtcStats::LocalCandidate(c) => {
                candidates.insert(c.rtc.id.clone(), describe_candidate(&c.local_candidate));
            }
            RtcStats::RemoteCandidate(c) => {
                candidates.insert(c.rtc.id.clone(), describe_candidate(&c.remote_candidate));
            }
            _ => {}
        }
    }

    let endpoint = |id: &str| candidates.get(id).cloned().unwrap_or_else(|| "?".to_owned());
    let mut pairs: Vec<String> = Vec::new();
    for stat in &stats {
        match stat {
            RtcStats::Transport(t) => tracing::info!(
                dtls = ?t.transport.dtls_state,
                ice = ?t.transport.ice_state,
                role = ?t.transport.ice_role,
                sent = t.transport.packets_sent,
                received = t.transport.packets_received,
                pair_changes = t.transport.selected_candidate_pair_changes,
                "sfu transport stats"
            ),
            RtcStats::CandidatePair(p) => pairs.push(format!(
                "{}->{} {:?}{} stun={}/{} rtp={}/{}",
                endpoint(&p.candidate_pair.local_candidate_id),
                endpoint(&p.candidate_pair.remote_candidate_id),
                p.candidate_pair.state,
                if p.candidate_pair.nominated { "*" } else { "" },
                p.candidate_pair.requests_sent,
                p.candidate_pair.responses_received,
                p.candidate_pair.packets_sent,
                p.candidate_pair.packets_received,
            )),
            _ => {}
        }
    }

    if !pairs.is_empty() {
        pairs.sort();
        tracing::info!(pairs = %pairs.join(" | "), "sfu ice pairs");
    }
}

fn jitter_depth_ms(inbound: &libwebrtc::stats::dictionaries::InboundRtpStreamStats) -> f64 {
    if inbound.jitter_buffer_emitted_count == 0 {
        return 0.0;
    }
    inbound.jitter_buffer_delay / inbound.jitter_buffer_emitted_count as f64 * 1000.0
}

fn describe_candidate(candidate: &libwebrtc::stats::dictionaries::IceCandidateStats) -> String {
    let kind = candidate
        .candidate_type
        .map_or_else(|| "?".to_owned(), |kind| format!("{kind:?}"));
    let address = if candidate.address.is_empty() {
        "-".to_owned()
    } else {
        format!("{}:{}", candidate.address, candidate.port)
    };
    format!("{kind}/{} {address}", candidate.protocol)
}

fn transceiver_summary(pc: &PeerConnection) -> String {
    pc.transceivers()
        .iter()
        .enumerate()
        .map(|(index, transceiver)| {
            format!(
                "#{index}:{}={:?}",
                transceiver.mid().unwrap_or_else(|| "-".to_owned()),
                transceiver.direction(),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn negotiate(
    pc: &PeerConnection,
    generation: u64,
    offer_sdp: &str,
    local: &LocalTracks,
    role: SfuRole,
    ws_tx: &mut WsSink,
) -> Result<()> {
    let previous = pc.current_remote_description().map(|d| d.to_string());
    let stabilized = sdp::stabilize_inactive_video_sections(offer_sdp, previous.as_deref());

    tracing::debug!(
        generation,
        directions = %sdp::direction_summary(&stabilized),
        codecs = %sdp::codec_summary(&stabilized),
        ufrag = %sdp::ice_ufrags(&stabilized),
        setup = %sdp::setup_roles(&stabilized),
        "offer m-lines"
    );

    let offer = SessionDescription::parse(&stabilized, SdpType::Offer)
        .map_err(|e| anyhow::anyhow!("parse offer: {e}"))?;
    pc.set_remote_description(offer)
        .await
        .context("set remote description")?;

    tracing::debug!(
        generation,
        transceivers = %transceiver_summary(pc),
        "transceivers after remote offer"
    );

    attach_local_tracks(Some(pc), local, role);
    open_uplink_directions(pc, &stabilized);

    let answer = pc
        .create_answer(AnswerOptions::default())
        .await
        .context("create answer")?;

    let derived = answer.to_string();
    let opened = sdp::munge_uplink_bitrates(
        &sdp::force_uplink_sendonly(&derived, &stabilized),
        CAMERA_BITRATE_LIMITS,
        SCREEN_BITRATE_LIMITS,
    );
    tracing::debug!(
        generation,
        derived = %sdp::direction_summary(&derived),
        opened = %sdp::direction_summary(&opened),
        "answer uplinks reopened"
    );
    let answer = SessionDescription::parse(&opened, SdpType::Answer)
        .map_err(|e| anyhow::anyhow!("parse patched answer: {e}"))?;

    pc.set_local_description(answer)
        .await
        .context("set local description")?;

    tune_uplinks(pc, local);
    local.apply_audio_gate(Some(pc), role);

    tracing::debug!(
        generation,
        transceivers = %transceiver_summary(pc),
        "transceivers after local answer"
    );

    let local_sdp = pc
        .current_local_description()
        .map(|d| d.to_string())
        .context("local description missing after set_local_description")?;

    tracing::debug!(
        generation,
        directions = %sdp::direction_summary(&local_sdp),
        codecs = %sdp::codec_summary(&local_sdp),
        ufrag = %sdp::ice_ufrags(&local_sdp),
        setup = %sdp::setup_roles(&local_sdp),
        "answer m-lines"
    );

    send(
        ws_tx,
        &ClientMessage::Answer {
            sdp: local_sdp,
            offer_generation: generation,
        },
    )
    .await
}

fn screen_scale_down(width: u32) -> f64 {
    if width == 0 {
        return 1.0;
    }
    (f64::from(width) / SCREEN_MAX_ENCODE_WIDTH).max(1.0)
}

fn tune_uplinks(pc: &PeerConnection, local: &LocalTracks) {
    for transceiver in pc.transceivers() {
        let Some(mid) = transceiver.mid() else {
            continue;
        };
        let (max_bitrate, max_framerate, scalability, degradation, scale_down, priority) =
            match mid.as_str() {
            MID_CAMERA => (
                CAMERA_MAX_BITRATE,
                CAMERA_MAX_FRAMERATE,
                None,
                DegradationPreference::MaintainFramerate,
                1.0,
                Priority::High,
            ),
            MID_SCREEN => (
                SCREEN_MAX_BITRATE,
                SCREEN_MAX_FRAMERATE,
                Some(SCREEN_SCALABILITY_MODE.to_owned()),
                DegradationPreference::MaintainResolution,
                local
                    .screen
                    .as_ref()
                    .map_or(1.0, |screen| screen_scale_down(screen.width)),
                Priority::High,
            ),
            _ => continue,
        };

        let sender = transceiver.sender();
        let mut parameters = sender.parameters();
        if parameters.encodings.is_empty() {
            if matches!(
                transceiver.direction(),
                RtpTransceiverDirection::SendOnly | RtpTransceiverDirection::SendRecv
            ) {
                tracing::warn!("mid {mid} has no encoding to carry the publish limits");
            }
            continue;
        }
        for encoding in parameters.encodings.iter_mut() {
            encoding.max_bitrate = Some(max_bitrate);
            encoding.max_framerate = Some(max_framerate);
            encoding.scalability_mode = scalability.clone();
            encoding.scale_resolution_down_by = (scale_down > 1.0).then_some(scale_down);
            encoding.priority = priority;
        }
        parameters.set_degradation_preference(degradation);

        match sender.set_parameters(parameters) {
            Ok(()) => tracing::info!(
                mid = %mid,
                bitrate = max_bitrate,
                fps = max_framerate,
                scale = scale_down,
                scalability = scalability.as_deref().unwrap_or("-"),
                "publish limits applied"
            ),
            Err(e) => {
                tracing::warn!("publish limits rejected on mid {mid}: {e}; retrying without scalability");
                let mut retry = sender.parameters();
                for encoding in retry.encodings.iter_mut() {
                    encoding.max_bitrate = Some(max_bitrate);
                    encoding.max_framerate = Some(max_framerate);
                    encoding.scalability_mode = None;
                    encoding.scale_resolution_down_by = (scale_down > 1.0).then_some(scale_down);
                    encoding.priority = priority;
                }
                retry.set_degradation_preference(degradation);
                if let Err(e) = sender.set_parameters(retry) {
                    tracing::error!("publish limits could not be applied on mid {mid}: {e}");
                }
            }
        }
    }
}

fn open_uplink_directions(pc: &PeerConnection, offer_sdp: &str) {
    let offered = sdp::directions_by_mid(offer_sdp);
    for transceiver in pc.transceivers() {
        let Some(mid) = transceiver.mid() else {
            continue;
        };
        if !mid::is_local_mid(&mid) {
            continue;
        }
        let invited = matches!(
            offered.get(&mid).map(String::as_str),
            Some("a=recvonly") | Some("a=sendrecv")
        );
        if !invited || transceiver.direction() == RtpTransceiverDirection::SendOnly {
            continue;
        }
        match transceiver.set_direction(RtpTransceiverDirection::SendOnly) {
            Ok(()) => tracing::debug!(mid = %mid, "uplink opened for sending"),
            Err(e) => tracing::warn!("uplink mid {mid} could not be opened for sending: {e}"),
        }
    }
}

fn attach_local_tracks(pc: Option<&PeerConnection>, local: &LocalTracks, role: SfuRole) {
    let Some(pc) = pc else { return };

    tracing::debug!(
        role = role.wire(),
        audio = local.audio.is_some(),
        camera = local.camera.is_some(),
        screen = local.screen.is_some(),
        "attaching local tracks to the sfu uplinks"
    );

    let publishes_video = role == SfuRole::Speaker;

    for transceiver in pc.transceivers() {
        let Some(mid) = transceiver.mid() else {
            continue;
        };
        let wanted = match mid.as_str() {
            MID_AUDIO => local.audio.clone().map(MediaStreamTrack::from),
            MID_CAMERA => local
                .camera
                .clone()
                .filter(|_| publishes_video)
                .map(MediaStreamTrack::from),
            MID_SCREEN => local
                .screen
                .clone()
                .filter(|_| publishes_video)
                .map(|screen| MediaStreamTrack::from(screen.track)),
            _ => continue,
        };

        let Some(track) = wanted else { continue };
        match transceiver.sender().set_track(Some(track)) {
            Ok(()) => tracing::debug!(
                mid = %mid,
                direction = ?transceiver.direction(),
                "local track attached"
            ),
            Err(e) => tracing::warn!("failed to attach local track on mid {mid}: {e}"),
        }
    }

    local.apply_audio_gate(Some(pc), role);
}

fn sync_remote_media(
    pc: &PeerConnection,
    membership: &mut Membership,
    evt_tx: &flume::Sender<SfuEvent>,
) {
    for transceiver in pc.transceivers() {
        let Some(mid) = transceiver.mid() else {
            continue;
        };
        if mid::is_local_mid(&mid) {
            continue;
        }
        let Some(remote) = mid::classify(&mid) else {
            continue;
        };

        let key = remote_frame_key(&mid);
        let negotiated = transceiver
            .current_direction()
            .unwrap_or_else(|| transceiver.direction());
        let idle = matches!(
            negotiated,
            RtpTransceiverDirection::Inactive | RtpTransceiverDirection::Stopped
        );
        let live = transceiver
            .receiver()
            .track()
            .filter(|track| !idle && track.state() == RtcTrackState::Live);

        let Some(track) = live else {
            if membership.live_tracks.remove(&key).is_some() {
                tracing::info!(mid = %mid, key, "remote media dropped");
                let _ = evt_tx.send(SfuEvent::RemoteGone { key });
            }
            continue;
        };

        let track_id = track.id();
        if membership.live_tracks.get(&key) == Some(&track_id) {
            continue;
        }

        match (track, remote.kind) {
            (MediaStreamTrack::Audio(track), RemoteKind::Audio) => {
                membership.live_tracks.insert(key, track_id);
                tracing::info!(mid = %mid, key, "remote audio attached");
                let _ = evt_tx.send(SfuEvent::RemoteAudio { key, track });
            }
            (MediaStreamTrack::Video(track), RemoteKind::Camera | RemoteKind::Screen) => {
                membership.live_tracks.insert(key, track_id);
                tracing::info!(mid = %mid, key, "remote video attached");
                let _ = evt_tx.send(SfuEvent::RemoteVideo { key, track });
            }
            (_, kind) => {
                tracing::warn!("mid {mid} carries a track of the wrong media type for {kind:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn reconnect_controls_wait_for_membership(held: bool) {
        use serde_json::json;
        use tokio::time::timeout;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = SfuConfig {
            ws_url: format!("ws://{}/ws", listener.local_addr().unwrap()),
            token: "test-token".into(),
            room: "test-room".into(),
            role: SfuRole::Audience,
            fallback_ice_servers: Vec::new(),
            refresh_token: None,
        };
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (evt_tx, evt_rx) = flume::unbounded();
        if !held {
            cmd_tx.send(EngineCommand::SetMute(true)).unwrap();
            cmd_tx.send(EngineCommand::SetCameraActive(false)).unwrap();
            cmd_tx.send(EngineCommand::SetScreenActive(false)).unwrap();
            cmd_tx.send(EngineCommand::PushToTalk(true)).unwrap();
            cmd_tx.send(EngineCommand::PushToTalk(false)).unwrap();
        }
        let session = tokio::spawn(async move {
            let mut local = LocalTracks::new();
            local.ptt_requested = true;
            let factory = PeerConnectionFactory::default();
            run_session(&config, &factory, &mut local, &cmd_rx, &evt_tx, true).await
        });

        let (socket, _) = listener.accept().await.unwrap();
        let mut server = tokio_tungstenite::accept_async(socket).await.unwrap();
        let join = server.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&join).unwrap()["type"],
            "join"
        );
        if !held {
            assert!(matches!(
                evt_rx.recv_async().await.unwrap(),
                SfuEvent::PttActive(false)
            ));
        }
        server
            .send(Message::Text(
                json!({
                    "type": "joined", "room": "test-room", "iceServers": []
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        server
            .send(Message::Text(json!({ "type": "ping" }).to_string().into()))
            .await
            .unwrap();
        let pong = server.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&pong).unwrap(),
            json!({ "type": "pong" }),
            "no room control may be sent before membership is confirmed"
        );
        assert!(
            evt_rx.is_empty(),
            "a joined acknowledgement must not report media reconnected"
        );

        server
            .send(Message::Text(
                json!({
                    "type": "room_snapshot", "members": []
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let mut expected = vec![json!({ "type": "mute", "is_mute": !held })];
        if held {
            expected.push(json!({ "type": "push_to_talk", "active": true }));
        }
        expected.push(json!({ "type": "visibility", "visible": true }));
        for wanted in expected {
            let frame = server.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&frame).unwrap(),
                wanted
            );
        }
        assert!(matches!(
            evt_rx.recv_async().await.unwrap(),
            SfuEvent::Peers(_)
        ));
        if !held {
            server
                .send(Message::Text(
                    json!({
                        "type": "push_to_talk_changed", "active": true
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            assert!(
                matches!(
                    evt_rx.recv_async().await.unwrap(),
                    SfuEvent::PttActive(false)
                ),
                "a delayed grant must not reopen the mic after the button was released"
            );
        }
        cmd_tx.send(EngineCommand::Close).unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(2), session)
                .await
                .unwrap()
                .unwrap(),
            SessionOutcome::Closed
        ));
        assert!(
            !evt_rx
                .try_iter()
                .any(|event| matches!(event, SfuEvent::Connected { .. } | SfuEvent::Reconnected)),
            "room membership without a media transport must not report success"
        );
    }

    #[tokio::test]
    async fn releasing_ptt_during_reconnect_does_not_send_controls_before_join() {
        tokio::time::timeout(
            Duration::from_secs(5),
            reconnect_controls_wait_for_membership(false),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn holding_ptt_across_reconnect_replays_it_after_the_snapshot() {
        tokio::time::timeout(
            Duration::from_secs(5),
            reconnect_controls_wait_for_membership(true),
        )
        .await
        .unwrap();
    }

    fn member(peer_id: u32, user_id: &str, mids: [u32; 3]) -> SnapshotMember {
        SnapshotMember {
            peer_id,
            user_id: user_id.to_owned(),
            role: "speaker".to_owned(),
            is_mute: false,
            camera_active: false,
            screen_active: false,
            mid_audio: mids[0],
            mid_video: mids[1],
            mid_screen: mids[2],
        }
    }

    #[test]
    fn the_access_token_is_appended_with_the_right_separator() {
        assert_eq!(
            build_ws_url("wss://sfu.example/ws", "a b"),
            "wss://sfu.example/ws?access_token=a%20b"
        );
        assert_eq!(
            build_ws_url("wss://sfu.example/ws?x=1", "t"),
            "wss://sfu.example/ws?x=1&access_token=t"
        );
    }

    #[test]
    fn an_empty_base_url_yields_an_empty_url() {
        assert!(build_ws_url("   ", "t").is_empty());
    }

    #[test]
    fn jwt_dots_survive_encoding_as_escapes() {
        let url = build_ws_url("wss://x/ws", "aa.bb.cc");
        assert!(url.ends_with("access_token=aa%2Ebb%2Ecc"));
    }

    #[test]
    fn membership_maps_every_non_zero_mid_to_its_peer() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        assert_eq!(membership.peer_by_mid.get("3"), Some(&3));
        assert_eq!(membership.peer_by_mid.get("5"), Some(&3));
        assert_eq!(membership.user_by_mid.get("4").map(String::as_str), Some("7"));
    }

    #[test]
    fn same_account_peers_have_a_deterministic_order() {
        let mut membership = Membership::default();
        for peer_id in [9, 3, 7] {
            membership.apply(member(peer_id, "7", [0, 0, 0]));
        }
        assert_eq!(
            membership.peers().iter().map(|p| p.peer_id).collect::<Vec<_>>(),
            vec![3, 7, 9]
        );
    }

    #[test]
    fn only_the_local_connection_is_excluded_for_a_shared_account() {
        let mut membership = Membership::default();
        membership.self_peer_id = 3;
        membership.apply(member(3, "7", [0, 0, 0]));
        let mut other_device = member(9, "7", [3, 4, 5]);
        other_device.screen_active = true;
        membership.apply(other_device);
        let peers = membership.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, 9);
        assert_eq!(peers[0].screenshare, Some(remote_frame_key("5")));
    }

    #[test]
    fn a_zero_mid_is_not_registered() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 0, 0]));
        assert_eq!(membership.peer_by_mid.len(), 1);
        assert!(!membership.peer_by_mid.contains_key("0"));
    }

    #[test]
    fn leaving_releases_every_mid_the_peer_owned() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        let released = membership.remove_peer(3, [3, 4, 5]);
        assert_eq!(released, vec!["3".to_owned(), "4".to_owned(), "5".to_owned()]);
        assert!(membership.peer_by_mid.is_empty());
        assert!(membership.peers().is_empty());
    }

    #[test]
    fn rejoining_on_the_same_mid_hands_ownership_to_the_new_peer() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        membership.remove_peer(3, [3, 4, 5]);
        membership.apply(member(9, "8", [3, 4, 5]));
        assert_eq!(membership.peer_by_mid.get("3"), Some(&9));
        assert_eq!(membership.peers()[0].audio, Some(remote_frame_key("3")));
    }

    #[test]
    fn peers_expose_video_keys_only_while_the_source_is_active() {
        let mut membership = Membership::default();
        let mut m = member(3, "7", [3, 4, 5]);
        m.camera_active = true;
        membership.apply(m);
        let peers = membership.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].camera, Some(remote_frame_key("4")));
        assert_eq!(peers[0].screenshare, None);
    }

    #[test]
    fn peers_without_a_user_id_are_withheld_until_one_arrives() {
        let mut membership = Membership::default();
        membership.apply(member(3, "", [3, 4, 5]));
        assert!(membership.peers().is_empty());
        membership.apply(member(3, "7", [3, 4, 5]));
        assert_eq!(membership.peers().len(), 1);
    }

    #[test]
    fn an_offer_msid_overrides_a_stale_membership_user_id() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:3\r\na=msid:room-u999-mic t\r\n");
        assert_eq!(membership.user_by_mid.get("3").map(String::as_str), Some("999"));
        assert_eq!(
            membership.peer_by_mid.get("3"),
            Some(&3),
            "without a peer segment the owner is left as membership said"
        );
    }

    #[test]
    fn msids_fill_in_mids_membership_has_not_named_yet() {
        let mut membership = Membership::default();
        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:6\r\na=msid:room-u42-mic t\r\n");
        assert_eq!(membership.user_by_mid.get("6").map(String::as_str), Some("42"));
    }

    #[test]
    fn an_offer_reoccupying_a_left_mid_clears_the_marker_and_names_the_peer() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        membership.remove_peer(3, [3, 4, 5]);
        let mut joiner = member(9, "8", [0, 0, 0]);
        joiner.role = "audience".to_owned();
        membership.apply(joiner);
        assert_eq!(membership.peers()[0].audio, None);

        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:3\r\na=msid:u8-p9 audio-u8-p9\r\n");

        assert_eq!(
            membership.peer_by_mid.get("3"),
            Some(&9),
            "the offer is the only signal that a push-to-talk slot was assigned"
        );
        let peers = membership.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].audio, Some(remote_frame_key("3")));
    }

    #[test]
    fn a_peer_update_after_the_offer_keeps_the_msid_learned_mid() {
        let mut membership = Membership::default();
        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:3\r\na=msid:u8-p9 audio-u8-p9\r\n");
        membership.apply(member(9, "8", [0, 0, 0]));
        assert_eq!(membership.peers()[0].audio, Some(remote_frame_key("3")));
    }

    #[test]
    fn a_late_leave_never_marks_a_mid_the_offer_gave_to_someone_else() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        membership.apply(member(9, "8", [0, 0, 0]));
        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:3\r\na=msid:u8-p9 audio-u8-p9\r\n");
        let released = membership.remove_peer(3, [3, 4, 5]);
        assert_eq!(released, vec!["4".to_owned(), "5".to_owned()]);
        assert_eq!(membership.peer_by_mid.get("3"), Some(&9));
        assert_eq!(membership.peers()[0].audio, Some(remote_frame_key("3")));
    }

    #[test]
    fn an_msid_without_a_peer_segment_still_names_the_user() {
        let mut membership = Membership::default();
        membership.apply(member(3, "7", [3, 4, 5]));
        membership.remove_peer(3, [3, 4, 5]);
        membership.absorb_msids("m=audio 9 RTP/SAVPF 111\r\na=mid:3\r\na=msid:room-u42-mic t\r\n");
        assert_eq!(membership.user_by_mid.get("3").map(String::as_str), Some("42"));
        assert!(membership.peer_by_mid.get("3").is_none());
    }

    #[test]
    fn remote_keys_differ_per_mid_and_are_stable() {
        assert_eq!(remote_frame_key("4"), remote_frame_key("4"));
        assert_ne!(remote_frame_key("4"), remote_frame_key("5"));
    }

    #[test]
    fn a_speaker_gates_audio_on_mute_and_an_audience_on_the_ptt_grant() {
        let mut local = LocalTracks::new();
        local.muted = false;
        assert!(local.microphone_open(SfuRole::Speaker));
        assert!(!local.microphone_open(SfuRole::Audience));

        local.ptt_active = true;
        assert!(local.microphone_open(SfuRole::Audience));

        local.muted = true;
        assert!(!local.microphone_open(SfuRole::Speaker));
        assert!(
            local.microphone_open(SfuRole::Audience),
            "the mute flag brackets a ptt request and must not close the grant"
        );
    }

    #[test]
    fn muting_closes_the_uplink_unless_screen_audio_is_shared() {
        let mut local = LocalTracks::new();
        local.muted = true;
        assert!(!local.uplink_open(SfuRole::Speaker));

        local.screen_audio = true;
        assert!(!local.microphone_open(SfuRole::Speaker));
        assert!(local.uplink_open(SfuRole::Speaker));

        local.screen_audio = false;
        local.muted = false;
        assert!(local.uplink_open(SfuRole::Speaker));
    }

    #[test]
    fn an_audience_uplink_follows_the_ptt_grant_not_the_mute_flag() {
        let mut local = LocalTracks::new();
        local.muted = false;
        assert!(!local.uplink_open(SfuRole::Audience));

        local.ptt_active = true;
        assert!(local.uplink_open(SfuRole::Audience));
    }

    #[test]
    fn a_held_turn_is_re_announced_on_the_next_session_but_the_grant_is_not_assumed() {
        let mut local = LocalTracks::new();
        assert!(!local.resumes_push_to_talk(SfuRole::Audience));

        local.ptt_requested = true;
        local.ptt_active = true;
        assert!(local.resumes_push_to_talk(SfuRole::Audience));
        assert!(!local.resumes_push_to_talk(SfuRole::Speaker));

        local.ptt_active = false;
        assert!(
            !local.uplink_open(SfuRole::Audience),
            "a dropped session voids the grant, so nothing may be sent until the sfu grants it again"
        );
        assert!(local.resumes_push_to_talk(SfuRole::Audience));

        local.ptt_requested = false;
        assert!(!local.resumes_push_to_talk(SfuRole::Audience));
    }

    #[test]
    fn a_peer_update_without_mids_keeps_the_video_keys_alive() {
        let mut membership = Membership::default();
        let mut joined = member(3, "7", [3, 4, 5]);
        joined.camera_active = true;
        joined.screen_active = true;
        membership.apply(joined);
        assert_eq!(membership.peers()[0].camera, Some(remote_frame_key("4")));

        let mut update = member(3, "7", [0, 0, 0]);
        update.camera_active = true;
        update.screen_active = true;
        membership.apply(update);

        let peers = membership.peers();
        assert_eq!(
            peers[0].camera,
            Some(remote_frame_key("4")),
            "a peer_updated must not orphan the remote camera tile"
        );
        assert_eq!(peers[0].screenshare, Some(remote_frame_key("5")));
        assert_eq!(membership.peer_by_mid.get("4"), Some(&3));
    }

    #[test]
    fn a_retina_capture_is_scaled_down_before_encoding() {
        assert_eq!(screen_scale_down(3024), 3024.0 / 1920.0);
        assert!(screen_scale_down(3840) > 1.9);
    }

    #[test]
    fn a_display_already_narrow_enough_is_left_alone() {
        assert_eq!(screen_scale_down(1920), 1.0);
        assert_eq!(screen_scale_down(1280), 1.0);
        assert_eq!(screen_scale_down(0), 1.0, "an unknown size must not divide by zero");
    }

    #[test]
    fn a_transport_drop_without_a_close_frame_is_retried() {
        assert_eq!(classify_close(None), CloseVerdict::Retry);
    }

    #[test]
    fn an_idle_timeout_is_retried() {
        assert_eq!(
            classify_close(Some(CloseCode::Library(4001))),
            CloseVerdict::Retry
        );
    }

    #[test]
    fn token_rejections_are_retried_with_a_fresh_token() {
        for code in [4004, 4005] {
            assert_eq!(
                classify_close(Some(CloseCode::Library(code))),
                CloseVerdict::RetryWithNewToken,
                "{code} is the server complaining about the token itself"
            );
        }
    }

    #[test]
    fn a_server_without_a_jwt_secret_is_retried_without_minting() {
        assert_eq!(
            classify_close(Some(CloseCode::Library(4003))),
            CloseVerdict::Retry
        );
    }

    #[test]
    fn a_kick_is_not_retried() {
        assert_eq!(
            classify_close(Some(CloseCode::Library(4006))),
            CloseVerdict::Kicked
        );
    }

    #[test]
    fn a_normal_closure_is_still_retried() {
        assert_eq!(classify_close(Some(CloseCode::Normal)), CloseVerdict::Retry);
    }
}
