use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, PixmapRef,
    Stroke, Transform,
};

use super::layout::{RADIUS, TileRect, TileShape, layout_tiles};
use super::text::TextPainter;
use cosmic_text::Weight;

const BACKGROUND: u32 = 0x0b0d10;
const TILE_BACKGROUND: u32 = 0x1e2124;
const SPEAKING: u32 = 0x3ba55d;
const SPEAKING_WIDTH: f32 = 3.0;
const AVATAR_RATIO: f32 = 0.34;
const AVATAR_MIN: f32 = 32.0;

const ACCENTS: [u32; 8] = [
    0x5865f2, 0x3ba55d, 0xfaa61a, 0xed4245, 0xeb459e, 0x00a8fc, 0x9b59b6, 0x1abc9c,
];

pub fn accent_for(seed: &str) -> u32 {
    let mut hash: u32 = 0;
    for byte in seed.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    ACCENTS[(hash % ACCENTS.len() as u32) as usize]
}

pub struct SourceImage<'a> {
    pub bgra: &'a [u8],
    pub width: u32,
    pub height: u32,
}

pub struct DrawTile<'a> {
    pub image: Option<SourceImage<'a>>,
    pub avatar: Option<SourceImage<'a>>,
    pub label: &'a str,
    pub initial: &'a str,
    pub accent: u32,
    pub shape: TileShape,
    pub speaking: bool,
}

pub struct Renderer {
    pixmap: Pixmap,
    scratch: Pixmap,
    source: Vec<u8>,
    output: Vec<u8>,
    text: TextPainter,
}

impl Renderer {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            pixmap: Pixmap::new(width, height)?,
            scratch: Pixmap::new(width, height)?,
            source: Vec::new(),
            output: vec![0u8; width as usize * height as usize * 4],
            text: TextPainter::new(),
        })
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    pub fn render(&mut self, tiles: &[DrawTile<'_>]) -> &[u8] {
        self.pixmap.fill(colour(BACKGROUND));

        let shapes: Vec<TileShape> = tiles.iter().map(|tile| tile.shape).collect();
        let width = self.pixmap.width() as f32;
        let height = self.pixmap.height() as f32;
        let rects = layout_tiles(width, height, &shapes);

        if let Some(focus) = shapes.iter().position(|shape| shape.focused)
            && let Some(Some(rect)) = rects.get(focus)
        {
            self.draw_tile(&tiles[focus], *rect);
        }
        for (index, rect) in rects.iter().enumerate() {
            if shapes.get(index).is_some_and(|shape| shape.focused) {
                continue;
            }
            if let Some(rect) = rect {
                self.draw_tile(&tiles[index], *rect);
            }
        }

        bgra_from_pixmap(self.pixmap.as_ref(), &mut self.output);
        &self.output
    }

    fn draw_tile(&mut self, tile: &DrawTile<'_>, rect: TileRect) {
        let x = rect.x.round();
        let y = rect.y.round();
        let w = rect.w.round();
        let h = rect.h.round();
        if w < 4.0 || h < 4.0 {
            return;
        }

        let Some(path) = rounded_rect(x, y, w, h, RADIUS) else {
            return;
        };

        let paint = Paint {
            anti_alias: true,
            shader: tiny_skia::Shader::SolidColor(colour(TILE_BACKGROUND)),
            ..Paint::default()
        };
        self.pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        match &tile.image {
            Some(image) if image.width > 0 && image.height > 0 && !image.bgra.is_empty() => {
                self.draw_image(image, x, y, w, h, tile.shape.contain, &path);
            }
            _ => match &tile.avatar {
                Some(avatar) => self.draw_avatar(avatar, x, y, w, h),
                None => self.draw_avatar_placeholder(tile.accent, tile.initial, x, y, w, h),
            },
        }

        self.draw_label(tile.label, x, y, w, h);

        if tile.speaking {
            let Some(border) = rounded_rect(
                x + SPEAKING_WIDTH / 2.0,
                y + SPEAKING_WIDTH / 2.0,
                w - SPEAKING_WIDTH,
                h - SPEAKING_WIDTH,
                RADIUS,
            ) else {
                return;
            };
            let stroke_paint = Paint {
                anti_alias: true,
                shader: tiny_skia::Shader::SolidColor(colour(SPEAKING)),
                ..Paint::default()
            };
            let stroke = Stroke {
                width: SPEAKING_WIDTH,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&border, &stroke_paint, &stroke, Transform::identity(), None);
        }
    }

    fn draw_image(
        &mut self,
        image: &SourceImage<'_>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        contain: bool,
        clip: &tiny_skia::Path,
    ) {
        let Some(source) = load_source(&mut self.source, image) else {
            return;
        };
        let fit = if contain {
            (w / image.width as f32).min(h / image.height as f32)
        } else {
            (w / image.width as f32).max(h / image.height as f32)
        };
        let draw_width = image.width as f32 * fit;
        let draw_height = image.height as f32 * fit;
        let offset_x = x + (w - draw_width) / 2.0;
        let offset_y = y + (h - draw_height) / 2.0;

        self.scratch.fill(Color::TRANSPARENT);
        self.scratch.draw_pixmap(
            0,
            0,
            source,
            &PixmapPaint {
                quality: FilterQuality::Bilinear,
                ..PixmapPaint::default()
            },
            Transform::from_scale(fit, fit).post_translate(offset_x, offset_y),
            None,
        );

        let paint = Paint {
            anti_alias: true,
            blend_mode: BlendMode::SourceOver,
            shader: tiny_skia::Pattern::new(
                self.scratch.as_ref(),
                tiny_skia::SpreadMode::Pad,
                FilterQuality::Nearest,
                1.0,
                Transform::identity(),
            ),
            ..Paint::default()
        };
        self.pixmap
            .fill_path(clip, &paint, FillRule::Winding, Transform::identity(), None);
    }

    fn draw_avatar(&mut self, avatar: &SourceImage<'_>, x: f32, y: f32, w: f32, h: f32) {
        let size = AVATAR_MIN.max(w.min(h) * AVATAR_RATIO);
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        let mut builder = PathBuilder::new();
        builder.push_circle(cx, cy, size / 2.0);
        let Some(circle) = builder.finish() else {
            return;
        };
        self.draw_image(
            avatar,
            cx - size / 2.0,
            cy - size / 2.0,
            size,
            size,
            false,
            &circle,
        );
    }

    fn draw_avatar_placeholder(
        &mut self,
        accent: u32,
        initial: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) {
        let size = AVATAR_MIN.max(w.min(h) * AVATAR_RATIO);
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        let mut builder = PathBuilder::new();
        builder.push_circle(cx, cy, size / 2.0);
        let Some(circle) = builder.finish() else {
            return;
        };
        let paint = Paint {
            anti_alias: true,
            shader: tiny_skia::Shader::SolidColor(colour(accent)),
            ..Paint::default()
        };
        self.pixmap.fill_path(
            &circle,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        if initial.is_empty() {
            return;
        }
        let font_size = (size * 0.42).round();
        let text_width = self.text.measure(initial, font_size, Weight::SEMIBOLD);
        self.text.draw(
            &mut self.pixmap,
            initial,
            font_size,
            Weight::SEMIBOLD,
            cx - text_width / 2.0,
            cy + 1.0,
            0xffffff,
        );
    }

    fn draw_label(&mut self, label: &str, x: f32, y: f32, w: f32, h: f32) {
        if label.is_empty() {
            return;
        }
        let font_size = (h * 0.075).round().clamp(11.0, 20.0);
        let pad = (font_size * 0.55).round();
        let max_text_width = w - pad * 4.0;
        if max_text_width <= 0.0 {
            return;
        }
        let text = self
            .text
            .truncate(label, font_size, Weight::MEDIUM, max_text_width);
        let text_width = self.text.measure(&text, font_size, Weight::MEDIUM);
        let pill_height = font_size + pad;
        let pill_y = y + h - pill_height - (pad * 0.7).round();
        let pill_x = x + (pad * 0.7).round();

        if let Some(pill) = rounded_rect(
            pill_x,
            pill_y,
            text_width + pad * 2.0,
            pill_height,
            pill_height / 2.0,
        ) {
            let paint = Paint {
                anti_alias: true,
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(0, 0, 0, 140)),
                ..Paint::default()
            };
            self.pixmap.fill_path(
                &pill,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }

        self.text.draw(
            &mut self.pixmap,
            &text,
            font_size,
            Weight::MEDIUM,
            x + (pad * 1.7).round(),
            pill_y + pill_height / 2.0,
            0xffffff,
        );
    }
}

fn colour(rgb: u32) -> Color {
    Color::from_rgba8(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        255,
    )
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let r = radius.min(w / 2.0).min(h / 2.0);
    let mut builder = PathBuilder::new();
    builder.move_to(x + r, y);
    builder.line_to(x + w - r, y);
    builder.quad_to(x + w, y, x + w, y + r);
    builder.line_to(x + w, y + h - r);
    builder.quad_to(x + w, y + h, x + w - r, y + h);
    builder.line_to(x + r, y + h);
    builder.quad_to(x, y + h, x, y + h - r);
    builder.line_to(x, y + r);
    builder.quad_to(x, y, x + r, y);
    builder.close();
    builder.finish()
}

fn load_source<'a>(buffer: &'a mut Vec<u8>, image: &SourceImage<'_>) -> Option<PixmapRef<'a>> {
    let expected = image.width as usize * image.height as usize * 4;
    if expected == 0 || image.bgra.len() < expected {
        return None;
    }
    if buffer.len() < expected {
        buffer.resize(expected, 0);
    }
    for (destination, source) in buffer[..expected]
        .chunks_exact_mut(4)
        .zip(image.bgra.chunks_exact(4))
    {
        destination[0] = source[2];
        destination[1] = source[1];
        destination[2] = source[0];
        destination[3] = 255;
    }
    PixmapRef::from_bytes(&buffer[..expected], image.width, image.height)
}

fn bgra_from_pixmap(pixmap: PixmapRef<'_>, out: &mut [u8]) {
    for (pixel, chunk) in pixmap.pixels().iter().zip(out.chunks_exact_mut(4)) {
        let demultiplied = pixel.demultiply();
        chunk[0] = demultiplied.blue();
        chunk[1] = demultiplied.green();
        chunk[2] = demultiplied.red();
        chunk[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
        colour
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect()
    }

    fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let index = (y as usize * width as usize + x as usize) * 4;
        [
            frame[index],
            frame[index + 1],
            frame[index + 2],
            frame[index + 3],
        ]
    }

    #[test]
    fn an_empty_scene_renders_the_background() {
        let mut renderer = Renderer::new(320, 180).expect("renderer");
        let frame = renderer.render(&[]).to_vec();
        assert_eq!(frame.len(), 320 * 180 * 4);
        assert_eq!(pixel(&frame, 320, 5, 5), [0x10, 0x0d, 0x0b, 255]);
    }

    #[test]
    fn a_tile_without_video_draws_its_accent_circle() {
        let mut renderer = Renderer::new(320, 180).expect("renderer");
        let frame = renderer
            .render(&[DrawTile {
                image: None,
                avatar: None,
                label: "",
                initial: "",
                accent: 0xff0000,
                shape: TileShape {
                    focused: false,
                    contain: false,
                },
                speaking: false,
            }])
            .to_vec();
        let centre = pixel(&frame, 320, 160, 90);
        assert!(
            centre[2] > 200 && centre[1] < 60,
            "centre is the accent red"
        );
    }

    #[test]
    fn a_camera_tile_fills_the_tile_with_its_frame() {
        let bgra = solid(64, 36, [0x20, 0xc0, 0x40, 255]);
        let mut renderer = Renderer::new(320, 180).expect("renderer");
        let frame = renderer
            .render(&[DrawTile {
                avatar: None,
                label: "",
                initial: "",
                image: Some(SourceImage {
                    bgra: &bgra,
                    width: 64,
                    height: 36,
                }),
                accent: 0x000000,
                shape: TileShape {
                    focused: false,
                    contain: false,
                },
                speaking: false,
            }])
            .to_vec();
        let centre = pixel(&frame, 320, 160, 90);
        assert!(centre[1] > 150, "centre carries the frame green");
    }

    #[test]
    fn a_speaking_tile_draws_a_green_border() {
        let mut renderer = Renderer::new(320, 180).expect("renderer");
        let quiet = renderer
            .render(&[DrawTile {
                image: None,
                avatar: None,
                label: "",
                initial: "",
                accent: 0x000000,
                shape: TileShape {
                    focused: true,
                    contain: false,
                },
                speaking: false,
            }])
            .to_vec();
        let loud = renderer
            .render(&[DrawTile {
                image: None,
                avatar: None,
                label: "",
                initial: "",
                accent: 0x000000,
                shape: TileShape {
                    focused: true,
                    contain: false,
                },
                speaking: true,
            }])
            .to_vec();
        assert_ne!(quiet, loud, "the speaking border changes the frame");
    }

    #[test]
    fn an_accent_is_stable_per_seed() {
        assert_eq!(accent_for("alice"), accent_for("alice"));
        assert!(ACCENTS.contains(&accent_for("bob")));
    }

    #[test]
    fn rendering_many_tiles_stays_inside_the_buffer() {
        let mut renderer = Renderer::new(1280, 720).expect("renderer");
        let tiles: Vec<DrawTile<'_>> = (0..12)
            .map(|index| DrawTile {
                image: None,
                avatar: None,
                label: "",
                initial: "",
                accent: accent_for(&index.to_string()),
                shape: TileShape {
                    focused: false,
                    contain: false,
                },
                speaking: index % 3 == 0,
            })
            .collect();
        assert_eq!(renderer.render(&tiles).len(), 1280 * 720 * 4);
    }

    fn bench_tile<'a>(
        image: Option<SourceImage<'a>>,
        avatar: Option<SourceImage<'a>>,
        focused: bool,
        contain: bool,
    ) -> DrawTile<'a> {
        DrawTile {
            image,
            avatar,
            label: "Nguyen Van Long",
            initial: "N",
            accent: 0x3ba55d,
            shape: TileShape { focused, contain },
            speaking: true,
        }
    }

    #[test]
    #[ignore = "timing benchmark; run with --ignored --nocapture"]
    fn compositor_stays_inside_the_frame_budget() {
        let budget = 1000.0 / super::super::super::record::RECORD_FPS as f64;
        let screen = solid(1920, 1080, [40, 80, 160, 255]);
        let camera = solid(1280, 720, [90, 110, 130, 255]);
        let face = solid(128, 128, [200, 180, 160, 255]);
        let mut renderer = Renderer::new(1280, 720).expect("renderer");

        let mut worst = 0.0f64;
        for (name, cameras, screen_share) in [("1 camera", 1), ("5 cameras", 5)]
            .map(|(n, c)| (n, c, false))
            .into_iter()
            .chain(std::iter::once(("screen + 4 cameras", 4, true)))
        {
            let mut tiles = Vec::new();
            if screen_share {
                tiles.push(bench_tile(
                    Some(SourceImage {
                        bgra: &screen,
                        width: 1920,
                        height: 1080,
                    }),
                    None,
                    true,
                    true,
                ));
            }
            for _ in 0..cameras {
                tiles.push(bench_tile(
                    Some(SourceImage {
                        bgra: &camera,
                        width: 1280,
                        height: 720,
                    }),
                    Some(SourceImage {
                        bgra: &face,
                        width: 128,
                        height: 128,
                    }),
                    false,
                    false,
                ));
            }

            renderer.render(&tiles);
            let started = std::time::Instant::now();
            for _ in 0..30 {
                renderer.render(&tiles);
            }
            let per_frame = started.elapsed().as_secs_f64() * 1000.0 / 30.0;
            worst = worst.max(per_frame);
            println!("{name:>20}: {per_frame:6.2} ms/frame (budget {budget:.1})");
        }

        assert!(
            worst < budget,
            "the compositor cannot sustain {} fps: {worst:.2} ms/frame",
            super::super::super::record::RECORD_FPS
        );
    }
}
