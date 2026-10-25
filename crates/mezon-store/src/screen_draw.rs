use std::time::{Duration, Instant};

use mezon_voice::{AnnotationItem, AnnotationPoint, AnnotationScene};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawTool {
    Pointer,
    Select,
    Pen,
    Highlighter,
    Laser,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Eraser,
}

impl DrawTool {
    pub fn shape_kind(self) -> Option<ShapeKind> {
        match self {
            Self::Arrow => Some(ShapeKind::Arrow),
            Self::Rectangle => Some(ShapeKind::Rectangle),
            Self::Ellipse => Some(ShapeKind::Ellipse),
            _ => None,
        }
    }

    pub fn is_freehand(self) -> bool {
        matches!(self, Self::Pen | Self::Highlighter | Self::Laser)
    }

    pub fn uses_width(self) -> bool {
        self.is_freehand() || self.shape_kind().is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedPoint {
    pub x: f32,
    pub y: f32,
}

impl NormalizedPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }

    pub fn distance_squared(&self, other: &Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    pub fn distance(&self, other: &Self) -> f32 {
        self.distance_squared(other).sqrt()
    }

    fn to_annotation(self) -> AnnotationPoint {
        AnnotationPoint {
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeKind {
    #[default]
    Pen,
    Highlighter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PenStroke {
    pub id: u64,
    pub color: u32,
    pub width: f32,
    pub points: Vec<NormalizedPoint>,
    #[serde(default)]
    pub kind: StrokeKind,
}

impl PenStroke {
    pub fn render_color(&self) -> u32 {
        match self.kind {
            StrokeKind::Pen => self.color,
            StrokeKind::Highlighter => with_alpha(self.color, HIGHLIGHTER_ALPHA),
        }
    }

    pub fn render_width(&self) -> f32 {
        match self.kind {
            StrokeKind::Pen => self.width,
            StrokeKind::Highlighter => self.width * HIGHLIGHTER_WIDTH_SCALE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Arrow,
    Rectangle,
    Ellipse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeAnnotation {
    pub id: u64,
    pub kind: ShapeKind,
    pub color: u32,
    pub width: f32,
    pub start: NormalizedPoint,
    pub end: NormalizedPoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextAnnotation {
    pub id: u64,
    pub color: u32,
    pub font_size: f32,
    pub position: NormalizedPoint,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawAnnotation {
    Stroke(PenStroke),
    Shape(ShapeAnnotation),
    Text(TextAnnotation),
}

impl DrawAnnotation {
    pub fn id(&self) -> u64 {
        match self {
            Self::Stroke(s) => s.id,
            Self::Shape(s) => s.id,
            Self::Text(t) => t.id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnnotationDrag {
    pub id: u64,
    pub start: NormalizedPoint,
    pub last: NormalizedPoint,
    pub moved: bool,
    pub edit_on_click: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marquee {
    pub start: NormalizedPoint,
    pub end: NormalizedPoint,
    pub canvas: (f32, f32),
}

impl Marquee {
    pub fn frame(&self) -> HitBox {
        let (width, height) = self.canvas;
        HitBox {
            left: self.start.x.min(self.end.x) * width,
            top: self.start.y.min(self.end.y) * height,
            right: self.start.x.max(self.end.x) * width,
            bottom: self.start.y.max(self.end.y) * height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextResize {
    pub id: u64,
    pub origin: (f32, f32),
    pub start_distance: f32,
    pub start_size: f32,
    pub canvas: (f32, f32),
    pub resized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl HitBox {
    pub fn of_points(points: impl IntoIterator<Item = (f32, f32)>, pad: f32) -> Option<Self> {
        let mut bounds: Option<Self> = None;
        for (x, y) in points {
            let next = match bounds {
                Some(b) => Self {
                    left: b.left.min(x),
                    top: b.top.min(y),
                    right: b.right.max(x),
                    bottom: b.bottom.max(y),
                },
                None => Self {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                },
            };
            bounds = Some(next);
        }
        bounds.map(|b| Self {
            left: b.left - pad,
            top: b.top - pad,
            right: b.right + pad,
            bottom: b.bottom + pad,
        })
    }

    pub fn of(text: &TextAnnotation, canvas: (f32, f32), text_width: f32) -> Self {
        let size = text_font_px(text.font_size);
        let x = text.position.x * canvas.0;
        let y = text.position.y * canvas.1;
        let width = text_width.max(size * 2.0);
        Self {
            left: x - TEXT_PAD_X / 2.0,
            top: y - TEXT_PAD_Y / 2.0,
            right: x + width + TEXT_PAD_X / 2.0,
            bottom: y + size * TEXT_LINE_HEIGHT + TEXT_PAD_Y / 2.0,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    pub fn handle_contains(&self, x: f32, y: f32) -> bool {
        (x - self.right).abs() <= TEXT_HANDLE_SIZE && (y - self.bottom).abs() <= TEXT_HANDLE_SIZE
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.left <= other.right
            && other.left <= self.right
            && self.top <= other.bottom
            && other.top <= self.bottom
    }
}

pub fn text_font_px(font_size: f32) -> f32 {
    font_size.clamp(TEXT_MIN_SIZE, TEXT_MAX_SIZE)
}

#[derive(Clone, Debug, PartialEq)]
pub struct VanishingStroke {
    pub stroke: PenStroke,
    pub released_at: Instant,
}

impl VanishingStroke {
    pub fn alpha(&self, now: Instant) -> f32 {
        let age = now.saturating_duration_since(self.released_at);
        if age <= VANISH_HOLD {
            return 1.0;
        }
        let fading = (age - VANISH_HOLD).as_secs_f32() / VANISH_FADE.as_secs_f32();
        (1.0 - fading).clamp(0.0, 1.0)
    }
}

pub const COLOR_RED: u32 = 0xEF4444FF;
pub const COLOR_ORANGE: u32 = 0xF97316FF;
pub const COLOR_YELLOW: u32 = 0xF59E0BFF;
pub const COLOR_GREEN: u32 = 0x10B981FF;
pub const COLOR_BLUE: u32 = 0x3B82F6FF;
pub const COLOR_PURPLE: u32 = 0x8B5CF6FF;
pub const COLOR_WHITE: u32 = 0xFFFFFFFF;
pub const COLOR_BLACK: u32 = 0x18181BFF;

pub const PRESET_COLORS: &[u32] = &[
    COLOR_RED,
    COLOR_YELLOW,
    COLOR_GREEN,
    COLOR_BLUE,
    COLOR_WHITE,
    COLOR_BLACK,
];

pub const STROKE_WIDTH_THIN: f32 = 2.0;
pub const STROKE_WIDTH_MEDIUM: f32 = 4.0;
pub const STROKE_WIDTH_THICK: f32 = 8.0;

pub const PRESET_WIDTHS: &[f32] = &[STROKE_WIDTH_THIN, STROKE_WIDTH_MEDIUM, STROKE_WIDTH_THICK];

pub const HIGHLIGHTER_ALPHA: u8 = 0x66;
pub const HIGHLIGHTER_WIDTH_SCALE: f32 = 3.0;
pub const VANISH_HOLD: Duration = Duration::from_millis(1500);
pub const VANISH_FADE: Duration = Duration::from_millis(600);
pub const ERASER_RADIUS: f32 = 0.02;

const MIN_POINT_SPACING_SQUARED: f32 = 0.000002;
const MIN_SHAPE_SIZE: f32 = 0.004;
pub const TEXT_MIN_SIZE: f32 = 10.0;
pub const TEXT_MAX_SIZE: f32 = 120.0;
pub const TEXT_LINE_HEIGHT: f32 = 1.3;
pub const TEXT_PAD_X: f32 = 8.0;
pub const TEXT_PAD_Y: f32 = 4.0;
pub const TEXT_HANDLE_SIZE: f32 = 10.0;

const TEXT_DRAG_THRESHOLD: f32 = 0.004;
const STROKE_HIT_SLOP: f32 = 6.0;
const UNDO_LIMIT: usize = 50;
const ELLIPSE_HIT_SEGMENTS: usize = 48;

pub fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xFFFF_FF00) | alpha as u32
}

pub fn scale_alpha(color: u32, factor: f32) -> u32 {
    let alpha = ((color & 0xFF) as f32 * factor.clamp(0.0, 1.0)).round() as u8;
    with_alpha(color, alpha)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentViewport {
    pub offset_x: f32,
    pub offset_y: f32,
    pub width: f32,
    pub height: f32,
}

impl ContentViewport {
    pub fn compute(container_w: f32, container_h: f32, content_w: f32, content_h: f32) -> Self {
        if container_w <= 0.0 || container_h <= 0.0 {
            return Self {
                offset_x: 0.0,
                offset_y: 0.0,
                width: container_w.max(1.0),
                height: container_h.max(1.0),
            };
        }
        let content_ratio = if content_w > 0.0 && content_h > 0.0 {
            content_w / content_h
        } else {
            16.0 / 9.0
        };
        let container_ratio = container_w / container_h;

        let (disp_w, disp_h) = if container_ratio > content_ratio {
            (container_h * content_ratio, container_h)
        } else {
            (container_w, container_w / content_ratio)
        };

        Self {
            offset_x: (container_w - disp_w) * 0.5,
            offset_y: (container_h - disp_h) * 0.5,
            width: disp_w,
            height: disp_h,
        }
    }

    pub fn fill(width: f32, height: f32) -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 0.0,
            width: width.max(1.0),
            height: height.max(1.0),
        }
    }

    pub fn to_normalized(&self, px: f32, py: f32) -> NormalizedPoint {
        let u = if self.width > 0.0 {
            (px - self.offset_x) / self.width
        } else {
            0.0
        };
        let v = if self.height > 0.0 {
            (py - self.offset_y) / self.height
        } else {
            0.0
        };
        NormalizedPoint::new(u, v)
    }

    pub fn to_pixels(&self, pt: &NormalizedPoint) -> (f32, f32) {
        (
            self.offset_x + pt.x * self.width,
            self.offset_y + pt.y * self.height,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenDrawState {
    pub is_active: bool,
    pub active_tool: DrawTool,
    pub stroke_color: u32,
    pub stroke_width: f32,
    pub text_font_size: f32,
    pub annotations: Vec<DrawAnnotation>,
    pub undo_stack: Vec<Vec<DrawAnnotation>>,
    pub redo_stack: Vec<Vec<DrawAnnotation>>,
    pub current_stroke: Option<PenStroke>,
    pub current_shape: Option<ShapeAnnotation>,
    pub drag: Option<AnnotationDrag>,
    pub text_resize: Option<TextResize>,
    pub selected: Option<u64>,
    pub selection: Vec<u64>,
    pub marquee: Option<Marquee>,
    pub vanishing: Vec<VanishingStroke>,
    pub editing_text_id: Option<u64>,
    pub editing_text_content: String,
    pub next_id: u64,
}

impl Default for ScreenDrawState {
    fn default() -> Self {
        Self {
            is_active: false,
            active_tool: DrawTool::Pen,
            stroke_color: COLOR_RED,
            stroke_width: STROKE_WIDTH_MEDIUM,
            text_font_size: 16.0,
            annotations: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_stroke: None,
            current_shape: None,
            drag: None,
            text_resize: None,
            selected: None,
            selection: Vec::new(),
            marquee: None,
            vanishing: Vec::new(),
            editing_text_id: None,
            editing_text_content: String::new(),
            next_id: 1,
        }
    }
}

impl ScreenDrawState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle_active(&mut self) {
        let active = !self.is_active;
        self.set_active(active);
    }

    pub fn set_active(&mut self, active: bool) {
        self.is_active = active;
        if !active {
            self.current_stroke = None;
            self.current_shape = None;
            self.drag = None;
            self.text_resize = None;
            self.selected = None;
            self.selection.clear();
            self.marquee = None;
            self.editing_text_id = None;
        }
    }

    pub fn set_tool(&mut self, tool: DrawTool) {
        if tool != DrawTool::Text && self.editing_text_id.is_some() {
            self.commit_editing_text();
        }
        self.active_tool = tool;
        self.current_stroke = None;
        self.current_shape = None;
        self.drag = None;
        self.text_resize = None;
        self.marquee = None;
        if !matches!(tool, DrawTool::Text | DrawTool::Pointer) {
            self.selected = None;
        }
        if tool != DrawTool::Select {
            self.selection.clear();
        }
    }

    pub fn set_color(&mut self, color: u32) {
        self.stroke_color = color;
    }

    pub fn set_width(&mut self, width: f32) {
        self.stroke_width = width.clamp(1.0, 32.0);
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.annotations.clone());
        self.redo_stack.clear();
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn is_drawing(&self) -> bool {
        self.current_stroke.is_some()
            || self.current_shape.is_some()
            || self.drag.is_some()
            || self.text_resize.is_some()
            || self.marquee.is_some()
    }

    pub fn pointer_down(
        &mut self,
        point: NormalizedPoint,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> bool {
        if self.active_tool != DrawTool::Text && self.editing_text_id.is_some() {
            self.commit_editing_text();
        }
        match self.active_tool {
            DrawTool::Pointer => {
                if let Some(resize) = self.resize_handle_at(&point, canvas, measure) {
                    self.text_resize = Some(resize);
                } else if let Some(existing) = self.annotation_at(&point, canvas, measure) {
                    self.start_drag(existing, point);
                } else {
                    self.selected = None;
                }
                true
            }
            DrawTool::Select => {
                match self.annotation_at(&point, canvas, measure) {
                    Some(id) => {
                        if !self.selection.contains(&id) {
                            self.selection = vec![id];
                        }
                        self.drag = Some(AnnotationDrag {
                            id,
                            start: point,
                            last: point,
                            moved: false,
                            edit_on_click: false,
                        });
                    }
                    None => {
                        self.selection.clear();
                        self.marquee = Some(Marquee {
                            start: point,
                            end: point,
                            canvas,
                        });
                    }
                }
                true
            }
            DrawTool::Pen | DrawTool::Highlighter | DrawTool::Laser => {
                self.start_stroke(point.x, point.y);
                true
            }
            DrawTool::Arrow | DrawTool::Rectangle | DrawTool::Ellipse => {
                if let Some(kind) = self.active_tool.shape_kind() {
                    self.start_shape(kind, point);
                }
                true
            }
            DrawTool::Eraser => {
                self.erase_at(point.x, point.y, ERASER_RADIUS);
                true
            }
            DrawTool::Text => {
                if let Some(resize) = self.resize_handle_at(&point, canvas, measure) {
                    self.commit_editing_text();
                    self.text_resize = Some(resize);
                } else if let Some(existing) = self.text_at(&point, canvas, measure) {
                    if self.editing_text_id != Some(existing) {
                        self.commit_editing_text();
                        self.start_drag(existing, point);
                    }
                } else {
                    self.commit_editing_text();
                    self.selected = None;
                    let id = self.add_text(point.x, point.y, String::new());
                    self.start_editing_text(id);
                }
                true
            }
        }
    }

    pub fn pointer_move(
        &mut self,
        point: NormalizedPoint,
        pressed: bool,
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> bool {
        if let Some(resize) = self.text_resize {
            return self.resize_text(resize, point);
        }
        if let Some(drag) = self.drag {
            return self.drag_annotation(drag, point);
        }
        if let Some(marquee) = self.marquee {
            return self.update_marquee(marquee, point, measure);
        }
        match self.active_tool {
            DrawTool::Pen | DrawTool::Highlighter | DrawTool::Laser
                if self.current_stroke.is_some() =>
            {
                self.add_stroke_point(point.x, point.y)
            }
            DrawTool::Arrow | DrawTool::Rectangle | DrawTool::Ellipse
                if self.current_shape.is_some() =>
            {
                self.update_shape(point);
                true
            }
            DrawTool::Eraser if pressed => self.erase_at(point.x, point.y, ERASER_RADIUS),
            _ => false,
        }
    }

    pub fn pointer_up(&mut self) -> bool {
        if self.text_resize.take().is_some() {
            true
        } else if let Some(drag) = self.drag.take() {
            if !drag.moved && drag.edit_on_click && self.active_tool == DrawTool::Text {
                self.start_editing_text(drag.id);
            }
            true
        } else if self.marquee.take().is_some() {
            true
        } else if self.current_stroke.is_some() {
            self.finish_stroke();
            true
        } else if self.current_shape.is_some() {
            self.finish_shape();
            true
        } else {
            false
        }
    }

    pub fn selected_text(&self) -> Option<&TextAnnotation> {
        let id = self.selected?;
        self.text(id)
    }

    fn text(&self, id: u64) -> Option<&TextAnnotation> {
        self.annotations.iter().find_map(|ann| match ann {
            DrawAnnotation::Text(t) if t.id == id => Some(t),
            _ => None,
        })
    }

    pub fn selected_annotation(&self) -> Option<&DrawAnnotation> {
        let id = self.selected?;
        self.annotations.iter().find(|ann| ann.id() == id)
    }

    pub fn annotation_bounds(
        &self,
        annotation: &DrawAnnotation,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> Option<HitBox> {
        let to_px = |p: &NormalizedPoint| (p.x * canvas.0, p.y * canvas.1);
        match annotation {
            DrawAnnotation::Text(t) => Some(HitBox::of(t, canvas, measure(t))),
            DrawAnnotation::Stroke(s) => {
                HitBox::of_points(s.points.iter().map(to_px), s.render_width() / 2.0)
            }
            DrawAnnotation::Shape(s) => {
                HitBox::of_points([to_px(&s.start), to_px(&s.end)], s.width / 2.0)
            }
        }
    }

    fn text_at(
        &self,
        point: &NormalizedPoint,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> Option<u64> {
        let (x, y) = (point.x * canvas.0, point.y * canvas.1);
        self.annotations.iter().rev().find_map(|ann| match ann {
            DrawAnnotation::Text(t) if HitBox::of(t, canvas, measure(t)).contains(x, y) => {
                Some(t.id)
            }
            _ => None,
        })
    }

    pub fn annotation_at(
        &self,
        point: &NormalizedPoint,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> Option<u64> {
        let to_px = |p: &NormalizedPoint| NormalizedPoint {
            x: p.x * canvas.0,
            y: p.y * canvas.1,
        };
        let cursor = to_px(point);
        self.annotations.iter().rev().find_map(|ann| {
            let hit = match ann {
                DrawAnnotation::Text(t) => {
                    HitBox::of(t, canvas, measure(t)).contains(cursor.x, cursor.y)
                }
                DrawAnnotation::Stroke(s) => {
                    let pixels: Vec<NormalizedPoint> = s.points.iter().map(to_px).collect();
                    let stroke = PenStroke {
                        points: pixels,
                        ..s.clone()
                    };
                    stroke_intersects_point(
                        &stroke,
                        &cursor,
                        s.render_width() / 2.0 + STROKE_HIT_SLOP,
                    )
                }
                DrawAnnotation::Shape(s) => {
                    let shape = ShapeAnnotation {
                        start: to_px(&s.start),
                        end: to_px(&s.end),
                        ..s.clone()
                    };
                    shape_intersects_point(&shape, &cursor, s.width / 2.0 + STROKE_HIT_SLOP)
                }
            };
            hit.then_some(ann.id())
        })
    }

    pub fn hit_test(
        &self,
        point: &NormalizedPoint,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> bool {
        self.resize_handle_at(point, canvas, measure).is_some()
            || self.annotation_at(point, canvas, measure).is_some()
    }

    fn start_drag(&mut self, id: u64, point: NormalizedPoint) {
        let was_selected = self.selected == Some(id);
        self.selected = Some(id);
        self.drag = Some(AnnotationDrag {
            id,
            start: point,
            last: point,
            moved: false,
            edit_on_click: was_selected,
        });
    }

    fn resize_handle_at(
        &self,
        point: &NormalizedPoint,
        canvas: (f32, f32),
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> Option<TextResize> {
        let text = self.selected_text()?;
        if self.editing_text_id == Some(text.id) {
            return None;
        }
        let (x, y) = (point.x * canvas.0, point.y * canvas.1);
        if !HitBox::of(text, canvas, measure(text)).handle_contains(x, y) {
            return None;
        }
        let origin = (text.position.x * canvas.0, text.position.y * canvas.1);
        Some(TextResize {
            id: text.id,
            origin,
            start_distance: (x - origin.0).hypot(y - origin.1).max(1.0),
            start_size: text_font_px(text.font_size),
            canvas,
            resized: false,
        })
    }

    fn resize_text(&mut self, resize: TextResize, point: NormalizedPoint) -> bool {
        let x = point.x * resize.canvas.0;
        let y = point.y * resize.canvas.1;
        let distance = (x - resize.origin.0).hypot(y - resize.origin.1);
        let size = text_font_px(resize.start_size * distance / resize.start_distance);
        if !resize.resized {
            self.push_undo();
        }
        self.text_resize = Some(TextResize {
            resized: true,
            ..resize
        });
        if let Some(text) = self.annotations.iter_mut().find_map(|ann| match ann {
            DrawAnnotation::Text(t) if t.id == resize.id => Some(t),
            _ => None,
        }) {
            text.font_size = size;
        }
        true
    }

    fn drag_annotation(&mut self, drag: AnnotationDrag, point: NormalizedPoint) -> bool {
        if !drag.moved && drag.start.distance(&point) < TEXT_DRAG_THRESHOLD {
            return false;
        }
        if !drag.moved {
            self.push_undo();
        }
        self.drag = Some(AnnotationDrag {
            moved: true,
            last: point,
            ..drag
        });
        let dx = point.x - drag.last.x;
        let dy = point.y - drag.last.y;
        let shift = |p: &mut NormalizedPoint| {
            p.x += dx;
            p.y += dy;
        };
        let moving = if self.selection.contains(&drag.id) {
            self.selection.clone()
        } else {
            vec![drag.id]
        };
        for ann in self
            .annotations
            .iter_mut()
            .filter(|ann| moving.contains(&ann.id()))
        {
            match ann {
                DrawAnnotation::Text(t) => {
                    t.position = NormalizedPoint::new(t.position.x + dx, t.position.y + dy);
                }
                DrawAnnotation::Stroke(s) => s.points.iter_mut().for_each(shift),
                DrawAnnotation::Shape(s) => {
                    shift(&mut s.start);
                    shift(&mut s.end);
                }
            }
        }
        true
    }

    fn update_marquee(
        &mut self,
        marquee: Marquee,
        point: NormalizedPoint,
        measure: &dyn Fn(&TextAnnotation) -> f32,
    ) -> bool {
        let marquee = Marquee {
            end: point,
            ..marquee
        };
        self.marquee = Some(marquee);
        self.selection = if marquee.start.distance(&point) < TEXT_DRAG_THRESHOLD {
            Vec::new()
        } else {
            let frame = marquee.frame();
            self.annotations
                .iter()
                .filter(|ann| {
                    self.annotation_bounds(ann, marquee.canvas, measure)
                        .is_some_and(|bounds| bounds.intersects(&frame))
                })
                .map(DrawAnnotation::id)
                .collect()
        };
        true
    }

    pub fn start_stroke(&mut self, x: f32, y: f32) {
        let id = self.take_id();
        let kind = if self.active_tool == DrawTool::Highlighter {
            StrokeKind::Highlighter
        } else {
            StrokeKind::Pen
        };
        self.current_stroke = Some(PenStroke {
            id,
            color: self.stroke_color,
            width: self.stroke_width,
            points: vec![NormalizedPoint::new(x, y)],
            kind,
        });
    }

    pub fn add_stroke_point(&mut self, x: f32, y: f32) -> bool {
        let Some(stroke) = self.current_stroke.as_mut() else {
            return false;
        };
        let pt = NormalizedPoint::new(x, y);
        if stroke
            .points
            .last()
            .is_some_and(|last| last.distance_squared(&pt) < MIN_POINT_SPACING_SQUARED)
        {
            return false;
        }
        stroke.points.push(pt);
        true
    }

    pub fn finish_stroke(&mut self) {
        let Some(stroke) = self.current_stroke.take() else {
            return;
        };
        if stroke.points.is_empty() {
            return;
        }
        if self.active_tool == DrawTool::Laser {
            self.vanishing.push(VanishingStroke {
                stroke,
                released_at: Instant::now(),
            });
            return;
        }
        self.push_undo();
        self.annotations.push(DrawAnnotation::Stroke(stroke));
    }

    pub fn cancel_stroke(&mut self) {
        self.current_stroke = None;
        self.current_shape = None;
        self.drag = None;
        self.text_resize = None;
        self.marquee = None;
    }

    fn start_shape(&mut self, kind: ShapeKind, point: NormalizedPoint) {
        let id = self.take_id();
        self.current_shape = Some(ShapeAnnotation {
            id,
            kind,
            color: self.stroke_color,
            width: self.stroke_width,
            start: point,
            end: point,
        });
    }

    fn update_shape(&mut self, point: NormalizedPoint) {
        if let Some(shape) = self.current_shape.as_mut() {
            shape.end = point;
        }
    }

    fn finish_shape(&mut self) {
        let Some(shape) = self.current_shape.take() else {
            return;
        };
        let dx = (shape.end.x - shape.start.x).abs();
        let dy = (shape.end.y - shape.start.y).abs();
        let big_enough = match shape.kind {
            ShapeKind::Arrow => dx.max(dy) >= MIN_SHAPE_SIZE,
            ShapeKind::Rectangle | ShapeKind::Ellipse => {
                dx >= MIN_SHAPE_SIZE && dy >= MIN_SHAPE_SIZE
            }
        };
        if big_enough {
            self.push_undo();
            self.annotations.push(DrawAnnotation::Shape(shape));
        }
    }

    pub fn add_text(&mut self, x: f32, y: f32, content: String) -> u64 {
        self.push_undo();
        let id = self.take_id();
        self.annotations.push(DrawAnnotation::Text(TextAnnotation {
            id,
            color: self.stroke_color,
            font_size: self.text_font_size,
            position: NormalizedPoint::new(x, y),
            content,
        }));
        id
    }

    pub fn update_text(&mut self, id: u64, content: String) {
        if let Some(item) = self.annotations.iter_mut().find_map(|ann| match ann {
            DrawAnnotation::Text(t) if t.id == id => Some(t),
            _ => None,
        }) {
            item.content = content;
        }
    }

    pub fn editing_text(&self) -> Option<&TextAnnotation> {
        let id = self.editing_text_id?;
        self.annotations.iter().find_map(|ann| match ann {
            DrawAnnotation::Text(t) if t.id == id => Some(t),
            _ => None,
        })
    }

    pub fn delete_annotation(&mut self, id: u64) -> bool {
        if !self.annotations.iter().any(|ann| ann.id() == id) {
            return false;
        }
        self.push_undo();
        self.annotations.retain(|ann| ann.id() != id);
        true
    }

    pub fn delete_selection(&mut self) -> bool {
        let targets: Vec<u64> = if self.selection.is_empty() {
            self.selected.into_iter().collect()
        } else {
            self.selection.clone()
        };
        if !self
            .annotations
            .iter()
            .any(|ann| targets.contains(&ann.id()))
        {
            return false;
        }
        self.push_undo();
        self.annotations.retain(|ann| !targets.contains(&ann.id()));
        self.selection.clear();
        self.selected = None;
        true
    }

    pub fn erase_at(&mut self, x: f32, y: f32, radius: f32) -> bool {
        let pt = NormalizedPoint::new(x, y);
        let hit = |ann: &DrawAnnotation| match ann {
            DrawAnnotation::Stroke(s) => {
                stroke_intersects_point(s, &pt, radius + (s.render_width() / 1000.0).max(0.005))
            }
            DrawAnnotation::Shape(s) => {
                shape_intersects_point(s, &pt, radius + (s.width / 1000.0).max(0.005))
            }
            DrawAnnotation::Text(t) => {
                text_intersects_point(t, &pt, radius + (t.font_size / 1000.0).max(0.015))
            }
        };
        if !self.annotations.iter().any(hit) {
            return false;
        }
        self.push_undo();
        self.annotations.retain(|ann| !hit(ann));
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            return false;
        };
        self.editing_text_id = None;
        self.drag = None;
        self.text_resize = None;
        let current = std::mem::replace(&mut self.annotations, prev);
        self.redo_stack.push(current);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.editing_text_id = None;
        self.drag = None;
        self.text_resize = None;
        let current = std::mem::replace(&mut self.annotations, next);
        self.undo_stack.push(current);
        true
    }

    pub fn start_editing_text(&mut self, id: u64) {
        if let Some(content) = self.annotations.iter().find_map(|a| match a {
            DrawAnnotation::Text(t) if t.id == id => Some(t.content.clone()),
            _ => None,
        }) {
            self.editing_text_content = content;
            self.editing_text_id = Some(id);
        }
    }

    pub fn set_editing_text_content(&mut self, content: String) {
        let content = content.replace(['\n', '\r'], " ");
        if let Some(id) = self.editing_text_id {
            self.update_text(id, content.clone());
            self.editing_text_content = content;
        }
    }

    pub fn commit_editing_text(&mut self) {
        let Some(id) = self.editing_text_id.take() else {
            return;
        };
        if self.editing_text_content.trim().is_empty() {
            self.discard_text(id);
        } else {
            let content = std::mem::take(&mut self.editing_text_content);
            self.update_text(id, content);
            self.selected = Some(id);
        }
        self.editing_text_content.clear();
    }

    pub fn cancel_editing_text(&mut self) {
        let Some(id) = self.editing_text_id.take() else {
            return;
        };
        let empty = self.annotations.iter().any(|a| match a {
            DrawAnnotation::Text(t) => t.id == id && t.content.trim().is_empty(),
            _ => false,
        });
        if empty {
            self.discard_text(id);
        }
        self.editing_text_content.clear();
    }

    fn discard_text(&mut self, id: u64) {
        self.annotations.retain(|ann| ann.id() != id);
        if self.undo_stack.last() == Some(&self.annotations) {
            self.undo_stack.pop();
        }
    }

    pub fn clear(&mut self) {
        self.current_stroke = None;
        self.current_shape = None;
        self.drag = None;
        self.text_resize = None;
        self.selected = None;
        self.selection.clear();
        self.marquee = None;
        self.vanishing.clear();
        self.editing_text_id = None;
        self.editing_text_content.clear();
        if !self.annotations.is_empty() {
            self.push_undo();
            self.annotations.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn has_content(&self) -> bool {
        !self.annotations.is_empty()
            || !self.vanishing.is_empty()
            || self.current_stroke.is_some()
            || self.current_shape.is_some()
    }

    pub fn prune_vanishing(&mut self, now: Instant) -> bool {
        let before = self.vanishing.len();
        self.vanishing.retain(|stroke| stroke.alpha(now) > 0.0);
        before != self.vanishing.len()
    }

    pub fn annotation_scene(
        &self,
        canvas_width: f32,
        canvas_height: f32,
        now: Instant,
    ) -> AnnotationScene {
        let mut items = Vec::with_capacity(self.annotations.len() + self.vanishing.len() + 2);
        for ann in &self.annotations {
            items.push(match ann {
                DrawAnnotation::Stroke(stroke) => stroke_item(stroke, stroke.render_color()),
                DrawAnnotation::Shape(shape) => shape_item(shape),
                DrawAnnotation::Text(text) => AnnotationItem::Text {
                    position: text.position.to_annotation(),
                    content: text.content.clone(),
                    color: text.color,
                    font_size: text.font_size,
                },
            });
        }
        for vanishing in &self.vanishing {
            let color = scale_alpha(vanishing.stroke.render_color(), vanishing.alpha(now));
            items.push(stroke_item(&vanishing.stroke, color));
        }
        if let Some(stroke) = &self.current_stroke {
            items.push(stroke_item(stroke, stroke.render_color()));
        }
        if let Some(shape) = &self.current_shape {
            items.push(shape_item(shape));
        }
        AnnotationScene {
            canvas_width,
            canvas_height,
            items,
        }
    }
}

fn stroke_item(stroke: &PenStroke, color: u32) -> AnnotationItem {
    AnnotationItem::Stroke {
        points: stroke.points.iter().map(|p| p.to_annotation()).collect(),
        color,
        width: stroke.render_width(),
    }
}

fn shape_item(shape: &ShapeAnnotation) -> AnnotationItem {
    let from = shape.start.to_annotation();
    let to = shape.end.to_annotation();
    let (color, width) = (shape.color, shape.width);
    match shape.kind {
        ShapeKind::Arrow => AnnotationItem::Arrow {
            from,
            to,
            color,
            width,
        },
        ShapeKind::Rectangle => AnnotationItem::Rectangle {
            from,
            to,
            color,
            width,
        },
        ShapeKind::Ellipse => AnnotationItem::Ellipse {
            from,
            to,
            color,
            width,
        },
    }
}

fn stroke_intersects_point(stroke: &PenStroke, pt: &NormalizedPoint, radius: f32) -> bool {
    let r2 = radius * radius;
    if stroke.points.len() == 1 {
        return stroke.points[0].distance_squared(pt) <= r2;
    }
    stroke
        .points
        .windows(2)
        .any(|pair| point_to_segment_distance_squared(pt, &pair[0], &pair[1]) <= r2)
}

fn shape_intersects_point(shape: &ShapeAnnotation, pt: &NormalizedPoint, radius: f32) -> bool {
    let r2 = radius * radius;
    let (a, b) = (shape.start, shape.end);
    match shape.kind {
        ShapeKind::Arrow => point_to_segment_distance_squared(pt, &a, &b) <= r2,
        ShapeKind::Rectangle => {
            let corners = [
                NormalizedPoint { x: a.x, y: a.y },
                NormalizedPoint { x: b.x, y: a.y },
                NormalizedPoint { x: b.x, y: b.y },
                NormalizedPoint { x: a.x, y: b.y },
            ];
            (0..4).any(|i| {
                point_to_segment_distance_squared(pt, &corners[i], &corners[(i + 1) % 4]) <= r2
            })
        }
        ShapeKind::Ellipse => {
            let cx = (a.x + b.x) / 2.0;
            let cy = (a.y + b.y) / 2.0;
            let rx = (b.x - a.x).abs() / 2.0;
            let ry = (b.y - a.y).abs() / 2.0;
            let at = |i: usize| {
                let angle = i as f32 / ELLIPSE_HIT_SEGMENTS as f32 * std::f32::consts::TAU;
                NormalizedPoint {
                    x: cx + rx * angle.cos(),
                    y: cy + ry * angle.sin(),
                }
            };
            (0..ELLIPSE_HIT_SEGMENTS)
                .any(|i| point_to_segment_distance_squared(pt, &at(i), &at(i + 1)) <= r2)
        }
    }
}

fn text_intersects_point(text: &TextAnnotation, pt: &NormalizedPoint, radius: f32) -> bool {
    let approx_width = (text.content.len() as f32 * 0.012).clamp(0.02, 0.4);
    let approx_height = (text.font_size / 600.0).clamp(0.02, 0.08);

    let left = text.position.x - radius;
    let right = text.position.x + approx_width + radius;
    let top = text.position.y - radius;
    let bottom = text.position.y + approx_height + radius;

    pt.x >= left && pt.x <= right && pt.y >= top && pt.y <= bottom
}

fn point_to_segment_distance_squared(
    p: &NormalizedPoint,
    a: &NormalizedPoint,
    b: &NormalizedPoint,
) -> f32 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let l2 = vx * vx + vy * vy;
    if l2 == 0.0 {
        return p.distance_squared(a);
    }
    let wx = p.x - a.x;
    let wy = p.y - a.y;
    let t = ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0);
    let dx = p.x - (a.x + t * vx);
    let dy = p.y - (a.y + t * vy);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draw_state_defaults() {
        let state = ScreenDrawState::default();
        assert!(!state.is_active);
        assert_eq!(state.active_tool, DrawTool::Pen);
        assert_eq!(state.stroke_color, COLOR_RED);
        assert!(state.annotations.is_empty());
    }

    #[test]
    fn test_pen_stroke_lifecycle() {
        let mut state = ScreenDrawState::default();
        state.set_active(true);
        state.start_stroke(0.1, 0.2);
        state.add_stroke_point(0.15, 0.25);
        state.add_stroke_point(0.2, 0.3);
        assert!(state.current_stroke.is_some());
        state.finish_stroke();

        assert_eq!(state.annotations.len(), 1);
        assert!(state.current_stroke.is_none());
        assert!(state.can_undo());

        if let DrawAnnotation::Stroke(s) = &state.annotations[0] {
            assert_eq!(s.points.len(), 3);
            assert_eq!(s.color, COLOR_RED);
        } else {
            panic!("Expected stroke annotation");
        }
    }

    #[test]
    fn test_text_annotation() {
        let mut state = ScreenDrawState::default();
        let id = state.add_text(0.3, 0.4, "Hello Mezon".to_string());
        assert_eq!(state.annotations.len(), 1);
        state.update_text(id, "Updated text".to_string());

        if let DrawAnnotation::Text(t) = &state.annotations[0] {
            assert_eq!(t.content, "Updated text");
            assert_eq!(t.position.x, 0.3);
            assert_eq!(t.position.y, 0.4);
        } else {
            panic!("Expected text annotation");
        }
    }

    #[test]
    fn test_eraser_hit_test() {
        let mut state = ScreenDrawState::default();
        state.start_stroke(0.1, 0.1);
        state.add_stroke_point(0.5, 0.5);
        state.finish_stroke();
        assert_eq!(state.annotations.len(), 1);

        let erased = state.erase_at(0.9, 0.9, 0.05);
        assert!(!erased);
        assert_eq!(state.annotations.len(), 1);

        let erased = state.erase_at(0.3, 0.3, 0.05);
        assert!(erased);
        assert_eq!(state.annotations.len(), 0);
    }

    #[test]
    fn test_undo_redo_and_clear() {
        let mut state = ScreenDrawState::default();
        state.start_stroke(0.1, 0.1);
        state.add_stroke_point(0.2, 0.2);
        state.finish_stroke();

        state.add_text(0.5, 0.5, "Text".to_string());
        assert_eq!(state.annotations.len(), 2);

        assert!(state.undo());
        assert_eq!(state.annotations.len(), 1);

        assert!(state.redo());
        assert_eq!(state.annotations.len(), 2);

        state.clear();
        assert_eq!(state.annotations.len(), 0);

        assert!(state.undo());
        assert_eq!(state.annotations.len(), 2);
    }

    #[test]
    fn test_content_viewport_aspect_ratio_contain() {
        let vp = ContentViewport::compute(1920.0, 800.0, 1920.0, 1080.0);
        assert!((vp.height - 800.0).abs() < 0.01);
        let expected_w = 800.0 * (16.0 / 9.0);
        assert!((vp.width - expected_w).abs() < 0.01);
        assert!((vp.offset_x - (1920.0 - expected_w) * 0.5).abs() < 0.01);
        assert_eq!(vp.offset_y, 0.0);

        let norm = vp.to_normalized(vp.offset_x + 0.5 * vp.width, vp.offset_y + 0.5 * vp.height);
        assert!((norm.x - 0.5).abs() < 0.001);
        assert!((norm.y - 0.5).abs() < 0.001);

        let (px, py) = vp.to_pixels(&norm);
        assert!((px - (vp.offset_x + 0.5 * vp.width)).abs() < 0.01);
        assert!((py - (vp.offset_y + 0.5 * vp.height)).abs() < 0.01);
    }

    #[test]
    fn test_text_editing_commit_and_cancel() {
        let mut state = ScreenDrawState::default();
        let id = state.add_text(0.2, 0.2, String::new());
        state.start_editing_text(id);
        assert_eq!(state.editing_text_id, Some(id));

        state.editing_text_content = "   ".to_string();
        state.commit_editing_text();
        assert_eq!(state.annotations.len(), 0);
        assert_eq!(state.editing_text_id, None);

        let id2 = state.add_text(0.3, 0.3, "Initial".to_string());
        state.start_editing_text(id2);
        state.editing_text_content = "Updated note".to_string();
        state.commit_editing_text();
        assert_eq!(state.annotations.len(), 1);
        if let DrawAnnotation::Text(t) = &state.annotations[0] {
            assert_eq!(t.content, "Updated note");
        }
    }
}
