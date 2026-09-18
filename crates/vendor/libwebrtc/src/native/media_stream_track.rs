use cxx::SharedPtr;
use webrtc_sys::{
    audio_track::ffi::media_to_audio, media_stream_track as sys_mst,
    video_track::ffi::media_to_video, MEDIA_TYPE_AUDIO, MEDIA_TYPE_VIDEO,
};

use crate::{
    audio_track,
    imp::{audio_track::RtcAudioTrack, video_track::RtcVideoTrack},
    media_stream_track::{MediaStreamTrack, RtcTrackState},
    video_track,
};

impl From<sys_mst::ffi::TrackState> for RtcTrackState {
    fn from(state: sys_mst::ffi::TrackState) -> Self {
        match state {
            sys_mst::ffi::TrackState::Live => RtcTrackState::Live,
            sys_mst::ffi::TrackState::Ended => RtcTrackState::Ended,
            _ => panic!("unknown TrackState"),
        }
    }
}

pub fn new_media_stream_track(
    sys_handle: SharedPtr<sys_mst::ffi::MediaStreamTrack>,
) -> MediaStreamTrack {
    if sys_handle.kind() == MEDIA_TYPE_AUDIO {
        MediaStreamTrack::Audio(audio_track::RtcAudioTrack {
            handle: RtcAudioTrack { sys_handle: unsafe { media_to_audio(sys_handle) } },
        })
    } else if sys_handle.kind() == MEDIA_TYPE_VIDEO {
        MediaStreamTrack::Video(video_track::RtcVideoTrack {
            handle: RtcVideoTrack::new(unsafe { media_to_video(sys_handle) }),
        })
    } else {
        panic!("unknown track kind")
    }
}

macro_rules! impl_media_stream_track {
    ($cast:expr) => {
        pub fn id(&self) -> String {
            let ptr = $cast(self.sys_handle.clone());
            ptr.id()
        }

        pub fn enabled(&self) -> bool {
            let ptr = $cast(self.sys_handle.clone());
            ptr.enabled()
        }

        pub fn set_enabled(&self, enabled: bool) -> bool {
            let ptr = $cast(self.sys_handle.clone());
            ptr.set_enabled(enabled)
        }

        pub fn state(&self) -> RtcTrackState {
            let ptr = $cast(self.sys_handle.clone());
            ptr.state().into()
        }
    };
}

pub(super) use impl_media_stream_track;
