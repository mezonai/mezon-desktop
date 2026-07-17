use gpui::{App, Global};
use std::sync::Arc;

#[allow(dead_code)]
mod baked_env {
    include!(concat!(env!("OUT_DIR"), "/baked_env.rs"));
}
// No `Debug` derive: AppConfig holds secrets (api_key, imgproxy_key, fcm/tenor/treasury keys,
// webrtc credential). Deny `{:?}` so they can't leak into logs; log specific non-secret fields.
#[derive(Clone)]
pub struct AppConfig {
    // ── REST API (bootstrap, pre-auth) ──────────────────────────────────────
    pub api_host: String,
    pub api_port: u16,
    pub api_secure: bool,
    pub api_key: String,
    pub api_gw_host: String,
    pub api_gw_port: u16,

    // ── WebSocket / streaming ─────────────────────────────────────────────────
    pub tcp_port: Option<u16>,
    pub stream_ws_url: String,
    pub meet_ws_url: String,
    pub notification_ws_url: String,

    // ── OAuth2 ────────────────────────────────────────────────────────────────
    pub oauth2_authorize_url: String,
    pub oauth2_client_id: String,
    pub oauth2_redirect_uri: String,
    pub oauth2_response_type: String,
    pub oauth2_scope: String,
    pub oauth2_code_challenge_method: String,
    pub oauth2_log_out: String,
    pub oauth2_log_out_callback: String,
    pub google_client_id: String,

    // ── CDN / media ───────────────────────────────────────────────────────────
    pub domain_url: String,
    pub redirect_uri: String,
    pub logo_mezon: String,
    pub base_img_url: String,
    pub profile_img_url: String,
    pub imgproxy_base_url: String,
    pub imgproxy_key: String,

    // ── Klipy (GIF search) ────────────────────────────────────────────────────
    pub klipy_key: String,
    pub klipy_base_url: String,

    // ── Treasury / blockchain ─────────────────────────────────────────────────
    pub mezon_treasury_url: String,
    pub mezon_treasury_key: String,
    pub contract_address: String,
    pub mezon_treasury_url_network: String,

    // ── WebRTC (voice/video) ──────────────────────────────────────────────────
    pub webrtc_ice_servers_url: String,
    pub webrtc_ice_servers_username: String,
    pub webrtc_ice_servers_credential: String,

    // ── Firebase / FCM ────────────────────────────────────────────────────────
    pub fcm_api_key: String,
    pub fcm_auth_domain: String,
    pub fcm_project_id: String,
    pub fcm_storage_bucket: String,
    pub fcm_messaging_sender_id: String,
    pub fcm_app_id: String,
    pub fcm_measurement_id: String,
    pub fcm_vapid_key: String,

    // ── Misc ──────────────────────────────────────────────────────────────────
    pub api_client_key_custom: String,
    pub sentry_dsn: String,
    pub anonymous_user_id: String,
    pub max_length_name_allowed: u32,
    pub update_url: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::dev_defaults()
    }
}

impl AppConfig {
    /// Development defaults (matches pre-env hardcoded values).
    pub fn dev_defaults() -> Self {
        Self {
            api_host: "dev-mezon.nccsoft.vn".into(),
            api_port: 8088,
            api_secure: true,
            api_key: "defaultkey".into(),
            api_gw_host: "dev-mezon.nccsoft.vn".into(),
            api_gw_port: 8088,

            tcp_port: Some(7349),
            stream_ws_url: "wss://stn.nccsoft.vn".into(),
            meet_ws_url: "wss://meet.nccsoft.vn".into(),
            notification_ws_url: "wss://gotify.mezon.ai".into(),

            oauth2_authorize_url: "https://oauth2.mezon.ai/oauth2/auth".into(),
            oauth2_client_id: "f049f29e-12a9-464c-938f-0a2f60c3210b".into(),
            oauth2_redirect_uri: "https://dev-mezon.nccsoft.vn/login/callback".into(),
            oauth2_response_type: "code".into(),
            oauth2_scope: "openid+offline".into(),
            oauth2_code_challenge_method: "S256".into(),
            oauth2_log_out: "https://oauth2.mezon.ai/oauth2/sessions/logout".into(),
            oauth2_log_out_callback: "https://mezon.ai/logout/callback".into(),
            google_client_id:
                "391688022389-1k9kb377ea6dccpqii7m5pifjj0agsjc.apps.googleusercontent.com".into(),

            domain_url: "https://mezon.ai".into(),
            redirect_uri: "https://mezon.ai".into(),
            logo_mezon: "https://cdn.komu.vn/images/mezon_logo.png".into(),
            base_img_url: "https://cdn.komu.vn".into(),
            profile_img_url: "https://profile.mezon.ai".into(),
            imgproxy_base_url: "https://dev-imgproxy.nccsoft.vn".into(),
            imgproxy_key: "_AEhOrrckkG-NjqIdVLtzc-dtLFuE4u6ClM0P46ICEY".into(),

            klipy_key: String::new(),
            klipy_base_url: "https://api.klipy.com/api/v1".into(),

            mezon_treasury_url: "https://withdraw-api.nccsoft.vn".into(),
            mezon_treasury_key: String::new(),
            contract_address: String::new(),
            mezon_treasury_url_network: "https://polygonscan.com".into(),

            webrtc_ice_servers_url: "turn:relay.mezon.vn:5349".into(),
            webrtc_ice_servers_username: "turnmezon".into(),
            webrtc_ice_servers_credential: String::new(),

            fcm_api_key: String::new(),
            fcm_auth_domain: "mezon-772fa.firebaseapp.com".into(),
            fcm_project_id: "mezon-772fa".into(),
            fcm_storage_bucket: "mezon-772fa.appspot.com".into(),
            fcm_messaging_sender_id: "285548761692".into(),
            fcm_app_id: String::new(),
            fcm_measurement_id: String::new(),
            fcm_vapid_key: String::new(),

            api_client_key_custom: "mezon.ai".into(),
            sentry_dsn: String::new(),
            anonymous_user_id: "1767478432163172999".into(),
            max_length_name_allowed: 64,
            update_url: "https://cdn.komu.vn/release/".into(),
        }
    }

    pub fn from_env() -> Self {
        let defaults = Self::dev_defaults();
        Self {
            api_host: opt_str(baked_env::NX_CHAT_APP_API_HOST, &defaults.api_host),
            api_port: opt_u16(baked_env::NX_CHAT_APP_API_PORT, defaults.api_port),
            api_secure: opt_bool(baked_env::NX_CHAT_APP_API_SECURE, defaults.api_secure),
            api_key: opt_str(baked_env::NX_CHAT_APP_API_KEY, &defaults.api_key),
            api_gw_host: opt_str(baked_env::NX_CHAT_APP_API_GW_HOST, &defaults.api_gw_host),
            api_gw_port: opt_u16(baked_env::NX_CHAT_APP_API_GW_PORT, defaults.api_gw_port),

            tcp_port: opt_tcp_port(baked_env::NX_CHAT_APP_TCP_PORT, defaults.tcp_port),
            stream_ws_url: opt_str(
                baked_env::NX_CHAT_APP_STREAM_WS_URL,
                &defaults.stream_ws_url,
            ),
            meet_ws_url: opt_str(baked_env::NX_CHAT_APP_MEET_WS_URL, &defaults.meet_ws_url),
            notification_ws_url: opt_str(
                baked_env::NX_CHAT_APP_NOTIFICATION_WS_URL,
                &defaults.notification_ws_url,
            ),

            oauth2_authorize_url: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_AUTHORIZE_URL,
                &defaults.oauth2_authorize_url,
            ),
            oauth2_client_id: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_CLIENT_ID,
                &defaults.oauth2_client_id,
            ),
            oauth2_redirect_uri: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_REDIRECT_URI,
                &defaults.oauth2_redirect_uri,
            ),
            oauth2_response_type: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_RESPONSE_TYPE,
                &defaults.oauth2_response_type,
            ),
            oauth2_scope: opt_str(baked_env::NX_CHAT_APP_OAUTH2_SCOPE, &defaults.oauth2_scope),
            oauth2_code_challenge_method: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_CODE_CHALLENGE_METHOD,
                &defaults.oauth2_code_challenge_method,
            ),
            oauth2_log_out: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_LOG_OUT,
                &defaults.oauth2_log_out,
            ),
            oauth2_log_out_callback: opt_str(
                baked_env::NX_CHAT_APP_OAUTH2_LOG_OUT_CALLBACK,
                &defaults.oauth2_log_out_callback,
            ),
            google_client_id: opt_str(
                baked_env::NX_CHAT_APP_GOOGLE_CLIENT_ID,
                &defaults.google_client_id,
            ),

            domain_url: opt_str(baked_env::NX_DOMAIN_URL, &defaults.domain_url),
            redirect_uri: opt_str(baked_env::NX_CHAT_APP_REDIRECT_URI, &defaults.redirect_uri),
            logo_mezon: opt_str(baked_env::NX_LOGO_MEZON, &defaults.logo_mezon),
            base_img_url: opt_str(baked_env::NX_BASE_IMG_URL, &defaults.base_img_url),
            profile_img_url: opt_str(baked_env::NX_PROFILE_IMG_URL, &defaults.profile_img_url),
            imgproxy_base_url: opt_str(
                baked_env::NX_IMGPROXY_BASE_URL,
                &defaults.imgproxy_base_url,
            ),
            imgproxy_key: opt_str(baked_env::NX_IMGPROXY_KEY, &defaults.imgproxy_key),

            klipy_key: opt_str(baked_env::NX_CHAT_APP_API_KLIPY_KEY, &defaults.klipy_key),
            klipy_base_url: opt_str(
                baked_env::NX_CHAT_APP_API_KLIPY_URL,
                &defaults.klipy_base_url,
            ),

            mezon_treasury_url: opt_str(
                baked_env::NX_CHAT_APP_MEZON_TREASURY_URL,
                &defaults.mezon_treasury_url,
            ),
            mezon_treasury_key: opt_str(
                baked_env::NX_CHAT_APP_API_MEZONTREASURY_KEY,
                &defaults.mezon_treasury_key,
            ),
            contract_address: opt_str(
                baked_env::NX_CHAT_APP_CONTRACT_ADDRESS,
                &defaults.contract_address,
            ),
            mezon_treasury_url_network: opt_str(
                baked_env::NX_CHAT_APP_MEZON_TREASURY_URL_NETWORK,
                &defaults.mezon_treasury_url_network,
            ),

            webrtc_ice_servers_url: opt_str(
                baked_env::NX_WEBRTC_ICESERVERS_URL,
                &defaults.webrtc_ice_servers_url,
            ),
            webrtc_ice_servers_username: opt_str(
                baked_env::NX_WEBRTC_ICESERVERS_USERNAME,
                &defaults.webrtc_ice_servers_username,
            ),
            webrtc_ice_servers_credential: opt_str(
                baked_env::NX_WEBRTC_ICESERVERS_CREDENTIAL,
                &defaults.webrtc_ice_servers_credential,
            ),

            fcm_api_key: opt_str(baked_env::NX_CHAT_APP_FCM_API_KEY, &defaults.fcm_api_key),
            fcm_auth_domain: opt_str(
                baked_env::NX_CHAT_APP_FCM_AUTH_DOMAIN,
                &defaults.fcm_auth_domain,
            ),
            fcm_project_id: opt_str(
                baked_env::NX_CHAT_APP_FCM_PROJECT_ID,
                &defaults.fcm_project_id,
            ),
            fcm_storage_bucket: opt_str(
                baked_env::NX_CHAT_APP_FCM_STORAGE_BUCKET,
                &defaults.fcm_storage_bucket,
            ),
            fcm_messaging_sender_id: opt_str(
                baked_env::NX_CHAT_APP_FCM_MESSAGING_SENDER_ID,
                &defaults.fcm_messaging_sender_id,
            ),
            fcm_app_id: opt_str(baked_env::NX_CHAT_APP_FCM_APP_ID, &defaults.fcm_app_id),
            fcm_measurement_id: opt_str(
                baked_env::NX_CHAT_APP_FCM_MEASUREMENT_ID,
                &defaults.fcm_measurement_id,
            ),
            fcm_vapid_key: opt_str(
                baked_env::NX_CHAT_APP_FCM_VAPID_KEY,
                &defaults.fcm_vapid_key,
            ),

            api_client_key_custom: opt_str(
                baked_env::NX_CHAT_APP_API_CLIENT_KEY_CUSTOM,
                &defaults.api_client_key_custom,
            ),
            sentry_dsn: opt_str(
                baked_env::NX_CHAT_SENTRY_DSN.or(baked_env::NX_CHAT_SENTRY_DNS),
                &defaults.sentry_dsn,
            ),
            anonymous_user_id: opt_str(
                baked_env::NX_CHAT_APP_ANNONYMOUS_USER_ID,
                &defaults.anonymous_user_id,
            ),
            max_length_name_allowed: opt_u32(
                baked_env::NX_MAX_LENGTH_NAME_ALLOWED,
                defaults.max_length_name_allowed,
            ),
            update_url: opt_str(baked_env::NX_UPDATE_URL, &defaults.update_url),
        }
    }

    /// REST client bootstrap host — mirrors `getMezonConfig()` in the web app
    /// (`NX_CHAT_APP_API_GW_HOST`, not `NX_CHAT_APP_API_HOST`).
    pub fn client_host(&self) -> &str {
        &self.api_gw_host
    }

    /// REST client bootstrap port — mirrors `getMezonConfig()` in the web app.
    pub fn client_port(&self) -> u16 {
        self.api_gw_port
    }

    pub fn init_global(config: Arc<AppConfig>, cx: &mut App) {
        cx.set_global(GlobalAppConfig(config));
    }

    pub fn global(cx: &App) -> &AppConfig {
        cx.global::<GlobalAppConfig>().0.as_ref()
    }

    pub fn try_global(cx: &App) -> Option<&AppConfig> {
        cx.try_global::<GlobalAppConfig>().map(|g| g.0.as_ref())
    }

    pub fn global_arc(cx: &App) -> Option<Arc<AppConfig>> {
        cx.try_global::<GlobalAppConfig>().map(|g| g.0.clone())
    }

    pub fn imgproxy_url(
        &self,
        source_image_url: &str,
        width: u32,
        height: u32,
        resize_type: &str,
    ) -> String {
        if source_image_url.is_empty() {
            return String::new();
        }
        if !source_image_url.starts_with("https://cdn.mezon")
            && !source_image_url.starts_with("https://profile.mezon")
        {
            return source_image_url.to_string();
        }
        let processing_options = format!("rs:{}:{}:{}:1/mb:2097152", resize_type, width, height);
        let path = format!("/{}/plain/{}@webp", processing_options, source_image_url);
        let base = self.imgproxy_base_url.trim_end_matches('/');
        format!("{}/{}{}", base, self.imgproxy_key, path)
    }

    pub fn voice_link(&self, clan_id: &str, channel_id: &str) -> String {
        let base = self.domain_url.trim_end_matches('/');
        if clan_id.is_empty() || clan_id == "0" {
            format!("{base}/chat/direct/message/{channel_id}/3")
        } else {
            format!("{base}/chat/clans/{clan_id}/channels/{channel_id}")
        }
    }

    pub fn avatar_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, 100, 100, "fit")
    }

    pub fn profile_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, 300, 300, "fill")
    }

    pub fn emoji_src(&self, emoji_id: &str) -> String {
        self.emoji_src_sized(emoji_id, 100)
    }

    pub fn emoji_src_sized(&self, emoji_id: &str, size: u32) -> String {
        if emoji_id.is_empty() || emoji_id == "0" {
            return String::new();
        }
        let source = format!("{}/emojis/{}.webp", self.base_img_url, emoji_id);
        self.imgproxy_url(&source, size, size, "fit")
    }

    pub fn attachment_proxy(
        &self,
        source: &str,
        real_width: u32,
        real_height: u32,
    ) -> (String, f32, f32) {
        let (display_w, display_h) = attachment_display_dimensions(real_width, real_height);
        if source.is_empty() {
            return (String::new(), display_w, display_h);
        }
        let proxy_w = display_w.ceil().max(1.0) as u32;
        let proxy_h = display_h.ceil().max(1.0) as u32;
        let resize = if real_width == 0 || real_height == 0 {
            "fill"
        } else if real_width < proxy_w || real_height < proxy_h {
            "fill-down"
        } else {
            "fill"
        };
        (
            self.imgproxy_url(source, proxy_w, proxy_h, resize),
            display_w,
            display_h,
        )
    }

    /// Full-size imgproxy URL for the image viewer. Caps the longest side to
    /// 1600 px preserving aspect ratio (React `MessageAttachment` / `GalleryModal`
    /// open: width clamped to 1600, height scaled).
    pub fn viewer_proxy(&self, source: &str, real_width: u32, real_height: u32) -> String {
        if source.is_empty() {
            return String::new();
        }
        let (w, h) = viewer_dimensions(real_width, real_height);
        self.imgproxy_url(source, w, h, "fit")
    }

    /// Square thumbnail URL for the gallery grid (React: 120x120 `fill`).
    pub fn gallery_thumb_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, GALLERY_THUMB_SIZE, GALLERY_THUMB_SIZE, "fill")
    }

    /// Thumbnail URL for the image viewer's sidebar strip.
    pub fn viewer_thumb_proxy(&self, source: &str) -> String {
        self.imgproxy_url(source, VIEWER_THUMB_SIZE, VIEWER_THUMB_SIZE, "fill")
    }
}

pub const VIEWER_MAX_DIMENSION: u32 = 1600;
pub const GALLERY_THUMB_SIZE: u32 = 120;
pub const VIEWER_THUMB_SIZE: u32 = 80;

/// Viewer imgproxy target dimensions: clamp the longest side to
/// [`VIEWER_MAX_DIMENSION`], preserving aspect ratio. `0` means "let imgproxy
/// decide" (unknown source size).
fn viewer_dimensions(real_width: u32, real_height: u32) -> (u32, u32) {
    if real_width == 0 || real_height == 0 {
        return (VIEWER_MAX_DIMENSION, 0);
    }
    if real_width <= VIEWER_MAX_DIMENSION && real_height <= VIEWER_MAX_DIMENSION {
        return (real_width, real_height);
    }
    if real_width >= real_height {
        let h = (VIEWER_MAX_DIMENSION as u64 * real_height as u64 / real_width as u64) as u32;
        (VIEWER_MAX_DIMENSION, h.max(1))
    } else {
        let w = (VIEWER_MAX_DIMENSION as u64 * real_width as u64 / real_height as u64) as u32;
        (w.max(1), VIEWER_MAX_DIMENSION)
    }
}

pub const REM: f32 = 16.0;
const SMALL_IMAGE_THRESHOLD: f32 = 12.0;
const MIN_MESSAGE_LENGTH_FOR_BLUR: usize = 40;
const MIN_MEDIA_WIDTH_WITH_TEXT: f32 = 20.0 * REM;
const MIN_MEDIA_WIDTH: f32 = SMALL_IMAGE_THRESHOLD * REM;
const MIN_MEDIA_HEIGHT: f32 = 5.0 * REM;
const MESSAGE_MAX_WIDTH_REM: f32 = 29.0;
const MESSAGE_OWN_MAX_WIDTH_REM: f32 = 30.0;
const AVAILABLE_HEIGHT_REM: f32 = 27.0;
const DEFAULT_MEDIA_SIDE: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaDimensions {
    pub width: f32,
    pub height: f32,
    pub is_small: bool,
}

pub fn media_available_width(is_own: bool) -> f32 {
    let rem = if is_own {
        MESSAGE_OWN_MAX_WIDTH_REM
    } else {
        MESSAGE_MAX_WIDTH_REM
    };
    rem * REM
}

fn fit_within_box(
    available_width: f32,
    available_height: f32,
    media_width: f32,
    media_height: f32,
) -> (f32, f32) {
    let aspect_ratio = media_height / media_width;
    let calculated_width = media_width.min(available_width);
    let calculated_height = (calculated_width * aspect_ratio).round();
    if calculated_height > available_height {
        ((available_height / aspect_ratio).round(), available_height)
    } else {
        (calculated_width, calculated_height)
    }
}

fn min_media_width_for_text(message_text_len: usize) -> f32 {
    if message_text_len > MIN_MESSAGE_LENGTH_FOR_BLUR {
        MIN_MEDIA_WIDTH_WITH_TEXT
    } else {
        MIN_MEDIA_WIDTH
    }
}

pub fn calculate_media_dimensions(
    real_width: u32,
    real_height: u32,
    is_own: bool,
    message_text_len: usize,
) -> MediaDimensions {
    let (base_width, base_height) = if real_width == 0 || real_height == 0 {
        (DEFAULT_MEDIA_SIDE, DEFAULT_MEDIA_SIDE)
    } else {
        (real_width as f32, real_height as f32)
    };
    let (width, height) = fit_within_box(
        media_available_width(is_own),
        AVAILABLE_HEIGHT_REM * REM,
        base_width,
        base_height,
    );
    let min_width = min_media_width_for_text(message_text_len);
    let mut stretch_factor = 1.0;
    if width < min_width && (min_width - width) < SMALL_IMAGE_THRESHOLD {
        stretch_factor = min_width / width;
    }
    if height * stretch_factor < MIN_MEDIA_HEIGHT
        && (MIN_MEDIA_HEIGHT - height * stretch_factor) < SMALL_IMAGE_THRESHOLD
    {
        stretch_factor = MIN_MEDIA_HEIGHT / height;
    }
    let final_width = (width * stretch_factor).round();
    let final_height = (height * stretch_factor).round();
    MediaDimensions {
        width: final_width,
        height: final_height,
        is_small: final_width < min_width || final_height < MIN_MEDIA_HEIGHT,
    }
}

pub fn attachment_display_dimensions(real_width: u32, real_height: u32) -> (f32, f32) {
    let dimensions = calculate_media_dimensions(real_width, real_height, false, 0);
    (dimensions.width, dimensions.height)
}

struct GlobalAppConfig(Arc<AppConfig>);
impl Global for GlobalAppConfig {}

fn normalize(value: Option<&'static str>) -> Option<&'static str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn opt_str(value: Option<&'static str>, default: &str) -> String {
    normalize(value)
        .map(str::to_owned)
        .unwrap_or_else(|| default.to_owned())
}

fn opt_u16(value: Option<&'static str>, default: u16) -> u16 {
    normalize(value)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
fn opt_opt_u16(value: Option<&'static str>) -> Option<u16> {
    normalize(value).and_then(|v| v.parse().ok())
}

fn opt_tcp_port(value: Option<&'static str>, default: Option<u16>) -> Option<u16> {
    match value {
        None => default,
        Some(v) => normalize(Some(v)).and_then(|v| v.parse().ok()),
    }
}

fn opt_u32(value: Option<&'static str>, default: u32) -> u32 {
    normalize(value)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn opt_bool(value: Option<&'static str>, default: bool) -> bool {
    match normalize(value) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(w: u32, h: u32) -> (f32, f32, bool) {
        let d = calculate_media_dimensions(w, h, false, 0);
        (d.width, d.height, d.is_small)
    }

    #[test]
    fn media_dimensions_landscape_caps_to_available_width() {
        assert_eq!(dims(800, 600), (464.0, 348.0, false));
    }

    #[test]
    fn media_dimensions_own_message_uses_wider_box() {
        let d = calculate_media_dimensions(800, 600, true, 0);
        assert_eq!((d.width, d.height), (480.0, 360.0));
    }

    #[test]
    fn media_dimensions_tall_image_is_small() {
        assert_eq!(dims(100, 400), (100.0, 400.0, true));
    }

    #[test]
    fn media_dimensions_panorama_caps_height_band() {
        assert_eq!(dims(2000, 100), (464.0, 23.0, true));
    }

    #[test]
    fn media_dimensions_small_image_min_stretch() {
        assert_eq!(dims(185, 75), (197.0, 80.0, false));
    }

    #[test]
    fn media_dimensions_unknown_defaults_to_hundred_square() {
        assert_eq!(dims(0, 0), (100.0, 100.0, true));
    }

    #[test]
    fn dev_defaults_match_legacy_constants() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.api_host, "dev-mezon.nccsoft.vn");
        assert_eq!(cfg.api_port, 8088);
        assert!(cfg.api_secure);
        assert_eq!(cfg.tcp_port, Some(7349));
        assert_eq!(cfg.client_host(), "dev-mezon.nccsoft.vn");
        assert_eq!(cfg.client_port(), 8088);
    }

    #[test]
    fn dev_defaults_use_dev_tcp_port() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.tcp_port, Some(7349));
        assert_eq!(cfg.client_host(), "dev-mezon.nccsoft.vn");
    }

    #[test]
    fn opt_helpers_fall_back_when_unset_or_blank() {
        assert_eq!(opt_str(None, "def"), "def");
        assert_eq!(opt_str(Some("  "), "def"), "def");
        assert_eq!(opt_str(Some(" val "), "def"), "val");
        assert_eq!(opt_u16(None, 8088), 8088);
        assert_eq!(opt_u16(Some("443"), 8088), 443);
        assert_eq!(opt_u16(Some("nope"), 8088), 8088);
        assert_eq!(opt_u32(Some("128"), 64), 128);
        assert_eq!(opt_opt_u16(None), None);
        assert_eq!(opt_opt_u16(Some("7349")), Some(7349));
        assert!(opt_bool(Some("true"), false));
        assert!(opt_bool(Some("1"), false));
        assert!(!opt_bool(None, false));
    }

    #[test]
    fn imgproxy_url_rewrites_cdn_urls() {
        let cfg = AppConfig {
            imgproxy_base_url: "https://imgproxy.example".into(),
            imgproxy_key: "sig".into(),
            ..AppConfig::dev_defaults()
        };
        let src = "https://cdn.komu.vn/images/avatar.png";
        let out = cfg.imgproxy_url(src, 100, 100, "fit");
        assert!(out.starts_with("https://imgproxy.example/sig/rs:fit:100:100:1/mb:2097152/plain/"));
        assert!(out.ends_with("@webp"));
        assert!(out.contains(src));
    }

    #[test]
    fn imgproxy_url_skips_external_urls() {
        let cfg = AppConfig::dev_defaults();
        let src = "https://example.com/avatar.png";
        assert_eq!(cfg.imgproxy_url(src, 100, 100, "fit"), src);
    }

    #[test]
    fn imgproxy_url_proxies_cdn_on_dev_base() {
        let cfg = AppConfig::dev_defaults();
        let src = "https://cdn.komu.vn/images/avatar.png";
        let out = cfg.imgproxy_url(src, 100, 100, "fit");
        assert!(out.starts_with("https://dev-imgproxy.nccsoft.vn/"));
        assert!(out.contains("/rs:fit:100:100:1/mb:2097152/plain/"));
        assert!(out.contains(src));
        assert!(out.ends_with("@webp"));
    }

    #[test]
    fn imgproxy_url_empty_returns_empty() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.imgproxy_url("", 100, 100, "fit"), "");
    }

    #[test]
    fn avatar_proxy_matches_react_fit_100() {
        let cfg = AppConfig {
            imgproxy_base_url: "https://imgproxy.example".into(),
            imgproxy_key: "sig".into(),
            ..AppConfig::dev_defaults()
        };
        let out = cfg.avatar_proxy("https://cdn.komu.vn/a.png");
        assert!(
            out.contains("rs:fit:100:100:1/mb:2097152/plain/"),
            "avatar must be 100x100 fit like React MessageAvatar: {out}"
        );
    }

    #[test]
    fn attachment_proxy_uses_one_x_display_dims_like_react() {
        let cfg = AppConfig {
            imgproxy_base_url: "https://imgproxy.example".into(),
            imgproxy_key: "sig".into(),
            ..AppConfig::dev_defaults()
        };
        let src = "https://cdn.komu.vn/images/photo.png";
        let (url, display_w, display_h) = cfg.attachment_proxy(src, 1200, 800);
        let pw = display_w.ceil() as u32;
        let ph = display_h.ceil() as u32;
        assert!(
            url.contains(&format!("rs:fill:{pw}:{ph}:1/mb:2097152/plain/")),
            "attachment proxy must be 1x display dims like React Photo.tsx: {url}"
        );
    }

    #[test]
    fn viewer_dimensions_clamp_longest_side() {
        assert_eq!(viewer_dimensions(800, 600), (800, 600));
        assert_eq!(viewer_dimensions(3200, 1600), (1600, 800));
        assert_eq!(viewer_dimensions(1600, 3200), (800, 1600));
        assert_eq!(viewer_dimensions(0, 0), (VIEWER_MAX_DIMENSION, 0));
    }

    #[test]
    fn viewer_proxy_empty_returns_empty() {
        let cfg = AppConfig::dev_defaults();
        assert_eq!(cfg.viewer_proxy("", 100, 100), "");
    }
}
