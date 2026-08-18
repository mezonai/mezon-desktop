use std::path::Path;

use crate::sink::{RecordError, RecordSink, VideoConfig};

pub fn is_supported() -> bool {
    false
}
pub fn file_extension() -> &'static str {
    "mp4"
}

pub fn create_sink(
    _path: &Path,
    _video: Option<VideoConfig>,
) -> Result<Box<dyn RecordSink>, RecordError> {
    Err(RecordError::Unsupported)
}
