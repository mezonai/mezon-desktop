#[cfg(target_os = "android")]
pub mod android;
pub mod apm;
pub mod audio_device_controller;
pub mod audio_mixer;
pub mod audio_resampler;
pub mod audio_track;
pub mod candidate;
pub mod data_channel;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub mod desktop_capturer;
pub mod frame_cryptor;
pub mod helper;
pub mod jsep;
pub mod media_stream;
pub mod media_stream_track;
pub mod packet_trailer;
pub mod peer_connection;
pub mod peer_connection_factory;
pub mod prohibit_libsrtp_initialization;
pub mod rtc_error;
pub mod rtp_parameters;
pub mod rtp_receiver;
pub mod rtp_sender;
pub mod rtp_transceiver;
pub mod video_frame;
pub mod video_frame_buffer;
pub mod video_track;
pub mod webrtc;
pub mod yuv_helper;

pub const MEDIA_TYPE_VIDEO: &str = "video";
pub const MEDIA_TYPE_AUDIO: &str = "audio";
pub const MEDIA_TYPE_DATA: &str = "data";

macro_rules! impl_thread_safety {
    ($obj:ty, Send) => {
        unsafe impl Send for $obj {}
    };

    ($obj:ty, Send + Sync) => {
        unsafe impl Send for $obj {}
        unsafe impl Sync for $obj {}
    };
}

pub(crate) use impl_thread_safety;
