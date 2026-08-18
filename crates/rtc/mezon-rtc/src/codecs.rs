use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_OPUS, MIME_TYPE_VP8, MIME_TYPE_VP9, MediaEngine,
};
use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability, RtpCodecKind,
};
use rtc::sdp::extmap::TRANSPORT_CC_URI;

use crate::error::RtcError;

pub const PT_OPUS: PayloadType = 111;
pub const PT_VP8: PayloadType = 96;
pub const PT_VP9: PayloadType = 98;
pub const PT_AV1: PayloadType = 100;

pub(crate) fn opus_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_OPUS.to_owned(),
        clock_rate: 48000,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
        rtcp_feedback: Vec::new(),
    }
}

pub(crate) fn vpx_codec(codec: mezon_codec::VpxCodec) -> RTCRtpCodec {
    let (mime_type, sdp_fmtp_line) = match codec {
        mezon_codec::VpxCodec::Vp8 => (MIME_TYPE_VP8.to_owned(), String::new()),
        mezon_codec::VpxCodec::Vp9 => (MIME_TYPE_VP9.to_owned(), "profile-id=0".to_owned()),
    };
    RTCRtpCodec {
        mime_type,
        clock_rate: 90000,
        channels: 0,
        sdp_fmtp_line,
        rtcp_feedback: Vec::new(),
    }
}

pub(crate) fn vpx_payload_type(codec: mezon_codec::VpxCodec) -> PayloadType {
    match codec {
        mezon_codec::VpxCodec::Vp8 => PT_VP8,
        mezon_codec::VpxCodec::Vp9 => PT_VP9,
    }
}

pub(crate) fn video_send_codec_preferences(
    codec: mezon_codec::VpxCodec,
) -> Vec<RTCRtpCodecParameters> {
    vec![RTCRtpCodecParameters {
        rtp_codec: vpx_codec(codec),
        payload_type: vpx_payload_type(codec),
    }]
}

pub fn register_sfu_codecs(me: &mut MediaEngine) -> Result<(), RtcError> {
    me.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: PT_OPUS,
        },
        RtpCodecKind::Audio,
    )
    .map_err(|e| RtcError::Codec(format!("register Opus: {e}")))?;

    me.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_VP8.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: PT_VP8,
        },
        RtpCodecKind::Video,
    )
    .map_err(|e| RtcError::Codec(format!("register VP8: {e}")))?;

    me.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_VP9.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: PT_VP9,
        },
        RtpCodecKind::Video,
    )
    .map_err(|e| RtcError::Codec(format!("register VP9: {e}")))?;

    me.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_AV1.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: PT_AV1,
        },
        RtpCodecKind::Video,
    )
    .map_err(|e| RtcError::Codec(format!("register AV1: {e}")))?;

    me.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: TRANSPORT_CC_URI.to_owned(),
        },
        RtpCodecKind::Video,
        None,
    )
    .map_err(|e| RtcError::Codec(format!("register transport-cc extension (video): {e}")))?;
    me.register_header_extension(
        RTCRtpHeaderExtensionCapability {
            uri: TRANSPORT_CC_URI.to_owned(),
        },
        RtpCodecKind::Audio,
        None,
    )
    .map_err(|e| RtcError::Codec(format!("register transport-cc extension (audio): {e}")))?;

    Ok(())
}
