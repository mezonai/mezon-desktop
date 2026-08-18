//! Loopback video round-trip: VP9 publish + subscribe/reassemble/decode over webrtc-rs.
//!
//! Peer A publishes a VP9 keyframe followed by several inter frames (a 320x240 gradient
//! encoded by [`mezon_codec::VpxEncoder`]) over a [`mezon_rtc::publish::LocalVideo`] track.
//! Peer B's `on_track` spawns [`mezon_rtc::subscribe::spawn_video_receiver`], which polls
//! the remote track, depacketizes VP9 RTP payloads, and reassembles whole encoded frames.
//! Those frames are decoded by [`mezon_codec::VpxDecoder`]. We assert the first received
//! frame is a keyframe and the decode reproduces 320x240 — proof that fragmented video
//! actually flowed A -> RTP/SRTP -> B, was reassembled correctly, and decodes.

use std::sync::Arc;
use std::time::Duration;

use mezon_codec::{EncodedFrame, I420Frame, VpxCodec, VpxDecoder, VpxEncoder};
use mezon_rtc::codecs::PT_VP9;
use mezon_rtc::engine::{PeerConnectionOpts, build_peer_connection};
use mezon_rtc::publish::LocalVideo;
use mezon_rtc::subscribe::spawn_video_receiver;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::peer_connection::{
    PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::{Receiver, Runtime, Sender, channel, default_runtime, timeout};

/// Handler that reports lifecycle over channels and, on receiving a remote track, spins up
/// the VP9 receiver and hands its [`flume::Receiver`] back to the test body.
struct VideoHandler {
    gather_tx: Sender<()>,
    state_tx: Sender<RTCPeerConnectionState>,
    frames_tx: flume::Sender<flume::Receiver<EncodedFrame>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for VideoHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.try_send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let rx = spawn_video_receiver(track, VpxCodec::Vp9);
        let _ = self.frames_tx.send(rx);
    }
}

async fn wait_connected(rx: &mut Receiver<RTCPeerConnectionState>, rt: &dyn Runtime) -> bool {
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
    timeout(rt, Duration::from_secs(20), wait)
        .await
        .unwrap_or(false)
}

/// A 320x240 horizontal luma gradient (left dark, right bright).
fn gradient(width: u32, height: u32) -> I420Frame {
    let mut f = I420Frame::new_black(width, height);
    for row in 0..height {
        for col in 0..width {
            f.y[(row * width + col) as usize] = ((col * 255) / width) as u8;
        }
    }
    f
}

async fn run(rt: Arc<dyn Runtime>) {
    let (a_gather_tx, mut a_gather_rx) = channel::<()>(1);
    let (a_state_tx, mut a_state_rx) = channel::<RTCPeerConnectionState>(16);
    let (b_gather_tx, mut b_gather_rx) = channel::<()>(1);
    let (b_state_tx, mut b_state_rx) = channel::<RTCPeerConnectionState>(16);

    // A never receives a track (it is the publisher); give it a dead frames channel.
    let (a_frames_tx, _a_frames_rx) = flume::bounded(1);
    // B hands the video receiver built inside `on_track` back to the test body.
    let (b_frames_tx, b_frames_rx) = flume::bounded::<flume::Receiver<EncodedFrame>>(1);

    let peer_a = build_peer_connection(
        Arc::new(VideoHandler {
            gather_tx: a_gather_tx,
            state_tx: a_state_tx,
            frames_tx: a_frames_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer A");

    let peer_b = build_peer_connection(
        Arc::new(VideoHandler {
            gather_tx: b_gather_tx,
            state_tx: b_state_tx,
            frames_tx: b_frames_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer B");

    // A publishes a VP9 track BEFORE creating the offer so it lands in the SDP.
    let ssrc = 0x0BAD_F00Du32;
    let video = LocalVideo::new(&peer_a, ssrc, PT_VP9, VpxCodec::Vp9)
        .await
        .expect("A add VP9 track");

    // Non-trickle offer/answer, moving SDP between the PCs in-process.
    let offer = peer_a.create_offer(None).await.expect("A create_offer");
    peer_a
        .set_local_description(offer)
        .await
        .expect("A set_local(offer)");
    let _ = timeout(&*rt, Duration::from_secs(10), a_gather_rx.recv()).await;
    let offer_sdp = peer_a.local_description().await.expect("A local_description");

    peer_b
        .set_remote_description(offer_sdp)
        .await
        .expect("B set_remote(offer)");
    let answer = peer_b.create_answer(None).await.expect("B create_answer");
    peer_b
        .set_local_description(answer)
        .await
        .expect("B set_local(answer)");
    let _ = timeout(&*rt, Duration::from_secs(10), b_gather_rx.recv()).await;
    let answer_sdp = peer_b.local_description().await.expect("B local_description");

    peer_a
        .set_remote_description(answer_sdp)
        .await
        .expect("A set_remote(answer)");

    assert!(
        wait_connected(&mut a_state_rx, &*rt).await,
        "peer A never reached Connected"
    );
    assert!(
        wait_connected(&mut b_state_rx, &*rt).await,
        "peer B never reached Connected"
    );

    // A: encode a keyframe + inter frames of the gradient and publish them, paced ~20 ms
    // apart. This must start BEFORE waiting on `on_track`: webrtc-rs only opens the remote
    // track (firing `on_track`) once the first RTP packet arrives.
    let (w, h) = (320u32, 240u32);
    let writer = tokio::spawn(async move {
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, w, h, 400).expect("vp9 encoder");
        let frame = gradient(w, h);
        // Repeat the keyframe + inter sequence a few times so `on_track` reliably fires
        // and enough frames arrive even if a leading packet or two is missed pre-open.
        for i in 0..30i64 {
            let force_key = i == 0;
            let encoded = enc.encode(&frame, force_key, i * 3000).expect("vp9 encode");
            for ef in &encoded {
                if video
                    .write_encoded(ef, Duration::from_millis(33))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // B's `on_track` fires on the first received RTP packet and hands us the receiver.
    let frames_rx = timeout(&*rt, Duration::from_secs(5), b_frames_rx.recv_async())
        .await
        .expect("on_track fired within timeout")
        .expect("received the video receiver");

    // Collect reassembled encoded frames until we have a keyframe plus a couple more.
    let mut received: Vec<EncodedFrame> = Vec::new();
    let collect = async {
        while received.len() < 4 {
            match frames_rx.recv_async().await {
                Ok(frame) => received.push(frame),
                Err(_) => break,
            }
        }
    };
    let _ = timeout(&*rt, Duration::from_secs(15), collect).await;
    writer.abort();

    assert!(
        !received.is_empty(),
        "no reassembled video frames were received"
    );
    assert!(
        received[0].is_keyframe,
        "the first received frame must be a keyframe"
    );
    // The inter frames that follow must be detected as NON-keyframes, proving the keyframe
    // flag actually discriminates rather than being trivially true for every frame.
    assert!(
        received.iter().any(|f| !f.is_keyframe),
        "inter frames must be reassembled and flagged as non-keyframes"
    );

    // Decode the reassembled frames; the keyframe must reproduce 320x240.
    let mut dec = VpxDecoder::new(VpxCodec::Vp9).expect("vp9 decoder");
    let mut decoded: Vec<I420Frame> = Vec::new();
    for ef in &received {
        decoded.extend(dec.decode(&ef.data).expect("vp9 decode"));
    }
    let first = decoded.first().expect("at least one decoded frame");
    assert_eq!(
        (first.width, first.height),
        (w, h),
        "decoded dimensions must match the published resolution"
    );

    let _ = peer_a.close().await;
    let _ = peer_b.close().await;
}

#[test]
fn loopback_video_vp9_roundtrip_reassembles_and_decodes() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    let rt_for_body = rt.clone();
    rt.block_on(Box::pin(async move { run(rt_for_body).await }));
}
