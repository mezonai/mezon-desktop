use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use tiny_skia::{Pixmap, PremultipliedColorU8};

pub struct TextPainter {
    fonts: FontSystem,
    cache: SwashCache,
}

impl TextPainter {
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    pub fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let buffer = self.shape(text, size, weight, f32::INFINITY);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0f32, f32::max)
    }

    /// Shortens `text` with a trailing ellipsis until it fits `max_width`.
    pub fn truncate(&mut self, text: &str, size: f32, weight: Weight, max_width: f32) -> String {
        if max_width <= 0.0 || self.measure(text, size, weight) <= max_width {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while chars.len() > 1 {
            chars.pop();
            let candidate: String = chars.iter().collect::<String>() + "…";
            if self.measure(&candidate, size, weight) <= max_width {
                return candidate;
            }
        }
        "…".to_string()
    }

    pub fn draw(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        size: f32,
        weight: Weight,
        left: f32,
        baseline_centre: f32,
        colour: u32,
    ) {
        let buffer = self.shape(text, size, weight, f32::INFINITY);
        let line_height = size * 1.2;
        let top = baseline_centre - line_height / 2.0;
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let red = ((colour >> 16) & 0xff) as u8;
        let green = ((colour >> 8) & 0xff) as u8;
        let blue = (colour & 0xff) as u8;

        let mut runs: Vec<(i32, i32, u8)> = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((left, top), 1.0);
                self.cache.with_pixels(
                    &mut self.fonts,
                    physical.cache_key,
                    cosmic_text::Color::rgb(red, green, blue),
                    |x, y, pixel| {
                        let alpha = pixel.a();
                        if alpha == 0 {
                            return;
                        }
                        runs.push((physical.x + x, physical.y + y + run.line_y as i32, alpha));
                    },
                );
            }
        }

        let pixels = pixmap.pixels_mut();
        for (x, y, alpha) in runs {
            if x < 0 || y < 0 || x >= width || y >= height {
                continue;
            }
            let index = y as usize * width as usize + x as usize;
            let Some(slot) = pixels.get_mut(index) else {
                continue;
            };
            *slot = blend(*slot, red, green, blue, alpha);
        }
    }

    fn shape(&mut self, text: &str, size: f32, weight: Weight, width: f32) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * 1.2));
        let attrs = Attrs::new().family(Family::SansSerif).weight(weight);
        {
            let mut borrowed = buffer.borrow_with(&mut self.fonts);
            borrowed.set_size(Some(width), None);
            borrowed.set_text(text, &attrs, Shaping::Advanced, None);
            borrowed.shape_until_scroll(false);
        }
        buffer
    }
}

impl Default for TextPainter {
    fn default() -> Self {
        Self::new()
    }
}

fn blend(
    destination: PremultipliedColorU8,
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
) -> PremultipliedColorU8 {
    let source = alpha as u32;
    let inverse = 255 - source;
    let mix = |src: u8, dst: u8| -> u8 {
        (((src as u32 * source) + (dst as u32 * inverse)) / 255).min(255) as u8
    };
    let existing = destination.demultiply();
    PremultipliedColorU8::from_rgba(
        mix(red, existing.red()),
        mix(green, existing.green()),
        mix(blue, existing.blue()),
        255,
    )
    .unwrap_or(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wider_string_measures_wider() {
        let mut painter = TextPainter::new();
        let short = painter.measure("i", 16.0, Weight::NORMAL);
        let long = painter.measure("mmmmmmmm", 16.0, Weight::NORMAL);
        assert!(long > short, "{long} should exceed {short}");
    }

    #[test]
    fn truncate_keeps_a_string_that_already_fits() {
        let mut painter = TextPainter::new();
        let width = painter.measure("alice", 16.0, Weight::NORMAL);
        assert_eq!(
            painter.truncate("alice", 16.0, Weight::NORMAL, width + 1.0),
            "alice"
        );
    }

    #[test]
    fn truncate_adds_an_ellipsis_and_respects_the_limit() {
        let mut painter = TextPainter::new();
        let limit = painter.measure("alice", 16.0, Weight::NORMAL);
        let cut = painter.truncate("alice in wonderland", 16.0, Weight::NORMAL, limit);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(painter.measure(&cut, 16.0, Weight::NORMAL) <= limit + 0.5);
    }

    #[test]
    fn drawing_marks_the_pixmap() {
        let mut painter = TextPainter::new();
        let mut pixmap = Pixmap::new(200, 60).expect("pixmap");
        pixmap.fill(tiny_skia::Color::BLACK);
        let before = pixmap.data().to_vec();
        painter.draw(
            &mut pixmap,
            "Mezon",
            24.0,
            Weight::SEMIBOLD,
            10.0,
            30.0,
            0xffffff,
        );
        assert_ne!(before, pixmap.data(), "text should change pixels");
    }
}
