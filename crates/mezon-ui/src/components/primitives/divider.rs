/// Divider — horizontal rule with optional centred label.
use gpui::{div, prelude::*, Rgba};

use crate::theme::Theme;

pub struct Divider {
    label: Option<String>,
    color: Option<Rgba>,
}

impl Divider {
    pub fn new() -> Self {
        Self {
            label: None,
            color: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let line_color = self.color.unwrap_or(theme.border);

        if let Some(label) = self.label {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w_full()
                .child(div().flex_1().h(gpui::px(1.0)).bg(line_color))
                .child(div().text_xs().text_color(theme.text_muted).child(label))
                .child(div().flex_1().h(gpui::px(1.0)).bg(line_color))
        } else {
            div().w_full().h(gpui::px(1.0)).bg(line_color)
        }
    }
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}
