use thiserror::Error;

#[path = "native/mod.rs"]
mod imp;

mod enum_dispatch;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MediaType {
    Audio,
    Video,
    Data,
    Unsupported,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RtcErrorType {
    Internal,
    InvalidSdp,
    InvalidState,
}

#[derive(Error, Debug)]
#[error("an RtcError occurred: {error_type:?} - {message}")]
pub struct RtcError {
    pub error_type: RtcErrorType,
    pub message: String,
}

pub mod audio_frame;
pub mod audio_source;
pub mod audio_stream;
pub mod audio_track;
pub mod data_channel;
pub mod ice_candidate;
pub mod media_stream;
pub mod media_stream_track;
pub mod peer_connection;
pub mod peer_connection_factory;
pub mod prelude;
pub mod rtp_parameters;
pub mod rtp_receiver;
pub mod rtp_sender;
pub mod rtp_transceiver;
pub mod session_description;
pub mod stats;
pub mod video_frame;
pub mod video_source;
pub mod video_stream;
pub mod video_track;

pub mod native {
    pub use webrtc_sys::webrtc::ffi::create_random_uuid;

    pub use crate::imp::{apm, packet_trailer, yuv_helper};
}
