use std::any::Any;

use crate::impl_thread_safety;

#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    extern "C++" {
        include!("mezon_rtc/webrtc.h");
        include!("mezon_rtc/rtp_parameters.h");
        include!("mezon_rtc/media_stream.h");

        type MediaType = crate::webrtc::ffi::MediaType;
        type VideoEncoderBackend = crate::webrtc::ffi::VideoEncoderBackend;
        type RtpEncodingParameters = crate::rtp_parameters::ffi::RtpEncodingParameters;
        type RtpParameters = crate::rtp_parameters::ffi::RtpParameters;
        type MediaStreamTrack = crate::media_stream::ffi::MediaStreamTrack;
    }

    unsafe extern "C++" {
        include!("mezon_rtc/rtp_sender.h");

        type RtpSender;

        fn set_track(self: &RtpSender, track: SharedPtr<MediaStreamTrack>) -> bool;
        fn track(self: &RtpSender) -> SharedPtr<MediaStreamTrack>;
        fn get_stats(
            self: &RtpSender,
            ctx: Box<SenderContext>,
            on_stats: fn(ctx: Box<SenderContext>, json: String),
        );
        fn ssrc(self: &RtpSender) -> u32;
        fn media_type(self: &RtpSender) -> MediaType;
        fn id(self: &RtpSender) -> String;
        fn stream_ids(self: &RtpSender) -> Vec<String>;
        fn set_streams(self: &RtpSender, stream_ids: &Vec<String>);
        fn init_send_encodings(self: &RtpSender) -> Vec<RtpEncodingParameters>;
        fn get_parameters(self: &RtpSender) -> RtpParameters;
        fn set_parameters(self: &RtpSender, parameters: RtpParameters) -> Result<()>;
        fn set_video_encoder_backend(self: &RtpSender, backend: VideoEncoderBackend);

        fn _shared_rtp_sender() -> SharedPtr<RtpSender>;
    }

    extern "Rust" {
        type SenderContext;
    }
}

#[repr(transparent)]
pub struct SenderContext(pub Box<dyn Any + Send>);

impl_thread_safety!(ffi::RtpSender, Send + Sync);
