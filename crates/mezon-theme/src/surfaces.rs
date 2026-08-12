use gpui::Rgba;

use crate::surface::{GradientStop, SurfaceGradient, SurfaceGradients};

pub(crate) fn surface_gradients(theme: &str) -> SurfaceGradients {
    match theme {
        "sunrise" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: true,
            }),
            secondary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: true,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: true,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.2549,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.56471,
                            b: 0.39216,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65098,
                            g: 0.58431,
                            b: 0.23922,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: true,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: true,
            }),
            footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.72,
                            g: 0.48,
                            b: 0.61276,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.8096,
                            g: 0.71253,
                            b: 0.6304,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.7888,
                            g: 0.75357,
                            b: 0.5712,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: true,
            }),
            ..SurfaceGradients::NONE
        },
        "purple_haze" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.056,
                            g: 0.70513,
                            b: 0.744,
                            a: 1.0,
                        },
                        position: 0.08,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29592,
                            g: 0.046,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.32,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.63383,
                            g: 0.0306,
                            b: 0.6494,
                            a: 1.0,
                        },
                        position: 0.54,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.60061,
                            g: 0.32,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.78,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.125,
                            g: 0.54125,
                            b: 0.875,
                            a: 1.0,
                        },
                        position: 1.0,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: true,
            }),
            secondary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.2944,
                            g: 0.70404,
                            b: 0.7256,
                            a: 1.0,
                        },
                        position: 0.08,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29592,
                            g: 0.046,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.32,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.92304,
                            g: 0.2746,
                            b: 0.9454,
                            a: 1.0,
                        },
                        position: 0.54,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.60061,
                            g: 0.32,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.78,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.2356,
                            g: 0.61444,
                            b: 0.9244,
                            a: 1.0,
                        },
                        position: 1.0,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: true,
            }),
            surface: Some(SurfaceGradient {
                angle: 128.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.0549,
                            g: 0.7098,
                            b: 0.74902,
                            a: 1.0,
                        },
                        position: 0.0394,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29804,
                            g: 0.04706,
                            b: 0.87844,
                            a: 1.0,
                        },
                        position: 0.261,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.63922,
                            g: 0.03137,
                            b: 0.65491,
                            a: 1.0,
                        },
                        position: 0.3982,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.60392,
                            g: 0.3255,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.5689,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.12941,
                            g: 0.54509,
                            b: 0.87843,
                            a: 1.0,
                        },
                        position: 0.7645,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: true,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 128.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.0549,
                            g: 0.7098,
                            b: 0.74902,
                            a: 1.0,
                        },
                        position: 0.0394,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29804,
                            g: 0.04706,
                            b: 0.87843,
                            a: 1.0,
                        },
                        position: 0.261,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.63922,
                            g: 0.03137,
                            b: 0.6549,
                            a: 1.0,
                        },
                        position: 0.3982,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.60392,
                            g: 0.32549,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.5689,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.12941,
                            g: 0.5451,
                            b: 0.87843,
                            a: 1.0,
                        },
                        position: 0.7645,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: false,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.056,
                            g: 0.70513,
                            b: 0.744,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29592,
                            g: 0.046,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.63383,
                            g: 0.0306,
                            b: 0.6494,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.056,
                            g: 0.70513,
                            b: 0.744,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29592,
                            g: 0.046,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.63383,
                            g: 0.0306,
                            b: 0.6494,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: true,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.2944,
                            g: 0.70404,
                            b: 0.7256,
                            a: 1.0,
                        },
                        position: 0.08,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.29592,
                            g: 0.046,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.32,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.92304,
                            g: 0.2746,
                            b: 0.9454,
                            a: 1.0,
                        },
                        position: 0.54,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.60061,
                            g: 0.32,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.78,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.2356,
                            g: 0.61444,
                            b: 0.9244,
                            a: 1.0,
                        },
                        position: 1.0,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: true,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.558,
                            g: 0.7518,
                            b: 0.762,
                            a: 1.0,
                        },
                        position: 0.08,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.5447,
                            g: 0.43,
                            b: 0.81,
                            a: 1.0,
                        },
                        position: 0.32,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.86373,
                            g: 0.566,
                            b: 0.874,
                            a: 1.0,
                        },
                        position: 0.54,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.71275,
                            g: 0.584,
                            b: 0.896,
                            a: 1.0,
                        },
                        position: 0.78,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.556,
                            g: 0.7144,
                            b: 0.844,
                            a: 1.0,
                        },
                        position: 1.0,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.6,
                }),
                base: None,
                viewport_anchored: true,
            }),
            footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.14,
                            g: 0.539,
                            b: 0.56,
                            a: 1.0,
                        },
                        position: 0.08,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.22939,
                            g: 0.0,
                            b: 0.76,
                            a: 1.0,
                        },
                        position: 0.32,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.9056,
                            g: 0.024,
                            b: 0.936,
                            a: 1.0,
                        },
                        position: 0.54,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.50664,
                            g: 0.16,
                            b: 1.0,
                            a: 1.0,
                        },
                        position: 0.78,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.042,
                            g: 0.4578,
                            b: 0.798,
                            a: 1.0,
                        },
                        position: 1.0,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: true,
            }),
        },
        "redDark" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: false,
            }),
            secondary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.1158,
                            g: 0.0042,
                            b: 0.0042,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            surface: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.65,
                        },
                        position: 0.0,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.24706,
                            g: 0.06667,
                            b: 0.06667,
                            a: 0.65,
                        },
                        position: 1.0,
                    },
                ],
                overlay: None,
                base: Some(Rgba {
                    r: 0.1538,
                    g: 0.0462,
                    b: 0.0462,
                    a: 1.0,
                }),
                viewport_anchored: true,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: true,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: true,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.1158,
                            g: 0.0042,
                            b: 0.0042,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.82,
                            g: 0.28,
                            b: 0.28,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.28,
                            g: 0.12,
                            b: 0.12,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.6,
                }),
                base: None,
                viewport_anchored: false,
            }),
            footer: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.4,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.08,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: false,
            }),
        },
        "abyss_dark" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.07843,
                            g: 0.02745,
                            b: 0.18824,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: false,
            }),
            secondary: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.07843,
                            g: 0.02745,
                            b: 0.18824,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            surface: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.2,
                            g: 0.10196,
                            b: 0.41176,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.02745,
                    g: 0.02549,
                    b: 0.02549,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: false,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.24706,
                            g: 0.13333,
                            b: 0.81176,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: false,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.08,
                            g: 0.08,
                            b: 0.32,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.045,
                            g: 0.045,
                            b: 0.055,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.07843,
                            g: 0.02745,
                            b: 0.18824,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 48.17,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.32549,
                            g: 0.28235,
                            b: 0.79216,
                            a: 1.0,
                        },
                        position: 0.1121,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.07843,
                            g: 0.02745,
                            b: 0.18824,
                            a: 1.0,
                        },
                        position: 0.6192,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.08,
                            g: 0.08,
                            b: 0.32,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.045,
                            g: 0.045,
                            b: 0.055,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            ..SurfaceGradients::NONE
        },
        "berrynade" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.68627,
                            g: 0.10196,
                            b: 0.42353,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76078,
                            g: 0.41961,
                            b: 0.12549,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.90588,
                            g: 0.64706,
                            b: 0.1451,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: false,
            }),
            secondary: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.68627,
                            g: 0.10196,
                            b: 0.42353,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76078,
                            g: 0.41961,
                            b: 0.12549,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.90588,
                            g: 0.64706,
                            b: 0.1451,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.68627,
                            g: 0.10196,
                            b: 0.42353,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76078,
                            g: 0.41961,
                            b: 0.12549,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.90588,
                            g: 0.64706,
                            b: 0.1451,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: false,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.68627,
                            g: 0.10196,
                            b: 0.42353,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76078,
                            g: 0.41961,
                            b: 0.12549,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.90588,
                            g: 0.64706,
                            b: 0.1451,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.68627,
                            g: 0.10196,
                            b: 0.42353,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76078,
                            g: 0.41961,
                            b: 0.12549,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.90588,
                            g: 0.64706,
                            b: 0.1451,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 161.03,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.56863,
                            g: 0.07843,
                            b: 0.35294,
                            a: 1.0,
                        },
                        position: 0.1879,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.64314,
                            g: 0.33333,
                            b: 0.09804,
                            a: 1.0,
                        },
                        position: 0.4976,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.78431,
                            g: 0.54902,
                            b: 0.11765,
                            a: 1.0,
                        },
                        position: 0.8072,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: false,
            }),
            footer: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.4,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.08,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.4,
                }),
                base: None,
                viewport_anchored: false,
            }),
            ..SurfaceGradients::NONE
        },
        "cisher" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.95294,
                            g: 0.70196,
                            b: 0.21176,
                            a: 1.0,
                        },
                        position: 0.311,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.93333,
                            g: 0.52157,
                            b: 0.3451,
                            a: 1.0,
                        },
                        position: 0.6709,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: false,
            }),
            secondary: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.95294,
                            g: 0.70196,
                            b: 0.21176,
                            a: 1.0,
                        },
                        position: 0.311,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.93333,
                            g: 0.52157,
                            b: 0.3451,
                            a: 1.0,
                        },
                        position: 0.6709,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.95294,
                            g: 0.70196,
                            b: 0.21176,
                            a: 1.0,
                        },
                        position: 0.311,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.93333,
                            g: 0.52157,
                            b: 0.3451,
                            a: 1.0,
                        },
                        position: 0.6709,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: false,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.25491,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.5647,
                            b: 0.39215,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65099,
                            g: 0.58432,
                            b: 0.23921,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.95294,
                            g: 0.70196,
                            b: 0.21176,
                            a: 1.0,
                        },
                        position: 0.311,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.93333,
                            g: 0.52157,
                            b: 0.3451,
                            a: 1.0,
                        },
                        position: 0.6709,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 180.0,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.95294,
                            g: 0.70196,
                            b: 0.21176,
                            a: 1.0,
                        },
                        position: 0.311,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.93333,
                            g: 0.52157,
                            b: 0.3451,
                            a: 1.0,
                        },
                        position: 0.6709,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.62353,
                            g: 0.2549,
                            b: 0.45882,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.76863,
                            g: 0.56471,
                            b: 0.39216,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.65098,
                            g: 0.58431,
                            b: 0.23922,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.85,
                }),
                base: None,
                viewport_anchored: false,
            }),
            footer: Some(SurfaceGradient {
                angle: 154.19,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.72,
                            g: 0.48,
                            b: 0.61276,
                            a: 1.0,
                        },
                        position: 0.0862,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.8096,
                            g: 0.71253,
                            b: 0.6304,
                            a: 1.0,
                        },
                        position: 0.4807,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.7888,
                            g: 0.75357,
                            b: 0.5712,
                            a: 1.0,
                        },
                        position: 0.7604,
                    },
                ],
                overlay: Some(Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: true,
            }),
            ..SurfaceGradients::NONE
        },
        "sunset" => SurfaceGradients {
            primary: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.7,
                }),
                base: None,
                viewport_anchored: false,
            }),
            secondary: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            surface: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.3451,
                            g: 0.2,
                            b: 0.6549,
                            a: 1.0,
                        },
                        position: 0.0557,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.65,
                }),
                base: None,
                viewport_anchored: false,
            }),
            direct_message: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.75,
                }),
                base: None,
                viewport_anchored: false,
            }),
            input_primary: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.58428,
                            g: 0.03532,
                            b: 0.03532,
                            a: 1.0,
                        },
                        position: 0.1617,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.72,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.5,
                }),
                base: None,
                viewport_anchored: true,
            }),
            active_friend_list: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            modal_search: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            outside_footer: Some(SurfaceGradient {
                angle: 141.68,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.28235,
                            g: 0.15686,
                            b: 0.54902,
                            a: 1.0,
                        },
                        position: 0.2757,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.85882,
                            g: 0.49804,
                            b: 0.29412,
                            a: 1.0,
                        },
                        position: 0.7125,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.8,
                }),
                base: None,
                viewport_anchored: false,
            }),
            footer: Some(SurfaceGradient {
                angle: 64.92,
                stops: &[
                    GradientStop {
                        color: Rgba {
                            r: 0.4,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.4,
                    },
                    GradientStop {
                        color: Rgba {
                            r: 0.08,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                        position: 0.9,
                    },
                ],
                overlay: Some(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.9,
                }),
                base: None,
                viewport_anchored: false,
            }),
        },
        _ => SurfaceGradients::NONE,
    }
}
