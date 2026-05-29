use gpui::{App, Context, Entity, Subscription, Window, div, prelude::*};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::components::OtpCompleteHandler;
use crate::theme::Theme;

pub struct OtpInput {
    digit_count: usize,
    inputs: Vec<Entity<InputState>>,
    on_complete: Option<OtpCompleteHandler>,
    _suppressing_change: bool,
    _subscriptions: Vec<Subscription>,
}

impl OtpInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, digit_count: usize) -> Self {
        let inputs: Vec<Entity<InputState>> = (0..digit_count)
            .map(|_| {
                cx.new(|cx| {
                    InputState::new(window, cx).validate(|text, _cx| {
                        text.is_empty() || text.chars().all(|c| c.is_ascii_digit())
                    })
                })
            })
            .collect();

        let _subscriptions = inputs
            .iter()
            .enumerate()
            .map(|(i, _)| {
                cx.subscribe_in(&inputs[i], window, {
                    move |this: &mut OtpInput, _, event: &InputEvent, window, cx| {
                        if let InputEvent::Change = event {
                            if this._suppressing_change {
                                return;
                            }
                            this._suppressing_change = true;

                            let raw = this.inputs[i].read(cx).value().to_string();
                            let digits: String =
                                raw.chars().filter(|c| c.is_ascii_digit()).collect();

                            if digits.is_empty() {
                                if i > 0 {
                                    this.inputs[i - 1].update(cx, |input, cx| {
                                        input.focus(window, cx);
                                    });
                                }
                            } else if digits.len() == 1 {
                                let next = i + 1;
                                if next < this.digit_count {
                                    this.inputs[next].update(cx, |input, cx| {
                                        input.focus(window, cx);
                                    });
                                }

                                if (next >= this.digit_count || this.all_filled(cx))
                                    && let Some(ref cb) = this.on_complete
                                {
                                    let code: String = this
                                        .inputs
                                        .iter()
                                        .map(|input| input.read(cx).value().to_string())
                                        .collect();
                                    cb(code, window, cx);
                                }
                            } else {
                                let first = digits.chars().next().unwrap();
                                this.inputs[i].update(cx, |input, cx| {
                                    input.set_value(first.to_string(), window, cx);
                                });
                                let end = std::cmp::min(i + digits.len(), this.digit_count);
                                for (offset, ch) in digits.chars().enumerate().skip(1) {
                                    let idx = i + offset;
                                    if idx >= this.digit_count {
                                        break;
                                    }
                                    this.inputs[idx].update(cx, |input, cx| {
                                        input.set_value(ch.to_string(), window, cx);
                                    });
                                }
                                let next = end;
                                if next < this.digit_count {
                                    this.inputs[next].update(cx, |input, cx| {
                                        input.focus(window, cx);
                                    });
                                }
                                if (end >= this.digit_count || this.all_filled(cx))
                                    && let Some(ref cb) = this.on_complete
                                {
                                    let code: String = this
                                        .inputs
                                        .iter()
                                        .map(|input| input.read(cx).value().to_string())
                                        .collect();
                                    cb(code, window, cx);
                                }
                            }

                            this._suppressing_change = false;
                        }
                    }
                })
            })
            .collect();

        Self {
            digit_count,
            inputs,
            on_complete: None,
            _suppressing_change: false,
            _subscriptions,
        }
    }

    fn all_filled(&self, cx: &App) -> bool {
        self.inputs
            .iter()
            .all(|input| !input.read(cx).value().is_empty())
    }

    pub fn on_complete(mut self, handler: OtpCompleteHandler) -> Self {
        self.on_complete = Some(handler);
        self
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in &self.inputs {
            input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
        }
        if let Some(first) = self.inputs.first() {
            first.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }
    }

    pub fn value(&self, cx: &App) -> String {
        self.inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect()
    }
}

impl Render for OtpInput {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        div()
            .flex()
            .flex_row()
            .gap_2()
            .justify_center()
            .children(self.inputs.iter().map(|input| {
                div()
                    .w(gpui::px(44.0))
                    .bg(theme.bg_primary)
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(Input::new(input))
            }))
    }
}
