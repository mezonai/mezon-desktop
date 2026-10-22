use std::sync::Arc;

use cosmic_text::Weight;
use parking_lot::Mutex;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect,
    Stroke, Transform,
};

use crate::compose::TextPainter;
use crate::video::{rgb_to_u, rgb_to_v, rgb_to_y};

const TEXT_LINE_HEIGHT: f32 = 1.3;
const MIN_CANVAS_SCALE: f32 = 0.25;
const MAX_CANVAS_SCALE: f32 = 8.0;
const ARROW_HEAD_MIN: f32 = 12.0;
const ARROW_HEAD_PER_WIDTH: f32 = 3.5;
const ARROW_HEAD_SPREAD: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnnotationPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationItem {
    Stroke {
        points: Vec<AnnotationPoint>,
        color: u32,
        width: f32,
    },
    Arrow {
        from: AnnotationPoint,
        to: AnnotationPoint,
        color: u32,
        width: f32,
    },
    Rectangle {
        from: AnnotationPoint,
        to: AnnotationPoint,
        color: u32,
        width: f32,
    },
    Ellipse {
        from: AnnotationPoint,
        to: AnnotationPoint,
        color: u32,
        width: f32,
    },
    Text {
        position: AnnotationPoint,
        content: String,
        color: u32,
        font_size: f32,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnnotationScene {
    pub canvas_width: f32,
    pub canvas_height: f32,
    pub items: Vec<AnnotationItem>,
}

pub fn arrow_head(from: (f32, f32), to: (f32, f32), width: f32) -> Option<[(f32, f32); 3]> {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return None;
    }
    let head = (ARROW_HEAD_MIN.max(width * ARROW_HEAD_PER_WIDTH)).min(length);
    let ux = dx / length;
    let uy = dy / length;
    let base_x = to.0 - ux * head;
    let base_y = to.1 - uy * head;
    let spread = head * ARROW_HEAD_SPREAD;
    Some([
        to,
        (base_x - uy * spread, base_y + ux * spread),
        (base_x + uy * spread, base_y - ux * spread),
    ])
}

type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub struct ScreenAnnotations {
    state: Mutex<AnnotationsState>,
}

#[derive(Default)]
struct AnnotationsState {
    scene: Option<Arc<AnnotationScene>>,
    version: u64,
    waker: Option<(u64, Waker)>,
    waker_token: u64,
}

impl ScreenAnnotations {
    pub fn set_scene(&self, scene: Option<AnnotationScene>) {
        let scene = scene.filter(|scene| !scene.items.is_empty()).map(Arc::new);
        let waker = {
            let mut state = self.state.lock();
            if state.scene.is_none() && scene.is_none() {
                return;
            }
            state.scene = scene;
            state.version = state.version.wrapping_add(1);
            state.waker.as_ref().map(|(_, waker)| waker.clone())
        };
        if let Some(waker) = waker {
            waker();
        }
    }

    pub(crate) fn has_scene(&self) -> bool {
        self.state.lock().scene.is_some()
    }

    fn snapshot(&self) -> (u64, Option<Arc<AnnotationScene>>) {
        let state = self.state.lock();
        (state.version, state.scene.clone())
    }

    pub(crate) fn set_waker(&self, waker: Waker) -> u64 {
        let mut state = self.state.lock();
        state.waker_token = state.waker_token.wrapping_add(1);
        let token = state.waker_token;
        state.waker = Some((token, waker));
        token
    }

    pub(crate) fn clear_waker(&self, token: u64) {
        let mut state = self.state.lock();
        if state
            .waker
            .as_ref()
            .is_some_and(|(current, _)| *current == token)
        {
            state.waker = None;
        }
    }
}

struct YuvOverlay {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    luma: Vec<u8>,
    luma_alpha: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    chroma_alpha: Vec<u8>,
}

pub(crate) struct AnnotationCompositor {
    annotations: Arc<ScreenAnnotations>,
    version: Option<u64>,
    frame_size: (u32, u32),
    pixmap: Option<Pixmap>,
    overlay: Option<YuvOverlay>,
    text: Option<TextPainter>,
}

impl AnnotationCompositor {
    pub(crate) fn new(annotations: Arc<ScreenAnnotations>) -> Self {
        Self {
            annotations,
            version: None,
            frame_size: (0, 0),
            pixmap: None,
            overlay: None,
            text: None,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.annotations.has_scene()
    }

    pub(crate) fn prepare(&mut self, width: u32, height: u32) -> bool {
        let (version, scene) = self.annotations.snapshot();
        if self.version == Some(version) && self.frame_size == (width, height) {
            return self.overlay.is_some();
        }
        self.version = Some(version);
        self.frame_size = (width, height);
        let overlay = match scene {
            Some(scene) => self.rasterize(&scene, width, height),
            None => None,
        };
        self.overlay = overlay;
        self.overlay.is_some()
    }

    pub(crate) fn blend(
        &self,
        y: &mut [u8],
        u: &mut [u8],
        v: &mut [u8],
        stride_y: usize,
        stride_u: usize,
        stride_v: usize,
    ) {
        let Some(overlay) = &self.overlay else {
            return;
        };
        for row in 0..overlay.height {
            let src = row * overlay.width;
            let dst = (overlay.top + row) * stride_y + overlay.left;
            let Some(dst_row) = y.get_mut(dst..dst + overlay.width) else {
                break;
            };
            mix_row(
                dst_row,
                &overlay.luma[src..src + overlay.width],
                &overlay.luma_alpha[src..src + overlay.width],
            );
        }
        let chroma_width = overlay.width / 2;
        let chroma_left = overlay.left / 2;
        let chroma_top = overlay.top / 2;
        for row in 0..overlay.height / 2 {
            let src = row * chroma_width;
            let alpha = &overlay.chroma_alpha[src..src + chroma_width];
            let dst_u = (chroma_top + row) * stride_u + chroma_left;
            if let Some(dst_row) = u.get_mut(dst_u..dst_u + chroma_width) {
                mix_row(dst_row, &overlay.u[src..src + chroma_width], alpha);
            }
            let dst_v = (chroma_top + row) * stride_v + chroma_left;
            if let Some(dst_row) = v.get_mut(dst_v..dst_v + chroma_width) {
                mix_row(dst_row, &overlay.v[src..src + chroma_width], alpha);
            }
        }
    }

    fn rasterize(
        &mut self,
        scene: &AnnotationScene,
        width: u32,
        height: u32,
    ) -> Option<YuvOverlay> {
        if width < 2 || height < 2 || scene.items.is_empty() {
            return None;
        }
        let mut pixmap = match self.pixmap.take() {
            Some(pixmap) if pixmap.width() == width && pixmap.height() == height => pixmap,
            _ => Pixmap::new(width, height)?,
        };
        pixmap.fill(Color::TRANSPARENT);

        let frame_w = width as f32;
        let frame_h = height as f32;
        let scale = if scene.canvas_width > 0.0 {
            (frame_w / scene.canvas_width).clamp(MIN_CANVAS_SCALE, MAX_CANVAS_SCALE)
        } else {
            1.0
        };
        let to_px = |point: &AnnotationPoint| (point.x * frame_w, point.y * frame_h);

        let mut bounds = PixelBounds::default();
        for item in &scene.items {
            match item {
                AnnotationItem::Stroke {
                    points,
                    color,
                    width,
                } => {
                    let stroke_width = (width * scale).max(1.0);
                    let pixels: Vec<(f32, f32)> = points.iter().map(to_px).collect();
                    for &(x, y) in &pixels {
                        bounds.include(x, y, stroke_width);
                    }
                    stroke_polyline(&mut pixmap, &pixels, *color, stroke_width);
                }
                AnnotationItem::Arrow {
                    from,
                    to,
                    color,
                    width,
                } => {
                    let stroke_width = (width * scale).max(1.0);
                    let start = to_px(from);
                    let end = to_px(to);
                    let Some(head) = arrow_head(start, end, stroke_width) else {
                        continue;
                    };
                    let shaft_end = ((head[1].0 + head[2].0) / 2.0, (head[1].1 + head[2].1) / 2.0);
                    stroke_polyline(&mut pixmap, &[start, shaft_end], *color, stroke_width);
                    fill_polygon(&mut pixmap, &head, *color);
                    bounds.include(start.0, start.1, stroke_width);
                    for (x, y) in head {
                        bounds.include(x, y, stroke_width);
                    }
                }
                AnnotationItem::Rectangle {
                    from,
                    to,
                    color,
                    width,
                } => {
                    let stroke_width = (width * scale).max(1.0);
                    let (x0, y0) = to_px(from);
                    let (x1, y1) = to_px(to);
                    if let Some(rect) =
                        Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
                    {
                        stroke_path(
                            &mut pixmap,
                            &PathBuilder::from_rect(rect),
                            *color,
                            stroke_width,
                        );
                        bounds.include(x0, y0, stroke_width);
                        bounds.include(x1, y1, stroke_width);
                    }
                }
                AnnotationItem::Ellipse {
                    from,
                    to,
                    color,
                    width,
                } => {
                    let stroke_width = (width * scale).max(1.0);
                    let (x0, y0) = to_px(from);
                    let (x1, y1) = to_px(to);
                    if let Some(path) =
                        Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
                            .and_then(PathBuilder::from_oval)
                    {
                        stroke_path(&mut pixmap, &path, *color, stroke_width);
                        bounds.include(x0, y0, stroke_width);
                        bounds.include(x1, y1, stroke_width);
                    }
                }
                AnnotationItem::Text {
                    position,
                    content,
                    color,
                    font_size,
                } => {
                    if content.trim().is_empty() {
                        continue;
                    }
                    let size = (font_size * scale).max(6.0);
                    let painter = self.text.get_or_insert_with(TextPainter::new);
                    let line_height = size * TEXT_LINE_HEIGHT;
                    let (x, y) = to_px(position);
                    let coverage =
                        painter.coverage(content, size, Weight::BOLD, x, y + line_height / 2.0);
                    for &(gx, gy, _) in &coverage {
                        bounds.include(gx as f32, gy as f32, 0.0);
                    }
                    paint_coverage(&mut pixmap, &coverage, *color);
                }
            }
        }

        let overlay = bounds
            .clamp(width as usize, height as usize)
            .map(|(left, top, right, bottom)| yuv_overlay(&pixmap, left, top, right, bottom));
        self.pixmap = Some(pixmap);
        overlay
    }
}

#[derive(Default)]
struct PixelBounds {
    rect: Option<(f32, f32, f32, f32)>,
}

impl PixelBounds {
    fn include(&mut self, x: f32, y: f32, stroke_width: f32) {
        let pad = stroke_width / 2.0 + 2.0;
        let (l, t, r, b) = (x - pad, y - pad, x + pad, y + pad);
        self.rect = Some(match self.rect {
            Some((l0, t0, r0, b0)) => (l0.min(l), t0.min(t), r0.max(r), b0.max(b)),
            None => (l, t, r, b),
        });
    }

    fn clamp(&self, width: usize, height: usize) -> Option<(usize, usize, usize, usize)> {
        let (l, t, r, b) = self.rect?;
        let left = (l.floor().max(0.0) as usize).min(width) & !1;
        let top = (t.floor().max(0.0) as usize).min(height) & !1;
        let right = ((r.ceil().max(0.0) as usize).saturating_add(1) & !1).min(width & !1);
        let bottom = ((b.ceil().max(0.0) as usize).saturating_add(1) & !1).min(height & !1);
        (right > left && bottom > top).then_some((left, top, right, bottom))
    }
}

fn yuv_overlay(
    pixmap: &Pixmap,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) -> YuvOverlay {
    let width = right - left;
    let height = bottom - top;
    let stride = pixmap.width() as usize;
    let pixels = pixmap.pixels();
    let mut luma = vec![0u8; width * height];
    let mut luma_alpha = vec![0u8; width * height];
    let chroma_len = (width / 2) * (height / 2);
    let mut u = vec![128u8; chroma_len];
    let mut v = vec![128u8; chroma_len];
    let mut chroma_alpha = vec![0u8; chroma_len];

    for row in 0..height {
        for col in 0..width {
            let pixel = pixels[(top + row) * stride + left + col];
            let alpha = pixel.alpha();
            if alpha == 0 {
                continue;
            }
            let color = pixel.demultiply();
            let index = row * width + col;
            luma[index] = rgb_to_y(
                color.red() as i32,
                color.green() as i32,
                color.blue() as i32,
            );
            luma_alpha[index] = alpha;
        }
    }

    for row in 0..height / 2 {
        for col in 0..width / 2 {
            let mut sum = [0u32; 4];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let pixel = pixels[(top + row * 2 + dy) * stride + left + col * 2 + dx];
                sum[0] += pixel.red() as u32;
                sum[1] += pixel.green() as u32;
                sum[2] += pixel.blue() as u32;
                sum[3] += pixel.alpha() as u32;
            }
            if sum[3] == 0 {
                continue;
            }
            let demultiply = |channel: u32| ((channel * 255 + sum[3] / 2) / sum[3]).min(255) as i32;
            let (r, g, b) = (demultiply(sum[0]), demultiply(sum[1]), demultiply(sum[2]));
            let index = row * (width / 2) + col;
            u[index] = rgb_to_u(r, g, b);
            v[index] = rgb_to_v(r, g, b);
            chroma_alpha[index] = ((sum[3] + 2) / 4) as u8;
        }
    }

    YuvOverlay {
        left,
        top,
        width,
        height,
        luma,
        luma_alpha,
        u,
        v,
        chroma_alpha,
    }
}

fn mix_row(dst: &mut [u8], src: &[u8], alpha: &[u8]) {
    for ((dst, &src), &alpha) in dst.iter_mut().zip(src).zip(alpha) {
        if alpha == 0 {
            continue;
        }
        let a = alpha as u32;
        *dst = ((src as u32 * a + *dst as u32 * (255 - a) + 127) / 255) as u8;
    }
}

fn paint_for(color: u32) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(
        (color >> 24) as u8,
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
    ));
    paint.anti_alias = true;
    paint
}

fn round_stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn stroke_path(pixmap: &mut Pixmap, path: &tiny_skia::Path, color: u32, width: f32) {
    pixmap.stroke_path(
        path,
        &paint_for(color),
        &round_stroke(width),
        Transform::identity(),
        None,
    );
}

fn fill_path(pixmap: &mut Pixmap, path: &tiny_skia::Path, color: u32) {
    pixmap.fill_path(
        path,
        &paint_for(color),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn stroke_polyline(pixmap: &mut Pixmap, points: &[(f32, f32)], color: u32, width: f32) {
    let Some(&(first_x, first_y)) = points.first() else {
        return;
    };
    if points.len() == 1 {
        if let Some(dot) = PathBuilder::from_circle(first_x, first_y, width / 2.0) {
            fill_path(pixmap, &dot, color);
        }
        return;
    }
    let mut builder = PathBuilder::new();
    builder.move_to(first_x, first_y);
    for &(x, y) in &points[1..] {
        builder.line_to(x, y);
    }
    if let Some(path) = builder.finish() {
        stroke_path(pixmap, &path, color, width);
    }
}

fn fill_polygon(pixmap: &mut Pixmap, points: &[(f32, f32)], color: u32) {
    let Some(&(first_x, first_y)) = points.first() else {
        return;
    };
    let mut builder = PathBuilder::new();
    builder.move_to(first_x, first_y);
    for &(x, y) in &points[1..] {
        builder.line_to(x, y);
    }
    builder.close();
    if let Some(path) = builder.finish() {
        fill_path(pixmap, &path, color);
    }
}

fn paint_coverage(pixmap: &mut Pixmap, coverage: &[(i32, i32, u8)], color: u32) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let red = (color >> 24) & 0xff;
    let green = (color >> 16) & 0xff;
    let blue = (color >> 8) & 0xff;
    let opacity = color & 0xff;
    let pixels = pixmap.pixels_mut();
    for &(x, y, cover) in coverage {
        if x < 0 || y < 0 || x >= width || y >= height {
            continue;
        }
        let Some(slot) = pixels.get_mut(y as usize * width as usize + x as usize) else {
            continue;
        };
        let alpha = cover as u32 * opacity / 255;
        let inverse = 255 - alpha;
        let mix = |source: u32, existing: u8| {
            ((source * alpha + existing as u32 * inverse + 127) / 255) as u8
        };
        if let Some(blended) = PremultipliedColorU8::from_rgba(
            mix(red, slot.red()),
            mix(green, slot.green()),
            mix(blue, slot.blue()),
            mix(255, slot.alpha()),
        ) {
            *slot = blended;
        }
    }
}
