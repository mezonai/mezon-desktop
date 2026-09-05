use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, Entity, HighlightStyle, Hsla, SharedString, WeakEntity};
use mezon_store::{
    ChannelType, ClanId, MessageId, ProfileContext, RichClick, RichLayout, Settings, SpriteAtlas,
    UserId,
};

use super::audio_player::AudioPlayerView;
use super::channel_messages::ChannelMessages;
use super::gif_video::GifVideoView;
use super::video_player::VideoPlayerView;
use crate::chat::mention_input::MentionInput;
use crate::components::primitives::{DatePicker, TextArea};
use crate::image_cache::LruImageCache;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingContext {
    pub clan_id: ClanId,
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
    pub is_topic_box: bool,
    pub scroll_active: bool,
    /// Message whose hover toolbar should show, after the React-style hover-intent
    /// delay (fast mouse sweeps never latch it). `None` = no toolbar visible.
    pub hovered_row: Option<MessageId>,
    /// Message with an open context menu — keeps row highlight/toolbar latched.
    pub context_menu_message: Option<MessageId>,
    pub avatar_cache: Entity<LruImageCache>,
    pub large_avatar_cache: Entity<LruImageCache>,
    pub icon_cache: Entity<LruImageCache>,
    pub ogp_cache: Entity<LruImageCache>,
    pub social_cache: Entity<LruImageCache>,
    pub sprite_cache: Entity<LruImageCache>,
    pub unread_boundary_id: Option<MessageId>,
    pub highlight_id: Option<MessageId>,
    pub reply_highlight_id: Option<MessageId>,
    pub profile_context: Option<ProfileContext>,
    pub settings: Entity<Settings>,
    pub active_videos: &'a HashMap<(MessageId, usize), Entity<VideoPlayerView>>,
    pub active_audios: &'a indexmap::IndexMap<(MessageId, usize), Entity<AudioPlayerView>>,
    pub gif_videos: &'a HashMap<(MessageId, usize), Entity<GifVideoView>>,
    pub embed_inputs: &'a HashMap<(MessageId, SharedString), Entity<TextArea>>,
    pub embed_date_pickers: &'a HashMap<(MessageId, SharedString), Entity<DatePicker>>,
    pub sprite_atlases: &'a HashMap<SharedString, Arc<SpriteAtlas>>,
    pub animation_starts: &'a HashMap<(MessageId, SharedString), std::time::Instant>,
    pub window_active: bool,
    pub video_host: WeakEntity<ChannelMessages>,
    pub now: chrono::DateTime<chrono::Local>,
    pub content_width: f32,
    /// Active clan of the currently open channel (permission-substitute + Topic gating).
    pub clan_id: Option<ClanId>,
    pub channel_type: Option<ChannelType>,
    /// The open channel is not itself a thread (`parent_id.is_none()`).
    pub channel_top_level: bool,
    /// `manage-thread` on the open channel, resolved once per render pass.
    pub can_manage_thread: bool,
    /// `send-message` on the open channel, resolved once per render pass.
    pub can_send_message: bool,
    /// The open conversation is a DM or group DM, where the clan permission
    /// above never applies.
    pub is_dm: bool,
    /// Message currently being edited inline, if any (shared across all rows).
    pub editing_id: Option<MessageId>,
    pub edit_input: Option<Entity<MentionInput>>,
    /// Up to 3 most-recently-used emoji for the hover toolbar's quick-react pills.
    pub emoji_recent: &'a [RecentEmojiCell],
    /// Cross-frame memo for per-row derived values that are expensive to
    /// recompute every frame (live avatar resolution, formatted time labels).
    /// Owned by the view; invalidated on member-store change, channel switch,
    /// locale change and day rollover.
    pub row_memo: Rc<RefCell<RowMemo>>,
    pub selection: super::selection::SharedSelection,
}

/// Per-frame view model for the quick-reaction strip. The proxied url and the
/// element-id prefix are identical for every row, so they are resolved once per
/// frame instead of rebuilt inside each row's hover actions.
pub struct RecentEmojiCell {
    pub id: SharedString,
    pub shortname: SharedString,
    pub src: SharedString,
    pub element_key: SharedString,
}

#[derive(Default)]
pub struct RowMemo {
    /// sender -> live-resolved (raw, proxied) avatar urls; `None` caches a
    /// failed resolution so the per-message fallback is used without a
    /// store lookup every frame.
    pub avatars: HashMap<UserId, Option<(SharedString, SharedString)>>,
    /// (clan, sender) -> resolved display name for head/avatar rows.
    pub display_names: HashMap<(Option<ClanId>, UserId), SharedString>,
    /// (clan, sender) -> resolved username colour + role icon for head rows.
    pub role_styles: HashMap<(Option<ClanId>, UserId), (Hsla, Option<SharedString>)>,
    /// message -> formatted head time label ("14:03" / "Yesterday at 14:03").
    pub time_labels: HashMap<MessageId, SharedString>,
    /// emoji id -> reaction pill image url. Reactions are not precomputed by the
    /// store the way message spans are, so without this the imgproxy url for
    /// every pill of every visible message is rebuilt on every frame.
    pub reaction_srcs: HashMap<SharedString, SharedString>,
    pub rich_text: HashMap<MessageId, RichTextRenderPlan>,
    pub selection_layouts: HashMap<MessageId, super::content::SelectableMessageLayoutCacheEntry>,
    pub selection_text_pieces:
        HashMap<SharedString, Rc<[super::content::CachedSelectableTextPiece]>>,
    pub hovered_rich_link: Option<(MessageId, Range<usize>)>,
    pub rich_link_hover_epoch: u32,
    pub poll_scrolls: HashMap<MessageId, gpui::ScrollHandle>,
}

pub const ROW_MEMO_CAPACITY: usize = 4096;

impl RowMemo {
    pub fn remember_role_style(
        &mut self,
        key: (Option<ClanId>, UserId),
        value: (Hsla, Option<SharedString>),
    ) {
        if self.role_styles.len() >= ROW_MEMO_CAPACITY {
            self.role_styles.clear();
        }
        self.role_styles.insert(key, value);
    }

    pub fn forget_clan_role_styles(&mut self, clan_id: ClanId) -> bool {
        let before = self.role_styles.len();
        self.role_styles
            .retain(|(clan, _), _| *clan != Some(clan_id));
        before != self.role_styles.len()
    }
}

#[derive(Clone)]
pub struct RichTextRenderPlan {
    pub layout: Arc<RichLayout>,
    pub colors: [Hsla; 7],
    pub edited: bool,
    pub text: SharedString,
    pub highlights: Arc<[(Range<usize>, HighlightStyle)]>,
    pub font_overrides: Arc<[(Range<usize>, SharedString)]>,
    pub click_ranges: Arc<[Range<usize>]>,
    pub actions: Arc<[RichClick]>,
    pub locale: SharedString,
}

pub const DEFAULT_DISPLAY_NAME_COLOR: u32 = 0x17_ac_86;
pub const REPLY_USERNAME_COLOR: u32 = 0x84_ad_ff;

pub const CONTENT_INSET: f32 = 72.0;
pub const AVATAR_SIZE: f32 = 40.0;
pub const AVATAR_LEFT: f32 = 16.0;
pub const CONTENT_RIGHT_PAD: f32 = 48.0;
pub const REPLY_INSET: f32 = 36.0;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;

    fn style() -> (Hsla, Option<SharedString>) {
        (Hsla::from(rgb(0x99_aa_b5)), None)
    }

    #[test]
    fn role_style_memo_is_keyed_by_clan_and_user() {
        let mut memo = RowMemo::default();
        let clan = ClanId(1);
        let user = UserId(2);
        memo.remember_role_style((Some(clan), user), style());
        memo.remember_role_style((None, user), (Hsla::from(rgb(0x17_ac_86)), None));

        assert_eq!(memo.role_styles.len(), 2);
        assert_eq!(memo.role_styles[&(Some(clan), user)].0.h, style().0.h);
    }

    #[test]
    fn role_style_memo_clears_at_capacity() {
        let mut memo = RowMemo::default();
        for index in 0..=ROW_MEMO_CAPACITY {
            memo.remember_role_style((Some(ClanId(1)), UserId(index as i64 + 1)), style());
        }
        assert_eq!(memo.role_styles.len(), 1);
    }

    #[test]
    fn forgetting_a_clan_keeps_other_scopes() {
        let mut memo = RowMemo::default();
        let user = UserId(2);
        memo.remember_role_style((Some(ClanId(1)), user), style());
        memo.remember_role_style((Some(ClanId(9)), user), style());
        memo.remember_role_style((None, user), style());

        assert!(memo.forget_clan_role_styles(ClanId(1)));
        assert_eq!(memo.role_styles.len(), 2);
        assert!(!memo.forget_clan_role_styles(ClanId(1)));
    }
}
