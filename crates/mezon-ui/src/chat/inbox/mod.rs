mod panel;
mod row;

pub use panel::{InboxPopoverPanel, clan_has_inbox_badge, open_topic_reply};
pub use row::{
    FOR_YOU_ROW_HEIGHT, MENTION_ROW_HEIGHT, MESSAGE_ROW_HEIGHT, ROW_HEIGHT, TOPIC_ROW_HEIGHT,
};

pub(crate) use row::render_message_head;

use mezon_store::InboxCategory;

pub const fn row_height_for_tab(tab: InboxTab) -> f32 {
    match tab {
        InboxTab::ForYou => FOR_YOU_ROW_HEIGHT,
        InboxTab::Mentions => MENTION_ROW_HEIGHT,
        InboxTab::Messages => MESSAGE_ROW_HEIGHT,
        InboxTab::Topics => TOPIC_ROW_HEIGHT,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxTab {
    ForYou,
    Messages,
    #[default]
    Mentions,
    Topics,
}

impl InboxTab {
    pub fn all() -> [Self; 4] {
        [Self::ForYou, Self::Messages, Self::Mentions, Self::Topics]
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::ForYou => "notifications.tabs.forYou",
            Self::Messages => "notifications.tabs.messages",
            Self::Mentions => "notifications.tabs.mentions",
            Self::Topics => "notifications.tabs.topics",
        }
    }

    pub fn category(self) -> Option<InboxCategory> {
        match self {
            Self::ForYou => Some(InboxCategory::ForYou),
            Self::Messages => Some(InboxCategory::Messages),
            Self::Mentions => Some(InboxCategory::Mentions),
            Self::Topics => None,
        }
    }
}
