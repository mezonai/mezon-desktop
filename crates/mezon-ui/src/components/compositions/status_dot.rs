/// StatusDot — small presence indicator dot.
use gpui::{div, prelude::*, Context, Window};

use crate::components::primitives::PresenceStatus;
use crate::theme::Theme;

pub struct StatusDot {
    status: PresenceStatus,
    size: f32,
}

impl StatusDot {
    pub fn new(status: PresenceStatus) -> Self {
        Self { status, size: 10.0 }
    }

    pub fn size(mut self, px: f32) -> Self {
        self.size = px;
        self
    }
}

impl Render for StatusDot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let color = match self.status {
            PresenceStatus::Online => theme.status_online,
            PresenceStatus::Idle => theme.status_idle,
            PresenceStatus::Dnd => theme.status_dnd,
            PresenceStatus::Offline => theme.status_offline,
        };
        let sz = gpui::px(self.size);
        div()
            .size(sz)
            .rounded_full()
            .bg(color)
            .border_2()
            .border_color(theme.bg_secondary)
    }
}
