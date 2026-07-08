//! Transport runtime wrapper with dedicated tokio runtime.
//!
//! Similar to how `ReqwestClient` manages its own tokio runtime via `static OnceLock<Runtime>`,
//! this allows transport operations to work when called from GPUI's smol-based executor.

use crate::abridged_tcp_adapter::AbridgedTcpAdapter;
use crate::transport::MezonTransport;
use anyhow::Result;
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, http};
use reqwest_client::ReqwestClient;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt as _;
use tokio::runtime::Runtime;

static TRANSPORT_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HTTP_CLIENT: OnceLock<ReqwestClient> = OnceLock::new();

const HTTP_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub(crate) fn http_client() -> &'static ReqwestClient {
    HTTP_CLIENT.get_or_init(new_http_client)
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

pub async fn download_to(url: &str, dest: std::path::PathBuf) -> Result<()> {
    let url = url.to_string();
    runtime()
        .spawn(async move {
            let outcome = stream_to_file(&url, &dest).await;
            if outcome.is_err() {
                let _ = tokio::fs::remove_file(&dest).await;
            }
            outcome
        })
        .await
        .map_err(|e| anyhow::anyhow!("download task failed: {e}"))?
}

async fn stream_to_file(url: &str, dest: &std::path::Path) -> Result<()> {
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
    let mut file = tokio::fs::File::create(dest).await?;
    let body = response.body_mut();
    let mut buffer = vec![0u8; 64 * 1024];
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
    }
    file.flush().await?;
    Ok(())
}

#[derive(Clone)]
pub struct TransportClient {
    inner: std::sync::Arc<MezonTransport>,
}

impl TransportClient {
    pub fn new(base_path: String) -> Self {
        let adapter = Box::new(AbridgedTcpAdapter::new());
        let transport = MezonTransport::new(adapter, base_path);
        Self {
            inner: std::sync::Arc::new(transport),
        }
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

    pub async fn list_clan_descs(&self) -> Result<Vec<crate::transport::ApiClanDesc>> {
        tracing::debug!("TransportClient::list_clan_descs() called");

        let transport = self.inner.clone();

        runtime()
            .spawn(async move { transport.list_clan_descs().await })
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

    /// Pin a message.
    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ChannelMessageHeader> {
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
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .update_channel_message(
                        clan_id, channel_id, message_id, &content, mentions, hashtags, emojis,
                        mode, is_public,
                    )
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
    }

    /// Delete a channel message.
    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> Result<()> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move {
                transport
                    .delete_channel_message(clan_id, channel_id, message_id)
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
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
        content: &str,
    ) -> Result<mezon_proto::api::ChannelMessageHeader> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .create_message_2_inbox(message_id, channel_id, clan_id, &content)
                    .await
            })
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

    pub async fn forward_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<mezon_proto::api::MessageAttachment>,
    ) -> Result<()> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .forward_channel_message(
                        clan_id,
                        channel_id,
                        &content,
                        is_public,
                        mode,
                        attachments,
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

    pub async fn get_list_permission(&self) -> Result<mezon_proto::api::PermissionList> {
        let transport = self.inner.clone();
        runtime()
            .spawn(async move { transport.get_list_permission().await })
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
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();

        runtime()
            .spawn(async move {
                transport
                    .send_channel_message(
                        clan_id, channel_id, &content, is_public, mode, mentions, hashtags, emojis,
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
    ) -> Result<crate::transport::ApiMessage> {
        let transport = self.inner.clone();
        let content = content.to_string();
        runtime()
            .spawn(async move {
                transport
                    .send_channel_message_reply(
                        clan_id, channel_id, &content, is_public, mode, reply, mentions, hashtags,
                        emojis,
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
        presign_finish: Vec<String>,
        create_time_seconds: u32,
        mode: i32,
        is_public: bool,
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
                        presign_finish,
                        create_time_seconds,
                        mode,
                        is_public,
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
    ) -> Result<Vec<crate::TopicDiscussion>> {
        let transport = self.inner.clone();
        let clan_id = clan_id
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("invalid clan_id {clan_id:?}: {e}"))?;
        runtime()
            .spawn(async move {
                let list = transport.list_sd_topic(clan_id, limit).await?;
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
    ) -> Result<()> {
        tracing::debug!("TransportClient::update_account() called");

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
            .map_err(|e| anyhow::anyhow!("transport task failed: {e}"))?
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
}
