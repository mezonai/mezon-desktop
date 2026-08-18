use gpui::{App, SharedString, Window, div, prelude::*, px};

use super::{Button, h_flex};
use crate::theme::ActiveTheme;

#[derive(IntoElement)]
pub struct UnsavedChangesBar {
    message: SharedString,
    reset_button: Button,
    save_button: Button,
}

impl UnsavedChangesBar {
    pub fn new(
        message: impl Into<SharedString>,
        reset_button: Button,
        save_button: Button,
    ) -> Self {
        Self {
            message: message.into(),
            reset_button,
            save_button,
        }
    }
}

impl RenderOnce for UnsavedChangesBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .absolute()
            .bottom(px(20.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .occlude()
            .child(
                div()
                    .w(px(700.0))
                    .max_w(gpui::relative(0.9))
                    .py(px(10.0))
                    .pl_4()
                    .pr(px(10.0))
                    .rounded(px(5.0))
                    .bg(theme.bg_floating)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_secondary)
                                    .child(self.message),
                            )
                            .child(
                                h_flex()
                                    .gap_4()
                                    .items_center()
                                    .child(self.reset_button)
                                    .child(self.save_button),
                            ),
                    ),
            )
    }
}
