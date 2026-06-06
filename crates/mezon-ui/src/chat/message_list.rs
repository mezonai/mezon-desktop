use chrono::DateTime;
use gpui::{div, prelude::*, px};

use mezon_store::Message;

use crate::chat::grouping::{MessageGroup, group_messages};
use crate::chat::message_row::MessageRow;
use crate::theme::Theme;

pub struct MessageList {
    messages: Vec<Message>,
    theme: Theme,
    current_user_id: String,
    typing_users: Vec<String>,
    show_unread_break: bool,
}

impl MessageList {
    pub fn new(messages: Vec<Message>, theme: &Theme, current_user_id: &str) -> Self {
        Self {
            messages,
            theme: theme.clone(),
            current_user_id: current_user_id.to_string(),
            typing_users: Vec::new(),
            show_unread_break: false,
        }
    }

    pub fn with_typing_users(mut self, users: Vec<String>) -> Self {
        self.typing_users = users;
        self
    }

    pub fn with_unread_break(mut self, show: bool) -> Self {
        self.show_unread_break = show;
        self
    }

    fn format_date(timestamp: i64) -> String {
        DateTime::from_timestamp(timestamp, 0)
            .map(|dt| dt.format("%B %d, %Y").to_string())
            .unwrap_or_default()
    }

    fn date_separator(theme: &Theme, label: &str) -> impl IntoElement {
        div()
            .id("date-separator")
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .w_full()
            .child(div().flex_1().h(px(1.)).bg(gpui::hsla(0., 0., 0., 0.08)))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(label.to_string()),
            )
            .child(div().flex_1().h(px(1.)).bg(gpui::hsla(0., 0., 0., 0.08)))
    }

    fn unread_break(_theme: &Theme) -> impl IntoElement {
        div()
            .id("unread-break")
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .w_full()
            .child(div().flex_1().h(px(1.)).bg(gpui::hsla(0., 0., 0., 0.08)))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(gpui::red())
                    .child("NEW MESSAGES"),
            )
            .child(div().flex_1().h(px(1.)).bg(gpui::red().alpha(0.3)))
    }

    fn typing_indicator(theme: &Theme, users: &[String]) -> impl IntoElement {
        let label = if users.is_empty() {
            "Someone is typing...".to_string()
        } else if users.len() == 1 {
            format!("{} is typing...", users.join(", "))
        } else {
            format!("{} are typing...", users.join(", "))
        };

        div()
            .id("typing-indicator")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .child(
                div()
                    .w(px(3.))
                    .h(px(3.))
                    .rounded_full()
                    .bg(theme.text_muted),
            )
            .child(
                div()
                    .w(px(3.))
                    .h(px(3.))
                    .rounded_full()
                    .bg(theme.text_muted),
            )
            .child(
                div()
                    .w(px(3.))
                    .h(px(3.))
                    .rounded_full()
                    .bg(theme.text_muted),
            )
            .child(div().text_xs().text_color(theme.text_muted).child(label))
    }

    fn render_group(
        group: &MessageGroup,
        theme: &Theme,
        current_user_id: &str,
    ) -> impl IntoElement {
        let mut children: Vec<gpui::AnyElement> = Vec::new();

        for (i, msg) in group.messages.iter().enumerate() {
            let is_combined = group.combined[i];
            let row = MessageRow::new((*msg).clone(), theme, current_user_id).combined(is_combined);
            let rendered = row.render();
            children.push(rendered.into_any_element());
        }

        div().flex().flex_col().w_full().children(children)
    }

    pub fn render(&self) -> impl IntoElement {
        let groups = group_messages(&self.messages);
        let theme = &self.theme;

        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut current_date_label: Option<String> = None;

        for (i, group) in groups.iter().enumerate() {
            let day_label = group
                .messages
                .first()
                .map(|m| Self::format_date(m.create_time));

            if day_label != current_date_label {
                if let Some(ref label) = day_label {
                    children.push(Self::date_separator(theme, label).into_any_element());
                }
                current_date_label = day_label;
            }
            children
                .push(Self::render_group(group, theme, &self.current_user_id).into_any_element());

            if self.show_unread_break && i == 2 {
                children.push(Self::unread_break(theme).into_any_element());
            }
        }

        if !self.typing_users.is_empty() {
            children.push(Self::typing_indicator(theme, &self.typing_users).into_any_element());
        }

        div()
            .id("messages-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .id("messages-wrap")
                    .flex()
                    .flex_col()
                    .min_h_full()
                    .mt_auto()
                    .justify_end()
                    .children(children),
            )
    }
}
