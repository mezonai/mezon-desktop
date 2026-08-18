use thiserror::Error;

#[derive(Debug, Error)]
pub enum SfuClientError {
    #[error("sfu connect failed: {0}")]
    Connect(String),

    #[error("sfu protocol error: {0}")]
    Protocol(String),

    #[error(transparent)]
    Rtc(#[from] mezon_rtc::RtcError),

    #[error("sfu client closed")]
    Closed,
}
