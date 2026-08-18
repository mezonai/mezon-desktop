pub mod area;
pub use mezon_canvas::{CanvasPopoverPanel, CanvasView, canvas_popover_on_open};
pub mod call_window;
pub mod channel_app_bar;
pub mod channel_header;
pub mod channel_settings;
pub mod channel_typing;
pub mod chat_sending;
pub mod clan_channels_page;
pub mod clan_events_page;
pub mod clan_management_page;
pub mod clan_members_page;
pub mod create_event_modal;
pub mod create_thread_panel;
pub mod create_topic_panel;
pub mod edit_group_modal;
pub mod file_type_icon;
pub mod files_popover;
pub mod friends_page;
pub mod gif_sticker_emoji;
pub mod grouping;
pub mod inbox;
pub mod input_bar;
pub mod layout;
pub mod media_channel;
pub mod member_list;
pub mod member_row_element;
pub mod mention_input;
pub mod message;
pub mod message_search;
pub mod notification_setting_modal;
pub mod notification_setting_popover;
pub mod pinned_popover;
pub mod record_window;
pub mod role_style;
pub mod screen_share_modal;
pub mod screen_share_pip;
pub mod stream;
pub mod threads_popover;
pub mod user_profile_modal;
pub mod user_profile_popover;
pub mod voice;
pub mod voice_sound_picker;

pub use area::ChatArea;
pub use channel_header::ChannelHeader;
pub use chat_sending::ChatSending;
pub use friends_page::FriendsPage;
pub use grouping::is_combined;
pub use input_bar::InputBar;
pub use layout::ChatLayout;
pub use member_list::MemberListPanel;
pub use mention_input::{MentionInput, MentionInputEvent};
pub use message::ChannelMessages;
pub use mezon_store::COMBINE_TIME_WINDOW;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTarget {
    pub sender_name: gpui::SharedString,
}
