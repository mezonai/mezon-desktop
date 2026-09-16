use crate::impl_thread_safety;

#[cxx::bridge(namespace = "mezon_ffi")]
pub mod ffi {
    extern "C++" {
        include!("mezon_rtc/helper.h");
        include!("mezon_rtc/media_stream_track.h");
        include!("mezon_rtc/audio_track.h");
        include!("mezon_rtc/video_track.h");

        type MediaStreamTrack = crate::media_stream_track::ffi::MediaStreamTrack;
        type AudioTrack = crate::audio_track::ffi::AudioTrack;
        type VideoTrack = crate::video_track::ffi::VideoTrack;
        type VideoTrackPtr = crate::helper::ffi::VideoTrackPtr;
        type AudioTrackPtr = crate::helper::ffi::AudioTrackPtr;
    }

    unsafe extern "C++" {
        include!("mezon_rtc/media_stream.h");

        type MediaStream;

        fn id(self: &MediaStream) -> String;
        fn get_audio_tracks(self: &MediaStream) -> Vec<AudioTrackPtr>;
        fn get_video_tracks(self: &MediaStream) -> Vec<VideoTrackPtr>;
        fn find_audio_track(self: &MediaStream, track_id: String) -> SharedPtr<AudioTrack>;
        fn find_video_track(self: &MediaStream, track_id: String) -> SharedPtr<VideoTrack>;
        fn add_track(self: &MediaStream, audio_track: SharedPtr<MediaStreamTrack>) -> bool;
        fn remove_track(self: &MediaStream, audio_track: SharedPtr<MediaStreamTrack>) -> bool;

        fn _shared_media_stream() -> SharedPtr<MediaStream>;
    }
}

impl_thread_safety!(ffi::MediaStream, Send + Sync);
