use gpui::{Context, Entity, FontWeight, Window, prelude::*};
use gpui_component::{h_flex, label::Label, switch::Switch, v_flex};
use mezon_store::Settings;
use crate::theme::Theme;

pub struct ActivityPage {
    settings: Entity<Settings>,
}

impl ActivityPage {
    pub fn new(settings: Entity<Settings>, _cx: &mut Context<Self>) -> Self {
        Self { settings }
    }
}

impl Render for ActivityPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let tracking = self.settings.read(cx).activity_tracking;
        let settings = self.settings.clone();

        v_flex()
            .gap_6()
            .child(
                Label::new("Activity")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
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
                                Label::new("Activity Tracking")
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                Switch::new("activity-tracking")
                                    .checked(tracking)
                                    .on_click(move |_, _window, cx| {
                                        settings.update(cx, |s, _| {
                                            s.activity_tracking = !s.activity_tracking;
                                            s.save_sync();
                                        });
                                    }),
                            ),
                    )
                    .child(
                        Label::new(
                            "Enable activity tracking to show your online status \
                             and current activity to other users.",
                        )
                        .text_sm()
                        .text_color(theme.text_muted),
                    ),
            )
    }
}
