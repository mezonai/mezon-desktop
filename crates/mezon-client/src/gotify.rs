use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const BACKOFF_BASE: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(32);
const MAX_RECONNECT_ATTEMPTS: u32 = 10;
const PING_TIMEOUT: Duration = Duration::from_secs(60);

/// Server-rendered notification payload from the Gotify `/stream` endpoint,
/// mirroring the React `NotificationData` shape. Snowflake ids arrive on the wire
/// as JSON integers, so id fields are decoded from either a number or a string.
#[derive(Debug, Clone, Deserialize)]
pub struct GotifyNotification {
    #[serde(default, deserialize_with = "de_id")]
    pub channel_id: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub image: String,
    #[serde(default, deserialize_with = "de_id")]
    pub sender_id: String,
    #[serde(default)]
    pub extras: GotifyExtras,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GotifyExtras {
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub e2eemess: String,
    #[serde(default, rename = "topicId", deserialize_with = "de_id")]
    pub topic_id: String,
    #[serde(default, rename = "messageId", deserialize_with = "de_id")]
    pub message_id: String,
}

/// Decode a snowflake id that may arrive as a JSON number, string, or null.
fn de_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Null => Ok(String::new()),
        other => Ok(other.to_string()),
    }
}

/// Run the Gotify notification stream until `tx` is closed. Reconnects with
/// exponential backoff (1s→32s, ×2, cap 10 attempts, reset on a clean open) and
/// replies `pong` to each `ping`, treating a 60s ping gap as a dead connection.
/// Parsed notifications are forwarded to `tx`; suppression is the caller's job.
pub async fn run_stream(
    ws_base: String,
    token: String,
    tx: mpsc::UnboundedSender<GotifyNotification>,
) {
    let url = format!("{}/stream?token={token}", ws_base.trim_end_matches('/'));
    let mut attempts: u32 = 0;
    let mut backoff = BACKOFF_BASE;

    loop {
        if tx.is_closed() {
            return;
        }
        match connect_once(&url, &token, &tx).await {
            ConnectOutcome::StopClean => return,
            ConnectOutcome::Opened => {
                attempts = 0;
                backoff = BACKOFF_BASE;
            }
            ConnectOutcome::FailedBeforeOpen => {}
        }

        attempts += 1;
        if attempts >= MAX_RECONNECT_ATTEMPTS {
            tracing::warn!("gotify: giving up after {MAX_RECONNECT_ATTEMPTS} reconnect attempts");
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

enum ConnectOutcome {
    /// Opened and later dropped by ping-timeout or read error — reconnect.
    Opened,
    /// Never opened (connect error) — reconnect after backoff.
    FailedBeforeOpen,
    /// Clean close (server Close frame / receiver dropped) — do not reconnect,
    /// matching React's `onclose` which only marks the socket inactive.
    StopClean,
}

async fn connect_once(
    url: &str,
    token: &str,
    tx: &mpsc::UnboundedSender<GotifyNotification>,
) -> ConnectOutcome {
    let stream = match tokio_tungstenite::connect_async(url).await {
        Ok((stream, _)) => stream,
        Err(e) => {
            // The error may embed the URL, which carries the auth token — redact it.
            tracing::warn!(
                "gotify: connect failed: {}",
                e.to_string().replace(token, "***")
            );
            return ConnectOutcome::FailedBeforeOpen;
        }
    };
    tracing::debug!("gotify: stream connected");
    let (mut write, mut read) = stream.split();

    // The 60s watchdog advances only on a received `ping`, mirroring React's
    // `startPingMonitoring`; a lapse means the connection is dead → reconnect.
    let mut deadline = tokio::time::Instant::now() + PING_TIMEOUT;

    loop {
        let frame = tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                tracing::debug!("gotify: ping timeout, reconnecting");
                return ConnectOutcome::Opened;
            }
            frame = read.next() => match frame {
                None => return ConnectOutcome::StopClean,
                Some(Err(e)) => {
                    tracing::warn!("gotify: read error: {e}");
                    return ConnectOutcome::Opened;
                }
                Some(Ok(frame)) => frame,
            },
        };

        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                Ok(t) => t,
                Err(_) => continue,
            },
            Message::Ping(p) => {
                let _ = write.send(Message::Pong(p)).await;
                continue;
            }
            Message::Close(_) => return ConnectOutcome::StopClean,
            _ => continue,
        };

        let trimmed = text.trim();
        if trimmed == "ping" || trimmed == "\"ping\"" {
            let _ = write.send(Message::Text("pong".into())).await;
            deadline = tokio::time::Instant::now() + PING_TIMEOUT;
            continue;
        }

        match serde_json::from_str::<GotifyNotification>(trimmed) {
            Ok(notification) => {
                if tx.send(notification).is_err() {
                    return ConnectOutcome::StopClean;
                }
            }
            Err(e) => tracing::debug!("gotify: unparseable frame: {e}"),
        }
    }
}
