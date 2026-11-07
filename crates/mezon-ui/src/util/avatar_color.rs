//! Average-color extraction for avatar-tinted banners.
//!
//! React derives the banner tint from the avatar with `getColorAverageFromURL`.
//! GPUI has no equivalent hook, so the color is read back out of the decoded
//! image once the avatar the view already renders lands in the image cache.

use std::{path::PathBuf, time::Duration};

use gpui::{Context, Entity, Rgba, Task};

use crate::image_cache::LruImageCache;

/// Weighted RMS average of a decoded BGRA buffer, ignoring fully transparent
/// pixels. Squaring before averaging keeps the result perceptually closer to
/// the avatar than a plain channel mean.
pub fn average_bgra_color(bytes: &[u8]) -> Option<Rgba> {
    let mut red = 0f64;
    let mut green = 0f64;
    let mut blue = 0f64;
    let mut weight = 0f64;
    for pixel in bytes.chunks_exact(4) {
        let alpha = f64::from(pixel[3]) / 255.;
        if alpha == 0. {
            continue;
        }
        red += f64::from(pixel[2]).powi(2) * alpha;
        green += f64::from(pixel[1]).powi(2) * alpha;
        blue += f64::from(pixel[0]).powi(2) * alpha;
        weight += alpha;
    }
    (weight > 0.).then(|| Rgba {
        r: (red / weight).sqrt() as f32 / 255.,
        g: (green / weight).sqrt() as f32 / 255.,
        b: (blue / weight).sqrt() as f32 / 255.,
        a: 1.,
    })
}

/// Polls `avatar_image_cache` until the avatar decodes, then hands its average
/// color to `apply`.
///
/// `avatar_url` must be the exact resource the view renders, otherwise this
/// polls something the cache was never asked to load and gives up empty-handed.
///
/// A cold avatar can take well over the 2s a flat 40x50ms poll allowed, and
/// giving up left the banner on the default color with nothing to retry it.
/// Returns `None` for an empty url so callers can clear their stored task.
pub fn spawn_banner_color_task<T: 'static>(
    avatar_image_cache: Entity<LruImageCache>,
    avatar_url: String,
    cx: &mut Context<T>,
    apply: impl Fn(&mut T, Rgba, &mut Context<T>) + 'static,
) -> Option<Task<()>> {
    spawn_banner_color_task_with_fallback(avatar_image_cache, avatar_url, String::new(), cx, apply)
}

pub fn spawn_banner_color_task_with_fallback<T: 'static>(
    avatar_image_cache: Entity<LruImageCache>,
    avatar_url: String,
    fallback_url: String,
    cx: &mut Context<T>,
    apply: impl Fn(&mut T, Rgba, &mut Context<T>) + 'static,
) -> Option<Task<()>> {
    if avatar_url.is_empty() && fallback_url.is_empty() {
        return None;
    }
    let resources: Vec<_> = [avatar_url, fallback_url]
        .into_iter()
        .filter(|url| !url.is_empty())
        .map(|url| gpui::Resource::Uri(url.into()))
        .collect();
    Some(cx.spawn(async move |this, cx| {
        for attempt in 0..60 {
            let color = resources.iter().find_map(|resource| {
                let image = avatar_image_cache
                    .read_with(cx, |cache, _| cache.cached_render_image(resource));
                image.and_then(|image| image.as_bytes(0).and_then(average_bgra_color))
            });
            if let Some(color) = color {
                let _ = this.update(cx, |this, cx| apply(this, color, cx));
                break;
            }
            let delay_ms = if attempt < 20 { 50 } else { 200 };
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
        }
    }))
}

pub fn spawn_local_banner_color_task<T: 'static>(
    avatar_path: PathBuf,
    cx: &mut Context<T>,
    apply: impl Fn(&mut T, Rgba, &mut Context<T>) + 'static,
) -> Option<Task<()>> {
    let executor = cx.background_executor().clone();
    Some(cx.spawn(async move |this, cx| {
        let color = executor
            .spawn(async move {
                let rgba = image::open(avatar_path).ok()?.to_rgba8();
                let mut bgra = rgba.into_raw();
                for pixel in bgra.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                average_bgra_color(&bgra)
            })
            .await;
        if let Some(color) = color {
            let _ = this.update(cx, |this, cx| apply(this, color, cx));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::average_bgra_color;

    #[test]
    fn average_color_uses_avatar_pixels() {
        let pixels = [30, 60, 120, 255].repeat(4);
        let color = average_bgra_color(&pixels).expect("color image has an average");
        assert!((color.r - 120. / 255.).abs() < 0.001);
        assert!((color.g - 60. / 255.).abs() < 0.001);
        assert!((color.b - 30. / 255.).abs() < 0.001);
    }

    #[test]
    fn fully_transparent_pixels_have_no_average() {
        assert!(average_bgra_color(&[10, 20, 30, 0].repeat(4)).is_none());
    }
}
