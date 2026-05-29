use gpui::{App, ClickEvent, Entity, SharedString, Window, div, prelude::*, px};

use gpui_component::Sizable;

use crate::components::primitives::{Avatar, Icon, IconName, Size};
use crate::text_utils::compute_initials;
use crate::theme::Theme;
use mezon_store::AuthState;

fn on_settings_click(
    on_settings: Option<crate::components::NavigateFn>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        if let Some(ref cb) = on_settings {
            cb(crate::components::NavOp::Push("/settings".to_string()), cx);
        }
    }
}
pub struct UserInfoBar {
    auth_state: Entity<AuthState>,
    presence: String,
    on_settings: Option<crate::components::NavigateFn>,
}

impl UserInfoBar {
    pub fn new(
        auth_state: Entity<AuthState>,
        on_settings: Option<crate::components::NavigateFn>,
    ) -> Self {
        Self {
            auth_state,
            presence: "Online".to_string(),
            on_settings,
        }
    }

    pub fn presence(mut self, status: impl Into<String>) -> Self {
        self.presence = status.into();
        self
    }

    pub fn render(&self, theme: &Theme, cx: &App) -> impl IntoElement {
        let username = match self.auth_state.read(cx) {
            mezon_store::AuthState::Authenticated(session) => &session.username,
            _ => "Unknown",
        };
        let initials = compute_initials(username);

        let presence_color = match self.presence.as_str() {
            "Online" => theme.status_online,
            "Idle" => theme.status_idle,
            "Dnd" => theme.status_dnd,
            _ => theme.status_offline,
        };

        let mut settings_btn = div()
            .id(SharedString::from("settings-btn"))
            .cursor_pointer()
            .child(
                Icon::new(IconName::Settings)
                    .size(px(16.0))
                    .text_color(theme.text_muted),
            );
        settings_btn
            .interactivity()
            .on_click(on_settings_click(self.on_settings.clone()));

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
                            .child(username.to_string()),
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
