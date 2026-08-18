//! Black-box exercise of the whole [`RtcSession`] public surface over loopback.
//!
//! A publisher session publishes real Opus audio and VP9 video and answers via
//! [`RtcSession::apply_remote_offer`] (its shaped answer carries `a=sendonly` + a FID group);
//! a subscriber session offers recvonly transceivers, accepts that shaped answer, and pulls
//! the forwarded media back out through [`RtcSession::subscribe_audio`] /
//! [`RtcSession::subscribe_video`], decoding it. Asserting the audio carries energy and the
//! video's first frame is a decodable 320x240 keyframe proves publish, subscribe, the
//! answer-only negotiation, and the FID-shaped answer all work through the public API alone.

use std::sync::Arc;
use std::time::Duration;

use mezon_codec::{
    AudioFrame, I420Frame, OpusDecoder, OpusEncoder, VpxCodec, VpxDecoder, VpxEncoder,
};
use mezon_rtc::codecs::{PT_OPUS, PT_VP9};
use mezon_rtc::{PeerConnectionOpts, RtcEvent, RtcSession};
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use webrtc::runtime::{Runtime, default_runtime, timeout};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

async fn wait_connected(rx: &flume::Receiver<RtcEvent>, rt: &dyn Runtime) -> bool {
    let wait = async {
        loop {
            match rx.recv_async().await {
                Ok(RtcEvent::Connected) => return true,
                Ok(RtcEvent::Disconnected) => return false,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    };
    timeout(rt, Duration::from_secs(20), wait).await.unwrap_or(false)
}

fn sine_440_20ms() -> Vec<f32> {
    (0..960)
        .map(|n| (n as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect()
}

fn gradient() -> I420Frame {
    let mut f = I420Frame::new_black(WIDTH, HEIGHT);
    for row in 0..HEIGHT {
        for col in 0..WIDTH {
            f.y[(row * WIDTH + col) as usize] = ((col * 255) / WIDTH) as u8;
        }
    }
    f
}

fn recvonly() -> Option<RTCRtpTransceiverInit> {
    Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly,
        streams: vec![],
        send_encodings: vec![],
    })
}

async fn run(rt: Arc<dyn Runtime>) {
    // Subscriber offers recvonly; publisher answers sendonly (+ FID) via apply_remote_offer.
    let subscriber = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build subscriber");
    let publisher = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build publisher");

    let sub_events = subscriber.events();
    let pub_events = publisher.events();
    let audio_rx = subscriber.subscribe_audio();
    let video_rx = subscriber.subscribe_video();

    // Publisher publishes real tracks before answering.
    let audio = publisher
        .publish_audio(0x0A0A_0A0A, PT_OPUS)
        .await
        .expect("publish audio");
    let video = publisher
        .publish_video(0x0B0B_0B0B, PT_VP9, VpxCodec::Vp9)
        .await
        .expect("publish video");

    // Subscriber offers two recvonly m-lines through its raw PeerConnection.
    let pc = subscriber.peer_connection();
    pc.add_transceiver_from_kind(RtpCodecKind::Audio, recvonly())
        .await
        .expect("add recvonly audio");
    pc.add_transceiver_from_kind(RtpCodecKind::Video, recvonly())
        .await
        .expect("add recvonly video");
    let offer = pc.create_offer(None).await.expect("create_offer");
    pc.set_local_description(offer)
        .await
        .expect("set_local(offer)");
    subscriber
        .wait_ice_gathering_complete(Duration::from_secs(10))
        .await;
    let offer_sdp = pc.local_description().await.expect("local_description").sdp;

    // Publisher answers (shaped: sendonly + FID on video); subscriber accepts the shaped SDP.
    let answer_sdp = publisher
        .apply_remote_offer(offer_sdp)
        .await
        .expect("publisher apply_remote_offer");
    pc.set_remote_description(RTCSessionDescription::answer(answer_sdp).expect("wrap answer"))
        .await
        .expect("subscriber set_remote(answer) with FID-shaped SDP");

    assert!(
        wait_connected(&pub_events, &*rt).await,
        "publisher never reached Connected"
    );
    assert!(
        wait_connected(&sub_events, &*rt).await,
        "subscriber never reached Connected"
    );

    // Pump audio + video from the publisher (media flows only after Connected; on_track fires
    // on the first RTP packet).
    let audio_writer = tokio::spawn(async move {
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
    let video_writer = tokio::spawn(async move {
        let mut enc = VpxEncoder::new(VpxCodec::Vp9, WIDTH, HEIGHT, 400).expect("vp9 encoder");
        let frame = gradient();
        for i in 0..30i64 {
            let encoded = enc.encode(&frame, i == 0, i * 3000).expect("vp9 encode");
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

    // Subscribe to audio and decode; assert real energy.
    let mut dec = OpusDecoder::new(1).expect("opus decoder");
    let mut decoded: Vec<Vec<f32>> = Vec::new();
    let collect_audio = async {
        while decoded.len() < 40 {
            match audio_rx.recv_async().await {
                Ok(pkt) => {
                    if let Ok(pcm) = dec.decode(&pkt.opus) {
                        decoded.push(pcm);
                    }
                }
                Err(_) => break,
            }
        }
    };
    let _ = timeout(&*rt, Duration::from_secs(15), collect_audio).await;

    // Subscribe to video and reassemble; assert a decodable 320x240 keyframe.
    let mut frames: Vec<mezon_codec::EncodedFrame> = Vec::new();
    let collect_video = async {
        while frames.len() < 4 {
            match video_rx.recv_async().await {
                Ok(frame) => frames.push(frame.frame),
                Err(_) => break,
            }
        }
    };
    let _ = timeout(&*rt, Duration::from_secs(15), collect_video).await;

    audio_writer.abort();
    video_writer.abort();

    assert!(
        decoded.len() >= 10,
        "expected decoded Opus frames to flow, got {}",
        decoded.len()
    );
    let tail: Vec<f32> = decoded
        .iter()
        .rev()
        .take(10)
        .flat_map(|f| f.iter().copied())
        .collect();
    let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(rms > 0.1, "subscribed audio must carry energy, rms={rms}");

    assert!(!frames.is_empty(), "no video frames were subscribed");
    assert!(
        frames[0].is_keyframe,
        "first subscribed video frame must be a keyframe"
    );
    let mut vdec = VpxDecoder::new(VpxCodec::Vp9).expect("vp9 decoder");
    let first = vdec
        .decode(&frames[0].data)
        .expect("vp9 decode")
        .into_iter()
        .next()
        .expect("at least one decoded frame");
    assert_eq!(
        (first.width, first.height),
        (WIDTH, HEIGHT),
        "decoded video dimensions must match the published resolution"
    );

    let _ = publisher.close().await;
    let _ = subscriber.close().await;
}

#[test]
fn publish_and_subscribe_over_loopback() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    let rt_for_body = rt.clone();
    rt.block_on(Box::pin(async move { run(rt_for_body).await }));
}
