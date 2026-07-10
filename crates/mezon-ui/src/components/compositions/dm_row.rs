use gpui::{AnyElement, ElementId, SharedString, div, prelude::*, px};
use mezon_store::DirectKind;

use crate::components::primitives::Avatar;
use crate::router::{Route, navigate};
use crate::theme::Theme;

pub struct DmRow {
    id: SharedString,
    label: SharedString,
    kind: DirectKind,
    selected: bool,
    unread: bool,
    online: bool,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    elem_id: ElementId,
    group_name: SharedString,
    close_id: SharedString,
    suppress_hover: bool,
    image_cache: Option<gpui::Entity<crate::image_cache::LruImageCache>>,
}

impl DmRow {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: DirectKind,
    ) -> Self {
        let id: SharedString = id.into();
        let label: SharedString = label.into();
        let elem_id: ElementId = SharedString::from(format!("dm-{}", id)).into();
        let group_name: SharedString = SharedString::from(format!("dm-row-{}", id));
        let close_id: SharedString = SharedString::from(format!("dm-close-{}", id));
        Self {
            id,
            label,
            kind,
            selected: false,
            unread: false,
            online: false,
            avatar_src: SharedString::from(""),
            avatar_raw: SharedString::from(""),
            elem_id,
            group_name,
            close_id,
            suppress_hover: false,
            image_cache: None,
        }
    }

    pub fn image_cache(mut self, cache: gpui::Entity<crate::image_cache::LruImageCache>) -> Self {
        self.image_cache = Some(cache);
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn suppress_hover(mut self, suppress: bool) -> Self {
        self.suppress_hover = suppress;
        self
    }

    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }

    pub fn online(mut self, online: bool) -> Self {
        self.online = online;
        self
    }

    pub fn avatar_src(mut self, src: impl Into<SharedString>) -> Self {
        self.avatar_src = src.into();
        self
    }

    pub fn avatar_raw(mut self, raw: impl Into<SharedString>) -> Self {
        self.avatar_raw = raw.into();
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement {
        let nav_id = self.id.to_string();
        let channel_type = self.kind.channel_type();
        let bg_hover = theme.bg_hover;
        let muted = theme.text_muted;
        let selected = self.selected;
        let highlight = selected || self.unread;
        let name_color = if highlight {
            theme.text_primary
        } else {
            theme.tokens.text_theme_primary
        };

        let avatar_slot = self.render_avatar(theme);
        let suppress_hover = self.suppress_hover;

        let close_btn = div()
            .id(self.close_id.clone())
            .flex()
            .items_center()
            .justify_center()
            .size(px(20.))
            .text_size(px(24.))
            .opacity(0.)
            .text_color(muted)
            .cursor_pointer()
            .when(!suppress_hover, |this| {
                this.group_hover(self.group_name.clone(), |this| this.opacity(1.))
                    .hover(move |this| this.text_color(gpui::rgb(0xef4444)))
            })
            .on_click(|_, _window, cx| cx.stop_propagation())
            .child("×");

        div()
            .id(self.elem_id.clone())
            .group(self.group_name.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(42.))
            .w_full()
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |this| this.bg(bg_hover))
            .when(!suppress_hover, |this| {
                this.hover(move |this| this.bg(bg_hover))
            })
            .on_click(move |_, _window, cx| {
                navigate(
                    cx,
                    Route::DirectMessage {
                        direct_id: nav_id.parse().unwrap_or_default(),
                        message_type: channel_type.to_string(),
                    },
                );
            })
            .child(avatar_slot)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_base()
                    .text_color(name_color)
                    .truncate()
                    .child(self.label.clone()),
            )
            .child(close_btn)
    }

    fn render_avatar(&self, theme: &Theme) -> AnyElement {
        let size = px(32.);
        let online = self.online && self.kind == DirectKind::Dm;

        let inner: AnyElement = if self.kind == DirectKind::Group && self.avatar_src.is_empty() {
            div()
                .size(size)
                .flex_shrink_0()
                .rounded_full()
                .overflow_hidden()
                .child(
                    gpui::img(crate::util::assets::AVATAR_GROUP)
                        .size(size)
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            let mut avatar = Avatar::new().name(self.label.clone()).size_px(size);
            if let Some(cache) = self.image_cache.clone() {
                avatar = avatar.image_cache(cache);
            }
            let src = self.avatar_src.clone();
            let raw = self.avatar_raw.clone();
            if !src.is_empty() {
                let proxied = src.clone();
                avatar = avatar.src(src.to_string());
                if !raw.is_empty() && raw != proxied {
                    avatar = avatar.fallback_src(raw.to_string());
                }
            } else if !raw.is_empty() {
                avatar = avatar.src(raw.to_string());
            }
            avatar.into_any_element()
        };

        div()
            .relative()
            .flex_shrink_0()
            .size(size)
            .child(inner)
            .when(online, |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(-1.))
                        .right(px(-1.))
                        .size(px(10.))
                        .rounded_full()
                        .border_2()
                        .border_color(theme.bg_secondary)
                        .bg(theme.status_online),
                )
            })
            .into_any_element()
    }
}
