use gpui::{AnyElement, Pixels, Rgba, div, prelude::*, px, rgba};
use mezon_store::{DmAvatarPresence, UserPresence};

use crate::components::primitives::{Icon, IconName};
use crate::theme::Theme;

pub const PRESENCE_DOT_SIZE: Pixels = px(12.);
const PRESENCE_IDLE_ICON_SIZE: Pixels = px(10.);

pub fn presence_badge_color(presence: DmAvatarPresence) -> Option<Rgba> {
    match presence {
        DmAvatarPresence::None => None,
        DmAvatarPresence::Online => Some(rgba(0x22c55eff)),
        DmAvatarPresence::Dnd => Some(rgba(0xef4444ff)),
        DmAvatarPresence::Idle => Some(rgba(0xf0b232ff)),
    }
}

/// The presence dot drawn over a DM avatar: a filled circle for online/dnd and
/// the crescent glyph for idle, nothing when the peer reads as offline. Expects
/// a `relative()` parent sized to the avatar; `surface` is the background the
/// avatar sits on, so the dot cuts a ring out of it.
pub fn presence_badge_element(
    presence: DmAvatarPresence,
    surface: Rgba,
    theme: &Theme,
) -> Option<AnyElement> {
    match presence {
        DmAvatarPresence::None => None,
        DmAvatarPresence::Online | DmAvatarPresence::Dnd => {
            let fill = presence_badge_color(presence).unwrap_or(theme.status_online);
            Some(
                div()
                    .absolute()
                    .bottom(px(-1.))
                    .right(px(-1.))
                    .size(PRESENCE_DOT_SIZE)
                    .rounded_full()
                    .border_2()
                    .border_color(surface)
                    .bg(fill)
                    .into_any_element(),
            )
        }
        DmAvatarPresence::Idle => Some(
            div()
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .size(PRESENCE_DOT_SIZE)
                .flex()
                .items_end()
                .justify_end()
                .child(
                    Icon::new(IconName::DarkModeIcon)
                        .size(PRESENCE_IDLE_ICON_SIZE)
                        .with_transformation(gpui::Transformation::rotate(gpui::radians(
                            -std::f32::consts::FRAC_PI_2,
                        )))
                        .text_color(
                            presence_badge_color(DmAvatarPresence::Idle)
                                .unwrap_or(theme.status_idle),
                        ),
                )
                .into_any_element(),
        ),
    }
}

pub fn status_icon(presence: UserPresence) -> IconName {
    match presence {
        UserPresence::Online => IconName::OnlineStatus,
        UserPresence::Idle => IconName::DarkModeIcon,
        UserPresence::Dnd => IconName::MinusCircleIcon,
        UserPresence::Invisible => IconName::OfflineStatus,
    }
}

pub fn status_color(presence: UserPresence, theme: &Theme) -> Rgba {
    match presence {
        UserPresence::Online => theme.status_online,
        UserPresence::Idle => theme.status_idle,
        UserPresence::Dnd => theme.status_dnd,
        UserPresence::Invisible => theme.status_offline,
    }
}

pub fn status_icon_and_color(status: &str, theme: &Theme) -> (IconName, Rgba) {
    let presence = UserPresence::from_status(status);
    (status_icon(presence), status_color(presence, theme))
}

pub fn status_label_key(presence: UserPresence) -> &'static str {
    match presence {
        UserPresence::Online => "userProfile.statusProfile.statusOptions.online",
        UserPresence::Idle => "userProfile.statusProfile.statusOptions.idle",
        UserPresence::Dnd => "userProfile.statusProfile.statusOptions.doNotDisturb",
        UserPresence::Invisible => "userProfile.statusProfile.statusOptions.invisible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [UserPresence; 4] = [
        UserPresence::Online,
        UserPresence::Idle,
        UserPresence::Dnd,
        UserPresence::Invisible,
    ];

    const LOCALES: [&str; 11] = [
        "en", "vi", "ru", "es", "tt", "de", "it", "pt", "jpn", "kr", "swe",
    ];

    fn themes() -> Vec<Theme> {
        vec![
            Theme::dark(),
            Theme::light(),
            Theme::purple(),
            Theme::abyss(),
            Theme::red_dark(),
        ]
    }

    fn rgba_bits(c: Rgba) -> (u32, u32, u32, u32) {
        (c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits())
    }

    #[test]
    fn every_wire_spelling_resolves_to_the_same_indicator() {
        let theme = Theme::dark();
        let spellings = [
            ("Online", UserPresence::Online),
            ("online", UserPresence::Online),
            ("", UserPresence::Online),
            ("Idle", UserPresence::Idle),
            ("idle", UserPresence::Idle),
            ("IDLE", UserPresence::Idle),
            ("Do Not Disturb", UserPresence::Dnd),
            ("do not disturb", UserPresence::Dnd),
            ("dnd", UserPresence::Dnd),
            ("DND", UserPresence::Dnd),
            ("Invisible", UserPresence::Invisible),
            ("invisible", UserPresence::Invisible),
            ("offline", UserPresence::Invisible),
            (" Idle ", UserPresence::Idle),
        ];
        for (raw, expected) in spellings {
            let resolved = UserPresence::from_status(raw);
            assert_eq!(resolved, expected, "classifying {raw:?}");
            assert!(
                status_icon(resolved) == status_icon(expected),
                "icon for {raw:?}"
            );
            assert_eq!(
                rgba_bits(status_color(resolved, &theme)),
                rgba_bits(status_color(expected, &theme)),
                "color for {raw:?}"
            );
            assert_eq!(
                status_label_key(resolved),
                status_label_key(expected),
                "label for {raw:?}"
            );
        }
    }

    #[test]
    fn each_status_has_a_distinct_color_in_every_theme() {
        for theme in themes() {
            let mut seen = Vec::new();
            for presence in ALL {
                let bits = rgba_bits(status_color(presence, &theme));
                assert!(
                    !seen.contains(&bits),
                    "{presence:?} shares a color with another status"
                );
                seen.push(bits);
            }
        }
    }

    #[test]
    fn only_invisible_renders_the_offline_color() {
        for theme in themes() {
            for presence in ALL {
                let is_offline_color =
                    rgba_bits(status_color(presence, &theme)) == rgba_bits(theme.status_offline);
                assert_eq!(
                    is_offline_color,
                    presence == UserPresence::Invisible,
                    "{presence:?} offline-color mismatch"
                );
            }
        }
    }

    #[test]
    fn each_status_has_a_distinct_icon() {
        let mut seen = Vec::new();
        for presence in ALL {
            let icon = status_icon(presence);
            assert!(!seen.contains(&icon), "{presence:?} shares an icon");
            seen.push(icon);
        }
    }

    #[test]
    fn status_labels_are_translated_in_every_locale() {
        for locale in LOCALES {
            for presence in ALL {
                let key = status_label_key(presence);
                let label = mezon_i18n::t(locale, key);
                assert_ne!(label, key, "{locale} is missing {key}");
                assert!(!label.is_empty(), "{locale} has an empty {key}");
            }
        }
    }

    #[test]
    fn visible_statuses_are_the_three_non_invisible_ones() {
        for presence in ALL {
            assert_eq!(
                presence.is_visible(),
                presence != UserPresence::Invisible,
                "{presence:?} visibility"
            );
        }
    }
}
