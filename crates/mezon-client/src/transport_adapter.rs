use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub type MessageHandler = Arc<dyn Fn(u16, u32, Vec<u8>) + Send + Sync>;

pub type OpenHandler = Arc<dyn Fn() + Send + Sync>;

pub type CloseHandler = Arc<dyn Fn(bool) + Send + Sync>;

pub type ErrorHandler = Arc<dyn Fn(String) + Send + Sync>;

/// The connection closed before the adapter could enqueue the message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct SocketUnavailable(pub(crate) &'static str);

#[async_trait]
pub trait TransportAdapter: Send + Sync {
    async fn connect(&self, host: &str, port: u16, token: &str) -> Result<()>;

    async fn send(&self, message: Vec<u8>) -> Result<()>;

    async fn send_ping(&self, cid: u16) -> Result<()>;

    fn is_open(&self) -> bool;

    /// How many frames the server has sent since the current connect started. Zero after the
    /// socket dies means the peer never answered, which is a transport failure — not the server
    /// refusing the credential.
    fn frames_received(&self) -> u64 {
        0
    }

    /// Whether the gateway answered the handshake with an HTTP 401/403. That is the only signal
    /// that names the credential as dead — a silent close means the connection was refused for
    /// another reason (the per-user session limit, an outage) and must not discard the session.
    fn credential_rejected(&self) -> bool {
        false
    }

    async fn close(&self) -> Result<()>;

    async fn set_on_message(&self, handler: MessageHandler);

    async fn set_on_open(&self, handler: OpenHandler);

    async fn set_on_close(&self, handler: CloseHandler);

    async fn set_on_error(&self, handler: ErrorHandler);
}

#[derive(Clone, Default)]
pub struct AdapterHandlers {
    pub on_message: Option<MessageHandler>,
    pub on_open: Option<OpenHandler>,
    pub on_close: Option<CloseHandler>,
    pub on_error: Option<ErrorHandler>,
}

impl AdapterHandlers {
    pub fn trigger_message(&self, cid: u16, code: u32, message: Vec<u8>) {
        if let Some(handler) = &self.on_message {
            handler(cid, code, message);
        }
    }

    pub fn trigger_open(&self) {
        if let Some(handler) = &self.on_open {
            handler();
        }
    }

    pub fn trigger_close(&self, was_clean: bool) {
        if let Some(handler) = &self.on_close {
            handler(was_clean);
        }
    }

    pub fn trigger_error(&self, error: String) {
        if let Some(handler) = &self.on_error {
            handler(error);
        }
    }
}
