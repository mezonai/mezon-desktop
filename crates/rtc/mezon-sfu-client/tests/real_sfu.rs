//! Real end-to-end validation against a running mezon-sfu.
//!
//! Ignored by default: it requires a running mezon-sfu (Docker) + a valid token. It reads
//! `MEZON_SFU_WS`, `MEZON_SFU_ROOM`, and `MEZON_SFU_TOKEN` from the environment, connects via
//! the real [`TungsteniteTransport`], joins as a speaker, and asserts the session reaches
//! [`SfuClientEvent::Connected`] within a timeout — proving ICE/DTLS interop end to end.
//!
//! Run with:
//! `MEZON_SFU_WS=ws://127.0.0.1:8000 MEZON_SFU_ROOM=1 MEZON_SFU_TOKEN=<jwt> \`
//! `cargo test -p mezon-sfu-client --test real_sfu -- --ignored`.

use std::time::Duration;

use mezon_rtc::codecs::PT_OPUS;
use mezon_rtc::{PeerConnectionOpts, RtcSession};
use mezon_sfu_client::{SfuClient, SfuClientEvent, SfuConfig, TungsteniteTransport};
use webrtc::runtime::{default_runtime, timeout};

async fn run() {
    let ws_url = std::env::var("MEZON_SFU_WS").expect("set MEZON_SFU_WS to the SFU ws URL");
    let room = std::env::var("MEZON_SFU_ROOM").expect("set MEZON_SFU_ROOM");
    let token = std::env::var("MEZON_SFU_TOKEN").expect("set MEZON_SFU_TOKEN");

    // A real (non-loopback) session that publishes audio, so the SFU's recvonly m-lines pair
    // with our sender on the speaker path.
    let rtc = RtcSession::new(PeerConnectionOpts { loopback: false })
        .await
        .expect("build session");
    rtc.publish_audio(0x0A0A_0A0A, PT_OPUS)
        .await
        .expect("publish audio");

    let transport = TungsteniteTransport::connect(&ws_url)
        .await
        .expect("connect to the sfu");
    let config = SfuConfig {
        ws_url,
        room,
        role: "speaker".to_owned(),
        token: Some(token),
        user_id: None,
    };
    let client = SfuClient::start(config, transport, rtc);
    let events = client.events();
    let rt = default_runtime().expect("runtime");

    let wait = async {
        loop {
            match events.recv_async().await {
                Ok(SfuClientEvent::Connected) => return true,
                Ok(SfuClientEvent::Disconnected) => return false,
                Ok(SfuClientEvent::Error(e)) => panic!("sfu signaling error: {e}"),
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    };
    let connected = timeout(rt.as_ref(), Duration::from_secs(20), wait)
        .await
        .unwrap_or(false);
    assert!(
        connected,
        "the session must reach Connected against the real SFU"
    );

    client.close();
}

#[test]
#[ignore = "requires a running mezon-sfu (Docker) + a valid token; run with --ignored"]
fn reaches_connected_against_real_sfu() {
    let rt = default_runtime().expect("a webrtc runtime (runtime-tokio) must be enabled");
    rt.block_on(Box::pin(async move { run().await }));
}
