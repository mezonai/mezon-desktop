use std::sync::Arc;

use gpui::{App, Context, Entity, Window, div, prelude::*};
use gpui_component::input::InputState;
use mezon_store::Message;

use crate::chat::ReplyTarget;
use crate::chat::channel_header::ChannelHeader;
use crate::chat::input_bar::InputBar;
use crate::chat::message_list::MessageList;
use crate::theme::Theme;

pub struct ChatArea {
    pub(crate) messages: Vec<Message>,
    input_state: Option<Entity<InputState>>,
    #[allow(dead_code)]
    replying_to: Option<ReplyTarget>,
}

impl Default for ChatArea {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatArea {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            input_state: None,
            replying_to: None,
        }
    }

    pub fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<crate::ChatLayout>) {
        if self.input_state.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Message #general"));
            self.input_state = Some(input);
        }
    }

    pub fn render(
        &self,
        theme: &Theme,
        layout_entity: Entity<crate::ChatLayout>,
        channel_name: &str,
        current_user_id: &str,
    ) -> impl IntoElement {
        let handle = layout_entity.clone();
        let user_id = current_user_id.to_string();
        #[allow(clippy::type_complexity)]
        let on_send: Arc<dyn Fn(&str, &mut Window, &mut App) + Send + Sync> =
            Arc::new(move |value: &str, _window: &mut Window, cx: &mut App| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let uid = user_id.clone();
                let msg_content = value.to_string();
                handle.update(cx, |this, cx| {
                    this.chat_area.messages.push(Message::new(
                        format!("mock-{}", this.chat_area.messages.len() + 1),
                        msg_content,
                        uid.clone(),
                        uid,
                        now,
                    ));
                    cx.notify();
                });
            });

        let input_bar = InputBar::new()
            .with_input(self.input_state.clone().unwrap())
            .on_send(on_send);

        let header = ChannelHeader::new(channel_name);
        let message_list = MessageList::new(self.messages.clone(), theme, current_user_id);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header.render(theme))
            .child(message_list.render())
            .child(input_bar.render(theme))
    }
}
