use crate::{PlayerError, VideoFrame};

pub struct PlayerImpl;

impl PlayerImpl {
    pub fn open(_url: &str, _max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        Err(PlayerError::Unsupported)
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        None
    }

    pub fn play(&self) {}

    pub fn pause(&self) {}

    pub fn is_playing(&self) -> bool {
        false
    }

    pub fn current_time(&self) -> f64 {
        0.0
    }

    pub fn duration(&self) -> f64 {
        0.0
    }

    pub fn seek(&self, _to_seconds: f64) {}

    pub fn set_volume(&self, _volume: f32) {}

    pub fn volume(&self) -> f32 {
        1.0
    }

    pub fn set_muted(&self, _muted: bool) {}

    pub fn is_muted(&self) -> bool {
        false
    }

    pub fn failed(&self) -> bool {
        false
    }

    pub fn set_live_offset(&self, _seconds: f64) {}

    pub fn set_audio_output_device(&self, _uid: Option<&str>) {}
}
