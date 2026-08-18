use gpui::{AnyElement, ElementId, Pixels, SharedString, div, prelude::*, px};
use mezon_store::{DirectKind, DmAvatarPresence};

use crate::components::primitives::{Avatar, Icon, IconName};
use crate::router::{Route, navigate};
use crate::theme::Theme;

pub const DM_ROW_HEIGHT: f32 = 42.;

const DM_AVATAR_SIZE: Pixels = px(32.);

pub struct DmRow {
    id: SharedString,
    label: SharedString,
    kind: DirectKind,
    selected: bool,
    unread: bool,
    presence_badge: DmAvatarPresence,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    elem_id: ElementId,
    group_name: SharedString,
    close_id: SharedString,
    suppress_hover: bool,
    in_voice_label: Option<SharedString>,
    image_cache: Option<gpui::Entity<crate::image_cache::LruImageCache>>,
}

impl DmRow {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: DirectKind,
    ) -> Self {
        let id: SharedString = id.into();
        let elem_id: ElementId = SharedString::from(format!("dm-{}", id)).into();
        let group_name: SharedString = SharedString::from(format!("dm-row-{}", id));
        let close_id: SharedString = SharedString::from(format!("dm-close-{}", id));
        Self::with_ids(id, label, kind, elem_id, group_name, close_id)
    }

    pub fn with_ids(
        id: SharedString,
        label: impl Into<SharedString>,
        kind: DirectKind,
        elem_id: ElementId,
        group_name: SharedString,
        close_id: SharedString,
    ) -> Self {
        let label: SharedString = label.into();
        Self {
            id,
            label,
            kind,
            selected: false,
            unread: false,
            presence_badge: DmAvatarPresence::None,
            avatar_src: SharedString::from(""),
            avatar_raw: SharedString::from(""),
            elem_id,
            group_name,
            close_id,
            suppress_hover: false,
            in_voice_label: None,
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

    pub fn presence_badge(mut self, badge: DmAvatarPresence) -> Self {
        self.presence_badge = badge;
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

    pub fn in_voice_label(mut self, label: SharedString) -> Self {
        self.in_voice_label = Some(label);
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
            .h(px(DM_ROW_HEIGHT - 1.))
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
            .child({
                let name_el = div()
                    .text_base()
                    .text_color(name_color)
                    .truncate()
                    .child(self.label.clone());
                match self.in_voice_label.clone() {
                    Some(label) => div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(2.))
                        .child(name_el.line_height(px(16.)))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(2.))
                                .h(px(16.))
                                .opacity(0.6)
                                .child(
                                    Icon::new(IconName::Speaker)
                                        .size(px(10.))
                                        .text_color(gpui::rgb(0x22c55e)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(label),
                                ),
                        )
                        .into_any_element(),
                    None => name_el.flex_1().min_w_0().into_any_element(),
                }
            })
            .child(close_btn)
    }

    fn render_avatar(&self, theme: &Theme) -> AnyElement {
        let size = DM_AVATAR_SIZE;

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
            .when_some(self.render_presence_badge(theme), |this, badge| {
                this.child(badge)
            })
            .into_any_element()
    }

    fn render_presence_badge(&self, theme: &Theme) -> Option<AnyElement> {
        crate::util::user_status::presence_badge_element(
            self.presence_badge,
            theme.bg_secondary,
            theme,
        )
    }
}
