use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Hsla, Pixels, Transformation, Window, div, ease_in_out,
    percentage, prelude::*, px, svg,
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
        Size::XSmall => px(12.),
        Size::Small => px(16.),
        Size::Medium => px(20.),
        Size::Large => px(28.),
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self
            .color
            .unwrap_or_else(|| cx.theme().text_secondary.into());
        let icon = svg()
            .path("icons/loading-spinner.svg")
            .size(diameter(self.size))
            .flex_none()
            .text_color(color)
            .with_animation(
                "spinner",
                Animation::new(Duration::from_secs_f64(0.8))
                    .repeat()
                    .with_easing(ease_in_out),
                |this, delta| this.with_transformation(Transformation::rotate(percentage(delta))),
            );

        div().child(icon)
    }
}
