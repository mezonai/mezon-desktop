use gpui::{AnyElement, Div, FontWeight, ParentElement, SharedString, Styled, div, px};

use crate::theme::Theme;

pub fn management_page(title: impl Into<SharedString>, body: AnyElement, theme: &Theme) -> Div {
    let title = title.into();
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .bg(theme.bg_primary)
        .child(
            div()
                .h(px(50.))
                .px_4()
                .flex()
                .items_center()
                .flex_shrink_0()
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
}

pub fn section_toolbar(title: impl Into<SharedString>, controls: AnyElement, theme: &Theme) -> Div {
    let title = title.into();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .h(px(50.))
        .px_4()
        .flex_shrink_0()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(controls)
}
