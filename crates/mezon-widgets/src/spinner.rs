use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Hsla, Pixels, Transformation, Window, div, ease_in_out,
    percentage, prelude::*, px,
};

use super::sizing::{Sizable, Size};
use mezon_theme::ActiveTheme;

#[derive(IntoElement)]
pub struct Spinner {
    size: Size,
    color: Option<Hsla>,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            size: Size::Medium,
            color: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Sizable for Spinner {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

fn diameter(size: Size) -> Pixels {
    match size {
        Size::XSmall => px(16.),
        Size::Small => px(16.),
        Size::Medium => px(20.),
        Size::Large => px(32.),
    }
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self
            .color
            .unwrap_or_else(|| cx.theme().tokens.icon_normal().into());
        let icon = crate::Icon::new(crate::IconName::Spinner)
            .motion_enabled(false)
            .size(diameter(self.size))
            .flex_none()
            .text_color(color);

        if !window.is_window_active() {
            return div().child(icon.into_any_element());
        }

        div().child(
            icon.with_animation(
                "spinner",
                Animation::new(Duration::from_secs_f64(0.8))
                    .repeat()
                    .with_easing(ease_in_out),
                |this, delta| this.with_transformation(Transformation::rotate(percentage(delta))),
            )
            .into_any_element(),
        )
    }
}
