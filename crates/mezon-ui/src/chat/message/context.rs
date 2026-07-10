use std::collections::HashMap;

use gpui::{App, Entity, SharedString, WeakEntity};
use mezon_store::{ChannelType, ClanId, Emoji, MessageId, ProfileContext, Settings};

use super::audio_player::AudioPlayerView;
use super::channel_messages::ChannelMessages;
use super::gif_video::GifVideoView;
use super::video_player::VideoPlayerView;
use crate::chat::mention_input::MentionInput;
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingContext {
    pub members_invited: bool,
    pub sent_message: bool,
    pub downloaded_app: bool,
    pub created_channel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WelcomeContext {
    Channel {
        name: SharedString,
        private: bool,
        is_stream: bool,
    },
    Thread {
        name: SharedString,
        private: bool,
        username: SharedString,
    },
    Direct {
        display_name: SharedString,
        username: SharedString,
        avatar: SharedString,
    },
    Group {
        name: SharedString,
        avatar: SharedString,
    },
}

pub struct RowCtx<'a> {
    pub app: &'a App,
    pub theme: &'a Theme,
    pub locale: &'a str,
    pub current_user_id: &'a str,
    pub current_role_ids: &'a [i64],
    pub welcome: Option<WelcomeContext>,
    pub onboarding: Option<OnboardingContext>,
    pub suppress_hover: bool,
    /// Message whose hover toolbar should show, after the React-style hover-intent
    /// delay (fast mouse sweeps never latch it). `None` = no toolbar visible.
    pub hovered_row: Option<MessageId>,
    pub avatar_cache: Entity<LruImageCache>,
    pub large_avatar_cache: Entity<LruImageCache>,
    pub unread_boundary_id: Option<MessageId>,
    pub highlight_id: Option<MessageId>,
    pub reply_highlight_id: Option<MessageId>,
    pub profile_context: Option<ProfileContext>,
    pub settings: Entity<Settings>,
    pub active_videos: &'a HashMap<(MessageId, usize), Entity<VideoPlayerView>>,
    pub active_audios: &'a HashMap<(MessageId, usize), Entity<AudioPlayerView>>,
    pub gif_videos: &'a HashMap<(MessageId, usize), Entity<GifVideoView>>,
    pub video_host: WeakEntity<ChannelMessages>,
    pub now: chrono::DateTime<chrono::Local>,
    /// Active clan of the currently open channel (permission-substitute + Topic gating).
    pub clan_id: Option<ClanId>,
    pub channel_type: Option<ChannelType>,
    /// The open channel is not itself a thread (`parent_id.is_none()`).
    pub channel_top_level: bool,
    /// Reduced substitute for React's role-permission checks: clan creator only.
    pub is_clan_owner: bool,
    /// Message currently being edited inline, if any (shared across all rows).
    pub editing_id: Option<MessageId>,
    pub edit_input: Option<Entity<MentionInput>>,
    /// Up to 3 most-recently-used emoji for the hover toolbar's quick-react pills.
    pub emoji_recent: &'a [Emoji],
    /// `common.comingSoon`, resolved once per render pass (not per row).
    pub coming_soon: SharedString,
}

pub const DEFAULT_DISPLAY_NAME_COLOR: u32 = 0x17_ac_86;
pub const REPLY_USERNAME_COLOR: u32 = 0x84_ad_ff;

pub const CONTENT_INSET: f32 = 72.0;
pub const AVATAR_SIZE: f32 = 40.0;
pub const AVATAR_LEFT: f32 = 16.0;
pub const CONTENT_RIGHT_PAD: f32 = 48.0;
pub const REPLY_INSET: f32 = 36.0;
