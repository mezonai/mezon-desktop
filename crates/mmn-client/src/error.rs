use thiserror::Error;

#[derive(Debug, Error)]
pub enum MmnError {
    #[error("http error: {0}")]
    Http(String),

    #[error("HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },

    #[error("request timeout after {0}ms")]
    Timeout(u64),

    #[error("JSON-RPC Error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("empty JSON-RPC result")]
    EmptyResult,

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, MmnError>;
