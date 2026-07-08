pub mod area;
pub mod channel_header;
pub mod channel_typing;
pub mod chat_sending;
pub mod create_thread_panel;
pub mod grouping;
pub mod inbox;
pub mod input_bar;
pub mod layout;
pub mod member_list;
pub mod mention_input;
pub mod message;
pub mod pinned_popover;
pub mod screen_share_modal;
pub mod screen_share_pip;
pub mod threads_popover;
pub mod user_profile_popover;
pub mod voice;
pub mod voice_sound_picker;

pub use area::ChatArea;
pub use channel_header::ChannelHeader;
pub use chat_sending::ChatSending;
pub use grouping::is_combined;
pub use input_bar::InputBar;
pub use layout::ChatLayout;
pub use member_list::MemberListPanel;
pub use mention_input::{MentionInput, MentionInputEvent};
pub use message::ChannelMessages;
pub use mezon_store::COMBINE_TIME_WINDOW;

#[derive(Debug, Clone)]
pub struct ReplyTarget {
    pub sender_name: String,
}
