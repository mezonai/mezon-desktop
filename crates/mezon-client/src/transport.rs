/// Main Mezon transport client with WebSocket/TCP support and REST API methods.
///
/// Handles connection management, message routing, and provides typed API methods
/// for interacting with the Mezon backend.
use crate::transport_adapter::TransportAdapter;
use anyhow::{Context, Result};
use mezon_proto::{api, realtime};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, oneshot};

const DEFAULT_TIMEOUT_MS: u64 = 7000;
const DEFAULT_SEND_TIMEOUT_MS: u64 = 10000;

/// Promise executor for matching responses to requests.
struct PromiseExecutor {
    sender: oneshot::Sender<(u32, Vec<u8>)>,
}

/// Main transport client.
pub struct MezonTransport {
    adapter: Arc<Mutex<Box<dyn TransportAdapter>>>,
    cid_counter: Arc<AtomicU16>,
    pending_requests: Arc<RwLock<HashMap<u16, PromiseExecutor>>>,
    timeout_ms: Duration,
    send_timeout_ms: Duration,
    #[allow(dead_code)]
    base_path: String,
    verbose: bool,
}

impl MezonTransport {
    /// Create a new transport with the given adapter.
    pub fn new(adapter: Box<dyn TransportAdapter>, base_path: String) -> Self {
        Self {
            adapter: Arc::new(Mutex::new(adapter)),
            cid_counter: Arc::new(AtomicU16::new(1)),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            timeout_ms: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            send_timeout_ms: Duration::from_millis(DEFAULT_SEND_TIMEOUT_MS),
            base_path,
            verbose: false,
        }
    }

    /// Set verbose logging.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.verbose = verbose;
    }

    /// Set request timeout.
    pub fn set_timeout(&mut self, timeout_ms: u64) {
        self.timeout_ms = Duration::from_millis(timeout_ms);
    }

    /// Generate a unique correlation ID.
    fn generate_cid(&self) -> u16 {
        self.cid_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Connect to the Mezon backend.
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
        token: &str,
        on_message: impl Fn(u16, u32, Vec<u8>) + Send + Sync + 'static,
        on_disconnected: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<()> {
        tracing::info!("🌐 MezonTransport::connect() starting");
        tracing::debug!("  Host: {}, Port: {}", host, port);
        tracing::debug!("  Token: {}...", &token[..token.len().min(20)]);

        tracing::debug!("Acquiring adapter lock...");
        let mut adapter = self.adapter.lock().await;
        tracing::debug!("  Adapter lock acquired");

        // Set up message handler
        tracing::debug!("Setting up message handler...");
        let pending_requests = self.pending_requests.clone();
        let verbose = self.verbose;
        adapter.set_on_message(Arc::new(move |cid, code, message| {
            if verbose {
                tracing::debug!(
                    "📨 Incoming message: cid={}, code={}, len={}",
                    cid,
                    code,
                    message.len()
                );
            }

            if cid != 0 {
                // Response to a request
                tracing::trace!("  Response for request cid={}", cid);
                let pending = pending_requests.clone();
                tokio::spawn(async move {
                    let mut pending_guard = pending.write().await;
                    if let Some(executor) = pending_guard.remove(&cid) {
                        tracing::trace!("  Resolving promise for cid={}", cid);
                        let _ = executor.sender.send((code, message));
                    } else if verbose {
                        tracing::warn!("⚠️  No pending request for cid={}", cid);
                    }
                });
            } else {
                // Server-initiated message
                tracing::debug!("  Server-initiated message");
                on_message(cid, code, message);
            }
        }));
        tracing::debug!("  Message handler set");

        // Set up close handler
        tracing::debug!("Setting up close handler...");
        adapter.set_on_close(Arc::new(on_disconnected));
        tracing::debug!("  Close handler set");

        // Connect
        tracing::info!("🔌 Calling adapter.connect()...");
        tracing::debug!("Host: {}, Port: {}, Token: {}...", host, port, token);
        adapter
            .connect(host, port, token)
            .await
            .with_context(|| format!("Failed to connect adapter to {host}:{port}"))?;

        tracing::info!("✅ MezonTransport::connect() completed successfully");
        Ok(())
    }

    /// Send a raw message and wait for response.
    pub async fn send(&self, cid: u16, message: Vec<u8>) -> Result<(u32, Vec<u8>)> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(cid, PromiseExecutor { sender: tx });
        }
        {
            let mut adapter = self.adapter.lock().await;
            adapter.send(message).await?;
        }
        let result = tokio::time::timeout(self.send_timeout_ms, rx)
            .await
            .map_err(|_| {
                let pending = self.pending_requests.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&cid);
                });
                anyhow::anyhow!("Request timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?;
        Ok(result)
    }

    /// Check if the adapter is connected.
    pub async fn is_open(&self) -> bool {
        let adapter = self.adapter.lock().await;
        adapter.is_open()
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<()> {
        let mut adapter = self.adapter.lock().await;
        adapter.close().await
    }

    /// Send a ping.
    pub async fn ping(&self, cid: u16) -> Result<()> {
        let mut adapter = self.adapter.lock().await;
        adapter.send_ping(cid).await
    }

    /// Send a ping and wait for matching pong.
    pub async fn ping_roundtrip(&self) -> Result<()> {
        let cid = self.generate_cid();
        tracing::info!("🏓 MezonTransport::ping_roundtrip() cid={}", cid);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(cid, PromiseExecutor { sender: tx });
            tracing::debug!(
                "  Registered ping pending request. Total pending: {}",
                pending.len()
            );
        }

        {
            let mut adapter = self.adapter.lock().await;
            tracing::info!("🏓 Sending ping cid={}", cid);
            adapter.send_ping(cid).await?;
        }

        tokio::time::timeout(self.send_timeout_ms, rx)
            .await
            .map_err(|_| {
                tracing::error!(
                    "✗ Ping timed out after {} ms",
                    self.send_timeout_ms.as_millis()
                );
                let pending = self.pending_requests.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&cid);
                });
                anyhow::anyhow!("Ping timed out")
            })?
            .map_err(|_| anyhow::anyhow!("Ping response channel closed"))?;

        tracing::info!("🏓 Pong received for cid={}", cid);
        Ok(())
    }
}

// ============================================================================
// API Methods - Hot Path (frequently called)
// ============================================================================

/// API response types (simplified - expand as needed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiAccount {
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSession {
    pub token: String,
    pub refresh_token: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiChannelDesc {
    pub channel_id: String,
    pub channel_label: String,
    pub channel_type: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClanDesc {
    pub clan_id: String,
    pub clan_name: String,
    pub creator_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMessage {
    pub message_id: String,
    pub content: String,
    pub sender_id: String,
    pub create_time: i64,
}

impl MezonTransport {
    /// Build a protobuf-encoded API request envelope.
    ///
    /// Wire format: Envelope { cid: uint32, api_request_event: ApiRequestEvent }
    fn build_api_request(&self, cid: u16, api_name: &str, body: Vec<u8>) -> Vec<u8> {
        let envelope = realtime::Envelope {
            cid: i32::from(cid),
            message: Some(realtime::envelope::Message::ApiRequestEvent(
                realtime::ApiRequestEvent {
                    api_index: self.get_api_index(api_name) as i32,
                    api_name: api_name.to_string(),
                    body,
                },
            )),
        };
        envelope.encode_to_vec()
    }

    async fn send_api_request(
        &self,
        cid: u16,
        api_name: &str,
        body: Vec<u8>,
    ) -> Result<(u32, Vec<u8>)> {
        self.send(cid, self.build_api_request(cid, api_name, body))
            .await
    }

    fn account_from_user(user: api::User, email: Option<String>) -> ApiAccount {
        ApiAccount {
            user_id: user.id.to_string(),
            username: user.username,
            email,
            display_name: (!user.display_name.is_empty()).then_some(user.display_name),
        }
    }

    fn channel_desc_from_proto(channel: api::ChannelDescription) -> ApiChannelDesc {
        ApiChannelDesc {
            channel_id: channel.channel_id.to_string(),
            channel_label: channel.channel_label,
            channel_type: channel.r#type as u32,
        }
    }

    fn clan_desc_from_proto(clan: api::ClanDesc) -> ApiClanDesc {
        ApiClanDesc {
            clan_id: clan.clan_id.to_string(),
            clan_name: clan.clan_name,
            creator_id: clan.creator_id.to_string(),
        }
    }

    fn message_from_proto(message: api::ChannelMessage) -> ApiMessage {
        ApiMessage {
            message_id: message.message_id.to_string(),
            content: message.content,
            sender_id: message.sender_id.to_string(),
            create_time: i64::from(message.create_time_seconds),
        }
    }

    /// Get API index from API name (matches TypeScript ApiNameEnum order)
    fn get_api_index(&self, api_name: &str) -> u32 {
        match api_name {
            // HOT PATH
            "ListChannelDescs" => 0,
            "GetAccount" => 1,
            "ListClanDescs" => 2,
            "ListClanUsers" => 3,
            "ListRoles" => 4,
            "ListEvents" => 5,
            "GetRoleOfUserInTheClan" => 6,
            "GetListPermission" => 7,
            "ListUserPermissionInChannel" => 8,
            "GetNotificationClan" => 9,
            "ListMutedChannel" => 10,
            "ListStreamingChannelUsers" => 11,
            "ListQuickMenuAccess" => 12,
            "GetNotificationChannel" => 13,
            "ListFriends" => 14,
            "EmojiRecentList" => 15,
            "GetListEmojisByUserId" => 16,
            "ListClanBadgeCount" => 17,
            "ListChannelBadgeCount" => 18,
            "ListLogedDevice" => 19,
            "ListClanUsersStatus" => 20,
            "ListChannelApps" => 21,
            "GetListFavoriteChannel" => 22,
            "ListCategoryDescs" => 23,
            "ListOnboarding" => 24,
            "GetListStickersByUserId" => 25,
            "GetSystemMessageByClanId" => 26,
            "GetPinMessagesList" => 27,
            "GetChannelCanvasList" => 28,
            "ListChannelTimeline" => 29,
            "ListChannelMessages" => 30,
            "ListActivity" => 31,
            "ListChannelByUserId" => 32,
            "ListUserClansByUserId" => 33,
            "GetUserProfileOnClan" => 34,
            "RegistFCMDeviceToken" => 35,
            "IsBanned" => 36,
            "ListThreadDescs" => 37,
            "ListArchivedChannelDescs" => 38,
            "ListChannelDetail" => 39,
            "GetChannelCategoryNotiSettingsList" => 40,
            "ListRoleUsers" => 41,
            "ListChannelUsers" => 42,
            "ListChannelAttachment" => 43,
            "ListChannelVoiceUsers" => 44,
            "ListUserOnline" => 45,
            "ListNotifications" => 46,
            "ListChannelUsersUC" => 47,
            "ListWebhookByChannelId" => 48,
            "GetPermissionByRoleIdChannelId" => 49,
            "ListChannelSetting" => 50,
            "ListApps" => 51,
            "GetApp" => 52,
            "ListForSaleItems" => 53,
            "ListClanWebhook" => 54,
            "GetUserStatus" => 55,
            "ListSdTopic" => 56,
            // COLD PATH
            "AddFriends" => 57,
            "AddChannelUsers" => 58,
            "RegistrationEmail" => 59,
            "BlockFriends" => 60,
            "UnblockFriends" => 61,
            "UploadAttachmentFile" => 62,
            "UploadOauthFile" => 63,
            "AddRolesChannelDesc" => 64,
            "CreateCategoryDesc" => 65,
            "CreateChannelDesc" => 66,
            "CreateRole" => 67,
            "CreateEvent" => 68,
            "DeleteRole" => 69,
            "DeleteEvent" => 70,
            "DeleteRoleChannelDesc" => 71,
            "DeleteChannelDesc" => 72,
            "CloseDMByChannelId" => 73,
            "OpenDMByChannelId" => 74,
            "DeleteAccount" => 75,
            "DeleteFriends" => 76,
            "DeleteCategoryDesc" => 77,
            "DeleteNotifications" => 78,
            "DeleteClanDesc" => 79,
            "UpdateUser" => 80,
            "UpdateUserProfileByClan" => 81,
            "UpdateClanOrder" => 82,
            "RemoveChannelUsers" => 83,
            "LeaveThread" => 84,
            "ArchiveChannel" => 85,
            "LinkSMS" => 86,
            "ConfirmLinkMezonOTP" => 87,
            "LinkEmail" => 88,
            "CreateClanDesc" => 89,
            "RemoveClanUsers" => 90,
            "BanClanUsers" => 91,
            "CreateLinkInviteUser" => 92,
            "InviteUser" => 93,
            "SetRoleChannelPermission" => 94,
            "SetNotificationChannelSetting" => 95,
            "SetMuteChannel" => 96,
            "SetMuteCategory" => 97,
            "SetNotificationClanSetting" => 98,
            "SetNotificationCategorySetting" => 99,
            "DeleteNotificationCategorySetting" => 100,
            "DeleteNotificationChannel" => 101,
            "CreatePinMessage" => 102,
            "CreateMessage2Inbox" => 103,
            "UnlinkMezon" => 104,
            "UnlinkEmail" => 105,
            "UpdateAccount" => 106,
            "UpdateUsername" => 107,
            "UpdateCategory" => 108,
            "UpdateCategoryOrder" => 109,
            "UpdateRoleOrder" => 110,
            "UpdateClanDesc" => 111,
            "UpdateChannelDesc" => 112,
            "UpdateChannelPrivate" => 113,
            "UpdateRole" => 114,
            "UpdateEvent" => 115,
            "SearchMessage" => 116,
            "CreateClanEmoji" => 117,
            "DeleteByIdClanEmoji" => 118,
            "UpdateClanEmojiById" => 119,
            "GenerateWebhook" => 120,
            "HandleWebhook" => 121,
            "UpdateWebhookById" => 122,
            "DeleteWebhookById" => 123,
            "AddClanSticker" => 124,
            "UpdateClanStickerById" => 125,
            "DeleteClanStickerById" => 126,
            "ChangeChannelCategory" => 127,
            "CheckDuplicateName" => 128,
            "AddApp" => 129,
            "DeleteApp" => 130,
            "UpdateApp" => 131,
            "AddAppToClan" => 132,
            "CreateSystemMessage" => 133,
            "UpdateSystemMessage" => 134,
            "DeleteSystemMessage" => 135,
            "StreamingServerCallback" => 136,
            "EditChannelCanvases" => 137,
            "GetChannelCanvasDetail" => 138,
            "DeleteChannelCanvas" => 139,
            "AddChannelFavorite" => 140,
            "RemoveChannelFavorite" => 141,
            "CreateActiviy" => 142,
            "GetPubKeys" => 143,
            "PushPubKey" => 144,
            "GetChanEncryptionMethod" => 145,
            "SetChanEncryptionMethod" => 146,
            "GetKeyServer" => 147,
            "ListAuditLog" => 148,
            "GetOnboardingDetail" => 149,
            "CreateOnboarding" => 150,
            "UpdateOnboarding" => 151,
            "DeleteOnboarding" => 152,
            "ListOnboardingStep" => 153,
            "UpdateOnboardingStep" => 154,
            "GenerateClanWebhook" => 155,
            "UpdateClanWebhookById" => 156,
            "DeleteClanWebhookById" => 157,
            "HandleClanWebhook" => 158,
            "UpdateUserStatus" => 159,
            "UpdateUserCustomStatus" => 160,
            "GetTopicDetail" => 161,
            "CreateSdTopic" => 162,
            "DeleteSdTopic" => 163,
            "CreateExternalMezonMeet" => 164,
            "GenerateMeetToken" => 165,
            "RemoveParticipantMezonMeet" => 166,
            "MuteParticipantMezonMeet" => 167,
            "CreateRoomChannelApps" => 168,
            "GetMezonOauthClient" => 169,
            "DeleteMezonOauthClient" => 170,
            "UpdateMezonOauthClient" => 171,
            "SearchThread" => 172,
            "GenerateHashChannelApps" => 173,
            "DeleteUserEvent" => 174,
            "AddUserEvent" => 175,
            "DeleteQuickMenuAccess" => 176,
            "AddQuickMenuAccess" => 177,
            "UpdateQuickMenuAccess" => 178,
            "TransferOwnership" => 179,
            "SendChannelMessage" => 180,
            "UpdateChannelMessage" => 181,
            "DeleteChannelMessage" => 182,
            "ReportMessageAbuse" => 183,
            "MessageButtonClick" => 184,
            "DropdownBoxSelected" => 185,
            "ActiveArchivedThread" => 186,
            "UpdateChannelTimeline" => 187,
            "AddAgentToChannel" => 188,
            "DisconnectAgent" => 189,
            "CreateChannelTimeline" => 190,
            "DetailChannelTimeline" => 191,
            "CreatePoll" => 192,
            "VotePoll" => 193,
            "ClosePoll" => 194,
            "GetPoll" => 195,
            "ReactChannelMessage" => 196,
            "MultipartUploadAttachmentFileStart" => 197,
            "MultipartUploadAttachmentFileFinish" => 198,
            "SessionRefresh" => 199,
            "SessionLogout" => 200,
            "Healthcheck" => 201,
            "UnbanClanUsers" => 202,
            "ListBannedUsers" => 203,
            "GetNotificationCategory" => 204,
            "ListRolePermissions" => 205,
            "IsFollower" => 206,
            "DeletePinMessage" => 207,
            "MarkAsRead" => 208,
            _ => {
                tracing::warn!("Unknown API name: {}, using index 0", api_name);
                0
            }
        }
    }

    /// Get the current user's account.
    pub async fn get_account(&self) -> Result<ApiAccount> {
        tracing::info!("📞 MezonTransport::get_account() called");

        let cid = self.generate_cid();
        tracing::debug!("  Generated CID: {}", cid);

        // Build API request envelope
        let api_name = "GetAccount";
        let body = Vec::new();

        tracing::debug!("  Building API request envelope...");
        tracing::debug!("    API name: {}", api_name);
        tracing::debug!("    API index: {}", self.get_api_index(api_name));
        tracing::debug!("    Body len: {}", body.len());

        let request_bytes = self.build_api_request(cid, api_name, body);
        tracing::debug!("  Request envelope size: {} bytes", request_bytes.len());

        tracing::debug!("  Calling self.send() with cid={}...", cid);
        let send_result = self.send(cid, request_bytes).await;

        match send_result {
            Ok((code, response)) => {
                tracing::info!(
                    "✓ Received response: code={}, len={} bytes",
                    code,
                    response.len()
                );

                if code != 0 {
                    tracing::error!("✗ API error: code={}", code);
                    return Err(anyhow::anyhow!("API error: code={}", code));
                }

                if let Ok(envelope) = realtime::Envelope::decode(response.as_slice())
                    && let Some(realtime::envelope::Message::Error(error)) = envelope.message
                {
                    return Err(anyhow::anyhow!(
                        "GetAccount API error: code={} error={}",
                        error.code,
                        error.message
                    ));
                }

                let account = api::Account::decode(response.as_slice())?;
                let user = account.user.unwrap_or_default();
                let account = Self::account_from_user(
                    user,
                    (!account.email.is_empty()).then_some(account.email),
                );
                tracing::info!("✓ Decoded account response: {} bytes", response.len());
                Ok(account)
            }
            Err(e) => {
                tracing::error!("✗ self.send() failed: {}", e);
                Err(e)
            }
        }
    }

    /// List users in a clan.
    pub async fn list_clan_users(&self, clan_id: &str) -> Result<Vec<api::ClanUserList>> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        let list = api::ClanUserList::decode(response.as_slice())?;
        Ok(vec![list])
    }

    /// List channels in a clan.
    pub async fn list_channel_descs(&self, clan_id: &str) -> Result<Vec<ApiChannelDesc>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelDescs";
        let body = api::ListChannelDescsRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let channels = api::ChannelDescList::decode(response.as_slice())?;
        Ok(channels
            .channeldesc
            .into_iter()
            .map(Self::channel_desc_from_proto)
            .collect())
    }

    /// List roles in a clan.
    pub async fn list_roles(
        &self,
        clan_id: &str,
        limit: i32,
        cursor: &str,
    ) -> Result<api::RoleListEventResponse> {
        let cid = self.generate_cid();
        let body = api::RoleListEventRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            state: 0,
            cursor: cursor.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoles", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleListEventResponse::decode(response.as_slice())?)
    }

    /// List user's clans.
    pub async fn list_clan_descs(&self) -> Result<Vec<ApiClanDesc>> {
        let cid = self.generate_cid();

        let body = api::ListClanDescRequest::default().encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanDescs", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let clans = api::ClanDescList::decode(response.as_slice())?;
        Ok(clans
            .clandesc
            .into_iter()
            .map(Self::clan_desc_from_proto)
            .collect())
    }

    /// List users in a channel.
    pub async fn list_channel_users(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::ChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelUserList::decode(response.as_slice())?)
    }

    /// List messages in a channel.
    pub async fn list_channel_messages(
        &self,
        _channel_id: &str,
        _limit: u32,
    ) -> Result<Vec<ApiMessage>> {
        let cid = self.generate_cid();

        let api_name = "ListChannelMessages";
        let body = api::ListChannelMessagesRequest {
            channel_id: _channel_id.parse().unwrap_or_default(),
            limit: _limit as i32,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let messages = api::ChannelMessageList::decode(response.as_slice())?;
        Ok(messages
            .messages
            .into_iter()
            .map(Self::message_from_proto)
            .collect())
    }

    /// Send a message to a channel.
    pub async fn send_channel_message(
        &self,
        _channel_id: &str,
        content: &str,
    ) -> Result<ApiMessage> {
        let cid = self.generate_cid();

        let api_name = "SendChannelMessage";
        let body = realtime::ChannelMessageSend {
            channel_id: _channel_id.parse().unwrap_or_default(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let message = api::ChannelMessage::decode(response.as_slice())?;
        Ok(Self::message_from_proto(message))
    }

    /// List user's friends.
    pub async fn list_friends(&self) -> Result<Vec<ApiAccount>> {
        let cid = self.generate_cid();

        let api_name = "ListFriends";
        let body = api::ListFriendsRequest::default().encode_to_vec();

        let (code, response) = self.send_api_request(cid, api_name, body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let friends = api::FriendList::decode(response.as_slice())?;
        Ok(friends
            .friends
            .into_iter()
            .map(|friend| Self::account_from_user(friend.user.unwrap_or_default(), None))
            .collect())
    }

    /// List clan badge counts.
    pub async fn list_clan_badge_count(&self) -> Result<api::ListClanBadgeCountResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListClanBadgeCount", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List channel badge counts.
    pub async fn list_channel_badge_count(
        &self,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<api::ListChannelBadgeCountResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelBadgeCountRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            page,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelBadgeCount", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelBadgeCountResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List notifications.
    pub async fn list_notifications(
        &self,
        clan_id: &str,
        limit: i32,
    ) -> Result<api::NotificationList> {
        let cid = self.generate_cid();
        let body = api::ListNotificationsRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationList::decode(response.as_slice())?)
    }

    /// Get user profile on a clan.
    pub async fn get_user_profile_on_clan(&self, clan_id: &str) -> Result<api::ClanProfile> {
        let cid = self.generate_cid();
        let body = api::ClanProfileRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetUserProfileOnClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanProfile::decode(response.as_slice())?)
    }

    /// List category descriptions in a clan.
    pub async fn list_category_descs(&self, clan_id: &str) -> Result<api::CategoryDescList> {
        let cid = self.generate_cid();
        let body = api::CategoryDesc {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListCategoryDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDescList::decode(response.as_slice())?)
    }

    /// List channel description detail.
    pub async fn list_channel_detail(&self, channel_id: &str) -> Result<api::ChannelDescription> {
        let cid = self.generate_cid();
        let body = api::ListChannelDetailRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescription::decode(response.as_slice())?)
    }

    /// List thread descriptions.
    pub async fn list_thread_descs(
        &self,
        channel_id: &str,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<api::ChannelDescList> {
        let cid = self.generate_cid();
        let body = api::ListThreadRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListThreadDescs", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescList::decode(response.as_slice())?)
    }

    /// List channels by user ID.
    pub async fn list_channel_by_user_id(&self) -> Result<api::ChannelDescList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListChannelByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescList::decode(response.as_slice())?)
    }

    /// Get notification settings for a clan.
    pub async fn get_notification_clan(&self, _clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::NotificationClan {
            clan_id: _clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();

        let (code, _response) = self
            .send_api_request(cid, "GetNotificationClan", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// List events in a clan.
    pub async fn list_events(&self, clan_id: &str) -> Result<api::EventList> {
        let cid = self.generate_cid();
        let body = api::ListEventsRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListEvents", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventList::decode(response.as_slice())?)
    }

    /// List activity.
    pub async fn list_activity(&self) -> Result<api::ListUserActivity> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListActivity", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserActivity::decode(response.as_slice())?)
    }

    /// List channel apps.
    pub async fn list_channel_apps(&self, clan_id: &str) -> Result<api::ListChannelAppsResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelAppsRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListChannelApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelAppsResponse::decode(response.as_slice())?)
    }

    /// List emoji recent by user ID.
    pub async fn emoji_recent_list(&self) -> Result<api::EmojiRecentList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "EmojiRecentList", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiRecentList::decode(response.as_slice())?)
    }

    /// List emojis by user ID.
    pub async fn list_emojis_by_user_id(&self) -> Result<api::EmojiListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListEmojisByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EmojiListedResponse::decode(response.as_slice())?)
    }

    /// List stickers by user ID.
    pub async fn list_stickers_by_user_id(&self) -> Result<api::StickerListedResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListStickersByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StickerListedResponse::decode(response.as_slice())?)
    }

    /// Get system message by clan ID.
    pub async fn get_system_message_by_clan_id(&self, clan_id: &str) -> Result<api::SystemMessage> {
        let cid = self.generate_cid();
        let body = api::GetSystemMessage {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetSystemMessageByClanId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SystemMessage::decode(response.as_slice())?)
    }

    /// Get pin messages list.
    pub async fn get_pin_messages_list(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<api::PinMessagesList> {
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPinMessagesList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PinMessagesList::decode(response.as_slice())?)
    }

    /// List channel timeline.
    pub async fn list_channel_timeline(
        &self,
        clan_id: &str,
        channel_id: &str,
        year: i32,
        limit: i32,
    ) -> Result<api::ListChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::ListChannelTimelineRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            year,
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get role of user in clan.
    pub async fn get_role_of_user_in_clan(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::RoleList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionOfUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetRoleOfUserInTheClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleList::decode(response.as_slice())?)
    }

    /// Get list permission.
    pub async fn get_list_permission(&self) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetListPermission", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// List user permission in channel.
    pub async fn list_user_permission_in_channel(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::UserPermissionInChannelListResponse> {
        let cid = self.generate_cid();
        let body = api::UserPermissionInChannelListRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListUserPermissionInChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserPermissionInChannelListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get user status.
    pub async fn get_user_status(&self) -> Result<api::UserStatus> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetUserStatus", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserStatus::decode(response.as_slice())?)
    }

    /// List online users.
    pub async fn list_user_online(
        &self,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<api::ListUserOnlineResponse> {
        let cid = self.generate_cid();
        let body = api::ListUserOnlineRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            page,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListUserOnline", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListUserOnlineResponse::decode(response.as_slice())?)
    }

    /// List streaming channel users.
    pub async fn list_streaming_channel_users(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::StreamingChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListStreamingChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::StreamingChannelUserList::decode(response.as_slice())?)
    }

    /// List quick menu access.
    pub async fn list_quick_menu_access(
        &self,
        bot_id: &str,
        channel_id: &str,
        menu_type: i32,
    ) -> Result<api::QuickMenuAccessList> {
        let cid = self.generate_cid();
        let body = api::ListQuickMenuAccessRequest {
            bot_id: bot_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            menu_type,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::QuickMenuAccessList::decode(response.as_slice())?)
    }

    /// Get notification channel.
    pub async fn get_notification_channel(
        &self,
        channel_id: &str,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel {
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// Get notification category.
    pub async fn get_notification_category(
        &self,
        category_id: &str,
    ) -> Result<api::NotificationUserChannel> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory {
            category_id: category_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetNotificationCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationUserChannel::decode(response.as_slice())?)
    }

    /// List clan users status.
    pub async fn list_clan_users_status(&self, clan_id: &str) -> Result<api::ClanUserStatusList> {
        let cid = self.generate_cid();
        let body = api::ListClanUsersStatusRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListClanUsersStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanUserStatusList::decode(response.as_slice())?)
    }

    /// Get list favorite channels.
    pub async fn get_list_favorite_channel(
        &self,
        clan_id: &str,
    ) -> Result<api::ListFavoriteChannelResponse> {
        let cid = self.generate_cid();
        let body = api::ListFavoriteChannelRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetListFavoriteChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListFavoriteChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List logged devices.
    pub async fn list_loged_device(&self) -> Result<api::LogedDeviceList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListLogedDevice", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::LogedDeviceList::decode(response.as_slice())?)
    }

    /// List channel users (UC variant).
    pub async fn list_channel_users_uc(
        &self,
        channel_id: &str,
        limit: i32,
    ) -> Result<api::AllUsersAddChannelResponse> {
        let cid = self.generate_cid();
        let body = api::AllUsersAddChannelRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            limit,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelUsersUC", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUsersAddChannelResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List webhook by channel ID.
    pub async fn list_webhook_by_channel_id(
        &self,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<api::WebhookListResponse> {
        let cid = self.generate_cid();
        let body = api::WebhookListRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListWebhookByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::WebhookListResponse::decode(response.as_slice())?)
    }

    /// Get permission by role ID and channel ID.
    pub async fn get_permission_by_role_id_channel_id(
        &self,
        role_id: &str,
        channel_id: &str,
    ) -> Result<api::PermissionRoleChannelListEventResponse> {
        let cid = self.generate_cid();
        let body = api::PermissionRoleChannelListEventRequest {
            role_id: role_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetPermissionByRoleIdChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionRoleChannelListEventResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List channel setting.
    pub async fn list_channel_setting(
        &self,
        clan_id: &str,
    ) -> Result<api::ChannelSettingListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelSettingListRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelSettingListResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List apps.
    pub async fn list_apps(&self, filter: &str) -> Result<api::AppList> {
        let cid = self.generate_cid();
        let body = api::ListAppsRequest {
            filter: filter.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListApps", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AppList::decode(response.as_slice())?)
    }

    /// Get app by ID.
    pub async fn get_app(&self, id: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::App {
            id: id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// List for sale items.
    pub async fn list_for_sale_items(&self, page: i32) -> Result<api::ForSaleItemList> {
        let cid = self.generate_cid();
        let body = api::ListForSaleItemsRequest { page }.encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListForSaleItems", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ForSaleItemList::decode(response.as_slice())?)
    }

    /// List clan webhook.
    pub async fn list_clan_webhook(&self, clan_id: &str) -> Result<api::ListClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::ListClanWebhookRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListClanWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListClanWebhookResponse::decode(response.as_slice())?)
    }

    /// List Sd Topics.
    pub async fn list_sd_topic(&self, clan_id: &str, limit: i32) -> Result<api::SdTopicList> {
        let cid = self.generate_cid();
        let body = api::ListSdTopicRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopicList::decode(response.as_slice())?)
    }

    /// Get topic detail.
    pub async fn get_topic_detail(&self, topic_id: &str) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicDetailRequest {
            topic_id: topic_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetTopicDetail", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// List channel attachment.
    pub async fn list_channel_attachment(
        &self,
        channel_id: &str,
        clan_id: &str,
        limit: i32,
    ) -> Result<api::ChannelAttachmentList> {
        let cid = self.generate_cid();
        let body = api::ListChannelAttachmentRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelAttachment", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelAttachmentList::decode(response.as_slice())?)
    }

    /// List voice channel users.
    pub async fn list_channel_voice_users(
        &self,
        clan_id: &str,
    ) -> Result<api::VoiceChannelUserList> {
        let cid = self.generate_cid();
        let body = api::ListChannelUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListChannelVoiceUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VoiceChannelUserList::decode(response.as_slice())?)
    }

    /// List archived channel descriptions.
    pub async fn list_archived_channel_descs(
        &self,
        clan_id: &str,
    ) -> Result<api::ListArchivedChannelDescsResponse> {
        let cid = self.generate_cid();
        let body = api::ListArchivedChannelDescsRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListArchivedChannelDescs", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListArchivedChannelDescsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List user clans by user ID.
    pub async fn list_user_clans_by_user_id(&self) -> Result<api::AllUserClans> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListUserClansByUserId", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::AllUserClans::decode(response.as_slice())?)
    }

    /// Check if user is banned.
    pub async fn is_banned(&self, channel_id: &str) -> Result<api::IsBannedResponse> {
        let cid = self.generate_cid();
        let body = api::IsBannedRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsBanned", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsBannedResponse::decode(response.as_slice())?)
    }

    /// List banned users.
    pub async fn list_banned_users(
        &self,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::BannedUserList> {
        let cid = self.generate_cid();
        let body = api::BannedUserListRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListBannedUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::BannedUserList::decode(response.as_slice())?)
    }

    /// Get channel canvas list.
    pub async fn get_channel_canvas_list(
        &self,
        channel_id: &str,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<api::ChannelCanvasListResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasListRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasListResponse::decode(response.as_slice())?)
    }

    /// Get channel canvas detail.
    pub async fn get_channel_canvas_detail(
        &self,
        id: &str,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::ChannelCanvasDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelCanvasDetailRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCanvasDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelCanvasDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List onboarding.
    pub async fn list_onboarding(
        &self,
        clan_id: &str,
        limit: i32,
        page: i32,
    ) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            limit,
            page,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Get onboarding detail.
    pub async fn get_onboarding_detail(
        &self,
        id: &str,
        clan_id: &str,
    ) -> Result<api::OnboardingItem> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetOnboardingDetail", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::OnboardingItem::decode(response.as_slice())?)
    }

    /// List onboarding steps.
    pub async fn list_onboarding_step(
        &self,
        clan_id: &str,
    ) -> Result<api::ListOnboardingStepResponse> {
        let cid = self.generate_cid();
        let body = api::ListOnboardingStepRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingStepResponse::decode(
            response.as_slice(),
        )?)
    }

    /// List role users.
    pub async fn list_role_users(&self, role_id: &str) -> Result<api::RoleUserList> {
        let cid = self.generate_cid();
        let body = api::ListRoleUsersRequest {
            role_id: role_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListRoleUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RoleUserList::decode(response.as_slice())?)
    }

    /// List role permissions.
    pub async fn list_role_permissions(&self, role_id: &str) -> Result<api::PermissionList> {
        let cid = self.generate_cid();
        let body = api::ListPermissionsRequest {
            role_id: role_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "ListRolePermissions", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::PermissionList::decode(response.as_slice())?)
    }

    /// Check if user is a follower.
    pub async fn is_follower(&self, follow_id: &str) -> Result<api::IsFollowerResponse> {
        let cid = self.generate_cid();
        let body = api::IsFollowerRequest {
            follow_id: follow_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "IsFollower", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::IsFollowerResponse::decode(response.as_slice())?)
    }

    /// Get channel encryption method.
    pub async fn get_chan_encryption_method(
        &self,
        channel_id: &str,
    ) -> Result<api::ChanEncryptionMethod> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChanEncryptionMethod::decode(response.as_slice())?)
    }

    /// Get key server.
    pub async fn get_key_server(&self) -> Result<api::GetKeyServerResp> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "GetKeyServer", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetKeyServerResp::decode(response.as_slice())?)
    }

    /// Get pub keys.
    pub async fn get_pub_keys(&self, user_ids: &[&str]) -> Result<api::GetPubKeysResponse> {
        let cid = self.generate_cid();
        let body = api::GetPubKeysRequest {
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPubKeys", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPubKeysResponse::decode(response.as_slice())?)
    }

    /// List audit log.
    pub async fn list_audit_log(&self, clan_id: &str) -> Result<api::ListAuditLog> {
        let cid = self.generate_cid();
        let body = api::ListAuditLogRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "ListAuditLog", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListAuditLog::decode(response.as_slice())?)
    }

    /// Search message.
    pub async fn search_message(
        &self,
        _query: &str,
        from: i32,
        size: i32,
    ) -> Result<api::SearchMessageResponse> {
        let cid = self.generate_cid();
        let body = api::SearchMessageRequest {
            from,
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SearchMessageResponse::decode(response.as_slice())?)
    }

    /// Search thread.
    pub async fn search_thread(&self, clan_id: &str, label: &str) -> Result<api::ChannelDescList> {
        let cid = self.generate_cid();
        let body = api::SearchThreadRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            label: label.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "SearchThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelDescList::decode(response.as_slice())?)
    }

    /// List Mezon OAuth client.
    pub async fn list_mezon_oauth_client(&self) -> Result<api::MezonOauthClientList> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "ListMezonOauthClient", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClientList::decode(response.as_slice())?)
    }

    /// Get Mezon OAuth client.
    pub async fn get_mezon_oauth_client(&self, client_id: &str) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::GetMezonOauthClientRequest {
            client_id: client_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Generate hash channel apps.
    pub async fn generate_hash_channel_apps(
        &self,
        app_id: &str,
    ) -> Result<api::GenerateHashChannelAppsResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateHashChannelAppsRequest {
            app_id: app_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateHashChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateHashChannelAppsResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Get notification category settings list.
    pub async fn get_channel_category_noti_settings_list(
        &self,
        clan_id: &str,
    ) -> Result<api::NotificationChannelCategorySettingList> {
        let cid = self.generate_cid();
        let body = api::NotificationClan {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GetChannelCategoryNotiSettingsList", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::NotificationChannelCategorySettingList::decode(
            response.as_slice(),
        )?)
    }

    /// List muted channels.
    pub async fn list_muted_channels(&self, clan_id: &str) -> Result<Vec<String>> {
        let cid = self.generate_cid();

        let body = api::ListMutedChannelRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();

        let (code, response) = self.send_api_request(cid, "ListMutedChannel", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let muted = api::MutedChannelList::decode(response.as_slice())?;
        Ok(muted
            .muted_list
            .into_iter()
            .map(|channel_id| channel_id.to_string())
            .collect())
    }
}

// ============================================================================
// API Methods - Cold Path (infrequent operations)
// ============================================================================

impl MezonTransport {
    /// Create a new channel.
    pub async fn create_channel(
        &self,
        _clan_id: &str,
        channel_label: &str,
        channel_type: u32,
    ) -> Result<ApiChannelDesc> {
        let cid = self.generate_cid();

        let body = api::CreateChannelDescRequest {
            clan_id: _clan_id.parse().unwrap_or_default(),
            channel_label: channel_label.to_string(),
            r#type: channel_type as i32,
            ..Default::default()
        }
        .encode_to_vec();

        let (code, response) = self
            .send_api_request(cid, "CreateChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        let channel = api::ChannelDescription::decode(response.as_slice())?;
        Ok(Self::channel_desc_from_proto(channel))
    }

    /// Delete a channel.
    pub async fn delete_channel(&self, _channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteChannelDescRequest {
            channel_id: _channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self
            .send_api_request(cid, "DeleteChannelDesc", body)
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Add a friend.
    pub async fn add_friend(&self, _user_id: &str) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::AddFriendsRequest {
            ids: vec![_user_id.parse().unwrap_or_default()],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "AddFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Delete a friend.
    pub async fn delete_friend(&self, _user_id: &str) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::DeleteFriendsRequest {
            ids: vec![_user_id.parse().unwrap_or_default()],
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "DeleteFriends", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Update channel description.
    pub async fn update_channel_desc(
        &self,
        clan_id: &str,
        channel_id: &str,
        label: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelDescRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            channel_label: Some(label.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel private.
    pub async fn update_channel_private(
        &self,
        clan_id: &str,
        channel_id: &str,
        channel_private: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelPrivateRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            channel_private,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelPrivate", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Change channel category.
    pub async fn change_channel_category(
        &self,
        clan_id: &str,
        channel_id: &str,
        new_category_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChangeChannelCategoryRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            new_category_id: new_category_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ChangeChannelCategory", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel users.
    pub async fn add_channel_users(&self, channel_id: &str, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddChannelUsersRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddChannelUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel users.
    pub async fn remove_channel_users(&self, channel_id: &str, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveChannelUsersRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelUsers", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Leave thread.
    pub async fn leave_thread(&self, clan_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::LeaveThreadRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "LeaveThread", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Archive channel.
    pub async fn archive_channel(&self, clan_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ArchiveChannelRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ArchiveChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Reactivate archived thread.
    pub async fn active_archived_thread(&self, clan_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ActiveArchivedThread {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ActiveArchivedThread", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Close DM.
    pub async fn close_dm_by_channel_id(&self, clan_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CloseDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Open DM.
    pub async fn open_dm_by_channel_id(&self, clan_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelDescRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "OpenDMByChannelId", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan.
    pub async fn create_clan_desc(
        &self,
        clan_name: &str,
        logo: &str,
        banner: &str,
    ) -> Result<api::ClanDesc> {
        let cid = self.generate_cid();
        let body = api::CreateClanDescRequest {
            clan_name: clan_name.to_string(),
            logo: logo.to_string(),
            banner: banner.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ClanDesc::decode(response.as_slice())?)
    }

    /// Update clan.
    pub async fn update_clan_desc(&self, clan_id: &str, clan_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanDescRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            clan_name: clan_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan.
    pub async fn delete_clan_desc(&self, clan_desc_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteClanDescRequest {
            clan_desc_id: clan_desc_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteClanDesc", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove clan users.
    pub async fn remove_clan_users(&self, clan_id: &str, user_ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveClanUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "RemoveClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Ban clan users.
    pub async fn ban_clan_users(
        &self,
        clan_id: &str,
        channel_id: &str,
        user_ids: &[&str],
        ban_time: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
            ban_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unban clan users.
    pub async fn unban_clan_users(
        &self,
        clan_id: &str,
        channel_id: &str,
        user_ids: &[&str],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BanClanUsersRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            user_ids: user_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnbanClanUsers", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create category.
    pub async fn create_category_desc(
        &self,
        category_name: &str,
        clan_id: &str,
    ) -> Result<api::CategoryDesc> {
        let cid = self.generate_cid();
        let body = api::CreateCategoryDescRequest {
            category_name: category_name.to_string(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CategoryDesc::decode(response.as_slice())?)
    }

    /// Delete category.
    pub async fn delete_category_desc(&self, category_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteCategoryDescRequest {
            category_id: category_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteCategoryDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category.
    pub async fn update_category(
        &self,
        category_id: &str,
        category_name: &str,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryDescRequest {
            category_id: category_id.parse().unwrap_or_default(),
            category_name: category_name.to_string(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Block friends.
    pub async fn block_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids.iter().map(|s| s.parse().unwrap_or_default()).collect(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "BlockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Unblock friends.
    pub async fn unblock_friends(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::BlockFriendsRequest {
            ids: ids.iter().map(|s| s.parse().unwrap_or_default()).collect(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UnblockFriends", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update username.
    pub async fn update_username(&self, username: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsernameRequest {
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUsername", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user profile.
    pub async fn update_user(&self, display_name: &str, avatar_url: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateUsersRequest {
            display_name: display_name.to_string(),
            avatar_url: avatar_url.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUser", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user profile by clan.
    pub async fn update_user_profile_by_clan(&self, clan_id: &str, nick_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanProfileRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            nick_name: Some(nick_name.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateUserProfileByClan", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Mark as read.
    pub async fn mark_as_read(
        &self,
        channel_id: &str,
        category_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MarkAsReadRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            category_id: category_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "MarkAsRead", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user status.
    pub async fn update_user_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateUserStatus", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update user custom status.
    pub async fn update_user_custom_status(&self, status: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserStatusUpdate {
            status: status.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateUserCustomStatus", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Session logout.
    pub async fn session_logout(&self, token: &str, refresh_token: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SessionLogoutRequest {
            token: token.to_string(),
            refresh_token: refresh_token.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SessionLogout", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification channel setting.
    pub async fn set_notification_channel_setting(
        &self,
        channel_category_id: &str,
        notification_type: i32,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id: channel_category_id.parse().unwrap_or_default(),
            notification_type,
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationChannelSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification clan setting.
    pub async fn set_notification_clan_setting(
        &self,
        clan_id: &str,
        notification_type: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetDefaultNotificationRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            notification_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationClanSetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set notification category setting.
    pub async fn set_notification_category_setting(
        &self,
        channel_category_id: &str,
        notification_type: i32,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetNotificationRequest {
            channel_category_id: channel_category_id.parse().unwrap_or_default(),
            notification_type,
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute channel.
    pub async fn set_mute_channel(&self, id: &str, mute_time: i32, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id: id.parse().unwrap_or_default(),
            mute_time,
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteChannel", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set mute category.
    pub async fn set_mute_category(&self, id: &str, mute_time: i32, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SetMuteRequest {
            id: id.parse().unwrap_or_default(),
            mute_time,
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "SetMuteCategory", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notifications.
    pub async fn delete_notifications(&self, ids: &[&str]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteNotificationsRequest {
            ids: ids.iter().map(|s| s.parse().unwrap_or_default()).collect(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotifications", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification category setting.
    pub async fn delete_notification_category_setting(&self, category_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DefaultNotificationCategory {
            category_id: category_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationCategorySetting", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete notification channel.
    pub async fn delete_notification_channel(&self, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::NotificationChannel {
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteNotificationChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set role channel permission.
    pub async fn set_role_channel_permission(&self, role_id: &str, channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleChannelRequest {
            role_id: role_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetRoleChannelPermission", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Check duplicate name.
    pub async fn check_duplicate_name(
        &self,
        name: &str,
        r#type: i32,
    ) -> Result<api::CheckDuplicateNameResponse> {
        let cid = self.generate_cid();
        let body = api::CheckDuplicateNameRequest {
            name: name.to_string(),
            r#type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CheckDuplicateName", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CheckDuplicateNameResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Upload attachment file.
    pub async fn upload_attachment_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UploadAttachmentFile", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Upload OAuth file.
    pub async fn upload_oauth_file(
        &self,
        filename: &str,
        filetype: &str,
        size: i32,
    ) -> Result<api::UploadAttachment> {
        let cid = self.generate_cid();
        let body = api::UploadAttachmentRequest {
            filename: filename.to_string(),
            filetype: filetype.to_string(),
            size,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UploadOauthFile", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UploadAttachment::decode(response.as_slice())?)
    }

    /// Push pub key.
    pub async fn push_pub_key(&self, encr: &[u8], sign: &[u8]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::PushPubKeyRequest {
            pk: Some(api::PubKey {
                encr: encr.to_vec(),
                sign: sign.to_vec(),
            }),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "PushPubKey", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Set channel encryption method.
    pub async fn set_chan_encryption_method(&self, channel_id: &str, method: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ChanEncryptionMethod {
            channel_id: channel_id.parse().unwrap_or_default(),
            method: method.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "SetChanEncryptionMethod", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Transfer ownership.
    pub async fn transfer_ownership(&self, clan_id: &str, new_owner_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::TransferOwnershipRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            new_owner_id: new_owner_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "TransferOwnership", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Report message abuse.
    pub async fn report_message_abuse(&self, message_id: &str, abuse_type: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ReportMessageAbuseReqest {
            message_id: message_id.parse().unwrap_or_default(),
            abuse_type: abuse_type.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReportMessageAbuse", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Message button click.
    pub async fn message_button_click(
        &self,
        message_id: &str,
        channel_id: &str,
        button_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::MessageButtonClicked {
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            button_id: button_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MessageButtonClick", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Dropdown box selected.
    pub async fn dropdown_box_selected(
        &self,
        message_id: &str,
        channel_id: &str,
        selectbox_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::DropdownBoxSelected {
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            selectbox_id: selectbox_id.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DropdownBoxSelected", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add agent to channel.
    pub async fn add_agent_to_channel(&self, channel_id: &str, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddAgentToChannel", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Disconnect agent.
    pub async fn disconnect_agent(&self, channel_id: &str, room_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateAiAgentRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DisconnectAgent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Regist FCM device token.
    pub async fn regist_fcm_device_token(
        &self,
        token: &str,
        device_id: &str,
        platform: &str,
    ) -> Result<api::RegistFcmDeviceTokenResponse> {
        let cid = self.generate_cid();
        let body = api::RegistFcmDeviceTokenRequest {
            token: token.to_string(),
            device_id: device_id.to_string(),
            platform: platform.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "RegistFCMDeviceToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::RegistFcmDeviceTokenResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Create link invite user.
    pub async fn create_link_invite_user(
        &self,
        clan_id: &str,
        channel_id: &str,
        expiry_time: i32,
    ) -> Result<api::LinkInviteUser> {
        let cid = self.generate_cid();
        let body = api::LinkInviteUserRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            expiry_time,
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateLinkInviteUser", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::LinkInviteUser::decode(response.as_slice())?)
    }

    /// Invite user.
    pub async fn invite_user(&self, invite_id: &str) -> Result<api::InviteUserRes> {
        let cid = self.generate_cid();
        let body = api::InviteUserRequest {
            invite_id: invite_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "InviteUser", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::InviteUserRes::decode(response.as_slice())?)
    }

    /// Create activity.
    pub async fn create_activiy(
        &self,
        activity_name: &str,
        activity_type: i32,
    ) -> Result<api::UserActivity> {
        let cid = self.generate_cid();
        let body = api::CreateActivityRequest {
            activity_name: activity_name.to_string(),
            activity_type,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateActiviy", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UserActivity::decode(response.as_slice())?)
    }

    /// Create message to inbox.
    pub async fn create_message_2_inbox(
        &self,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
        content: &str,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::Message2InboxRequest {
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateMessage2Inbox", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Create pin message.
    pub async fn create_pin_message(
        &self,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<api::ChannelMessageHeader> {
        let cid = self.generate_cid();
        let body = api::PinMessageRequest {
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelMessageHeader::decode(response.as_slice())?)
    }

    /// Delete pin message.
    pub async fn delete_pin_message(
        &self,
        id: &str,
        message_id: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeletePinMessage {
            id: id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeletePinMessage", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan order.
    pub async fn update_clan_order(&self, clans_order: &[(i32, &str)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanOrderRequest {
            clans_order: clans_order
                .iter()
                .map(
                    |(order, clan_id)| api::update_clan_order_request::ClanOrder {
                        order: *order,
                        clan_id: clan_id.parse().unwrap_or_default(),
                    },
                )
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateClanOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update category order.
    pub async fn update_category_order(
        &self,
        clan_id: &str,
        categories: &[(i32, &str)],
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateCategoryOrderRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            categories: categories
                .iter()
                .map(|(order, category_id)| api::CategoryOrderUpdate {
                    order: *order,
                    category_id: category_id.parse().unwrap_or_default(),
                })
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateCategoryOrder", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role order.
    pub async fn update_role_order(&self, clan_id: &str, roles: &[(i32, &str)]) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleOrderRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            roles: roles
                .iter()
                .map(|(order, role_id)| api::RoleOrderUpdate {
                    order: *order,
                    role_id: role_id.parse().unwrap_or_default(),
                })
                .collect(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRoleOrder", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create event.
    pub async fn create_event(
        &self,
        title: &str,
        clan_id: &str,
        channel_id: &str,
        start_time: u32,
        end_time: u32,
    ) -> Result<api::EventManagement> {
        let cid = self.generate_cid();
        let body = api::CreateEventRequest {
            title: title.to_string(),
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            start_time_seconds: start_time,
            end_time_seconds: end_time,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EventManagement::decode(response.as_slice())?)
    }

    /// Delete event.
    pub async fn delete_event(&self, event_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteEventRequest {
            event_id: event_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update event.
    pub async fn update_event(&self, event_id: &str, clan_id: &str, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateEventRequest {
            event_id: event_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add user event.
    pub async fn add_user_event(&self, clan_id: &str, event_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            event_id: event_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete user event.
    pub async fn delete_user_event(&self, clan_id: &str, event_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UserEventRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            event_id: event_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteUserEvent", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create role.
    pub async fn create_role(&self, title: &str, clan_id: &str) -> Result<api::Role> {
        let cid = self.generate_cid();
        let body = api::CreateRoleRequest {
            title: title.to_string(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::Role::decode(response.as_slice())?)
    }

    /// Delete role.
    pub async fn delete_role(&self, role_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id: role_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update role.
    pub async fn update_role(&self, role_id: &str, title: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateRoleRequest {
            role_id: role_id.parse().unwrap_or_default(),
            title: Some(title.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateRole", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete role channel desc.
    pub async fn delete_role_channel_desc(&self, role_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteRoleRequest {
            role_id: role_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteRoleChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add roles channel desc.
    pub async fn add_roles_channel_desc(&self, role_ids: &[&str], channel_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddRoleChannelDescRequest {
            role_ids: role_ids
                .iter()
                .map(|s| s.parse().unwrap_or_default())
                .collect(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddRolesChannelDesc", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create clan emoji.
    pub async fn create_clan_emoji(
        &self,
        clan_id: &str,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiCreateRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "CreateClanEmoji", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan emoji by ID.
    pub async fn delete_clan_emoji_by_id(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiDeleteRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteByIdClanEmoji", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan emoji by ID.
    pub async fn update_clan_emoji_by_id(
        &self,
        id: &str,
        shortname: &str,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanEmojiUpdateRequest {
            id: id.parse().unwrap_or_default(),
            shortname: shortname.to_string(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanEmojiById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add clan sticker.
    pub async fn add_clan_sticker(
        &self,
        clan_id: &str,
        source: &str,
        shortname: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerAddRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            source: source.to_string(),
            shortname: shortname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddClanSticker", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update clan sticker by ID.
    pub async fn update_clan_sticker_by_id(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerUpdateByIdRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan sticker by ID.
    pub async fn delete_clan_sticker_by_id(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanStickerDeleteRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanStickerById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate webhook.
    pub async fn generate_webhook(
        &self,
        webhook_name: &str,
        channel_id: &str,
        clan_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookCreateRequest {
            webhook_name: webhook_name.to_string(),
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "GenerateWebhook", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update webhook by ID.
    pub async fn update_webhook_by_id(&self, id: &str, webhook_name: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookUpdateRequestById {
            id: id.parse().unwrap_or_default(),
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete webhook by ID.
    pub async fn delete_webhook_by_id(&self, id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::WebhookDeleteRequestById {
            id: id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Generate clan webhook.
    pub async fn generate_clan_webhook(
        &self,
        clan_id: &str,
        webhook_name: &str,
    ) -> Result<api::GenerateClanWebhookResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateClanWebhookRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateClanWebhook", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateClanWebhookResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update clan webhook by ID.
    pub async fn update_clan_webhook_by_id(
        &self,
        id: &str,
        clan_id: &str,
        webhook_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateClanWebhookRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            webhook_name: webhook_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete clan webhook by ID.
    pub async fn delete_clan_webhook_by_id(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClanWebhookRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteClanWebhookById", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add app.
    pub async fn add_app(&self, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::AddAppRequest {
            appname: appname.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "AddApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Delete app.
    pub async fn delete_app(&self, id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::App {
            id: id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update app.
    pub async fn update_app(&self, id: &str, appname: &str) -> Result<api::App> {
        let cid = self.generate_cid();
        let body = api::UpdateAppRequest {
            id: id.parse().unwrap_or_default(),
            appname: Some(appname.to_string()),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "UpdateApp", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::App::decode(response.as_slice())?)
    }

    /// Add app to clan.
    pub async fn add_app_to_clan(&self, app_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AppClan {
            app_id: app_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "AddAppToClan", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create system message.
    pub async fn create_system_message(&self, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SystemMessageRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "CreateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update system message.
    pub async fn update_system_message(&self, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::SystemMessageRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete system message.
    pub async fn delete_system_message(&self, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteSystemMessage {
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteSystemMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Edit channel canvases.
    pub async fn edit_channel_canvases(
        &self,
        channel_id: &str,
        clan_id: &str,
        title: &str,
        content: &str,
    ) -> Result<api::EditChannelCanvasResponse> {
        let cid = self.generate_cid();
        let body = api::EditChannelCanvasRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            title: title.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "EditChannelCanvases", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::EditChannelCanvasResponse::decode(response.as_slice())?)
    }

    /// Delete channel canvas.
    pub async fn delete_channel_canvas(
        &self,
        canvas_id: &str,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::DeleteChannelCanvasRequest {
            canvas_id: canvas_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelCanvas", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Add channel favorite.
    pub async fn add_channel_favorite(&self, channel_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::AddFavoriteChannelRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Remove channel favorite.
    pub async fn remove_channel_favorite(&self, channel_id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::RemoveFavoriteChannelRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveChannelFavorite", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create onboarding.
    pub async fn create_onboarding(&self, clan_id: &str) -> Result<api::ListOnboardingResponse> {
        let cid = self.generate_cid();
        let body = api::CreateOnboardingRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ListOnboardingResponse::decode(response.as_slice())?)
    }

    /// Update onboarding.
    pub async fn update_onboarding(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "UpdateOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete onboarding.
    pub async fn delete_onboarding(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::OnboardingRequest {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "DeleteOnboarding", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update onboarding step.
    pub async fn update_onboarding_step(&self, clan_id: &str, onboarding_step: i32) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::UpdateOnboardingStepRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            onboarding_step,
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateOnboardingStep", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create Sd topic.
    pub async fn create_sd_topic(
        &self,
        message_id: &str,
        clan_id: &str,
        channel_id: &str,
    ) -> Result<api::SdTopic> {
        let cid = self.generate_cid();
        let body = api::SdTopicRequest {
            message_id: message_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreateSdTopic", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::SdTopic::decode(response.as_slice())?)
    }

    /// Create external Mezon meet.
    pub async fn create_external_mezon_meet(&self) -> Result<api::GenerateMezonMeetResponse> {
        let cid = self.generate_cid();
        let (code, response) = self
            .send_api_request(cid, "CreateExternalMezonMeet", Vec::new())
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMezonMeetResponse::decode(response.as_slice())?)
    }

    /// Generate meet token.
    pub async fn generate_meet_token(
        &self,
        channel_id: &str,
        room_name: &str,
    ) -> Result<api::GenerateMeetTokenResponse> {
        let cid = self.generate_cid();
        let body = api::GenerateMeetTokenRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "GenerateMeetToken", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GenerateMeetTokenResponse::decode(response.as_slice())?)
    }

    /// Remove participant Mezon meet.
    pub async fn remove_participant_mezon_meet(
        &self,
        channel_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "RemoveParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Mute participant Mezon meet.
    pub async fn mute_participant_mezon_meet(
        &self,
        channel_id: &str,
        room_name: &str,
        username: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MeetParticipantRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
            username: username.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "MuteParticipantMezonMeet", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create room channel apps.
    pub async fn create_room_channel_apps(
        &self,
        channel_id: &str,
        room_name: &str,
    ) -> Result<api::CreateRoomChannelApps> {
        let cid = self.generate_cid();
        let body = api::CreateRoomChannelApps {
            channel_id: channel_id.parse().unwrap_or_default(),
            room_name: room_name.to_string(),
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateRoomChannelApps", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateRoomChannelApps::decode(response.as_slice())?)
    }

    /// Update Mezon OAuth client.
    pub async fn update_mezon_oauth_client(
        &self,
        client_id: &str,
        client_name: &str,
    ) -> Result<api::MezonOauthClient> {
        let cid = self.generate_cid();
        let body = api::MezonOauthClient {
            client_id: client_id.to_string(),
            client_name: client_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateMezonOauthClient", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::MezonOauthClient::decode(response.as_slice())?)
    }

    /// Add quick menu access.
    pub async fn add_quick_menu_access(
        &self,
        bot_id: &str,
        clan_id: &str,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id: bot_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "AddQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update quick menu access.
    pub async fn update_quick_menu_access(
        &self,
        bot_id: &str,
        clan_id: &str,
        menu_name: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            bot_id: bot_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            menu_name: menu_name.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete quick menu access.
    pub async fn delete_quick_menu_access(&self, id: &str, clan_id: &str) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::QuickMenuAccess {
            id: id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteQuickMenuAccess", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Update channel message.
    pub async fn update_channel_message(
        &self,
        clan_id: &str,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ChannelMessageUpdate {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            content: content.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "UpdateChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Delete channel message.
    pub async fn delete_channel_message(
        &self,
        clan_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = realtime::ChannelMessageRemove {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "DeleteChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// React channel message.
    pub async fn react_channel_message(
        &self,
        clan_id: &str,
        channel_id: &str,
        message_id: &str,
        emoji_id: &str,
        emoji: &str,
        count: i32,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::MessageReaction {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            emoji_id: emoji_id.parse().unwrap_or_default(),
            emoji: emoji.to_string(),
            count,
            ..Default::default()
        }
        .encode_to_vec();
        let (code, _) = self
            .send_api_request(cid, "ReactChannelMessage", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Create poll.
    pub async fn create_poll(
        &self,
        channel_id: &str,
        clan_id: &str,
        question: &str,
    ) -> Result<api::CreatePollResponse> {
        let cid = self.generate_cid();
        let body = api::CreatePollRequest {
            channel_id: channel_id.parse().unwrap_or_default(),
            clan_id: clan_id.parse().unwrap_or_default(),
            question: question.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "CreatePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreatePollResponse::decode(response.as_slice())?)
    }

    /// Vote poll.
    pub async fn vote_poll(
        &self,
        poll_id: &str,
        message_id: &str,
        channel_id: &str,
    ) -> Result<api::VotePollResponse> {
        let cid = self.generate_cid();
        let body = api::VotePollRequest {
            poll_id: poll_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "VotePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::VotePollResponse::decode(response.as_slice())?)
    }

    /// Close poll.
    pub async fn close_poll(
        &self,
        poll_id: &str,
        message_id: &str,
        channel_id: &str,
    ) -> Result<()> {
        let cid = self.generate_cid();
        let body = api::ClosePollRequest {
            poll_id: poll_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, _) = self.send_api_request(cid, "ClosePoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(())
    }

    /// Get poll.
    pub async fn get_poll(
        &self,
        poll_id: &str,
        message_id: &str,
        channel_id: &str,
    ) -> Result<api::GetPollResponse> {
        let cid = self.generate_cid();
        let body = api::GetPollRequest {
            poll_id: poll_id.parse().unwrap_or_default(),
            message_id: message_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
        }
        .encode_to_vec();
        let (code, response) = self.send_api_request(cid, "GetPoll", body).await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::GetPollResponse::decode(response.as_slice())?)
    }

    /// Create channel timeline.
    pub async fn create_channel_timeline(
        &self,
        clan_id: &str,
        channel_id: &str,
        title: &str,
    ) -> Result<api::CreateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::CreateChannelTimelineRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "CreateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::CreateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update channel timeline.
    pub async fn update_channel_timeline(
        &self,
        clan_id: &str,
        channel_id: &str,
        id: &str,
        title: &str,
    ) -> Result<api::UpdateChannelTimelineResponse> {
        let cid = self.generate_cid();
        let body = api::UpdateChannelTimelineRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            id: id.parse().unwrap_or_default(),
            title: title.to_string(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "UpdateChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::UpdateChannelTimelineResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Detail channel timeline.
    pub async fn detail_channel_timeline(
        &self,
        clan_id: &str,
        channel_id: &str,
        id: &str,
    ) -> Result<api::ChannelTimelineDetailResponse> {
        let cid = self.generate_cid();
        let body = api::ChannelTimelineDetailRequest {
            clan_id: clan_id.parse().unwrap_or_default(),
            channel_id: channel_id.parse().unwrap_or_default(),
            id: id.parse().unwrap_or_default(),
            ..Default::default()
        }
        .encode_to_vec();
        let (code, response) = self
            .send_api_request(cid, "DetailChannelTimeline", body)
            .await?;
        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }
        Ok(api::ChannelTimelineDetailResponse::decode(
            response.as_slice(),
        )?)
    }

    /// Update user account.
    pub async fn update_account(&self, _username: Option<&str>) -> Result<()> {
        let cid = self.generate_cid();

        let body = api::UpdateAccountRequest {
            display_name: _username.map(str::to_string),
            ..Default::default()
        }
        .encode_to_vec();

        let (code, _response) = self.send_api_request(cid, "UpdateAccount", body).await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }

    /// Delete user account.
    pub async fn delete_account(&self) -> Result<()> {
        let cid = self.generate_cid();

        let (code, _response) = self
            .send_api_request(cid, "DeleteAccount", Vec::new())
            .await?;

        if code != 0 {
            return Err(anyhow::anyhow!("API error: code={}", code));
        }

        Ok(())
    }
}
