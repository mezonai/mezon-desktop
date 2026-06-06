use gpui::{div, prelude::*, px};

use crate::theme::Theme;

pub struct ChannelHeader {
    name: String,
}

impl ChannelHeader {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .h(px(44.))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.bg_primary)
            .child(
                div().flex().flex_row().items_center().gap_1().child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(format!("# {}", self.name)),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .child("📌"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .child("🔍"),
            )
    }
}
