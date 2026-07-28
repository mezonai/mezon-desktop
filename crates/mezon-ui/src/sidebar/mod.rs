pub mod channel_sidebar;
pub mod clan_sidebar;
pub mod direct_sidebar;

use gpui::{App, Div, FontWeight, Pixels, SharedString, div, prelude::*, px, rgb, white};
use mezon_store::{ChannelId, NotificationSettingStore};

pub(crate) fn dm_muted(channel_id: ChannelId, cx: &App) -> bool {
    NotificationSettingStore::try_global(cx)
        .is_some_and(|store| store.read(cx).is_time_muted(channel_id))
}

pub(crate) fn friend_request_badge(count: usize, text_size: Pixels) -> Div {
    let label = if count >= 100 {
        SharedString::from("99+")
    } else {
        SharedString::from(count.to_string())
    };
    div()
        .h(px(16.))
        .min_w(px(16.))
        .px(px(3.))
        .rounded_full()
        .bg(rgb(0xda_37_3c))
        .flex()
        .items_center()
        .justify_center()
        .text_size(text_size)
        .font_weight(FontWeight::BOLD)
        .text_color(white())
        .child(label)
}
