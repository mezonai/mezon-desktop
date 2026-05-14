use gpui::{div, prelude::*, px};
use mezon_store::ChannelType;

use crate::components::primitives::Icon;
use crate::components::primitives::IconName;
use crate::theme::Theme;

pub struct ChannelRow {
    name: String,
    channel_type: ChannelType,
    unread: bool,
    private: bool,
    selected: bool,
}

impl ChannelRow {
    pub fn new(name: impl Into<String>, channel_type: ChannelType) -> Self {
        Self {
            name: name.into(),
            channel_type,
            unread: false,
            private: false,
            selected: false,
        }
    }

    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }

    pub fn private(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn render(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .px_2()
            .py_1()
            .cursor_pointer()
            .when(self.selected, |el| {
                el.bg(theme.bg_primary).text_color(theme.text_primary)
            })
            .when(!self.selected, |el| el.text_color(theme.text_secondary))
            .child(match self.channel_type {
                ChannelType::Text => div().size_4().child("#").into_any_element(),
                ChannelType::Voice => Icon::new(IconName::Speaker)
                    .size(px(16.0))
                    .text_color(theme.text_secondary)
                    .into_any_element(),
            })
            .child(div().flex_1().mx_1().text_sm().child(self.name.clone()))
            .when(self.unread, |el| {
                el.child(div().size_2().rounded_full().bg(theme.brand))
            })
            .when(self.private, |el| el.child(div().size_3().child("🔒")))
    }
}
