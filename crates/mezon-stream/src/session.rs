use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use flume::{Receiver, Sender};
use futures::{SinkExt, StreamExt};
use mezon_codec::{EncodedFrame, OpusDecoder, VpxCodec, VpxDecoder};
use mezon_rtc::subscribe::{spawn_opus_receiver, spawn_video_receiver};
use mezon_rtc::{PeerConnectionOpts, build_peer_connection};
use mezon_voice::{AudioFormat, StreamAudioOutput, VideoFrameStore, i420_to_bgra_into};
use parking_lot::Mutex;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceCandidateInit;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::peer_connection::{PeerConnectionEventHandler, RTCPeerConnectionIceEvent};

use crate::STREAM_FRAME_KEY;
use crate::signaling::{
    InboundMessage, OutboundMessage, parse_channels, parse_ice_candidate, parse_sdp_answer,
    ws_connect_url,
};

const OPUS_DECODE_RATE: u32 = 48_000;

#[derive(Debug, Clone)]
pub struct StreamSessionConfig {
    pub ws_base_url: String,
    pub username: String,
    pub token: String,
    pub clan_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub stream_id: String,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Live,
    NoBroadcast,
    RemoteVideo(bool),
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
        frame_store: Arc<VideoFrameStore>,
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

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let _ = event_tx.send(StreamEvent::Error("stream runtime failed".into()));
                let _ = event_tx.send(StreamEvent::Disconnected);
                return;
            };
            runtime.block_on(run_session(
                config,
                frame_store,
                audio_output,
                stop_rx,
                event_tx,
            ));
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
        let _ = self.stop_tx.send(());
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.disconnect();
    }
}

async fn run_session(
    config: StreamSessionConfig,
    frame_store: Arc<VideoFrameStore>,
    audio: Arc<StreamAudioOutput>,
    stop_rx: Receiver<()>,
    event_tx: Sender<StreamEvent>,
) {
    if let Err(_err) =
        run_session_inner(config, frame_store, audio, stop_rx, event_tx.clone()).await
    {
        tracing::warn!("stream session ended with error");
        let _ = event_tx.send(StreamEvent::Error("Stream connection failed".to_string()));
    }
    let _ = event_tx.send(StreamEvent::Disconnected);
}

struct StreamHandler {
    outbound_tx: Sender<OutboundMessage>,
    event_tx: Sender<StreamEvent>,
    frame_store: Arc<VideoFrameStore>,
    audio: Arc<StreamAudioOutput>,
    out_fmt: AudioFormat,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for StreamHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let Ok(init) = event.candidate.to_json() else {
            return;
        };
        if init.candidate.is_empty() {
            return;
        }
        let value = serde_json::json!({
            "candidate": init.candidate,
            "sdpMid": init.sdp_mid,
            "sdpMLineIndex": init.sdp_mline_index,
        });
        let _ = self.outbound_tx.send(OutboundMessage::ice_candidate(value));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        match track.kind().await {
            RtpCodecKind::Video => {
                let _ = self.event_tx.send(StreamEvent::RemoteVideo(true));
                let codec = infer_vpx_codec(&track).await.unwrap_or(VpxCodec::Vp9);
                let frames = spawn_video_receiver(track, codec);
                tokio::spawn(decode_video(
                    frames,
                    codec,
                    self.frame_store.clone(),
                    self.event_tx.clone(),
                ));
            }
            RtpCodecKind::Audio => {
                let _ = self.event_tx.send(StreamEvent::RemoteAudio(true));
                let packets = spawn_opus_receiver(track);
                tokio::spawn(decode_audio(
                    packets,
                    self.audio.clone(),
                    self.out_fmt,
                    self.event_tx.clone(),
                ));
            }
            RtpCodecKind::Unspecified => {}
        }
    }
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

fn recvonly() -> Option<RTCRtpTransceiverInit> {
    Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly,
        streams: vec![],
        send_encodings: vec![],
    })
}

async fn run_session_inner(
    config: StreamSessionConfig,
    frame_store: Arc<VideoFrameStore>,
    audio: Arc<StreamAudioOutput>,
    stop_rx: Receiver<()>,
    event_tx: Sender<StreamEvent>,
) -> anyhow::Result<()> {
    let url = ws_connect_url(&config.ws_base_url, &config.username, &config.token)?;
    let (ws_stream, _) = connect_async(url.as_str())
        .await
        .context("stream websocket connect failed")?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (outbound_tx, outbound_rx) = flume::unbounded::<OutboundMessage>();

    let out_fmt = audio.format();
    let handler = Arc::new(StreamHandler {
        outbound_tx,
        event_tx: event_tx.clone(),
        frame_store,
        audio,
        out_fmt,
    });
    let pc = build_peer_connection(handler, PeerConnectionOpts { loopback: false })
        .await
        .context("build peer connection failed")?;

    pc.add_transceiver_from_kind(RtpCodecKind::Audio, recvonly())
        .await
        .map_err(|e| anyhow!("add audio transceiver failed: {e}"))?;
    pc.add_transceiver_from_kind(RtpCodecKind::Video, recvonly())
        .await
        .map_err(|e| anyhow!("add video transceiver failed: {e}"))?;

    let offer = pc
        .create_offer(None)
        .await
        .map_err(|e| anyhow!("create offer failed: {e}"))?;
    pc.set_local_description(offer)
        .await
        .map_err(|e| anyhow!("set local description failed: {e}"))?;
    let local = pc
        .local_description()
        .await
        .ok_or_else(|| anyhow!("missing local description"))?;

    let offer_value = serde_json::json!({
        "type": "offer",
        "sdp": local.sdp,
    });
    send_ws(
        &mut ws_tx,
        OutboundMessage::session_subscriber(
            &config.clan_id,
            &config.channel_id,
            &config.user_id,
            offer_value,
        ),
    )
    .await?;
    send_ws(&mut ws_tx, OutboundMessage::get_channels()).await?;

    let mut live = false;

    loop {
        tokio::select! {
            _ = stop_rx.recv_async() => {
                break;
            }
            outbound = outbound_rx.recv_async() => {
                let Ok(message) = outbound else { continue; };
                send_ws(&mut ws_tx, message).await?;
            }
            msg = ws_rx.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg.context("websocket read failed")?;
                if msg.is_close() {
                    break;
                }
                let Message::Text(text) = msg else { continue; };
                let inbound: InboundMessage = match serde_json::from_str(&text) {
                    Ok(inbound) => inbound,
                    Err(_) => {
                        tracing::warn!("invalid websocket payload");
                        continue;
                    }
                };
                match inbound.key.as_str() {
                    "channels" => {
                        let Some(value) = inbound.value.as_ref() else { continue; };
                        let channels = parse_channels(value);
                        let is_live = channels.iter().any(|id| id == &config.stream_id);
                        if is_live && !live {
                            live = true;
                            send_ws(
                                &mut ws_tx,
                                OutboundMessage::connect_subscriber(
                                    &config.clan_id,
                                    &config.channel_id,
                                    &config.user_id,
                                    &config.stream_id,
                                ),
                            ).await?;
                            let _ = event_tx.send(StreamEvent::Live);
                        } else if !is_live {
                            live = false;
                            let _ = event_tx.send(StreamEvent::NoBroadcast);
                        }
                    }
                    "sd_answer" => {
                        let Some(value) = inbound.value.as_ref() else { continue; };
                        let Some(sdp) = parse_sdp_answer(value) else { continue; };
                        let answer = RTCSessionDescription::answer(sdp)
                            .map_err(|e| anyhow!("sdp answer parse: {e}"))?;
                        pc.set_remote_description(answer)
                            .await
                            .map_err(|e| anyhow!("set remote description failed: {e}"))?;
                    }
                    "ice_candidate" => {
                        let Some(value) = inbound.value.as_ref() else { continue; };
                        let Some((sdp_mid, sdp_mline_index, candidate)) = parse_ice_candidate(value) else { continue; };
                        let init = RTCIceCandidateInit {
                            candidate,
                            sdp_mid: Some(sdp_mid),
                            sdp_mline_index: Some(sdp_mline_index.max(0) as u16),
                            username_fragment: None,
                            url: None,
                        };
                        pc.add_ice_candidate(init).await.ok();
                    }
                    "error" => {
                        let message = inbound
                            .value
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_else(|| "stream signaling error".into());
                        let _ = event_tx.send(StreamEvent::Error(message));
                    }
                    "session_received" => {}
                    _ => {}
                }
            }
        }
    }

    let _ = pc.close().await;
    Ok(())
}

async fn send_ws(
    ws_tx: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    message: OutboundMessage,
) -> anyhow::Result<()> {
    let text = serde_json::to_string(&message)?;
    ws_tx
        .send(Message::Text(text.into()))
        .await
        .context("websocket send failed")?;
    Ok(())
}

async fn decode_video(
    frames: Receiver<EncodedFrame>,
    codec: VpxCodec,
    frame_store: Arc<VideoFrameStore>,
    event_tx: Sender<StreamEvent>,
) {
    let mut decoder = match VpxDecoder::new(codec) {
        Ok(decoder) => decoder,
        Err(e) => {
            tracing::error!("stream vpx decoder unavailable: {e}");
            let _ = event_tx.send(StreamEvent::RemoteVideo(false));
            return;
        }
    };
    let mut bgra: Vec<u8> = Vec::new();
    while let Ok(frame) = frames.recv_async().await {
        let images = match decoder.decode(&frame.data) {
            Ok(images) => images,
            Err(e) => {
                tracing::warn!("stream vpx decode failed: {e}");
                continue;
            }
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
            if let Some(recycled) = frame_store.publish(
                STREAM_FRAME_KEY,
                image.width,
                image.height,
                std::mem::take(&mut bgra),
            ) {
                bgra = recycled;
            }
        }
    }
    let _ = event_tx.send(StreamEvent::RemoteVideo(false));
}

async fn decode_audio(
    packets: Receiver<Vec<u8>>,
    player: Arc<StreamAudioOutput>,
    out_fmt: AudioFormat,
    event_tx: Sender<StreamEvent>,
) {
    let mut decoder = match OpusDecoder::new(1) {
        Ok(decoder) => decoder,
        Err(e) => {
            tracing::error!("stream opus decoder unavailable: {e}");
            let _ = event_tx.send(StreamEvent::RemoteAudio(false));
            return;
        }
    };
    let channels = out_fmt.channels.max(1) as usize;
    let mut resampler = LinearResampler::new(OPUS_DECODE_RATE, out_fmt.sample_rate);
    let mut resampled: Vec<f32> = Vec::new();
    let mut interleaved: Vec<i16> = Vec::new();
    while let Ok(packet) = packets.recv_async().await {
        let mono = match decoder.decode(&packet) {
            Ok(mono) => mono,
            Err(e) => {
                tracing::warn!("stream opus decode failed: {e}");
                continue;
            }
        };
        resampled.clear();
        resampler.process(&mono, &mut resampled);
        interleaved.clear();
        interleaved.reserve(resampled.len() * channels);
        for sample in &resampled {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            for _ in 0..channels {
                interleaved.push(value);
            }
        }
        player.push(&interleaved);
    }
    player.clear();
    let _ = event_tx.send(StreamEvent::RemoteAudio(false));
}

struct LinearResampler {
    ratio: f64,
    passthrough: bool,
    prev: f32,
    primed: bool,
    frac: f64,
}

impl LinearResampler {
    fn new(src_rate: u32, dst_rate: u32) -> Self {
        Self {
            ratio: if dst_rate == 0 {
                1.0
            } else {
                src_rate as f64 / dst_rate as f64
            },
            passthrough: dst_rate == 0 || src_rate == dst_rate,
            prev: 0.0,
            primed: false,
            frac: 0.0,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        for &sample in input {
            if !self.primed {
                self.prev = sample;
                self.primed = true;
                continue;
            }
            while self.frac < 1.0 {
                let interp = self.prev + (sample - self.prev) * self.frac as f32;
                out.push(interp);
                self.frac += self.ratio;
            }
            self.frac -= 1.0;
            self.prev = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_passthrough_preserves_samples() {
        let mut resampler = LinearResampler::new(48_000, 48_000);
        let mut out = Vec::new();
        resampler.process(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn resampler_downsamples_roughly_by_ratio() {
        let mut resampler = LinearResampler::new(48_000, 24_000);
        let input: Vec<f32> = (0..4800).map(|n| (n as f32 * 0.01).sin()).collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 0.5).abs() < 0.02, "expected ~0.5, got {ratio}");
    }

    #[test]
    fn resampler_upsamples_roughly_by_ratio() {
        let mut resampler = LinearResampler::new(24_000, 48_000);
        let input: Vec<f32> = (0..2400).map(|n| (n as f32 * 0.01).sin()).collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        let ratio = out.len() as f32 / input.len() as f32;
        assert!((ratio - 2.0).abs() < 0.02, "expected ~2.0, got {ratio}");
    }

    #[test]
    fn resampler_keeps_phase_across_chunks() {
        let input: Vec<f32> = (0..9600).map(|n| (n as f32 * 0.005).sin()).collect();
        let mut whole = LinearResampler::new(48_000, 44_100);
        let mut whole_out = Vec::new();
        whole.process(&input, &mut whole_out);

        let mut split = LinearResampler::new(48_000, 44_100);
        let mut split_out = Vec::new();
        split.process(&input[..4800], &mut split_out);
        split.process(&input[4800..], &mut split_out);

        let diff = (whole_out.len() as i64 - split_out.len() as i64).abs();
        assert!(
            diff <= 1,
            "chunked output length must match whole within 1: {diff}"
        );
    }
}
