//! Loopback negotiation through the [`RtcSession`] façade.
//!
//! Two `RtcSession`s negotiate in-process: an "offerer" publishes an Opus track and drives
//! the initial offer via its raw PeerConnection; the "answerer" consumes it through the
//! mezon-sfu answer-only path ([`RtcSession::apply_remote_offer`]). We assert BOTH sessions
//! emit [`RtcEvent::Connected`] over their event streams, and that the answerer emits
//! [`RtcEvent::TrackAdded`] for the audio the offerer publishes — proof the event stream and
//! the answer-only negotiation actually work end-to-end.

use std::sync::Arc;
use std::time::Duration;

use mezon_codec::{AudioFrame, OpusEncoder};
use mezon_rtc::codecs::PT_OPUS;
use mezon_rtc::{PeerConnectionOpts, RtcEvent, RtcSession};
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use webrtc::runtime::{Runtime, default_runtime, timeout};

/// Drain `rx` until an event satisfies `pred`, or a 20 s timeout.
async fn wait_for_event(
    rx: &flume::Receiver<RtcEvent>,
    rt: &dyn Runtime,
    pred: impl Fn(&RtcEvent) -> bool,
) -> bool {
    let wait = async {
        loop {
            match rx.recv_async().await {
                Ok(event) => {
                    if pred(&event) {
                        return true;
                    }
                }
                Err(_) => return false,
            }
        }
    };
    timeout(rt, Duration::from_secs(20), wait).await.unwrap_or(false)
}

/// One 20 ms frame (960 samples @ 48 kHz mono) of a 440 Hz sine at amplitude 0.5.
fn sine_440_20ms() -> Vec<f32> {
    (0..960)
        .map(|n| (n as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect()
}

async fn run(rt: Arc<dyn Runtime>) {
    let offerer = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build offerer session");
    let answerer = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build answerer session");

    let offerer_events = offerer.events();
    let answerer_events = answerer.events();

    // The offerer publishes an Opus track before offering so it lands in the SDP.
    let ssrc = 0x1111_2222u32;
    let audio = offerer
        .publish_audio(ssrc, PT_OPUS)
        .await
        .expect("offerer publish audio");

    // Offerer drives the initial offer over its raw PeerConnection (answer-only sessions
    // never create offers themselves).
    let pc = offerer.peer_connection();
    let offer = pc.create_offer(None).await.expect("offerer create_offer");
    pc.set_local_description(offer)
        .await
        .expect("offerer set_local(offer)");
    offerer
        .wait_ice_gathering_complete(Duration::from_secs(10))
        .await;
    let offer_sdp = pc
        .local_description()
        .await
        .expect("offerer local_description")
        .sdp;

    // Answerer produces the answer through the mezon-sfu path.
    let answer_sdp = answerer
        .apply_remote_offer(offer_sdp)
        .await
        .expect("answerer apply_remote_offer");

    pc.set_remote_description(
        RTCSessionDescription::answer(answer_sdp).expect("wrap answer sdp"),
    )
    .await
    .expect("offerer set_remote(answer)");

    // Both sessions must reach Connected.
    assert!(
        wait_for_event(&offerer_events, &*rt, |e| *e == RtcEvent::Connected).await,
        "offerer never emitted RtcEvent::Connected"
    );
    assert!(
        wait_for_event(&answerer_events, &*rt, |e| *e == RtcEvent::Connected).await,
        "answerer never emitted RtcEvent::Connected"
    );

    // Pump Opus frames so the answerer opens the remote track (webrtc-rs fires on_track on
    // the first RTP packet). This must run AFTER Connected but the TrackAdded wait below
    // happens concurrently with the pump.
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

    // The answerer must report the forwarded audio track, on its negotiated mid.
    let got_track = wait_for_event(&answerer_events, &*rt, |e| {
        matches!(
            e,
            RtcEvent::TrackAdded {
                kind: RtpCodecKind::Audio,
                mid,
            } if !mid.is_empty()
        )
    })
    .await;
    writer.abort();

    assert!(
        got_track,
        "answerer never emitted RtcEvent::TrackAdded for the forwarded audio track"
    );

    let _ = offerer.close().await;
    let _ = answerer.close().await;
}

#[test]
fn two_sessions_negotiate_and_emit_events() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    let rt_for_body = rt.clone();
    rt.block_on(Box::pin(async move { run(rt_for_body).await }));
}
