pub mod account;
pub mod activity;
pub mod album_layout;
pub mod audio;
pub mod audit_log;
pub mod auto_update;
pub mod badge;
pub mod cache;
pub mod canvas;
pub mod channel;
pub mod channel_media;
pub mod channel_members;
pub mod channel_permissions;
pub mod channel_role_permissions;
pub mod channel_settings;
pub mod channel_users;
pub mod clan;
pub mod clan_load;
pub mod clan_members;
pub mod compose;
pub mod config;
pub mod connection;
pub mod direct;
pub mod emoji;
pub mod events;
pub mod files;
pub mod friend;
pub mod gallery;
pub mod gif;
pub mod group_members;
pub mod ids;
pub mod inbox;
pub mod login;
pub mod message;
pub mod message_search;
pub mod message_time;
pub mod messages;
pub mod notification_push;
pub mod notification_setting;
pub mod ogp;
pub mod permissions;
pub mod pinned;
pub mod platform;
pub mod presence;
pub mod presign;
pub mod quick_menu;
pub mod realtime;
pub mod roles;
pub mod sticker;
pub mod stream;
pub mod threads;
pub mod topic_badges;
pub mod topics;
pub mod ui_state;
pub mod user_profile;
pub mod users_by_user;
pub mod voice;
pub mod wallet;
mod wallet_persist;
pub mod webhook;
pub mod winstore_update;

use anyhow::{Context, Result};
use dirs::config_dir;
pub use mezon_client::Session;
pub use mezon_client::transport::{MENTION_HERE_ID, MENTION_HERE_USER_ID, is_here_user_id};
pub use mezon_client::{
    clean_download_url, download_url_to_downloads, resolve_download_filename, sanitize_filename,
    write_bytes_to_downloads,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs;

pub use account::*;
pub use activity::{ActivityEvent, ActivityStore, UserActivity};
pub use album_layout::{AlbumLayout, AlbumTile, calculate_album_layout};
pub use audio::{
    AudioDeviceInfo, AudioStore, MicCaptureFactory, MicCaptureHandle, MicPcmCaptureFactory,
    MicPcmFormat,
};
pub use audit_log::{
    ALL_ACTION_INDEX, AUDIT_ACTION_OPTIONS, AuditActionOption, AuditLogEntry, AuditLogQuery,
    AuditLogState, AuditLogStateView, AuditLogStore, audit_action_api_value, audit_action_i18n_key,
    audit_action_index_for_api_log,
};
pub use auto_update::{AutoUpdateStatus, AutoUpdateStore};
pub use badge::BadgeService;
pub use cache::{Freshness, KeyedCache};
pub use canvas::{CanvasDetail, CanvasStore, CanvasSummary, UploadedCanvasImage, canvas_web_link};
pub use channel::*;
pub use channel_media::{
    CHANNEL_MEDIA_CACHE_TTL, CHANNEL_MEDIA_PAGE_SIZE, ChannelMediaEvent, ChannelMediaStore,
    ChannelTimeline, ChannelTimelineAttachment, CreateTimelineInput, TimelineCardSide,
    UpdateTimelineInput, first_event_year, timeline_card_position,
};
pub use channel_members::{ChannelMember, ChannelMembersEvent, ChannelMembersStore};
pub use channel_permissions::{
    ChannelPermissionsEvent, ChannelPermissionsStore, OVERRIDDEN_SLUGS, PERMISSION_DELETE_MESSAGE,
    PERMISSION_MANAGE_THREAD, PERMISSION_SEND_MESSAGE, is_overridden_slug,
};
pub use channel_role_permissions::{
    ChannelRolePermissionsEvent, ChannelRolePermissionsStore, OVERRIDE_TYPE_ALLOW,
    OVERRIDE_TYPE_DENY, OVERRIDE_TYPE_NEUTRAL, PermissionEntity,
};
pub use channel_settings::{ChannelSetting, ChannelSettingsEvent, ChannelSettingsStore};
pub use channel_users::{ChannelUsersEvent, ChannelUsersStore};
pub use clan::*;
pub use clan_load::ClanLoadScheduler;
pub use clan_members::{
    ClanMember, ClanMembersEvent, ClanMembersStore, User, split_members_by_status,
};
pub use compose::{ComposeDraft, ComposeStore, ComposeToken, ComposeTokenKind, PendingAttachment};
pub use config::AppConfig;
pub use connection::{ConnectionStore, resolve_initial_auth_state};
pub use direct::{
    DirectChannel, DirectEvent, DirectKind, DirectMessageBody, DirectMessageStore,
    dm_counts_toward_unread_badge,
};
pub use emoji::{
    EMOJI_UPLOAD_MAX_PX, EMOTICON_ALLOWED_EXTENSIONS, EMOTICON_SHORTNAME_MAX,
    EMOTICON_SHORTNAME_MIN, Emoji, EmojiEvent, EmojiStore, MAX_EMOJI_BYTES, MAX_STICKER_BYTES,
    STICKER_UPLOAD_MAX_PX, generate_snowflake_id, is_valid_emoticon_shortname,
    normalize_emoji_shortname, strip_emoji_colons, validate_emoji_create_shortname,
    validate_emoticon_file,
};
pub use events::{ClanEventItem, CreateEventDraft, EventsEvent, EventsStore};
pub use files::{
    ChannelDocument, FILES_BROAD_QUERY, FILES_CACHE_TTL, FILES_PAGE_SIZE, FILES_TYPED_QUERY,
    FilesEvent, FilesStore, filename_matches_query, is_document, short_file_type_label,
    short_file_type_label_for,
};
pub use friend::{Friend, FriendEvent, FriendState, FriendStore};
pub use gallery::{
    AttachmentSeedInput, ChannelAttachment, FetchedChannelAttachments, GalleryEvent, GalleryStore,
    LoadDirection, MediaFilter, UploaderInfo, enrich_uploader, fetch_channel_attachments,
    initial_page_has_more, next_page_has_more, resolve_attachment_uploader,
};
pub use gif::{Gif, GifCategory, GifEvent, GifStore};
pub use group_members::{GroupMember, GroupMembersEvent, GroupMembersStore};
pub use ids::{ChannelId, ClanId, MessageId, ParseIdError, RoleId, UserId};
pub use inbox::{GLOBAL_INBOX_BUCKET_CLAN_ID, InboxEvent, InboxStore};
pub use login::{LoginStore, token_from_oauth_callback_url};
pub use message::*;
pub use message::{
    COMBINE_TIME_WINDOW, Message, MessageAttachment, message_combined_with_prev,
    recompute_message_grouping, same_message_sender, should_show_message_head,
};
pub use message_search::{
    ChannelSearchState, MessageSearchEvent, MessageSearchStore, SearchHit, SearchHitImage,
    enrich_search_results_ogp, resolve_search_hit_ogp, search_hit_from_document,
};
pub use messages::*;
pub use mezon_client::{
    InboxCategory, InboxMentionSpan, InboxMessagePreview, InboxNotification, TopicDiscussion,
    TopicReplyPreview, attachment_link_is_image, message_content_is_attachment,
};
pub use mezon_client::{
    SearchDropdownMode, SearchPageToken, autocomplete_needle, expand_mention_name_tokens,
    finalize_incomplete_filter_token, has_filter_options, insert_filter_markup,
    search_content_highlight_terms, search_dropdown_mode, search_filter_chip_ranges,
    search_page_count, search_page_numbers, should_show_search_dropdown,
};
pub use mmn_client::{DECIMAL_FACTOR as TOKEN_DECIMAL_FACTOR, DECIMALS as TOKEN_DECIMALS};
pub use notification_push::NotificationPushStore;
pub use notification_setting::{NotificationSettingEvent, NotificationSettingStore};
pub use ogp::{
    OgpResult, OutgoingOgp, fetch_invite_preview, fetch_ogp, first_previewable_url,
    internal_invite_id, invite_id_from_url, trusted_invite_id,
};
pub use permissions::{
    ClanSettingsPermissions, PERMISSION_ADMINISTRATOR, PERMISSION_CLAN_OWNER,
    PERMISSION_MANAGE_CHANNEL, PERMISSION_MANAGE_CLAN, PERMISSION_SCOPE_CHANNEL,
    PermissionDefinition, PermissionEvent, PermissionStore, can_manage_channel,
};
pub use pinned::{PinnedEvent, PinnedMessage, PinnedMessagesStore};
pub use platform::{
    CliInstallHooks, CliInstallStateFn, CliInstallToggleFn, CliInstallVisibleFn,
    DesktopNotification, DownloadEvent, McpServerHooks, McpServerStatus, McpStartFn, McpStatusFn,
    McpStopFn, NotifyFn, OpenUrlFn, PlatformStore, copy_image_url_to_clipboard,
    download_url_with_dialog,
};
pub use presence::*;
pub use quick_menu::{QuickMenuItem, QuickMenuStore};
pub use realtime::{RealtimeDispatch, RealtimeKind};
pub use roles::{
    ClanRoleDetail, DEFAULT_ROLE_COLOR, MAX_ROLE_ICON_BYTES, Role, RoleDraft, RolePermission,
    RoleStyle, RoleUser, RolesEvent, RolesStore, everyone_slug, parse_role_color,
};
pub use sticker::{ClanSound, Sticker, StickerEvent, StickerStore};
pub use stream::{StreamMember, StreamPhase, StreamStore};
pub use threads::{
    GroupedThreadIndexes, THREAD_STATUS_ARCHIVED, THREAD_STATUS_JOINED, ThreadCreateFailReason,
    ThreadSummary, ThreadsEvent, ThreadsStore, group_threads,
};
pub use topic_badges::{TopicBadgeEvent, TopicBadgeStore};
pub use topics::{TopicsEvent, TopicsStore};
pub use ui_state::UiState;
pub use user_profile::{
    ProfileContext, UserProfileView, active_clan_id, current_user_clan_avatar, resolve_avatar_url,
    resolve_user_profile,
};
pub use users_by_user::{UsersByUserEvent, UsersByUserStore};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use voice::record_wayland_session;
pub use voice::{
    DeviceKind, DeviceMenuKind, DisplayedReaction, MAX_SOUND_BYTES, NetworkQuality, PickedScreen,
    SOUND_ALLOWED_EXTENSIONS, ScreenShareKind, ScreenShareListError, ScreenShareOption,
    ScreenSharePreview, VideoFrameData, VideoFrameStore, VoiceCallStatus, VoiceConnection,
    VoiceModerationError, VoiceParticipant, VoiceRenderFrame, VoiceStore, camera_tile_id,
    capture_screen_share_preview, list_screen_share_options, peek_screen_share_options,
    screen_tile_id, system_screen_share_pick, upload_sound_file, validate_sound_file,
};
pub use wallet::{
    SendTokenRequest, TransactionCursor, WalletDetail, WalletEvent, WalletStore, WalletTransaction,
    WalletTransactionPage,
};
pub use webhook::{
    ChannelWebhook, ClanWebhook, MAX_WEBHOOK_AVATAR_BYTES, WEBHOOK_NAME_MAX_LENGTH, WebhookEvent,
    WebhookStore,
};
pub use winstore_update::{
    WinstoreUpdateStore, effective_update_status, update_available_clicked, update_check_clicked,
    update_error_clicked, update_manual_install_clicked, update_restart_clicked,
};

pub const CACHE_TTL: Duration = Duration::from_secs(20 * 60);

/// TTL for clan-scoped catalog data that realtime events keep patched in place
/// (roles). Reconnect and a detected event-stream gap both `mark_all_stale`, so
/// this timer only has to catch events silently missed while the socket stayed
/// up — a case that does not need a 20-minute poll.
pub const EVENT_BACKED_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const SETTINGS_SAVE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Default)]
struct SettingsSaver {
    task: Option<gpui::Task<()>>,
    dirty: bool,
    entity_id: Option<gpui::EntityId>,
}
impl gpui::Global for SettingsSaver {}

/// `UserClanRemoved` / `UserChannelRemoved` / `UserChannelAdded` are broadcast to
/// everyone in the clan or channel, not just the affected members, so a handler that
/// adds or tears down local state must first confirm the signed-in user is one of the
/// ids carried by the event. Without this, kicking one member wipes the clan or
/// channel for every other member, and adding one member makes a private channel
/// appear in the sidebar of people who cannot open it.
pub(crate) fn event_targets_user(user_ids: &[i64], me: Option<ids::UserId>) -> bool {
    me.is_some_and(|me| user_ids.contains(&me.get()))
}

pub(crate) fn running_in_app_sandbox() -> bool {
    cfg!(target_os = "macos") && std::env::var("APP_SANDBOX_CONTAINER_ID").is_ok()
}

pub(crate) fn running_from_windows_store() -> bool {
    cfg!(target_os = "windows")
        && std::env::current_exe().is_ok_and(|exe| {
            exe.components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"))
        })
}

/// Persist [`Settings`] through one serialized, coalescing writer: burst
/// changes (slider drags) collapse into a single debounced write, writes never
/// overlap (so the shared tmp-file path cannot commit an older snapshot last),
/// and the snapshot is taken at write time so the latest state always wins.
pub fn schedule_settings_save(settings: &gpui::Entity<Settings>, cx: &mut gpui::App) {
    let saver = cx.default_global::<SettingsSaver>();
    debug_assert!(
        saver.entity_id.is_none_or(|id| id == settings.entity_id()),
        "schedule_settings_save called with a second Settings entity; the in-flight saver only snapshots the first"
    );
    saver.entity_id = Some(settings.entity_id());
    saver.dirty = true;
    if saver.task.is_some() {
        return;
    }
    let weak = settings.downgrade();
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SETTINGS_SAVE_DEBOUNCE).await;
            let snapshot = cx.update(|cx| {
                cx.default_global::<SettingsSaver>().dirty = false;
                weak.upgrade().map(|settings| settings.read(cx).clone())
            });
            let Some(snapshot) = snapshot else {
                cx.update(|cx| {
                    cx.default_global::<SettingsSaver>().task = None;
                });
                return;
            };
            cx.background_executor()
                .spawn(async move { snapshot.save_sync() })
                .await;
            let idle = cx.update(|cx| {
                let saver = cx.default_global::<SettingsSaver>();
                if saver.dirty {
                    false
                } else {
                    saver.task = None;
                    true
                }
            });
            if idle {
                return;
            }
        }
    });
    cx.default_global::<SettingsSaver>().task = Some(task);
}

/// Persistent application settings — written to ~/.config/mezon/settings.json
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Launch app on system login
    pub auto_start: bool,
    /// Enable GPU hardware acceleration
    pub hardware_acceleration: bool,
    /// UI zoom/scale factor (0.8 – 1.5)
    pub zoom_factor: f32,
    /// Last window bounds [x, y, width, height]
    pub window_bounds: Option<[i32; 4]>,
    /// UI theme: "dark" | "light" | "system"
    pub theme: String,
    /// UI language/locale code: "en" | "vi"
    pub language: String,
    /// Enable desktop notifications
    pub notifications_enabled: bool,
    /// Hide message content in push notifications
    pub notifications_hide_content: bool,
    /// Enable activity tracking (show online status to others)
    pub activity_tracking: bool,
    /// Microphone volume (0.0 – 1.0)
    pub mic_volume: f32,
    /// Speaker/headphone volume (0.0 – 1.0)
    pub speaker_volume: f32,
    /// Selected audio input device identifier
    pub input_device_id: Option<String>,
    /// Selected audio output device identifier
    pub output_device_id: Option<String>,
    /// Selected camera (video input) device identifier
    pub camera_device_id: Option<String>,
    /// User-defined clan ordering: list of clan_ids in display order
    #[serde(default)]
    pub clan_order: Vec<ClanId>,
    /// Last active clan_id (restored on startup to resume where the user left off)
    #[serde(default)]
    pub last_clan_id: Option<ClanId>,
    /// Last active channel_id within the last clan (restored on startup)
    #[serde(default)]
    pub last_channel_id: Option<ChannelId>,
    /// Start the HTTP MCP server in read-only mode (no write tools)
    #[serde(default)]
    pub mcp_read_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_start: false,
            hardware_acceleration: true,
            zoom_factor: 1.0,
            window_bounds: None,
            theme: "dark".to_string(),
            language: "en".to_string(),
            notifications_enabled: true,
            notifications_hide_content: false,
            activity_tracking: true,
            mic_volume: 0.8,
            speaker_volume: 0.8,
            input_device_id: None,
            output_device_id: None,
            camera_device_id: None,
            clan_order: Vec::new(),
            last_clan_id: None,
            last_channel_id: None,
            mcp_read_only: false,
        }
    }
}

impl Settings {
    /// Returns the path to the settings file: ~/.config/mezon/settings.json
    pub fn path() -> PathBuf {
        config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mezon")
            .join("settings.json")
    }

    /// Load settings from disk synchronously. Returns defaults if the file does not exist.
    pub fn load_sync() -> Self {
        let path = Self::path();
        if !path.exists() {
            tracing::debug!(
                "Settings file not found, using defaults: {}",
                path.display()
            );
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str(&data) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to parse settings, using defaults: {e}");
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read settings: {}", e);
                Self::default()
            }
        }
    }

    pub fn save_sync(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let data = match serde_json::to_string_pretty(self) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("Failed to serialize settings: {e}");
                return;
            }
        };
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, &data) {
            tracing::warn!("Failed to write settings tmp file: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!("Failed to rename settings tmp file: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Load settings from disk. Returns defaults if the file does not exist.
    pub async fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            tracing::debug!(
                "Settings file not found, using defaults: {}",
                path.display()
            );
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read settings from {}", path.display()))?;
        let settings: Self =
            serde_json::from_str(&data).with_context(|| "Failed to parse settings.json")?;
        tracing::debug!("Loaded settings from {}", path.display());
        Ok(settings)
    }

    pub async fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(self).context("Failed to serialize settings")?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &data)
            .await
            .with_context(|| format!("Failed to write settings tmp to {}", tmp.display()))?;
        fs::rename(&tmp, &path)
            .await
            .with_context(|| format!("Failed to rename settings tmp to {}", path.display()))?;
        tracing::debug!("Saved settings to {}", path.display());
        Ok(())
    }

    pub fn init_global(entity: &gpui::Entity<Self>, cx: &mut gpui::App) {
        cx.set_global(GlobalSettings(entity.clone()));
    }

    pub fn try_global(cx: &gpui::App) -> Option<gpui::Entity<Self>> {
        cx.try_global::<GlobalSettings>().map(|g| g.0.clone())
    }
}

struct GlobalSettings(gpui::Entity<Settings>);
impl gpui::Global for GlobalSettings {}

/// Which login method is currently shown in the `LoginView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginMethod {
    /// Email OTP — two-step flow (default).
    #[default]
    Otp,
    /// Email + password — single-step form.
    Password,
}

/// A user-visible login error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// Wrong credentials / bad OTP.
    InvalidCredentials,
    /// The server returned an unexpected error.
    ServerError(String),
    /// Could not reach the server.
    NetworkError(String),
    /// The OTP has expired; user must request a new one.
    OtpExpired,
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCredentials => write!(f, "Invalid credentials. Please try again."),
            Self::ServerError(msg) => write!(f, "Server error: {msg}"),
            Self::NetworkError(msg) => write!(f, "Network error: {msg}"),
            Self::OtpExpired => write!(f, "OTP has expired. Please request a new one."),
        }
    }
}

/// Authentication state of the application.
///
/// Drives which view is shown in the content area of `RootView`.
#[derive(Debug, Clone, Default)]
// Session is kept inline because AuthState is cloned and matched as a simple UI/store snapshot.
#[allow(clippy::large_enum_variant)]
pub enum AuthState {
    /// No session — show login form.
    #[default]
    NotAuthenticated,
    /// OTP email was sent; waiting for the user to enter the code.
    OtpRequested {
        /// Server-issued request ID — required for the confirm-OTP call.
        req_id: String,
        /// The email address the OTP was sent to.
        email: String,
    },
    /// OAuth2 browser was opened; waiting for the `mezonapp://callback` deep link.
    /// Kept for future OAuth integration.
    AwaitingCallback,
    /// TCP transport handshake in progress — loading screen visible.
    Connecting(Session),
    /// Token received, transport connected, and session is valid.
    Authenticated(Session),
}

impl AuthState {
    pub fn session_credentials(&self) -> Option<(String, String)> {
        match self {
            Self::Authenticated(session) | Self::Connecting(session) => {
                if session.id_token.is_empty() || session.user_id.is_empty() {
                    None
                } else {
                    Some((session.id_token.clone(), session.user_id.clone()))
                }
            }
            _ => None,
        }
    }
}
