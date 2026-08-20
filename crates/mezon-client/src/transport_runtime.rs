//! Transport runtime wrapper with dedicated tokio runtime.
//!
//! Similar to how `ReqwestClient` manages its own tokio runtime via `static OnceLock<Runtime>`,
//! this allows transport operations to work when called from GPUI's smol-based executor.

use crate::abridged_tcp_adapter::AbridgedTcpAdapter;
use crate::transport::{MezonTransport, UpdateChannelDescParams};
use anyhow::Result;
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, http};
use reqwest_client::ReqwestClient;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt as _;
use tokio::runtime::Runtime;

static TRANSPORT_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HTTP_CLIENT: OnceLock<Arc<ReqwestClient>> = OnceLock::new();

const HTTP_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

fn shared_http_client() -> &'static Arc<ReqwestClient> {
    HTTP_CLIENT.get_or_init(|| Arc::new(new_http_client()))
}

pub(crate) fn http_client() -> &'static ReqwestClient {
    shared_http_client()
}

pub fn http_client_arc() -> Arc<dyn HttpClient> {
    shared_http_client().clone()
}

pub fn new_http_client() -> ReqwestClient {
    let _guard = runtime().enter();
    ReqwestClient::new()
}

pub fn new_http_client_with_user_agent(agent: &str) -> ReqwestClient {
    let _guard = runtime().enter();
    ReqwestClient::user_agent(agent).unwrap_or_else(|_| ReqwestClient::new())
}

pub(crate) fn runtime() -> &'static Runtime {
    TRANSPORT_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("mezon-transport")
            .build()
            .expect("Failed to build transport runtime")
    })
}

pub fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}

fn parse_optional_id(value: &str, field: &str) -> Result<i64> {
    if value.is_empty() {
        return Ok(0);
    }
    value
        .parse::<i64>()
        .map_err(|e| anyhow::anyhow!("invalid {field}: {e}"))
}

pub async fn put_bytes_to_url(url: &str, data: Vec<u8>) -> Result<()> {
    put_bytes_to_content_type(url, data, "application/octet-stream").await
}

pub async fn put_bytes_to_content_type(url: &str, data: Vec<u8>, content_type: &str) -> Result<()> {
    tracing::debug!("put_bytes_to_content_type: PUTting {} bytes", data.len());
    let url = url.to_string();
    let content_type = content_type.to_string();
    runtime()
        .spawn(async move {
            let request = http::Request::builder()
                .method(http::Method::PUT)
                .uri(&url)
                .header("Content-Type", content_type)
                .body(AsyncBody::from(data))?;
            let response = match tokio::time::timeout(
                HTTP_TRANSFER_TIMEOUT,
                http_client().send(request),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => anyhow::bail!(
                    "HTTP PUT timed out after {}s",
                    HTTP_TRANSFER_TIMEOUT.as_secs()
                ),
            };
            let status = response.status();
            tracing::debug!("put_bytes_to_content_type: response status={}", status);
            if !status.is_success() {
                tracing::error!(
                    "put_bytes_to_content_type: HTTP PUT failed with status {}",
                    status
                );
                anyhow::bail!("HTTP PUT failed with status {}", status);
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("upload task failed: {e}"))?
}

const MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

pub async fn fetch_bytes(url: &str) -> Result<(Vec<u8>, Option<String>)> {
    let url = url.to_string();
    runtime()
        .spawn(async move {
            let request = http::Request::builder()
                .method(http::Method::GET)
                .uri(&url)
                .body(AsyncBody::empty())?;
            match tokio::time::timeout(HTTP_TRANSFER_TIMEOUT, async move {
                let mut response = http_client().send(request).await?;
                let status = response.status();
                if !status.is_success() {
                    anyhow::bail!("HTTP GET failed with status {}", status);
                }
                let content_type = response
                    .headers()
                    .get(http::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let mut bytes: Vec<u8> = Vec::new();
                let mut limited = response.body_mut().take(MAX_FETCH_BYTES + 1);
                limited.read_to_end(&mut bytes).await?;
                if bytes.len() as u64 > MAX_FETCH_BYTES {
                    anyhow::bail!("response exceeds {MAX_FETCH_BYTES}-byte cap");
                }
                Ok((bytes, content_type))
            })
            .await
            {
                Ok(result) => result,
                Err(_) => anyhow::bail!(
                    "HTTP GET timed out after {}s",
                    HTTP_TRANSFER_TIMEOUT.as_secs()
                ),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("fetch task failed: {e}"))?
}

const NOTIFICATION_ICON_MAX_PX: u32 = 64;
const NOTIFICATION_ICON_KEEP: usize = 8;
const NOTIFICATION_ICON_DECODE_MAX_PX: u32 = 2048;

fn retire_temp_icon(path: std::path::PathBuf) {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    static RECENT: Mutex<VecDeque<std::path::PathBuf>> = Mutex::new(VecDeque::new());
    let Ok(mut recent) = RECENT.lock() else {
        return;
    };
    recent.push_back(path);
    while recent.len() > NOTIFICATION_ICON_KEEP {
        if let Some(stale) = recent.pop_front() {
            let _ = std::fs::remove_file(&stale);
        }
    }
}

fn notification_icon_limits() -> image::Limits {
    let max_px = NOTIFICATION_ICON_DECODE_MAX_PX;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_px);
    limits.max_image_height = Some(max_px);
    limits.max_alloc = Some(max_px as u64 * max_px as u64 * 4);
    limits
}

fn shrink_icon_to_png(bytes: &[u8]) -> Result<Vec<u8>> {
    use image::ImageEncoder as _;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(notification_icon_limits());
    let decoded = reader.decode()?;
    let oversized = decoded.width().max(decoded.height()) > NOTIFICATION_ICON_MAX_PX;
    let image = if oversized {
        decoded.thumbnail(NOTIFICATION_ICON_MAX_PX, NOTIFICATION_ICON_MAX_PX)
    } else {
        decoded
    }
    .to_rgba8();
    let (width, height) = image.dimensions();
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(
        image.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Write bytes to a uniquely-named temp file for use as a notification icon
/// attachment. The OS notification system copies the file into its own store, so
/// the returned path is safe to delete afterwards.
pub async fn write_temp_icon(bytes: Vec<u8>) -> Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("mezon-noti-icon-{}-{seq}.png", std::process::id()));
    runtime()
        .spawn_blocking(move || {
            let encoded = shrink_icon_to_png(&bytes)?;
            std::fs::write(&path, &encoded)?;
            retire_temp_icon(path.clone());
            Ok(path)
        })
        .await
        .map_err(|e| anyhow::anyhow!("icon write task failed: {e}"))?
}

pub async fn read_file(path: std::path::PathBuf) -> Result<Vec<u8>> {
    runtime()
        .spawn_blocking(move || std::fs::read(&path))
        .await
        .map_err(|e| anyhow::anyhow!("file read task failed: {e}"))?
        .map_err(Into::into)
}

pub async fn file_len(path: std::path::PathBuf) -> Result<u64> {
    runtime()
        .spawn_blocking(move || std::fs::metadata(&path).map(|m| m.len()))
        .await
        .map_err(|e| anyhow::anyhow!("file metadata task failed: {e}"))?
        .map_err(Into::into)
}

pub async fn read_file_range(path: std::path::PathBuf, offset: u64, len: usize) -> Result<Vec<u8>> {
    runtime()
        .spawn_blocking(move || {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = vec![0u8; len];
            file.read_exact(&mut buf)?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        })
        .await
        .map_err(|e| anyhow::anyhow!("file range read task failed: {e}"))?
        .map_err(Into::into)
}

pub async fn put_bytes_return_etag(url: &str, data: Vec<u8>, content_type: &str) -> Result<String> {
    let url = url.to_string();
    let content_type = content_type.to_string();
    runtime()
        .spawn(async move {
            let request = http::Request::builder()
                .method(http::Method::PUT)
                .uri(&url)
                .header("Content-Type", content_type)
                .body(AsyncBody::from(data))?;
            let response = match tokio::time::timeout(
                HTTP_TRANSFER_TIMEOUT,
                http_client().send(request),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => anyhow::bail!(
                    "HTTP PUT part timed out after {}s",
                    HTTP_TRANSFER_TIMEOUT.as_secs()
                ),
            };
            let status = response.status();
            if !status.is_success() {
                anyhow::bail!("HTTP PUT part failed with status {}", status);
            }
            response
                .headers()
                .get(http::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("multipart part response missing ETag header"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("upload part task failed: {e}"))?
}

pub async fn download_to(
    url: &str,
    dest: std::path::PathBuf,
    on_progress: impl Fn(u64, Option<u64>) + Send + 'static,
) -> Result<()> {
    let url = url.to_string();
    runtime()
        .spawn(async move {
            let outcome = stream_to_file(&url, &dest, on_progress).await;
            if outcome.is_err() {
                let _ = tokio::fs::remove_file(&dest).await;
            }
            outcome
        })
        .await
        .map_err(|e| anyhow::anyhow!("download task failed: {e}"))?
}

async fn stream_to_file(
    url: &str,
    dest: &std::path::Path,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(url)
        .body(AsyncBody::empty())?;
    let mut response =
        match tokio::time::timeout(HTTP_TRANSFER_TIMEOUT, http_client().send(request)).await {
            Ok(result) => result?,
            Err(_) => anyhow::bail!(
                "download request timed out after {}s",
                HTTP_TRANSFER_TIMEOUT.as_secs()
            ),
        };
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP GET failed with status {status}");
    }
    let total = response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|len| *len > 0);
    let mut file = tokio::fs::File::create(dest).await?;
    let body = response.body_mut();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut reported: u64 = 0;
    on_progress(0, total);
    loop {
        let read = tokio::time::timeout(HTTP_TRANSFER_TIMEOUT, body.read(&mut buffer)).await;
        let count = match read {
            Ok(result) => result?,
            Err(_) => anyhow::bail!(
                "download stalled (no data for {}s)",
                HTTP_TRANSFER_TIMEOUT.as_secs()
            ),
        };
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count]).await?;
        written += count as u64;
        // Throttle to ~1% steps (known size) / 256 KB (unknown) to avoid flooding the UI.
        let step = match total {
            Some(t) => written * 100 / t != reported * 100 / t,
            None => written - reported >= 256 * 1024,
        };
        if step {
            reported = written;
            on_progress(written, total);
        }
    }
    file.flush().await?;
    on_progress(written, total.or(Some(written)));
    Ok(())
}

#[derive(Clone)]
pub struct TransportClient {
    inner: std::sync::Arc<MezonTransport>,
}

impl TransportClient {
    pub async fn send_channel_message_structured(
        &self,
        channel_id: i64,
        content_json: &str,
        mode: i32,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content_json = content_json.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_structured(channel_id, &content_json, mode)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn send_channel_message_structured_with_code(
        &self,
        channel_id: i64,
        content_json: &str,
        mode: i32,
        message_code: i32,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content_json = content_json.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_structured_with_code(
                        channel_id,
                        &content_json,
                        mode,
                        message_code,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }
    pub fn new(base_path: String) -> Self {
        let adapter = Box::new(AbridgedTcpAdapter::new());
        let transport = MezonTransport::new(adapter, base_path);
        Self {
            inner: std::sync::Arc::new(transport),
        }
    }

    pub fn set_http_fallback(&self, fallback: Option<crate::transport::HttpFallbackSession>) {
        self.inner.set_http_fallback(fallback);
    }

    pub fn frames_received(&self) -> u64 {
        self.inner.frames_received()
    }

    pub fn credential_rejected(&self) -> bool {
        self.inner.credential_rejected()
    }

    pub async fn renew_fallback_token(&self) -> Result<crate::transport::RenewedTokens> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.renew_fallback_token().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub fn renewed_tokens(
        &self,
    ) -> tokio::sync::watch::Receiver<Option<crate::transport::RenewedTokens>> {
        self.inner.renewed_tokens()
    }

    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        token: &str,
        on_event: impl Fn(crate::transport::RealtimeEvent) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::debug!("TransportClient::connect() starting");
        tracing::debug!("  Spawning connection task on dedicated transport runtime...");

        let transport = self.inner.clone();
        let host = host.to_string();
        let token = token.to_string();

        runtime()
            .spawn(async move {
                tracing::debug!("Inside transport runtime, calling MezonTransport::connect()...");
                let result = transport
                    .connect(&host, port, &token, on_event, on_disconnected)
                    .await;

                match &result {
                    Ok(_) => tracing::debug!("MezonTransport::connect() succeeded in runtime"),
                    Err(e) => {
                        tracing::error!("MezonTransport::connect() failed in runtime: {}", e)
                    }
                }

                result
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))??;

        tracing::debug!("TransportClient::connect() completed");
        Ok(())
    }

    pub async fn get_account(&self) -> Result<crate::transport::ApiAccount> {
        tracing::debug!("TransportClient::get_account() called");

        let transport = self.inner.clone();

        tracing::debug!("  Spawning on transport runtime...");
        let result = runtime()
            .spawn(async move {
                tracing::debug!("  Inside transport runtime task");
                transport.get_account().await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?;

        tracing::debug!("  Transport runtime task completed");
        result
    }

    pub async fn list_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        tracing::debug!("TransportClient::list_channel_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_channel_descs(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_detail(
        &self,
        channel_id: i64,
    ) -> Result<crate::transport::ApiChannelDesc> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_channel_detail(channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_archived_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ListArchivedChannelDescsResponse> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_archived_channel_descs(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_by_user_id(&self) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        tracing::debug!("TransportClient::list_channel_by_user_id() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_channel_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_dm_channel_descs(
        &self,
        page: i32,
    ) -> Result<Vec<crate::transport::ApiDirectChannel>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_dm_channel_descs(page).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .mark_as_read(channel_id, category_id, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_user_status(
        &self,
        status: String,
        minutes: i32,
        until_turn_on: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .update_user_status(&status, minutes, until_turn_on)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_user_custom_status(
        &self,
        status: String,
        minutes: i32,
        until_turn_on: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .update_user_custom_status(&status, minutes, until_turn_on)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_categories_typed(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiCategoryDesc>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_categories_typed(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_badge_counts(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelDesc>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_badge_counts(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_voice_channel_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiVoiceChannelUser>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_voice_channel_users(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_streaming_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        state: i32,
        limit: i32,
    ) -> Result<mezon_proto::api::StreamingChannelUserList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_streaming_channel_users(clan_id, channel_id, channel_type, state, limit)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::ClanUserList>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_users(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn remove_clan_users(&self, clan_id: i64, user_ids: Vec<String>) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport.remove_clan_users(clan_id, &refs).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn ban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: Vec<String>,
        ban_time: i32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport
                    .ban_clan_users(clan_id, channel_id, &refs, ban_time)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn unban_clan_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        user_ids: Vec<String>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport.unban_clan_users(clan_id, channel_id, &refs).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_banned_users(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::BannedUserList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_banned_users(clan_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_emojis_by_user_id(&self) -> Result<mezon_proto::api::EmojiListedResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_emojis_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn emoji_recent_list(&self) -> Result<mezon_proto::api::EmojiRecentList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.emoji_recent_list().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_stickers_by_user_id(
        &self,
    ) -> Result<mezon_proto::api::StickerListedResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_stickers_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_clan_emoji(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
        category: &str,
        id: i64,
        is_for_sale: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let source = source.to_string();
        let shortname = shortname.to_string();
        let category = category.to_string();
        runtime()
            .spawn(async move {
                transport
                    .create_clan_emoji(clan_id, &source, &shortname, &category, id, is_for_sale)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_clan_emoji_by_id(
        &self,
        id: i64,
        shortname: &str,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let shortname = shortname.to_string();
        runtime()
            .spawn(async move {
                transport
                    .update_clan_emoji_by_id(id, &shortname, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_clan_emoji_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_clan_emoji_by_id(id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_clan_sticker(
        &self,
        clan_id: i64,
        source: &str,
        shortname: &str,
        category: &str,
        id: i64,
        media_type: i32,
        is_for_sale: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let source = source.to_string();
        let shortname = shortname.to_string();
        let category = category.to_string();
        runtime()
            .spawn(async move {
                transport
                    .add_clan_sticker(
                        clan_id,
                        &source,
                        &shortname,
                        &category,
                        id,
                        media_type,
                        is_for_sale,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_clan_sticker_by_id(
        &self,
        id: i64,
        clan_id: i64,
        source: &str,
        shortname: &str,
        category: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let source = source.to_string();
        let shortname = shortname.to_string();
        let category = category.to_string();
        runtime()
            .spawn(async move {
                transport
                    .update_clan_sticker_by_id(id, clan_id, &source, &shortname, &category)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_clan_sticker_by_id(id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<mezon_proto::api::ChannelUserList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_channel_users(clan_id, channel_id, channel_type)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_user_clans_by_user_id(&self) -> Result<mezon_proto::api::AllUserClans> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_user_clans_by_user_id().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<mezon_proto::api::AllUsersAddChannelResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_users_uc(channel_id, limit).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn generate_meet_token(&self, channel_id: &str, room_name: &str) -> Result<String> {
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let room_name = room_name.to_string();
        runtime()
            .spawn(async move {
                transport
                    .generate_meet_token(channel_id, &room_name)
                    .await
                    .map(|resp| resp.token)
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn write_voice_reaction(&self, emojis: Vec<String>, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.write_voice_reaction(emojis, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn forward_webrtc_signaling(
        &self,
        receiver_id: i64,
        data_type: i32,
        json_data: String,
        channel_id: i64,
        caller_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .forward_webrtc_signaling(
                        receiver_id,
                        data_type,
                        json_data,
                        channel_id,
                        caller_id,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn make_call_push(
        &self,
        receiver_id: i64,
        json_data: String,
        channel_id: i64,
        caller_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .make_call_push(receiver_id, json_data, channel_id, caller_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_channel_message_structured(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content_json: String,
        mode: i32,
        create_time_seconds: u32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_message_structured(
                        clan_id,
                        channel_id,
                        message_id,
                        content_json,
                        mode,
                        create_time_seconds,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn write_voice_interactive(
        &self,
        clan_id: i64,
        voice_channel_id: i64,
        sender_id: i64,
        receiver_id: i64,
        event_type: i32,
        params: String,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .write_voice_interactive(
                        clan_id,
                        voice_channel_id,
                        sender_id,
                        receiver_id,
                        event_type,
                        params,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn remove_participant_mezon_meet(
        &self,
        channel_id: &str,
        clan_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let clan_id = parse_optional_id(clan_id, "clan_id")?;
        let room_name = room_name.to_string();
        let username = username.to_string();
        runtime()
            .spawn(async move {
                transport
                    .remove_participant_mezon_meet(channel_id, clan_id, &room_name, &username)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn mute_participant_mezon_meet(
        &self,
        channel_id: &str,
        clan_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let clan_id = parse_optional_id(clan_id, "clan_id")?;
        let room_name = room_name.to_string();
        let username = username.to_string();
        runtime()
            .spawn(async move {
                transport
                    .mute_participant_mezon_meet(channel_id, clan_id, &room_name, &username)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn add_agent_to_channel(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let transport = self.inner.clone();
        let room_name = room_name.to_string();
        runtime()
            .spawn(async move { transport.add_agent_to_channel(channel_id, &room_name).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn disconnect_agent(&self, channel_id: i64, room_name: &str) -> Result<()> {
        let transport = self.inner.clone();
        let room_name = room_name.to_string();
        runtime()
            .spawn(async move { transport.disconnect_agent(channel_id, &room_name).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_badge_count(&self) -> Result<Vec<(String, i32, bool)>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_badge_count_typed().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_notification_clan(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_notification_category(
        &self,
        category_id: i64,
    ) -> Result<mezon_proto::api::NotificationUserChannel> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_notification_category(category_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_notification_channel(
        &self,
        channel_id: i64,
    ) -> Result<mezon_proto::api::NotificationUserChannel> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_notification_channel(channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_notification_channel_setting(
        &self,
        channel_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_notification_channel_setting(channel_id, notification_type, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_notification_channel(&self, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_notification_channel(channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_mute_channel(
        &self,
        channel_id: i64,
        mute_seconds: i32,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_mute_channel(channel_id, mute_seconds, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_notification_clan_setting(
        &self,
        clan_id: i64,
        notification_type: i32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_notification_clan_setting(clan_id, notification_type)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_notification_category_setting(
        &self,
        category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_notification_category_setting(category_id, notification_type, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_mute_category(
        &self,
        category_id: i64,
        mute_seconds: i32,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_mute_category(category_id, mute_seconds, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_notification_category_setting(&self, category_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .delete_notification_category_setting(category_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_channel_category_noti_settings_list(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::NotificationChannelCategorySettingList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .get_channel_category_noti_settings_list(clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub fn spawn_gotify_stream(
        &self,
        ws_base: String,
        token: String,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<crate::gotify::GotifyNotification>,
        tokio::sync::oneshot::Receiver<crate::gotify::StreamEnd>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (end_tx, end_rx) = tokio::sync::oneshot::channel();
        runtime().spawn(async move {
            let end = crate::gotify::run_once(&ws_base, &token, &tx).await;
            let _ = end_tx.send(end);
        });
        (rx, end_rx)
    }

    pub async fn regist_fcm_device_token(
        &self,
        token: String,
        device_id: String,
        platform: String,
    ) -> Result<(String, String)> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .regist_fcm_device_token(&token, &device_id, &platform)
                    .await
                    .map(|resp| (resp.token, resp.device_id))
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_muted_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_muted_channels(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<crate::transport::ApiClanDesc>> {
        tracing::debug!("TransportClient::list_clan_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_clan_descs().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_onboarding(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<mezon_proto::api::ListOnboardingResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_onboarding(clan_id, limit, page).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_onboarding(
        &self,
        clan_id: i64,
        contents: Vec<mezon_proto::api::OnboardingContent>,
    ) -> Result<mezon_proto::api::ListOnboardingResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.create_onboarding(clan_id, contents).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_onboarding(
        &self,
        id: i64,
        clan_id: i64,
        content: mezon_proto::api::OnboardingContent,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_onboarding(id, clan_id, content).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_onboarding(&self, id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_onboarding(id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Create a new clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<crate::transport::ApiClanDesc> {
        tracing::debug!("TransportClient::create_clan_desc() called");

        let transport = self.inner.clone();
        let clan_name = clan_name.to_string();
        let logo = logo.to_string();
        let banner = banner.to_string();

        runtime()
            .spawn(async move { transport.create_clan_desc(&clan_name, &logo, &banner).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_clan_desc(
        &self,
        request: mezon_proto::api::UpdateClanDescRequest,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_clan_desc(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_clan_desc(&self, clan_desc_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_clan_desc(clan_desc_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_system_message_by_clan_id(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::SystemMessage> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_system_message_by_clan_id(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_system_message(
        &self,
        request: mezon_proto::api::SystemMessageRequest,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_system_message(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_audit_log(
        &self,
        clan_id: i64,
        action_log: &str,
        user_id: Option<i64>,
        date_log: &str,
    ) -> Result<mezon_proto::api::ListAuditLog> {
        let transport = self.inner.clone();
        let action_log = action_log.to_string();
        let date_log = date_log.to_string();
        runtime()
            .spawn(async move {
                transport
                    .list_audit_log(clan_id, &action_log, user_id, &date_log)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        tracing::debug!("TransportClient::ping_roundtrip() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.ping_roundtrip().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn is_open(&self) -> bool {
        self.inner.is_open().await
    }

    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<crate::transport::ListChannelMessagesResult> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                transport
                    .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_topic_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        topic_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<crate::transport::ListChannelMessagesResult> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                transport
                    .list_topic_messages(
                        clan_id, channel_id, topic_id, message_id, direction, limit,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// List threads for a parent channel.
    pub async fn list_thread_descs(
        &self,
        channel_id: &str,
        clan_id: &str,
        page: i32,
    ) -> Result<Vec<crate::transport::ApiThreadDesc>> {
        use crate::transport::THREAD_LIST_LIMIT;
        let transport = self.inner.clone();
        let channel_id = channel_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid channel_id: {e}"))?;
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id: {e}"))?;

        runtime()
            .spawn(async move {
                transport
                    .list_thread_descs(channel_id, clan_id, THREAD_LIST_LIMIT, page, 0, None)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Get pin messages list for a channel.
    pub async fn get_pin_messages_list(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<Vec<crate::transport::ApiPinMessage>> {
        let transport = self.inner.clone();
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();

        runtime()
            .spawn(async move { transport.get_pin_messages_list(&channel_id, &clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_channel_canvas_list(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<Vec<crate::transport::ApiCanvas>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .get_channel_canvas_list(channel_id, clan_id, limit, page)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_channel_canvas_detail(
        &self,
        id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<crate::transport::ApiCanvasDetail> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .get_channel_canvas_detail(id, clan_id, channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_channel_canvas(
        &self,
        id: i64,
        channel_id: i64,
        clan_id: i64,
        title: &str,
        content: &str,
        is_default: bool,
        status: i32,
    ) -> Result<String> {
        let transport = self.inner.clone();
        let title = title.to_string();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .edit_channel_canvases(
                        id, channel_id, clan_id, &title, &content, is_default, status,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_channel_canvas(
        &self,
        canvas_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .delete_channel_canvas(canvas_id, clan_id, channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Pin a message.
    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                transport
                    .create_pin_message(message_id, channel_id, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Unpin (delete) a pinned message.
    pub async fn delete_pin_message(
        &self,
        id: &str,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let id = id.to_string();
        let message_id = message_id.to_string();
        let channel_id = channel_id.to_string();
        let clan_id = clan_id.to_string();

        runtime()
            .spawn(async move {
                transport
                    .delete_pin_message(&id, &message_id, &channel_id, &clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Search threads by label within a parent channel.
    pub async fn search_thread(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<Vec<crate::transport::ApiThreadDesc>> {
        let transport = self.inner.clone();
        let clan_id = clan_id.to_string();
        let channel_id = channel_id.to_string();
        let label = label.to_string();

        runtime()
            .spawn(async move { transport.search_thread(&clan_id, &channel_id, &label).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn search_message(
        &self,
        filters: Vec<mezon_proto::api::FilterParam>,
        from: i32,
        size: i32,
        sorts: Vec<mezon_proto::api::SortParam>,
    ) -> Result<mezon_proto::api::SearchMessageResponse> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.search_message(filters, from, size, sorts).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .join_chat(clan_id, channel_id, channel_type, is_public)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn react_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        emoji_id: i64,
        emoji: &str,
        count: i32,
        message_sender_id: i64,
        mode: i32,
        is_public: bool,
        remove: bool,
        topic_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let emoji = emoji.to_string();
        runtime()
            .spawn(async move {
                transport
                    .react_channel_message(
                        clan_id,
                        channel_id,
                        message_id,
                        emoji_id,
                        &emoji,
                        count,
                        message_sender_id,
                        mode,
                        is_public,
                        remove,
                        topic_id,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Update a channel message's content.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        mode: i32,
        is_public: bool,
        topic_id: i64,
        is_update_msg_topic: bool,
        hide_editted: bool,
        create_time_seconds: u32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_message(
                        clan_id,
                        channel_id,
                        message_id,
                        &content,
                        mentions,
                        hashtags,
                        emojis,
                        mode,
                        is_public,
                        topic_id,
                        is_update_msg_topic,
                        hide_editted,
                        create_time_seconds,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        mode: i32,
        is_public: bool,
        has_attachment: bool,
        topic_id: i64,
        has_mentions: bool,
        has_references: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .delete_channel_message(
                        clan_id,
                        channel_id,
                        message_id,
                        mode,
                        is_public,
                        has_attachment,
                        topic_id,
                        has_mentions,
                        has_references,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn join_clan_chat(&self, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.join_clan_chat(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn write_last_seen_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        mode: i32,
        timestamp_seconds: u32,
        badge_count: i32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .write_last_seen_message(
                        clan_id,
                        channel_id,
                        message_id,
                        mode,
                        timestamp_seconds,
                        badge_count,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_last_pin_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        mode: i32,
        is_public: bool,
        timestamp_seconds: u32,
        operation: i32,
        avatar: &str,
        sender_id: &str,
        sender_username: &str,
        content: &str,
        attachment: &str,
        created_time: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let avatar = avatar.to_string();
        let sender_id = sender_id.to_string();
        let sender_username = sender_username.to_string();
        let content = content.to_string();
        let attachment = attachment.to_string();
        let created_time = created_time.to_string();
        runtime()
            .spawn(async move {
                transport
                    .write_last_pin_message(
                        clan_id,
                        channel_id,
                        message_id,
                        mode,
                        is_public,
                        timestamp_seconds,
                        operation,
                        &avatar,
                        &sender_id,
                        &sender_username,
                        &content,
                        &attachment,
                        &created_time,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn report_message_abuse(&self, message_id: i64, abuse_type: &str) -> Result<()> {
        let transport = self.inner.clone();
        let abuse_type = abuse_type.to_string();
        runtime()
            .spawn(async move {
                transport
                    .report_message_abuse(message_id, &abuse_type)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_message_2_inbox(
        &self,
        request: mezon_proto::api::Message2InboxRequest,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.create_message_2_inbox(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_sd_topic(
        &self,
        message_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::SdTopic> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .create_sd_topic(message_id, clan_id, channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn message_button_click(
        &self,
        message_id: i64,
        channel_id: i64,
        button_id: &str,
        sender_id: i64,
        user_id: i64,
        extra_data: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let button_id = button_id.to_string();
        let extra_data = extra_data.to_string();
        runtime()
            .spawn(async move {
                transport
                    .message_button_click(
                        message_id,
                        channel_id,
                        &button_id,
                        sender_id,
                        user_id,
                        &extra_data,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn dropdown_box_selected(
        &self,
        message_id: i64,
        channel_id: i64,
        selectbox_id: &str,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let selectbox_id = selectbox_id.to_string();
        runtime()
            .spawn(async move {
                transport
                    .dropdown_box_selected(message_id, channel_id, &selectbox_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_ephemeral_message(
        &self,
        receiver_id: i64,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        reply: Option<crate::transport::OutgoingReply>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .write_ephemeral_message(
                        receiver_id,
                        clan_id,
                        channel_id,
                        &content,
                        is_public,
                        mode,
                        mentions,
                        hashtags,
                        emojis,
                        attachments,
                        reply,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn write_message_typing(
        &self,
        clan_id: i64,
        channel_id: i64,
        mode: i32,
        is_public: bool,
        sender_display_name: &str,
        topic_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let sender_display_name = sender_display_name.to_string();
        runtime()
            .spawn(async move {
                transport
                    .write_message_typing(
                        clan_id,
                        channel_id,
                        mode,
                        is_public,
                        &sender_display_name,
                        topic_id,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn write_quick_menu_event(
        &self,
        menu_name: &str,
        clan_id: i64,
        channel_id: i64,
        mode: i32,
        is_public: bool,
        content_json: &str,
        mentions: Vec<mezon_proto::api::MessageMention>,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        references: Vec<mezon_proto::api::MessageRef>,
        anonymous_message: bool,
        mention_everyone: bool,
        avatar: &str,
        message_code: i32,
        topic_id: i64,
        message_id: i64,
        message_sender_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let menu_name = menu_name.to_string();
        let content_json = content_json.to_string();
        let avatar = avatar.to_string();
        runtime()
            .spawn(async move {
                transport
                    .write_quick_menu_event(
                        &menu_name,
                        clan_id,
                        channel_id,
                        mode,
                        is_public,
                        &content_json,
                        mentions,
                        attachments,
                        references,
                        anonymous_message,
                        mention_everyone,
                        &avatar,
                        message_code,
                        topic_id,
                        message_id,
                        message_sender_id,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_quick_menu_access(
        &self,
        bot_id: i64,
        channel_id: i64,
        menu_type: i32,
    ) -> Result<mezon_proto::api::QuickMenuAccessList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_quick_menu_access(bot_id, channel_id, menu_type)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_with_flags(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        ogp: Option<crate::transport::OutgoingOgp>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_with_flags(
                        clan_id, channel_id, &content, is_public, mode, mentions, hashtags, emojis,
                        ogp, flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn send_channel_message_prebuilt(
        &self,
        clan_id: i64,
        channel_id: i64,
        content_json: &str,
        is_public: bool,
        mode: i32,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content_json = content_json.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_prebuilt(
                        clan_id,
                        channel_id,
                        &content_json,
                        is_public,
                        mode,
                        flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn forward_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content_raw: &str,
        text: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        mentions: Vec<crate::transport::OutgoingMention>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content_raw = content_raw.to_string();
        let text = text.to_string();
        runtime()
            .spawn(async move {
                transport
                    .forward_channel_message(
                        clan_id,
                        channel_id,
                        &content_raw,
                        &text,
                        is_public,
                        mode,
                        attachments,
                        mentions,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        mode: i32,
        is_public: bool,
        topic_id: i64,
        is_update_msg_topic: bool,
        create_time_seconds: u32,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_message_with_attachments(
                        clan_id,
                        channel_id,
                        message_id,
                        &content,
                        attachments,
                        mode,
                        is_public,
                        topic_id,
                        is_update_msg_topic,
                        create_time_seconds,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_users_status(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanUserStatusList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_users_status(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_user_online(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<mezon_proto::api::ListUserOnlineResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_user_online(clan_id, limit, page).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleListEventResponse> {
        let transport = self.inner.clone();
        let cursor = cursor.to_string();
        runtime()
            .spawn(async move { transport.list_roles(clan_id, limit, &cursor).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_events(&self, clan_id: i64) -> Result<mezon_proto::api::EventList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_events(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_event(
        &self,
        request: mezon_proto::api::CreateEventRequest,
    ) -> Result<mezon_proto::api::EventManagement> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.create_event(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_role_users(
        &self,
        role_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleUserList> {
        let transport = self.inner.clone();
        let cursor = cursor.to_string();
        runtime()
            .spawn(async move { transport.list_role_users(role_id, limit, &cursor).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_role(
        &self,
        request: mezon_proto::api::CreateRoleRequest,
    ) -> Result<mezon_proto::api::Role> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.create_role(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_role(&self, request: mezon_proto::api::UpdateRoleRequest) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_role(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_role(&self, role_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_role(role_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_role_order(&self, clan_id: i64, roles: &[(i32, i64)]) -> Result<()> {
        let transport = self.inner.clone();
        let roles = roles.to_vec();
        runtime()
            .spawn(async move { transport.update_role_order(clan_id, &roles).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_role_permissions(
        &self,
        role_id: i64,
    ) -> Result<mezon_proto::api::PermissionList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_role_permissions(role_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_list_permission(&self) -> Result<mezon_proto::api::PermissionList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_list_permission().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_permission_by_role_id_channel_id(
        &self,
        role_id: i64,
        channel_id: i64,
        user_id: i64,
    ) -> Result<mezon_proto::api::PermissionRoleChannelListEventResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .get_permission_by_role_id_channel_id(role_id, channel_id, user_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn set_role_channel_permission(
        &self,
        role_id: i64,
        channel_id: i64,
        user_id: i64,
        max_permission_id: i64,
        permission_update: Vec<mezon_proto::api::PermissionUpdate>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .set_role_channel_permission(
                        role_id,
                        channel_id,
                        user_id,
                        max_permission_id,
                        permission_update,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn add_roles_channel_desc(
        &self,
        role_ids: Vec<String>,
        channel_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = role_ids.iter().map(String::as_str).collect();
                transport.add_roles_channel_desc(&refs, channel_id).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_role_channel_desc(
        &self,
        role_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .delete_role_channel_desc(role_id, channel_id, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_channel_private(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_private: i32,
        user_ids: Vec<i64>,
        role_ids: Vec<i64>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_private(
                        clan_id,
                        channel_id,
                        channel_private,
                        user_ids,
                        role_ids,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn change_channel_category(
        &self,
        clan_id: i64,
        channel_id: i64,
        new_category_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .change_channel_category(clan_id, channel_id, new_category_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn remove_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport.remove_channel_users(channel_id, &refs).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_clan_user_role(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::RoleList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .get_role_of_user_in_clan(clan_id, channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_setting_page(
        &self,
        clan_id: i64,
        parent_id: i64,
        limit: i32,
        page: i32,
        channel_label: &str,
    ) -> Result<mezon_proto::api::ChannelSettingListResponse> {
        let transport = self.inner.clone();
        let channel_label = channel_label.to_string();
        runtime()
            .spawn(async move {
                transport
                    .list_channel_setting_page(clan_id, parent_id, limit, page, &channel_label)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        ogp: Option<crate::transport::OutgoingOgp>,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_channel_message(
                        clan_id, channel_id, &content, is_public, mode, mentions, hashtags, emojis,
                        ogp,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_topic_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        topic_id: i64,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        reply: Option<crate::transport::OutgoingReply>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_topic_message(
                        clan_id, channel_id, &content, is_public, mode, topic_id, mentions,
                        hashtags, emojis, reply, flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_topic_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        topic_id: i64,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        presign_finish: Option<Vec<String>>,
        reply: Option<crate::transport::OutgoingReply>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_topic_message_with_attachments(
                        clan_id,
                        channel_id,
                        &content,
                        is_public,
                        mode,
                        topic_id,
                        attachments,
                        mentions,
                        hashtags,
                        emojis,
                        presign_finish,
                        reply,
                        flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_reply(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        reply: crate::transport::OutgoingReply,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        ogp: Option<crate::transport::OutgoingOgp>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_reply(
                        clan_id, channel_id, &content, is_public, mode, reply, mentions, hashtags,
                        emojis, ogp, flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
        reply: Option<crate::transport::OutgoingReply>,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        presign_finish: Option<Vec<String>>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_with_attachments(
                        clan_id,
                        channel_id,
                        &content,
                        is_public,
                        mode,
                        attachments,
                        reply,
                        mentions,
                        hashtags,
                        emojis,
                        presign_finish,
                        flags,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn patch_message_presign_finish(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        mentions: Vec<crate::transport::OutgoingMention>,
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        presign_finish: Vec<String>,
        create_time_seconds: u32,
        mode: i32,
        is_public: bool,
        topic_id: i64,
        is_update_msg_topic: bool,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .patch_message_presign_finish(
                        clan_id,
                        channel_id,
                        message_id,
                        &content,
                        mentions,
                        hashtags,
                        emojis,
                        presign_finish,
                        create_time_seconds,
                        mode,
                        is_public,
                        topic_id,
                        is_update_msg_topic,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
        channel_private: i32,
    ) -> Result<crate::transport::ApiChannelDesc> {
        let transport = self.inner.clone();
        let channel_label = channel_label.to_string();

        runtime()
            .spawn(async move {
                transport
                    .create_channel(
                        clan_id,
                        &channel_label,
                        channel_type,
                        category_id,
                        parent_id,
                        channel_private,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_direct_channel(
        &self,
        user_ids: &[i64],
    ) -> Result<crate::transport::ApiChannelDesc> {
        let transport = self.inner.clone();
        let user_ids = user_ids.to_vec();

        runtime()
            .spawn(async move { transport.create_direct_channel(&user_ids).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_link_invite_user(
        &self,
        clan_id: i64,
        channel_id: i64,
        expiry_time: i32,
    ) -> Result<mezon_proto::api::LinkInviteUser> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .create_link_invite_user(clan_id, channel_id, expiry_time)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn invite_user(&self, invite_id: i64) -> Result<mezon_proto::api::InviteUserRes> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.invite_user(invite_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_friends(&self) -> Result<Vec<crate::transport::ApiFriend>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_friends().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_activity(&self) -> Result<mezon_proto::api::ListUserActivity> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_activity().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn add_friends(&self, ids: Vec<i64>, usernames: Vec<String>) -> Result<Vec<i64>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.add_friends(ids, usernames).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_friends(&self, ids: Vec<i64>, usernames: Vec<String>) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_friends(ids, usernames).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn block_friends(&self, ids: Vec<i64>) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.block_friends(ids).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn unblock_friends(&self, ids: Vec<i64>) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.unblock_friends(ids).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_category_desc(
        &self,
        category_name: &str,
        clan_id: i64,
    ) -> Result<mezon_proto::api::CategoryDesc> {
        let transport = self.inner.clone();
        let category_name = category_name.to_string();

        runtime()
            .spawn(async move {
                transport
                    .create_category_desc(&category_name, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_category(
        &self,
        category_id: i64,
        category_name: &str,
        clan_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let category_name = category_name.to_string();

        runtime()
            .spawn(async move {
                transport
                    .update_category(category_id, &category_name, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_category_desc(&self, category_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.delete_category_desc(category_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_category_order(
        &self,
        clan_id: i64,
        categories: &[(i32, i64)],
    ) -> Result<()> {
        let transport = self.inner.clone();
        let categories = categories.to_vec();

        runtime()
            .spawn(async move { transport.update_category_order(clan_id, &categories).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn active_archived_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.active_archived_thread(clan_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn archive_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.archive_channel(clan_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.delete_channel(clan_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn leave_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.leave_thread(clan_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Add users to a channel.
    pub async fn add_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move {
                let refs: Vec<&str> = user_ids.iter().map(String::as_str).collect();
                transport.add_channel_users(channel_id, &refs).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_notifications(
        &self,
        clan_id: &str,
        limit: i32,
        notification_id: &str,
        category: i32,
        direction: i32,
    ) -> Result<Vec<crate::InboxNotification>> {
        let transport = self.inner.clone();
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id {clan_id:?}: {e}"))?;
        let notification_id = if notification_id.is_empty() || notification_id == "0" {
            0
        } else {
            notification_id
                .parse::<i64>()
                .map_err(|e| anyhow::anyhow!("invalid notification_id {notification_id:?}: {e}"))?
        };
        runtime()
            .spawn(async move {
                let list = transport
                    .list_notifications(clan_id, limit, notification_id, category, direction)
                    .await?;
                crate::inbox_notifications_from_list(list)
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_notifications(&self, ids: &[&str], category: i32) -> Result<()> {
        let transport = self.inner.clone();
        let ids: Vec<String> = ids.iter().map(|s| (*s).to_string()).collect();
        runtime()
            .spawn(async move {
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                transport.delete_notifications(&refs, category).await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_sd_topics(
        &self,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<Vec<crate::TopicDiscussion>> {
        let transport = self.inner.clone();
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id {clan_id:?}: {e}"))?;
        runtime()
            .spawn(async move {
                let list = transport.list_sd_topic(clan_id, limit, page).await?;
                Ok(crate::topics_from_list(list))
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_topic_detail(&self, topic_id: &str) -> Result<crate::TopicDiscussion> {
        let transport = self.inner.clone();
        let topic_id = topic_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid topic_id {topic_id:?}: {e}"))?;
        runtime()
            .spawn(async move {
                let topic = transport.get_topic_detail(topic_id).await?;
                Ok(crate::topic_discussion_from_api(topic))
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Close the connection.
    ///
    /// Spawns the close operation on the dedicated transport runtime.
    pub async fn close(&self) -> Result<()> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.close().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))??;

        Ok(())
    }

    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let transport = self.inner.clone();
        let display_name = display_name.to_string();
        let avatar_url = avatar_url.to_string();

        runtime()
            .spawn(async move { transport.update_user(&display_name, &avatar_url).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_loged_device(&self) -> Result<Vec<mezon_proto::api::LogedDevice>> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_loged_device().await.map(|l| l.devices) })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
        logo: Option<&str>,
    ) -> Result<()> {
        tracing::debug!("TransportClient::update_account() called");

        let transport = self.inner.clone();
        let display_name = display_name.map(str::to_string);
        let avatar_url = avatar_url.map(str::to_string);
        let about_me = about_me.map(str::to_string);
        let logo = logo.map(str::to_string);

        runtime()
            .spawn(async move {
                transport
                    .update_account(
                        display_name.as_deref(),
                        avatar_url.as_deref(),
                        about_me.as_deref(),
                        logo.as_deref(),
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn registration_password(
        &self,
        email: &str,
        password: &str,
        old_password: &str,
    ) -> std::result::Result<
        crate::transport::ApiSession,
        crate::transport::RegistrationPasswordError,
    > {
        let transport = self.inner.clone();
        let request = mezon_proto::api::RegistrationEmailRequest {
            email: email.to_string(),
            password: password.to_string(),
            old_password: old_password.to_string(),
            ..Default::default()
        };
        runtime()
            .spawn(async move { transport.registration_password(request).await })
            .await
            .map_err(|error| {
                crate::transport::RegistrationPasswordError::Transport(format!(
                    "transport task failed: {error}"
                ))
            })?
    }

    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        let transport = self.inner.clone();
        let filename = filename.to_string();
        let filetype = filetype.to_string();

        runtime()
            .spawn(async move {
                transport
                    .upload_attachment_file(&filename, &filetype, size, width, height)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn multipart_upload_attachment_file_start(
        &self,
        req: mezon_proto::api::UploadAttachmentRequest,
    ) -> Result<mezon_proto::api::MultipartUploadAttachment> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.multipart_upload_attachment_file_start(req).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn multipart_upload_attachment_file_finish(
        &self,
        req: mezon_proto::api::MultipartUploadAttachmentFinishRequest,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.multipart_upload_attachment_file_finish(req).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_user_profile_on_clan(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanProfile> {
        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.get_user_profile_on_clan(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_user_profile_by_clan(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let nick_name = nick_name.to_string();
        let avatar_url = avatar_url.map(str::to_string);

        runtime()
            .spawn(async move {
                transport
                    .update_user_profile_by_clan(clan_id, &nick_name, avatar_url.as_deref())
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn check_duplicate_name(
        &self,
        name: &str,
        r#type: i32,
        condition_id: i64,
    ) -> Result<mezon_proto::api::CheckDuplicateNameResponse> {
        let transport = self.inner.clone();
        let name = name.to_string();

        runtime()
            .spawn(async move {
                transport
                    .check_duplicate_name(&name, r#type, condition_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Check duplicate thread name within a parent channel.
    pub async fn check_duplicate_thread_name(
        &self,
        name: &str,
        parent_channel_id: &str,
    ) -> Result<bool> {
        let transport = self.inner.clone();
        let name = name.to_string();
        let parent_channel_id = parent_channel_id.to_string();

        runtime()
            .spawn(async move {
                transport
                    .check_duplicate_thread_name(&name, &parent_channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let transport = self.inner.clone();
        let token = token.to_string();
        let refresh_token = refresh_token.to_string();

        runtime()
            .spawn(async move { transport.session_logout(&token, &refresh_token).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
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
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn is_follower(&self, follow_id: i64) -> Result<bool> {
        let transport = self.inner.clone();
        let response = runtime()
            .spawn(async move { transport.is_follower(follow_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))??;
        Ok(response.is_follower)
    }

    pub async fn delete_account(&self) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_account().await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn link_phone(
        &self,
        phone_number: &str,
    ) -> std::result::Result<String, crate::transport::LinkPhoneError> {
        let transport = self.inner.clone();
        let request = mezon_proto::api::AccountMezon {
            phone_number: phone_number.to_string(),
            ..Default::default()
        };
        let confirm = runtime()
            .spawn(async move { transport.link_sms(request).await })
            .await
            .map_err(|e| {
                crate::transport::LinkPhoneError::Transport(format!("transport task failed: {e}"))
            })??;
        Ok(confirm.req_id)
    }

    pub async fn confirm_phone_otp(
        &self,
        req_id: &str,
        otp_code: &str,
    ) -> Result<crate::transport::ApiSession> {
        let transport = self.inner.clone();
        let request = mezon_proto::api::LinkAccountConfirmRequest {
            req_id: req_id.to_string(),
            otp_code: otp_code.to_string(),
            ..Default::default()
        };
        runtime()
            .spawn(async move { transport.confirm_link_mezon_otp(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_list_favorite_channel(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ListFavoriteChannelResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_list_favorite_channel(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_apps(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::transport::ApiChannelApp>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_channel_apps(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn generate_hash_channel_apps(
        &self,
        app_id: i64,
    ) -> Result<mezon_proto::api::GenerateHashChannelAppsResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.generate_hash_channel_apps(app_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_channel_attachment(
        &self,
        clan_id: i64,
        channel_id: i64,
        file_type: String,
        state: i32,
        limit: i32,
        before: u32,
        after: u32,
    ) -> Result<Vec<crate::transport::ApiChannelAttachment>> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_channel_attachment(
                        clan_id, channel_id, file_type, state, limit, before, after,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        year: i32,
        limit: i32,
    ) -> Result<mezon_proto::api::ListChannelTimelineResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_channel_timeline(clan_id, channel_id, year, limit)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_channel_timeline(
        &self,
        req: mezon_proto::api::CreateChannelTimelineRequest,
    ) -> Result<mezon_proto::api::CreateChannelTimelineResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.create_channel_timeline(req).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_channel_timeline(
        &self,
        req: mezon_proto::api::UpdateChannelTimelineRequest,
    ) -> Result<mezon_proto::api::UpdateChannelTimelineResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_channel_timeline(req).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_channel_desc(
        &self,
        clan_id: i64,
        channel_id: i64,
        params: UpdateChannelDescParams,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_desc(clan_id, channel_id, params)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn detail_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
        start_time_seconds: u32,
    ) -> Result<mezon_proto::api::ChannelTimelineDetailResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .detail_channel_timeline(clan_id, channel_id, id, start_time_seconds)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.add_channel_favorite(channel_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.remove_channel_favorite(channel_id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::UserPermissionInChannelListResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_user_permission_in_channel(clan_id, channel_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn create_poll(
        &self,
        channel_id: i64,
        clan_id: i64,
        question: String,
        answers: Vec<String>,
        expire_hours: i32,
        poll_type: i32,
    ) -> Result<mezon_proto::api::CreatePollResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .create_poll(
                        channel_id,
                        clan_id,
                        &question,
                        answers,
                        expire_hours,
                        poll_type,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn vote_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
        answer_indices: Vec<i32>,
    ) -> Result<mezon_proto::api::VotePollResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .vote_poll(poll_id, message_id, channel_id, answer_indices)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn get_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::GetPollResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_poll(poll_id, message_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn close_poll(&self, poll_id: i64, message_id: i64, channel_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.close_poll(poll_id, message_id, channel_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_webhook_by_channel_id(
        &self,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<mezon_proto::api::WebhookListResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .list_webhook_by_channel_id(channel_id, clan_id)
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn generate_webhook(
        &self,
        request: mezon_proto::api::WebhookCreateRequest,
    ) -> Result<mezon_proto::api::WebhookGenerateResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.generate_webhook(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_webhook_by_id(
        &self,
        request: mezon_proto::api::WebhookUpdateRequestById,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_webhook_by_id(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_webhook_by_id(
        &self,
        request: mezon_proto::api::WebhookDeleteRequestById,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_webhook_by_id(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn list_clan_webhook(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ListClanWebhookResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.list_clan_webhook(clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn generate_clan_webhook(
        &self,
        request: mezon_proto::api::GenerateClanWebhookRequest,
    ) -> Result<mezon_proto::api::GenerateClanWebhookResponse> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.generate_clan_webhook(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn update_clan_webhook_by_id(
        &self,
        request: mezon_proto::api::UpdateClanWebhookRequest,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.update_clan_webhook_by_id(request).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    pub async fn delete_clan_webhook_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.delete_clan_webhook_by_id(id, clan_id).await })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder as _;
        let pixels = vec![0u8; (width * height * 4) as usize];
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .expect("encode source png");
        out
    }

    fn dimensions_of(bytes: &[u8]) -> (u32, u32) {
        image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .expect("guess format")
            .into_dimensions()
            .expect("read dimensions")
    }

    #[test]
    fn oversized_icon_is_capped_to_the_max_edge() {
        let encoded = shrink_icon_to_png(&png_of(512, 512)).expect("shrink");
        assert_eq!(
            dimensions_of(&encoded),
            (NOTIFICATION_ICON_MAX_PX, NOTIFICATION_ICON_MAX_PX)
        );
    }

    #[test]
    fn non_square_icon_keeps_its_aspect_ratio() {
        let encoded = shrink_icon_to_png(&png_of(512, 256)).expect("shrink");
        assert_eq!(
            dimensions_of(&encoded),
            (NOTIFICATION_ICON_MAX_PX, NOTIFICATION_ICON_MAX_PX / 2)
        );
    }

    #[test]
    fn small_icon_is_not_upscaled() {
        let encoded = shrink_icon_to_png(&png_of(32, 32)).expect("shrink");
        assert_eq!(dimensions_of(&encoded), (32, 32));
    }

    #[test]
    fn output_is_png_regardless_of_input_format() {
        use image::ImageEncoder as _;
        let mut source = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut source)
            .write_image(
                &vec![0u8; 128 * 128 * 3],
                128,
                128,
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode source jpeg");

        let encoded = shrink_icon_to_png(&source).expect("shrink");
        assert_eq!(
            image::guess_format(&encoded).expect("guess output format"),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn a_decode_bomb_is_rejected_rather_than_allocated() {
        let oversized = NOTIFICATION_ICON_DECODE_MAX_PX + 1;
        let err = shrink_icon_to_png(&png_of(oversized, 1));
        assert!(
            err.is_err(),
            "expected the decode limit to reject the image"
        );
    }

    #[test]
    fn garbage_bytes_do_not_panic() {
        assert!(shrink_icon_to_png(b"not an image at all").is_err());
    }
}
