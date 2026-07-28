use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use futures::AsyncReadExt as _;
use gpui::{
    AnyElement, App, EntityId, Global, InteractiveElement, ObjectFit, Pixels, RenderImage,
    SharedString, Styled, div, img, prelude::*, px,
};
use smallvec::smallvec;

use mezon_store::AppConfig;
use mezon_store::config::url_has_origin;
use mezon_theme::Theme;
use mezon_widgets::{Icon, IconName};

pub const CANVAS_IMAGE_FALLBACK_HEIGHT: Pixels = px(200.);
const CANVAS_VIEW_IMAGE_MAX_WIDTH: Pixels = px(720.);

const IMAGE_DIMENSION_PROBE_MAX_BYTES: u64 = 24 * 1024 * 1024;
const CANVAS_IMAGE_DIM_CACHE_MAX: usize = 512;
const CANVAS_DATA_IMAGE_CACHE_MAX: usize = 64;
const CANVAS_DECODE_MAX_WIDTH: u32 = 16_384;
const CANVAS_DECODE_MAX_HEIGHT: u32 = 16_384;
const CANVAS_DECODE_MAX_PIXELS: u64 = 48_000_000;
const CANVAS_DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;

const STATIC_CANVAS_IMAGE_ORIGINS: [&str; 4] = [
    "https://cdn.mezon.ai",
    "http://cdn.mezon.ai",
    "https://profile.mezon.ai",
    "http://profile.mezon.ai",
];

#[derive(Clone, Copy)]
enum ImageDimState {
    Loading,
    Ready(u32, u32),
    Failed,
}

#[derive(Default)]
struct CanvasImageDimCache {
    entries: HashMap<String, ImageDimState>,
}
impl Global for CanvasImageDimCache {}

fn trim_dim_cache(entries: &mut HashMap<String, ImageDimState>) {
    if entries.len() <= CANVAS_IMAGE_DIM_CACHE_MAX {
        return;
    }
    entries.retain(|_, state| matches!(state, ImageDimState::Ready(_, _)));
    while entries.len() > CANVAS_IMAGE_DIM_CACHE_MAX {
        let Some(key) = entries.keys().next().cloned() else {
            break;
        };
        entries.remove(&key);
    }
}

pub fn reset_canvas_image_caches(cx: &mut App) {
    cx.default_global::<CanvasImageDimCache>().entries.clear();
    if let Some(cache) = CANVAS_DATA_IMAGE_CACHE.get()
        && let Ok(mut entries) = cache.lock()
    {
        entries.clear();
    }
}

pub fn canvas_image_known_size(cx: &App, src: &str) -> Option<(u32, u32)> {
    match cx.try_global::<CanvasImageDimCache>()?.entries.get(src)? {
        ImageDimState::Ready(width, height) => Some((*width, *height)),
        _ => None,
    }
}

pub fn remember_canvas_image_size(cx: &mut App, src: &str, width: u32, height: u32) {
    if src.is_empty() || width == 0 || height == 0 {
        return;
    }
    let cache = cx.default_global::<CanvasImageDimCache>();
    cache
        .entries
        .insert(src.to_string(), ImageDimState::Ready(width, height));
    trim_dim_cache(&mut cache.entries);
}

pub fn ensure_canvas_image_dimensions_loaded(cx: &mut App, src: &str, notify: EntityId) {
    if src.is_empty() || is_data_image_url(src) {
        return;
    }
    let url = canvas_image_display_src_with_app(cx, src);
    if url.is_empty() {
        return;
    }
    let cache = cx.default_global::<CanvasImageDimCache>();
    if cache.entries.contains_key(src) {
        return;
    }
    cache
        .entries
        .insert(src.to_string(), ImageDimState::Loading);
    let client = cx.http_client();
    let src_owned = src.to_string();
    cx.spawn(async move |cx| {
        let dims = fetch_remote_image_dimensions(client, url).await;
        cx.update(|cx| {
            let state = match dims {
                Some((width, height)) if width > 0 && height > 0 => {
                    ImageDimState::Ready(width, height)
                }
                _ => ImageDimState::Failed,
            };
            let cache = cx.default_global::<CanvasImageDimCache>();
            cache.entries.insert(src_owned, state);
            trim_dim_cache(&mut cache.entries);
            cx.notify(notify);
        });
    })
    .detach();
}

async fn fetch_remote_image_dimensions(
    client: Arc<dyn gpui::http_client::HttpClient>,
    url: String,
) -> Option<(u32, u32)> {
    let mut response = client.get(&url, Default::default(), false).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .take(IMAGE_DIMENSION_PROBE_MAX_BYTES)
        .read_to_end(&mut body)
        .await
        .ok()?;
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(body))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if !canvas_decode_dims_allowed(width, height) {
        return None;
    }
    Some((width, height))
}

fn canvas_decode_dims_allowed(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= CANVAS_DECODE_MAX_WIDTH
        && height <= CANVAS_DECODE_MAX_HEIGHT
        && (width as u64).saturating_mul(height as u64) <= CANVAS_DECODE_MAX_PIXELS
}

fn canvas_image_decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(CANVAS_DECODE_MAX_WIDTH);
    limits.max_image_height = Some(CANVAS_DECODE_MAX_HEIGHT);
    limits.max_alloc = Some(CANVAS_DECODE_MAX_ALLOC);
    limits
}

fn is_allowed_canvas_image_origin(src: &str, cx: Option<&App>) -> bool {
    if is_data_image_url(src) {
        return true;
    }
    if !(src.starts_with("https://") || src.starts_with("http://")) {
        return false;
    }
    if let Some(cx) = cx
        && let Some(cfg) = AppConfig::try_global(cx)
    {
        if cfg.is_own_media_origin(src) {
            return true;
        }
        let imgproxy = cfg.imgproxy_base_url.trim_end_matches('/');
        if !imgproxy.is_empty() && url_has_origin(src, imgproxy) {
            return true;
        }
    }
    STATIC_CANVAS_IMAGE_ORIGINS
        .iter()
        .any(|origin| url_has_origin(src, origin))
}

pub fn canvas_image_display_src(src: &str) -> String {
    canvas_image_display_src_checked(src, None)
}

pub fn canvas_image_display_src_with_app(cx: &App, src: &str) -> String {
    canvas_image_display_src_checked(src, Some(cx))
}

fn canvas_image_display_src_checked(src: &str, cx: Option<&App>) -> String {
    if src.is_empty() {
        return String::new();
    }
    if is_data_image_url(src) {
        return src.to_string();
    }
    if !is_allowed_canvas_image_origin(src, cx) {
        return String::new();
    }
    if url_has_origin(src, "http://cdn.mezon.ai") {
        return src.replacen("http://", "https://", 1);
    }
    if url_has_origin(src, "http://profile.mezon.ai") {
        return src.replacen("http://", "https://", 1);
    }
    src.to_string()
}

pub fn is_data_image_url(src: &str) -> bool {
    src.starts_with("data:image/")
}

fn decode_data_image(src: &str) -> Option<Vec<u8>> {
    let payload = src.strip_prefix("data:")?;
    let (_, data) = payload.split_once(',')?;
    if src.contains(";base64,") {
        base64::engine::general_purpose::STANDARD.decode(data).ok()
    } else {
        None
    }
}

fn image_pixel_size(src: &str) -> Option<(u32, u32)> {
    if is_data_image_url(src) {
        let bytes = decode_data_image(src)?;
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .ok()?;
        reader.limits(canvas_image_decode_limits());
        let (width, height) = reader.into_dimensions().ok()?;
        if !canvas_decode_dims_allowed(width, height) {
            return None;
        }
        return Some((width, height));
    }
    None
}

pub fn fit_image_to_max_width(width: u32, height: u32, max_width: Pixels) -> (Pixels, Pixels) {
    if width == 0 || height == 0 {
        return (max_width, CANVAS_IMAGE_FALLBACK_HEIGHT);
    }
    let max_w = max_width.as_f32().max(1.);
    let fw = width as f32;
    let fh = height as f32;
    if fw <= max_w {
        return (px(fw), px(fh));
    }
    let scale = max_w / fw;
    (px(max_w), px(fh * scale))
}

pub fn canvas_image_display_size(cx: &App, src: &str, max_width: Pixels) -> (Pixels, Pixels) {
    if let Some((w, h)) = image_pixel_size(src) {
        return fit_image_to_max_width(w, h, max_width);
    }
    if let Some((w, h)) = canvas_image_known_size(cx, src) {
        return fit_image_to_max_width(w, h, max_width);
    }
    let fallback_h = (max_width * 9. / 16.).max(CANVAS_IMAGE_FALLBACK_HEIGHT);
    (max_width, fallback_h)
}

static CANVAS_DATA_IMAGE_CACHE: OnceLock<Mutex<HashMap<String, Arc<RenderImage>>>> =
    OnceLock::new();

fn canvas_image_element_id(src: &str) -> SharedString {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    src.hash(&mut hasher);
    SharedString::from(format!("canvas-img-{:x}", hasher.finish()))
}

fn trim_data_image_cache(entries: &mut HashMap<String, Arc<RenderImage>>) {
    while entries.len() > CANVAS_DATA_IMAGE_CACHE_MAX {
        let Some(key) = entries.keys().next().cloned() else {
            break;
        };
        entries.remove(&key);
    }
}

fn canvas_data_render_image(src: &str) -> Option<Arc<RenderImage>> {
    let cache = CANVAS_DATA_IMAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut entries = cache.lock().ok()?;
    if let Some(cached) = entries.get(src) {
        return Some(cached.clone());
    }
    let bytes = decode_data_image(src)?;
    let render = bytes_to_render_image(&bytes)?;
    entries.insert(src.to_string(), render.clone());
    trim_data_image_cache(&mut entries);
    Some(render)
}

fn bytes_to_render_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(canvas_image_decode_limits());
    let (width, height) = reader.into_dimensions().ok()?;
    if !canvas_decode_dims_allowed(width, height) {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(canvas_image_decode_limits());
    let decoded = reader.decode().ok()?;
    let mut data = decoded.into_rgba8();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec![image::Frame::new(
        data
    )])))
}

fn canvas_image_fallback_inner(fallback_fg: gpui::Rgba, fallback_bg: gpui::Rgba) -> AnyElement {
    div()
        .max_w_full()
        .h(CANVAS_IMAGE_FALLBACK_HEIGHT)
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(fallback_bg)
        .child(
            Icon::new(IconName::ImageThumbnail)
                .size(px(32.))
                .text_color(fallback_fg),
        )
        .into_any_element()
}

pub fn canvas_img(
    cx: &App,
    src: &str,
    img_id: impl Into<gpui::ElementId>,
    fallback_fg: gpui::Rgba,
    fallback_bg: gpui::Rgba,
    max_width: Option<Pixels>,
    display_height: Option<Pixels>,
) -> AnyElement {
    let img_id = img_id.into();
    if let Some(max_w) = max_width {
        let (display_w, display_h) = canvas_image_display_size(cx, src, max_w);
        let height = display_height.unwrap_or(display_h);
        if is_data_image_url(src) {
            if let Some(render) = canvas_data_render_image(src) {
                return img(render)
                    .id(canvas_image_element_id(src))
                    .w(display_w)
                    .h(height)
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
                    .into_any_element();
            }
            return canvas_image_fallback_inner(fallback_fg, fallback_bg);
        }
        let display_src = canvas_image_display_src_with_app(cx, src);
        if display_src.is_empty() {
            return canvas_image_fallback_inner(fallback_fg, fallback_bg);
        }
        return img(SharedString::from(display_src))
            .id(img_id)
            .w(display_w)
            .h(height)
            .object_fit(ObjectFit::Contain)
            .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
            .into_any_element();
    }

    if is_data_image_url(src) {
        if let Some(render) = canvas_data_render_image(src) {
            return img(render)
                .id(canvas_image_element_id(src))
                .max_w_full()
                .object_fit(ObjectFit::Contain)
                .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
                .into_any_element();
        }
        return canvas_image_fallback_inner(fallback_fg, fallback_bg);
    }
    let display_src = canvas_image_display_src_with_app(cx, src);
    if display_src.is_empty() {
        return canvas_image_fallback_inner(fallback_fg, fallback_bg);
    }
    img(SharedString::from(display_src))
        .id(img_id)
        .max_w_full()
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
        .into_any_element()
}

pub fn render_canvas_image(src: &str, theme: &Theme, cx: &App) -> AnyElement {
    if src.is_empty() {
        return div().into_any_element();
    }
    let fallback_fg = theme.text_muted;
    let fallback_bg = theme.bg_tertiary;
    let img_id = canvas_image_element_id(src);
    let max_w = CANVAS_VIEW_IMAGE_MAX_WIDTH;
    let (_, reserved_h) = canvas_image_display_size(cx, src, max_w);
    div()
        .w_full()
        .py(px(16.))
        .flex()
        .items_start()
        .child(canvas_img(
            cx,
            src,
            img_id,
            fallback_fg,
            fallback_bg,
            Some(max_w),
            Some(reserved_h),
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_http_cdn_to_https() {
        let src = "http://cdn.mezon.ai/clan/file.png";
        assert_eq!(
            canvas_image_display_src(src),
            "https://cdn.mezon.ai/clan/file.png"
        );
    }

    #[test]
    fn passes_through_https_cdn() {
        let src = "https://cdn.mezon.ai/clan/file.png";
        assert_eq!(canvas_image_display_src(src), src);
    }

    #[test]
    fn passes_through_data_url() {
        let src = "data:image/png;base64,abcd";
        assert_eq!(canvas_image_display_src(src), src);
    }

    #[test]
    fn rejects_external_and_lookalike_hosts() {
        assert!(canvas_image_display_src("https://example.com/x.png").is_empty());
        assert!(canvas_image_display_src("https://cdn.mezon.ai.attacker.com/x.png").is_empty());
        assert!(canvas_image_display_src("file:///etc/passwd").is_empty());
        assert!(canvas_image_display_src("http://127.0.0.1/secret.png").is_empty());
    }

    #[test]
    fn decodes_tiny_png_data_url() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        let src = format!("data:image/png;base64,{png}");
        let bytes = decode_data_image(&src).expect("decode");
        assert!(bytes_to_render_image(&bytes).is_some());
    }

    #[test]
    fn fits_wide_image_to_max_width() {
        let (w, h) = fit_image_to_max_width(800, 600, px(400.));
        assert_eq!(w, px(400.));
        assert_eq!(h, px(300.));
    }

    #[test]
    fn keeps_small_image_intrinsic_size() {
        let (w, h) = fit_image_to_max_width(200, 100, px(800.));
        assert_eq!(w, px(200.));
        assert_eq!(h, px(100.));
    }

    #[gpui::test]
    fn unknown_remote_image_uses_widescreen_box(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let (w, h) = canvas_image_display_size(cx, "https://cdn.mezon.ai/x.png", px(640.));
            assert_eq!(w, px(640.));
            assert_eq!(h, px(360.));
        });
    }

    #[gpui::test]
    fn known_remote_image_size_overrides_fallback(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            remember_canvas_image_size(cx, "https://cdn.mezon.ai/x.png", 300, 200);
            let (w, h) = canvas_image_display_size(cx, "https://cdn.mezon.ai/x.png", px(640.));
            assert_eq!(w, px(300.));
            assert_eq!(h, px(200.));
        });
    }
}
