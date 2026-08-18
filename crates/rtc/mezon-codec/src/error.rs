#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("codec init failed: {0}")]
    Init(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
}
