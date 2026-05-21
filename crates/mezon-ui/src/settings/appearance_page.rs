use gpui::{Context, FontWeight, Window, prelude::*};
use gpui_component::{label::Label, v_flex};
use crate::theme::Theme;

pub struct AppearancePage;

impl AppearancePage {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for AppearancePage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        v_flex()
            .gap_6()
            .child(
                Label::new("Appearance")
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                Label::new("Theme settings coming soon")
                    .text_sm()
                    .text_color(theme.text_muted),
            )
    }
}
