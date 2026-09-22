use gpui::{AnyElement, App, Hsla, Pixels, SharedString, Window, div, img, prelude::*, px};

use super::sizing::{Sizable, Size};
use crate::theme::ActiveTheme;

#[derive(IntoElement)]
pub struct Avatar {
    src: Option<SharedString>,
    name: Option<SharedString>,
    size: Size,
    custom_size: Option<Pixels>,
    border_color: Option<Hsla>,
    indicator: Option<AnyElement>,
    grayscale: bool,
    fallback_src: Option<SharedString>,
    image_cache: Option<gpui::Entity<crate::image_cache::LruImageCache>>,
    is_anonymous: bool,
    group_default: bool,
}

const ANONYMOUS_AVATAR_ICON: &str = "icons/anonymous-avatar.svg";

impl Avatar {
    pub fn new() -> Self {
        Self {
            src: None,
            name: None,
            size: Size::Medium,
            custom_size: None,
            border_color: None,
            indicator: None,
            grayscale: false,
            fallback_src: None,
            image_cache: None,
            is_anonymous: false,
            group_default: false,
        }
    }

    pub fn image_cache(mut self, cache: gpui::Entity<crate::image_cache::LruImageCache>) -> Self {
        self.image_cache = Some(cache);
        self
    }

    /// Override the avatar diameter with an explicit pixel size (e.g. 32px DM rows).
    pub fn size_px(mut self, size: Pixels) -> Self {
        self.custom_size = Some(size);
        self
    }

    pub fn grayscale(mut self, grayscale: bool) -> Self {
        self.grayscale = grayscale;
        self
    }

    pub fn anonymous(mut self, is_anonymous: bool) -> Self {
        self.is_anonymous = is_anonymous;
        self
    }

    pub fn group_default(mut self, group_default: bool) -> Self {
        self.group_default = group_default;
        self
    }

    pub fn src(mut self, src: impl Into<SharedString>) -> Self {
        self.src = Some(src.into());
        self
    }

    pub fn fallback_src(mut self, src: impl Into<SharedString>) -> Self {
        self.fallback_src = Some(src.into());
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn border_color(mut self, color: impl Into<Hsla>) -> Self {
        self.border_color = Some(color.into());
        self
    }

    pub fn indicator<E: IntoElement>(mut self, indicator: impl Into<Option<E>>) -> Self {
        self.indicator = indicator.into().map(IntoElement::into_any_element);
        self
    }
}

impl Default for Avatar {
    fn default() -> Self {
        Self::new()
    }
}

impl Sizable for Avatar {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

fn diameter(size: Size) -> Pixels {
    match size {
        Size::XSmall => px(20.),
        Size::Small => px(40.),
        Size::Medium => px(48.),
        Size::Large => px(80.),
    }
}

fn style_initials_tile<E>(
    el: E,
    size: Pixels,
    corner_radius: Option<Pixels>,
    bg: Hsla,
    text_color: Hsla,
    initials: SharedString,
) -> E
where
    E: Styled + ParentElement,
{
    let el = el
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(size)
        .bg(bg)
        .text_color(text_color)
        .text_size(size * 0.4)
        .child(initials);
    match corner_radius {
        Some(radius) => el.rounded(radius),
        None => el.rounded_full(),
    }
}

pub(crate) fn initials_tile(
    size: Pixels,
    corner_radius: Option<Pixels>,
    bg: Hsla,
    text_color: Hsla,
    initials: impl Into<SharedString>,
) -> AnyElement {
    style_initials_tile(div(), size, corner_radius, bg, text_color, initials.into())
        .into_any_element()
}

pub(crate) fn initials_tile_identified(
    size: Pixels,
    corner_radius: Option<Pixels>,
    bg: Hsla,
    text_color: Hsla,
    initials: impl Into<SharedString>,
    id: SharedString,
    hover_bg: Option<Hsla>,
) -> AnyElement {
    let el = style_initials_tile(
        div().id(id),
        size,
        corner_radius,
        bg,
        text_color,
        initials.into(),
    );
    match hover_bg {
        Some(hover_bg) => el
            .hover(move |s| s.bg(hover_bg).text_color(Hsla::white()))
            .into_any_element(),
        None => el.into_any_element(),
    }
}

pub(crate) fn clipped_initials_tile(
    size: Pixels,
    corner_radius: Pixels,
    src: SharedString,
    grayscale: bool,
    element_bg: Hsla,
    name: &str,
) -> AnyElement {
    if name.is_empty() {
        return div().size(size).into_any_element();
    }
    let mut bg = avatar_color(name);
    if grayscale {
        bg = bg.grayscale();
    }
    let text_color = avatar_text_color(bg);
    let initials = name_initials(name);
    let loading_initials = initials.clone();
    let fallback_initials = initials;
    let proxied = src.clone();
    div()
        .size(size)
        .rounded(corner_radius)
        .overflow_hidden()
        .child(
            img(src)
                .id(("initials-tile-image", gpui::hash(&proxied)))
                .size(size)
                .rounded(corner_radius)
                .object_fit(gpui::ObjectFit::Cover)
                .grayscale(grayscale)
                .bg(element_bg)
                .with_loading(move || {
                    initials_tile(
                        size,
                        Some(corner_radius),
                        bg,
                        text_color,
                        loading_initials.clone(),
                    )
                })
                .with_fallback(move || {
                    initials_tile(
                        size,
                        Some(corner_radius),
                        bg,
                        text_color,
                        fallback_initials.clone(),
                    )
                }),
        )
        .into_any_element()
}

fn initials_circle(d: Pixels, bg: Hsla, text_color: Hsla, initials: String) -> AnyElement {
    initials_tile(d, None, bg, text_color, initials)
}

fn anonymous_circle(d: Pixels) -> AnyElement {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(d)
        .rounded_full()
        .bg(Hsla::from(gpui::rgb(0xffffff)))
        .child(img(ANONYMOUS_AVATAR_ICON).size(d * 0.8))
        .into_any_element()
}

fn group_default_circle(d: Pixels) -> AnyElement {
    div()
        .size(d)
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .child(
            img(crate::util::assets::AVATAR_GROUP)
                .size(d)
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover),
        )
        .into_any_element()
}

fn clipped_image(
    size: Pixels,
    src: SharedString,
    grayscale: bool,
    element_bg: Hsla,
    loading: impl Fn() -> AnyElement + 'static,
    on_error: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    div()
        .size(size)
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .child(
            img(src.clone())
                .id(("avatar-image", gpui::hash(&src)))
                .size(size)
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .grayscale(grayscale)
                .bg(element_bg)
                .with_loading(loading)
                .with_fallback(on_error),
        )
        .into_any_element()
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border_width = if self.border_color.is_some() {
            px(1.)
        } else {
            px(0.)
        };

        let image_size = self.custom_size.unwrap_or_else(|| diameter(self.size));
        let container_size = image_size + border_width * 2.;

        let avatar_cache = self.image_cache.clone().unwrap_or_else(|| {
            if image_size <= px(40.) {
                crate::image_cache::shared_small_avatar_cache(cx)
            } else {
                crate::image_cache::shared_avatar_cache(cx)
            }
        });

        let name = self.name.clone().unwrap_or_default();
        let bg = avatar_color(name.as_ref());
        let text_color = avatar_text_color(bg);
        let element_bg = Hsla::from(cx.theme().bg_tertiary);
        let initials = name_initials(name.as_ref());
        let is_anonymous = self.is_anonymous;
        let group_default = self.group_default;

        div()
            .size(container_size)
            .flex_shrink_0()
            .rounded_full()
            .when_some(self.border_color, |this, color| {
                this.border(border_width).border_color(color)
            })
            .image_cache(avatar_cache)
            .child(match self.src {
                _ if is_anonymous => anonymous_circle(image_size),
                Some(src) => {
                    let loading_initials = initials.clone();
                    let fallback_initials = initials.clone();
                    let raw_fallback = self.fallback_src.clone();
                    let proxied = src.clone();
                    let grayscale = self.grayscale;
                    clipped_image(
                        image_size,
                        src,
                        grayscale,
                        element_bg,
                        move || {
                            initials_circle(image_size, bg, text_color, loading_initials.clone())
                        },
                        move || {
                            if let Some(raw) = raw_fallback.clone()
                                && !raw.is_empty()
                                && raw.as_ref() != proxied.as_ref()
                            {
                                return clipped_image(
                                    image_size,
                                    raw,
                                    grayscale,
                                    element_bg,
                                    {
                                        let retry_loading = fallback_initials.clone();
                                        move || {
                                            initials_circle(
                                                image_size,
                                                bg,
                                                text_color,
                                                retry_loading.clone(),
                                            )
                                        }
                                    },
                                    {
                                        let final_initials = fallback_initials.clone();
                                        move || {
                                            initials_circle(
                                                image_size,
                                                bg,
                                                text_color,
                                                final_initials.clone(),
                                            )
                                        }
                                    },
                                );
                            }
                            initials_circle(image_size, bg, text_color, fallback_initials.clone())
                        },
                    )
                }
                None if group_default => group_default_circle(image_size),
                None => initials_circle(image_size, bg, text_color, initials),
            })
            .children(self.indicator.map(|indicator| div().child(indicator)))
    }
}

/// Avatar fallback palette, 1:1 with mezon-react `avatarColors`. The background is picked by
/// `firstChar.charCodeAt(0) % 7`, matching the web client exactly.
const AVATAR_COLORS: [u32; 7] = [
    0xade603, 0x00b2cc, 0xfda63c, 0xe16dcc, 0xe8467b, 0x9c7cfd, 0x22e2b3,
];

fn first_upper_char(name: &str) -> Option<char> {
    name.chars().next().and_then(|c| c.to_uppercase().next())
}

pub(crate) fn avatar_color(name: &str) -> Hsla {
    let code = first_upper_char(name).map(|c| c as u32).unwrap_or(0);
    Hsla::from(gpui::rgb(AVATAR_COLORS[(code % 7) as usize]))
}

fn relative_luminance(r: f32, g: f32, b: f32) -> f32 {
    fn channel(c: f32) -> f32 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

pub(crate) fn avatar_text_color(bg: Hsla) -> Hsla {
    let rgba = bg.to_rgb();
    if relative_luminance(rgba.r, rgba.g, rgba.b) > 0.55 {
        Hsla::from(gpui::rgb(0x1f2937))
    } else {
        Hsla::white()
    }
}

pub(crate) fn name_initials(name: &str) -> String {
    first_upper_char(name)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string())
}
