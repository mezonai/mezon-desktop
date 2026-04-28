/// Badge — small count pill for unread/mention indicators.
use gpui::{div, prelude::*, Rgba};

use crate::theme::Theme;

pub struct Badge {
    count: u32,
    mention: bool,
    color: Option<Rgba>,
}

impl Badge {
    pub fn new(count: u32) -> Self {
        Self {
            count,
            mention: false,
            color: None,
        }
    }

    /// Show as a mention badge (red background).
    pub fn mention(mut self) -> Self {
        self.mention = true;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        if self.count == 0 {
            return div().into_any_element();
        }

        let bg = self.color.unwrap_or(if self.mention {
            theme.mention_badge
        } else {
            theme.brand
        });

        let label = if self.count > 99 {
            "99+".to_string()
        } else {
            self.count.to_string()
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .min_w_4()
            .h_4()
            .px_1()
            .rounded_full()
            .bg(bg)
            .text_color(gpui::white())
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .child(label)
            .into_any_element()
    }
}
