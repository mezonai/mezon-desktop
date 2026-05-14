use gpui::{App, ClickEvent, SharedString, Window, div, prelude::*};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

pub struct SettingsScreen {
    navigate: crate::components::NavigateFn,
}

impl SettingsScreen {
    pub fn new(navigate: crate::components::NavigateFn) -> Self {
        Self { navigate }
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        let navigate = self.navigate.clone();
        let mut close_btn = div()
            .id(SharedString::from("settings-close-btn"))
            .absolute()
            .top_4()
            .right_4()
            .cursor_pointer()
            .child(
                Icon::new(IconName::Close)
                    .size_6()
                    .text_color(theme.text_secondary),
            );
        close_btn
            .interactivity()
            .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                navigate("/chat", cx);
            });

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.bg_primary)
            .child(close_btn)
            .child(
                div()
                    .text_2xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.text_primary)
                    .child("Settings"),
            )
    }
}
