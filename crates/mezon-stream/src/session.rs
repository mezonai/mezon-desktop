use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use flume::{Receiver, Sender};
use futures::{SinkExt, StreamExt};
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::ice_candidate::IceCandidate;
use livekit::webrtc::media_stream_track::MediaStreamTrack;
use livekit::webrtc::peer_connection::OfferOptions;
use livekit::webrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceServer, IceTransportsType, PeerConnectionFactory, RtcConfiguration,
};
use livekit::webrtc::prelude::MediaType;
use livekit::webrtc::prelude::VideoBuffer;
use livekit::webrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
use livekit::webrtc::session_description::{SdpType, SessionDescription};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use mezon_voice::{StreamAudioOutput, VideoFrameStore, i420_to_bgra_into};
use parking_lot::Mutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::STREAM_FRAME_KEY;
use crate::signaling::{
    InboundMessage, OutboundMessage, parse_channels, parse_ice_candidate, parse_sdp_answer,
    ws_connect_url,
};

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
            let runtime_handle = runtime.handle().clone();
            runtime.block_on(run_session(
                config,
                frame_store,
                audio_output,
                stop_rx,
                event_tx,
                runtime_handle,
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
    runtime_handle: tokio::runtime::Handle,
) {
    if let Err(_err) = run_session_inner(
        config,
        frame_store,
        audio,
        stop_rx,
        event_tx.clone(),
        runtime_handle,
    )
    .await
    {
        tracing::warn!("stream session ended with error");
        let _ = event_tx.send(StreamEvent::Error("Stream connection failed".to_string()));
    }
    let _ = event_tx.send(StreamEvent::Disconnected);
}

async fn run_session_inner(
    config: StreamSessionConfig,
    frame_store: Arc<VideoFrameStore>,
    audio: Arc<StreamAudioOutput>,
    stop_rx: Receiver<()>,
    event_tx: Sender<StreamEvent>,
    runtime_handle: tokio::runtime::Handle,
) -> anyhow::Result<()> {
    let url = ws_connect_url(&config.ws_base_url, &config.username, &config.token)?;
    let (ws_stream, _) = connect_async(url.as_str())
        .await
        .context("stream websocket connect failed")?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (outbound_tx, outbound_rx) = flume::unbounded::<OutboundMessage>();

    let factory = PeerConnectionFactory::default();
    let rtc_config = RtcConfiguration {
        ice_servers: vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: String::new(),
            password: String::new(),
        }],
        continual_gathering_policy: ContinualGatheringPolicy::GatherContinually,
        ice_transport_type: IceTransportsType::All,
    };
    let pc = factory
        .create_peer_connection(rtc_config)
        .context("create peer connection failed")?;

    pc.on_ice_candidate(Some(Box::new(move |candidate| {
        if candidate.candidate().is_empty() {
            return;
        }
        let value = serde_json::json!({
            "candidate": candidate.candidate(),
            "sdpMid": candidate.sdp_mid(),
            "sdpMLineIndex": candidate.sdp_mline_index(),
        });
        let _ = outbound_tx.send(OutboundMessage::ice_candidate(value));
    })));

    let event_for_track = event_tx.clone();
    let frame_store_for_track = frame_store.clone();
    let audio_for_track = audio.clone();
    pc.on_track(Some(Box::new(move |track_event| match track_event.track {
        MediaStreamTrack::Video(video_track) => {
            let _ = event_for_track.send(StreamEvent::RemoteVideo(true));
            let store = frame_store_for_track.clone();
            let events = event_for_track.clone();
            let runtime_handle = runtime_handle.clone();
            std::thread::spawn(move || {
                pump_video_track(video_track, store, events, runtime_handle);
            });
        }
        MediaStreamTrack::Audio(audio_track) => {
            let _ = event_for_track.send(StreamEvent::RemoteAudio(true));
            let player = audio_for_track.clone();
            let out_fmt = player.format();
            let events = event_for_track.clone();
            let runtime_handle = runtime_handle.clone();
            std::thread::spawn(move || {
                pump_audio_track(audio_track, player, out_fmt, events, runtime_handle);
            });
        }
    })));

    pc.add_transceiver_for_media(
        MediaType::Audio,
        RtpTransceiverInit {
            direction: RtpTransceiverDirection::RecvOnly,
            stream_ids: vec![],
            send_encodings: vec![],
        },
    )
    .context("add audio transceiver failed")?;

    let offer = pc
        .create_offer(OfferOptions {
            ice_restart: false,
            offer_to_receive_audio: true,
            offer_to_receive_video: true,
        })
        .await
        .context("create offer failed")?;
    pc.set_local_description(offer.clone())
        .await
        .context("set local description failed")?;

    let offer_value = serde_json::json!({
        "type": "offer",
        "sdp": offer.to_string(),
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

    let pc = Arc::new(pc);
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
                        let answer = SessionDescription::parse(&sdp, SdpType::Answer)
                            .map_err(|e| anyhow!("sdp answer parse: {} {}", e.line, e.description))?;
                        pc.set_remote_description(answer)
                            .await
                            .context("set remote description failed")?;
                    }
                    "ice_candidate" => {
                        let Some(value) = inbound.value.as_ref() else { continue; };
                        let Some((sdp_mid, sdp_mline_index, candidate)) = parse_ice_candidate(value) else { continue; };
                        let ice = IceCandidate::parse(&sdp_mid, sdp_mline_index, &candidate)
                            .map_err(|e| anyhow!("ice candidate parse: {} {}", e.line, e.description))?;
                        pc.add_ice_candidate(ice).await.ok();
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

    pc.close();
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

fn pump_video_track(
    video_track: livekit::webrtc::video_track::RtcVideoTrack,
    frame_store: Arc<VideoFrameStore>,
    event_tx: Sender<StreamEvent>,
    runtime_handle: tokio::runtime::Handle,
) {
    runtime_handle.block_on(async {
        let mut bgra: Vec<u8> = Vec::new();
        let mut stream = NativeVideoStream::new(video_track);
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
            if let Some(recycled) =
                frame_store.publish(STREAM_FRAME_KEY, width, height, std::mem::take(&mut bgra))
            {
                bgra = recycled;
            }
        }
        let _ = event_tx.send(StreamEvent::RemoteVideo(false));
    });
}

fn pump_audio_track(
    audio_track: livekit::webrtc::audio_track::RtcAudioTrack,
    player: Arc<StreamAudioOutput>,
    out_fmt: mezon_voice::AudioFormat,
    event_tx: Sender<StreamEvent>,
    runtime_handle: tokio::runtime::Handle,
) {
    runtime_handle.block_on(async {
        let mut stream = NativeAudioStream::new(
            audio_track,
            out_fmt.sample_rate as i32,
            out_fmt.channels as i32,
        );
        while let Some(frame) = stream.next().await {
            player.push(&frame.data);
        }
        player.clear();
        let _ = event_tx.send(StreamEvent::RemoteAudio(false));
    });
}
