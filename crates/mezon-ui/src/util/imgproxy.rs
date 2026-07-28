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

pub fn avatar_url(cx: &App, source_url: &str) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.avatar_proxy(source_url))
        .unwrap_or_else(|| source_url.to_string())
}

pub fn profile_url(cx: &App, source_url: &str) -> String {
    AppConfig::try_global(cx)
        .map(|cfg| cfg.profile_proxy(source_url))
        .unwrap_or_else(|| source_url.to_string())
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
