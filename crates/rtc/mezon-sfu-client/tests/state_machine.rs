//! Drive the `SfuClient` server-offer state machine over a `MockTransport` against a real
//! `RtcSession`, feeding it a synthetic mezon-sfu offer (the shape from mezon-rtc's
//! `answer_shape` test: ice-lite, `setup:passive`, mid0 audio recvonly PT111, mid1 video
//! recvonly VP9 98 / VP8 96 / AV1 100). No live SFU is required.

use std::time::Duration;

use mezon_codec::VpxCodec;
use mezon_rtc::codecs::{PT_OPUS, PT_VP9};
use mezon_rtc::{PeerConnectionOpts, RtcSession};
use mezon_sfu_client::{
    ClientMessage, MockHandle, MockTransport, ServerMessage, SfuClient, SfuClientEvent, SfuConfig,
};
use webrtc::runtime::{Runtime, default_runtime, timeout};

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

/// Read and parse the next frame the client sent, bounded by a timeout.
async fn next_client_msg(handle: &MockHandle, rt: &dyn Runtime) -> ClientMessage {
    let raw = timeout(rt, Duration::from_secs(15), handle.next_client())
        .await
        .expect("a client frame within the timeout")
        .expect("client transport still open");
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse client frame {raw:?}: {e}"))
}

/// Wait for the first `SfuClientEvent` matching `pred`, bounded by a timeout.
async fn wait_event(
    events: &flume::Receiver<SfuClientEvent>,
    rt: &dyn Runtime,
    pred: impl Fn(&SfuClientEvent) -> bool,
) -> Option<SfuClientEvent> {
    let wait = async {
        loop {
            match events.recv_async().await {
                Ok(ev) if pred(&ev) => return Some(ev),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    };
    timeout(rt, Duration::from_secs(15), wait).await.ok().flatten()
}

async fn run() {
    // A real session that has published audio+video, so the answer flips the recvonly
    // offer m-lines to sendonly.
    let rtc = RtcSession::new(PeerConnectionOpts { loopback: true })
        .await
        .expect("build session");
    rtc.publish_audio(0x0A0A_0A0A, PT_OPUS)
        .await
        .expect("publish audio");
    rtc.publish_video(0x0B0B_0B0B, PT_VP9, VpxCodec::Vp9)
        .await
        .expect("publish video");

    let (transport, handle) = MockTransport::new();
    let config = SfuConfig {
        ws_url: "mock://sfu".to_owned(),
        room: "42".to_owned(),
        role: "speaker".to_owned(),
        token: Some("t".to_owned()),
        user_id: None,
    };
    let client = SfuClient::start(config, transport, rtc);
    let events = client.events();
    let rt = default_runtime().expect("runtime");

    // On start the client sends `join`.
    match next_client_msg(&handle, rt.as_ref()).await {
        ClientMessage::Join { room, role, token, .. } => {
            assert_eq!(room, "42");
            assert_eq!(role, "speaker");
            assert_eq!(token.as_deref(), Some("t"));
        }
        other => panic!("expected join, got {other:?}"),
    }

    // (i) `joined` -> the client emits `Joined`.
    handle.push(r#"{"type":"joined","room":"42"}"#);
    assert!(
        wait_event(&events, rt.as_ref(), |e| matches!(e, SfuClientEvent::Joined))
            .await
            .is_some(),
        "client must emit Joined after `joined`"
    );

    // (ii) `offer` -> the client answers, and the answer is DTLS-active + sendonly.
    handle.push(serde_json::to_string(&ServerMessage::Offer { sdp: SFU_OFFER.to_owned() }).unwrap());
    match next_client_msg(&handle, rt.as_ref()).await {
        ClientMessage::Answer { sdp } => {
            assert!(
                sdp.contains("a=setup:active"),
                "answer must be DTLS-active:\n{sdp}"
            );
            assert!(
                sdp.contains("a=sendonly"),
                "answer must flip published m-lines to sendonly:\n{sdp}"
            );
        }
        other => panic!("expected answer, got {other:?}"),
    }

    // (iii) `ping` -> the client replies `pong`.
    handle.push(r#"{"type":"ping"}"#);
    assert!(
        matches!(next_client_msg(&handle, rt.as_ref()).await, ClientMessage::Pong),
        "client must reply pong to a ping"
    );

    // (iv) `peer_left` -> the client emits `PeerLeft` carrying the user id.
    handle.push(
        r#"{"type":"peer_left","ufrag":"u","user_id":"7","peer_id":"3","mid_audio":"2","mid_video":"3"}"#,
    );
    let left = wait_event(&events, rt.as_ref(), |e| {
        matches!(e, SfuClientEvent::PeerLeft { .. })
    })
    .await
    .expect("client must emit PeerLeft");
    assert_eq!(left, SfuClientEvent::PeerLeft { user_id: "7".to_owned() });

    client.close();
}

#[test]
fn server_offer_flow_answers_pings_and_reports_peers() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    rt.block_on(Box::pin(async move { run().await }));
}
