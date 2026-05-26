use crate::theme::resolve_theme;
use gpui::{Context, Entity, FontWeight, Window, div, prelude::*};
use gpui_component::{Icon, IconName, h_flex, label::Label, v_flex};
use mezon_store::Settings;

pub struct LanguagePage {
    settings: Entity<Settings>,
}

impl LanguagePage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());
        Self { settings }
    }
}

impl Render for LanguagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);
        v_flex()
            .gap_6()
            .child(
                Label::new("Language")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                Label::new("Select your language")
                    .text_sm()
                    .text_color(theme.text_muted),
            )
            .child(
                v_flex().rounded_lg().bg(theme.bg_primary).child(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .child(div().rounded_full().size_2().bg(theme.status_online))
                        .child(
                            Label::new("\u{1F1FA}\u{1F1F8} English").text_color(theme.text_primary),
                        )
                        .child(div().flex_1())
                        .child(
                            Icon::new(IconName::Check)
                                .size_4()
                                .text_color(theme.text_primary),
                        ),
                ),
            )
    }
}
