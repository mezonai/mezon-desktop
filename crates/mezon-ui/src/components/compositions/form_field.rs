use gpui::{Context, Entity, FontWeight, Window, div, prelude::*};

use crate::components::TextChangeHandler;
use crate::components::primitives::TextInput;
use crate::theme::Theme;

pub struct FormField {
    label: Option<String>,
    input: Entity<TextInput>,
}

impl FormField {
    pub fn new(cx: &mut Context<Self>, label: impl Into<String>) -> Self {
        let label_str: String = label.into();
        let placeholder = label_str.clone();
        let input = cx.new(|cx| TextInput::new(cx, label_str.clone()).placeholder(placeholder));
        Self {
            label: Some(label_str),
            input,
        }
    }

    pub fn set_masked(&self, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.masked = true;
            cx.notify();
        });
    }

    pub fn set_on_change(&self, cb: TextChangeHandler, cx: &mut Context<Self>) {
        self.input.update(cx, |input, _cx| {
            input.on_change = Some(cb);
        });
    }

    pub fn set_error(&self, err: Option<String>, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_error(err, cx);
        });
    }

    pub fn value(&self, cx: &gpui::App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn input_entity(&self) -> &Entity<TextInput> {
        &self.input
    }
}

impl Render for FormField {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        let mut container = div().flex().flex_col().gap_1().w_full();

        if let Some(label) = &self.label {
            container = container.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child(label.to_uppercase()),
            );
        }

        container.child(self.input.clone())
    }
}
