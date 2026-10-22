use gpui::{AnyView, App, Context, SharedString, Window, div, prelude::*, px};

use crate::theme::ActiveTheme;

pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }

    pub fn build(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
        cx.new(|_| Tooltip::new(text)).into()
    }

    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| Self::build(title.clone(), cx)
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(crate::theme::ui_font_family(cx))
            .px(px(8.))
            .py(px(4.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tokens.bg_tooltip_app)
            .text_xs()
            .text_color(cx.theme().tokens.text_tooltip_app)
            .child(self.text.clone())
    }
}
