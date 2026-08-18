use std::time::Duration;

pub const RECORD_SAMPLE_RATE: u32 = 48_000;
pub const RECORD_CHANNELS: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("call recording is not supported on this platform")]
    Unsupported,
    #[error("could not create the recording file")]
    Create,
    #[error("the encoder rejected a sample")]
    Encode,
    #[error("could not finalize the recording file")]
    Finish,
    #[error("the recording was cut short and the partial file was kept")]
    Incomplete,
    #[error("not enough free disk space to record")]
    DiskSpace,
}

#[derive(Debug, Clone, Copy)]
pub enum PixelData<'a> {
    Bgra {
        data: &'a [u8],
        stride: usize,
    },
    Nv12 {
        y: &'a [u8],
        y_stride: usize,
        uv: &'a [u8],
        uv_stride: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct VideoFrameRef<'a> {
    pub width: u32,
    pub height: u32,
    pub data: PixelData<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

pub trait RecordSink: Send {
    fn push_audio(&mut self, pcm: &[i16], pts: Duration) -> Result<(), RecordError>;
    fn push_video(&mut self, frame: VideoFrameRef<'_>, pts: Duration) -> Result<(), RecordError>;
    fn finish(self: Box<Self>) -> Result<(), RecordError>;
}
