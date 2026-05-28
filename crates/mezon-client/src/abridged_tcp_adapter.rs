use crate::transport_adapter::{AdapterHandlers, TransportAdapter};
use anyhow::Result;
use async_trait::async_trait;
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{DigitallySignedStruct, SignatureScheme};

#[derive(Debug)]
struct NoCertVerifier;

impl ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

static CRYPTO_PROVIDER: OnceLock<()> = OnceLock::new();

const CODE_FIN: u16 = 0xff;
const PREFIX_RAW: u8 = 0xff;
const PREFIX_EXTENDED: u8 = 0x7f;
const RAW_HEADER_LENGTH: usize = 7;
const RAW_CHUNK_HEADER_LENGTH: usize = 11;

type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

pub struct AbridgedTcpAdapter {
    write_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    handlers: Arc<Mutex<AdapterHandlers>>,
    streams: Arc<Mutex<HashMap<u16, Vec<Vec<u8>>>>>,
    is_connected: Arc<Mutex<bool>>,
    read_buffer: Arc<Mutex<Vec<u8>>>,
}

impl AbridgedTcpAdapter {
    pub fn new() -> Self {
        Self {
            write_tx: Arc::new(Mutex::new(None)),
            handlers: Arc::new(Mutex::new(AdapterHandlers::default())),
            streams: Arc::new(Mutex::new(HashMap::new())),
            is_connected: Arc::new(Mutex::new(false)),
            read_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn handle_data(&self, incoming: Vec<u8>) -> Result<()> {
        if incoming.is_empty() {
            return Ok(());
        }

        self.read_buffer.lock().await.extend(incoming);

        let handlers = self.handlers.lock().await.clone();

        loop {
            let msg_bytes = {
                let mut buf = self.read_buffer.lock().await;
                if buf.is_empty() {
                    return Ok(());
                }

                let first_byte = buf[0];
                if first_byte == 0x00 {
                    if buf.len() < 3 {
                        return Ok(());
                    }
                    let msg = buf[..3].to_vec();
                    buf.drain(..3);
                    Some((msg, false))
                } else if first_byte == PREFIX_RAW {
                    if buf.len() < RAW_HEADER_LENGTH {
                        return Ok(());
                    }
                    let code = u32::from_be_bytes([buf[3], buf[4], buf[5], buf[6]]);
                    let fin_flag = (code & 0xffff) as u16;
                    if fin_flag == CODE_FIN {
                        if buf.len() == RAW_HEADER_LENGTH {
                            return Ok(());
                        }
                        let msg = buf.clone();
                        buf.clear();
                        Some((msg, false))
                    } else if buf.len() < RAW_CHUNK_HEADER_LENGTH {
                        return Ok(());
                    } else {
                        let payload_len =
                            u32::from_be_bytes([buf[7], buf[8], buf[9], buf[10]]) as usize;
                        let total = RAW_CHUNK_HEADER_LENGTH + payload_len;
                        if buf.len() < total {
                            return Ok(());
                        }
                        let msg = buf[..total].to_vec();
                        buf.drain(..total);
                        Some((msg, false))
                    }
                } else if first_byte < 127 {
                    let payload_len = first_byte as usize * 4;
                    let total = 1 + payload_len;
                    if buf.len() < total {
                        return Ok(());
                    }
                    let msg = buf[..total].to_vec();
                    buf.drain(..total);
                    Some((msg, false))
                } else if first_byte == PREFIX_EXTENDED {
                    if buf.len() < 4 {
                        return Ok(());
                    }
                    let payload_len = u32::from_le_bytes([buf[1], buf[2], buf[3], 0]) as usize * 4;
                    let total = 4 + payload_len;
                    if buf.len() < total {
                        return Ok(());
                    }
                    let msg = buf[..total].to_vec();
                    buf.drain(..total);
                    Some((msg, false))
                } else {
                    tracing::warn!("📥 Unexpected first byte: {:#x}, skipping", first_byte);
                    buf.drain(..1);
                    Some((vec![], true))
                }
            };

            let (data, skipped) = match msg_bytes {
                Some(v) => v,
                None => continue,
            };

            if skipped {
                continue;
            }

            tracing::info!("📥 process_message: {} bytes", data.len());

            if data[0] == 0x00 {
                let cid = u16::from_be_bytes([data[1], data[2]]);
                tracing::info!("📨 PONG: cid={}", cid);
                handlers.trigger_message(cid, 0, vec![]);
                continue;
            }

            if data[0] == PREFIX_RAW {
                let cid = u16::from_be_bytes([data[1], data[2]]);
                let code = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
                let response_code = (code >> 16) & 0xffff;
                let fin_flag = (code & 0xffff) as u16;

                let (payload, payload_len) = if fin_flag == CODE_FIN {
                    (
                        data[RAW_HEADER_LENGTH..].to_vec(),
                        data.len() - RAW_HEADER_LENGTH,
                    )
                } else {
                    let len = u32::from_be_bytes([data[7], data[8], data[9], data[10]]) as usize;
                    (
                        data[RAW_CHUNK_HEADER_LENGTH..RAW_CHUNK_HEADER_LENGTH + len].to_vec(),
                        len,
                    )
                };

                tracing::info!(
                    "📥 RAW: cid={} code={:#x} response_code={} fin_flag={:#x} payload_len={}",
                    cid,
                    code,
                    response_code,
                    fin_flag,
                    payload_len
                );

                let mut streams = self.streams.lock().await;
                if fin_flag == CODE_FIN {
                    let chunks = streams.entry(cid).or_insert_with(Vec::new);
                    if payload_len > 0 {
                        chunks.push(payload);
                    }
                    let complete_buffer: Vec<u8> = chunks.concat();
                    tracing::info!(
                        "📨 Complete API response: cid={} code={} len={} bytes",
                        cid,
                        response_code,
                        complete_buffer.len()
                    );
                    handlers.trigger_message(cid, response_code, complete_buffer);
                    streams.remove(&cid);
                } else {
                    let chunks = streams.entry(cid).or_insert_with(Vec::new);
                    chunks.push(payload);
                    tracing::info!("📥 Buffered chunk for cid={} ({} total)", cid, chunks.len());
                }
                continue;
            }

            let (header_size, payload_length) = if data[0] < 127 {
                tracing::info!(
                    "📥 Standard msg: 1-byte header ({}*4={}bytes)",
                    data[0],
                    data[0] as usize * 4
                );
                (1, data[0] as usize * 4)
            } else {
                let len = u32::from_le_bytes([data[1], data[2], data[3], 0]) as usize * 4;
                tracing::info!("📥 Extended msg: len={}", len);
                (4, len)
            };

            let payload = &data[header_size..header_size + payload_length];
            tracing::info!(
                "📨 Std msg payload: {} bytes {:02x?}",
                payload.len(),
                &payload[..payload.len().min(32)]
            );

            if let Ok(envelope) = mezon_proto::realtime::Envelope::decode(payload) {
                tracing::info!("📨 Envelope decoded: cid={}", envelope.cid);
                handlers.trigger_message(envelope.cid as u16, 0, payload.to_vec());
            } else {
                let cid = decode_cid_field(payload).unwrap_or(0);
                tracing::warn!("📨 Failed to decode Envelope, passing raw cid={cid}");
                handlers.trigger_message(cid, 0, payload.to_vec());
            }
        }
    }

    async fn io_loop(
        mut tls: TlsStream,
        mut write_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        ready_tx: oneshot::Sender<()>,
        handlers: Arc<Mutex<AdapterHandlers>>,
        streams: Arc<Mutex<HashMap<u16, Vec<Vec<u8>>>>>,
        is_connected: Arc<Mutex<bool>>,
        read_buffer: Arc<Mutex<Vec<u8>>>,
    ) {
        let mut read_buf = vec![0u8; 8192];
        let mut read_count = 0u64;

        // Signal that io_loop is ready (select is polling)
        let _ = ready_tx.send(());
        tracing::info!("🔄 I/O loop running, entering select branch");

        loop {
            tracing::trace!("🔄 select iteration begin");
            tokio::select! {
                result = tls.read(&mut read_buf) => {
                    tracing::debug!("🔄 select: READ branch fired");
                    match result {
                        Ok(0) => {
                            tracing::info!("📖 Server closed connection after {} reads", read_count);
                            *is_connected.lock().await = false;
                            let h = handlers.lock().await;
                            h.trigger_close(true);
                            break;
                        }
                        Ok(n) => {
                            read_count += 1;
                            tracing::info!("📖 READ {} bytes (total reads: {})", n, read_count);
                            tracing::info!("📖 RAW bytes: {:02x?}", &read_buf[..n.min(128)]);

                            let data = read_buf[..n].to_vec();
                            let adapter = AbridgedTcpAdapter {
                                write_tx: Arc::new(Mutex::new(None)),
                                handlers: handlers.clone(),
                                streams: streams.clone(),
                                is_connected: is_connected.clone(),
                                read_buffer: read_buffer.clone(),
                            };
                            if let Err(e) = adapter.handle_data(data).await {
                                tracing::error!("✗ handle_data error: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::error!("✗ READ error: kind={:?} msg={}", e.kind(), e);
                            *is_connected.lock().await = false;
                            let h = handlers.lock().await;
                            h.trigger_error(e.to_string());
                            h.trigger_close(false);
                            break;
                        }
                    }
                }
                maybe_msg = write_rx.recv() => {
                    tracing::debug!("🔄 select: WRITE branch fired");
                    match maybe_msg {
                        Some(packet) => {
                            tracing::info!("📤 WRITE: {} bytes {:02x?}", packet.len(), &packet[..packet.len().min(64)]);
                            match tls.write_all(&packet).await {
                                Ok(()) => tracing::info!("📤 write_all OK"),
                                Err(e) => {
                                    tracing::error!("✗ write_all error: {}", e);
                                    break;
                                }
                            }
                            match tls.flush().await {
                                Ok(()) => tracing::info!("📤 flush OK"),
                                Err(e) => {
                                    tracing::error!("✗ flush error: {}", e);
                                    break;
                                }
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

        tracing::info!("📖 I/O loop exited (total reads: {})", read_count);
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

impl Default for AbridgedTcpAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TransportAdapter for AbridgedTcpAdapter {
    async fn connect(&mut self, host: &str, port: u16, token: &str) -> Result<()> {
        tracing::info!("🔌 === CONNECT START: {}:{} ===", host, port);
        tracing::info!("🔌 Token length: {}", token.len());

        CRYPTO_PROVIDER.get_or_init(|| {
            tracing::debug!("🔌 Installing ring crypto provider");
            tokio_rustls::rustls::crypto::ring::default_provider()
                .install_default()
                .expect("Failed to install crypto provider");
        });

        tracing::debug!("🔌 Building TLS config (dangerous, no cert verify)");
        let config = tokio_rustls::rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));

        let addr = format!("{}:{}", host, port);
        tracing::info!("🔌 TCP connecting to {}...", addr);
        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("TCP connect failed: {e}"))?;
        let local = tcp
            .local_addr()
            .map_err(|e| anyhow::anyhow!("local_addr: {e}"))?;
        let peer = tcp
            .peer_addr()
            .map_err(|e| anyhow::anyhow!("peer_addr: {e}"))?;
        tracing::info!("🔌 ✓ TCP connected: local={} peer={}", local, peer);

        let domain = ServerName::try_from(host.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid DNS name: {e}"))?;
        tracing::info!("🔌 DNS name parsed: {:?}", domain);

        tracing::info!("🔌 Starting TLS handshake with {}...", host);
        let tls = connector
            .connect(domain, tcp)
            .await
            .map_err(|e| anyhow::anyhow!("TLS handshake failed: {e}"))?;
        tracing::info!("🔌 ✓ TLS handshake complete");

        // Spawn I/O loop and wait for it to be ready
        let (ready_tx, ready_rx) = oneshot::channel();
        let (write_tx, write_rx) = mpsc::unbounded_channel();
        let h = self.handlers.clone();
        let st = self.streams.clone();
        let ic = self.is_connected.clone();
        let rb = self.read_buffer.clone();

        tracing::info!("🔌 Spawning I/O loop...");
        tokio::spawn(async move {
            Self::io_loop(tls, write_rx, ready_tx, h, st, ic, rb).await;
        });

        // Wait for I/O loop to signal readiness
        tracing::info!("🔌 Waiting for I/O loop to be ready...");
        ready_rx
            .await
            .map_err(|_| anyhow::anyhow!("I/O loop panicked before starting"))?;
        tracing::info!("🔌 ✓ I/O loop confirmed READY");

        // Now send handshake — I/O loop is definitely listening
        let token_bytes = token.as_bytes();
        let padding = (4 - (token_bytes.len() % 4)) % 4;
        let mut final_token = token_bytes.to_vec();
        final_token.extend(vec![0u8; padding]);
        let len_header = (final_token.len() / 4) as u8;
        let mut handshake = vec![0xef, len_header];
        handshake.extend(&final_token);

        tracing::info!(
            "🔌 Sending handshake: {} bytes (magic=0xef len={})",
            handshake.len(),
            len_header
        );
        tracing::info!(
            "🔌 Handshake hex: {:02x?}",
            &handshake[..handshake.len().min(32)]
        );
        write_tx
            .send(handshake)
            .map_err(|_| anyhow::anyhow!("Write channel closed early"))?;
        tracing::info!("🔌 ✓ Handshake queued via mpsc channel");

        *self.write_tx.lock().await = Some(write_tx);
        *self.is_connected.lock().await = true;
        tracing::info!("🔌 Connection state: is_connected=true, write_tx set");

        {
            let h = self.handlers.lock().await;
            h.trigger_open();
        }
        tracing::info!("🔌 ✓ on_open triggered");

        tracing::info!("🔌 === CONNECT COMPLETE ===");
        Ok(())
    }

    async fn send(&mut self, message: Vec<u8>) -> Result<()> {
        tracing::info!("📤 send() called: {} bytes", message.len());
        tracing::info!("📤 Raw msg hex FULL: {:02x?}", message);

        if !self.is_open() {
            tracing::warn!("📤 send(): connection NOT open, rejecting");
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        tracing::info!("📤 Connection is open");

        let padding_needed = (4 - (message.len() % 4)) % 4;
        let mut final_payload = message;
        final_payload.extend(vec![0u8; padding_needed]);
        tracing::info!(
            "📤 Padded to {} bytes (+{} padding)",
            final_payload.len(),
            padding_needed
        );

        let len_div4 = final_payload.len() / 4;
        let header = if len_div4 < 127 {
            tracing::info!("📤 Abridged header: 1-byte ({})", len_div4);
            vec![len_div4 as u8]
        } else {
            let mut h = vec![PREFIX_EXTENDED, 0, 0, 0];
            h[1..4].copy_from_slice(&(len_div4 as u32).to_le_bytes()[..3]);
            tracing::info!("📤 Abridged header: 4-byte extended ({})", len_div4);
            h
        };

        let mut packet = header;
        packet.extend(&final_payload);
        tracing::info!(
            "📤 Full abridged packet: {} bytes {:02x?}",
            packet.len(),
            &packet[..packet.len().min(64)]
        );

        let guard = self.write_tx.lock().await;
        match *guard {
            Some(ref tx) => {
                tx.send(packet).map_err(|_| {
                    tracing::error!("📤 mpsc send failed: channel closed");
                    anyhow::anyhow!("Write channel closed")
                })?;
                tracing::info!("📤 ✓ Packet queued via mpsc channel");
            }
            None => {
                tracing::error!("📤 Write channel not available (None)");
                return Err(anyhow::anyhow!("Write channel not available"));
            }
        }

        Ok(())
    }

    async fn send_ping(&mut self, cid: u16) -> Result<()> {
        if !self.is_open() {
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        let mut buffer = vec![0x00];
        buffer.extend(&cid.to_be_bytes());
        let guard = self.write_tx.lock().await;
        if let Some(ref tx) = *guard {
            tx.send(buffer)
                .map_err(|_| anyhow::anyhow!("Write channel closed"))?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.is_connected.try_lock().map(|g| *g).unwrap_or(false)
    }
    async fn close(&mut self) -> Result<()> {
        *self.is_connected.lock().await = false;
        *self.write_tx.lock().await = None;
        Ok(())
    }

    fn set_on_message(&mut self, handler: crate::transport_adapter::MessageHandler) {
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_message = Some(handler);
        }
    }
    fn set_on_open(&mut self, handler: crate::transport_adapter::OpenHandler) {
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_open = Some(handler);
        }
    }
    fn set_on_close(&mut self, handler: crate::transport_adapter::CloseHandler) {
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_close = Some(handler);
        }
    }
    fn set_on_error(&mut self, handler: crate::transport_adapter::ErrorHandler) {
        if let Ok(mut h) = self.handlers.try_lock() {
            h.on_error = Some(handler);
        }
    }
}
