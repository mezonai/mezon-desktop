use std::fmt::Debug;

use crate::{
    imp::rtp_sender as imp_rs, media_stream_track::MediaStreamTrack, rtp_parameters::RtpParameters,
    stats::RtcStats, RtcError,
};

/// Preferred backend for video encoding on an [`RtpSender`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum VideoEncoderBackend {
    /// Use the SDK's default encoder selection.
    #[default]
    Auto,
    /// Prefer a software encoder.
    Software,
    /// Prefer any available hardware encoder.
    Hardware,
    /// Prefer NVIDIA NVENC when available.
    Nvenc,
    /// Prefer VAAPI when available.
    Vaapi,
    /// Prefer VideoToolbox on Apple platforms when available.
    VideoToolbox,
    /// Pass pre-encoded frames through without encoding raw video frames.
    PreEncoded,
}

impl VideoEncoderBackend {
    /// Returns the video encoder backends available in this process.
    ///
    /// The result reflects the current platform, build flags, and runtime
    /// hardware capability checks.
    ///
    /// ```
    /// use libwebrtc::rtp_sender::VideoEncoderBackend;
    ///
    /// let backends: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
    /// assert!(backends.contains(&VideoEncoderBackend::Auto));
    /// assert!(backends.contains(&VideoEncoderBackend::Software));
    /// ```
    pub fn list_available() -> impl IntoIterator<Item = VideoEncoderBackend> {
        imp_rs::video_encoder_backend_list()
    }
}

#[derive(Clone)]
pub struct RtpSender {
    pub(crate) handle: imp_rs::RtpSender,
}

impl RtpSender {
    pub fn track(&self) -> Option<MediaStreamTrack> {
        self.handle.track()
    }

    pub async fn get_stats(&self) -> Result<Vec<RtcStats>, RtcError> {
        self.handle.get_stats().await
    }

    pub fn set_track(&self, track: Option<MediaStreamTrack>) -> Result<(), RtcError> {
        self.handle.set_track(track)
    }

    pub fn parameters(&self) -> RtpParameters {
        self.handle.parameters()
    }

    pub fn set_parameters(&self, parameters: RtpParameters) -> Result<(), RtcError> {
        self.handle.set_parameters(parameters)
    }

    /// Sets the preferred video encoder backend for this sender.
    ///
    /// If the requested backend is unavailable, libwebrtc falls back to another
    /// compatible encoder.
    pub fn set_video_encoder_backend(&self, backend: VideoEncoderBackend) {
        self.handle.set_video_encoder_backend(backend)
    }
}

impl Debug for RtpSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtpReceiver").field("cname", &self.parameters().rtcp.cname).finish()
    }
}
