use crate::components::primitives::{Label, Switch, h_flex, v_flex};
use crate::theme::ActiveTheme;
use gpui::{Context, Entity, FontWeight, Window, prelude::*};
use mezon_store::Settings;

pub struct ActivityPage {
    settings: Entity<Settings>,
}

impl ActivityPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self { settings }
    }
}

impl Render for ActivityPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let tracking = self.settings.read(cx).activity_tracking;

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_4()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.activity.title"))
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.activity.description"))
                            .text_sm()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                Switch::new("activity-tracking")
                    .checked(tracking)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.update(cx, |s, cx| {
                            s.activity_tracking = !s.activity_tracking;
                            cx.notify();
                        });
                        mezon_store::schedule_settings_save(&this.settings, cx);
                        cx.notify();
                    })),
            )
    }
}
