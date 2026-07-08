mod audio_player;
mod call_log_card;
mod channel_messages;
mod content;
mod context;
mod dispatch;
mod embed_card;
mod embed_fields;
mod forward_modal;
mod gif_video;
mod invite_card;
mod message_actions_panel;
mod message_context_menu;
mod ogp_embed;
mod parts;
mod poll_card;
mod poll_detail_modal;
mod reaction_detail;
mod reaction_picker;
mod report_modal;
mod share_contact_card;
mod skeleton;
mod system_row;
mod time;
mod token_transaction_card;
mod topic_view_button;
mod user_row;
mod video_player;

pub use channel_messages::ChannelMessages;
pub use context::DEFAULT_DISPLAY_NAME_COLOR;
pub use gif_video::VideoThumbView;
pub(crate) use reaction_picker::{ReactionPicker, ReactionPickerEvent};
pub use video_player::{VideoActivation, VideoFullscreenMode, VideoLayout, VideoPlayerView};

use gpui::{App, SharedString};

use crate::app::shell::Shell;

pub(crate) fn coming_soon_toast(locale: &str, cx: &mut App) {
    let message = SharedString::from(mezon_i18n::t(locale, "common.comingSoon").to_string());
    Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
}
