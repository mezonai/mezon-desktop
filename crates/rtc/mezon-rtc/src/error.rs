use thiserror::Error;

#[derive(Debug, Error)]
pub enum RtcError {
    #[error("rtc initialization failed: {0}")]
    Init(String),

    #[error("sdp error: {0}")]
    Sdp(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("codec registration failed: {0}")]
    Codec(String),
}
