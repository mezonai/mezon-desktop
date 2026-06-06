pub mod channel_header;
pub mod grouping;
pub mod input_bar;
pub mod message_list;
pub mod message_row;

pub use channel_header::ChannelHeader;
pub use grouping::{COMBINE_TIME_WINDOW, MessageGroup, group_messages};
pub use input_bar::InputBar;
pub use message_list::MessageList;
pub use message_row::MessageRow;

#[derive(Debug, Clone)]
pub struct ReplyTarget {
    pub sender_name: String,
    pub content_preview: String,
}
