use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{ApiAccount, ApiChannelDesc, ApiClanDesc},
};

#[derive(Clone)]
pub struct AppApi {
    transport: Arc<TransportClient>,
}

impl AppApi {
    pub fn new(transport: Arc<TransportClient>) -> Self {
        Self { transport }
    }

    pub async fn get_account(&self) -> Result<ApiAccount> {
        self.transport.get_account().await
    }

    pub async fn list_channel_descs(&self, clan_id: &str) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_descs(clan_id).await
    }

    pub async fn list_channel_by_user_id(&self) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_by_user_id().await
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        self.transport.list_clan_descs().await
    }

    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<ApiClanDesc> {
        self.transport
            .create_clan_desc(clan_name, logo, banner)
            .await
    }

    pub async fn is_open(&self) -> bool {
        self.transport.is_open().await
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        self.transport.ping_roundtrip().await
    }

    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        self.transport.update_user(display_name, avatar_url).await
    }

    pub async fn update_account(
        &self,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        about_me: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_account(display_name, avatar_url, about_me)
            .await
    }

    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        self.transport
            .upload_attachment_file(filename, filetype, size)
            .await
    }

    /// Full avatar upload flow: get pre-signed URL, PUT file bytes, return permanent URL.
    pub async fn get_user_clan_profile(
        &self,
        clan_id: &str,
    ) -> Result<mezon_proto::api::ClanProfile> {
        self.transport.get_user_profile_on_clan(clan_id).await
    }

    pub async fn update_user_clan_profile(
        &self,
        clan_id: &str,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_user_profile_by_clan(clan_id, nick_name, avatar_url)
            .await
    }

    pub async fn check_duplicate_clan_nickname(
        &self,
        clan_id: &str,
        nick_name: &str,
    ) -> Result<bool> {
        let resp = self
            .transport
            .check_duplicate_name(nick_name, 4, clan_id.parse().unwrap_or_default())
            .await?;
        Ok(resp.is_duplicate)
    }

    pub async fn upload_avatar(&self, path: &Path) -> Result<String> {
        tracing::info!("upload_avatar: reading file path={:?}", path);
        let data = std::fs::read(path)?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("avatar")
            .to_string();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_string();
        let filetype = format!("image/{}", ext);
        let size = data.len() as i32;
        tracing::info!(
            "upload_avatar: file read ok filename={} filetype={} size={}",
            filename,
            filetype,
            size
        );

        tracing::info!("upload_avatar: requesting presigned URL");
        let upload = self
            .transport
            .upload_attachment_file(&filename, &filetype, size)
            .await?;
        tracing::info!("upload_avatar: presigned URL received url={}", upload.url);

        tracing::info!("upload_avatar: PUTing file bytes to presigned URL");
        crate::transport_runtime::put_bytes_to_url(&upload.url, data).await?;
        tracing::info!("upload_avatar: PUT completed successfully");

        let permanent_url = upload
            .url
            .split('?')
            .next()
            .unwrap_or(&upload.url)
            .to_string();

        tracing::info!("Avatar upload complete: url={}", permanent_url);

        Ok(permanent_url)
    }

    pub async fn list_loged_device(&self) -> Result<Vec<mezon_proto::api::LogedDevice>> {
        self.transport.list_loged_device().await
    }

    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        self.transport.session_logout(token, refresh_token).await
    }

    pub async fn logout_device(
        &self,
        token: &str,
        refresh_token: &str,
        device_id: &str,
    ) -> Result<()> {
        self.transport
            .logout_device(token, refresh_token, device_id)
            .await
    }
}
