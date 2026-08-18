use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::peer_connection::PeerConnection;

use crate::codecs::{opus_codec, vpx_codec};
use crate::error::RtcError;

async fn add_sample_track(
    pc: &Arc<dyn PeerConnection>,
    ssrc: u32,
    kind: RtpCodecKind,
    codec: RTCRtpCodec,
    stream_id: &str,
    track_id: String,
    label: &str,
) -> Result<Arc<TrackLocalStaticSample>, RtcError> {
    let track = MediaStreamTrack::new(
        stream_id.to_owned(),
        track_id,
        label.to_owned(),
        kind,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            active: true,
            codec,
            ..Default::default()
        }],
    );
    let local = Arc::new(
        TrackLocalStaticSample::new(track)
            .map_err(|e| RtcError::Codec(format!("build sample track: {e}")))?,
    );
    pc.add_track(local.clone() as Arc<dyn TrackLocal>)
        .await
        .map_err(|e| RtcError::Transport(format!("add_track: {e}")))?;
    Ok(local)
}

pub struct LocalAudio {
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    payload_type: PayloadType,
}

impl LocalAudio {
    pub async fn new(
        pc: &Arc<dyn PeerConnection>,
        ssrc: u32,
        payload_type: PayloadType,
    ) -> Result<Self, RtcError> {
        let track = add_sample_track(
            pc,
            ssrc,
            RtpCodecKind::Audio,
            opus_codec(),
            "mezon-audio",
            format!("audio-{ssrc}"),
            "mezon-audio",
        )
        .await?;
        Ok(Self {
            track,
            ssrc,
            payload_type,
        })
    }

    pub fn track_local(&self) -> Arc<dyn TrackLocal> {
        self.track.clone() as Arc<dyn TrackLocal>
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub async fn write_encoded(
        &self,
        opus_packet: &[u8],
        duration: Duration,
    ) -> Result<(), RtcError> {
        let sample = Sample {
            data: Bytes::copy_from_slice(opus_packet),
            duration,
            ..Default::default()
        };
        self.track
            .write_sample(self.ssrc, self.payload_type, &sample, &[])
            .await
            .map_err(|e| RtcError::Transport(format!("write Opus sample: {e}")))
    }
}

pub struct LocalVideo {
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    payload_type: PayloadType,
    codec: mezon_codec::VpxCodec,
}

impl LocalVideo {
    pub async fn new(
        pc: &Arc<dyn PeerConnection>,
        ssrc: u32,
        payload_type: PayloadType,
        codec: mezon_codec::VpxCodec,
    ) -> Result<Self, RtcError> {
        let track = add_sample_track(
            pc,
            ssrc,
            RtpCodecKind::Video,
            vpx_codec(codec),
            "mezon-video",
            format!("video-{ssrc}"),
            "mezon-video",
        )
        .await?;
        Ok(Self {
            track,
            ssrc,
            payload_type,
            codec,
        })
    }

    pub fn track_local(&self) -> Arc<dyn TrackLocal> {
        self.track.clone() as Arc<dyn TrackLocal>
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub fn codec(&self) -> mezon_codec::VpxCodec {
        self.codec
    }

    pub async fn write_encoded(
        &self,
        frame: &mezon_codec::EncodedFrame,
        duration: Duration,
    ) -> Result<(), RtcError> {
        let sample = Sample {
            data: Bytes::copy_from_slice(&frame.data),
            duration,
            ..Default::default()
        };
        self.track
            .write_sample(self.ssrc, self.payload_type, &sample, &[])
            .await
            .map_err(|e| RtcError::Transport(format!("write VPx sample: {e}")))
    }
}
