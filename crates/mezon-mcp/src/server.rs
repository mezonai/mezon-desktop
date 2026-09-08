use crate::http_server::HttpMcpService;
use crate::protocol::{McpStartResult, McpStatus, mcp_url};
use crate::state;
use crate::tools::McpBackend;
use anyhow::Context as _;
use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use futures::channel::mpsc::UnboundedSender;
use mezon_client::AppApi;
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct ControllerInner {
    api: Option<Arc<AppApi>>,
    ui_tx: Option<UnboundedSender<crate::command::McpCommand>>,
    status: McpStatus,
    cancel: Option<CancellationToken>,
    server_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct McpController {
    inner: Arc<Mutex<ControllerInner>>,
    preferred_port: AtomicU16,
}

impl McpController {
    pub fn new() -> Self {
        Self {
            preferred_port: AtomicU16::new(0),
            inner: Arc::new(Mutex::new(ControllerInner {
                api: None,
                ui_tx: None,
                status: McpStatus::stopped(),
                cancel: None,
                server_task: None,
            })),
        }
    }

    pub async fn status(&self) -> McpStatus {
        self.inner.lock().await.status.clone()
    }

    pub fn set_preferred_port(&self, port: u16) {
        self.preferred_port.store(port, Ordering::Relaxed);
    }

    pub async fn set_backend(
        &self,
        api: Arc<AppApi>,
        ui_tx: UnboundedSender<crate::command::McpCommand>,
    ) {
        let mut inner = self.inner.lock().await;
        inner.api = Some(api);
        inner.ui_tx = Some(ui_tx);
    }

    pub async fn start(
        &self,
        read_only: bool,
        port: Option<u16>,
    ) -> anyhow::Result<McpStartResult> {
        let mut inner = self.inner.lock().await;
        if inner.status.running {
            anyhow::bail!("MCP server is already running");
        }
        let api = inner
            .api
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP backend is not initialized"))?;
        let ui_tx = inner.ui_tx.clone();
        let backend = McpBackend::new(api, ui_tx, read_only);

        let cancel = CancellationToken::new();
        let requested = port.unwrap_or_else(|| self.preferred_port.load(Ordering::Relaxed));
        let requested = (requested != 0).then_some(requested);
        let listener = bind_listener(requested).await?;
        let bound_port = listener
            .local_addr()
            .context("Reading MCP listener address")?
            .port();
        let url = mcp_url(bound_port);

        let cancel_for_task = cancel.clone();
        let service = StreamableHttpService::new(
            move || Ok(HttpMcpService::new(backend.clone())),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_sse_keep_alive(None)
                .with_cancellation_token(cancel.child_token()),
        );
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn(enforce_local_origin));
        let server_task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    cancel_for_task.cancelled_owned().await;
                })
                .await
            {
                tracing::error!("MCP HTTP server exited with error: {e}");
            }
        });

        let status = McpStatus {
            running: true,
            port: Some(bound_port),
            read_only,
            url: Some(url.clone()),
        };
        if let Err(e) = state::write_state(&status) {
            tracing::warn!("Failed to persist MCP state: {e}");
        }

        inner.status = status;
        inner.cancel = Some(cancel);
        inner.server_task = Some(server_task);

        Ok(McpStartResult {
            port: bound_port,
            url,
            read_only,
        })
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        if !inner.status.running {
            return Ok(());
        }
        if let Some(cancel) = inner.cancel.take() {
            cancel.cancel();
        }
        if let Some(task) = inner.server_task.take() {
            task.abort();
            let _ = task.await;
        }
        inner.status = McpStatus::stopped();
        state::clear_state();
        Ok(())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        let inner = self.inner.lock().await;
        let api = inner
            .api
            .clone()
            .ok_or_else(|| anyhow::anyhow!("MCP backend is not initialized"))?;
        let backend = McpBackend::new(api, inner.ui_tx.clone(), inner.status.read_only);
        backend.call_tool(name, arguments).await
    }
}

async fn bind_listener(port: Option<u16>) -> anyhow::Result<tokio::net::TcpListener> {
    if let Some(port) = port {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) => tracing::warn!("MCP port {port} is unavailable ({e}); using a free port"),
        }
    }
    tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("Binding MCP HTTP listener")
}

async fn enforce_local_origin(
    headers: HeaderMap,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if !origin_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}

fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}

impl Default for McpController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::bind_listener;

    #[tokio::test]
    async fn a_named_port_is_honoured_so_the_url_does_not_move() {
        for attempt in 1..=5 {
            let listener = bind_listener(Some(0)).await.expect("bind any port");
            let port = listener.local_addr().expect("addr").port();
            drop(listener);

            let listener = bind_listener(Some(port)).await.expect("bind named port");
            let bound = listener.local_addr().expect("addr").port();
            if bound == port {
                return;
            }
            assert!(attempt < 5, "port {port} was taken on every attempt");
        }
    }

    #[tokio::test]
    async fn a_taken_port_falls_back_instead_of_leaving_the_app_without_a_server() {
        let held = bind_listener(Some(0)).await.expect("bind any port");
        let taken = held.local_addr().expect("addr").port();

        let listener = bind_listener(Some(taken)).await.expect("fall back");
        assert_ne!(listener.local_addr().expect("addr").port(), taken);
    }
}
