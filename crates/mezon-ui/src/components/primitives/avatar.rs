/// Avatar — round image or initials fallback with optional presence dot.
use gpui::{Rgba, div, img, prelude::*};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarSize {
    Xs,  // 24px
    Sm,  // 32px
    Md,  // 40px (default)
    Lg,  // 48px
    Xl,  // 64px
    Xl2, // 80px
}

impl AvatarSize {
    fn px(self) -> f32 {
        match self {
            AvatarSize::Xs => 24.0,
            AvatarSize::Sm => 32.0,
            AvatarSize::Md => 40.0,
            AvatarSize::Lg => 48.0,
            AvatarSize::Xl => 64.0,
            AvatarSize::Xl2 => 80.0,
        }
    }

    fn text_px(self) -> f32 {
        match self {
            AvatarSize::Xs => 10.0,
            AvatarSize::Sm => 13.0,
            AvatarSize::Md => 16.0,
            AvatarSize::Lg => 19.0,
            AvatarSize::Xl => 24.0,
            AvatarSize::Xl2 => 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceStatus {
    Online,
    Idle,
    Dnd,
    Offline,
}

pub struct Avatar {
    url: Option<String>,
    initials: String,
    size: AvatarSize,
    presence: Option<PresenceStatus>,
    bg_color: Option<Rgba>,
}

impl Avatar {
    pub fn new(initials: impl Into<String>) -> Self {
        Self {
            url: None,
            initials: initials.into(),
            size: AvatarSize::Md,
            presence: None,
            bg_color: None,
        }
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }

    pub fn presence(mut self, status: PresenceStatus) -> Self {
        self.presence = Some(status);
        self
    }

    pub fn bg_color(mut self, color: Rgba) -> Self {
        self.bg_color = Some(color);
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let sz = gpui::px(self.size.px());
        let dot_sz = gpui::px((self.size.px() * 0.32).max(10.0));
        let dot_border = gpui::px(2.0);

        let presence_dot = self.presence.map(|status| {
            let dot_color = match status {
                PresenceStatus::Online => theme.status_online,
                PresenceStatus::Idle => theme.status_idle,
                PresenceStatus::Dnd => theme.status_dnd,
                PresenceStatus::Offline => theme.status_offline,
            };
            div()
                .absolute()
                .bottom(gpui::px(0.0))
                .right(gpui::px(0.0))
                .size(dot_sz)
                .rounded_full()
                .bg(dot_color)
                .border(dot_border)
                .border_color(theme.bg_secondary)
        });

        let avatar_bg = self.bg_color.unwrap_or(theme.brand);

        let avatar_inner = if let Some(url) = self.url {
            img(url)
                .size(sz)
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .into_any_element()
        } else {
            div()
                .size(sz)
                .rounded_full()
                .bg(avatar_bg)
                .flex()
                .items_center()
                .justify_center()
                .text_color(gpui::white())
                .text_size(gpui::px(self.size.text_px()))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(self.initials.chars().next().unwrap_or('?').to_string())
                .into_any_element()
        };

        let mut container = div()
            .relative()
            .size(sz)
            .flex_shrink_0()
            .child(avatar_inner);

        if let Some(dot) = presence_dot {
            container = container.child(dot);
        }

        container
    }
}
