use crate::theme::Theme;

pub fn theme_is_light(theme: &Theme) -> bool {
    let bg = theme.bg_primary;
    0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b > 0.5
}
