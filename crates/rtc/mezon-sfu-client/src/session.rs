use mezon_rtc::{RtcEvent, RtcSession};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

use crate::error::SfuClientError;
use crate::messages::{ClientMessage, ServerMessage};
use crate::transport::SfuTransport;

#[derive(Debug, Clone)]
pub struct SfuConfig {
    pub ws_url: String,
    pub room: String,
    pub role: String,
    pub token: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfuClientEvent {
    Joined,
    Connected,
    Disconnected,
    PeerLeft {
        user_id: String,
    },
    RoleChanged {
        user_id: String,
        role: String,
    },
    PeerJoined {
        user_id: String,
    },
    RoomSnapshot {
        user_ids: Vec<String>,
    },
    RemoteAudio,
    RemoteVideo,
    Error(String),
}

enum Control {
    SetRole(String),
    PushToTalk,
    Close,
}

enum Step {
    Frame(Option<Result<String, SfuClientError>>),
    Rtc(Result<RtcEvent, flume::RecvError>),
    Answer(Result<Result<String, SfuClientError>, flume::RecvError>),
    Control(Result<Control, flume::RecvError>),
}

pub struct SfuClient {
    events_rx: flume::Receiver<SfuClientEvent>,
    control_tx: flume::Sender<Control>,
    _task: tokio::task::JoinHandle<()>,
}

impl SfuClient {
    pub fn start<T: SfuTransport + 'static>(
        config: SfuConfig,
        transport: T,
        rtc: RtcSession,
    ) -> Self {
        let (events_tx, events_rx) = flume::unbounded();
        let (control_tx, control_rx) = flume::unbounded();
        let task = tokio::spawn(run(config, transport, rtc, events_tx, control_rx));
        Self {
            events_rx,
            control_tx,
            _task: task,
        }
    }

    pub fn events(&self) -> flume::Receiver<SfuClientEvent> {
        self.events_rx.clone()
    }

    pub fn set_role(&self, role: impl Into<String>) -> Result<(), SfuClientError> {
        self.control_tx
            .send(Control::SetRole(role.into()))
            .map_err(|_| SfuClientError::Closed)
    }

    pub fn push_to_talk(&self) -> Result<(), SfuClientError> {
        self.control_tx
            .send(Control::PushToTalk)
            .map_err(|_| SfuClientError::Closed)
    }

    pub fn close(&self) {
        let _ = self.control_tx.send(Control::Close);
    }
}

async fn run<T: SfuTransport>(
    config: SfuConfig,
    mut transport: T,
    rtc: RtcSession,
    events_tx: flume::Sender<SfuClientEvent>,
    control_rx: flume::Receiver<Control>,
) {
    let rtc_events = rtc.events();
    let (offer_tx, offer_rx) = flume::unbounded::<String>();
    let (answer_tx, answer_rx) = flume::unbounded::<Result<String, SfuClientError>>();

    let reneg = tokio::spawn(renegotiate(rtc, offer_rx, answer_tx));

    let join = ClientMessage::Join {
        room: config.room.clone(),
        role: config.role.clone(),
        token: config.token.clone(),
        user_id: config.user_id.clone(),
    };
    tracing::info!(
        ws_url = %config.ws_url,
        room = %config.room,
        role = %config.role,
        "mezon-sfu: sending join"
    );
    if send_msg(&mut transport, &events_tx, &join).await.is_err() {
        drop(offer_tx);
        let _ = reneg.await;
        return;
    }

    loop {
        let step = tokio::select! {
            frame = transport.recv() => Step::Frame(frame),
            evt = rtc_events.recv_async() => Step::Rtc(evt),
            answer = answer_rx.recv_async() => Step::Answer(answer),
            ctl = control_rx.recv_async() => Step::Control(ctl),
        };

        match step {
            Step::Frame(Some(Ok(text))) => {
                if !handle_server_frame(&text, &mut transport, &offer_tx, &events_tx).await {
                    break;
                }
            }
            Step::Frame(Some(Err(e))) => {
                let _ = events_tx.send(SfuClientEvent::Error(e.to_string()));
                break;
            }
            Step::Frame(None) => {
                let _ = events_tx.send(SfuClientEvent::Disconnected);
                break;
            }
            Step::Rtc(Ok(event)) => forward_rtc_event(event, &events_tx),
            Step::Rtc(Err(_)) => {}
            Step::Answer(Ok(Ok(sdp))) => {
                tracing::info!(sdp_bytes = sdp.len(), "mezon-sfu: -> answer");
                let answer = ClientMessage::Answer { sdp };
                if send_msg(&mut transport, &events_tx, &answer).await.is_err() {
                    break;
                }
            }
            Step::Answer(Ok(Err(e))) => {
                let _ = events_tx.send(SfuClientEvent::Error(e.to_string()));
            }
            Step::Answer(Err(_)) => {}
            Step::Control(Ok(Control::SetRole(role))) => {
                let msg = ClientMessage::RoleChange { role };
                if send_msg(&mut transport, &events_tx, &msg).await.is_err() {
                    break;
                }
            }
            Step::Control(Ok(Control::PushToTalk)) => {
                tracing::info!("mezon-sfu: -> push_to_talk");
                if send_msg(&mut transport, &events_tx, &ClientMessage::PushToTalk)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Step::Control(Ok(Control::Close) | Err(_)) => break,
        }
    }

    drop(offer_tx);
    let _ = reneg.await;
}

async fn handle_server_frame<T: SfuTransport>(
    text: &str,
    transport: &mut T,
    offer_tx: &flume::Sender<String>,
    events_tx: &flume::Sender<SfuClientEvent>,
) -> bool {
    let msg: ServerMessage = match serde_json::from_str(text) {
        Ok(msg) => msg,
        Err(e) => {
            tracing::warn!(error = %e, frame = %text, "unparseable sfu frame");
            let _ = events_tx.send(SfuClientEvent::Error(format!("malformed frame: {e}")));
            return true;
        }
    };

    match msg {
        ServerMessage::Joined { .. } => {
            tracing::info!("mezon-sfu: <- joined (SFU acknowledged join)");
            let _ = events_tx.send(SfuClientEvent::Joined);
        }
        ServerMessage::Offer { sdp } => {
            tracing::info!(sdp_bytes = sdp.len(), "mezon-sfu: <- offer (applying; will answer)");
            if offer_tx.send(sdp).is_err() {
                return false;
            }
        }
        ServerMessage::Ping => {
            tracing::debug!("mezon-sfu: <- ping (replying pong)");
            if send_msg(transport, events_tx, &ClientMessage::Pong)
                .await
                .is_err()
            {
                return false;
            }
        }
        ServerMessage::Pong => {}
        ServerMessage::PeerLeft { user_id, .. } => {
            tracing::info!(%user_id, "mezon-sfu: <- peer_left");
            let _ = events_tx.send(SfuClientEvent::PeerLeft { user_id });
        }
        ServerMessage::RoleChanged { user_id, role } => {
            tracing::info!(%user_id, %role, "mezon-sfu: <- role_changed");
            let _ = events_tx.send(SfuClientEvent::RoleChanged { user_id, role });
        }
        ServerMessage::PeerJoined { peer } => {
            if let Some(peer) = peer {
                tracing::info!(user_id = %peer.user_id, "mezon-sfu: <- peer_joined");
                let _ = events_tx.send(SfuClientEvent::PeerJoined {
                    user_id: peer.user_id,
                });
            }
        }
        ServerMessage::RoomSnapshot { members } => {
            let user_ids: Vec<String> = members.into_iter().map(|m| m.user_id).collect();
            tracing::info!(count = user_ids.len(), "mezon-sfu: <- room_snapshot");
            let _ = events_tx.send(SfuClientEvent::RoomSnapshot { user_ids });
        }
        ServerMessage::Error { message } => {
            tracing::warn!(%message, "mezon-sfu: <- ERROR from SFU");
            let _ = events_tx.send(SfuClientEvent::Error(message));
        }
        ServerMessage::Unknown => {
            tracing::debug!("mezon-sfu: <- ignoring unrecognized message type");
        }
    }
    true
}

fn forward_rtc_event(event: RtcEvent, events_tx: &flume::Sender<SfuClientEvent>) {
    let mapped = match event {
        RtcEvent::Connected => {
            tracing::info!("mezon-sfu: ✅ CONNECTED — ICE+DTLS up, media path to SFU established");
            Some(SfuClientEvent::Connected)
        }
        RtcEvent::Disconnected => {
            tracing::warn!("mezon-sfu: PeerConnection disconnected");
            Some(SfuClientEvent::Disconnected)
        }
        RtcEvent::TrackAdded { kind, .. } => match kind {
            RtpCodecKind::Audio => {
                tracing::info!("mezon-sfu: remote AUDIO track added");
                Some(SfuClientEvent::RemoteAudio)
            }
            RtpCodecKind::Video => {
                tracing::info!("mezon-sfu: remote VIDEO track added");
                Some(SfuClientEvent::RemoteVideo)
            }
            RtpCodecKind::Unspecified => None,
        },
        RtcEvent::TrackRemoved { .. } => None,
    };
    if let Some(ev) = mapped {
        let _ = events_tx.send(ev);
    }
}

async fn send_msg<T: SfuTransport>(
    transport: &mut T,
    events_tx: &flume::Sender<SfuClientEvent>,
    msg: &ClientMessage,
) -> Result<(), SfuClientError> {
    let text = serde_json::to_string(msg).map_err(|e| SfuClientError::Protocol(e.to_string()))?;
    if let Err(e) = transport.send(text).await {
        let _ = events_tx.send(SfuClientEvent::Error(e.to_string()));
        return Err(e);
    }
    Ok(())
}

async fn renegotiate(
    rtc: RtcSession,
    offer_rx: flume::Receiver<String>,
    answer_tx: flume::Sender<Result<String, SfuClientError>>,
) {
    while let Ok(sdp) = offer_rx.recv_async().await {
        let result = rtc.apply_remote_offer(sdp).await.map_err(SfuClientError::from);
        if answer_tx.send(result).is_err() {
            break;
        }
    }
    let _ = rtc.close().await;
}
