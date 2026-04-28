// mezon-client: Rust equivalent of mezon-js
// Handles REST API calls and WebSocket connection to Mezon backend.

pub mod auth;
pub mod keychain;
pub mod session;

pub use auth::MezonClient;
pub use auth::{DEFAULT_API_HOST, DEFAULT_API_PORT, DEFAULT_API_SECURE, DEFAULT_SERVER_KEY};
pub use session::Session;

/// Default WebSocket host (used for Stage 2+ WebSocket connection).
pub const DEFAULT_WS_HOST: &str = "sock.mezon.ai";
pub const DEFAULT_WS_PORT: u16 = 443;
pub const DEFAULT_WS_SECURE: bool = true;
