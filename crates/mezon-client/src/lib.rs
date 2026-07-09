// mezon-client: Rust equivalent of mezon-js
// Handles REST API calls and WebSocket connection to Mezon backend.

pub mod abridged_tcp_adapter;
pub mod app_api;
pub mod attachment_download;
pub mod auth;
pub mod image_disk_cache;
pub mod inbox;
pub mod keychain;
pub mod network_monitor;
pub mod network_probe;
pub mod search_message;
pub mod session;
pub mod tls_crypto;
pub mod transport;
pub mod transport_adapter;
pub mod transport_runtime;

pub use abridged_tcp_adapter::AbridgedTcpAdapter;
pub use app_api::{
    AppApi, AttachmentUploadOutcome, ConnectionStatus, UploadFile, UploadThumbnail, UrlAttachment,
};
pub use attachment_download::{
    clean_download_url, download_url_to_downloads, resolve_download_filename, sanitize_filename,
    write_bytes_to_downloads,
};
pub use auth::MezonClient;
pub use auth::QrLoginId;
pub use auth::{DEFAULT_API_HOST, DEFAULT_API_PORT, DEFAULT_API_SECURE, DEFAULT_SERVER_KEY};
pub use inbox::{
    DIRECTION_AROUND_TIMESTAMP, DIRECTION_BEFORE_TIMESTAMP, INBOX_PAGE_LIMIT, InboxCategory,
    InboxMentionSpan, InboxMessagePreview, InboxNotification, TopicDiscussion,
    attachment_link_is_image, display_text_from_message_content, inbox_notification_from_api,
    inbox_notifications_from_list, message_content_is_attachment, topic_discussion_from_api,
    topics_from_list,
};
pub use network_monitor::NetworkMonitor;
pub use network_probe::{
    RECONNECT_NETWORK_PROBE_TIMEOUT, favicon_probe_url, probe_network_reachability,
};
pub use search_message::{
    SEARCH_PAGE_SIZE, build_clan_channel_content_search, build_direct_content_search,
    build_search_request, clan_channel_scope, content_filter, direct_channel_scope, filter,
    has_filter, mention_user_filter, username_filter,
};
pub use session::Session;
pub use transport::MezonTransport;
pub use transport::RealtimeEvent;
pub use transport::{
    ApiCategoryDesc, ApiChannelApp, ApiChannelAttachment, ApiChannelDesc, ApiPinMessage,
    ApiThreadDesc, ApiVoiceChannelUser,
};
pub use transport_adapter::TransportAdapter;
pub use transport_runtime::TransportClient;

/// Default realtime socket host (`ws_url` when the server omits one).
pub const DEFAULT_WS_HOST: &str = "sock.mezon.ai";
/// TLS port for bare-hostname prod endpoints (not shown in logs — see [`socket_connect_label`]).
pub const DEFAULT_WS_TLS_PORT: u16 = 443;
pub const DEFAULT_WS_SECURE: bool = true;

/// Log/UX label for a realtime connect target (prod: hostname only, dev: `host:7349`).
pub fn socket_connect_label(host: &str, explicit_port: Option<u16>) -> String {
    match explicit_port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// TCP/TLS port for the underlying socket (443 when the endpoint is hostname-only).
pub fn resolve_connect_port(explicit_port: Option<u16>) -> u16 {
    explicit_port.unwrap_or(DEFAULT_WS_TLS_PORT)
}
