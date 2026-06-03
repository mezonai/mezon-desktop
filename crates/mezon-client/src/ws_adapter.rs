use crate::transport_adapter::{AdapterHandlers, TransportAdapter};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{SplitSink, SplitStream, StreamExt};
use futures::SinkExt;
use mezon_proto::realtime;
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const PREFIX_RAW: u8 = 0xff;
const CODE_FIN: u16 = 0xff;
const RAW_HEADER_LENGTH: usize = 7;
const RAW_CHUNK_HEADER_LENGTH: usize = 11;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsWriteHalf = SplitSink<WsStream, WsMessage>;
type WsReadHalf = SplitStream<WsStream>;

struct IoLoopState {
    handlers: Arc<Mutex<AdapterHandlers>>,
    streams: Arc<Mutex<HashMap<u16, Vec<Vec<u8>>>>>,
    is_connected: Arc<Mutex<bool>>,
}

pub struct WsAdapter {
    write_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    handlers: Arc<Mutex<AdapterHandlers>>,
    streams: Arc<Mutex<HashMap<u16, Vec<Vec<u8>>>>>,
    is_connected: Arc<Mutex<bool>>,
}

impl WsAdapter {
    pub fn new() -> Self {
        Self {
            write_tx: Arc::new(Mutex::new(None)),
            handlers: Arc::new(Mutex::new(AdapterHandlers::default())),
            streams: Arc::new(Mutex::new(HashMap::new())),
            is_connected: Arc::new(Mutex::new(false)),
        }
    }

    fn build_url(host: &str, port: u16, token: &str) -> String {
        format!(
            "wss://{host}:{port}/ws?lang=en&status=true&token={token}&format=protobuf"
        )
    }

    async fn handle_message(&self, data: Vec<u8>) {
        if data.is_empty() {
            tracing::warn!("📥 ws_adapter: empty frame, skipping");
            return;
        }

        let first_byte = data[0];
        tracing::trace!(
            "📥 ws_adapter: msg {} bytes, first_byte={:#04x}",
            data.len(),
            first_byte
        );

        if first_byte == 0x00 && data.len() >= 3 {
            let cid = u16::from_be_bytes([data[1], data[2]]);
            tracing::trace!("📨 PONG: cid={}", cid);
            let handlers = self.handlers.lock().await;
            handlers.trigger_message(cid, 0, vec![]);
            return;
        }

        if first_byte == PREFIX_RAW {
            self.handle_raw_response(data).await;
            return;
        }

        let handlers = self.handlers.lock().await;
        if let Ok(envelope) = realtime::Envelope::decode(data.as_slice()) {
            tracing::trace!("📨 Envelope decoded: cid={}", envelope.cid);
            handlers.trigger_message(envelope.cid as u16, 0, data);
        } else {
            let cid = decode_cid_field(&data).unwrap_or(0);
            tracing::warn!("📨 Failed to decode Envelope, passing raw cid={cid}");
            handlers.trigger_message(cid, 0, data);
        }
    }

    async fn handle_raw_response(&self, data: Vec<u8>) {
        if data.len() < RAW_HEADER_LENGTH {
            tracing::warn!("📥 ws_adapter: short RAW header ({})", data.len());
            return;
        }

        let cid = u16::from_be_bytes([data[1], data[2]]);
        let code = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
        let response_code = (code >> 16) & 0xffff;
        let fin_flag = (code & 0xffff) as u16;

        tracing::debug!(
            "📥 RAW: cid={} code={:#x} response_code={} fin_flag={:#x} payload_len={}",
            cid,
            code,
            response_code,
            fin_flag,
            data.len().saturating_sub(RAW_HEADER_LENGTH),
        );

        if fin_flag == CODE_FIN {
            let payload = data[RAW_HEADER_LENGTH..].to_vec();
            let mut streams = self.streams.lock().await;
            if !payload.is_empty() {
                streams.entry(cid).or_default().push(payload);
            }
            let complete = streams.remove(&cid).unwrap_or_default().concat();

            tracing::debug!(
                "📨 Complete API response: cid={} code={} len={} bytes",
                cid,
                response_code,
                complete.len(),
            );

            let handlers = self.handlers.lock().await;
            handlers.trigger_message(cid, response_code, complete);
        } else {
            if data.len() < RAW_CHUNK_HEADER_LENGTH {
                tracing::warn!("📥 ws_adapter: short RAW chunk header ({})", data.len());
                return;
            }
            let payload_len =
                u32::from_be_bytes([data[7], data[8], data[9], data[10]]) as usize;
            let total = RAW_CHUNK_HEADER_LENGTH + payload_len;
            if data.len() < total {
                tracing::warn!("📥 ws_adapter: short RAW chunk body ({})", data.len());
                return;
            }
            let payload = data[RAW_CHUNK_HEADER_LENGTH..total].to_vec();
            let mut streams = self.streams.lock().await;
            let chunks = streams.entry(cid).or_default();
            chunks.push(payload);
            tracing::trace!("📥 Buffered chunk for cid={} ({} total)", cid, chunks.len());
        }
    }

    async fn io_loop(
        mut write_sink: WsWriteHalf,
        mut read_stream: WsReadHalf,
        mut write_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        ready_tx: oneshot::Sender<()>,
        state: IoLoopState,
    ) {
        tracing::info!("🔄 I/O loop running, entering select branch");
        let _ = ready_tx.send(());

        loop {
            tokio::select! {
                msg = read_stream.next() => {
                    match msg {
                        Some(Ok(WsMessage::Binary(data))) => {
                            tracing::trace!(
                                "📖 Binary frame: {} bytes {:02x?}",
                                data.len(),
                                &data[..data.len().min(128)],
                            );
                            let adapter = WsAdapter {
                                write_tx: Arc::new(Mutex::new(None)),
                                handlers: state.handlers.clone(),
                                streams: state.streams.clone(),
                                is_connected: state.is_connected.clone(),
                            };
                            adapter.handle_message(data).await;
                        }
                        Some(Ok(WsMessage::Text(_))) => {
                            tracing::info!("📖 ws_adapter: ignoring text frame");
                        }
                        Some(Ok(WsMessage::Ping(_))) => {
                            tracing::trace!("📖 ws_adapter: protocol ping (handled by tungstenite)");
                        }
                        Some(Ok(WsMessage::Pong(_))) => {
                            tracing::trace!("📖 ws_adapter: protocol pong");
                        }
                        Some(Ok(WsMessage::Frame(_))) => {}
                        Some(Ok(WsMessage::Close(frame))) => {
                            let code = frame.as_ref().map(|f| f.code);
                            let reason: &str = frame.as_ref().map_or("", |f| f.reason.as_ref());
                            tracing::info!(
                                "📖 Server closed WS: code={:?} reason={}",
                                code,
                                reason,
                            );
                            *state.is_connected.lock().await = false;
                            state.handlers.lock().await.trigger_close(true);
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::error!("📖 ws_adapter read error: {}", e);
                            *state.is_connected.lock().await = false;
                            let err = e.to_string();
                            state.handlers.lock().await.trigger_error(err);
                            state.handlers.lock().await.trigger_close(false);
                            break;
                        }
                        None => {
                            tracing::info!("📖 WS read stream ended (EOF)");
                            *state.is_connected.lock().await = false;
                            state.handlers.lock().await.trigger_close(true);
                            break;
                        }
                    }
                }
                msg = write_rx.recv() => {
                    match msg {
                        Some(data) => {
                            tracing::trace!(
                                "📤 WRITE: {} bytes {:02x?}",
                                data.len(),
                                &data[..data.len().min(64)],
                            );
                            if let Err(e) = write_sink.send(WsMessage::Binary(data)).await {
                                tracing::error!("📤 ws_adapter write error: {}", e);
                                *state.is_connected.lock().await = false;
                                let err = e.to_string();
                                state.handlers.lock().await.trigger_error(err);
                                break;
                            }
                        }
                        None => {
                            tracing::info!("📤 Write channel closed, exiting I/O loop");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("🔄 I/O loop exited");
    }
}

impl Default for WsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TransportAdapter for WsAdapter {
    async fn connect(&mut self, host: &str, port: u16, token: &str) -> Result<()> {
        tracing::info!("🔌 === WsAdapter CONNECT START: {}:{} ===", host, port);
        tracing::info!("🔌 Token length: {}", token.len());

        crate::transport_runtime::ensure_crypto_provider();

        let url = Self::build_url(host, port, token);
        tracing::info!("🔌 Connecting to {}", url);

        let (ws_stream, resp) = connect_async(&url).await?;
        tracing::info!("🔌 ✓ WS handshake complete (status={})", resp.status());

        tracing::info!("🔌 Splitting WS stream...");
        let (write_sink, read_stream) = ws_stream.split();

        tracing::info!("🔌 Spawning I/O loop...");
        let (ready_tx, ready_rx) = oneshot::channel();
        let (write_tx, write_rx) = mpsc::unbounded_channel();

        let state = IoLoopState {
            handlers: self.handlers.clone(),
            streams: self.streams.clone(),
            is_connected: self.is_connected.clone(),
        };

        tokio::spawn(async move {
            Self::io_loop(write_sink, read_stream, write_rx, ready_tx, state).await;
        });

        tracing::info!("🔌 Waiting for I/O loop to be ready...");
        ready_rx
            .await
            .map_err(|_| anyhow::anyhow!("I/O loop panicked before starting"))?;
        tracing::info!("🔌 ✓ I/O loop confirmed READY");

        *self.write_tx.lock().await = Some(write_tx);
        *self.is_connected.lock().await = true;
        tracing::info!("🔌 Connection state: is_connected=true, write_tx set");

        {
            let h = self.handlers.lock().await;
            h.trigger_open();
        }
        tracing::info!("🔌 ✓ on_open triggered");

        tracing::info!("🔌 === WsAdapter CONNECT COMPLETE ===");
        Ok(())
    }

    async fn send(&mut self, message: Vec<u8>) -> Result<()> {
        tracing::trace!("📤 send() called: {} bytes {:02x?}", message.len(), &message[..message.len().min(64)]);

        if !self.is_open() {
            tracing::warn!("📤 send(): connection NOT open, rejecting");
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        tracing::trace!("📤 Connection is open");

        let guard = self.write_tx.lock().await;
        match *guard {
            Some(ref tx) => {
                tx.send(message).map_err(|_| {
                    tracing::error!("📤 mpsc send failed: channel closed");
                    anyhow::anyhow!("Write channel closed")
                })?;
                tracing::trace!("📤 ✓ Packet queued via mpsc channel");
                Ok(())
            }
            None => {
                tracing::error!("📤 Write channel not available (None)");
                Err(anyhow::anyhow!("Write channel not available"))
            }
        }
    }

    async fn send_ping(&mut self, cid: u16) -> Result<()> {
        tracing::info!("🏓 Sending ping cid={}", cid);
        if !self.is_open() {
            tracing::warn!("🏓 send_ping(): connection NOT open");
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        let mut buffer = vec![0x00];
        buffer.extend(&cid.to_be_bytes());
        let guard = self.write_tx.lock().await;
        if let Some(ref tx) = *guard {
            tx.send(buffer)
                .map_err(|_| anyhow::anyhow!("Write channel closed"))?;
            tracing::info!("🏓 Ping cid={} queued", cid);
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        let open = self.is_connected.try_lock().map(|g| *g).unwrap_or(false);
        tracing::trace!("🔌 is_open() = {}", open);
        open
    }

    async fn close(&mut self) -> Result<()> {
        tracing::info!("🔌 close() called");
        *self.is_connected.lock().await = false;
        *self.write_tx.lock().await = None;
        tracing::info!("🔌 Connection closed");
        Ok(())
    }

    fn set_on_message(&mut self, handler: crate::transport_adapter::MessageHandler) {
        tracing::debug!("🔌 set_on_message");
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_message = Some(handler);
        }
    }

    fn set_on_open(&mut self, handler: crate::transport_adapter::OpenHandler) {
        tracing::debug!("🔌 set_on_open");
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_open = Some(handler);
        }
    }

    fn set_on_close(&mut self, handler: crate::transport_adapter::CloseHandler) {
        tracing::debug!("🔌 set_on_close");
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_close = Some(handler);
        }
    }

    fn set_on_error(&mut self, handler: crate::transport_adapter::ErrorHandler) {
        tracing::debug!("🔌 set_on_error");
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_error = Some(handler);
        }
    }
}

fn decode_cid_field(payload: &[u8]) -> Option<u16> {
    if payload.first().copied()? != 0x08 {
        return None;
    }

    let mut value = 0u32;
    let mut shift = 0;
    for byte in payload.iter().copied().skip(1) {
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return u16::try_from(value).ok();
        }
        shift += 7;
        if shift >= 16 {
            return None;
        }
    }

    None
}
