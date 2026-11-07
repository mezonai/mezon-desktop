use gpui::{App, Hsla};

use crate::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MzAnim {
    None,
    Hop,
    Ring,
    Swing,
    Spin { turn_deg: u16 },
    Zoom,
    Typing,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MzIcon {
    pub id: &'static str,
    pub size: u16,
    pub anim: MzAnim,
    pub(crate) asset: &'static str,
}

impl MzIcon {
    pub const fn path(self) -> &'static str {
        self.asset
    }

    pub const fn part_paths(self) -> &'static [&'static str] {
        &[]
    }
}

pub type MzIconElement = Icon;

#[track_caller]
pub fn mz_icon(icon: MzIcon) -> MzIconElement {
    Icon::from_mz(icon)
}

pub fn mz_motion_enabled(cx: &App) -> bool {
    crate::IconMotionPreferences::enabled(cx)
}

impl Icon {
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        use gpui::Styled;
        self.svg.style().text.color = Some(color.into());
        self.preserve_color = true;
        self
    }
}

pub(crate) fn standard_size(size: f32) -> u16 {
    [16_u16, 20, 24, 32, 40, 48]
        .into_iter()
        .min_by(|a, b| {
            (size - f32::from(*a))
                .abs()
                .total_cmp(&(size - f32::from(*b)).abs())
                .then_with(|| b.cmp(a))
        })
        .unwrap_or(16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mz;

    #[test]
    fn path_is_size_folder() {
        assert_eq!(mz::BELL_20.path(), "icons/mz/20/bell.svg");
    }

    #[test]
    fn sizes_snap_with_ties_up() {
        for (input, expected) in [
            (12., 16),
            (18., 20),
            (22., 24),
            (26., 24),
            (28., 32),
            (36., 40),
        ] {
            assert_eq!(standard_size(input), expected);
        }
    }
}
