mod container;
mod disk;
mod mixer;
mod recorder;
mod sink;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod unsupported;
#[cfg(windows)]
#[path = "windows.rs"]
mod windows_impl;

#[cfg(target_os = "linux")]
pub(crate) use linux as platform;
#[cfg(target_os = "macos")]
pub(crate) use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(crate) use unsupported as platform;
#[cfg(windows)]
pub(crate) use windows_impl as platform;

pub use mixer::AudioSource;
pub use recorder::{AudioTap, RecordStats, Recorder, RecorderConfig, VideoTap};
pub use sink::{
    PixelData, RECORD_CHANNELS, RECORD_SAMPLE_RATE, RecordError, VideoConfig, VideoFrameRef,
};

pub fn is_supported() -> bool {
    platform::is_supported()
}

pub fn file_extension() -> &'static str {
    platform::file_extension()
}
