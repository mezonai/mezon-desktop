use async_trait::async_trait;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::error::SfuClientError;

#[async_trait]
pub trait SfuTransport: Send {
    async fn send(&mut self, text: String) -> Result<(), SfuClientError>;
    async fn recv(&mut self) -> Option<Result<String, SfuClientError>>;
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct TungsteniteTransport {
    sink: SplitSink<Ws, Message>,
    stream: SplitStream<Ws>,
}

impl TungsteniteTransport {
    pub async fn connect(ws_url: &str) -> Result<Self, SfuClientError> {
        tracing::info!(ws_url, "mezon-sfu: connecting WebSocket");
        let (ws, _resp) = connect_async(ws_url).await.map_err(|e| {
            tracing::error!(ws_url, error = %e, "mezon-sfu: WebSocket connect FAILED");
            SfuClientError::Connect(e.to_string())
        })?;
        tracing::info!(ws_url, "mezon-sfu: WebSocket connected");
        let (sink, stream) = ws.split();
        Ok(Self { sink, stream })
    }
}

#[async_trait]
impl SfuTransport for TungsteniteTransport {
    async fn send(&mut self, text: String) -> Result<(), SfuClientError> {
        self.sink
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| SfuClientError::Connect(e.to_string()))
    }

    async fn recv(&mut self) -> Option<Result<String, SfuClientError>> {
        while let Some(frame) = self.stream.next().await {
            match frame {
                Ok(Message::Text(text)) => return Some(Ok(text.as_str().to_owned())),
                Ok(
                    Message::Binary(_)
                    | Message::Ping(_)
                    | Message::Pong(_)
                    | Message::Frame(_),
                ) => continue,
                Ok(Message::Close(_)) => return None,
                Err(e) => return Some(Err(SfuClientError::Connect(e.to_string()))),
            }
        }
        None
    }
}

pub struct MockTransport {
    outgoing: flume::Sender<String>,
    incoming: flume::Receiver<String>,
}

pub struct MockHandle {
    sent_by_client: flume::Receiver<String>,
    to_client: flume::Sender<String>,
}

impl MockTransport {
    pub fn new() -> (MockTransport, MockHandle) {
        let (out_tx, out_rx) = flume::unbounded();
        let (in_tx, in_rx) = flume::unbounded();
        (
            MockTransport {
                outgoing: out_tx,
                incoming: in_rx,
            },
            MockHandle {
                sent_by_client: out_rx,
                to_client: in_tx,
            },
        )
    }
}

#[async_trait]
impl SfuTransport for MockTransport {
    async fn send(&mut self, text: String) -> Result<(), SfuClientError> {
        self.outgoing.send(text).map_err(|_| SfuClientError::Closed)
    }

    async fn recv(&mut self) -> Option<Result<String, SfuClientError>> {
        self.incoming.recv_async().await.ok().map(Ok)
    }
}

impl MockHandle {
    pub fn push(&self, frame: impl Into<String>) {
        let _ = self.to_client.send(frame.into());
    }

    pub async fn next_client(&self) -> Option<String> {
        self.sent_by_client.recv_async().await.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{MockTransport, SfuTransport};

    #[tokio::test]
    async fn mock_transport_roundtrips_both_directions() {
        let (mut transport, handle) = MockTransport::new();

        transport
            .send(r#"{"type":"ping"}"#.to_owned())
            .await
            .expect("mock send");
        assert_eq!(
            handle.next_client().await.as_deref(),
            Some(r#"{"type":"ping"}"#)
        );

        handle.push(r#"{"type":"pong"}"#);
        let got = transport.recv().await.expect("a frame").expect("no error");
        assert_eq!(got, r#"{"type":"pong"}"#);
    }
}
