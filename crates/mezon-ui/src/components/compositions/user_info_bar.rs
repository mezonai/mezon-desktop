use std::sync::Arc;

use gpui::{App, ClickEvent, Window, div, prelude::*, px};

use gpui_component::Sizable;

use crate::components::primitives::{Avatar, Icon, IconName, Size};
use crate::theme::Theme;

pub struct UserInfoBar {
    username: String,
    presence: String,
    on_settings: Option<Arc<dyn Fn(&str, &mut App) + Send + Sync>>,
}

impl UserInfoBar {
    pub fn new(
        username: impl Into<String>,
        on_settings: Option<Arc<dyn Fn(&str, &mut App) + Send + Sync>>,
    ) -> Self {
        Self {
            username: username.into(),
            presence: "Online".to_string(),
            on_settings,
        }
    }

    pub fn presence(mut self, status: impl Into<String>) -> Self {
        self.presence = status.into();
        self
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        let initials = self
            .username
            .chars()
            .next()
            .unwrap_or('?')
            .to_string()
            .to_uppercase();

        let presence_color = match self.presence.as_str() {
            "Online" => theme.status_online,
            "Idle" => theme.status_idle,
            "Dnd" => theme.status_dnd,
            _ => theme.status_offline,
        };

        let on_settings = self.on_settings.clone();
        let mut settings_btn = div().cursor_pointer().child(
            Icon::new(IconName::Settings)
                .size(px(16.0))
                .text_color(theme.text_muted),
        );
        settings_btn.interactivity().on_click(
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(ref cb) = on_settings {
                    cb("/settings", cx);
                }
            },
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_2()
            .py_2()
            .gap_2()
            .bg(theme.bg_primary)
            .child(
                div()
                    .relative()
                    .child(Avatar::new().name(initials).with_size(Size::Small))
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .right_0()
                            .size_2()
                            .rounded_full()
                            .bg(presence_color),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(self.username.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(self.presence.clone()),
                    ),
            )
            .child(div().flex_1())
            .child(
                Icon::new(IconName::Mic)
                    .size(px(16.0))
                    .text_color(theme.text_muted),
            )
            .child(
                Icon::new(IconName::Deafen)
                    .size(px(16.0))
                    .text_color(theme.text_muted),
            )
            .child(settings_btn)
    }
}
