use gpui::SharedString;
use mezon_store::{AppChannel, ChannelType};

#[derive(Clone, PartialEq)]
pub(super) struct VoiceMemberSlot {
    pub(super) user_id: String,
    pub(super) display_name: String,
    pub(super) avatar_url: String,
    pub(super) avatar_raw: String,
    pub(super) sharing_screen: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) struct AppChannelSlot {
    pub(crate) app_id: String,
    pub(crate) app_name: String,
    pub(crate) app_logo: Option<String>,
    pub(crate) app_url: String,
    pub(crate) channel_id: mezon_store::ChannelId,
}

impl From<&AppChannel> for AppChannelSlot {
    fn from(a: &AppChannel) -> Self {
        Self {
            app_id: a.app_id.clone(),
            app_name: a.app_name.clone(),
            app_logo: a.app_logo.clone(),
            app_url: a.app_url.clone(),
            channel_id: a.channel_id,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) enum SidebarItem {
    BannerAndEvents {
        banner_url: Option<String>,
        app_channels: Vec<AppChannelSlot>,
    },
    Category {
        elem_id: SharedString,
        name: String,
        name_upper: String,
        id: String,
        collapsed: bool,
    },
    Channel {
        elem_id: SharedString,
        row_elem_id: SharedString,
        id: String,
        name: String,
        channel_type: ChannelType,
        unread: bool,
        private: bool,
        selected: bool,
        badge_count: u32,
        badge_label: SharedString,
        muted: bool,
        is_thread: bool,
        is_favorite: bool,
        line_above: bool,
        line_below: bool,
        voice_members: Vec<VoiceMemberSlot>,
        voice_compact: bool,
    },
}
