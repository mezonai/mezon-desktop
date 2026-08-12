use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::{
    TransportClient,
    transport::{
        ApiAccount, ApiAttachment, ApiCanvas, ApiCanvasDetail, ApiCategoryDesc, ApiChannelApp,
        ApiChannelAttachment, ApiChannelDesc, ApiClanDesc, ApiDirectChannel, ApiFriend, ApiMessage,
        ApiPinMessage, ApiThreadDesc, ApiVoiceChannelUser, HttpFallbackSession, RealtimeEvent,
        UpdateChannelDescParams,
    },
};

const CHECK_NAME_TYPE_CHANNEL: i32 = 2;
const CHECK_NAME_TYPE_NICKNAME: i32 = 4;

pub fn sanitize_upload_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
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

fn emoticon_id_from_filename(filename: &str) -> Option<i64> {
    let file_name = filename.rsplit('/').next().filter(|s| !s.is_empty())?;
    let stem = file_name.rsplit_once('.').map(|(stem, _)| stem)?;
    stem.parse().ok()
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
    pub duration: i32,
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
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone)]
pub enum AttachmentUploadOutcome {
    Uploaded(String),
    Failed(String),
}

impl AppApi {
    pub async fn send_channel_message_structured(
        &self,
        channel_id: i64,
        content_json: &str,
        mode: i32,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_structured(channel_id, content_json, mode)
            .await
    }

    pub async fn send_channel_message_structured_with_code(
        &self,
        channel_id: i64,
        content_json: &str,
        mode: i32,
        message_code: i32,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_structured_with_code(channel_id, content_json, mode, message_code)
            .await
    }
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

    pub fn set_http_fallback(&self, fallback: Option<HttpFallbackSession>) {
        self.transport.set_http_fallback(fallback);
    }

    pub async fn renew_fallback_token(&self) -> Result<crate::transport::RenewedTokens> {
        self.transport.renew_fallback_token().await
    }

    pub fn renewed_tokens(
        &self,
    ) -> tokio::sync::watch::Receiver<Option<crate::transport::RenewedTokens>> {
        self.transport.renewed_tokens()
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

    pub async fn list_channel_detail(&self, channel_id: i64) -> Result<ApiChannelDesc> {
        self.transport.list_channel_detail(channel_id).await
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

    pub async fn update_user_status(
        &self,
        status: String,
        minutes: i32,
        until_turn_on: bool,
    ) -> Result<()> {
        self.transport
            .update_user_status(status, minutes, until_turn_on)
            .await
    }

    pub async fn update_user_custom_status(
        &self,
        status: String,
        minutes: i32,
        until_turn_on: bool,
    ) -> Result<()> {
        self.transport
            .update_user_custom_status(status, minutes, until_turn_on)
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

    pub async fn list_audit_log(
        &self,
        clan_id: i64,
        action_log: &str,
        user_id: Option<i64>,
        date_log: &str,
    ) -> Result<mezon_proto::api::ListAuditLog> {
        self.transport
            .list_audit_log(clan_id, action_log, user_id, date_log)
            .await
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

    pub async fn list_topic_messages(
        &self,
        clan_id: i64,
        channel_id: i64,
        topic_id: i64,
        direction: i32,
        limit: u32,
    ) -> Result<crate::transport::ListChannelMessagesResult> {
        self.transport
            .list_topic_messages(clan_id, channel_id, topic_id, direction, limit)
            .await
    }

    /// List threads for a parent channel.
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
    pub async fn list_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        year: i32,
        limit: i32,
    ) -> Result<mezon_proto::api::ListChannelTimelineResponse> {
        self.transport
            .list_channel_timeline(clan_id, channel_id, year, limit)
            .await
    }

    pub async fn create_channel_timeline(
        &self,
        req: mezon_proto::api::CreateChannelTimelineRequest,
    ) -> Result<mezon_proto::api::CreateChannelTimelineResponse> {
        self.transport.create_channel_timeline(req).await
    }

    pub async fn update_channel_timeline(
        &self,
        req: mezon_proto::api::UpdateChannelTimelineRequest,
    ) -> Result<mezon_proto::api::UpdateChannelTimelineResponse> {
        self.transport.update_channel_timeline(req).await
    }

    pub async fn update_channel_desc(
        &self,
        clan_id: i64,
        channel_id: i64,
        params: UpdateChannelDescParams,
    ) -> Result<()> {
        self.transport
            .update_channel_desc(clan_id, channel_id, params)
            .await
    }

    pub async fn detail_channel_timeline(
        &self,
        clan_id: i64,
        channel_id: i64,
        id: i64,
        start_time_seconds: u32,
    ) -> Result<mezon_proto::api::ChannelTimelineDetailResponse> {
        self.transport
            .detail_channel_timeline(clan_id, channel_id, id, start_time_seconds)
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

    pub async fn get_channel_canvas_list(
        &self,
        channel_id: i64,
        clan_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<Vec<ApiCanvas>> {
        self.transport
            .get_channel_canvas_list(channel_id, clan_id, limit, page)
            .await
    }

    pub async fn get_channel_canvas_detail(
        &self,
        id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<ApiCanvasDetail> {
        self.transport
            .get_channel_canvas_detail(id, clan_id, channel_id)
            .await
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
        self.transport
            .edit_channel_canvas(id, channel_id, clan_id, title, content, is_default, status)
            .await
    }

    pub async fn delete_channel_canvas(
        &self,
        canvas_id: i64,
        clan_id: i64,
        channel_id: i64,
    ) -> Result<()> {
        self.transport
            .delete_channel_canvas(canvas_id, clan_id, channel_id)
            .await
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
        topic_id: i64,
        is_update_msg_topic: bool,
        hide_editted: bool,
        create_time_seconds: u32,
    ) -> Result<()> {
        self.transport
            .update_channel_message(
                clan_id,
                channel_id,
                message_id,
                content,
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
        self.transport
            .create_poll(
                channel_id,
                clan_id,
                question,
                answers,
                expire_hours,
                poll_type,
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
        self.transport
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
        topic_id: i64,
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
                topic_id,
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
        self.transport
            .write_last_pin_message(
                clan_id,
                channel_id,
                message_id,
                mode,
                is_public,
                timestamp_seconds,
                operation,
                avatar,
                sender_id,
                sender_username,
                content,
                attachment,
                created_time,
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
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message(
                clan_id, channel_id, content, is_public, mode, mentions, hashtags, emojis, ogp,
            )
            .await
    }

    /// Send a message into a discussion topic (mezon-js `writeChatMessage(..., topicId)`).
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
    ) -> Result<ApiMessage> {
        self.transport
            .send_topic_message(
                clan_id, channel_id, content, is_public, mode, topic_id, mentions, hashtags,
                emojis, reply, flags,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_topic_message_with_attachment_urls(
        &self,
        clan_id: i64,
        channel_id: i64,
        is_public: bool,
        mode: i32,
        topic_id: i64,
        attachments: Vec<UrlAttachment>,
        reply: Option<crate::transport::OutgoingReply>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<ApiMessage> {
        let proto: Vec<mezon_proto::api::MessageAttachment> = attachments
            .iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename.clone(),
                size: 0,
                url: a.url.clone(),
                filetype: a.filetype.clone(),
                width: a.width,
                height: a.height,
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
                width: a.width,
                height: a.height,
                thumbnail: String::new(),
                duration: 0,
                size: 0,
            })
            .collect();
        let mut sent = self
            .transport
            .send_topic_message_with_attachments(
                clan_id,
                channel_id,
                "",
                is_public,
                mode,
                topic_id,
                proto,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
                reply,
                flags,
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
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
        ogp: Option<crate::transport::OutgoingOgp>,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_reply(
                clan_id, channel_id, content, is_public, mode, reply, mentions, hashtags, emojis,
                ogp, flags,
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
        request: mezon_proto::api::Message2InboxRequest,
    ) -> Result<()> {
        self.transport.create_message_2_inbox(request).await
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
        self.transport
            .write_ephemeral_message(
                receiver_id,
                clan_id,
                channel_id,
                content,
                is_public,
                mode,
                mentions,
                hashtags,
                emojis,
                attachments,
                reply,
            )
            .await
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
        self.transport
            .write_message_typing(
                clan_id,
                channel_id,
                mode,
                is_public,
                sender_display_name,
                topic_id,
            )
            .await
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
        self.transport
            .write_quick_menu_event(
                menu_name,
                clan_id,
                channel_id,
                mode,
                is_public,
                content_json,
                mentions,
                attachments,
                references,
                anonymous_message,
                mention_everyone,
                avatar,
                message_code,
                topic_id,
                message_id,
                message_sender_id,
            )
            .await
    }

    pub async fn list_quick_menu_access(
        &self,
        channel_id: i64,
        menu_type: i32,
    ) -> Result<Vec<mezon_proto::api::QuickMenuAccess>> {
        let list = self
            .transport
            .list_quick_menu_access(0, channel_id, menu_type)
            .await?;
        Ok(list.list_menus)
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
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_with_flags(
                clan_id, channel_id, content, is_public, mode, mentions, hashtags, emojis, ogp,
                flags,
            )
            .await
    }

    pub async fn send_channel_message_prebuilt(
        &self,
        clan_id: i64,
        channel_id: i64,
        content_json: &str,
        is_public: bool,
        mode: i32,
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<ApiMessage> {
        self.transport
            .send_channel_message_prebuilt(
                clan_id,
                channel_id,
                content_json,
                is_public,
                mode,
                flags,
            )
            .await
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
        attachments: Vec<ApiAttachment>,
        mentions: Vec<crate::transport::OutgoingMention>,
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
            .forward_channel_message(
                clan_id,
                channel_id,
                content_raw,
                text,
                is_public,
                mode,
                proto,
                mentions,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
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
        topic_id: i64,
        is_update_msg_topic: bool,
        create_time_seconds: u32,
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
                clan_id,
                channel_id,
                message_id,
                content,
                proto,
                mode,
                is_public,
                topic_id,
                is_update_msg_topic,
                create_time_seconds,
            )
            .await
    }

    pub async fn list_emojis_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanEmoji>> {
        let resp = self.transport.list_emojis_by_user_id().await?;
        Ok(resp.emoji_list)
    }

    pub async fn list_events(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::EventManagement>> {
        Ok(self.transport.list_events(clan_id).await?.events)
    }

    pub async fn create_event(
        &self,
        request: mezon_proto::api::CreateEventRequest,
    ) -> Result<mezon_proto::api::EventManagement> {
        self.transport.create_event(request).await
    }

    pub async fn emoji_recent_list(&self) -> Result<Vec<mezon_proto::api::EmojiRecent>> {
        let resp = self.transport.emoji_recent_list().await?;
        Ok(resp.emoji_recents)
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
        self.transport
            .create_clan_emoji(clan_id, source, shortname, category, id, is_for_sale)
            .await
    }

    pub async fn update_clan_emoji_by_id(
        &self,
        id: i64,
        shortname: &str,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .update_clan_emoji_by_id(id, shortname, clan_id)
            .await
    }

    pub async fn delete_clan_emoji_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        self.transport.delete_clan_emoji_by_id(id, clan_id).await
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
        self.transport
            .add_clan_sticker(
                clan_id,
                source,
                shortname,
                category,
                id,
                media_type,
                is_for_sale,
            )
            .await
    }

    pub async fn update_clan_sticker_by_id(
        &self,
        id: i64,
        clan_id: i64,
        source: &str,
        shortname: &str,
        category: &str,
    ) -> Result<()> {
        self.transport
            .update_clan_sticker_by_id(id, clan_id, source, shortname, category)
            .await
    }

    pub async fn delete_clan_sticker_by_id(&self, id: i64, clan_id: i64) -> Result<()> {
        self.transport.delete_clan_sticker_by_id(id, clan_id).await
    }

    pub async fn upload_emoticon(
        &self,
        folder: &str,
        id: i64,
        extension: &str,
        filetype: &str,
        data: Vec<u8>,
    ) -> Result<(i64, String)> {
        let ext = extension.trim_start_matches('.');
        let filename = format!("{}/{id}.{ext}", folder.trim_matches('/'));
        let size = clamp_i32(data.len());
        let (width, height) = if filetype.starts_with("image/") {
            image_dimensions(&data)
        } else {
            (0, 0)
        };
        let upload = self
            .transport
            .upload_attachment_file(&filename, filetype, size, width, height)
            .await?;
        crate::transport_runtime::put_bytes_to_content_type(&upload.url, data, filetype).await?;
        let resolved_id = emoticon_id_from_filename(&upload.filename).ok_or_else(|| {
            anyhow::anyhow!("invalid emoticon upload filename: {}", upload.filename)
        })?;
        let url = attachment_cdn_url(&self.base_img_url, &upload.filename)?;
        Ok((resolved_id, url))
    }

    pub async fn list_roles(
        &self,
        clan_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleListEventResponse> {
        self.transport.list_roles(clan_id, limit, cursor).await
    }

    pub async fn create_role(
        &self,
        request: mezon_proto::api::CreateRoleRequest,
    ) -> Result<mezon_proto::api::Role> {
        self.transport.create_role(request).await
    }

    pub async fn update_role(&self, request: mezon_proto::api::UpdateRoleRequest) -> Result<()> {
        self.transport.update_role(request).await
    }

    pub async fn list_role_users(
        &self,
        role_id: i64,
        limit: i32,
        cursor: &str,
    ) -> Result<mezon_proto::api::RoleUserList> {
        self.transport.list_role_users(role_id, limit, cursor).await
    }

    pub async fn delete_role(&self, role_id: i64, clan_id: i64) -> Result<()> {
        self.transport.delete_role(role_id, clan_id).await
    }

    pub async fn update_role_order(&self, clan_id: i64, roles: &[(i32, i64)]) -> Result<()> {
        self.transport.update_role_order(clan_id, roles).await
    }

    pub async fn list_role_permissions(
        &self,
        role_id: i64,
    ) -> Result<mezon_proto::api::PermissionList> {
        self.transport.list_role_permissions(role_id).await
    }

    pub async fn get_list_permission(&self) -> Result<mezon_proto::api::PermissionList> {
        self.transport.get_list_permission().await
    }

    pub async fn get_permission_by_role_id_channel_id(
        &self,
        role_id: i64,
        channel_id: i64,
        user_id: i64,
    ) -> Result<mezon_proto::api::PermissionRoleChannelListEventResponse> {
        self.transport
            .get_permission_by_role_id_channel_id(role_id, channel_id, user_id)
            .await
    }

    pub async fn set_role_channel_permission(
        &self,
        role_id: i64,
        channel_id: i64,
        user_id: i64,
        max_permission_id: i64,
        permission_update: Vec<mezon_proto::api::PermissionUpdate>,
    ) -> Result<()> {
        self.transport
            .set_role_channel_permission(
                role_id,
                channel_id,
                user_id,
                max_permission_id,
                permission_update,
            )
            .await
    }

    pub async fn add_roles_channel_desc(
        &self,
        role_ids: Vec<String>,
        channel_id: i64,
    ) -> Result<()> {
        self.transport
            .add_roles_channel_desc(role_ids, channel_id)
            .await
    }

    pub async fn delete_role_channel_desc(
        &self,
        role_id: i64,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .delete_role_channel_desc(role_id, channel_id, clan_id)
            .await
    }

    pub async fn update_channel_private(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_private: i32,
        user_ids: Vec<i64>,
        role_ids: Vec<i64>,
    ) -> Result<()> {
        self.transport
            .update_channel_private(clan_id, channel_id, channel_private, user_ids, role_ids)
            .await
    }

    pub async fn change_channel_category(
        &self,
        clan_id: i64,
        channel_id: i64,
        new_category_id: i64,
    ) -> Result<()> {
        self.transport
            .change_channel_category(clan_id, channel_id, new_category_id)
            .await
    }

    pub async fn remove_channel_users(&self, channel_id: i64, user_ids: Vec<String>) -> Result<()> {
        self.transport
            .remove_channel_users(channel_id, user_ids)
            .await
    }

    pub async fn get_clan_user_role(&self, clan_id: i64) -> Result<mezon_proto::api::RoleList> {
        self.transport.get_clan_user_role(clan_id, 0).await
    }

    pub async fn list_channel_setting_page(
        &self,
        clan_id: i64,
        parent_id: i64,
        limit: i32,
        page: i32,
    ) -> Result<mezon_proto::api::ChannelSettingListResponse> {
        self.transport
            .list_channel_setting_page(clan_id, parent_id, limit, page, "")
            .await
    }

    pub async fn list_stickers_by_user_id(&self) -> Result<Vec<mezon_proto::api::ClanSticker>> {
        let resp = self.transport.list_stickers_by_user_id().await?;
        Ok(resp.stickers)
    }

    pub async fn list_webhooks_by_channel(
        &self,
        channel_id: i64,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::Webhook>> {
        let resp = self
            .transport
            .list_webhook_by_channel_id(channel_id, clan_id)
            .await?;
        Ok(resp.webhooks)
    }

    pub async fn generate_webhook(
        &self,
        request: mezon_proto::api::WebhookCreateRequest,
    ) -> Result<mezon_proto::api::WebhookGenerateResponse> {
        self.transport.generate_webhook(request).await
    }

    pub async fn update_webhook(
        &self,
        request: mezon_proto::api::WebhookUpdateRequestById,
    ) -> Result<()> {
        self.transport.update_webhook_by_id(request).await
    }

    pub async fn delete_webhook(
        &self,
        request: mezon_proto::api::WebhookDeleteRequestById,
    ) -> Result<()> {
        self.transport.delete_webhook_by_id(request).await
    }

    pub async fn list_clan_webhooks(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::ClanWebhook>> {
        let resp = self.transport.list_clan_webhook(clan_id).await?;
        Ok(resp.list_clan_webhooks)
    }

    pub async fn generate_clan_webhook(
        &self,
        request: mezon_proto::api::GenerateClanWebhookRequest,
    ) -> Result<mezon_proto::api::GenerateClanWebhookResponse> {
        self.transport.generate_clan_webhook(request).await
    }

    pub async fn update_clan_webhook(
        &self,
        request: mezon_proto::api::UpdateClanWebhookRequest,
    ) -> Result<()> {
        self.transport.update_clan_webhook_by_id(request).await
    }

    pub async fn delete_clan_webhook(&self, id: i64, clan_id: i64) -> Result<()> {
        self.transport.delete_clan_webhook_by_id(id, clan_id).await
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
            let api = self.clone();
            futures::stream::iter(media_urls.iter().cloned().map(move |url| {
                let api = api.clone();
                async move { api.upload_media_from_url(&url).await }
            }))
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
                crate::transport::OutgoingMessageFlags::default(),
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
            duration,
            thumbnail,
        } = file;
        let upload_name = sanitize_upload_filename(&filename);
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
                    filename: upload_name.clone(),
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
                .upload_attachment_file(&upload_name, &filetype, size, width, height)
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
                duration,
            },
            plan,
        })
    }

    pub async fn upload_presigned_all(&self, presigned: Vec<PresignedAttachment>) -> Result<()> {
        use futures::StreamExt as _;
        futures::stream::iter(presigned.into_iter().map(|item| self.execute_upload(item)))
            .buffered(ATTACHMENT_UPLOAD_CONCURRENCY)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(())
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
        presign_finish: Option<Vec<String>>,
        flags: crate::transport::OutgoingMessageFlags,
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
                presign_finish,
                flags,
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_topic_presigned_message(
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
            .send_topic_message_with_attachments(
                clan_id,
                channel_id,
                content,
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
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        presign_finish: Vec<String>,
        create_time_seconds: u32,
        mode: i32,
        is_public: bool,
        topic_id: i64,
        is_update_msg_topic: bool,
    ) -> Result<()> {
        self.transport
            .patch_message_presign_finish(
                clan_id,
                channel_id,
                message_id,
                content,
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
        hashtags: Vec<crate::transport::OutgoingHashtag>,
        emojis: Vec<crate::transport::OutgoingEmoji>,
        create_time_seconds: u32,
        presigned: Vec<PresignedAttachment>,
        keys: Vec<String>,
        mode: i32,
        is_public: bool,
        topic_id: i64,
        is_update_msg_topic: bool,
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
                        hashtags.clone(),
                        emojis.clone(),
                        finished.clone(),
                        create_time_seconds,
                        mode,
                        is_public,
                        topic_id,
                        is_update_msg_topic,
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
                    hashtags,
                    emojis,
                    finished,
                    create_time_seconds,
                    mode,
                    is_public,
                    topic_id,
                    is_update_msg_topic,
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
        flags: crate::transport::OutgoingMessageFlags,
    ) -> Result<ApiMessage> {
        let proto: Vec<mezon_proto::api::MessageAttachment> = attachments
            .iter()
            .map(|a| mezon_proto::api::MessageAttachment {
                filename: a.filename.clone(),
                size: 0,
                url: a.url.clone(),
                filetype: a.filetype.clone(),
                width: a.width,
                height: a.height,
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
                width: a.width,
                height: a.height,
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
                flags,
            )
            .await?;
        sent.attachments = echo;
        Ok(sent)
    }

    async fn upload_thumbnail(&self, thumbnail: UploadThumbnail) -> String {
        let filename = sanitize_upload_filename(&thumbnail.filename);
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

    async fn upload_avatar_bytes(
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
        crate::transport_runtime::put_bytes_to_content_type(&upload.url, data, filetype).await?;
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
            stem.to_string()
        } else {
            format!("{stem}.{ext}")
        };
        let upload_name = sanitize_upload_filename(&filename);
        let size = clamp_i32(data.len());

        let (width, height) = if filetype.starts_with("image/") {
            image_dimensions(&data)
        } else {
            (0, 0)
        };

        let url = self
            .upload_bytes(&upload_name, &filetype, size, width, height, data)
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
        logo: Option<&str>,
    ) -> Result<()> {
        self.transport
            .update_account(display_name, avatar_url, about_me, logo)
            .await
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
        self.transport
            .registration_password(email, password, old_password)
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

    pub async fn check_duplicate_channel_name(
        &self,
        name: &str,
        category_id: &str,
    ) -> Result<bool> {
        let cond: i64 = category_id
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid category_id {category_id:?}: {e}"))?;
        let resp = self
            .transport
            .check_duplicate_name(name, CHECK_NAME_TYPE_CHANNEL, cond)
            .await?;
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
        if data.len() > 10 * 1024 * 1024 {
            anyhow::bail!("image exceeds the 10 MB avatar limit");
        }

        let raw_filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("avatar")
            .to_string();
        let filename = sanitize_upload_filename(&raw_filename);
        let format =
            image::guess_format(&data).map_err(|_| anyhow::anyhow!("unsupported image type"))?;
        let filetype = match format {
            image::ImageFormat::Jpeg => "image/jpeg",
            image::ImageFormat::Png => "image/png",
            image::ImageFormat::Gif => "image/gif",
            image::ImageFormat::WebP => "image/webp",
            _ => anyhow::bail!("unsupported image type; use JPEG, PNG, GIF, or WEBP"),
        };
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
            .upload_avatar_bytes(&filename, filetype, size, width, height, data)
            .await?;

        tracing::info!("Avatar upload complete: url={}", permanent_url);

        Ok(permanent_url)
    }

    pub async fn list_categories_typed(&self, clan_id: i64) -> Result<Vec<ApiCategoryDesc>> {
        self.transport.list_categories_typed(clan_id).await
    }

    pub async fn update_category_order(
        &self,
        clan_id: i64,
        categories: &[(i32, i64)],
    ) -> Result<()> {
        self.transport
            .update_category_order(clan_id, categories)
            .await
    }

    pub async fn list_archived_channel_descs(
        &self,
        clan_id: i64,
    ) -> Result<Vec<mezon_proto::api::ChannelDescription>> {
        Ok(self
            .transport
            .list_archived_channel_descs(clan_id)
            .await?
            .channeldesc)
    }

    pub async fn restore_archived_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        self.transport
            .active_archived_thread(clan_id, channel_id)
            .await
    }

    pub async fn archive_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        self.transport.archive_channel(clan_id, channel_id).await
    }

    pub async fn delete_channel(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        self.transport.delete_channel(clan_id, channel_id).await
    }

    pub async fn list_clan_badge_count(&self) -> Result<Vec<(String, i32, bool)>> {
        self.transport.list_clan_badge_count().await
    }

    pub async fn get_notification_clan(&self, clan_id: i64) -> Result<i32> {
        self.transport.get_notification_clan(clan_id).await
    }

    pub async fn get_notification_category(&self, category_id: i64) -> Result<i32> {
        let dto = self
            .transport
            .get_notification_category(category_id)
            .await?;
        Ok(dto.notification_setting_type)
    }

    pub async fn get_notification_category_setting(
        &self,
        category_id: i64,
    ) -> Result<crate::ChannelNotificationSetting> {
        let dto = self
            .transport
            .get_notification_category(category_id)
            .await?;
        Ok(crate::ChannelNotificationSetting::from_api(&dto))
    }

    pub async fn set_notification_clan_setting(
        &self,
        clan_id: i64,
        notification_type: i32,
    ) -> Result<()> {
        self.transport
            .set_notification_clan_setting(clan_id, notification_type)
            .await
    }

    pub async fn set_notification_category_setting(
        &self,
        category_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .set_notification_category_setting(category_id, notification_type, clan_id)
            .await
    }

    pub async fn set_mute_category(
        &self,
        category_id: i64,
        mute_seconds: i32,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .set_mute_category(category_id, mute_seconds, clan_id)
            .await
    }

    pub async fn delete_notification_category_setting(&self, category_id: i64) -> Result<()> {
        self.transport
            .delete_notification_category_setting(category_id)
            .await
    }

    pub async fn get_channel_category_noti_settings_list(
        &self,
        clan_id: i64,
    ) -> Result<Vec<crate::NotificationOverride>> {
        let list = self
            .transport
            .get_channel_category_noti_settings_list(clan_id)
            .await?;
        Ok(list
            .notification_channel_category_settings_list
            .iter()
            .map(crate::NotificationOverride::from_api)
            .collect())
    }

    pub async fn get_notification_channel(
        &self,
        channel_id: i64,
    ) -> Result<crate::ChannelNotificationSetting> {
        let dto = self.transport.get_notification_channel(channel_id).await?;
        Ok(crate::ChannelNotificationSetting::from_api(&dto))
    }

    pub async fn set_notification_channel_setting(
        &self,
        channel_id: i64,
        notification_type: i32,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .set_notification_channel_setting(channel_id, notification_type, clan_id)
            .await
    }

    pub async fn delete_notification_channel(&self, channel_id: i64) -> Result<()> {
        self.transport.delete_notification_channel(channel_id).await
    }

    pub async fn set_mute_channel(
        &self,
        channel_id: i64,
        mute_seconds: i32,
        clan_id: i64,
    ) -> Result<()> {
        self.transport
            .set_mute_channel(channel_id, mute_seconds, clan_id)
            .await
    }

    pub async fn list_muted_channels(&self, clan_id: i64) -> Result<Vec<String>> {
        self.transport.list_muted_channels(clan_id).await
    }

    pub fn spawn_gotify_stream(
        &self,
        ws_base: String,
        token: String,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<crate::gotify::GotifyNotification>,
        tokio::sync::oneshot::Receiver<crate::gotify::StreamEnd>,
    ) {
        self.transport.spawn_gotify_stream(ws_base, token)
    }

    /// Download a notification's sender avatar to a temp file for use as the OS
    /// notification icon. Rejects non-https URLs; size/timeout are bounded by
    /// [`crate::transport_runtime::fetch_bytes`].
    pub async fn download_notification_icon(&self, url: &str) -> Result<std::path::PathBuf> {
        if !url.starts_with("https://") {
            anyhow::bail!("notification icon url must be https");
        }
        let (bytes, _) = crate::transport_runtime::fetch_bytes(url).await?;
        crate::transport_runtime::write_temp_icon(bytes).await
    }

    /// Register a device token and return `(notify_token, device_id)`. The notify
    /// token opens the Gotify stream; the device id is cached and reused as the
    /// request token on the next registration, matching React.
    pub async fn regist_fcm_device_token(
        &self,
        token: &str,
        device_id: &str,
        platform: &str,
    ) -> Result<(String, String)> {
        self.transport
            .regist_fcm_device_token(
                token.to_string(),
                device_id.to_string(),
                platform.to_string(),
            )
            .await
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

    pub async fn list_streaming_channel_users(
        &self,
        clan_id: i64,
        channel_id: i64,
        channel_type: i32,
        state: i32,
        limit: i32,
    ) -> Result<mezon_proto::api::StreamingChannelUserList> {
        self.transport
            .list_streaming_channel_users(clan_id, channel_id, channel_type, state, limit)
            .await
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

    pub async fn add_agent_to_channel(&self, channel_id: i64, room_name: &str) -> Result<()> {
        self.transport
            .add_agent_to_channel(channel_id, room_name)
            .await
    }

    pub async fn disconnect_agent(&self, channel_id: i64, room_name: &str) -> Result<()> {
        self.transport.disconnect_agent(channel_id, room_name).await
    }

    pub async fn list_channel_apps(&self, clan_id: i64) -> Result<Vec<ApiChannelApp>> {
        self.transport.list_channel_apps(clan_id).await
    }

    pub async fn generate_hash_channel_apps(
        &self,
        app_id: i64,
    ) -> Result<mezon_proto::api::GenerateHashChannelAppsResponse> {
        self.transport.generate_hash_channel_apps(app_id).await
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

    pub async fn active_archived_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        self.transport
            .active_archived_thread(clan_id, channel_id)
            .await
    }

    pub async fn leave_thread(&self, clan_id: i64, channel_id: i64) -> Result<()> {
        self.transport.leave_thread(clan_id, channel_id).await
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

    pub async fn create_link_invite_user(
        &self,
        clan_id: i64,
        channel_id: i64,
        expiry_time: i32,
    ) -> Result<mezon_proto::api::LinkInviteUser> {
        self.transport
            .create_link_invite_user(clan_id, channel_id, expiry_time)
            .await
    }

    pub async fn invite_user(&self, invite_id: i64) -> Result<mezon_proto::api::InviteUserRes> {
        self.transport.invite_user(invite_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MULTIPART_PART_SIZE, attachment_cdn_url, multipart_part_ranges, sanitize_upload_filename,
    };

    #[test]
    fn upload_filename_folds_every_non_ascii_char() {
        assert_eq!(
            sanitize_upload_filename("Báo cáo tháng 8.pdf"),
            "B_o_c_o_th_ng_8.pdf"
        );
        assert_eq!(sanitize_upload_filename("日本語.png"), "___.png");
        assert!(
            sanitize_upload_filename("Đơn xin nghỉ phép.pdf")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        );
    }

    #[test]
    fn upload_filename_keeps_ascii_stem_and_extension() {
        assert_eq!(sanitize_upload_filename("report-v2.pdf"), "report_v2.pdf");
        assert_eq!(sanitize_upload_filename("a b.PNG"), "a_b.PNG");
    }

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
                "https://cdn.example",
                "mezon/1826814768338440192/2074336632294608896.png",
            )
            .unwrap(),
            "https://cdn.example/mezon/1826814768338440192/2074336632294608896.png"
        );
    }

    #[test]
    fn attachment_url_trims_trailing_slash_on_base() {
        assert_eq!(
            attachment_cdn_url("https://cdn.example/", "x.png").unwrap(),
            "https://cdn.example/x.png"
        );
    }

    #[test]
    fn attachment_url_errors_when_filename_empty() {
        assert!(attachment_cdn_url("https://cdn.example", "").is_err());
    }
}
