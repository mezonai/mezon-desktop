//! SFU answer-shaping: feed a synthetic SFU offer into [`RtcSession::apply_remote_offer`]
//! and assert the answer is shaped the way the mezon-sfu expects from a publisher.
//!
//! The synthetic offer is SFU-style: ice-lite, `a=setup:passive`, a recvonly Opus audio
//! m-line (mid 0) and a recvonly video m-line (mid 1) offering VP9 98 / VP8 96 / AV1 100,
//! all with the transport-cc extmap. After publishing audio + video, the session's answer
//! must flip both m-lines to `sendonly`, carry `a=ssrc` (+ a FID group on video), the
//! transport-cc extmap, and `a=setup:active` (we are the DTLS client) — and must NOT contain
//! any duplicated `a=rtcp-fb` line.

use mezon_codec::VpxCodec;
use mezon_rtc::codecs::{PT_OPUS, PT_VP9};
use mezon_rtc::{PeerConnectionOpts, RtcSession};
use webrtc::runtime::default_runtime;

/// A synthetic mezon-sfu-style offer: ice-lite, DTLS-passive, two recvonly m-lines.
const SFU_OFFER: &str = "v=0\r\n\
o=- 4611731400430051336 2 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-lite\r\n\
a=fingerprint:sha-256 9A:71:44:08:15:44:B3:07:B5:7E:63:85:A3:C7:1C:7B:14:D6:84:28:10:B1:E4:CA:A9:21:CA:BE:98:5C:69:5D\r\n\
a=extmap-allow-mixed\r\n\
a=group:BUNDLE 0 1\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
c=IN IP4 0.0.0.0\r\n\
a=setup:passive\r\n\
a=mid:0\r\n\
a=ice-ufrag:sfuUfrag01\r\n\
a=ice-pwd:sfuPasswordsfuPasswordsfuPas\r\n\
a=rtcp-mux\r\n\
a=rtcp-rsize\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=fmtp:111 minptime=10;useinbandfec=1\r\n\
a=rtcp-fb:111 transport-cc\r\n\
a=extmap:1 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
a=recvonly\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 98 96 100\r\n\
c=IN IP4 0.0.0.0\r\n\
a=setup:passive\r\n\
a=mid:1\r\n\
a=ice-ufrag:sfuUfrag01\r\n\
a=ice-pwd:sfuPasswordsfuPasswordsfuPas\r\n\
a=rtcp-mux\r\n\
a=rtcp-rsize\r\n\
a=rtpmap:98 VP9/90000\r\n\
a=fmtp:98 profile-id=0\r\n\
a=rtcp-fb:98 nack\r\n\
a=rtcp-fb:98 nack pli\r\n\
a=rtcp-fb:98 transport-cc\r\n\
a=rtpmap:96 VP8/90000\r\n\
a=rtcp-fb:96 nack\r\n\
a=rtcp-fb:96 nack pli\r\n\
a=rtcp-fb:96 transport-cc\r\n\
a=rtpmap:100 AV1/90000\r\n\
a=fmtp:100 profile-id=0\r\n\
a=rtcp-fb:100 nack\r\n\
a=rtcp-fb:100 nack pli\r\n\
a=rtcp-fb:100 transport-cc\r\n\
a=extmap:1 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
a=recvonly\r\n";

const TRANSPORT_CC_EXTMAP: &str =
    "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01";

/// Split an SDP into (mid, section-lines) for each media section.
fn media_sections(sdp: &str) -> Vec<(String, Vec<&str>)> {
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for line in sdp.split("\r\n") {
        if line.starts_with("m=") {
            sections.push(vec![line]);
        } else if let Some(last) = sections.last_mut() {
            last.push(line);
        }
    }
    sections
        .into_iter()
        .map(|lines| {
            let mid = lines
                .iter()
                .find_map(|l| l.strip_prefix("a=mid:"))
                .unwrap_or("")
                .to_owned();
            (mid, lines)
        })
        .collect()
}

fn section_has(lines: &[&str], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

async fn run() {
    let session = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build session");

    // Publish before answering so the recvonly m-lines pair with our senders as sendonly.
    session
        .publish_audio(0x0A0A_0A0A, PT_OPUS)
        .await
        .expect("publish audio");
    session
        .publish_video(0x0B0B_0B0B, PT_VP9, VpxCodec::Vp9)
        .await
        .expect("publish video");

    let answer = session
        .apply_remote_offer(SFU_OFFER.to_owned())
        .await
        .expect("apply_remote_offer");

    // We are the DTLS client: the answer must be setup:active, never passive/actpass.
    assert!(
        answer.contains("a=setup:active"),
        "answer must be DTLS-active (setup:active):\n{answer}"
    );
    assert!(
        !answer.contains("a=setup:passive") && !answer.contains("a=setup:actpass"),
        "answer must not remain DTLS-passive/actpass:\n{answer}"
    );

    let sections = media_sections(&answer);
    let audio = sections
        .iter()
        .find(|(mid, _)| mid == "0")
        .map(|(_, l)| l.clone())
        .expect("answer has an audio section at mid 0");
    let video = sections
        .iter()
        .find(|(mid, _)| mid == "1")
        .map(|(_, l)| l.clone())
        .expect("answer has a video section at mid 1");

    // Both m-lines flipped from recvonly to sendonly.
    assert!(
        audio.contains(&"a=sendonly"),
        "audio m-line must flip to sendonly:\n{audio:#?}"
    );
    assert!(
        video.contains(&"a=sendonly"),
        "video m-line must flip to sendonly:\n{video:#?}"
    );

    // a=ssrc on both, plus the transport-cc extmap on both.
    assert!(
        section_has(&audio, "a=ssrc:"),
        "audio must carry a=ssrc:\n{audio:#?}"
    );
    assert!(
        section_has(&video, "a=ssrc:"),
        "video must carry a=ssrc:\n{video:#?}"
    );
    assert!(
        section_has(&audio, TRANSPORT_CC_EXTMAP),
        "audio must carry the transport-cc extmap:\n{audio:#?}"
    );
    assert!(
        section_has(&video, TRANSPORT_CC_EXTMAP),
        "video must carry the transport-cc extmap:\n{video:#?}"
    );

    // The FID group is added to the video m-line (RTX pairing the SFU keys off of).
    assert!(
        video.iter().any(|l| l.starts_with("a=ssrc-group:FID")),
        "video must carry an a=ssrc-group:FID:\n{video:#?}"
    );

    // The R5 feedback fix: no a=rtcp-fb line may be duplicated anywhere in the answer.
    let mut fb: Vec<&str> = answer
        .split("\r\n")
        .filter(|l| l.starts_with("a=rtcp-fb:"))
        .collect();
    let total = fb.len();
    fb.sort_unstable();
    fb.dedup();
    assert_eq!(
        fb.len(),
        total,
        "answer contains duplicate a=rtcp-fb lines:\n{answer}"
    );
    // Sanity: the feedback we expect actually made it through (video nack/pli/transport-cc,
    // audio transport-cc only), proving the dedup assertion above is not vacuous.
    assert!(
        fb.iter().any(|l| l.contains("nack") && !l.contains("111")),
        "video must carry nack feedback:\n{answer}"
    );
    assert!(
        fb.contains(&"a=rtcp-fb:111 transport-cc"),
        "audio must carry transport-cc feedback:\n{answer}"
    );
    assert!(
        !fb.iter().any(|l| l.contains("111") && l.contains("nack")),
        "audio (PT 111) must NOT carry nack feedback:\n{answer}"
    );

    let _ = session.close().await;
}

#[test]
fn sfu_offer_yields_publisher_shaped_answer() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    rt.block_on(Box::pin(async move { run().await }));
}
