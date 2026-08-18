use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceCandidateInit;
use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};

use mezon_codec::{EncodedFrame, VpxCodec};

use crate::engine::{PeerConnectionOpts, build_peer_connection};
use crate::error::RtcError;
use crate::publish::{LocalAudio, LocalVideo};
use crate::subscribe::{spawn_opus_receiver, spawn_video_receiver};
use crate::twcc::{SendBitrateController, spawn_twcc_monitor};

const DEFAULT_MIN_KBPS: u32 = 100;
const DEFAULT_MAX_KBPS: u32 = 2500;
const DEFAULT_START_KBPS: u32 = 800;

const GATHER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcEvent {
    Connected,
    Disconnected,
    TrackAdded {
        mid: String,
        kind: RtpCodecKind,
    },
    TrackRemoved {
        mid: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteVideoKind {
    Camera,
    Screen,
}

pub struct RemoteAudio {
    pub user_id: String,
    pub opus: Vec<u8>,
}

pub struct RemoteVideo {
    pub user_id: String,
    pub kind: RemoteVideoKind,
    pub codec: VpxCodec,
    pub frame: EncodedFrame,
}

struct SessionHandler {
    events_tx: flume::Sender<RtcEvent>,
    gather_tx: flume::Sender<()>,
    audio_tx: flume::Sender<RemoteAudio>,
    video_tx: flume::Sender<RemoteVideo>,
    pc: Arc<OnceLock<Weak<dyn PeerConnection>>>,
}

fn parse_user_id(stream_id: &str) -> Option<String> {
    stream_id.split('-').find_map(|part| {
        let digits = part.strip_prefix('u')?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.to_string())
    })
}

fn remote_media_slot(mid: &str) -> Option<u8> {
    let m: u32 = mid.parse().ok()?;
    (m >= REMOTE_MID_BASE).then(|| ((m - REMOTE_MID_BASE) % 3) as u8)
}

const REMOTE_MID_BASE: u32 = 3;

async fn resolve_mid(
    pc_slot: &Arc<OnceLock<Weak<dyn PeerConnection>>>,
    track: &Arc<dyn TrackRemote>,
) -> Option<String> {
    let pc = pc_slot.get()?.upgrade()?;
    let target = track.track_id().await;
    for transceiver in pc.get_transceivers().await {
        if let Ok(Some(receiver)) = transceiver.receiver().await
            && receiver.track().track_id().await == target
            && let Ok(Some(mid)) = transceiver.mid().await
        {
            return Some(mid);
        }
    }
    None
}

async fn infer_vpx_codec(track: &Arc<dyn TrackRemote>) -> Option<VpxCodec> {
    let ssrc = track.ssrcs().await.first().copied()?;
    let mime = track.codec(ssrc).await?.mime_type.to_ascii_uppercase();
    if mime.contains("VP8") {
        Some(VpxCodec::Vp8)
    } else if mime.contains("VP9") {
        Some(VpxCodec::Vp9)
    } else {
        None
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SessionHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        tracing::debug!(?state, "mezon-rtc: ice gathering state");
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        tracing::info!(?state, "mezon-rtc: peer connection state");
        match state {
            RTCPeerConnectionState::Connected => {
                let _ = self.events_tx.send(RtcEvent::Connected);
            }
            RTCPeerConnectionState::Disconnected
            | RTCPeerConnectionState::Failed
            | RTCPeerConnectionState::Closed => {
                let _ = self.events_tx.send(RtcEvent::Disconnected);
            }
            _ => {}
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let events_tx = self.events_tx.clone();
        let audio_tx = self.audio_tx.clone();
        let video_tx = self.video_tx.clone();
        let pc_slot = self.pc.clone();
        tokio::spawn(async move {
            let kind = track.kind().await;
            let mid = resolve_mid(&pc_slot, &track).await.unwrap_or_default();
            let user_id = parse_user_id(&track.stream_id().await).unwrap_or_default();
            let slot = remote_media_slot(&mid);
            let _ = events_tx.send(RtcEvent::TrackAdded {
                mid: mid.clone(),
                kind,
            });

            match kind {
                RtpCodecKind::Audio => {
                    let rx = spawn_opus_receiver(track);
                    while let Ok(opus) = rx.recv_async().await {
                        if audio_tx
                            .send(RemoteAudio {
                                user_id: user_id.clone(),
                                opus,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                RtpCodecKind::Video => {
                    let codec = infer_vpx_codec(&track).await.unwrap_or(VpxCodec::Vp9);
                    let video_kind = if slot == Some(2) {
                        RemoteVideoKind::Screen
                    } else {
                        RemoteVideoKind::Camera
                    };
                    let rx = spawn_video_receiver(track, codec);
                    while let Ok(frame) = rx.recv_async().await {
                        if video_tx
                            .send(RemoteVideo {
                                user_id: user_id.clone(),
                                kind: video_kind,
                                codec,
                                frame,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                RtpCodecKind::Unspecified => {}
            }

            let _ = events_tx.send(RtcEvent::TrackRemoved { mid });
        });
    }
}

pub struct RtcSession {
    pc: Arc<dyn PeerConnection>,
    events_rx: flume::Receiver<RtcEvent>,
    gather_rx: flume::Receiver<()>,
    audio_rx: flume::Receiver<RemoteAudio>,
    video_rx: flume::Receiver<RemoteVideo>,
    controller: Arc<Mutex<SendBitrateController>>,
    audio_pubs: Mutex<Vec<Arc<LocalAudio>>>,
    video_pubs: Mutex<Vec<Arc<LocalVideo>>>,
}

impl RtcSession {
    pub async fn new(opts: PeerConnectionOpts) -> Result<Self, RtcError> {
        let (events_tx, events_rx) = flume::unbounded();
        let (gather_tx, gather_rx) = flume::unbounded();
        let (audio_tx, audio_rx) = flume::unbounded();
        let (video_tx, video_rx) = flume::unbounded();

        let pc_slot: Arc<OnceLock<Weak<dyn PeerConnection>>> = Arc::new(OnceLock::new());
        let handler = Arc::new(SessionHandler {
            events_tx,
            gather_tx,
            audio_tx,
            video_tx,
            pc: pc_slot.clone(),
        });

        let pc = build_peer_connection(handler, opts).await?;
        let _ = pc_slot.set(Arc::downgrade(&pc));

        let controller = Arc::new(Mutex::new(SendBitrateController::new(
            DEFAULT_MIN_KBPS,
            DEFAULT_MAX_KBPS,
            DEFAULT_START_KBPS,
        )));

        Ok(Self {
            pc,
            events_rx,
            gather_rx,
            audio_rx,
            video_rx,
            controller,
            audio_pubs: Mutex::new(Vec::new()),
            video_pubs: Mutex::new(Vec::new()),
        })
    }

    pub fn events(&self) -> flume::Receiver<RtcEvent> {
        self.events_rx.clone()
    }

    pub fn subscribe_audio(&self) -> flume::Receiver<RemoteAudio> {
        self.audio_rx.clone()
    }

    pub fn subscribe_video(&self) -> flume::Receiver<RemoteVideo> {
        self.video_rx.clone()
    }

    pub fn bitrate_controller(&self) -> Arc<Mutex<SendBitrateController>> {
        self.controller.clone()
    }

    pub fn peer_connection(&self) -> Arc<dyn PeerConnection> {
        self.pc.clone()
    }

    pub async fn publish_audio(
        &self,
        ssrc: u32,
        payload_type: PayloadType,
    ) -> Result<Arc<LocalAudio>, RtcError> {
        let audio = Arc::new(LocalAudio::new(&self.pc, ssrc, payload_type).await?);
        spawn_twcc_monitor(audio.track_local(), self.controller.clone());
        self.audio_pubs.lock().expect("audio_pubs poisoned").push(audio.clone());
        Ok(audio)
    }

    pub async fn publish_video(
        &self,
        ssrc: u32,
        payload_type: PayloadType,
        codec: VpxCodec,
    ) -> Result<Arc<LocalVideo>, RtcError> {
        let video = Arc::new(LocalVideo::new(&self.pc, ssrc, payload_type, codec).await?);
        spawn_twcc_monitor(video.track_local(), self.controller.clone());
        self.video_pubs.lock().expect("video_pubs poisoned").push(video.clone());
        Ok(video)
    }

    pub async fn apply_remote_offer(&self, sdp: String) -> Result<String, RtcError> {
        let salvaged_candidates = inactive_offer_candidates(&sdp);

        let offer = RTCSessionDescription::offer(sdp)
            .map_err(|e| RtcError::Sdp(format!("parse remote offer: {e}")))?;
        self.pc
            .set_remote_description(offer)
            .await
            .map_err(|e| RtcError::Transport(format!("set_remote_description(offer): {e}")))?;

        if !salvaged_candidates.is_empty() {
            let count = salvaged_candidates.len();
            for candidate in salvaged_candidates {
                if let Err(e) = self
                    .pc
                    .add_ice_candidate(RTCIceCandidateInit {
                        candidate,
                        ..Default::default()
                    })
                    .await
                {
                    tracing::warn!(
                        "mezon-rtc: failed to re-add inactive-offer ICE candidate: {e}"
                    );
                }
            }
            tracing::info!(
                ice_candidates = count,
                "mezon-rtc: all offered m-lines inactive — re-added the SFU's in-SDP ICE candidates so the transport can establish"
            );
        }

        self.force_video_send_codec().await;

        let answer = self
            .pc
            .create_answer(None)
            .await
            .map_err(|e| RtcError::Sdp(format!("create_answer: {e}")))?;
        self.pc
            .set_local_description(answer)
            .await
            .map_err(|e| RtcError::Sdp(format!("set_local_description(answer): {e}")))?;

        self.wait_ice_gathering_complete(GATHER_TIMEOUT).await;

        let local = self
            .pc
            .local_description()
            .await
            .ok_or_else(|| RtcError::Sdp("no local description after answer".to_owned()))?;
        let audio_ssrcs: Vec<u32> = self
            .audio_pubs
            .lock()
            .expect("audio_pubs poisoned")
            .iter()
            .map(|p| p.ssrc())
            .collect();
        let video_ssrcs: Vec<u32> = self
            .video_pubs
            .lock()
            .expect("video_pubs poisoned")
            .iter()
            .map(|p| p.ssrc())
            .collect();
        let shaped = shape_answer_sdp(&local.sdp, &audio_ssrcs, &video_ssrcs);
        Ok(shaped)
    }

    async fn force_video_send_codec(&self) {
        let Some(video) = self
            .video_pubs
            .lock()
            .expect("video_pubs poisoned")
            .first()
            .cloned()
        else {
            return;
        };
        let prefs = crate::codecs::video_send_codec_preferences(video.codec());
        for transceiver in self.pc.get_transceivers().await {
            let Ok(Some(sender)) = transceiver.sender().await else {
                continue;
            };
            if sender.track().kind().await != RtpCodecKind::Video {
                continue;
            }
            match transceiver.set_codec_preferences(prefs.clone()).await {
                Ok(()) => tracing::info!(
                    codec = ?video.codec(),
                    "mezon-rtc: forced video send-transceiver codec preference so the SFU learns the real publish codec"
                ),
                Err(e) => {
                    tracing::warn!("mezon-rtc: set_codec_preferences(video) failed: {e}")
                }
            }
        }
    }

    pub async fn wait_ice_gathering_complete(&self, timeout: Duration) -> bool {
        matches!(
            tokio::time::timeout(timeout, self.gather_rx.recv_async()).await,
            Ok(Ok(()))
        )
    }

    pub async fn close(&self) -> Result<(), RtcError> {
        self.pc
            .close()
            .await
            .map_err(|e| RtcError::Transport(format!("close: {e}")))
    }
}

fn inactive_offer_candidates(sdp: &str) -> Vec<String> {
    let mut media_sections = 0usize;
    let mut all_inactive = true;
    let mut in_media = false;
    let mut current_inactive = false;
    let mut candidates: Vec<String> = Vec::new();

    for line in sdp.lines() {
        if line.starts_with("m=") {
            if in_media && !current_inactive {
                all_inactive = false;
            }
            in_media = true;
            current_inactive = false;
            media_sections += 1;
        } else if in_media {
            if line == "a=inactive" {
                current_inactive = true;
            } else if let Some(value) = line.strip_prefix("a=")
                && value.starts_with("candidate:")
            {
                candidates.push(value.to_owned());
            }
        }
    }
    if in_media && !current_inactive {
        all_inactive = false;
    }

    if media_sections > 0 && all_inactive {
        candidates
    } else {
        Vec::new()
    }
}

fn shape_answer_sdp(sdp: &str, audio_ssrcs: &[u32], video_ssrcs: &[u32]) -> String {
    let eol = if sdp.contains("\r\n") { "\r\n" } else { "\n" };

    let mut header: Vec<&str> = Vec::new();
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for line in sdp.split(eol) {
        if line.starts_with("m=") {
            sections.push(vec![line]);
        } else if let Some(section) = sections.last_mut() {
            section.push(line);
        } else {
            header.push(line);
        }
    }

    let mut out: Vec<String> = header.into_iter().map(str::to_owned).collect();
    let mut audio_idx = 0usize;
    let mut video_idx = 0usize;
    for section in &sections {
        let is_sendonly = section.contains(&"a=sendonly");
        let is_audio = section.first().is_some_and(|m| m.starts_with("m=audio"));
        let is_video = section.first().is_some_and(|m| m.starts_with("m=video"));
        if is_sendonly && is_audio {
            let target = audio_ssrcs.get(audio_idx).copied();
            audio_idx += 1;
            out.extend(shape_publisher_section(section, target, false));
        } else if is_sendonly && is_video {
            let target = video_ssrcs.get(video_idx).copied();
            video_idx += 1;
            out.extend(shape_publisher_section(section, target, true));
        } else {
            out.extend(section.iter().map(|l| (*l).to_owned()));
        }
    }
    out.join(eol)
}

fn shape_publisher_section(section: &[&str], target: Option<u32>, add_fid: bool) -> Vec<String> {
    let Some(current) = section.iter().find_map(|l| parse_ssrc_id(l)) else {
        return section.iter().map(|l| (*l).to_owned()).collect();
    };
    let primary = target.unwrap_or(current);
    let old_prefix = format!("a=ssrc:{current} ");
    let new_prefix = format!("a=ssrc:{primary} ");
    let rtx = primary.wrapping_add(1);
    let rtx_prefix = format!("a=ssrc:{rtx} ");
    let want_fid = add_fid && !section.iter().any(|l| l.starts_with("a=ssrc-group:FID"));

    let mut result: Vec<String> = Vec::with_capacity(section.len() + 4);
    let mut fid_inserted = false;
    let mut rtx_lines: Vec<String> = Vec::new();
    for line in section {
        if let Some(rest) = line.strip_prefix(old_prefix.as_str()) {
            if want_fid && !fid_inserted {
                result.push(format!("a=ssrc-group:FID {primary} {rtx}"));
                fid_inserted = true;
            }
            result.push(format!("{new_prefix}{rest}"));
            if want_fid {
                rtx_lines.push(format!("{rtx_prefix}{rest}"));
            }
        } else {
            result.append(&mut rtx_lines);
            result.push((*line).to_owned());
        }
    }
    result.append(&mut rtx_lines);
    result
}

fn parse_ssrc_id(line: &str) -> Option<u32> {
    line.strip_prefix("a=ssrc:")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO_ANSWER: &str = "v=0\r\n\
o=- 1 2 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 1\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 98\r\n\
a=setup:active\r\n\
a=mid:1\r\n\
a=rtpmap:98 VP9/90000\r\n\
a=ssrc:185273099 cname:mezon-video\r\n\
a=ssrc:185273099 msid:mezon-video video-185273099\r\n\
a=sendonly\r\n";

    #[test]
    fn shaping_adds_fid_group_to_video_sendonly() {
        let shaped = shape_answer_sdp(VIDEO_ANSWER, &[], &[]);
        assert!(
            shaped.contains("a=ssrc-group:FID 185273099 185273100"),
            "expected a FID group pairing the primary with a derived RTX SSRC:\n{shaped}"
        );
        assert!(
            shaped.contains("a=ssrc:185273100 cname:mezon-video"),
            "expected the RTX SSRC to mirror the primary cname:\n{shaped}"
        );
        let fid = shaped.find("a=ssrc-group:FID").expect("fid present");
        let primary = shaped
            .find("a=ssrc:185273099 cname")
            .expect("primary ssrc present");
        assert!(fid < primary, "FID group must precede the ssrc lines");
    }

    #[test]
    fn shaping_is_idempotent() {
        let once = shape_answer_sdp(VIDEO_ANSWER, &[], &[]);
        let twice = shape_answer_sdp(&once, &[], &[]);
        assert_eq!(once, twice, "shaping an already-shaped answer must be a no-op");
    }

    #[test]
    fn shaping_leaves_audio_untouched() {
        let audio_answer = "v=0\r\n\
t=0 0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
a=setup:active\r\n\
a=mid:0\r\n\
a=ssrc:168430090 cname:mezon-audio\r\n\
a=sendonly\r\n";
        let shaped = shape_answer_sdp(audio_answer, &[], &[]);
        assert!(
            !shaped.contains("ssrc-group:FID"),
            "audio has no RTX, so no FID group must be added:\n{shaped}"
        );
        assert_eq!(shaped, audio_answer, "audio answer must be unchanged");
    }

    const INACTIVE_AUDIENCE_OFFER: &str = "v=0\r\n\
o=- 1 2 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0 1\r\n\
a=ice-ufrag:sfu\r\n\
a=ice-pwd:sfupwd\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
a=mid:0\r\n\
a=candidate:1 1 udp 2130706431 10.0.0.1 5000 typ host\r\n\
a=inactive\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 98\r\n\
a=mid:1\r\n\
a=candidate:1 1 udp 2130706431 10.0.0.1 5000 typ host\r\n\
a=inactive\r\n";

    #[test]
    fn inactive_only_offer_yields_its_candidates() {
        let candidates = inactive_offer_candidates(INACTIVE_AUDIENCE_OFFER);
        assert_eq!(
            candidates,
            vec![
                "candidate:1 1 udp 2130706431 10.0.0.1 5000 typ host".to_owned(),
                "candidate:1 1 udp 2130706431 10.0.0.1 5000 typ host".to_owned(),
            ],
            "every m-line is inactive, so webrtc-rs drops these — we must salvage them"
        );
    }

    #[test]
    fn offer_with_an_active_section_salvages_nothing() {
        let mixed = INACTIVE_AUDIENCE_OFFER.replacen("a=inactive", "a=recvonly", 1);
        assert!(
            inactive_offer_candidates(&mixed).is_empty(),
            "a live section must suppress the salvage path"
        );
    }

    #[test]
    fn offer_with_no_media_sections_salvages_nothing() {
        assert!(inactive_offer_candidates("v=0\r\nt=0 0\r\n").is_empty());
    }

    #[test]
    fn shaping_forces_the_publisher_ssrc_on_video() {
        let shaped = shape_answer_sdp(VIDEO_ANSWER, &[], &[42]);
        assert!(
            shaped.contains("a=ssrc:42 cname:mezon-video"),
            "primary a=ssrc must be rewritten to the publisher's send-ssrc:\n{shaped}"
        );
        assert!(
            shaped.contains("a=ssrc-group:FID 42 43"),
            "FID must use the forced ssrc:\n{shaped}"
        );
        assert!(
            !shaped.contains("a=ssrc:185273099"),
            "no a=ssrc line may keep the webrtc-assigned ssrc:\n{shaped}"
        );
    }
}
