use crate::components::primitives::{Label, Switch, h_flex, v_flex};
use crate::theme::ActiveTheme;
use gpui::{Context, Entity, Window, prelude::*};
use mezon_store::Settings;

pub struct NotificationsPage {
    settings: Entity<Settings>,
}

impl NotificationsPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self { settings }
    }
}

impl Render for NotificationsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let hide_content = self.settings.read(cx).notifications_hide_content;

        v_flex()
            .gap_6()
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
                                Label::new(mezon_i18n::t(
                                    &locale,
                                    "setting.notifications.hideContent",
                                ))
                                .text_color(theme.text_primary),
                            )
                            .child(
                                Switch::new("hide-notification-content")
                                    .checked(hide_content)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings.update(cx, |s, _| {
                                            s.notifications_hide_content =
                                                !s.notifications_hide_content;
                                        });
                                        let snapshot = this.settings.read(cx).clone();
                                        cx.background_executor()
                                            .spawn(async move {
                                                snapshot.save_sync();
                                            })
                                            .detach();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            &locale,
                            "setting.notifications.hideContentDesc",
                        ))
                        .text_sm()
                        .text_color(theme.text_muted),
                    ),
            )
    }
}
