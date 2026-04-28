/// UserChip — Avatar + username label inline.
use gpui::{div, prelude::*, Context, Window};

use crate::components::primitives::{Avatar, AvatarSize, Label, LabelSize, PresenceStatus};
use crate::theme::Theme;

pub struct UserChip {
    username: String,
    avatar_url: Option<String>,
    presence: Option<PresenceStatus>,
    size: AvatarSize,
}

impl UserChip {
    pub fn new(username: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            avatar_url: None,
            presence: None,
            size: AvatarSize::Sm,
        }
    }

    pub fn avatar_url(mut self, url: impl Into<String>) -> Self {
        self.avatar_url = Some(url.into());
        self
    }

    pub fn presence(mut self, status: PresenceStatus) -> Self {
        self.presence = Some(status);
        self
    }

    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }
}

impl Render for UserChip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();

        let initials = self
            .username
            .chars()
            .next()
            .unwrap_or('?')
            .to_string()
            .to_uppercase();

        let mut avatar = Avatar::new(&initials).size(self.size);
        if let Some(url) = &self.avatar_url {
            avatar = avatar.url(url);
        }
        if let Some(p) = self.presence {
            avatar = avatar.presence(p);
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(avatar.render(&theme))
            .child(
                Label::new(self.username.clone())
                    .size(LabelSize::Sm)
                    .render(&theme),
            )
    }
}
