use gpui::div;
use gpui::prelude::*;

use crate::components::primitives::{Avatar, AvatarSize, Icon, IconName, PresenceStatus};
use crate::theme::Theme;

pub struct UserInfoBar {
    username: String,
    presence: PresenceStatus,
}

impl UserInfoBar {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            presence: PresenceStatus::Online,
        }
    }

    pub fn presence(mut self, status: PresenceStatus) -> Self {
        self.presence = status;
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

        let presence_label = match self.presence {
            PresenceStatus::Online => "Online",
            PresenceStatus::Idle => "Idle",
            PresenceStatus::Dnd => "Do Not Disturb",
            PresenceStatus::Offline => "Offline",
        };

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
                Avatar::new(initials)
                    .size(AvatarSize::Sm)
                    .presence(self.presence)
                    .render(theme),
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
                            .child(presence_label),
                    ),
            )
            .child(div().flex_1())
            .child(
                Icon::new(IconName::Mic)
                    .size(16.0)
                    .color(theme.text_muted)
                    .render(theme),
            )
            .child(
                Icon::new(IconName::Deafen)
                    .size(16.0)
                    .color(theme.text_muted)
                    .render(theme),
            )
            .child(
                Icon::new(IconName::Settings)
                    .size(16.0)
                    .color(theme.text_muted)
                    .render(theme),
            )
    }
}
