pub mod tokens;

use std::sync::Arc;

use gpui::{App, Global, Rgba};

use crate::tokens::ThemeTokens;

struct GlobalTheme(Arc<Theme>);
impl Global for GlobalTheme {}

pub trait ActiveTheme {
    fn theme(&self) -> &Arc<Theme>;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Arc<Theme> {
        &self.global::<GlobalTheme>().0
    }
}

struct MezonThemeSettings {
    ui_font: gpui::Font,
    buffer_font: gpui::Font,
}

impl ::theme::ThemeSettingsProvider for MezonThemeSettings {
    fn ui_font<'a>(&'a self, _cx: &'a App) -> &'a gpui::Font {
        &self.ui_font
    }

    fn buffer_font<'a>(&'a self, _cx: &'a App) -> &'a gpui::Font {
        &self.buffer_font
    }

    fn ui_font_size(&self, _cx: &App) -> gpui::Pixels {
        gpui::px(14.0)
    }

    fn buffer_font_size(&self, _cx: &App) -> gpui::Pixels {
        gpui::px(14.0)
    }

    fn ui_density(&self, _cx: &App) -> ::theme::UiDensity {
        ::theme::UiDensity::Default
    }
}

pub fn init_theme_settings_provider(cx: &mut App) {
    ::theme::set_theme_settings_provider(
        Box::new(MezonThemeSettings {
            ui_font: gpui::font("gg sans"),
            buffer_font: gpui::font("gg sans"),
        }),
        cx,
    );
}

pub fn set_theme(theme: Theme, cx: &mut App) {
    apply_zed_palette(&theme, cx);
    cx.set_global(GlobalTheme(Arc::new(theme)));
}

fn apply_zed_palette(theme: &Theme, cx: &mut App) {
    if !cx.has_global::<::theme::GlobalTheme>() {
        return;
    }
    let mut zed = (**::theme::GlobalTheme::theme(cx)).clone();
    let colors = &mut zed.styles.colors;
    colors.background = theme.bg_primary.into();
    colors.surface_background = theme.bg_secondary.into();
    colors.elevated_surface_background = theme.bg_floating.into();
    colors.panel_background = theme.bg_secondary.into();
    colors.element_background = theme.bg_tertiary.into();
    colors.element_hover = theme.bg_hover.into();
    colors.element_active = theme.bg_hover.into();
    colors.element_selected = theme.brand.into();
    colors.element_disabled = theme.bg_tertiary.into();
    colors.ghost_element_hover = theme.bg_hover.into();
    colors.ghost_element_active = theme.bg_hover.into();
    colors.border = theme.border.into();
    colors.border_variant = theme.border.into();
    colors.border_focused = theme.brand.into();
    colors.text = theme.text_primary.into();
    colors.text_muted = theme.text_secondary.into();
    colors.text_disabled = theme.text_muted.into();
    colors.text_accent = theme.brand.into();

    let scroll = theme.tokens.thread_scroll.into();
    colors.scrollbar_thumb_background = scroll;
    colors.scrollbar_thumb_hover_background = theme.text_muted.into();
    colors.scrollbar_thumb_active_background = theme.text_secondary.into();
    colors.scrollbar_thumb_border = gpui::Hsla::transparent_black();
    colors.scrollbar_track_background = gpui::Hsla::transparent_black();

    let status = &mut zed.styles.status;
    status.error = theme.status_dnd.into();
    status.success = theme.status_online.into();
    status.warning = theme.status_idle.into();
    status.info = theme.text_link.into();

    ::theme::GlobalTheme::update_theme(cx, std::sync::Arc::new(zed));
}

pub fn resolve_theme(theme_name: &str) -> Theme {
    match theme_name {
        "light" => Theme::light(),
        "purple" | "purple_haze" => Theme::purple(),
        "abyss" | "abyss_dark" => Theme::abyss(),
        "red_dark" | "redDark" => Theme::red_dark(),
        "sunrise" => Theme::sunrise(),
        "sunset" => Theme::sunset(),
        "cisher" => Theme::cisher(),
        "berrynade" => Theme::berrynade(),
        _ => Theme::dark(),
    }
}

/// Mezon dark theme color tokens — matching #313338 background (Discord-style dark)
#[derive(Debug, Clone)]
pub struct Theme {
    // Backgrounds
    pub bg_primary: Rgba,   // #313338 — main background
    pub bg_secondary: Rgba, // #2b2d31 — sidebar background
    pub bg_tertiary: Rgba,  // #1e1f22 — clan sidebar background
    pub bg_floating: Rgba,  // #111214 — modals, tooltips
    pub bg_hover: Rgba,     // rgba(255,255,255,0.06) — hover state

    // Text
    pub text_primary: Rgba,   // #f2f3f5 — primary text
    pub text_secondary: Rgba, // #b5bac1 — muted text
    pub text_muted: Rgba,     // #80848e — very muted
    pub text_link: Rgba,      // #00aff4 — links

    // Interactive
    pub interactive_normal: Rgba, // #b5bac1
    pub interactive_hover: Rgba,  // #dbdee1
    pub interactive_active: Rgba, // #f2f3f5

    // Brand / accent
    pub brand: Rgba,       // #5865f2 — brand purple (Mezon accent)
    pub brand_hover: Rgba, // #4752c4

    // Destructive actions (React colorDanger / menu danger labels)
    pub danger: Rgba,          // #DA363C
    pub danger_text: Rgba,     // #E13542
    pub danger_hover_bg: Rgba, // #f67e882a

    // Status
    pub status_online: Rgba,  // #23a55a
    pub status_idle: Rgba,    // #f0b232
    pub status_dnd: Rgba,     // #E1024F
    pub status_offline: Rgba, // #80848e

    // Unread / notification
    pub unread_dot: Rgba,    // #f2f3f5
    pub mention_badge: Rgba, // #DA373C

    // Borders
    pub border: Rgba, // rgba(255,255,255,0.08)

    // Title bar
    pub title_bar_bg: Rgba, // #1e1f22

    pub tokens: ThemeTokens,
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self::from_react("dark")
    }

    pub fn light() -> Self {
        Self::from_react("light")
    }

    pub fn purple() -> Self {
        Self::from_react("purple_haze")
    }

    pub fn abyss() -> Self {
        Self::from_react("abyss_dark")
    }

    pub fn red_dark() -> Self {
        Self::from_react("redDark")
    }

    fn dark_base() -> Self {
        Self {
            bg_primary: rgba(49, 51, 56, 1.0),
            bg_secondary: rgba(43, 45, 49, 1.0),
            bg_tertiary: rgba(30, 31, 34, 1.0),
            bg_floating: rgba(17, 18, 20, 1.0),
            bg_hover: rgba(255, 255, 255, 0.06),

            text_primary: rgba(242, 243, 245, 1.0),
            text_secondary: rgba(181, 186, 193, 1.0),
            text_muted: rgba(128, 132, 142, 1.0),
            text_link: rgba(0, 175, 244, 1.0),

            interactive_normal: rgba(181, 186, 193, 1.0),
            interactive_hover: rgba(219, 222, 225, 1.0),
            interactive_active: rgba(242, 243, 245, 1.0),

            brand: rgba(82, 101, 236, 1.0),
            brand_hover: rgba(71, 82, 196, 1.0),

            danger: rgba(218, 54, 60, 1.0),
            danger_text: rgba(225, 53, 66, 1.0),
            danger_hover_bg: rgba(246, 126, 136, 42.0 / 255.0),

            status_online: rgba(35, 165, 90, 1.0),
            status_idle: rgba(240, 178, 50, 1.0),
            status_dnd: rgba(225, 2, 79, 1.0),
            status_offline: rgba(128, 132, 142, 1.0),

            unread_dot: rgba(242, 243, 245, 1.0),
            mention_badge: rgba(218, 55, 60, 1.0),

            border: rgba(100, 100, 100, 0.4),
            title_bar_bg: rgba(30, 31, 34, 1.0),
            tokens: ThemeTokens::for_theme("dark"),
        }
    }

    fn light_base() -> Self {
        Self {
            bg_primary: rgba(255, 255, 255, 1.0),
            bg_secondary: rgba(242, 243, 245, 1.0),
            bg_tertiary: rgba(227, 229, 232, 1.0),
            bg_floating: rgba(255, 255, 255, 1.0),
            bg_hover: rgba(0, 0, 0, 0.06),

            text_primary: rgba(6, 6, 7, 1.0),
            text_secondary: rgba(79, 84, 92, 1.0),
            text_muted: rgba(128, 132, 142, 1.0),
            text_link: rgba(0, 103, 224, 1.0),

            interactive_normal: rgba(79, 84, 92, 1.0),
            interactive_hover: rgba(43, 45, 49, 1.0),
            interactive_active: rgba(6, 6, 7, 1.0),

            brand: rgba(82, 101, 236, 1.0),
            brand_hover: rgba(71, 82, 196, 1.0),

            danger: rgba(218, 54, 60, 1.0),
            danger_text: rgba(225, 53, 66, 1.0),
            danger_hover_bg: rgba(246, 126, 136, 42.0 / 255.0),

            status_online: rgba(35, 165, 90, 1.0),
            status_idle: rgba(240, 178, 50, 1.0),
            status_dnd: rgba(225, 2, 79, 1.0),
            status_offline: rgba(128, 132, 142, 1.0),

            unread_dot: rgba(6, 6, 7, 1.0),
            mention_badge: rgba(218, 55, 60, 1.0),

            border: rgba(218, 220, 224, 1.0),
            title_bar_bg: rgba(227, 229, 232, 1.0),
            tokens: ThemeTokens::for_theme("light"),
        }
    }

    fn from_tokens(mut base: Theme, t: ThemeTokens) -> Theme {
        base.bg_primary = t.bg_secondary;
        base.bg_secondary = t.bg_theme_direct_message;
        base.bg_tertiary = t.bg_primary;
        base.bg_floating = t.bg_tooltip_app;
        base.bg_hover = t.bg_item_hover;
        base.brand = t.button_theme_primary;
        base.brand_hover = t.bg_button_primary_hover;
        base.border = t.border_primary;
        base.text_link = t.color_mention_hover;
        base.title_bar_bg = t.bg_primary;
        base.tokens = t;
        base
    }

    fn from_react(name: &str) -> Theme {
        let t = ThemeTokens::for_theme(name);
        let bg = t.bg_secondary;
        let luminance = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
        let base = if luminance > 0.5 {
            Theme::light_base()
        } else {
            Theme::dark_base()
        };
        Self::from_tokens(base, t)
    }

    pub fn sunrise() -> Self {
        Self::from_react("sunrise")
    }

    pub fn sunset() -> Self {
        Self::from_react("sunset")
    }

    pub fn cisher() -> Self {
        Self::from_react("cisher")
    }

    pub fn berrynade() -> Self {
        Self::from_react("berrynade")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_tracks_react(theme: Theme, token_name: &str) {
        let t = ThemeTokens::for_theme(token_name);
        assert_eq!(theme.bg_primary, t.bg_secondary);
        assert_eq!(theme.bg_secondary, t.bg_theme_direct_message);
        assert_eq!(theme.bg_tertiary, t.bg_primary);
        assert_eq!(theme.border, t.border_primary);
        assert_eq!(theme.brand, t.button_theme_primary);
        assert_eq!(theme.title_bar_bg, t.bg_primary);
    }

    #[test]
    fn semantic_fields_track_react_tokens() {
        assert_tracks_react(Theme::dark(), "dark");
        assert_tracks_react(Theme::light(), "light");
        assert_tracks_react(Theme::purple(), "purple_haze");
        assert_tracks_react(Theme::abyss(), "abyss_dark");
        assert_tracks_react(Theme::red_dark(), "redDark");
    }

    #[test]
    fn dark_text_hierarchy_not_inverted() {
        let t = Theme::dark();
        let lum = |c: Rgba| c.r + c.g + c.b;
        assert!(lum(t.text_primary) > lum(t.text_secondary));
        assert!(lum(t.text_secondary) > lum(t.text_muted));
    }
}
