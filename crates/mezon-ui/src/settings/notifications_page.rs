use crate::theme::resolve_theme;
use gpui::{Context, Entity, FontWeight, Window, prelude::*};
use gpui_component::{h_flex, label::Label, switch::Switch, v_flex};
use mezon_store::Settings;

pub struct NotificationsPage {
    settings: Entity<Settings>,
}

impl NotificationsPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self { settings }
    }
}

impl Render for NotificationsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);
        let hide_content = self.settings.read(cx).notifications_hide_content;
        let settings = self.settings.clone();

        v_flex()
            .gap_6()
            .child(
                Label::new("Notifications")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .p_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                Label::new("Hide Notification Content")
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                Switch::new("hide-notification-content")
                                    .checked(hide_content)
                                    .on_click(move |_, _window, cx| {
                                        settings.update(cx, |s, _| {
                                            s.notifications_hide_content =
                                                !s.notifications_hide_content;
                                            s.save_sync();
                                        });
                                    }),
                            ),
                    )
                    .child(
                        Label::new(
                            "When enabled, message content will be hidden \
                             from notification popups and the lock screen.",
                        )
                        .text_sm()
                        .text_color(theme.text_muted),
                    ),
            )
    }
}
