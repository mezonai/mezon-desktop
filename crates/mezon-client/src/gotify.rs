use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(32);
const PING_TIMEOUT: Duration = Duration::from_secs(60);
const JITTER_NUMERATOR: u32 = 4;

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
    pub date: String,
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

/// Why a [`run_once`] session ended. The caller owns retry policy: it decides the backoff, when
/// to stop, and whether the notification token needs re-registering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    /// Opened and later dropped by ping-timeout or read error.
    Dropped,
    /// Never opened — the endpoint was unreachable or the handshake failed at the transport level.
    ConnectFailed,
    /// The handshake was refused with a 4xx, so the notification token is likely dead.
    Rejected,
    /// The server closed the stream cleanly.
    ClosedByServer,
    /// The notification consumer went away; there is nothing left to reconnect for.
    ReceiverGone,
}

/// Open the Gotify stream once and pump it until it ends, replying `pong` to each `ping` and
/// treating a 60s ping gap as a dead connection. Parsed notifications are forwarded to `tx`;
/// suppression is the caller's job.
pub async fn run_once(
    ws_base: &str,
    token: &str,
    tx: &mpsc::UnboundedSender<GotifyNotification>,
) -> StreamEnd {
    let url = format!("{}/stream?token={token}", ws_base.trim_end_matches('/'));
    tracing::info!(target: "noti", host = %ws_base, "gotify: connecting to the notification stream");
    let stream = match tokio_tungstenite::connect_async(&url).await {
        Ok((stream, _)) => stream,
        Err(e) => {
            let rejected = matches!(
                &e,
                tokio_tungstenite::tungstenite::Error::Http(response)
                    if response.status().is_client_error()
            );
            // The error may embed the URL, which carries the auth token — redact it.
            tracing::warn!(
                target: "noti",
                rejected,
                "gotify: connect failed: {}",
                e.to_string().replace(token, "***")
            );
            return if rejected {
                StreamEnd::Rejected
            } else {
                StreamEnd::ConnectFailed
            };
        }
    };
    tracing::info!(target: "noti", "gotify: stream connected — listening for notifications");
    let (mut write, mut read) = stream.split();

    // The 60s watchdog advances only on a received `ping`, mirroring React's
    // `startPingMonitoring`; a lapse means the connection is dead → reconnect.
    let mut deadline = tokio::time::Instant::now() + PING_TIMEOUT;

    loop {
        let frame = tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                tracing::warn!(target: "noti", secs = PING_TIMEOUT.as_secs(), "gotify: no ping within the watchdog window, reconnecting");
                return StreamEnd::Dropped;
            }
            frame = read.next() => match frame {
                None => return StreamEnd::ClosedByServer,
                Some(Err(e)) => {
                    tracing::warn!(target: "noti", "gotify: read error: {e}");
                    return StreamEnd::Dropped;
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
            Message::Close(_) => return StreamEnd::ClosedByServer,
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
                tracing::info!(
                    target: "noti",
                    title = %notification.title,
                    channel_id = %notification.channel_id,
                    sender_id = %notification.sender_id,
                    date = %notification.date,
                    bytes = trimmed.len(),
                    "gotify: frame received from the server"
                );
                if tx.send(notification).is_err() {
                    tracing::warn!(target: "noti", "gotify: nobody is consuming notifications any more");
                    return StreamEnd::ReceiverGone;
                }
            }
            Err(e) => tracing::warn!(
                target: "noti",
                bytes = trimmed.len(),
                "gotify: could not parse a frame the server sent: {e}"
            ),
        }
    }
}

pub fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(BACKOFF_MAX)
}

/// Spread reconnects over ±1/4 of the delay so a server-side outage doesn't bring every client
/// back in lockstep once it recovers.
pub fn with_jitter(delay: Duration) -> Duration {
    let spread = delay / JITTER_NUMERATOR;
    let spread_micros = spread.as_micros() as u64;
    if spread_micros == 0 {
        return delay;
    }
    let offset = rand::random_range(0..=spread_micros * 2);
    (delay - spread).saturating_add(Duration::from_micros(offset))
}

#[cfg(test)]
mod backoff_tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let mut delay = BACKOFF_BASE;
        let mut seen = vec![delay];
        for _ in 0..8 {
            delay = next_backoff(delay);
            seen.push(delay);
        }
        assert_eq!(
            seen,
            vec![1, 2, 4, 8, 16, 32, 32, 32, 32]
                .into_iter()
                .map(Duration::from_secs)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn backoff_never_grows_past_the_cap() {
        let mut delay = BACKOFF_MAX;
        for _ in 0..100 {
            delay = next_backoff(delay);
        }
        assert_eq!(delay, BACKOFF_MAX);
    }

    #[test]
    fn jitter_stays_within_a_quarter_of_the_delay() {
        for _ in 0..1000 {
            let jittered = with_jitter(BACKOFF_MAX);
            assert!(jittered >= BACKOFF_MAX - BACKOFF_MAX / 4);
            assert!(jittered <= BACKOFF_MAX + BACKOFF_MAX / 4);
        }
    }

    #[test]
    fn jitter_keeps_a_sub_millisecond_delay_in_range() {
        let tiny = Duration::from_micros(400);
        for _ in 0..1000 {
            let jittered = with_jitter(tiny);
            assert!(jittered >= tiny - tiny / 4);
            assert!(jittered <= tiny + tiny / 4);
        }
    }

    #[test]
    fn jitter_leaves_a_delay_too_small_to_split_alone() {
        let tiny = Duration::from_nanos(3);
        assert_eq!(with_jitter(tiny), tiny);
    }
}
