use gpui::{prelude::*, radians, svg, Animation, AnimationExt, Rgba, Transformation};
/// Spinner — animated loading ring using GPUI's animation element.
use std::time::Duration;

pub struct Spinner {
    size: u16,
    color: Option<Rgba>,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            size: 20,
            color: None,
        }
    }

    pub fn size(mut self, size: u16) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn render(self) -> impl IntoElement {
        let sz = gpui::px(self.size as f32);
        let color = self.color.unwrap_or(Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        });

        const SPINNER_SVG: &str = r#"<svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" d="M12 4a8 8 0 0 1 7.938 7H21.95A10 10 0 1 0 12 22v-2a8 8 0 0 1 0-16Z"/></svg>"#;

        svg()
            .path(SPINNER_SVG)
            .size(sz)
            .text_color(color)
            .with_animation(
                "spinner-rotation",
                Animation::new(Duration::from_millis(700)).repeat(),
                |el, progress| {
                    el.with_transformation(Transformation::rotate(radians(
                        progress * std::f32::consts::TAU,
                    )))
                },
            )
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}
