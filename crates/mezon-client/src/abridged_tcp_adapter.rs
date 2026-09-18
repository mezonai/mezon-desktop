use crate::tls_crypto;
use anyhow::Result;
use async_trait::async_trait;
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_rustls::rustls::pki_types::ServerName;

const WRITE_QUEUE_CAPACITY: usize = 256;
const WRITE_ENQUEUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SOCKET_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const SOCK_HOST_IP_ENV: &str = "MEZON_SOCK_HOST_IP";
// Budget for one whole connect attempt: TCP, the TLS handshake and I/O-loop
// startup share it. Only the TCP leg used to be bounded, so a stalled TLS
// handshake or a wedged I/O loop hung the reconnect forever.
const CONNECT_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(debug_assertions)]
fn sock_host_ip_override(host: &str) -> Option<String> {
    if host != crate::DEFAULT_WS_HOST {
        return None;
    }
    std::env::var(SOCK_HOST_IP_ENV)
        .ok()
        .map(|ip| ip.trim().to_string())
        .filter(|ip| !ip.is_empty())
}

#[cfg(not(debug_assertions))]
fn sock_host_ip_override(_host: &str) -> Option<String> {
    let _ = SOCK_HOST_IP_ENV;
    None
}

async fn connect_tcp(host: &str, port: u16, deadline: tokio::time::Instant) -> Result<TcpStream> {
    tokio::time::timeout_at(deadline, TcpStream::connect(format!("{host}:{port}")))
        .await
        .map_err(|_| anyhow::anyhow!("TCP connect timed out after 10s"))?
        .map_err(|e| anyhow::anyhow!("TCP connect failed: {e}"))
}

#[cfg(debug_assertions)]
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
#[cfg(debug_assertions)]
use tokio_rustls::rustls::pki_types::{CertificateDer, UnixTime};
#[cfg(debug_assertions)]
use tokio_rustls::rustls::{DigitallySignedStruct, SignatureScheme};

#[cfg(debug_assertions)]
#[derive(Debug)]
struct NoCertVerifier;

#[cfg(debug_assertions)]
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

use crate::transport_adapter::{AdapterHandlers, TransportAdapter};

fn build_client_config() -> tokio_rustls::rustls::ClientConfig {
    tls_crypto::ensure_crypto_provider();

    #[cfg(debug_assertions)]
    if matches!(
        std::env::var("MEZON_DANGER_ACCEPT_INVALID_CERTS").as_deref(),
        Ok("1") | Ok("true")
    ) {
        tracing::warn!(
            "TLS certificate verification DISABLED via MEZON_DANGER_ACCEPT_INVALID_CERTS (debug build only)"
        );
        return tokio_rustls::rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
            .with_no_client_auth();
    }

    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

const CODE_FIN: u16 = 0xff;
const PREFIX_RAW: u8 = 0xff;
const PREFIX_EXTENDED: u8 = 0x7f;
const RAW_HEADER_LENGTH: usize = 11;
const MAX_REALTIME_FRAME_LEN: usize = 1 << 20;
const MAX_API_RESPONSE_LEN: usize = 16 << 20;
const RESPONSE_CODE_TOO_LARGE: u32 = u16::MAX as u32;
const ENVELOPE_CID_TAG: u8 = 0x08;

fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 64 {
            return None;
        }
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None
}

/// A realtime frame that fails Envelope decode is either corrupt or not an
/// Envelope at all (schema drift, stray JSON, misaligned framing). Log a capped
/// hex prefix for the first few occurrences so a field report identifies the
/// actual wire content instead of just prost's error string.
fn log_undecodable_frame(kind: &str, payload: &[u8], err: &impl std::fmt::Display) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n < 4 {
        let prefix_len = payload.len().min(64);
        let hex: String = payload[..prefix_len]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        tracing::warn!(
            "{kind} decode failed (len={}, occurrence {n}): {err}; first {prefix_len} bytes: {hex}",
            payload.len()
        );
    } else {
        tracing::warn!("{kind} decode failed (len={}): {err}", payload.len());
    }
}

fn scan_realtime_cid(payload: &[u8]) -> Option<i32> {
    match payload.first() {
        Some(&ENVELOPE_CID_TAG) => {
            let (value, _) = read_varint(&payload[1..])?;
            Some(value as i32)
        }
        _ => Some(0),
    }
}

fn protobuf_message_len(buf: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    while pos < buf.len() {
        let (tag, tag_len) = read_varint(&buf[pos..])?;
        let field = tag >> 3;
        let wire = tag & 7;
        if field == 0 || matches!(wire, 3 | 4 | 6 | 7) {
            return Some(pos);
        }
        let value_start = pos + tag_len;
        let value_end = match wire {
            0 => {
                let (_, n) = read_varint(buf.get(value_start..)?)?;
                value_start + n
            }
            1 => value_start + 8,
            5 => value_start + 4,
            2 => {
                let (len, n) = read_varint(buf.get(value_start..)?)?;
                if len as usize > MAX_REALTIME_FRAME_LEN {
                    return Some(pos);
                }
                value_start + n + len as usize
            }
            _ => return Some(pos),
        };
        if value_end > buf.len() {
            return None;
        }
        pos = value_end;
    }
    Some(pos)
}

fn frame_kind(first: u8) -> &'static str {
    match first {
        0x00 => "ping/pong",
        0x82 => "ws-binary",
        0x80 | 0x81 | 0x83..=0x8f => "ws-other",
        0xef => "handshake",
        PREFIX_EXTENDED => "abridged-ext",
        PREFIX_RAW => "raw/api",
        0x01..=0x7e => "abridged",
        _ => "unknown",
    }
}

fn envelope_kind(envelope: &mezon_proto::realtime::Envelope) -> &'static str {
    use mezon_proto::realtime::envelope::Message as M;
    match &envelope.message {
        None => "<empty>",
        Some(M::ChannelMessage(_)) => "ChannelMessage",
        Some(M::MessageTypingEvent(_)) => "MessageTyping",
        Some(M::ChannelPresenceEvent(_)) => "ChannelPresence",
        Some(M::StatusPresenceEvent(_)) => "StatusPresence",
        Some(M::CustomStatusEvent(_)) => "CustomStatus",
        Some(M::UserStatusEvent(_)) => "UserStatus",
        Some(M::MessageReactionEvent(_)) => "MessageReaction",
        Some(M::MarkAsRead(_)) => "MarkAsRead",
        Some(M::ChannelCreatedEvent(_)) => "ChannelCreated",
        Some(M::ChannelUpdatedEvent(_)) => "ChannelUpdated",
        Some(M::ChannelDeletedEvent(_)) => "ChannelDeleted",
        Some(M::VoiceStartedEvent(_)) => "VoiceStarted",
        Some(M::VoiceEndedEvent(_)) => "VoiceEnded",
        Some(M::VoiceJoinedEvent(_)) => "VoiceJoined",
        Some(M::VoiceLeavedEvent(_)) => "VoiceLeaved",
        Some(M::UserChannelAddedEvent(_)) => "UserChannelAdded",
        Some(M::UserChannelRemovedEvent(_)) => "UserChannelRemoved",
        Some(M::AddClanUserEvent(_)) => "AddClanUser",
        Some(M::UserClanRemovedEvent(_)) => "UserClanRemoved",
        Some(M::BanUserEvent(_)) => "BanUser",
        Some(M::ClanUpdatedEvent(_)) => "ClanUpdated",
        Some(M::ClanProfileUpdatedEvent(_)) => "ClanProfileUpdated",
        Some(M::UserProfileUpdatedEvent(_)) => "UserProfileUpdated",
        Some(M::ClanDeletedEvent(_)) => "ClanDeleted",
        Some(M::AddFriend(_)) => "AddFriend",
        Some(M::RemoveFriend(_)) => "RemoveFriend",
        Some(M::RefreshSessionEvent(_)) => "RefreshSession",
        Some(M::ApiRequestEvent(_)) => "ApiRequest",
        Some(M::ChannelJoin(_)) => "ChannelJoin",
        Some(M::Error(_)) => "Error",
        Some(_) => "Other",
    }
}

fn content_snippet(content: &str) -> String {
    let text = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("t").and_then(|t| t.as_str().map(str::to_owned)))
        .unwrap_or_else(|| content.to_owned());
    let one_line = text.replace('\n', " ");
    match one_line.char_indices().nth(120) {
        Some((idx, _)) => format!("{}...", &one_line[..idx]),
        None => one_line,
    }
}

fn envelope_detail(envelope: &mezon_proto::realtime::Envelope) -> String {
    use mezon_proto::realtime::envelope::Message as M;
    match &envelope.message {
        Some(M::ChannelMessage(m)) => format!(
            "ChannelMessage id={} ch={} from={} \"{}\"",
            m.message_id,
            m.channel_id,
            m.sender_id,
            content_snippet(&m.content)
        ),
        Some(M::ApiRequestEvent(e)) => format!("ApiRequest({})", e.api_name),
        Some(M::Error(e)) => format!("Error code={} {}", e.code, e.message),
        _ => envelope_kind(envelope).to_string(),
    }
}

fn log_realtime_envelope(payload: &[u8]) {
    if !tracing::enabled!(tracing::Level::TRACE) {
        return;
    }
    if let Ok(envelope) = mezon_proto::realtime::Envelope::decode(payload) {
        tracing::trace!(
            "realtime -> Envelope cid={} {}",
            envelope.cid,
            envelope_detail(&envelope)
        );
    }
}

type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

fn is_unauthorized_status_line(status_line: &str) -> bool {
    status_line
        .split_whitespace()
        .any(|token| token == "401" || token == "403")
}

struct IoLoopState {
    handlers: Arc<Mutex<AdapterHandlers>>,
    streams: HashMap<u16, Vec<Vec<u8>>>,
    is_connected: Arc<AtomicBool>,
    frames_received: Arc<AtomicU64>,
    credential_rejected: Arc<AtomicBool>,
    read_buffer: Vec<u8>,
}

pub struct AbridgedTcpAdapter {
    write_tx: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    handlers: Arc<Mutex<AdapterHandlers>>,
    is_connected: Arc<AtomicBool>,
    frames_received: Arc<AtomicU64>,
    credential_rejected: Arc<AtomicBool>,
    io_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AbridgedTcpAdapter {
    pub fn new() -> Self {
        Self {
            write_tx: Arc::new(Mutex::new(None)),
            handlers: Arc::new(Mutex::new(AdapterHandlers::default())),
            is_connected: Arc::new(AtomicBool::new(false)),
            frames_received: Arc::new(AtomicU64::new(0)),
            credential_rejected: Arc::new(AtomicBool::new(false)),
            io_task: Arc::new(Mutex::new(None)),
        }
    }
}

enum Frame {
    Ping(u16),
    Raw {
        cid: u16,
        code: u32,
        fin: bool,
        payload: Vec<u8>,
    },
    Realtime(Vec<u8>),
}

enum FrameStep {
    NeedMore,
    Reset(&'static str),
    Frame { consumed: usize, frame: Frame },
}

fn realtime_payload(framed: &[u8]) -> Vec<u8> {
    let end = protobuf_message_len(framed).unwrap_or(framed.len());
    framed[..end].to_vec()
}

fn decode_frame(buf: &[u8]) -> FrameStep {
    let first = buf[0];
    match first {
        0x00 => {
            if buf.len() < 3 {
                return FrameStep::NeedMore;
            }
            let cid = u16::from_be_bytes([buf[1], buf[2]]);
            FrameStep::Frame {
                consumed: 3,
                frame: Frame::Ping(cid),
            }
        }
        PREFIX_RAW => {
            if buf.len() < RAW_HEADER_LENGTH {
                return FrameStep::NeedMore;
            }
            let cid = u16::from_be_bytes([buf[1], buf[2]]);
            let code = u32::from_be_bytes([buf[3], buf[4], buf[5], buf[6]]);
            let len = u32::from_be_bytes([buf[7], buf[8], buf[9], buf[10]]) as usize;
            if len > MAX_API_RESPONSE_LEN {
                return FrameStep::Reset("raw frame length too large");
            }
            let total = RAW_HEADER_LENGTH + len;
            if buf.len() < total {
                return FrameStep::NeedMore;
            }
            let response_code = (code >> 16) & 0xffff;
            let fin = (code & 0xffff) as u16 == CODE_FIN;
            FrameStep::Frame {
                consumed: total,
                frame: Frame::Raw {
                    cid,
                    code: response_code,
                    fin,
                    payload: buf[RAW_HEADER_LENGTH..total].to_vec(),
                },
            }
        }
        f if f < PREFIX_EXTENDED => {
            let total = 1 + f as usize * 4;
            if buf.len() < total {
                return FrameStep::NeedMore;
            }
            FrameStep::Frame {
                consumed: total,
                frame: Frame::Realtime(realtime_payload(&buf[1..total])),
            }
        }
        PREFIX_EXTENDED => {
            if buf.len() < 4 {
                return FrameStep::NeedMore;
            }
            let payload_len = u32::from_le_bytes([buf[1], buf[2], buf[3], 0]) as usize * 4;
            if payload_len > MAX_REALTIME_FRAME_LEN {
                return FrameStep::Reset("extended frame length too large");
            }
            let total = 4 + payload_len;
            if buf.len() < total {
                return FrameStep::NeedMore;
            }
            FrameStep::Frame {
                consumed: total,
                frame: Frame::Realtime(realtime_payload(&buf[4..total])),
            }
        }
        0x82 => {
            if buf.len() < 2 {
                return FrameStep::NeedMore;
            }
            let b1 = buf[1];
            if b1 & 0x80 != 0 {
                return FrameStep::Reset("masked websocket frame");
            }
            let len7 = b1 as usize;
            let (header, payload_len) = if len7 < 126 {
                (2, len7)
            } else if len7 == 126 {
                if buf.len() < 4 {
                    return FrameStep::NeedMore;
                }
                (4, u16::from_be_bytes([buf[2], buf[3]]) as usize)
            } else {
                if buf.len() < 10 {
                    return FrameStep::NeedMore;
                }
                (
                    10,
                    u64::from_be_bytes([
                        buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8], buf[9],
                    ]) as usize,
                )
            };
            if payload_len > MAX_REALTIME_FRAME_LEN {
                return FrameStep::Reset("websocket frame length too large");
            }
            let total = header + payload_len;
            if buf.len() < total {
                return FrameStep::NeedMore;
            }
            FrameStep::Frame {
                consumed: total,
                frame: Frame::Realtime(buf[header..total].to_vec()),
            }
        }
        _ => FrameStep::Reset("unexpected lead byte"),
    }
}

impl IoLoopState {
    fn handle_data(&mut self, handlers: &AdapterHandlers, incoming: &[u8]) -> Result<()> {
        if incoming.is_empty() {
            return Ok(());
        }
        self.read_buffer.extend_from_slice(incoming);
        self.process_raw_buffer(handlers)
    }

    fn process_raw_buffer(&mut self, handlers: &AdapterHandlers) -> Result<()> {
        let mut start = 0usize;

        loop {
            let frame = {
                if start >= self.read_buffer.len() {
                    self.read_buffer.drain(..start);
                    return Ok(());
                }
                match decode_frame(&self.read_buffer[start..]) {
                    FrameStep::NeedMore => {
                        self.read_buffer.drain(..start);
                        return Ok(());
                    }
                    FrameStep::Reset(reason) => {
                        let abandoned: Vec<u16> = self.streams.keys().copied().collect();
                        tracing::error!(
                            "frame desync ({reason}); dropping {} buffered bytes and {} partial response(s) {abandoned:?} — forcing reconnect",
                            self.read_buffer.len(),
                            abandoned.len(),
                        );
                        self.read_buffer.clear();
                        self.streams.clear();
                        return Err(anyhow::anyhow!("frame desync: {reason}"));
                    }
                    FrameStep::Frame { consumed, frame } => {
                        start += consumed;
                        frame
                    }
                }
            };

            match frame {
                Frame::Ping(cid) => {
                    tracing::trace!("PONG: cid={cid}");
                    handlers.trigger_message(cid, 0, vec![]);
                }
                Frame::Raw {
                    cid,
                    code,
                    fin,
                    payload,
                } => {
                    if fin {
                        let body = match self.streams.remove(&cid) {
                            Some(chunks) => {
                                let mut combined: Vec<u8> = chunks.concat();
                                combined.extend_from_slice(&payload);
                                combined
                            }
                            None => payload,
                        };
                        tracing::debug!(
                            "Complete API response: cid={cid} code={code} len={} bytes",
                            body.len()
                        );
                        handlers.trigger_message(cid, code, body);
                    } else {
                        let chunks = self.streams.entry(cid).or_default();
                        chunks.push(payload);
                        let buffered: usize = chunks.iter().map(Vec::len).sum();
                        if buffered > MAX_API_RESPONSE_LEN {
                            self.streams.remove(&cid);
                            tracing::warn!(
                                "cid={cid} response exceeds {MAX_API_RESPONSE_LEN}-byte cap, failing request"
                            );
                            handlers.trigger_message(cid, RESPONSE_CODE_TOO_LARGE, vec![]);
                        }
                    }
                }
                Frame::Realtime(payload) => {
                    let cid = match scan_realtime_cid(&payload) {
                        Some(cid) => cid,
                        None => match mezon_proto::realtime::Envelope::decode(payload.as_slice()) {
                            Ok(envelope) => envelope.cid,
                            Err(e) => {
                                log_undecodable_frame("realtime frame", &payload, &e);
                                continue;
                            }
                        },
                    };
                    log_realtime_envelope(&payload);
                    match u16::try_from(cid) {
                        Ok(cid) => handlers.trigger_message(cid, 0, payload),
                        Err(_) => {
                            tracing::warn!("envelope cid {cid} out of u16 range, dropping")
                        }
                    }
                }
            }
        }
    }
}

enum LoopExit {
    ServerClosed,
    Error(String),
    WriteChannelClosed,
}

impl AbridgedTcpAdapter {
    async fn io_loop(
        mut tls: TlsStream,
        mut write_rx: mpsc::Receiver<Vec<u8>>,
        ready_tx: oneshot::Sender<()>,
        mut state: IoLoopState,
    ) {
        let mut read_buf = vec![0u8; 8192];
        let mut read_count = 0u64;
        let handlers = state.handlers.lock().await.clone();

        // Signal that io_loop is ready (select is polling)
        let _ = ready_tx.send(());
        tracing::debug!("I/O loop running, entering select branch");

        let exit: LoopExit = loop {
            tracing::trace!("select iteration begin");
            tokio::select! {
                result = tls.read(&mut read_buf) => {
                    match result {
                        Ok(0) => {
                            tracing::info!("Server closed connection after {} reads", read_count);
                            break LoopExit::ServerClosed;
                        }
                        Ok(n) => {
                            read_count += 1;
                            state.frames_received.fetch_add(1, Ordering::Release);
                            tracing::trace!(
                                "READ {} bytes [{}] (reads: {}) {:02x?}",
                                n,
                                frame_kind(read_buf[0]),
                                read_count,
                                &read_buf[..n.min(16)]
                            );

                            let chunk = &read_buf[..n];
                            if chunk.starts_with(b"HTTP/")
                                || chunk.starts_with(b"GET ")
                                || chunk.starts_with(b"POST ")
                            {
                                let preview = String::from_utf8_lossy(&chunk[..n.min(120)]);
                                let status_line = preview.lines().next().unwrap_or("?");
                                if is_unauthorized_status_line(status_line) {
                                    state
                                        .credential_rejected
                                        .store(true, Ordering::Release);
                                    tracing::warn!(
                                        "Gateway refused the socket credential: {status_line}"
                                    );
                                    break LoopExit::Error(
                                        "gateway rejected the socket credential".into(),
                                    );
                                }
                                tracing::error!(
                                    "Server returned HTTP on abridged TCP port (expected binary framing, not nginx/WSS). Response: {status_line}"
                                );
                                break LoopExit::Error(
                                    "server spoke HTTP instead of abridged TCP".into(),
                                );
                            }

                            if let Err(e) = state.handle_data(&handlers, &read_buf[..n]) {
                                tracing::error!("handle_data error: {}", e);
                                break LoopExit::Error(e.to_string());
                            }
                        }
                        Err(e) => {
                            tracing::error!("READ error: kind={:?} msg={}", e.kind(), e);
                            break LoopExit::Error(e.to_string());
                        }
                    }
                }
                maybe_msg = write_rx.recv() => {
                    match maybe_msg {
                        Some(packet) => {
                            if packet.first() == Some(&0xef) {
                                tracing::trace!("WRITE handshake frame");
                            } else {
                                tracing::trace!(
                                    "WRITE {} bytes [{}] {:02x?}",
                                    packet.len(),
                                    frame_kind(packet[0]),
                                    &packet[..packet.len().min(32)]
                                );
                            }
                            match tokio::time::timeout(
                                SOCKET_WRITE_TIMEOUT,
                                tls.write_all(&packet),
                            )
                            .await
                            {
                                Ok(Ok(())) => tracing::trace!("write_all OK"),
                                Ok(Err(e)) => {
                                    tracing::error!("write_all error: {}", e);
                                    break LoopExit::Error(e.to_string());
                                }
                                Err(_) => {
                                    tracing::error!("write_all stalled; socket is not draining");
                                    break LoopExit::Error("socket write timed out".to_string());
                                }
                            }
                            match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, tls.flush()).await {
                                Ok(Ok(())) => tracing::trace!("flush OK"),
                                Ok(Err(e)) => {
                                    tracing::error!("flush error: {}", e);
                                    break LoopExit::Error(e.to_string());
                                }
                                Err(_) => {
                                    tracing::error!("flush stalled; socket is not draining");
                                    break LoopExit::Error("socket flush timed out".to_string());
                                }
                            }
                        }
                        None => {
                            tracing::info!("Write channel closed, exiting I/O loop");
                            break LoopExit::WriteChannelClosed;
                        }
                    }
                }
            }
        };

        match exit {
            LoopExit::ServerClosed => {
                state.is_connected.store(false, Ordering::Release);
                handlers.trigger_close(true);
            }
            LoopExit::Error(msg) => {
                state.is_connected.store(false, Ordering::Release);
                handlers.trigger_error(msg);
                handlers.trigger_close(false);
            }
            LoopExit::WriteChannelClosed => {}
        }

        tracing::info!("I/O loop exited (total reads: {})", read_count);
    }
}

fn frame_handshake(token: &str) -> Vec<u8> {
    let token_bytes = token.as_bytes();
    let padding = (4 - (token_bytes.len() % 4)) % 4;
    let mut final_token = token_bytes.to_vec();
    final_token.extend(vec![0u8; padding]);
    let len_div4 = final_token.len() / 4;
    let mut frame = if len_div4 < 127 {
        vec![0xef, len_div4 as u8]
    } else {
        let mut h = vec![0xef, PREFIX_EXTENDED, 0, 0, 0];
        h[2..5].copy_from_slice(&(len_div4 as u32).to_le_bytes()[..3]);
        h
    };
    frame.extend(&final_token);
    frame
}

impl Default for AbridgedTcpAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TransportAdapter for AbridgedTcpAdapter {
    async fn connect(&self, host: &str, port: u16, token: &str) -> Result<()> {
        tracing::info!("=== CONNECT START: {}:{} ===", host, port);
        self.frames_received.store(0, Ordering::Release);
        self.credential_rejected.store(false, Ordering::Release);

        let config = build_client_config();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));

        let deadline = tokio::time::Instant::now() + CONNECT_ATTEMPT_TIMEOUT;

        tracing::debug!("TCP connecting...");
        let tcp = match sock_host_ip_override(host) {
            Some(ip) => match connect_tcp(&ip, port, deadline).await {
                Ok(tcp) => tcp,
                Err(e) => {
                    tracing::warn!("pinned socket host failed ({e}); falling back to DNS");
                    // Fresh budget: the pinned attempt may already have spent it all.
                    let dns_deadline = tokio::time::Instant::now() + CONNECT_ATTEMPT_TIMEOUT;
                    connect_tcp(host, port, dns_deadline).await?
                }
            },
            None => connect_tcp(host, port, deadline).await?,
        };
        let local = tcp
            .local_addr()
            .map_err(|e| anyhow::anyhow!("local_addr: {e}"))?;
        tracing::debug!("TCP connected: local={}", local);

        let domain = ServerName::try_from(host.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid DNS name: {e}"))?;

        tracing::debug!("Starting TLS handshake...");
        let tls = tokio::time::timeout_at(deadline, connector.connect(domain, tcp))
            .await
            .map_err(|_| anyhow::anyhow!("TLS handshake timed out after 10s"))?
            .map_err(|e| anyhow::anyhow!("TLS handshake failed: {e}"))?;
        tracing::debug!("TLS handshake complete");

        let (ready_tx, ready_rx) = oneshot::channel();
        let (write_tx, write_rx) = mpsc::channel(WRITE_QUEUE_CAPACITY);
        let state = IoLoopState {
            handlers: self.handlers.clone(),
            streams: HashMap::new(),
            is_connected: self.is_connected.clone(),
            frames_received: self.frames_received.clone(),
            credential_rejected: self.credential_rejected.clone(),
            read_buffer: Vec::new(),
        };

        let previous = self.io_task.lock().await.take();
        if let Some(previous) = previous {
            previous.abort();
            let _ = previous.await;
        }

        tracing::info!("Spawning I/O loop...");
        let task = tokio::spawn(async move {
            Self::io_loop(tls, write_rx, ready_tx, state).await;
        });

        tracing::debug!("Waiting for I/O loop to be ready...");
        tokio::time::timeout_at(deadline, ready_rx)
            .await
            .map_err(|_| anyhow::anyhow!("I/O loop startup timed out after 10s"))?
            .map_err(|_| anyhow::anyhow!("I/O loop panicked before starting"))?;
        tracing::info!("I/O loop confirmed READY");
        *self.io_task.lock().await = Some(task);

        let handshake = frame_handshake(token);

        tracing::debug!("Sending handshake");
        write_tx
            .send(handshake)
            .await
            .map_err(|_| anyhow::anyhow!("Write channel closed early"))?;
        tracing::debug!("Handshake queued via mpsc channel");

        *self.write_tx.lock().await = Some(write_tx);
        self.is_connected.store(true, Ordering::Release);
        tracing::debug!("Connection state: is_connected=true, write_tx set");

        {
            let h = self.handlers.lock().await;
            h.trigger_open();
        }
        tracing::debug!("on_open triggered");

        tracing::info!("Transport connected");
        Ok(())
    }

    async fn send(&self, message: Vec<u8>) -> Result<()> {
        if tracing::enabled!(tracing::Level::TRACE) {
            match mezon_proto::realtime::Envelope::decode(message.as_slice()) {
                Ok(envelope) => tracing::trace!(
                    "send() {} bytes -> Envelope cid={} {}",
                    message.len(),
                    envelope.cid,
                    envelope_detail(&envelope)
                ),
                Err(_) => tracing::trace!(
                    "send() {} bytes (non-envelope) {:02x?}",
                    message.len(),
                    &message[..message.len().min(16)]
                ),
            }
        }

        if !self.is_open() {
            tracing::warn!("send(): connection NOT open, rejecting");
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        tracing::trace!("Connection is open");

        let padding_needed = (4 - (message.len() % 4)) % 4;
        let mut final_payload = message;
        final_payload.extend(vec![0u8; padding_needed]);
        tracing::trace!(
            "Padded to {} bytes (+{} padding)",
            final_payload.len(),
            padding_needed
        );

        let len_div4 = final_payload.len() / 4;
        let header = if len_div4 < 127 {
            tracing::trace!("Abridged header: 1-byte ({})", len_div4);
            vec![len_div4 as u8]
        } else {
            let mut h = vec![PREFIX_EXTENDED, 0, 0, 0];
            h[1..4].copy_from_slice(&(len_div4 as u32).to_le_bytes()[..3]);
            tracing::trace!("Abridged header: 4-byte extended ({})", len_div4);
            h
        };

        let mut packet = header;
        packet.extend(&final_payload);
        tracing::trace!(
            "Full abridged packet: {} bytes {:02x?}",
            packet.len(),
            &packet[..packet.len().min(64)]
        );

        let tx = {
            let guard = self.write_tx.lock().await;
            match *guard {
                Some(ref tx) => tx.clone(),
                None => {
                    tracing::error!("Write channel not available (None)");
                    return Err(anyhow::anyhow!("Write channel not available"));
                }
            }
        };
        tx.send_timeout(packet, WRITE_ENQUEUE_TIMEOUT)
            .await
            .map_err(|err| match err {
                mpsc::error::SendTimeoutError::Timeout(_) => {
                    tracing::error!("write queue full; the socket is not draining");
                    anyhow::anyhow!("Write queue full")
                }
                mpsc::error::SendTimeoutError::Closed(_) => {
                    tracing::error!("mpsc send failed: channel closed");
                    anyhow::anyhow!("Write channel closed")
                }
            })?;
        tracing::trace!("Packet queued via mpsc channel");

        Ok(())
    }

    async fn send_ping(&self, cid: u16) -> Result<()> {
        if !self.is_open() {
            return Err(anyhow::anyhow!("Connection is not open"));
        }
        let mut buffer = vec![0x00];
        buffer.extend(&cid.to_be_bytes());
        let tx = {
            let guard = self.write_tx.lock().await;
            match *guard {
                Some(ref tx) => tx.clone(),
                None => return Err(anyhow::anyhow!("Write channel not available")),
            }
        };
        tx.send_timeout(buffer, WRITE_ENQUEUE_TIMEOUT)
            .await
            .map_err(|err| match err {
                mpsc::error::SendTimeoutError::Timeout(_) => {
                    anyhow::anyhow!("Write queue full")
                }
                mpsc::error::SendTimeoutError::Closed(_) => {
                    anyhow::anyhow!("Write channel closed")
                }
            })?;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.is_connected.load(Ordering::Acquire)
    }

    fn frames_received(&self) -> u64 {
        self.frames_received.load(Ordering::Acquire)
    }

    fn credential_rejected(&self) -> bool {
        self.credential_rejected.load(Ordering::Acquire)
    }

    async fn close(&self) -> Result<()> {
        self.is_connected.store(false, Ordering::Release);
        *self.write_tx.lock().await = None;
        let task = self.io_task.lock().await.take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        Ok(())
    }

    async fn set_on_message(&self, handler: crate::transport_adapter::MessageHandler) {
        self.handlers.lock().await.on_message = Some(handler);
    }
    async fn set_on_open(&self, handler: crate::transport_adapter::OpenHandler) {
        self.handlers.lock().await.on_open = Some(handler);
    }
    async fn set_on_close(&self, handler: crate::transport_adapter::CloseHandler) {
        self.handlers.lock().await.on_close = Some(handler);
    }
    async fn set_on_error(&self, handler: crate::transport_adapter::ErrorHandler) {
        self.handlers.lock().await.on_error = Some(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimits_ack_body_before_next_frame() {
        let stream = [
            0x10, 0x80, 0xa0, 0x80, 0xf0, 0xfe, 0xd1, 0xd3, 0xc5, 0x19, 0x0c, 0xc2, 0x01, 0x2a,
        ];
        assert_eq!(protobuf_message_len(&stream), Some(10));
    }

    #[test]
    fn consumes_whole_message_when_no_trailing_frame() {
        let msg = [0x10, 0x80, 0xa0, 0x80, 0xf0, 0xfe, 0xd1, 0xd3, 0xc5, 0x19];
        assert_eq!(protobuf_message_len(&msg), Some(10));
    }

    #[test]
    fn handles_length_delimited_field() {
        let stream = [0x0a, 0x03, b'a', b'b', b'c', 0x17, 0x32, 0x57];
        assert_eq!(protobuf_message_len(&stream), Some(5));
    }

    #[test]
    fn returns_none_on_incomplete_varint() {
        let stream = [0x10, 0x80, 0xa0];
        assert_eq!(protobuf_message_len(&stream), None);
    }

    #[test]
    fn returns_none_on_incomplete_length_delimited() {
        let stream = [0x0a, 0x05, b'a', b'b'];
        assert_eq!(protobuf_message_len(&stream), None);
    }

    #[test]
    fn caps_absurd_field_length() {
        let stream = [0x0a, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x00, 0x00];
        assert_eq!(protobuf_message_len(&stream), Some(0));
    }

    #[test]
    fn protobuf_len_includes_trailing_empty_field() {
        let framed = [0x08, 0x58, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(protobuf_message_len(&framed), Some(4));
    }

    fn raw_frame(cid: u16, fin: bool, payload: &[u8]) -> Vec<u8> {
        let code: u32 = if fin { u32::from(CODE_FIN) } else { 0 };
        let mut frame = vec![PREFIX_RAW];
        frame.extend_from_slice(&cid.to_be_bytes());
        frame.extend_from_slice(&code.to_be_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    fn channel_messages_body(content_len: usize) -> Vec<u8> {
        mezon_proto::api::ChannelMessageList {
            messages: vec![mezon_proto::api::ChannelMessage {
                content: "x".repeat(content_len),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode_to_vec()
    }

    type CapturedMessages = Arc<std::sync::Mutex<Vec<(u16, u32, Vec<u8>)>>>;

    fn captured_state() -> (IoLoopState, AdapterHandlers, CapturedMessages) {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = received.clone();
        let handlers = AdapterHandlers {
            on_message: Some(Arc::new(move |cid, code, bytes| {
                sink.lock().unwrap().push((cid, code, bytes));
            })),
            ..Default::default()
        };
        let state = IoLoopState {
            handlers: Arc::new(Mutex::new(handlers.clone())),
            streams: HashMap::new(),
            is_connected: Arc::new(AtomicBool::new(true)),
            frames_received: Arc::new(AtomicU64::new(0)),
            credential_rejected: Arc::new(AtomicBool::new(false)),
            read_buffer: Vec::new(),
        };
        (state, handlers, received)
    }

    #[tokio::test]
    async fn reassembles_chunked_api_response_split_across_frames() {
        let body = channel_messages_body(6000);
        let chunk = 4096;
        assert!(body.len() > chunk);

        let (mut state, handlers, received) = captured_state();

        state
            .handle_data(&handlers, &raw_frame(6, false, &body[..chunk]))
            .unwrap();
        state
            .handle_data(&handlers, &raw_frame(6, true, &body[chunk..]))
            .unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, 6);
        assert_eq!(received[0].1, 0);
        assert_eq!(received[0].2, body);
        mezon_proto::api::ChannelMessageList::decode(received[0].2.as_slice()).unwrap();
    }

    #[tokio::test]
    async fn delivers_single_frame_api_response() {
        let body = channel_messages_body(16);

        let (mut state, handlers, received) = captured_state();

        state
            .handle_data(&handlers, &raw_frame(7, true, &body))
            .unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, 7);
        assert_eq!(received[0].2, body);
    }

    #[tokio::test]
    async fn raw_frame_split_header_and_payload_across_reads() {
        let body = channel_messages_body(64);
        let frame = raw_frame(8, true, &body);

        let (mut state, handlers, received) = captured_state();

        state.handle_data(&handlers, &frame[..5]).unwrap();
        assert!(received.lock().unwrap().is_empty());
        state.handle_data(&handlers, &frame[5..]).unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, 8);
        assert_eq!(received[0].2, body);
    }

    #[test]
    fn handshake_short_token_uses_1byte_header() {
        let frame = frame_handshake("abc");
        assert_eq!(frame[0], 0xef);
        assert_eq!(frame[1], 1);
        assert_eq!(frame.len(), 2 + 4);
    }

    #[test]
    fn handshake_long_token_uses_extended_header() {
        let token = "x".repeat(600);
        let frame = frame_handshake(&token);
        assert_eq!(frame[0], 0xef);
        assert_eq!(frame[1], PREFIX_EXTENDED);
        let len_div4 = u32::from_le_bytes([frame[2], frame[3], frame[4], 0]) as usize;
        assert_eq!(len_div4, 150);
        assert_eq!(frame.len(), 5 + 600);
    }

    #[tokio::test]
    async fn empty_response_delivered_immediately() {
        let (mut state, handlers, received) = captured_state();

        state
            .handle_data(&handlers, &raw_frame(9, true, &[]))
            .unwrap();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, 9);
        assert_eq!(received[0].2, Vec::<u8>::new());
    }

    #[tokio::test]
    async fn concurrent_responses_route_by_cid() {
        let (mut state, handlers, received) = captured_state();

        let a = channel_messages_body(16);
        let b = channel_messages_body(32);
        let mut burst = raw_frame(20, true, &a);
        burst.extend_from_slice(&raw_frame(21, true, &[]));
        burst.extend_from_slice(&raw_frame(22, true, &b));
        state.handle_data(&handlers, &burst).unwrap();

        let received = received.lock().unwrap();
        let by: std::collections::HashMap<u16, Vec<u8>> =
            received.iter().map(|m| (m.0, m.2.clone())).collect();
        assert_eq!(by.get(&20), Some(&a));
        assert_eq!(by.get(&21), Some(&Vec::<u8>::new()));
        assert_eq!(by.get(&22), Some(&b));
    }
}
