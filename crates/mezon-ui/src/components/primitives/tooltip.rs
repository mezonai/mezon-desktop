use gpui::{AnyView, App, Context, FontWeight, SharedString, Window, div, prelude::*, px};

use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TooltipSize {
    Compact,
    Prominent,
}

pub struct Tooltip {
    text: SharedString,
    size: TooltipSize,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: TooltipSize::Compact,
        }
    }

    pub fn build(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
        cx.new(|_| Tooltip::new(text)).into()
    }

    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| Self::build(title.clone(), cx)
    }

    pub fn prominent_text(
        title: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let title = title.into();
        move |_, cx| {
            let text = title.clone();
            cx.new(|_| Tooltip {
                text,
                size: TooltipSize::Prominent,
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prominent = self.size == TooltipSize::Prominent;
        div()
            .font_family(crate::theme::ui_font_family(cx))
            .px(px(8.))
            .py(px(4.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tokens.bg_tooltip_app)
            .when(prominent, |el| el.text_sm().font_weight(FontWeight::MEDIUM))
            .when(!prominent, |el| el.text_xs())
            .text_color(cx.theme().tokens.text_tooltip_app)
            .child(self.text.clone())
    }
}
