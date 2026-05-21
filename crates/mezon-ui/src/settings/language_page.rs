use crate::theme::Theme;
use gpui::{Context, FontWeight, Window, div, prelude::*};
use gpui_component::{Icon, IconName, h_flex, label::Label, v_flex};

pub struct LanguagePage;

impl LanguagePage {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for LanguagePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        v_flex()
            .gap_6()
            .child(
                Label::new("Language")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
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
