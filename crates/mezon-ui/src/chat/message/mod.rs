mod audio_player;
mod call_log_card;
mod channel_messages;
mod content;
mod context;
mod create_poll_modal;
mod custom_status_modal;
mod dispatch;
mod embed_card;
mod embed_fields;
mod forward_modal;
mod gif_video;
pub mod inline_content;
mod invite_card;
mod message_actions_panel;
mod message_buzz_modal;
mod message_context_menu;
mod ogp_embed;
mod parts;
mod poll_card;
mod poll_detail_modal;
mod reaction_detail;
mod reaction_picker;
mod report_modal;
mod selection;
mod send_token_modal;
mod share_contact_card;
mod share_location_modal;
mod skeleton;
mod system_row;
mod time;
mod token_transaction_card;
mod topic_view_button;
mod transaction_history_modal;
mod user_row;
mod video_player;

pub use channel_messages::ChannelMessages;
pub(crate) use content::open_message_link;
pub use context::DEFAULT_DISPLAY_NAME_COLOR;
pub(crate) use create_poll_modal::CreatePollModal;
pub(crate) use custom_status_modal::CustomStatusModal;
pub use forward_modal::{ShareContactModal, share_contact_subject};
pub use gif_video::VideoThumbView;
pub(crate) use message_buzz_modal::MessageBuzzModal;
pub(crate) use ogp_embed::render_ogp_preview;
pub(crate) use reaction_picker::{ReactionPicker, ReactionPickerEvent};
pub(crate) use send_token_modal::SendTokenModal;
pub(crate) use share_location_modal::ShareLocationModal;
pub(crate) use time::format_channel_setting_relative_time_from_seconds;
pub(crate) use time::format_message_time;
pub use time::format_relative_time_from_seconds;
pub(crate) use transaction_history_modal::TransactionHistoryModal;
pub use video_player::{VideoActivation, VideoFullscreenMode, VideoLayout, VideoPlayerView};

use gpui::{App, SharedString};

use crate::app::shell::Shell;

pub(crate) fn coming_soon_toast(locale: &str, cx: &mut App) {
    let message = SharedString::from(mezon_i18n::t(locale, "common.comingSoon").to_string());
    Shell::global(cx).update(cx, move |shell, cx| shell.info(message, cx));
}
