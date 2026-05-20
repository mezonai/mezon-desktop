use gpui::{App, ClickEvent, Context, Entity, Window, div, prelude::*, px, relative};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Sizable, v_flex};
use mezon_store::Message;

use crate::components::primitives::{Avatar, Button, Size};
use crate::theme::Theme;

pub struct ChatArea {
    messages: Vec<Message>,
    input_state: Option<Entity<InputState>>,
}

impl Default for ChatArea {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatArea {
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            messages: vec![
                Message {
                    id: "1".into(),
                    content: "Hey team, how's the project going?".into(),
                    sender_id: "alice".into(),
                    sender_name: "Alice".into(),
                    create_time: now - 360,
                },
                Message {
                    id: "2".into(),
                    content: "Going well! Just finishing up the chat UI.".into(),
                    sender_id: "bob".into(),
                    sender_name: "Bob".into(),
                    create_time: now - 300,
                },
                Message {
                    id: "3".into(),
                    content: "I can take a look at the PR after lunch.".into(),
                    sender_id: "charlie".into(),
                    sender_name: "Charlie".into(),
                    create_time: now - 240,
                },
                Message {
                    id: "4".into(),
                    content: "Sounds good, no rush.".into(),
                    sender_id: "bob".into(),
                    sender_name: "Bob".into(),
                    create_time: now - 180,
                },
                Message {
                    id: "5".into(),
                    content: "Actually, can we sync quickly at 3?".into(),
                    sender_id: "alice".into(),
                    sender_name: "Alice".into(),
                    create_time: now - 120,
                },
            ],
            input_state: None,
        }
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.input_state.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Message #general"));
            self.input_state = Some(input);
        }
    }

    fn format_timestamp(ts: i64) -> String {
        let seconds_since_midnight = ts % 86400;
        let hours = seconds_since_midnight / 3600;
        let minutes = (seconds_since_midnight % 3600) / 60;

        let period = if hours >= 12 { "PM" } else { "AM" };
        let display_hour = if hours == 0 {
            12
        } else if hours > 12 {
            hours - 12
        } else {
            hours
        };
        format!("{}:{:02} {}", display_hour, minutes, period)
    }

    pub fn render(
        &self,
        theme: &Theme,
        layout_entity: Entity<crate::ChatLayout>,
    ) -> impl IntoElement {
        let messages = &self.messages;

        let message_list = v_flex()
            .id("v-scroll-container")
            .w_full()
            .h(relative(0.8))
            .overflow_y_scrollbar()
            .gap_3()
            .children(messages.iter().map(|msg| {
                let is_self = msg.sender_name == "You";
                let name = msg.sender_name.clone();
                let time = Self::format_timestamp(msg.create_time);
                let content = msg.content.clone();

                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .when(is_self, |d| d.flex_row_reverse().self_end())
                    .child(Avatar::new().name(name.clone()).with_size(Size::Small))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_baseline()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_primary)
                                            .child(name),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.text_muted).child(time),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_secondary)
                                    .child(content),
                            ),
                    )
            }));

        let handle = layout_entity.clone();
        let input = self.input_state.as_ref().unwrap().clone();
        let on_send = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let value = input.read(cx).value().to_string();
            if !value.is_empty() {
                input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                    cx.notify();
                });
                handle.update(cx, |this, cx| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    this.chat_area.messages.push(Message {
                        id: format!("mock-{}", this.chat_area.messages.len() + 1),
                        content: value,
                        sender_id: "current-user".into(),
                        sender_name: "You".into(),
                        create_time: now,
                    });
                    cx.notify();
                });
            }
        };

        let input_bar = div()
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
            .child(Button::new("send-btn").label("Send").on_click(on_send));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(message_list)
            .child(input_bar)
    }
}
