use std::sync::Arc;

use gpui::{App, ClickEvent, Window, div, prelude::*, px};
use gpui_component::input::{Input, InputState};

use crate::chat::ReplyTarget;
use crate::components::primitives::Button;
use crate::theme::Theme;

type SendHandler = Arc<dyn Fn(&str, &mut Window, &mut App) + Send + Sync>;

pub struct InputBar {
    input_state: Option<gpui::Entity<InputState>>,
    on_send: Option<SendHandler>,
    replying_to: Option<ReplyTarget>,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            input_state: None,
            on_send: None,
            replying_to: None,
        }
    }

    pub fn with_input(mut self, state: gpui::Entity<InputState>) -> Self {
        self.input_state = Some(state);
        self
    }

    pub fn on_send(mut self, handler: SendHandler) -> Self {
        self.on_send = Some(handler);
        self
    }

    pub fn replying_to(mut self, target: Option<ReplyTarget>) -> Self {
        self.replying_to = target;
        self
    }

    fn reply_preview_bar(theme: &Theme, target: &ReplyTarget) -> impl IntoElement {
        div()
            .id("reply-preview-bar")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(gpui::hsla(0., 0., 0., 0.02))
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(3.))
                    .h_full()
                    .min_h(px(20.))
                    .rounded(px(2.))
                    .bg(gpui::hsla(0., 0., 0., 0.15)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(format!("Replying to {}", target.sender_name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(target.content_preview.clone()),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .child("×"),
            )
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        let input_state = self.input_state.clone();
        let on_send = self.on_send.clone();

        let on_click = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(ref state) = input_state {
                let value = state.read(cx).value().to_string();
                if !value.is_empty() && on_send.is_some() {
                    let handler = on_send.clone().unwrap();
                    handler(&value, window, cx);
                    state.update(cx, |s, cx| {
                        s.set_value("", window, cx);
                        cx.notify();
                    });
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .when_some(self.replying_to.as_ref(), |d, target| {
                d.child(Self::reply_preview_bar(theme, target))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.bg_primary)
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(self.input_state.as_ref().unwrap())),
                    )
                    .child(Button::new("send-btn").label("Send").on_click(on_click)),
            )
    }
}
