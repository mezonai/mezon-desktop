use gpui::{div, prelude::*, px};
use gpui_component::Sizable;
use mezon_store::Message;

use crate::components::primitives::{Avatar, Size};
use crate::theme::Theme;

pub struct MessageRow {
    message: Message,
    combined: bool,
    reply: bool,
    theme: Theme,
}

impl MessageRow {
    pub fn new(message: Message, theme: &Theme, _current_user_id: &str) -> Self {
        Self {
            message,
            combined: false,
            reply: false,
            theme: theme.clone(),
        }
    }

    pub fn combined(mut self, combined: bool) -> Self {
        self.combined = combined;
        self
    }

    pub fn reply(mut self, reply: bool) -> Self {
        self.reply = reply;
        self
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

    pub fn render(&self) -> impl IntoElement {
        let msg = &self.message;
        let theme = &self.theme;
        let time = Self::format_timestamp(msg.create_time);

        let display_name = &msg.sender_name;

        let avatar = Avatar::new().name(display_name).with_size(Size::Small);

        let name_row = div()
            .flex()
            .flex_row()
            .items_baseline()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(display_name.to_string()),
            )
            .child(div().text_xs().text_color(theme.text_muted).child(time));

        let content = div()
            .text_sm()
            .text_color(theme.text_secondary)
            .child(msg.content.clone());

        let reply_placeholder = div()
            .id("reply-placeholder")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .mb_1()
            .child(
                div()
                    .w(px(2.))
                    .h_full()
                    .min_h(px(16.))
                    .rounded(px(2.))
                    .bg(gpui::hsla(0., 0., 0., 0.15)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child("Replying to someone"),
            );

        let content_area = div()
            .flex()
            .flex_col()
            .when(!self.combined, |d| d.pl(px(42.)))
            .when(self.combined, |d| d.pl(px(10.)))
            .child(if self.reply {
                reply_placeholder.into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if !self.combined {
                name_row.into_any_element()
            } else {
                div().into_any_element()
            })
            .child(content)
            .when(!msg.reactions.is_empty(), |d| {
                d.child(
                    div()
                        .id("reactions-placeholder")
                        .flex()
                        .flex_row()
                        .gap_1()
                        .mt_1()
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(gpui::hsla(0., 0., 0., 0.05))
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("[👍 ❤️]"),
                        ),
                )
            })
            .when(!msg.attachments.is_empty(), |d| {
                d.child(
                    div()
                        .id("attachments-placeholder")
                        .flex()
                        .flex_row()
                        .gap_1()
                        .mt_1()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(gpui::hsla(0., 0., 0., 0.03))
                                .border_1()
                                .border_color(theme.border)
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child("[attachment.png]"),
                        ),
                )
            });

        div()
            .flex()
            .flex_row()
            .w_full()
            .px_4()
            .py(px(2.))
            .when(!self.combined, |d| d.pt_3())
            .child(
                div()
                    .when(!self.combined, |d| d.absolute().left(px(16.)).top(px(10.)))
                    .when(self.combined, |d| d.invisible().w(px(32.)))
                    .child(avatar),
            )
            .child(content_area)
    }
}
