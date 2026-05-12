use gpui::{Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::components::TextChangeHandler;
use crate::theme::Theme;

pub struct FormField {
    label: Option<String>,
    input: Entity<InputState>,
    error: Option<String>,
    masked: bool,
    on_change: Option<TextChangeHandler>,
    _subscriptions: Vec<Subscription>,
}

impl FormField {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, label: impl Into<String>) -> Self {
        let label_str: String = label.into();
        let placeholder = label_str.clone();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));

        let _subscriptions = vec![cx.subscribe_in(&input, window, {
            let input = input.clone();
            move |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    if let Some(handler) = &this.on_change {
                        let value = input.read(cx).value();
                        handler(value.as_ref(), window, cx);
                    }
                    cx.notify();
                }
            }
        })];

        Self {
            label: Some(label_str),
            input,
            error: None,
            masked: false,
            on_change: None,
            _subscriptions,
        }
    }

    pub fn set_masked(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_masked(true, window, cx);
        });
    }

    pub fn set_on_change(&mut self, cb: TextChangeHandler, cx: &mut Context<Self>) {
        self.on_change = Some(cb);
        cx.notify();
    }

    pub fn set_error(&mut self, err: Option<String>, cx: &mut Context<Self>) {
        self.error = err;
        cx.notify();
    }

    pub fn value(&self, cx: &gpui::App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn input_entity(&self) -> &Entity<InputState> {
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

        let mut input = Input::new(&self.input).w_full();
        if self.masked {
            input = input.mask_toggle();
        }

        container = container.child(input);

        if let Some(error) = &self.error {
            container = container.child(
                div()
                    .text_xs()
                    .text_color(theme.status_dnd)
                    .child(SharedString::from(error.clone())),
            );
        }

        container
    }
}
