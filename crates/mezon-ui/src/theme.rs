use gpui::Rgba;

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

    // Status
    pub status_online: Rgba,  // #23a55a
    pub status_idle: Rgba,    // #f0b232
    pub status_dnd: Rgba,     // #f23f43
    pub status_offline: Rgba, // #80848e

    // Unread / notification
    pub unread_dot: Rgba,    // #f2f3f5
    pub mention_badge: Rgba, // #f23f43

    // Borders
    pub border: Rgba, // rgba(255,255,255,0.08)

    // Title bar
    pub title_bar_bg: Rgba, // #1e1f22
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

            brand: rgba(88, 101, 242, 1.0),
            brand_hover: rgba(71, 82, 196, 1.0),

            status_online: rgba(35, 165, 90, 1.0),
            status_idle: rgba(240, 178, 50, 1.0),
            status_dnd: rgba(242, 63, 67, 1.0),
            status_offline: rgba(128, 132, 142, 1.0),

            unread_dot: rgba(242, 243, 245, 1.0),
            mention_badge: rgba(242, 63, 67, 1.0),

            border: rgba(255, 255, 255, 0.08),
            title_bar_bg: rgba(30, 31, 34, 1.0),
        }
    }

    pub fn light() -> Self {
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

            brand: rgba(88, 101, 242, 1.0),
            brand_hover: rgba(71, 82, 196, 1.0),

            status_online: rgba(35, 165, 90, 1.0),
            status_idle: rgba(240, 178, 50, 1.0),
            status_dnd: rgba(242, 63, 67, 1.0),
            status_offline: rgba(128, 132, 142, 1.0),

            unread_dot: rgba(6, 6, 7, 1.0),
            mention_badge: rgba(242, 63, 67, 1.0),

            border: rgba(0, 0, 0, 0.08),
            title_bar_bg: rgba(227, 229, 232, 1.0),
        }
    }
}
