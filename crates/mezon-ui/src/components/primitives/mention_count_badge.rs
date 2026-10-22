use gpui::{FontWeight, InteractiveElement, SharedString, Styled, div, prelude::*, px, rgb, white};

/// Numeric unread / mention badge — matches React `ChannelBadge` and clan sidebar pills
/// (`bg-red-600`, white `text-xs`, 16×16 or 22×16 for double digits).
pub fn mention_count_badge(count: u32) -> gpui::Div {
    let wide = count >= 10;
    let label = if count >= 100 {
        SharedString::from("99+")
    } else {
        SharedString::from(count.to_string())
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(16.))
        .when(wide, |s| s.w(px(22.)))
        .when(!wide, |s| s.w(px(16.)))
        .rounded_full()
        .bg(rgb(0xda373c))
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(white())
        .child(label)
}

pub fn unread_dot() -> gpui::Div {
    div()
        .flex_none()
        .size(px(7.))
        .rounded_full()
        .bg(rgb(0xda373c))
}

/// Channel row badge: absolute right, hidden on row hover (React `group-hover:hidden`).
pub fn mention_count_badge_on_channel_row(count: u32, group_name: SharedString) -> gpui::Div {
    mention_count_badge(count)
        .absolute()
        .top(px(9.))
        .right(px(12.))
        .group_hover(group_name, |s| s.opacity(0.))
}
