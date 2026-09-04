use gpui::{
    Background, Fill, Hsla, LinearColorStop, MAX_GRADIENT_RAMP_STOPS, Rgba, linear_color_stop,
    linear_gradient_multi,
};

use crate::surfaces::surface_gradients;
use crate::tokens::ThemeTokens;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub color: Rgba,
    pub position: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGradient {
    pub angle: f32,
    pub stops: &'static [GradientStop],
    pub overlay: Option<Rgba>,
    pub base: Option<Rgba>,
    pub viewport_anchored: bool,
}

impl SurfaceGradient {
    fn composite(&self, color: Rgba) -> Rgba {
        let mut out = color;
        if let Some(base) = self.base {
            out = source_over(out, base);
        }
        if let Some(overlay) = self.overlay {
            out = source_over(overlay, out);
        }
        out
    }
}

pub(crate) struct SurfaceGradients {
    pub(crate) primary: Option<SurfaceGradient>,
    pub(crate) secondary: Option<SurfaceGradient>,
    pub(crate) surface: Option<SurfaceGradient>,
    pub(crate) direct_message: Option<SurfaceGradient>,
    pub(crate) input_primary: Option<SurfaceGradient>,
    pub(crate) active_friend_list: Option<SurfaceGradient>,
    pub(crate) modal_search: Option<SurfaceGradient>,
    pub(crate) outside_footer: Option<SurfaceGradient>,
    pub(crate) footer: Option<SurfaceGradient>,
}

impl SurfaceGradients {
    pub(crate) const NONE: Self = Self {
        primary: None,
        secondary: None,
        surface: None,
        direct_message: None,
        input_primary: None,
        active_friend_list: None,
        modal_search: None,
        outside_footer: None,
        footer: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeSurface {
    pub solid: Rgba,
    pub gradient: Option<SurfaceGradient>,
    background: Background,
}

impl ThemeSurface {
    pub fn new(solid: Rgba, gradient: Option<SurfaceGradient>) -> Self {
        let background = Self::build_background(solid, gradient);
        Self {
            solid,
            gradient,
            background,
        }
    }

    pub fn from_solid(color: Rgba) -> Self {
        Self::new(color, None)
    }

    pub fn viewport_anchored(&self) -> bool {
        self.gradient
            .is_some_and(|gradient| gradient.viewport_anchored)
    }

    pub fn fill(&self) -> Background {
        match self.gradient {
            Some(gradient) if !gradient.viewport_anchored => self.background,
            _ => Background::from(Hsla::from(self.solid)),
        }
    }

    pub fn ramp(&self) -> Background {
        self.background
    }

    fn build_background(solid: Rgba, gradient: Option<SurfaceGradient>) -> Background {
        let Some(gradient) = gradient else {
            return Background::from(Hsla::from(solid));
        };
        let count = gradient.stops.len().min(MAX_GRADIENT_RAMP_STOPS);
        let Some(first) = gradient.stops.first() else {
            return Background::from(Hsla::from(solid));
        };
        if count < 2 {
            return Background::from(Hsla::from(gradient.composite(first.color)));
        }

        let mut stops = [LinearColorStop::default(); MAX_GRADIENT_RAMP_STOPS];
        for (slot, stop) in gradient.stops.iter().take(count).enumerate() {
            stops[slot] = linear_color_stop(gradient.composite(stop.color), stop.position);
        }
        let background = linear_gradient_multi(gradient.angle, &stops[..count]);
        if gradient.viewport_anchored {
            background.viewport_anchored()
        } else {
            background
        }
    }
}

impl From<ThemeSurface> for Background {
    fn from(surface: ThemeSurface) -> Self {
        surface.fill()
    }
}

impl From<ThemeSurface> for Fill {
    fn from(surface: ThemeSurface) -> Self {
        Self::Color(surface.fill())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeSurfaces {
    pub primary: ThemeSurface,
    pub secondary: ThemeSurface,
    pub surface: ThemeSurface,
    pub direct_message: ThemeSurface,
    pub input_primary: ThemeSurface,
    pub active_friend_list: ThemeSurface,
    pub modal_search: ThemeSurface,
    pub outside_footer: ThemeSurface,
    pub footer: ThemeSurface,
}

impl ThemeSurfaces {
    pub fn solid(tokens: &ThemeTokens) -> Self {
        Self {
            primary: ThemeSurface::from_solid(tokens.bg_primary),
            secondary: ThemeSurface::from_solid(tokens.bg_secondary),
            surface: ThemeSurface::from_solid(tokens.bg_surface),
            direct_message: ThemeSurface::from_solid(tokens.bg_theme_direct_message),
            input_primary: ThemeSurface::from_solid(tokens.bg_theme_input_primary),
            active_friend_list: ThemeSurface::from_solid(tokens.bg_active_friend_list),
            modal_search: ThemeSurface::from_solid(tokens.bg_modal_theme_search),
            outside_footer: ThemeSurface::from_solid(tokens.bg_outside_footer),
            footer: ThemeSurface::from_solid(tokens.bg_footer),
        }
    }

    pub fn for_theme(theme: &str, tokens: &ThemeTokens) -> Self {
        let gradients = surface_gradients(theme);
        Self {
            primary: ThemeSurface::new(tokens.bg_primary, gradients.primary),
            secondary: ThemeSurface::new(tokens.bg_secondary, gradients.secondary),
            surface: ThemeSurface::new(tokens.bg_surface, gradients.surface),
            direct_message: ThemeSurface::new(
                tokens.bg_theme_direct_message,
                gradients.direct_message,
            ),
            input_primary: ThemeSurface::new(
                tokens.bg_theme_input_primary,
                gradients.input_primary,
            ),
            active_friend_list: ThemeSurface::new(
                tokens.bg_active_friend_list,
                gradients.active_friend_list,
            ),
            modal_search: ThemeSurface::new(tokens.bg_modal_theme_search, gradients.modal_search),
            outside_footer: ThemeSurface::new(tokens.bg_outside_footer, gradients.outside_footer),
            footer: ThemeSurface::new(tokens.bg_footer, gradients.footer),
        }
    }
}

fn source_over(top: Rgba, bottom: Rgba) -> Rgba {
    let alpha = top.a + bottom.a * (1.0 - top.a);
    if alpha <= f32::EPSILON {
        return Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
    }
    let blend = |top_channel: f32, bottom_channel: f32| {
        (top_channel * top.a + bottom_channel * bottom.a * (1.0 - top.a)) / alpha
    };
    Rgba {
        r: blend(top.r, bottom.r),
        g: blend(top.g, bottom.g),
        b: blend(top.b, bottom.b),
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
        Rgba { r, g, b, a }
    }

    const fn stop(color: Rgba, position: f32) -> GradientStop {
        GradientStop { color, position }
    }

    const RED_TO_BLUE: &[GradientStop] = &[
        stop(rgba(1.0, 0.0, 0.0, 1.0), 0.0),
        stop(rgba(0.0, 0.0, 1.0, 1.0), 1.0),
    ];

    const TRANSLUCENT_WHITE: &[GradientStop] = &[stop(rgba(1.0, 1.0, 1.0, 0.5), 0.0)];

    const THREE_STOPS: &[GradientStop] = &[
        stop(rgba(1.0, 0.0, 0.0, 1.0), 0.0),
        stop(rgba(0.0, 1.0, 0.0, 1.0), 0.5),
        stop(rgba(0.0, 0.0, 1.0, 1.0), 1.0),
    ];

    const REACT_GRADIENT_THEMES: [&str; 7] = [
        "sunrise",
        "purple_haze",
        "redDark",
        "abyss_dark",
        "berrynade",
        "cisher",
        "sunset",
    ];

    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 0.0005
    }

    #[test]
    fn overlay_composites_over_gradient_stop() {
        let gradient = SurfaceGradient {
            angle: 0.0,
            stops: RED_TO_BLUE,
            overlay: Some(rgba(0.0, 0.0, 0.0, 0.5)),
            base: None,
            viewport_anchored: false,
        };
        let composited = gradient.composite(RED_TO_BLUE[0].color);
        assert!(close(composited.r, 0.5), "{composited:?}");
        assert!(close(composited.a, 1.0), "{composited:?}");
    }

    #[test]
    fn translucent_stop_composites_over_base() {
        let gradient = SurfaceGradient {
            angle: 0.0,
            stops: TRANSLUCENT_WHITE,
            overlay: None,
            base: Some(rgba(0.0, 0.0, 0.0, 1.0)),
            viewport_anchored: false,
        };
        let composited = gradient.composite(TRANSLUCENT_WHITE[0].color);
        assert!(close(composited.r, 0.5), "{composited:?}");
        assert!(close(composited.a, 1.0), "{composited:?}");
    }

    #[test]
    fn every_stop_survives_the_ramp() {
        let surface = ThemeSurface::new(
            rgba(0.0, 0.0, 0.0, 1.0),
            Some(SurfaceGradient {
                angle: 90.0,
                stops: THREE_STOPS,
                overlay: None,
                base: None,
                viewport_anchored: false,
            }),
        );
        let ramp = surface.ramp();
        assert!(ramp.as_solid().is_none());
        assert_eq!(ramp, surface.fill());
        assert_ne!(
            ramp,
            linear_gradient_multi(
                90.0,
                &[
                    linear_color_stop(THREE_STOPS[0].color, THREE_STOPS[0].position),
                    linear_color_stop(THREE_STOPS[2].color, THREE_STOPS[2].position),
                ]
            ),
            "the middle stop must not be dropped"
        );
    }

    #[test]
    fn solid_surface_stays_solid() {
        let surface = ThemeSurface::from_solid(rgba(0.1, 0.2, 0.3, 1.0));
        assert_eq!(surface.fill().as_solid(), Some(Hsla::from(surface.solid)));
        assert_eq!(surface.ramp().as_solid(), Some(Hsla::from(surface.solid)));
    }

    #[test]
    fn viewport_anchored_surface_falls_back_to_solid_fill() {
        let surface = ThemeSurface::new(
            rgba(0.1, 0.2, 0.3, 1.0),
            Some(SurfaceGradient {
                angle: 154.19,
                stops: RED_TO_BLUE,
                overlay: None,
                base: None,
                viewport_anchored: true,
            }),
        );
        assert_eq!(surface.fill().as_solid(), Some(Hsla::from(surface.solid)));
        assert!(surface.ramp().as_solid().is_none());
    }

    #[test]
    fn react_themes_expose_gradient_surfaces() {
        for theme in REACT_GRADIENT_THEMES {
            let tokens = ThemeTokens::for_theme(theme);
            let surfaces = ThemeSurfaces::for_theme(theme, &tokens);
            assert!(
                surfaces.primary.gradient.is_some(),
                "{theme} should carry a primary gradient"
            );
            assert!(
                surfaces.secondary.gradient.is_some(),
                "{theme} should carry a secondary gradient"
            );
        }
    }

    #[test]
    fn composited_stops_average_to_the_flattened_token() {
        let mut worst = 0.0f32;
        let mut worst_label = String::new();
        for theme in REACT_GRADIENT_THEMES {
            let tokens = ThemeTokens::for_theme(theme);
            let surfaces = ThemeSurfaces::for_theme(theme, &tokens);
            for (name, surface) in [
                ("primary", surfaces.primary),
                ("secondary", surfaces.secondary),
                ("surface", surfaces.surface),
                ("direct_message", surfaces.direct_message),
                ("input_primary", surfaces.input_primary),
                ("active_friend_list", surfaces.active_friend_list),
                ("modal_search", surfaces.modal_search),
                ("outside_footer", surfaces.outside_footer),
                ("footer", surfaces.footer),
            ] {
                let Some(gradient) = surface.gradient else {
                    continue;
                };
                let count = gradient.stops.len() as f32;
                let mut sum = [0.0f32; 3];
                for stop in gradient.stops {
                    let composited = gradient.composite(stop.color);
                    sum[0] += composited.r;
                    sum[1] += composited.g;
                    sum[2] += composited.b;
                }
                let expected = surface.solid;
                for (channel, total) in [expected.r, expected.g, expected.b].iter().zip(sum) {
                    let delta = (total / count - channel).abs();
                    if delta > worst {
                        worst = delta;
                        worst_label = format!("{theme}.{name}");
                    }
                }
            }
        }
        assert!(worst < 0.02, "{worst_label} drifts by {worst}");
    }

    #[test]
    fn flat_themes_stay_solid() {
        for theme in ["dark", "light"] {
            let tokens = ThemeTokens::for_theme(theme);
            let surfaces = ThemeSurfaces::for_theme(theme, &tokens);
            assert!(surfaces.primary.gradient.is_none());
            assert_eq!(
                surfaces.primary.fill().as_solid(),
                Some(Hsla::from(tokens.bg_primary))
            );
        }
    }
}
