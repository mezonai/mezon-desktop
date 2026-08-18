use std::io::{Read as _, Write as _};

use anyhow::{Context as _, Result};
use base64::Engine as _;

pub fn compress_sdp(json: &str) -> Result<String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(json.as_bytes())
        .context("gzip write failed")?;
    let bytes = encoder.finish().context("gzip finish failed")?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn decompress_sdp(b64: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .context("base64 decode failed")?;
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut out = String::new();
    decoder
        .read_to_string(&mut out)
        .context("gzip read failed")?;
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OfferPayload {
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
    #[serde(rename = "callerName", default)]
    pub caller_name: String,
    #[serde(rename = "callerAvatar", default)]
    pub caller_avatar: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnswerPayload {
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IcePayload {
    pub candidate: String,
    #[serde(rename = "sdpMid", default)]
    pub sdp_mid: Option<String>,
    #[serde(rename = "sdpMLineIndex", default)]
    pub sdp_mline_index: Option<i32>,
}
