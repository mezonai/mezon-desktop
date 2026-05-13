/// Transport adapter trait for WebSocket and custom TCP connections.
///
/// Provides a common interface for different transport mechanisms (WebSocket, Abridged TCP, etc.)
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Handler for incoming messages.
/// Parameters: (cid, code, message_bytes)
pub type MessageHandler = Arc<dyn Fn(u16, u32, Vec<u8>) + Send + Sync>;

/// Handler for connection open events.
pub type OpenHandler = Arc<dyn Fn() + Send + Sync>;

/// Handler for connection close events.
pub type CloseHandler = Arc<dyn Fn(bool) + Send + Sync>; // was_clean

/// Handler for connection errors.
pub type ErrorHandler = Arc<dyn Fn(String) + Send + Sync>;

#[async_trait]
pub trait TransportAdapter: Send + Sync {
    /// Connect to the remote endpoint.
    async fn connect(&mut self, host: &str, port: u16, token: &str) -> Result<()>;

    /// Send a message through the transport.
    async fn send(&mut self, message: Vec<u8>) -> Result<()>;

    /// Send a ping message with the given CID.
    async fn send_ping(&mut self, cid: u16) -> Result<()>;

    /// Check if the connection is open.
    fn is_open(&self) -> bool;

    /// Close the connection.
    async fn close(&mut self) -> Result<()>;

    /// Set the message handler.
    fn set_on_message(&mut self, handler: MessageHandler);

    /// Set the open handler.
    fn set_on_open(&mut self, handler: OpenHandler);

    /// Set the close handler.
    fn set_on_close(&mut self, handler: CloseHandler);

    /// Set the error handler.
    fn set_on_error(&mut self, handler: ErrorHandler);
}

/// Shared state for handlers.
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
