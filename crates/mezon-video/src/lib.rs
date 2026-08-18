mod frame_util;

#[cfg(target_os = "macos")]
mod image_scale;
#[cfg(target_os = "linux")]
mod image_scale_linux;
#[cfg(windows)]
mod image_scale_windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod poster;
#[cfg(not(target_os = "macos"))]
mod render_frame;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod unsupported;
#[cfg(windows)]
#[path = "windows.rs"]
mod windows_impl;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
use unsupported as platform;
#[cfg(windows)]
use windows_impl as platform;

#[cfg(target_os = "macos")]
pub use macos::VideoFrame;
#[cfg(not(target_os = "macos"))]
pub use render_frame::VideoFrame;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("inline video playback is not supported on this platform")]
    Unsupported,
    #[error("media url is not valid")]
    InvalidUrl,
    #[error("could not open media for playback")]
    Open,
}

#[derive(Debug, Clone)]
pub struct VideoProbe {
    pub width: u32,
    pub height: u32,
    pub poster_jpeg: Option<Vec<u8>>,
}

pub fn probe_video(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    {
        platform::probe_video(path, max_poster_edge)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (path, max_poster_edge);
        None
    }
}

#[derive(Debug, Clone)]
pub struct ScaledImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[cfg(target_os = "macos")]
pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    image_scale::scaled_image_decode(bytes, max_px)
}

#[cfg(windows)]
pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    image_scale_windows::scaled_image_decode(bytes, max_px)
}

#[cfg(target_os = "linux")]
pub fn scaled_image_decode(bytes: &[u8], max_px: u32) -> Option<ScaledImage> {
    image_scale_linux::scaled_image_decode(bytes, max_px)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn scaled_image_decode(_bytes: &[u8], _max_px: u32) -> Option<ScaledImage> {
    None
}

#[cfg(target_os = "macos")]
pub fn scaled_image_decode_path(path: &std::path::Path, max_px: u32) -> Option<ScaledImage> {
    image_scale::scaled_image_decode_path(path, max_px)
}

#[cfg(windows)]
pub fn scaled_image_decode_path(path: &std::path::Path, max_px: u32) -> Option<ScaledImage> {
    image_scale_windows::scaled_image_decode_path(path, max_px)
}

#[cfg(target_os = "linux")]
pub fn scaled_image_decode_path(path: &std::path::Path, max_px: u32) -> Option<ScaledImage> {
    image_scale_linux::scaled_image_decode_path(path, max_px)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn scaled_image_decode_path(_path: &std::path::Path, _max_px: u32) -> Option<ScaledImage> {
    None
}

pub struct VideoPlayer {
    inner: platform::PlayerImpl,
}

impl VideoPlayer {
    pub fn open(url: &str, max_size: Option<(u32, u32)>) -> Result<Self, PlayerError> {
        let inner = platform::PlayerImpl::open(url, max_size)?;
        Ok(Self { inner })
    }

    pub fn copy_frame(&self) -> Option<VideoFrame> {
        self.inner.copy_frame()
    }

    pub fn play(&self) {
        self.inner.play();
    }

    pub fn pause(&self) {
        self.inner.pause();
    }

    pub fn is_playing(&self) -> bool {
        self.inner.is_playing()
    }

    pub fn current_time(&self) -> f64 {
        self.inner.current_time()
    }

    pub fn duration(&self) -> f64 {
        self.inner.duration()
    }

    pub fn seek(&self, to_seconds: f64) {
        self.inner.seek(to_seconds);
    }

    pub fn set_volume(&self, volume: f32) {
        self.inner.set_volume(volume);
    }

    pub fn volume(&self) -> f32 {
        self.inner.volume()
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.set_muted(muted);
    }

    pub fn is_muted(&self) -> bool {
        self.inner.is_muted()
    }

    pub fn failed(&self) -> bool {
        self.inner.failed()
    }
}
