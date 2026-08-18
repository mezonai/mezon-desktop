use std::sync::Arc;

use rtc::ice::network_type::NetworkType;
use rtc::peer_connection::transport::RTCDtlsRole;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, Registry,
    SettingEngine, configure_nack, configure_twcc,
};

use crate::codecs::register_sfu_codecs;
use crate::error::RtcError;

pub struct PeerConnectionOpts {
    pub loopback: bool,
}

pub async fn build_peer_connection(
    handler: Arc<dyn PeerConnectionEventHandler>,
    opts: PeerConnectionOpts,
) -> Result<Arc<dyn PeerConnection>, RtcError> {
    let mut media_engine = MediaEngine::default();
    register_sfu_codecs(&mut media_engine)?;

    let mut setting_engine = SettingEngine::default();
    setting_engine
        .set_answering_dtls_role(RTCDtlsRole::Client)
        .map_err(|e| RtcError::Init(format!("set_answering_dtls_role: {e}")))?;
    setting_engine.set_network_types(vec![NetworkType::Udp4]);
    if opts.loopback {
        setting_engine.set_include_loopback_candidate(true);
    }

    let registry = Registry::new();
    let registry = configure_nack(registry, &mut media_engine);
    let registry = configure_twcc(registry, &mut media_engine)
        .map_err(|e| RtcError::Codec(format!("configure_twcc: {e}")))?;

    let udp_addr = if opts.loopback {
        "127.0.0.1:0"
    } else {
        "0.0.0.0:0"
    };
    let pc = PeerConnectionBuilder::new()
        .with_media_engine(media_engine)
        .with_setting_engine(setting_engine)
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_udp_addrs(vec![udp_addr.to_string()])
        .build()
        .await
        .map_err(|e| RtcError::Init(format!("build peer connection: {e}")))?;

    Ok(Arc::new(pc) as Arc<dyn PeerConnection>)
}
