pub mod engine;

use std::{error::Error, sync::mpsc, time::Duration};

use anyhow::anyhow;

use engine::{ChannelItem, ChannelSender};

use crate::{
    frame::{Frame, FrameType},
    has_permission, is_supported,
    targets::Target,
};

pub use engine::get_output_frame_size;

#[derive(Debug, Clone, Copy, Default)]
pub enum Resolution {
    _480p,
    _720p,
    _1080p,
    _1440p,
    _2160p,
    _4320p,

    #[default]
    Captured,
}

impl Resolution {
    fn value(&self, aspect_ratio: f32) -> [u32; 2] {
        match *self {
            Resolution::_480p => [640, (640_f32 / aspect_ratio).floor() as u32],
            Resolution::_720p => [1280, (1280_f32 / aspect_ratio).floor() as u32],
            Resolution::_1080p => [1920, (1920_f32 / aspect_ratio).floor() as u32],
            Resolution::_1440p => [2560, (2560_f32 / aspect_ratio).floor() as u32],
            Resolution::_2160p => [3840, (3840_f32 / aspect_ratio).floor() as u32],
            Resolution::_4320p => [7680, (7680_f32 / aspect_ratio).floor() as u32],
            Resolution::Captured => {
                panic!(".value should not be called when Resolution type is Captured")
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Default, Clone)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}
#[derive(Debug, Default, Clone)]
pub struct Area {
    pub origin: Point,
    pub size: Size,
}

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub fps: u32,
    pub show_cursor: bool,
    pub show_highlight: bool,
    pub target: Option<Target>,
    pub crop_area: Option<Area>,
    pub output_type: FrameType,
    pub output_resolution: Resolution,
    pub excluded_targets: Option<Vec<Target>>,
    pub portal_source_types: Option<u32>,
    pub use_portal: bool,
}

pub struct Capturer {
    engine: engine::Engine,
    rx: mpsc::Receiver<anyhow::Result<ChannelItem>>,
}

#[derive(Debug)]
pub enum CapturerBuildError {
    NotSupported,
    PermissionNotGranted,
}

impl std::fmt::Display for CapturerBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapturerBuildError::NotSupported => write!(f, "Screen capturing is not supported"),
            CapturerBuildError::PermissionNotGranted => {
                write!(f, "Permission to capture the screen is not granted")
            }
        }
    }
}

impl Error for CapturerBuildError {}

impl Capturer {
    #[deprecated(
        since = "0.0.6",
        note = "Use `build` instead of `new` to create a new capturer instance."
    )]
    pub fn new(options: Options) -> anyhow::Result<Capturer> {
        let (tx, rx) = capture_channel();
        let engine = engine::Engine::new(&options, tx)?;

        Ok(Capturer { engine, rx })
    }

    pub fn build(options: Options) -> anyhow::Result<Capturer> {
        if !is_supported() {
            return Err(anyhow!(CapturerBuildError::NotSupported));
        }

        if !has_permission() {
            return Err(anyhow!(CapturerBuildError::PermissionNotGranted));
        }

        let (tx, rx) = capture_channel();
        let engine = engine::Engine::new(&options, tx)?;

        Ok(Capturer { engine, rx })
    }

    pub fn start_capture(&mut self) {
        self.engine.start();
    }

    pub fn stop_capture(&mut self) {
        self.engine.stop();
    }

    pub fn get_next_frame(&self) -> anyhow::Result<Frame> {
        loop {
            let res = self.rx.recv()??;

            if let Some(frame) = self.engine.process_channel_item(res) {
                return Ok(frame);
            }
        }
    }

    pub fn get_next_frame_timeout(&self, timeout: Duration) -> anyhow::Result<Option<Frame>> {
        loop {
            let res = match self.rx.recv_timeout(timeout) {
                Ok(res) => res?,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("Screen capture frame channel disconnected"));
                }
            };

            if let Some(frame) = self.engine.process_channel_item(res) {
                return Ok(Some(frame));
            }
        }
    }

    pub fn get_output_frame_size(&mut self) -> [u32; 2] {
        self.engine.get_output_frame_size()
    }

    pub fn raw(&self) -> RawCapturer<'_> {
        RawCapturer { capturer: self }
    }

    pub fn target(&self) -> Option<&Target> {
        self.engine.target()
    }
}

#[cfg(target_os = "macos")]
fn capture_channel() -> (
    ChannelSender,
    mpsc::Receiver<anyhow::Result<ChannelItem>>,
) {
    mpsc::sync_channel(2)
}

#[cfg(not(target_os = "macos"))]
fn capture_channel() -> (
    ChannelSender,
    mpsc::Receiver<anyhow::Result<ChannelItem>>,
) {
    mpsc::channel()
}

pub struct RawCapturer<'a> {
    capturer: &'a Capturer,
}
