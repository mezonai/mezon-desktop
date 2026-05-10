//! Transport runtime wrapper with dedicated tokio runtime.
//!
//! Similar to how `ReqwestClient` manages its own tokio runtime via `static OnceLock<Runtime>`,
//! this allows transport operations to work when called from GPUI's smol-based executor.

use crate::abridged_tcp_adapter::AbridgedTcpAdapter;
use crate::transport::MezonTransport;
use anyhow::Result;
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

/// Transport client wrapper that spawns all operations on a dedicated tokio runtime.
///
/// This allows transport operations (TCP connections, async I/O) to work correctly
/// when called from GPUI's smol-based executor, without requiring a tokio context
/// at the call site.
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
                tracing::debug!("🔧 Inside transport runtime, calling MezonTransport::connect()...");
                let result = transport
                    .connect(&host, port, &token, on_message, on_disconnected)
                    .await;
                
                match &result {
                    Ok(_) => tracing::debug!("✓ MezonTransport::connect() succeeded in runtime"),
                    Err(e) => tracing::error!("✗ MezonTransport::connect() failed in runtime: {}", e),
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
}
