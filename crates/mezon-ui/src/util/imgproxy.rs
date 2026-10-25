use gpui::App;

use mezon_store::AppConfig;

pub fn proxied(cx: &App, source_url: &str, width: u32, height: u32, resize_type: &str) -> String {
    if source_url.is_empty() {
        return String::new();
    }
    AppConfig::try_global(cx)
        .map(|cfg| cfg.imgproxy_url(source_url, width, height, resize_type))
        .unwrap_or_else(|| source_url.to_string())
}

pub fn clan_logo_url(cx: &App, source_url: &str) -> String {
    if source_url.is_empty() {
        return String::new();
    }
    AppConfig::try_global(cx)
        .map(|cfg| cfg.clan_logo_proxy(source_url, 100))
        .unwrap_or_else(|| source_url.to_string())
}

pub fn avatar_url(cx: &App, source_url: &str) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.avatar_proxy(source_url))
        .unwrap_or_else(|| source_url.to_string())
}

/// Role icons render at 12-20px; request them at the same cap the icon decode
/// loader uses (`ICON_DECODE_MAX_PX`) so the full-size upload is never fetched.
pub fn role_icon_url(cx: &App, source_url: &str) -> String {
    proxied(cx, source_url, 64, 64, "fit")
}

/// The role-icon picker preview is a 64pt box, so it needs 128px to stay sharp
/// on a 2x display. Everything else renders at 12-20pt and uses [`role_icon_url`].
pub fn role_icon_preview_url(cx: &App, source_url: &str) -> String {
    proxied(cx, source_url, 128, 128, "fit")
}

pub fn profile_url(cx: &App, source_url: &str) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.profile_proxy(source_url))
        .unwrap_or_else(|| source_url.to_string())
}

pub fn stream_cover_url(cx: &App, source_url: &str) -> String {
    proxied(cx, source_url, 1280, 720, "fill")
}

pub fn cdn_asset_url(cx: &App, path: &str) -> String {
    let base = AppConfig::try_global(cx)
        .map(|cfg| cfg.base_img_url.clone())
        .unwrap_or_else(|| AppConfig::dev_defaults().base_img_url);
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub fn emoji_url(cx: &App, emoji_id: &str) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.emoji_src(emoji_id))
        .unwrap_or_default()
}

pub fn emoji_url_sized(cx: &App, emoji_id: &str, size: u32) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.emoji_src_sized(emoji_id, size))
        .unwrap_or_default()
}
