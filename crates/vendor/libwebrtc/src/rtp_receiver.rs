use std::fmt::Debug;

use crate::{
    imp::rtp_receiver as imp_rr, media_stream::MediaStream, media_stream_track::MediaStreamTrack,
    rtp_parameters::RtpParameters, stats::RtcStats, RtcError,
};

#[derive(Clone)]
pub struct RtpReceiver {
    pub(crate) handle: imp_rr::RtpReceiver,
}

impl RtpReceiver {
    pub fn track(&self) -> Option<MediaStreamTrack> {
        self.handle.track()
    }

    pub fn stream_ids(&self) -> Vec<String> {
        self.handle.stream_ids()
    }

    pub fn streams(&self) -> Vec<MediaStream> {
        self.handle.streams()
    }

    pub async fn get_stats(&self) -> Result<Vec<RtcStats>, RtcError> {
        self.handle.get_stats().await
    }

    pub fn parameters(&self) -> RtpParameters {
        self.handle.parameters()
    }
}

impl Debug for RtpReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtpReceiver")
            .field("track", &self.track())
            .field("cname", &self.parameters().rtcp.cname)
            .finish()
    }
}
