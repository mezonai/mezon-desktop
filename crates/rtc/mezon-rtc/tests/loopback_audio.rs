//! Loopback audio round-trip: Opus publish + subscribe over webrtc-rs.
//!
//! Peer A publishes real Opus frames (a 440 Hz sine encoded by
//! [`mezon_codec::OpusEncoder`], 20 ms/frame) over a [`mezon_rtc::publish::LocalAudio`]
//! track. Peer B's `on_track` spawns a [`mezon_rtc::subscribe::spawn_opus_receiver`]
//! poll loop, whose emitted Opus packets are decoded by [`mezon_codec::OpusDecoder`].
//! The decoded PCM must carry real energy (RMS > 0.1) once the codecs converge — proof
//! that encoded media actually flowed A -> RTP/SRTP -> B and back out as audio.

use std::sync::Arc;
use std::time::Duration;

use mezon_codec::{AudioFrame, OpusDecoder, OpusEncoder};
use mezon_rtc::codecs::PT_OPUS;
use mezon_rtc::engine::{PeerConnectionOpts, build_peer_connection};
use mezon_rtc::publish::LocalAudio;
use mezon_rtc::subscribe::spawn_opus_receiver;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::peer_connection::{
    PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::{Receiver, Runtime, Sender, channel, default_runtime, timeout};

/// Handler that reports lifecycle over channels and, on receiving a remote track, spins up
/// the Opus receiver and hands its [`flume::Receiver`] back to the test body.
struct AudioHandler {
    gather_tx: Sender<()>,
    state_tx: Sender<RTCPeerConnectionState>,
    opus_tx: flume::Sender<flume::Receiver<Vec<u8>>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for AudioHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_tx.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.try_send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let rx = spawn_opus_receiver(track);
        let _ = self.opus_tx.send(rx);
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

/// One 20 ms frame (960 samples @ 48 kHz mono) of a 440 Hz sine at amplitude 0.5.
fn sine_440_20ms() -> Vec<f32> {
    (0..960)
        .map(|n| (n as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect()
}

async fn run(rt: Arc<dyn Runtime>) {
    let (a_gather_tx, mut a_gather_rx) = channel::<()>(1);
    let (a_state_tx, mut a_state_rx) = channel::<RTCPeerConnectionState>(16);
    let (b_gather_tx, mut b_gather_rx) = channel::<()>(1);
    let (b_state_tx, mut b_state_rx) = channel::<RTCPeerConnectionState>(16);

    // A never receives a track (it is the publisher); give it a dead opus channel.
    let (a_opus_tx, _a_opus_rx) = flume::bounded(1);
    // B hands the Opus receiver built inside `on_track` back to the test body.
    let (b_opus_tx, b_opus_rx) = flume::bounded::<flume::Receiver<Vec<u8>>>(1);

    let peer_a = build_peer_connection(
        Arc::new(AudioHandler {
            gather_tx: a_gather_tx,
            state_tx: a_state_tx,
            opus_tx: a_opus_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer A");

    let peer_b = build_peer_connection(
        Arc::new(AudioHandler {
            gather_tx: b_gather_tx,
            state_tx: b_state_tx,
            opus_tx: b_opus_tx,
        }),
        PeerConnectionOpts { loopback: true },
    )
    .await
    .expect("build peer B");

    // A publishes an Opus track BEFORE creating the offer so it lands in the SDP.
    let ssrc = 0x1234_5678u32;
    let audio = LocalAudio::new(&peer_a, ssrc, PT_OPUS)
        .await
        .expect("A add Opus track");

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

    // A: pump encoded Opus frames of the 440 Hz sine, paced ~10 ms apart. This must start
    // BEFORE waiting on `on_track`: webrtc-rs only opens the remote track (firing
    // `on_track`) when the first RTP packet for it arrives, so media has to flow first.
    let writer = tokio::spawn(async move {
        let mut enc = OpusEncoder::new(1, 32_000).expect("opus encoder");
        let sine = sine_440_20ms();
        for _ in 0..200 {
            let pkt = enc
                .encode(AudioFrame {
                    samples: &sine,
                    channels: 1,
                })
                .expect("opus encode");
            if audio
                .write_encoded(&pkt, Duration::from_millis(20))
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // B's `on_track` fires on the first received RTP packet and hands us the Opus receiver.
    let opus_rx = timeout(&*rt, Duration::from_secs(5), b_opus_rx.recv_async())
        .await
        .expect("on_track fired within timeout")
        .expect("received the Opus receiver");

    // B: drain received Opus packets and decode them back to PCM.
    let mut dec = OpusDecoder::new(1).expect("opus decoder");
    let mut decoded: Vec<Vec<f32>> = Vec::new();
    let collect = async {
        while decoded.len() < 40 {
            match opus_rx.recv_async().await {
                Ok(pkt) => {
                    if let Ok(pcm) = dec.decode(&pkt) {
                        decoded.push(pcm);
                    }
                }
                Err(_) => break,
            }
        }
    };
    let _ = timeout(&*rt, Duration::from_secs(15), collect).await;
    writer.abort();

    assert!(
        decoded.len() >= 10,
        "expected many decoded Opus frames to flow, got {}",
        decoded.len()
    );

    // Opus needs a few frames to converge; measure RMS over the last converged frames.
    let tail: Vec<f32> = decoded
        .iter()
        .rev()
        .take(10)
        .flat_map(|f| f.iter().copied())
        .collect();
    let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(
        rms > 0.1,
        "decoded audio must carry real energy (RMS>0.1), got rms={rms} over {} samples",
        tail.len()
    );

    let _ = peer_a.close().await;
    let _ = peer_b.close().await;
}

#[test]
fn loopback_audio_opus_roundtrip_carries_energy() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    let rt_for_body = rt.clone();
    rt.block_on(Box::pin(async move { run(rt_for_body).await }));
}
