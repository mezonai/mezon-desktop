use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, http};
use serde::de::DeserializeOwned;

use crate::error::{MmnError, Result};

pub(crate) type Headers = BTreeMap<String, String>;

const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

pub fn is_secure_endpoint(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    #[cfg(debug_assertions)]
    if let Some(rest) = url.strip_prefix("http://") {
        return ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
            rest.strip_prefix(host).is_some_and(|tail| {
                tail.is_empty() || tail.starts_with(':') || tail.starts_with('/')
            })
        });
    }
    false
}

pub(crate) async fn execute(
    http: Arc<dyn HttpClient>,
    method: http::Method,
    url: String,
    headers: Headers,
    body: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let timeout_ms = timeout.as_millis() as u64;

    let join = reqwest_client::runtime().spawn(async move {
        let mut builder = http::Request::builder().method(method).uri(url);
        for (key, value) in &headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let request = builder
            .body(match body {
                Some(bytes) => AsyncBody::from(bytes),
                None => AsyncBody::empty(),
            })
            .map_err(|e| MmnError::Http(e.to_string()))?;

        let outcome = tokio::time::timeout(timeout, async move {
            let response = http
                .send(request)
                .await
                .map_err(|e| MmnError::Http(e.to_string()))?;
            let status = response.status();
            let mut buffer = Vec::new();
            response
                .into_body()
                .take(MAX_RESPONSE_BYTES + 1)
                .read_to_end(&mut buffer)
                .await
                .map_err(|e| MmnError::Http(e.to_string()))?;
            if buffer.len() as u64 > MAX_RESPONSE_BYTES {
                return Err(MmnError::Http(format!(
                    "response exceeds {MAX_RESPONSE_BYTES} bytes"
                )));
            }

            if !status.is_success() {
                let message = String::from_utf8_lossy(&buffer).trim().to_string();
                let message = if message.is_empty() {
                    status.canonical_reason().unwrap_or("").to_string()
                } else {
                    message
                };
                return Err(MmnError::HttpStatus {
                    status: status.as_u16(),
                    message,
                });
            }

            Ok(buffer)
        })
        .await;

        match outcome {
            Ok(result) => result,
            Err(_) => Err(MmnError::Timeout(timeout_ms)),
        }
    });

    join.await
        .map_err(|e| MmnError::Http(format!("request task failed: {e}")))?
}

pub(crate) fn deserialize<R: DeserializeOwned>(bytes: &[u8]) -> Result<R> {
    serde_json::from_slice(bytes).map_err(MmnError::from)
}

pub(crate) fn build_query(url: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{url}?{query}")
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
