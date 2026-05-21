//! Transport runtime wrapper with dedicated tokio runtime.
//!
//! Similar to how `ReqwestClient` manages its own tokio runtime via `static OnceLock<Runtime>`,
//! this allows transport operations to work when called from GPUI's smol-based executor.

use crate::abridged_tcp_adapter::AbridgedTcpAdapter;
use crate::transport::MezonTransport;
use anyhow::Result;
use http_client::{AsyncBody, HttpClient, http};
use reqwest_client::ReqwestClient;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

static TRANSPORT_RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Get or create the shared transport runtime.
fn runtime() -> &'static Runtime {
    TRANSPORT_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2) // Small dedicated pool for transport
            .thread_name("mezon-transport")
            .build()
            .expect("Failed to build transport runtime")
    })
}

/// HTTP PUT bytes to a pre-signed URL (e.g., S3 upload URL from upload_attachment_file).
pub async fn put_bytes_to_url(url: &str, data: Vec<u8>) -> Result<()> {
    let client = ReqwestClient::new();
    let request = http::Request::builder()
        .method(http::Method::PUT)
        .uri(url)
        .header("Content-Type", "application/octet-stream")
        .body(AsyncBody::from(data))?;
    let response = client.send(request).await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP PUT failed with status {}", status);
    }
    Ok(())
}

/// Transport client wrapper that spawns all operations on a dedicated tokio runtime.
///
/// This allows transport operations (TCP connections, async I/O) to work correctly
/// when called from GPUI's smol-based executor, without requiring a tokio context
/// at the call site.
#[derive(Clone)]
pub struct TransportClient {
    inner: std::sync::Arc<MezonTransport>,
}

impl TransportClient {
    /// Create a new transport client with the given base API path.
    pub fn new(base_path: String) -> Self {
        let adapter = Box::new(AbridgedTcpAdapter::new());
        let transport = MezonTransport::new(adapter, base_path);
        Self {
            inner: std::sync::Arc::new(transport),
        }
    }

    /// Connect to the Mezon backend.
    ///
    /// Spawns the connection task on the dedicated transport runtime.
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        token: &str,
        on_message: impl Fn(u16, u32, Vec<u8>) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::info!("🚀 TransportClient::connect() starting");
        tracing::debug!("  Spawning connection task on dedicated transport runtime...");

        let transport = self.inner.clone();
        let host = host.to_string();
        let token = token.to_string();

        runtime()
            .spawn(async move {
                tracing::debug!(
                    "🔧 Inside transport runtime, calling MezonTransport::connect()..."
                );
                let result = transport
                    .connect(&host, port, &token, on_message, on_disconnected)
                    .await;

                match &result {
                    Ok(_) => tracing::debug!("✓ MezonTransport::connect() succeeded in runtime"),
                    Err(e) => {
                        tracing::error!("✗ MezonTransport::connect() failed in runtime: {}", e)
                    }
                }

                result
            })
            .await
            .expect("Transport task panicked")?;

        tracing::info!("✅ TransportClient::connect() completed");
        Ok(())
    }

    /// Get account data.
    ///
    /// Spawns the API call on the dedicated transport runtime.
    pub async fn get_account(&self) -> Result<crate::transport::ApiAccount> {
        tracing::info!("📞 TransportClient::get_account() called");

        let transport = self.inner.clone();

        tracing::debug!("  Spawning on transport runtime...");
        let result = runtime()
            .spawn(async move {
                tracing::debug!("  Inside transport runtime task");
                transport.get_account().await
            })
            .await
            .expect("Transport task panicked");

        tracing::debug!("  Transport runtime task completed");
        result
    }

    /// List channel descriptions over the shared transport.
    pub async fn list_channel_descs(
        &self,
        clan_id: &str,
    ) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        tracing::info!("📞 TransportClient::list_channel_descs() called");

        let transport = self.inner.clone();
        let clan_id = clan_id.to_string();

        runtime()
            .spawn(async move { transport.list_channel_descs(&clan_id).await })
            .await
            .expect("Transport task panicked")
    }

    /// List clan descriptions over the shared transport.
    pub async fn list_clan_descs(&self) -> Result<Vec<crate::transport::ApiClanDesc>> {
        tracing::info!("📞 TransportClient::list_clan_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_clan_descs().await })
            .await
            .expect("Transport task panicked")
    }

    /// Create a new clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<crate::transport::ApiClanDesc> {
        tracing::info!("📞 TransportClient::create_clan_desc() called");

        let transport = self.inner.clone();
        let clan_name = clan_name.to_string();
        let logo = logo.to_string();
        let banner = banner.to_string();

        runtime()
            .spawn(async move { transport.create_clan_desc(&clan_name, &logo, &banner).await })
            .await
            .expect("Transport task panicked")
    }

    /// Ping server and wait for pong.
    pub async fn ping_roundtrip(&self) -> Result<()> {
        tracing::info!("🏓 TransportClient::ping_roundtrip() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.ping_roundtrip().await })
            .await
            .expect("Transport task panicked")
    }

    /// Check if the connection is open.
    pub async fn is_open(&self) -> bool {
        self.inner.is_open().await
    }

    /// Close the connection.
    ///
    /// Spawns the close operation on the dedicated transport runtime.
    pub async fn close(&self) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.close().await })
            .await
            .expect("Transport task panicked")?;

        Ok(())
    }

    /// Update user profile (display name, avatar URL).
    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let transport = self.inner.clone();
        let display_name = display_name.to_string();
        let avatar_url = avatar_url.to_string();

        runtime()
            .spawn(async move { transport.update_user(&display_name, &avatar_url).await })
            .await
            .expect("Transport task panicked")
    }

    /// List currently logged-in devices.
    pub async fn list_loged_device(&self) -> Result<Vec<mezon_proto::api::LogedDevice>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_loged_device().await.map(|l| l.devices) })
            .await
            .expect("Transport task panicked")
    }

    /// Update account profile (display name, avatar URL, about me).
    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let display_name = display_name.map(str::to_string);
        let avatar_url = avatar_url.map(str::to_string);
        let about_me = about_me.map(str::to_string);

        runtime()
            .spawn(async move {
                transport
                    .update_account(
                        display_name.as_deref(),
                        avatar_url.as_deref(),
                        about_me.as_deref(),
                    )
                    .await
            })
            .await
            .expect("Transport task panicked")
    }

    /// Upload an attachment file (used for avatar upload).
    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        let transport = self.inner.clone();
        let filename = filename.to_string();
        let filetype = filetype.to_string();

        runtime()
            .spawn(async move { transport.upload_attachment_file(&filename, &filetype, size).await })
            .await
            .expect("Transport task panicked")
    }

    /// Log out the current session.
    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let transport = self.inner.clone();
        let token = token.to_string();
        let refresh_token = refresh_token.to_string();

        runtime()
            .spawn(async move { transport.session_logout(&token, &refresh_token).await })
            .await
            .expect("Transport task panicked")
    }

    pub async fn logout_device(&self, token: &str, refresh_token: &str, device_id: &str) -> Result<()> {
        let transport = self.inner.clone();
        let token = token.to_string();
        let refresh_token = refresh_token.to_string();
        let device_id = device_id.to_string();

        runtime()
            .spawn(async move {
                transport
                    .logout_device(&token, &refresh_token, &device_id)
                    .await
            })
            .await
            .expect("Transport task panicked")
    }
}
