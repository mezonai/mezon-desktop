use gpui::SharedString;
use mezon_store::{AppChannel, ChannelType, VoiceMember};

#[derive(Clone, PartialEq)]
pub(super) struct VoiceMemberSlot {
    pub(super) user_id: String,
    pub(super) display_name: String,
    pub(super) avatar_url: String,
}

impl From<&VoiceMember> for VoiceMemberSlot {
    fn from(m: &VoiceMember) -> Self {
        Self {
            user_id: m.user_id.to_string(),
            display_name: m.display_name.clone(),
            avatar_url: m.avatar_url.clone(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct AppChannelSlot {
    pub(super) app_logo: Option<String>,
}

impl From<&AppChannel> for AppChannelSlot {
    fn from(a: &AppChannel) -> Self {
        Self {
            app_logo: a.app_logo.clone(),
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
    },
}
