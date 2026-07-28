use gpui::{
    AnyElement, Entity, Hsla, ObjectFit, Pixels, Rgba, SharedString, div, img, prelude::*, px,
};

use crate::components::primitives::{Icon, IconName};
use crate::image_cache::LruImageCache;

pub fn render_media_image(
    bg: Rgba,
    muted: Rgba,
    url: SharedString,
    width: Pixels,
    height: Pixels,
) -> impl IntoElement {
    img(url)
        .w(width)
        .h(height)
        .object_fit(ObjectFit::Cover)
        .bg(Hsla::from(bg))
        .with_loading(move || image_placeholder(bg, muted, width, height))
        .with_fallback(move || image_placeholder(bg, muted, width, height))
}

pub fn render_media_image_cover(
    bg: Rgba,
    muted: Rgba,
    url: SharedString,
    height: Pixels,
    image_cache: Entity<LruImageCache>,
) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(height)
        .flex_none()
        .overflow_hidden()
        .bg(Hsla::from(bg))
        .child(
            img(url)
                .image_cache(&image_cache)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_loading(move || image_placeholder_fill(bg, muted))
                .with_fallback(move || image_placeholder_fill(bg, muted)),
        )
}

fn image_placeholder_fill(bg: Rgba, muted: Rgba) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bg)
        .child(
            Icon::new(IconName::ImageThumbnail)
                .size(px(28.))
                .text_color(muted),
        )
        .into_any()
}

pub fn image_placeholder(bg: Rgba, muted: Rgba, width: Pixels, height: Pixels) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(width)
        .h(height)
        .bg(bg)
        .child(
            Icon::new(IconName::ImageThumbnail)
                .size(px(28.))
                .text_color(muted),
        )
        .into_any()
}
