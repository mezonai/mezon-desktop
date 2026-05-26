use crate::theme::resolve_theme;
use gpui::{Context, Entity, FontWeight, Window, prelude::*};
use gpui_component::{h_flex, label::Label, switch::Switch, v_flex};
use mezon_store::Settings;

pub struct AdvancedPage {
    settings: Entity<Settings>,
}

impl AdvancedPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self { settings }
    }
}

impl Render for AdvancedPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);
        let hw_accel = self.settings.read(cx).hardware_acceleration;
        let settings = self.settings.clone();

        v_flex()
            .gap_6()
            .child(
                Label::new("Advanced")
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
                                Label::new("Hardware Acceleration").text_color(theme.text_primary),
                            )
                            .child(
                                Switch::new("hardware-acceleration")
                                    .checked(hw_accel)
                                    .on_click(move |_, _window, cx| {
                                        settings.update(cx, |s, _| {
                                            s.hardware_acceleration = !s.hardware_acceleration;
                                            s.save_sync();
                                        });
                                    }),
                            ),
                    )
                    .child(
                        Label::new(
                            "Use GPU to accelerate rendering. Restart required to apply changes.",
                        )
                        .text_sm()
                        .text_color(theme.text_muted),
                    ),
            )
    }
}
