use std::any::Any;

use crate::impl_thread_safety;

#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {

    extern "C++" {
        include!("mezon_rtc/webrtc.h");
        include!("mezon_rtc/rtp_parameters.h");
        include!("mezon_rtc/helper.h");
        include!("mezon_rtc/media_stream.h");

        type MediaType = crate::webrtc::ffi::MediaType;
        type RtpParameters = crate::rtp_parameters::ffi::RtpParameters;
        type MediaStreamPtr = crate::helper::ffi::MediaStreamPtr;
        type MediaStreamTrack = crate::media_stream::ffi::MediaStreamTrack;
        type MediaStream = crate::media_stream::ffi::MediaStream;
    }

    unsafe extern "C++" {
        include!("mezon_rtc/rtp_receiver.h");

        type RtpReceiver;

        fn track(self: &RtpReceiver) -> SharedPtr<MediaStreamTrack>;
        fn get_stats(
            self: &RtpReceiver,
            ctx: Box<ReceiverContext>,
            on_stats: fn(ctx: Box<ReceiverContext>, json: String),
        );
        fn stream_ids(self: &RtpReceiver) -> Vec<String>;
        fn streams(self: &RtpReceiver) -> Vec<MediaStreamPtr>;
        fn media_type(self: &RtpReceiver) -> MediaType;
        fn id(self: &RtpReceiver) -> String;
        fn get_parameters(self: &RtpReceiver) -> RtpParameters;
        fn set_jitter_buffer_minimum_delay(self: &RtpReceiver, is_some: bool, delay_seconds: f64);

        fn _shared_rtp_receiver() -> SharedPtr<RtpReceiver>;
    }

    extern "Rust" {
        type ReceiverContext;
    }
}

pub struct ReceiverContext(pub Box<dyn Any + Send>);

impl_thread_safety!(ffi::RtpReceiver, Send + Sync);
