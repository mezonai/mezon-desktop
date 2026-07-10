use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{
        ApiAccount, ApiAttachment, ApiCategoryDesc, ApiChannelApp, ApiChannelAttachment,
        ApiChannelDesc, ApiClanDesc, ApiDirectChannel, ApiFriend, ApiMessage, ApiPinMessage,
        ApiThreadDesc, ApiVoiceChannelUser, RealtimeEvent,
    },
};

const CHECK_NAME_TYPE_NICKNAME: i32 = 4;

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn clamp_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn multipart_part_ranges(total: u64) -> Vec<(u64, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0u64;
    while offset < total {
        let len = (total - offset).min(MULTIPART_PART_SIZE) as usize;
        ranges.push((offset, len));
        offset += len as u64;
    }
    ranges
}

fn attachment_cdn_url(base_img_url: &str, filename: &str) -> Result<String> {
    if filename.is_empty() {
        anyhow::bail!("attachment upload returned an empty filename");
    }
    Ok(format!(
        "{}/{}",
        base_img_url.trim_end_matches('/'),
        filename
    ))
}

fn image_dimensions(data: &[u8]) -> (i32, i32) {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map(|(w, h)| {
            (
                i32::try_from(w).unwrap_or(i32::MAX),
                i32::try_from(h).unwrap_or(i32::MAX),
            )
        })
        .unwrap_or((0, 0))
}

/// Connection lifecycle of the realtime transport — the analog of Zed's `client::Status`.
/// Exposed as a `watch` stream via [`AppApi::status`] so stores/UI react instead of polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Clone)]
pub struct AppApi {
    transport: Arc<TransportClient>,
    realtime_tx: Arc<tokio::sync::broadcast::Sender<RealtimeEvent>>,
    status_tx: Arc<tokio::sync::watch::Sender<ConnectionStatus>>,
    base_img_url: String,
}

const MULTIPART_PART_SIZE: u64 = 10 * 1024 * 1024;
const MULTIPART_MIN_FILE_SIZE: u64 = 16 * 1024 * 1024;
const MULTIPART_PART_CONCURRENCY: usize = 2;
const ATTACHMENT_UPLOAD_CONCURRENCY: usize = 4;
const PRESIGN_EDIT_BATCH_SIZE: usize = 4;

#[derive(Debug, Clone)]
pub struct UploadFile {
    pub path: PathBuf,
    pub filename: String,
    pub filetype: String,
    pub width: i32,
    pub height: i32,
    pub thumbnail: Option<UploadThumbnail>,
}

#[derive(Debug, Clone)]
pub struct UploadThumbnail {
    pub filename: String,
    pub data: Vec<u8>,
}

pub struct PresignedAttachment {
    pub attachment: mezon_proto::api::MessageAttachment,
    plan: UploadPlan,
}

enum UploadPlan {
    Single {
        put_url: String,
        path: PathBuf,
    },
    Multipart {
        upload_id: String,
        part_urls: Vec<String>,
        ranges: Vec<(u64, usize)>,
        path: PathBuf,
        content_type: String,
        filename: String,
    },
}

#[derive(Debug, Clone)]
pub struct UrlAttachment {
    pub url: String,
    pub filename: String,
    pub filetype: String,
}

#[derive(Debug, Clone)]
pub enum AttachmentUploadOutcome {
    Uploaded(String),
    Failed(String),
}

impl AppApi {
    pub fn new(transport: Arc<TransportClient>, base_img_url: String) -> Self {
        let (realtime_tx, _) = tokio::sync::broadcast::channel(1024);
        let (status_tx, _) = tokio::sync::watch::channel(ConnectionStatus::Disconnected);
        Self {
            transport,
            realtime_tx: Arc::new(realtime_tx),
            status_tx: Arc::new(status_tx),
            base_img_url,
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RealtimeEvent> {
        self.realtime_tx.subscribe()
    }

    pub fn publish_event(&self, event: RealtimeEvent) {
        let _ = self.realtime_tx.send(event);
    }

    /// Watch the realtime connection status (cf. Zed `Client::status`). Reactive — no polling.
    pub fn status(&self) -> tokio::sync::watch::Receiver<ConnectionStatus> {
        self.status_tx.subscribe()
    }

    /// Current connection status snapshot.
    pub fn connection_status(&self) -> ConnectionStatus {
        *self.status_tx.borrow()
    }

    /// Update the connection status — called by the transport connection manager.
    pub fn set_status(&self, status: ConnectionStatus) {
        let _ = self.status_tx.send(status);
    }

    pub async fn get_account(&self) -> Result<ApiAccount> {
        self.transport.get_account().await
    }

    pub async fn list_channel_descs(
        &self,
        clan_id: i64,
        channel_type: i32,
    ) -> Result<Vec<ApiChannelDesc>> {
        let _ = channel_type;
        self.transport.list_channel_descs(clan_id).await
    }

    pub async fn list_channel_by_user_id(&self) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_by_user_id().await
    }

    pub async fn list_dm_channels(&self, page: i32) -> Result<Vec<ApiDirectChannel>> {
        self.transport.list_dm_channel_descs(page).await
    }

    pub async fn mark_as_read(
        &self,
        channel_id: i64,
        category_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .mark_as_read(channel_id, category_id, clan_id)
            .await
    }

    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        self.transport.list_clan_descs().await
    }

    pub async fn list_clan_users(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::clan_user_list::ClanUser>> {
        let lists = self.transport.list_clan_users(clan_id).await?;
        Ok(lists.into_iter().flat_map(|list| list.clan_users).collect())
    }

    pub async fn list_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
    ) -> Result<Vec<mezon_proto::api::channel_user_list::ChannelUser>> {
        let list = self
            .transport
            .list_channel_users(clan_id, channel_id, channel_type)
            .await?;
        Ok(list.channel_users)
    }

    pub async fn list_user_clans_by_user(&self) -> Result<Vec<mezon_proto::api::User>> {
        let list = self.transport.list_user_clans_by_user_id().await?;
        Ok(list.users)
    }

    pub async fn list_channel_users_uc(
        &self,
        channel_id: i64,
        limit: i32,
    ) -> Result<mezon_proto::api::AllUsersAddChannelResponse> {
        self.transport
            .list_channel_users_uc(channel_id, limit)
            .await
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

    pub async fn update_clan_desc(
        &self,
        request: mezon_proto::api::UpdateClanDescRequest,
    ) -> Result<()> {
        self.transport.update_clan_desc(request).await
    }

    pub async fn get_system_message_by_clan_id(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::SystemMessage> {
        self.transport.get_system_message_by_clan_id(clan_id).await
    }

    pub async fn update_system_message(
        &self,
        request: mezon_proto::api::SystemMessageRequest,
    ) -> Result<()> {
        self.transport.update_system_message(request).await
    }

    pub async fn is_open(&self) -> bool {
        self.transport.is_open().await
    }

    pub async fn ping_roundtrip(&self) -> Result<()> {
        self.transport.ping_roundtrip().await
    }

    pub async fn list_channel_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<crate::transport::ListChannelMessagesResult> {
        self.transport
            .list_channel_messages(clan_id, channel_id, message_id, direction, limit)
            .await
    }

    pub async fn list_thread_descs(
        &self,
        channel_id: &str,
        clan_id: &str,
        page: i32,
    ) -> Result<Vec<ApiThreadDesc>> {
        self.transport
            .list_thread_descs(channel_id, clan_id, page)
            .await
    }

    pub async fn search_thread(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<Vec<ApiThreadDesc>> {
        self.transport
            .search_thread(clan_id, channel_id, label)
            .await
    }

    pub async fn search_message(
        &self,
        request: mezon_proto::api::SearchMessageRequest,
    ) -> Result<mezon_proto::api::SearchMessageResponse> {
        self.transport
            .search_message(request.filters, request.from, request.size, request.sorts)
            .await
    }

    pub async fn check_duplicate_thread_name(
        &self,
        name: &str,
        parent_channel_id: &str,
    ) -> Result<bool> {
        self.transport
            .check_duplicate_thread_name(name, parent_channel_id)
            .await
    }

    pub async fn get_pin_messages_list(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<Vec<ApiPinMessage>> {
        self.transport
            .get_pin_messages_list(channel_id, clan_id)
            .await
    }

    pub async fn delete_pin_message(
        &self,
        id: &str,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        self.transport
            .delete_pin_message(id, message_id, channel_id, clan_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_channel_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        file_type: &str,
        state: i32,
        limit: i32,
        before: u32,
        after: u32,
    ) -> Result<Vec<ApiChannelAttachment>> {
        self.transport
            .list_channel_attachment(
                clan_id,
                channel_id,
                file_type.to_string(),
                state,
                limit,
                before,
                after,
            )
            .await
    }

    pub async fn create_pin_message(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .create_pin_message(message_id, channel_id, clan_id)
            .await?;
        Ok(())
    }

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
        self.transport
            .update_channel_message(
                clan_id, channel_id, message_id, content, mentions, hashtags, emojis, mode,
                is_public,
            )
            .await
    }

    pub async fn vote_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
        answer_indices: Vec<i32>,
    ) -> Result<mezon_proto::api::VotePollResponse> {
        self.transport
            .vote_poll(poll_id, message_id, channel_id, answer_indices)
            .await
    }

    pub async fn get_poll(
        &self,
        poll_id: i64,
        message_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::GetPollResponse> {
        self.transport
            .get_poll(poll_id, message_id, channel_id)
            .await
    }

    pub async fn close_poll(&self, poll_id: i64, message_id: i64, channel_id: i64) -> Result<()> {
        self.transport
            .close_poll(poll_id, message_id, channel_id)
            .await
    }

    pub async fn delete_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
    ) -> Result<()> {
        self.transport
            .delete_channel_message(clan_id, channel_id, message_id)
            .await
    }

    pub async fn join_chat(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        is_public: bool,
    ) -> Result<()> {
        self.transport
            .join_chat(clan_id, channel_id, channel_type, is_public)
            .await
    }

    pub async fn join_clan_chat(&self, clan_id: i64) -> Result<()> {
        self.transport.join_clan_chat(clan_id).await
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
        self.transport
            .react_channel_message(
                clan_id,
                channel_id,
                message_id,
                emoji_id,
                emoji,
                count,
                message_sender_id,
                mode,
                is_public,
                remove,
            )
            .await
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
        self.transport
            .write_last_seen_message(
                clan_id,
                channel_id,
                message_id,
                mode,
                timestamp_seconds,
                badge_count,
            )
            .await
    }

    pub async fn list_clan_users_status(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanUserStatusList> {
        self.transport.list_clan_users_status(clan_id).await
    }

    pub async fn list_user_online(
        &self,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<Vec<mezon_proto::api::User>> {
        let resp = self
            .transport
            .list_user_online(clan_id, limit, page)
            .await?;
        Ok(resp.users)
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
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message(
                clan_id, channel_id, content, is_public, mode, mentions, hashtags, emojis,
            )
            .await
    }

    /// Send a message as a reply to another message.
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
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_reply(
                clan_id, channel_id, content, is_public, mode, reply, mentions, hashtags, emojis,
            )
            .await
    }

    pub async fn report_message_abuse(&self, message_id: i64, abuse_type: &str) -> Result<()> {
        self.transport
            .report_message_abuse(message_id, abuse_type)
            .await
    }

    pub async fn create_message_2_inbox(
        &self,
        message_id: i64,
        channel_id: i64,
        clan_id: i64,
        content: &str,
    ) -> Result<()> {
        self.transport
            .create_message_2_inbox(message_id, channel_id, clan_id, content)
            .await
            .map(|_| ())
    }

    pub async fn create_sd_topic(
        &self,
        message_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::SdTopic> {
        self.transport
            .create_sd_topic(message_id, clan_id, channel_id)
            .await
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
        self.transport
            .message_button_click(
                message_id, channel_id, button_id, sender_id, user_id, extra_data,
            )
            .await
    }

    pub async fn dropdown_box_selected(
        &self,
        message_id: i64,
        channel_id: i64,
        selectbox_id: &str,
    ) -> Result<()> {
        self.transport
            .dropdown_box_selected(message_id, channel_id, selectbox_id)
            .await
    }

    pub async fn forward_channel_message(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        attachments: Vec<ApiAttachment>,
    ) -> Result<()> {
        let proto = attachments
            .into_iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename,
                size: a.size,
                url: a.url,
                filetype: a.filetype,
                width: a.width,
                height: a.height,
                thumbnail: a.thumbnail,
                duration: a.duration,
            })
            .collect();
        self.transport
            .forward_channel_message(clan_id, channel_id, content, is_public, mode, proto)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_channel_message_with_attachments(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        attachments: Vec<ApiAttachment>,
        mode: i32,
        is_public: bool,
    ) -> Result<()> {
        let proto = attachments
            .into_iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename,
                size: a.size,
                url: a.url,
                filetype: a.filetype,
                width: a.width,
                height: a.height,
                thumbnail: a.thumbnail,
                duration: a.duration,
            })
            .collect();
        self.transport
            .update_channel_message_with_attachments(
                clan_id, channel_id, message_id, content, proto, mode, is_public,
            )
            .await
    }

    pub async fn list_emojis_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanEmoji>> {
        let resp = self.transport.list_emojis_by_user_id().await?;
        Ok(resp.emoji_list)
    }

    pub async fn emoji_recent_list(&self) -> Result<Vec<mezon_proto::api::EmojiRecent>> {
        let resp = self.transport.emoji_recent_list().await?;
        Ok(resp.emoji_recents)
    }

    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleListEventResponse> {
        self.transport.list_roles(clan_id, limit, cursor).await
    }

    pub async fn get_list_permission(&self) -> Result<mezon_proto::api::PermissionList> {
        self.transport.get_list_permission().await
    }

    pub async fn get_clan_user_role(&self, clan_id: i64) -> Result<mezon_proto::api::RoleList> {
        self.transport.get_clan_user_role(clan_id, 0).await
    }

    pub async fn list_stickers_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanSticker>> {
        let resp = self.transport.list_stickers_by_user_id().await?;
        Ok(resp.stickers)
    }

    pub async fn create_channel(
        &self,
        clan_id: i64,
        channel_label: &str,
        channel_type: u32,
        category_id: Option<i64>,
        parent_id: Option<i64>,
        channel_private: i32,
    ) -> Result<ApiChannelDesc> {
        self.transport
            .create_channel(
                clan_id,
                channel_label,
                channel_type,
                category_id,
                parent_id,
                channel_private,
            )
            .await
    }

    pub async fn create_direct_channel(&self, user_ids: &[i64]) -> Result<ApiChannelDesc> {
        self.transport.create_direct_channel(user_ids).await
    }

    /// List the current user's friends, with relationship state preserved.
    pub async fn list_friends(&self) -> Result<Vec<ApiFriend>> {
        self.transport.list_friends().await
    }

    /// Send or accept a friend request by ids and/or usernames. Returns response ids.
    pub async fn add_friends(&self, ids: Vec<i64>, usernames: Vec<String>) -> Result<Vec<i64>> {
        self.transport.add_friends(ids, usernames).await
    }

    /// Delete, cancel, or reject a friend relationship by ids and/or usernames.
    pub async fn delete_friends(&self, ids: Vec<i64>, usernames: Vec<String>) -> Result<()> {
        self.transport.delete_friends(ids, usernames).await
    }

    /// Block users by id.
    pub async fn block_friends(&self, ids: Vec<i64>) -> Result<()> {
        self.transport.block_friends(ids).await
    }

    /// Unblock users by id.
    pub async fn unblock_friends(&self, ids: Vec<i64>) -> Result<()> {
        self.transport.unblock_friends(ids).await
    }

    /// List rich-presence activities for users (React `listActivity` / `ListActivity`).
    pub async fn list_activity(&self) -> Result<mezon_proto::api::ListUserActivity> {
        self.transport.list_activity().await
    }

    /// Create a category in a clan; returns its id.
    pub async fn create_category(
        &self,
        clan_id: i64,
        category_name: &str,
    ) -> Result<ApiCategoryDesc> {
        let category = self
            .transport
            .create_category_desc(category_name, clan_id)
            .await?;
        Ok(ApiCategoryDesc {
            category_id: category.category_id,
            category_name: category.category_name,
            clan_id: category.clan_id,
            category_order: category.category_order,
        })
    }

    pub async fn add_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        self.transport.add_channel_users(channel_id, user_ids).await
    }

    /// Send a channel message, uploading each `media_url` as an attachment first.
    pub async fn send_message_with_media(
        &self,
        clan_id: i64,
        channel_id: i64,
        content: &str,
        is_public: bool,
        mode: i32,
        media_urls: &[String],
    ) -> Result<ApiMessage> {
        let attachments: Vec<_> = {
            use futures::StreamExt as _;
            futures::stream::iter(media_urls.iter().map(|url| self.upload_media_from_url(url)))
                .buffer_unordered(4)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?
        };
        self.transport
            .send_channel_message_with_attachments(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                attachments,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
            )
            .await
    }

    pub async fn presign_file(&self, file: UploadFile) -> Result<PresignedAttachment> {
        let UploadFile {
            path,
            filename,
            filetype,
            width,
            height,
            thumbnail,
        } = file;
        let filename = sanitize_filename(&filename);
        let raw_size = crate::transport_runtime::file_len(path.clone()).await?;
        let size = i32::try_from(raw_size)
            .map_err(|_| anyhow::anyhow!("attachment too large to upload: {raw_size} bytes"))?;
        let thumbnail_url = match thumbnail {
            Some(thumb) => self.upload_thumbnail(thumb).await,
            None => String::new(),
        };
        let (url, plan) = if size as u64 >= MULTIPART_MIN_FILE_SIZE {
            let ranges = multipart_part_ranges(size as u64);
            let started = self
                .transport
                .multipart_upload_attachment_file_start(mezon_proto::api::UploadAttachmentRequest {
                    filename: filename.clone(),
                    filetype: filetype.clone(),
                    size,
                    width,
                    height,
                    part_count: ranges.len() as i32,
                })
                .await?;
            if started.urls.len() != ranges.len() {
                anyhow::bail!(
                    "multipart start returned {} urls, expected {}",
                    started.urls.len(),
                    ranges.len()
                );
            }
            let url = attachment_cdn_url(&self.base_img_url, &started.filename)?;
            (
                url,
                UploadPlan::Multipart {
                    upload_id: started.upload_id,
                    part_urls: started.urls,
                    ranges,
                    path,
                    content_type: filetype.clone(),
                    filename: started.filename,
                },
            )
        } else {
            let upload = self
                .transport
                .upload_attachment_file(&filename, &filetype, size, width, height)
                .await?;
            let url = attachment_cdn_url(&self.base_img_url, &upload.filename)?;
            (
                url,
                UploadPlan::Single {
                    put_url: upload.url,
                    path,
                },
            )
        };
        Ok(PresignedAttachment {
            attachment: mezon_proto::api::MessageAttachment {
                filename,
                size,
                url,
                filetype,
                width,
                height,
                thumbnail: thumbnail_url,
                duration: 0,
            },
            plan,
        })
    }

    pub async fn execute_upload(&self, presigned: PresignedAttachment) -> Result<()> {
        match presigned.plan {
            UploadPlan::Single { put_url, path } => {
                let data = crate::transport_runtime::read_file(path).await?;
                crate::transport_runtime::put_bytes_to_url(&put_url, data).await?;
                Ok(())
            }
            UploadPlan::Multipart {
                upload_id,
                part_urls,
                ranges,
                path,
                content_type,
                filename,
            } => {
                let mut parts: Vec<mezon_proto::api::MultipartUploadAttachmentPart> = {
                    use futures::StreamExt as _;
                    futures::stream::iter(part_urls.into_iter().zip(ranges).enumerate().map(
                        |(index, (url, (offset, len)))| {
                            let path = path.clone();
                            let content_type = content_type.clone();
                            async move {
                                let part =
                                    crate::transport_runtime::read_file_range(path, offset, len)
                                        .await?;
                                let e_tag = crate::transport_runtime::put_bytes_return_etag(
                                    &url,
                                    part,
                                    &content_type,
                                )
                                .await?;
                                Ok::<_, anyhow::Error>(
                                    mezon_proto::api::MultipartUploadAttachmentPart {
                                        part_number: (index + 1) as i32,
                                        e_tag,
                                    },
                                )
                            }
                        },
                    ))
                    .buffered(MULTIPART_PART_CONCURRENCY)
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>>>()?
                };
                parts.sort_by_key(|p| p.part_number);
                self.transport
                    .multipart_upload_attachment_file_finish(
                        mezon_proto::api::MultipartUploadAttachmentFinishRequest {
                            upload_id,
                            parts,
                            filename,
                        },
                    )
                    .await?;
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_presigned_message(
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
        presign_finish: Vec<String>,
    ) -> Result<ApiMessage> {
        let echo: Vec<crate::transport::ApiAttachment> = attachments
            .iter()
            .map(|a| crate::transport::ApiAttachment {
                url: a.url.clone(),
                filename: a.filename.clone(),
                filetype: a.filetype.clone(),
                width: a.width,
                height: a.height,
                thumbnail: a.thumbnail.clone(),
                duration: a.duration,
                size: a.size,
            })
            .collect();
        let mut sent = self
            .transport
            .send_channel_message_with_attachments(
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                attachments,
                reply,
                mentions,
                hashtags,
                emojis,
                Some(presign_finish),
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_presign_finish(
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
        self.transport
            .patch_message_presign_finish(
                clan_id,
                channel_id,
                message_id,
                content,
                mentions,
                presign_finish,
                create_time_seconds,
                mode,
                is_public,
            )
            .await
    }

    pub async fn presign_files(&self, files: Vec<UploadFile>) -> Result<Vec<PresignedAttachment>> {
        use futures::StreamExt as _;
        futures::stream::iter(files.into_iter().map(|file| self.presign_file(file)))
            .buffered(ATTACHMENT_UPLOAD_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upload_presigned_and_patch(
        &self,
        clan_id: i64,
        channel_id: i64,
        message_id: i64,
        content: &str,
        mentions: Vec<crate::transport::OutgoingMention>,
        create_time_seconds: u32,
        presigned: Vec<PresignedAttachment>,
        keys: Vec<String>,
        mode: i32,
        is_public: bool,
        on_complete: tokio::sync::mpsc::UnboundedSender<AttachmentUploadOutcome>,
    ) {
        use futures::StreamExt as _;
        let mut stream = futures::stream::iter(
            presigned
                .into_iter()
                .zip(keys)
                .map(|(item, key)| async move { (key, self.execute_upload(item).await) }),
        )
        .buffer_unordered(ATTACHMENT_UPLOAD_CONCURRENCY);
        let mut finished: Vec<String> = Vec::new();
        let mut synced = 0usize;
        while let Some((key, result)) = stream.next().await {
            match result {
                Ok(()) => {
                    let _ = on_complete.send(AttachmentUploadOutcome::Uploaded(key.clone()));
                    finished.push(key);
                }
                Err(e) => {
                    tracing::error!("attachment upload failed: {e}");
                    let _ = on_complete.send(AttachmentUploadOutcome::Failed(key));
                }
            }
            if finished.len() - synced >= PRESIGN_EDIT_BATCH_SIZE {
                if let Err(e) = self
                    .update_presign_finish(
                        clan_id,
                        channel_id,
                        message_id,
                        content,
                        mentions.clone(),
                        finished.clone(),
                        create_time_seconds,
                        mode,
                        is_public,
                    )
                    .await
                {
                    tracing::error!("presign_finish sync failed: {e}");
                } else {
                    synced = finished.len();
                }
            }
        }
        if finished.len() > synced
            && let Err(e) = self
                .update_presign_finish(
                    clan_id,
                    channel_id,
                    message_id,
                    content,
                    mentions,
                    finished,
                    create_time_seconds,
                    mode,
                    is_public,
                )
                .await
        {
            tracing::error!("presign_finish final sync failed: {e}");
        }
    }

    pub async fn send_message_with_attachment_urls(
        &self,
        clan_id: i64,
        channel_id: i64,
        is_public: bool,
        mode: i32,
        attachments: Vec<UrlAttachment>,
    ) -> Result<ApiMessage> {
        let proto: Vec<mezon_proto::api::MessageAttachment> = attachments
            .iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename.clone(),
                size: 0,
                url: a.url.clone(),
                filetype: a.filetype.clone(),
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
            })
            .collect();
        let echo: Vec<crate::transport::ApiAttachment> = attachments
            .into_iter()
            .map(|a| crate::transport::ApiAttachment {
                url: a.url,
                filename: a.filename,
                filetype: a.filetype,
                width: 0,
                height: 0,
                thumbnail: String::new(),
                duration: 0,
                size: 0,
            })
            .collect();
        let mut sent = self
            .transport
            .send_channel_message_with_attachments(
                clan_id,
                channel_id,
                "",
                is_public,
                mode,
                proto,
                None,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    async fn upload_thumbnail(&self, thumbnail: UploadThumbnail) -> String {
        let filename = sanitize_filename(&thumbnail.filename);
        let size = clamp_i32(thumbnail.data.len());
        match self
            .upload_bytes(&filename, "image/jpeg", size, 0, 0, thumbnail.data)
            .await
        {
            Ok(url) => url,
            Err(e) => {
                tracing::error!("attachment poster upload failed: {e}");
                String::new()
            }
        }
    }

    async fn upload_bytes(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
        width: i32,
        height: i32,
        data: Vec<u8>,
    ) -> Result<String> {
        let upload = self
            .transport
            .upload_attachment_file(filename, filetype, size, width, height)
            .await?;
        crate::transport_runtime::put_bytes_to_url(&upload.url, data).await?;
        attachment_cdn_url(&self.base_img_url, &upload.filename)
    }

    async fn upload_media_from_url(
        &self,
        url: &str,
    ) -> Result<mezon_proto::api::MessageAttachment> {
        let (data, content_type) = crate::transport_runtime::fetch_bytes(url).await?;
        let filetype = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
        let ext = match filetype.split('/').nth(1) {
            Some(e) if !e.is_empty() => e,
            _ => "bin",
        };
        let stem = url
            .split('?')
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("media");
        let filename = if stem.contains('.') {
            sanitize_filename(stem)
        } else {
            sanitize_filename(&format!("{stem}.{ext}"))
        };
        let size = clamp_i32(data.len());

        let (width, height) = if filetype.starts_with("image/") {
            image_dimensions(&data)
        } else {
            (0, 0)
        };

        let url = self
            .upload_bytes(&filename, &filetype, size, width, height, data)
            .await?;

        Ok(mezon_proto::api::MessageAttachment {
            filename,
            size,
            url,
            filetype,
            width,
            height,
            thumbnail: String::new(),
            duration: 0,
        })
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
        width: i32,
        height: i32,
    ) -> Result<mezon_proto::api::UploadAttachment> {
        self.transport
            .upload_attachment_file(filename, filetype, size, width, height)
            .await
    }

    pub async fn get_user_clan_profile(
        &self,
        clan_id: i64,
    ) -> Result<mezon_proto::api::ClanProfile> {
        self.transport.get_user_profile_on_clan(clan_id).await
    }

    pub async fn update_user_clan_profile(
        &self,
        clan_id: i64,
        nick_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_user_profile_by_clan(clan_id, nick_name, avatar_url)
            .await
    }

    pub async fn check_duplicate_clan_name(&self, name: &str, condition_id: &str) -> Result<bool> {
        let cond: i64 = condition_id
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid condition_id {condition_id:?}: {e}"))?;
        let resp = self.transport.check_duplicate_name(name, 0, cond).await?;
        Ok(resp.is_duplicate)
    }

    pub async fn check_duplicate_clan_nickname(
        &self,
        clan_id: i64,
        nick_name: &str,
    ) -> Result<bool> {
        let resp = self
            .transport
            .check_duplicate_name(nick_name, CHECK_NAME_TYPE_NICKNAME, clan_id)
            .await?;
        Ok(resp.is_duplicate)
    }

    pub async fn upload_avatar(&self, path: &Path) -> Result<String> {
        let data = crate::transport_runtime::read_file(path.to_path_buf()).await?;

        let raw_filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("avatar")
            .to_string();
        let filename = sanitize_filename(&raw_filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .to_string();
        let filetype = format!("image/{}", ext);
        let size = clamp_i32(data.len());
        let (width, height) = image_dimensions(&data);

        tracing::info!(
            "upload_avatar: file read ok filename={} filetype={} size={} width={} height={}",
            filename,
            filetype,
            size,
            width,
            height
        );

        let permanent_url = self
            .upload_bytes(&filename, &filetype, size, width, height, data)
            .await?;

        tracing::info!("Avatar upload complete: url={}", permanent_url);

        Ok(permanent_url)
    }

    pub async fn list_categories_typed(&self, clan_id: i64) -> Result<Vec<ApiCategoryDesc>> {
        self.transport.list_categories_typed(clan_id).await
    }

    pub async fn list_clan_badge_count(&self) -> Result<Vec<(String, i32, bool)>> {
        self.transport.list_clan_badge_count().await
    }

    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        self.transport.get_notification_clan(clan_id).await
    }

    pub async fn list_notifications(
        &self,
        clan_id: &str,
        limit: i32,
        notification_id: &str,
        category: i32,
        direction: i32,
    ) -> Result<Vec<crate::InboxNotification>> {
        self.transport
            .list_notifications(clan_id, limit, notification_id, category, direction)
            .await
    }

    pub async fn delete_notifications(&self, ids: &[&str], category: i32) -> Result<()> {
        self.transport.delete_notifications(ids, category).await
    }

    pub async fn list_sd_topics(
        &self,
        clan_id: &str,
        limit: i32,
    ) -> Result<Vec<crate::TopicDiscussion>> {
        self.transport.list_sd_topics(clan_id, limit).await
    }

    pub async fn get_topic_detail(&self, topic_id: &str) -> Result<crate::TopicDiscussion> {
        self.transport.get_topic_detail(topic_id).await
    }

    pub async fn list_channel_badge_counts(&self, clan_id: i64) -> Result<Vec<ApiChannelDesc>> {
        self.transport.list_channel_badge_counts(clan_id).await
    }

    pub async fn list_voice_channel_users(&self, clan_id: i64) -> Result<Vec<ApiVoiceChannelUser>> {
        self.transport.list_voice_channel_users(clan_id).await
    }

    pub async fn generate_meet_token(&self, channel_id: &str, room_name: &str) -> Result<String> {
        self.transport
            .generate_meet_token(channel_id, room_name)
            .await
    }

    pub async fn write_voice_reaction(&self, emojis: Vec<String>, channel_id: i64) -> Result<()> {
        self.transport
            .write_voice_reaction(emojis, channel_id)
            .await
    }

    pub async fn remove_participant_mezon_meet(
        &self,
        channel_id: &str,
        clan_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        self.transport
            .remove_participant_mezon_meet(channel_id, clan_id, room_name, username)
            .await
    }

    pub async fn mute_participant_mezon_meet(
        &self,
        channel_id: &str,
        clan_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        self.transport
            .mute_participant_mezon_meet(channel_id, clan_id, room_name, username)
            .await
    }

    pub async fn list_channel_apps(&self, clan_id: i64) -> Result<Vec<ApiChannelApp>> {
        self.transport.list_channel_apps(clan_id).await
    }

    pub async fn list_favorite_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        let resp = self.transport.get_list_favorite_channel(clan_id).await?;
        Ok(resp
            .channel_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect())
    }

    pub async fn add_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        self.transport
            .add_channel_favorite(channel_id, clan_id)
            .await
    }

    pub async fn remove_channel_favorite(&self, channel_id: i64, clan_id: i64) -> Result<()> {
        self.transport
            .remove_channel_favorite(channel_id, clan_id)
            .await
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

    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<mezon_proto::api::UserPermissionInChannelListResponse> {
        self.transport
            .list_user_permission_in_channel(clan_id, channel_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::{MULTIPART_PART_SIZE, attachment_cdn_url, multipart_part_ranges};

    #[test]
    fn multipart_ranges_cover_whole_file_and_are_contiguous() {
        let total = 55 * 1024 * 1024;
        let ranges = multipart_part_ranges(total);
        assert_eq!(ranges.len(), 6);
        let mut expected_offset = 0u64;
        for (offset, len) in &ranges {
            assert_eq!(*offset, expected_offset);
            assert!(*len as u64 <= MULTIPART_PART_SIZE);
            expected_offset += *len as u64;
        }
        assert_eq!(expected_offset, total);
        assert_eq!(ranges.last().unwrap().1 as u64, 5 * 1024 * 1024);
    }

    #[test]
    fn multipart_ranges_exact_multiple_has_no_trailing_part() {
        let total = 30 * 1024 * 1024;
        let ranges = multipart_part_ranges(total);
        assert_eq!(ranges.len(), 3);
        assert!(
            ranges
                .iter()
                .all(|(_, len)| *len as u64 == MULTIPART_PART_SIZE)
        );
    }

    #[test]
    fn multipart_ranges_empty_for_zero() {
        assert!(multipart_part_ranges(0).is_empty());
    }

    #[test]
    fn attachment_url_uses_base_img_host_not_presigned() {
        assert_eq!(
            attachment_cdn_url(
                "https://cdn.mezon.ai",
                "mezon/1826814768338440192/2074336632294608896.png",
            )
            .unwrap(),
            "https://cdn.mezon.ai/mezon/1826814768338440192/2074336632294608896.png"
        );
    }

    #[test]
    fn attachment_url_trims_trailing_slash_on_base() {
        assert_eq!(
            attachment_cdn_url("https://cdn.mezon.ai/", "x.png").unwrap(),
            "https://cdn.mezon.ai/x.png"
        );
    }

    #[test]
    fn attachment_url_errors_when_filename_empty() {
        assert!(attachment_cdn_url("https://cdn.mezon.ai", "").is_err());
    }
}
