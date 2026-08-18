//! Loopback connectivity acceptance test for the SFU-tuned PeerConnection factory.
//!
//! Builds two `PeerConnection`s via [`mezon_rtc::engine::build_peer_connection`] with
//! loopback candidates enabled, runs the non-trickle offer/answer exchange entirely
//! in-process (no SFU), and asserts BOTH peers reach
//! [`RTCPeerConnectionState::Connected`] — a genuine ICE + DTLS handshake.

use std::sync::Arc;
use std::time::Duration;

use mezon_rtc::engine::{PeerConnectionOpts, build_peer_connection};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use webrtc::peer_connection::{
    PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::{Receiver, Runtime, Sender, channel, default_runtime, timeout};

/// Reports peer-connection lifecycle over channels: ICE-gathering completion (for the
/// non-trickle exchange) and every `RTCPeerConnectionState` transition.
struct StateHandler {
    gather_tx: Sender<()>,
    state_tx: Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for StateHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.try_send(state);
    }
}

/// Drain connection-state transitions until `Connected` (returns `true`) or a terminal
/// failure / timeout (returns `false`).
async fn wait_connected(
    rx: &mut Receiver<RTCPeerConnectionState>,
    rt: &dyn Runtime,
) -> bool {
    let wait = async {
        loop {
            match rx.recv().await {
                Some(RTCPeerConnectionState::Connected) => return true,
                Some(RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed) => {
                    return false;
                }
                Some(_) => continue,
                None => return false,
            }
        }
    };
    timeout(rt, Duration::from_secs(20), wait).await.unwrap_or(false)
}

async fn run(rt: Arc<dyn Runtime>) {
    let (a_gather_tx, mut a_gather_rx) = channel::<()>(1);
    let (a_state_tx, mut a_state_rx) = channel::<RTCPeerConnectionState>(16);
    let (b_gather_tx, mut b_gather_rx) = channel::<()>(1);
    let (b_state_tx, mut b_state_rx) = channel::<RTCPeerConnectionState>(16);

    let peer_a = build_peer_connection(
        Arc::new(StateHandler {
            gather_tx: a_gather_tx,
            state_tx: a_state_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer A");

    let peer_b = build_peer_connection(
        Arc::new(StateHandler {
            gather_tx: b_gather_tx,
            state_tx: b_state_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer B");

    // Give A something to negotiate: a recvonly audio m-line, which exercises the
    // registered SFU codec table (Opus PT111) through real SDP negotiation.
    peer_a
        .add_transceiver_from_kind(RtpCodecKind::Audio, None)
        .await
        .expect("add audio transceiver to A");

    // A: create offer -> set local -> wait for ICE gathering complete -> read SDP.
    let offer = peer_a.create_offer(None).await.expect("A create_offer");
    peer_a
        .set_local_description(offer)
        .await
        .expect("A set_local_description(offer)");
    let _ = timeout(&*rt, Duration::from_secs(10), a_gather_rx.recv()).await;
    let offer_sdp = peer_a
        .local_description()
        .await
        .expect("A local_description after gathering");

    // B: set remote(offer) -> create answer -> set local -> wait gathering -> read SDP.
    peer_b
        .set_remote_description(offer_sdp)
        .await
        .expect("B set_remote_description(offer)");
    let answer = peer_b.create_answer(None).await.expect("B create_answer");
    peer_b
        .set_local_description(answer)
        .await
        .expect("B set_local_description(answer)");
    let _ = timeout(&*rt, Duration::from_secs(10), b_gather_rx.recv()).await;
    let answer_sdp = peer_b
        .local_description()
        .await
        .expect("B local_description after gathering");

    // A: set remote(answer). The DTLS/ICE handshake now runs to completion.
    peer_a
        .set_remote_description(answer_sdp)
        .await
        .expect("A set_remote_description(answer)");

    let a_connected = wait_connected(&mut a_state_rx, &*rt).await;
    let b_connected = wait_connected(&mut b_state_rx, &*rt).await;

    assert!(a_connected, "peer A never reached RTCPeerConnectionState::Connected");
    assert!(b_connected, "peer B never reached RTCPeerConnectionState::Connected");

    let _ = peer_a.close().await;
    let _ = peer_b.close().await;
}

#[test]
fn loopback_two_peers_connect() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    let rt_for_body = rt.clone();
    rt.block_on(Box::pin(async move { run(rt_for_body).await }));
}
