use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use futures::AsyncReadExt as _;
use gpui::{
    AnyElement, App, EntityId, Global, InteractiveElement, ObjectFit, Pixels, RenderImage,
    SharedString, Styled, StyledImage, div, img, prelude::*, px,
};
use smallvec::smallvec;

use mezon_store::AppConfig;
use mezon_store::config::url_has_origin;
use mezon_widgets::{Icon, IconName};

pub const CANVAS_IMAGE_FALLBACK_HEIGHT: Pixels = px(200.);
pub const CANVAS_IMAGE_BLOCK_PADDING: Pixels = px(16.);
const CANVAS_IMAGE_PROXY_MAX: u32 = 1440;

const IMAGE_DIMENSION_PROBE_MAX_BYTES: u64 = 24 * 1024 * 1024;
const CANVAS_IMAGE_DIM_CACHE_MAX: usize = 512;
const CANVAS_DATA_IMAGE_CACHE_MAX: usize = 64;
const CANVAS_DECODE_MAX_WIDTH: u32 = 16_384;
const CANVAS_DECODE_MAX_HEIGHT: u32 = 16_384;
const CANVAS_DECODE_MAX_PIXELS: u64 = 48_000_000;
const CANVAS_DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;

const STATIC_CANVAS_IMAGE_ORIGINS: [&str; 16] = [
    "https://cdn.mezon.ai",
    "http://cdn.mezon.ai",
    "https://cdn.mezon.vn",
    "http://cdn.mezon.vn",
    "https://cdn.komu.ai",
    "http://cdn.komu.ai",
    "https://cdn.komu.vn",
    "http://cdn.komu.vn",
    "https://profile.mezon.ai",
    "http://profile.mezon.ai",
    "https://profile.mezon.vn",
    "https://pub-35517170c1554a008bed9d7565fa4bb2.r2.dev",
    "https://imgproxy.mezon.ai",
    "https://imgproxy.komu.vn",
    "https://dev-imgproxy.nccsoft.vn",
    "https://imgproxy.nccsoft.vn",
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
    let url = canvas_image_probe_src(cx, src);
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

fn is_http_url(src: &str) -> bool {
    src.starts_with("https://") || src.starts_with("http://")
}

fn is_static_canvas_origin(src: &str) -> bool {
    STATIC_CANVAS_IMAGE_ORIGINS
        .iter()
        .any(|origin| url_has_origin(src, origin))
}

fn canvas_url_host(src: &str) -> Option<&str> {
    let rest = src
        .strip_prefix("https://")
        .or_else(|| src.strip_prefix("http://"))?;
    let hostport = rest.split(['/', '?', '#']).next()?;
    let host = hostport.rsplit('@').next()?;
    if let Some(inner) = host.strip_prefix('[') {
        return inner.split(']').next().filter(|h| !h.is_empty());
    }
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    if host.is_empty() { None } else { Some(host) }
}

fn is_r2_dev_url(src: &str) -> bool {
    canvas_url_host(src).is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "r2.dev" || host.ends_with(".r2.dev")
    })
}

fn is_already_imgproxy_url(src: &str, cx: Option<&App>) -> bool {
    if src.contains("/plain/http://") || src.contains("/plain/https://") {
        return true;
    }
    if let Some(cfg) = cx.and_then(AppConfig::try_global) {
        let imgproxy = cfg.imgproxy_base_url.trim_end_matches('/');
        if !imgproxy.is_empty() && url_has_origin(src, imgproxy) {
            return true;
        }
    }
    [
        "https://imgproxy.mezon.ai",
        "https://imgproxy.komu.vn",
        "https://dev-imgproxy.nccsoft.vn",
        "https://imgproxy.nccsoft.vn",
    ]
    .iter()
    .any(|origin| url_has_origin(src, origin))
}

fn parse_loose_ipv4(host: &str) -> Option<Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in host.split('.') {
        if count == 4 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value = if part.starts_with('0') && part.len() > 1 {
            u32::from_str_radix(part, 8).ok()?
        } else {
            part.parse::<u32>().ok()?
        };
        if value > 255 {
            return None;
        }
        octets[count] = value as u8;
        count += 1;
    }
    (count == 4).then_some(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]))
}

fn is_blocked_canvas_image_url(src: &str) -> bool {
    let Some(host) = canvas_url_host(src) else {
        return true;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_non_global_ip(ip);
    }
    parse_loose_ipv4(&host).is_some_and(|ip| is_non_global_ip(IpAddr::V4(ip)))
}

fn is_non_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || v6.is_loopback()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

fn should_upgrade_http_to_https(src: &str, cx: Option<&App>) -> bool {
    let https = format!("https://{}", src.trim_start_matches("http://"));
    if is_static_canvas_origin(src) || is_static_canvas_origin(&https) {
        return true;
    }
    if let Some(cfg) = cx.and_then(AppConfig::try_global) {
        return cfg.is_own_media_origin(src) || cfg.is_own_media_origin(&https);
    }
    false
}

fn canvas_should_imgproxy(src: &str, cx: Option<&App>) -> bool {
    if is_r2_dev_url(src) || is_already_imgproxy_url(src, cx) {
        return false;
    }
    if let Some(cfg) = cx.and_then(AppConfig::try_global) {
        return cfg.is_own_media_origin(src);
    }
    false
}

fn normalize_canvas_image_src(src: &str, cx: Option<&App>) -> String {
    let src = src.trim().replace("&amp;", "&");
    if src.is_empty() {
        return String::new();
    }
    if is_data_image_url(&src) {
        return src;
    }
    let src = if let Some(rest) = src.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        src
    };
    if !is_http_url(&src) {
        let Some(cfg) = cx.and_then(AppConfig::try_global) else {
            return String::new();
        };
        let base = cfg.base_img_url.trim_end_matches('/');
        if base.is_empty() {
            return String::new();
        }
        return format!("{base}/{}", src.trim_start_matches('/'));
    }
    let src = if src.starts_with("http://") && should_upgrade_http_to_https(&src, cx) {
        format!("https://{}", src.trim_start_matches("http://"))
    } else {
        src
    };
    if let Some(cfg) = cx.and_then(AppConfig::try_global) {
        return cfg.read_media_url(&src).into_owned();
    }
    src
}

pub fn canvas_image_display_src_with_app(cx: &App, src: &str) -> String {
    canvas_image_display_src_checked(src, Some(cx))
}

fn canvas_image_probe_src(cx: &App, src: &str) -> String {
    let src = normalize_canvas_image_src(src, Some(cx));
    if src.is_empty() || is_data_image_url(&src) || !is_http_url(&src) {
        return String::new();
    }
    if is_blocked_canvas_image_url(&src) {
        return String::new();
    }
    if let Some(cfg) = AppConfig::try_global(cx) {
        if !cfg.is_own_media_origin(&src) {
            return String::new();
        }
    } else if !is_static_canvas_origin(&src) {
        return String::new();
    }
    src
}

fn canvas_image_display_src_checked(src: &str, cx: Option<&App>) -> String {
    let src = normalize_canvas_image_src(src, cx);
    if src.is_empty() {
        return String::new();
    }
    if is_data_image_url(&src) {
        return src;
    }
    if !is_http_url(&src) || is_blocked_canvas_image_url(&src) {
        return String::new();
    }
    if canvas_should_imgproxy(&src, cx)
        && let Some(cfg) = cx.and_then(AppConfig::try_global)
    {
        let proxied = cfg.imgproxy_url(&src, CANVAS_IMAGE_PROXY_MAX, CANVAS_IMAGE_PROXY_MAX, "fit");
        if !proxied.is_empty() {
            return proxied;
        }
    }
    src
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

pub fn canvas_image_known_display_size(
    cx: &App,
    src: &str,
    max_width: Pixels,
) -> Option<(Pixels, Pixels)> {
    let (width, height) = image_pixel_size(src).or_else(|| canvas_image_known_size(cx, src))?;
    Some(fit_image_to_max_width(width, height, max_width))
}

pub fn canvas_image_display_size(cx: &App, src: &str, max_width: Pixels) -> (Pixels, Pixels) {
    canvas_image_known_display_size(cx, src, max_width)
        .unwrap_or((max_width, CANVAS_IMAGE_FALLBACK_HEIGHT))
}

static CANVAS_DATA_IMAGE_CACHE: OnceLock<Mutex<HashMap<String, Arc<RenderImage>>>> =
    OnceLock::new();

pub fn canvas_image_element_id_at(src: &str, instance: usize) -> SharedString {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    src.hash(&mut hasher);
    SharedString::from(format!("canvas-img-{:x}-{instance}", hasher.finish()))
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

fn apply_canvas_image_size<I: Styled + StyledImage>(
    el: I,
    fitted: Option<(Pixels, Pixels)>,
    max_width: Pixels,
) -> I {
    match fitted {
        Some((width, height)) => el.w(width).h(height).object_fit(ObjectFit::Contain),
        None => el
            .max_w(max_width)
            .max_h(CANVAS_IMAGE_FALLBACK_HEIGHT)
            .object_fit(ObjectFit::Contain),
    }
}

pub fn canvas_img(
    cx: &App,
    src: &str,
    fallback_fg: gpui::Rgba,
    fallback_bg: gpui::Rgba,
    max_width: Pixels,
    display_src: Option<&str>,
    instance: usize,
) -> AnyElement {
    let img_id = canvas_image_element_id_at(src, instance);
    let max_width = max_width.max(px(1.));
    let fitted = canvas_image_known_display_size(cx, src, max_width);
    if is_data_image_url(src) {
        if let Some(render) = canvas_data_render_image(src) {
            return apply_canvas_image_size(img(render).id(img_id), fitted, max_width)
                .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
                .into_any_element();
        }
        return canvas_image_fallback_inner(fallback_fg, fallback_bg);
    }
    let display_src = display_src
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| canvas_image_display_src_with_app(cx, src));
    if display_src.is_empty() {
        return canvas_image_fallback_inner(fallback_fg, fallback_bg);
    }
    apply_canvas_image_size(
        img(SharedString::from(display_src)).id(img_id),
        fitted,
        max_width,
    )
    .with_fallback(move || canvas_image_fallback_inner(fallback_fg, fallback_bg))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_http_cdn_to_https() {
        let src = "http://cdn.mezon.ai/clan/file.png";
        assert_eq!(
            canvas_image_display_src_checked(src, None),
            "https://cdn.mezon.ai/clan/file.png"
        );
    }

    #[test]
    fn passes_through_https_cdn() {
        let src = "https://cdn.mezon.ai/clan/file.png";
        assert_eq!(canvas_image_display_src_checked(src, None), src);
    }

    #[test]
    fn passes_through_data_url() {
        let src = "data:image/png;base64,abcd";
        assert_eq!(canvas_image_display_src_checked(src, None), src);
    }

    #[test]
    fn passes_through_https_like_react_img() {
        assert_eq!(
            canvas_image_display_src_checked("https://example.com/x.png", None),
            "https://example.com/x.png"
        );
        assert_eq!(
            canvas_image_display_src_checked(
                "https://imgproxy.mezon.ai/k/rs:fit:100:100:1/mb:2097152/plain/https://cdn.mezon.ai/a.png@webp",
                None
            ),
            "https://imgproxy.mezon.ai/k/rs:fit:100:100:1/mb:2097152/plain/https://cdn.mezon.ai/a.png@webp"
        );
        assert_eq!(
            canvas_image_display_src_checked("http://example.com/x.png", None),
            "http://example.com/x.png"
        );
    }

    #[test]
    fn rejects_local_and_non_http_sources() {
        assert!(canvas_image_display_src_checked("file:///etc/passwd", None).is_empty());
        assert!(canvas_image_display_src_checked("http://127.0.0.1/secret.png", None).is_empty());
        assert!(canvas_image_display_src_checked("https://localhost/x.png", None).is_empty());
        assert!(canvas_image_display_src_checked("javascript:alert(1)", None).is_empty());
        assert!(canvas_image_display_src_checked("http://192.168.1.1/x.png", None).is_empty());
        assert!(canvas_image_display_src_checked("http://10.0.0.8/x.png", None).is_empty());
        assert!(canvas_image_display_src_checked("http://0177.0.0.1/x.png", None).is_empty());
        assert!(canvas_image_display_src_checked("http://[::1]/x.png", None).is_empty());
    }

    #[test]
    fn allows_web_cdn_hosts_without_app() {
        assert_eq!(
            canvas_image_display_src_checked("https://cdn.komu.vn/photo.png", None),
            "https://cdn.komu.vn/photo.png"
        );
        assert_eq!(
            canvas_image_display_src_checked(
                "https://pub-35517170c1554a008bed9d7565fa4bb2.r2.dev/clan/photo.png",
                None
            ),
            "https://pub-35517170c1554a008bed9d7565fa4bb2.r2.dev/clan/photo.png"
        );
        assert_eq!(
            canvas_image_display_src_checked("https://cdn.mezon.vn/clan/photo.png", None),
            "https://cdn.mezon.vn/clan/photo.png"
        );
    }

    #[test]
    fn upgrades_protocol_relative_cdn() {
        assert_eq!(
            canvas_image_display_src_checked("//cdn.mezon.ai/clan/file.png", None),
            "https://cdn.mezon.ai/clan/file.png"
        );
    }

    #[test]
    fn trims_and_unescapes_src() {
        assert_eq!(
            canvas_image_display_src_checked("  https://cdn.mezon.ai/a.png?x=1&amp;y=2  ", None),
            "https://cdn.mezon.ai/a.png?x=1&y=2"
        );
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

    #[test]
    fn view_and_edit_share_column_fit() {
        let (view_w, view_h) = fit_image_to_max_width(2000, 1000, px(900.));
        let (edit_w, edit_h) = fit_image_to_max_width(2000, 1000, px(900.));
        assert_eq!((view_w, view_h), (edit_w, edit_h));
        assert_eq!(view_w, px(900.));
        assert_eq!(view_h, px(450.));
        let (capped_w, _) = fit_image_to_max_width(2000, 1000, px(720.));
        assert_ne!(view_w, capped_w);
    }

    #[gpui::test]
    fn unknown_remote_image_does_not_invent_pixel_size(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(
                canvas_image_known_display_size(cx, "https://cdn.mezon.ai/x.png", px(640.))
                    .is_none()
            );
            let (w, h) = canvas_image_display_size(cx, "https://cdn.mezon.ai/x.png", px(640.));
            assert_eq!(w, px(640.));
            assert_eq!(h, CANVAS_IMAGE_FALLBACK_HEIGHT);
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

    #[gpui::test]
    fn display_src_with_app_proxies_legacy_and_upload_origins(cx: &mut gpui::TestAppContext) {
        use std::sync::Arc;
        cx.update(|cx| {
            let cfg = AppConfig::dev_defaults();
            let base = cfg.base_img_url.clone();
            let upload = format!("{}/images/photo.png", cfg.upload_img_url);
            let expected_read = format!("{base}/images/photo.png");
            AppConfig::init_global(Arc::new(cfg), cx);

            let legacy =
                canvas_image_display_src_with_app(cx, "https://cdn.mezon.ai/clan/file.png");
            assert!(
                legacy.starts_with(&format!(
                    "{}/",
                    AppConfig::global(cx)
                        .imgproxy_base_url
                        .trim_end_matches('/')
                )),
                "{legacy}"
            );
            assert!(legacy.contains("https://cdn.mezon.ai/clan/file.png"));

            let own = canvas_image_display_src_with_app(cx, &format!("{base}/uploads/a.png"));
            assert!(own.contains(&format!("{base}/uploads/a.png")), "{own}");
            assert!(own.starts_with(&AppConfig::global(cx).imgproxy_base_url));

            let rewritten = canvas_image_display_src_with_app(cx, &upload);
            assert!(rewritten.contains(&expected_read), "{rewritten}");
            assert!(!rewritten.contains(&upload));

            let relative = canvas_image_display_src_with_app(cx, "clan/file.png");
            assert!(
                relative.contains(&format!("{base}/clan/file.png")),
                "{relative}"
            );

            let r2 = "https://pub-35517170c1554a008bed9d7565fa4bb2.r2.dev/clan/photo.png";
            let r2_out = canvas_image_display_src_with_app(cx, r2);
            assert_eq!(r2_out, r2);

            let stored_proxy = "https://imgproxy.komu.vn/k/rs:fit:100:100:1/mb:2097152/plain/https://cdn.mezon.ai/a.png@webp";
            assert_eq!(
                canvas_image_display_src_with_app(cx, stored_proxy),
                stored_proxy
            );
        });
    }
}
