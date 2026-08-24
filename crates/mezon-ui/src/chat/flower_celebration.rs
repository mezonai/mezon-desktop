use std::time::Instant;

use gpui::{
    App, Bounds, Corners, Element, ElementId, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, PathBuilder, Pixels, Point, SharedString, Style,
    TextAlign, TruncateFrom, Window, black, fill, point, px, relative, rgb, size, white,
};

const MAX_PARTICLES: usize = 420;

const PINK: u32 = 0xff6ed4;
const BLUE: u32 = 0x5cb8ec;
const GOLD: u32 = 0xffb600;
const GOLD_RIBBON: u32 = 0xffb90c;
const GOLD_DARK: u32 = 0xe2a100;
const PURPLE: u32 = 0x7037ff;
const CYAN: u32 = 0x50deef;
const HOT_PINK: u32 = 0xff37b0;
const BOX_RED: u32 = 0xd33a3a;
const LID_RED: u32 = 0xce3737;
const LID_DEPTH: u32 = 0xb52525;

const FIREWORK_COLORS: [u32; 7] = [0x1fd7ff, 0xff1f90, 0xff4333, 0x008545, PINK, BLUE, GOLD];
const RIBBON_COLORS: [u32; 6] = [PURPLE, CYAN, HOT_PINK, GOLD, 0x1fd7ff, 0xff1f90];

const BOX_BOUNCE_MS: f32 = 350.0;
const LID_START_MS: f32 = 360.0;
const LID_END_MS: f32 = 920.0;
const BURST_MS: f32 = 380.0;
const WAVE2_MS: f32 = 460.0;
const FLOWER_START_MS: f32 = 400.0;
const FLOWER_END_MS: f32 = 780.0;
const FADE_START_MS: f32 = 2000.0;
const FADE_END_MS: f32 = 2600.0;
const LID_LIFT: f32 = 220.0;
const LID_TILT: f32 = -0.22;

const CANNON_COUNT: usize = 36;
const BOX_RIBBONS: usize = 14;
const BOX_STARS: usize = 14;
const WAVE2_COUNT: usize = 28;

const BOX_W: f32 = 177.0;
const BOX_H: f32 = 129.0;
const RIBBON_W: f32 = 24.0;
const ISO_DX: f32 = 36.0;
const ISO_DY: f32 = -24.0;
const LID_LIP: f32 = 20.0;
const FACE_RADIUS: f32 = 12.0;
const REF_FRAME_MS: f32 = 16.0;
const CAPTION_MAX_WIDTH_FRACTION: f32 = 0.85;
const CAPTION_ELLIPSIS: &str = "…";

#[derive(Clone, Copy)]
enum ParticleShape {
    Rect,
    Circle,
    Streamer,
    Ribbon,
    Star,
    Square,
}

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    gravity: f32,
    drag: f32,
    size: f32,
    color: Hsla,
    shape: ParticleShape,
    rotation: f32,
    rot_speed: f32,
    life: f32,
    decay: f32,
    flutter: f32,
    flutter_speed: f32,
    length: f32,
    amp: f32,
    opacity: f32,
    pop: f32,
    fall_gravity: f32,
}

#[derive(Clone, Copy)]
struct GiftScene {
    t: f32,
    x: f32,
    y: f32,
    lid_lift: f32,
    lid_rot: f32,
    flower_scale: f32,
    alpha: f32,
    box_scale: f32,
    squash: f32,
    shadow_alpha: f32,
    burst_fired: bool,
    wave2_fired: bool,
}

struct CelebrationRuntime {
    started_at: Option<Instant>,
    last_frame: Option<Instant>,
    scene: Option<GiftScene>,
    particles: Vec<Particle>,
    rng: u64,
    caption_key: Option<SharedString>,
    caption_appeared_at: Option<Instant>,
}

impl Default for CelebrationRuntime {
    fn default() -> Self {
        Self {
            started_at: None,
            last_frame: None,
            scene: None,
            particles: Vec::new(),
            rng: 1,
            caption_key: None,
            caption_appeared_at: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Xf {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Xf {
    fn unit() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn then(self, o: Self) -> Self {
        Self {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    fn translate(self, x: f32, y: f32) -> Self {
        self.then(Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        })
    }

    fn scale_xy(self, sx: f32, sy: f32) -> Self {
        self.then(Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        })
    }

    fn rotate(self, rad: f32) -> Self {
        let (s, c) = rad.sin_cos();
        self.then(Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        })
    }

    fn pt(self, x: f32, y: f32) -> Point<Pixels> {
        let (x, y) = self.apply(x, y);
        point(px(x), px(y))
    }
}

fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

fn range01(t: f32, start: f32, end: f32) -> f32 {
    clamp01((t - start) / (end - start))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn hex(color: u32) -> Hsla {
    rgb(color).into()
}

fn rng_f32(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*state >> 33) as f32) * (1.0 / ((1u32 << 31) as f32))
}

fn pick_u32(state: &mut u64, items: &[u32]) -> u32 {
    items[(rng_f32(state) * items.len() as f32) as usize % items.len()]
}

fn create_gift_scene(x: f32, y: f32) -> GiftScene {
    GiftScene {
        t: 0.0,
        x,
        y,
        lid_lift: 0.0,
        lid_rot: 0.0,
        flower_scale: 0.0,
        alpha: 1.0,
        box_scale: 0.7,
        squash: 1.0,
        shadow_alpha: 0.35,
        burst_fired: false,
        wave2_fired: false,
    }
}

fn update_gift_scene(scene: &mut GiftScene, dt_ms: f32) -> (bool, bool) {
    scene.t += dt_ms;
    let mut burst = false;
    let mut wave2 = false;

    if scene.t < BOX_BOUNCE_MS {
        let u = clamp01(scene.t / BOX_BOUNCE_MS);
        scene.box_scale = lerp(0.7, 1.0, ease_out_back(u));
        scene.squash = if u < 0.45 {
            lerp(0.86, 1.12, u / 0.45)
        } else {
            lerp(1.12, 1.0, (u - 0.45) / 0.55)
        };
    } else {
        scene.box_scale = 1.0;
        scene.squash = 1.0;
    }

    let lid_t = range01(scene.t, LID_START_MS, LID_END_MS);
    scene.lid_lift = LID_LIFT * ease_out_cubic(lid_t);
    scene.lid_rot = LID_TILT * ease_out_cubic(lid_t);
    scene.shadow_alpha = lerp(0.35, 0.08, lid_t);

    let flower_t = range01(scene.t, FLOWER_START_MS, FLOWER_END_MS);
    scene.flower_scale = if flower_t <= 0.0 {
        0.0
    } else {
        ease_out_back(flower_t)
    };
    scene.alpha = if scene.t < FADE_START_MS {
        1.0
    } else {
        1.0 - range01(scene.t, FADE_START_MS, FADE_END_MS)
    };

    if !scene.burst_fired && scene.t >= BURST_MS {
        scene.burst_fired = true;
        burst = true;
    }
    if !scene.wave2_fired && scene.t >= WAVE2_MS {
        scene.wave2_fired = true;
        wave2 = true;
    }
    (burst, wave2)
}

fn create_arc_particle(
    rng: &mut u64,
    x: f32,
    y: f32,
    aim: f32,
    shape: ParticleShape,
    color: Hsla,
    ribbon: bool,
    opacity: f32,
) -> Particle {
    let spread = if ribbon {
        std::f32::consts::PI * 0.95
    } else {
        0.85
    };
    let dir = aim + (rng_f32(rng) - 0.5) * spread;
    let speed = if ribbon {
        9.0 + rng_f32(rng) * 11.0
    } else {
        11.0 + rng_f32(rng) * 15.0
    };
    let size = match shape {
        ParticleShape::Streamer | ParticleShape::Ribbon => 4.5 + rng_f32(rng) * 2.4,
        _ => 7.0 + rng_f32(rng) * 7.0,
    };
    Particle {
        x,
        y,
        vx: dir.cos() * speed,
        vy: dir.sin() * speed,
        gravity: if ribbon {
            0.18 + rng_f32(rng) * 0.08
        } else {
            0.36 + rng_f32(rng) * 0.1
        },
        fall_gravity: if ribbon {
            0.05 + rng_f32(rng) * 0.03
        } else {
            0.07 + rng_f32(rng) * 0.035
        },
        drag: if ribbon { 0.993 } else { 0.991 },
        size,
        color,
        shape,
        rotation: rng_f32(rng) * std::f32::consts::TAU,
        rot_speed: (rng_f32(rng) - 0.5) * if ribbon { 0.12 } else { 0.5 },
        life: 1.0,
        decay: if ribbon {
            0.0014 + rng_f32(rng) * 0.0008
        } else {
            0.0015 + rng_f32(rng) * 0.0009
        },
        flutter: rng_f32(rng) * std::f32::consts::TAU,
        flutter_speed: 0.12 + rng_f32(rng) * 0.16,
        length: if ribbon {
            72.0 + rng_f32(rng) * 40.0
        } else {
            38.0 + rng_f32(rng) * 22.0
        },
        amp: if ribbon {
            14.0 + rng_f32(rng) * 10.0
        } else {
            8.0 + rng_f32(rng) * 8.0
        },
        opacity,
        pop: 0.0,
    }
}

const CANNON_SHAPES: [ParticleShape; 9] = [
    ParticleShape::Circle,
    ParticleShape::Circle,
    ParticleShape::Star,
    ParticleShape::Star,
    ParticleShape::Rect,
    ParticleShape::Rect,
    ParticleShape::Square,
    ParticleShape::Square,
    ParticleShape::Streamer,
];

fn cap_particles(particles: &mut Vec<Particle>) {
    if particles.len() > MAX_PARTICLES {
        let drop = particles.len() - MAX_PARTICLES;
        particles.drain(0..drop);
    }
}

fn spawn_cannon(
    particles: &mut Vec<Particle>,
    rng: &mut u64,
    x: f32,
    y: f32,
    aim: f32,
    count: usize,
    opacity: f32,
) {
    for _ in 0..count {
        let shape = CANNON_SHAPES
            [(rng_f32(rng) * CANNON_SHAPES.len() as f32) as usize % CANNON_SHAPES.len()];
        let color = hex(pick_u32(rng, &FIREWORK_COLORS));
        particles.push(create_arc_particle(
            rng, x, y, aim, shape, color, false, opacity,
        ));
    }
}

fn spawn_all_cannons(
    particles: &mut Vec<Particle>,
    rng: &mut u64,
    origin_x: f32,
    origin_y: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
) {
    spawn_cannon(
        particles,
        rng,
        origin_x + width * 0.88,
        origin_y + height * 0.78,
        -std::f32::consts::FRAC_PI_2 - 0.55,
        CANNON_COUNT,
        opacity,
    );
    spawn_cannon(
        particles,
        rng,
        origin_x + width * 0.12,
        origin_y + height * 0.78,
        -std::f32::consts::FRAC_PI_2 + 0.55,
        CANNON_COUNT,
        opacity,
    );
    spawn_cannon(
        particles,
        rng,
        origin_x + width * 0.5,
        origin_y + height * 0.5,
        -std::f32::consts::FRAC_PI_2,
        WAVE2_COUNT,
        opacity,
    );
    spawn_cannon(
        particles,
        rng,
        x,
        y - 16.0,
        -std::f32::consts::FRAC_PI_2,
        12,
        opacity,
    );
}

fn spawn_main_burst(
    particles: &mut Vec<Particle>,
    rng: &mut u64,
    origin_x: f32,
    origin_y: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    spawn_all_cannons(particles, rng, origin_x, origin_y, x, y, width, height, 1.0);
    for _ in 0..BOX_RIBBONS {
        let color = hex(pick_u32(rng, &RIBBON_COLORS));
        let opacity = if rng_f32(rng) < 0.35 { 0.55 } else { 1.0 };
        particles.push(create_arc_particle(
            rng,
            x,
            y - 20.0,
            -std::f32::consts::FRAC_PI_2,
            ParticleShape::Ribbon,
            color,
            true,
            opacity,
        ));
    }
    for _ in 0..BOX_STARS {
        let color = hex(pick_u32(rng, &FIREWORK_COLORS));
        particles.push(create_arc_particle(
            rng,
            x,
            y - 24.0,
            -std::f32::consts::FRAC_PI_2,
            ParticleShape::Star,
            color,
            false,
            1.0,
        ));
    }
    cap_particles(particles);
}

fn spawn_wave2_burst(
    particles: &mut Vec<Particle>,
    rng: &mut u64,
    origin_x: f32,
    origin_y: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    spawn_all_cannons(
        particles, rng, origin_x, origin_y, x, y, width, height, 0.55,
    );
    for _ in 0..8 {
        let color = hex(pick_u32(rng, &RIBBON_COLORS));
        particles.push(create_arc_particle(
            rng,
            x,
            y - 20.0,
            -std::f32::consts::FRAC_PI_2,
            ParticleShape::Ribbon,
            color,
            true,
            0.7,
        ));
    }
    cap_particles(particles);
}

fn paint_path(window: &mut Window, builder: PathBuilder, color: Hsla) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn fill_ellipse(
    window: &mut Window,
    xf: Xf,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    rot: f32,
    color: Hsla,
) {
    if rx <= 0.1 || ry <= 0.1 {
        return;
    }
    let xf = xf.translate(cx, cy).rotate(rot);
    let kappa = 0.552_284_8;
    let ox = rx * kappa;
    let oy = ry * kappa;
    let mut b = PathBuilder::fill();
    b.move_to(xf.pt(rx, 0.0));
    b.cubic_bezier_to(xf.pt(0.0, ry), xf.pt(rx, oy), xf.pt(ox, ry));
    b.cubic_bezier_to(xf.pt(-rx, 0.0), xf.pt(-ox, ry), xf.pt(-rx, oy));
    b.cubic_bezier_to(xf.pt(0.0, -ry), xf.pt(-rx, -oy), xf.pt(-ox, -ry));
    b.cubic_bezier_to(xf.pt(rx, 0.0), xf.pt(ox, -ry), xf.pt(rx, -oy));
    b.close();
    paint_path(window, b, color);
}

fn fill_round_rect(
    window: &mut Window,
    xf: Xf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    color: Hsla,
) {
    if w.abs() < 0.5 || h.abs() < 0.5 {
        return;
    }
    let (x0, y0) = xf.apply(x, y);
    let (x1, y1) = xf.apply(x + w, y);
    let (x2, y2) = xf.apply(x + w, y + h);
    let (x3, y3) = xf.apply(x, y + h);
    let axis_aligned = (y0 - y1).abs() < 0.2 && (x0 - x3).abs() < 0.2;
    if axis_aligned {
        let left = x0.min(x1).min(x2).min(x3);
        let top = y0.min(y1).min(y2).min(y3);
        let right = x0.max(x1).max(x2).max(x3);
        let bottom = y0.max(y1).max(y2).max(y3);
        window.paint_quad(
            fill(
                Bounds {
                    origin: point(px(left), px(top)),
                    size: size(px(right - left), px(bottom - top)),
                },
                color,
            )
            .corner_radii(Corners::all(px(r))),
        );
        return;
    }
    let mut b = PathBuilder::fill();
    b.move_to(point(px(x0), px(y0)));
    b.line_to(point(px(x1), px(y1)));
    b.line_to(point(px(x2), px(y2)));
    b.line_to(point(px(x3), px(y3)));
    b.close();
    paint_path(window, b, color);
}

fn rounded_poly(window: &mut Window, xf: Xf, pts: &[(f32, f32)], radius: f32, color: Hsla) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let mapped: Vec<(f32, f32)> = pts.iter().map(|(x, y)| xf.apply(*x, *y)).collect();
    let mut b = PathBuilder::fill();
    for i in 0..n {
        let prev = mapped[(i + n - 1) % n];
        let curr = mapped[i];
        let next = mapped[(i + 1) % n];
        let dx1 = curr.0 - prev.0;
        let dy1 = curr.1 - prev.1;
        let dx2 = next.0 - curr.0;
        let dy2 = next.1 - curr.1;
        let len1 = dx1.hypot(dy1).max(1.0);
        let len2 = dx2.hypot(dy2).max(1.0);
        let r = radius.min(len1 / 2.0).min(len2 / 2.0);
        let p1x = curr.0 - (dx1 / len1) * r;
        let p1y = curr.1 - (dy1 / len1) * r;
        let p2x = curr.0 + (dx2 / len2) * r;
        let p2y = curr.1 + (dy2 / len2) * r;
        if i == 0 {
            b.move_to(point(px(p1x), px(p1y)));
        } else {
            b.line_to(point(px(p1x), px(p1y)));
        }
        b.curve_to(point(px(p2x), px(p2y)), point(px(curr.0), px(curr.1)));
    }
    b.close();
    paint_path(window, b, color);
}

fn fill_front_face(window: &mut Window, xf: Xf, left: f32, top: f32, w: f32, h: f32, color: Hsla) {
    rounded_poly(
        window,
        xf,
        &[
            (left, top),
            (left + w, top),
            (left + w, top + h),
            (left, top + h),
        ],
        FACE_RADIUS,
        color,
    );
}

fn fill_right_face(window: &mut Window, xf: Xf, left: f32, top: f32, w: f32, h: f32, color: Hsla) {
    rounded_poly(
        window,
        xf,
        &[
            (left + w, top),
            (left + w + ISO_DX, top + ISO_DY),
            (left + w + ISO_DX, top + h + ISO_DY),
            (left + w, top + h),
        ],
        FACE_RADIUS,
        color,
    );
}

fn fill_top_face(window: &mut Window, xf: Xf, left: f32, top: f32, w: f32, color: Hsla) {
    rounded_poly(
        window,
        xf,
        &[
            (left, top),
            (left + w, top),
            (left + w + ISO_DX, top + ISO_DY),
            (left + ISO_DX, top + ISO_DY),
        ],
        FACE_RADIUS,
        color,
    );
}

fn path_right_band(
    window: &mut Window,
    xf: Xf,
    left: f32,
    y: f32,
    w: f32,
    band_h: f32,
    color: Hsla,
) {
    let mut b = PathBuilder::fill();
    b.move_to(xf.pt(left + w, y));
    b.line_to(xf.pt(left + w + ISO_DX, y + ISO_DY));
    b.line_to(xf.pt(left + w + ISO_DX, y + band_h + ISO_DY));
    b.line_to(xf.pt(left + w, y + band_h));
    b.close();
    paint_path(window, b, color);
}

fn draw_bow(window: &mut Window, xf: Xf, alpha: f32) {
    let xf = xf.scale_xy(1.5, 1.5);
    let gold = hex(GOLD_RIBBON).opacity(alpha);
    let dark = hex(GOLD_DARK).opacity(alpha);
    fill_ellipse(window, xf, -16.0, -6.0, 16.0, 9.0, -0.5, gold);
    fill_ellipse(window, xf, 16.0, -6.0, 16.0, 9.0, 0.5, gold);
    let mut left = PathBuilder::fill();
    left.move_to(xf.pt(-6.0, 2.0));
    left.curve_to(xf.pt(-10.0, 34.0), xf.pt(-18.0, 22.0));
    left.curve_to(xf.pt(0.0, 8.0), xf.pt(-2.0, 18.0));
    left.close();
    paint_path(window, left, dark);
    let mut right = PathBuilder::fill();
    right.move_to(xf.pt(6.0, 2.0));
    right.curve_to(xf.pt(10.0, 34.0), xf.pt(18.0, 22.0));
    right.curve_to(xf.pt(0.0, 8.0), xf.pt(2.0, 18.0));
    right.close();
    paint_path(window, right, dark);
    fill_ellipse(window, xf, 0.0, -2.0, 6.5, 6.5, 0.0, gold);
}

struct BloomPetal {
    a: f32,
    dx: f32,
    dy: f32,
    rx: f32,
    ry: f32,
    c: u32,
}

const PINK_PALETTE: [u32; 5] = [0xff8ad0, 0xff6ed4, 0xff4ec4, 0xff79d4, 0xffb3e4];
const ORANGE_PALETTE: [u32; 5] = [0xffb14a, 0xff8a1a, 0xff9f32, 0xffd18a, 0xff7a00];
const BEIGE_PALETTE: [u32; 5] = [0xf4e2c4, 0xe6d0a6, 0xdcc49a, 0xf8edd8, 0xc9b089];

struct Bloom {
    x: f32,
    y: f32,
    s: f32,
    rot: f32,
    palette: [u32; 5],
}

const BLOOMS: [Bloom; 9] = [
    Bloom {
        x: 0.0,
        y: -52.0,
        s: 1.0,
        rot: -0.04,
        palette: PINK_PALETTE,
    },
    Bloom {
        x: -36.0,
        y: -40.0,
        s: 0.84,
        rot: -0.38,
        palette: ORANGE_PALETTE,
    },
    Bloom {
        x: 38.0,
        y: -42.0,
        s: 0.86,
        rot: 0.34,
        palette: BEIGE_PALETTE,
    },
    Bloom {
        x: -10.0,
        y: -34.0,
        s: 0.78,
        rot: -0.16,
        palette: ORANGE_PALETTE,
    },
    Bloom {
        x: 12.0,
        y: -32.0,
        s: 0.76,
        rot: 0.14,
        palette: BEIGE_PALETTE,
    },
    Bloom {
        x: -52.0,
        y: -22.0,
        s: 0.72,
        rot: -0.55,
        palette: BEIGE_PALETTE,
    },
    Bloom {
        x: 54.0,
        y: -20.0,
        s: 0.74,
        rot: 0.5,
        palette: ORANGE_PALETTE,
    },
    Bloom {
        x: -22.0,
        y: -14.0,
        s: 0.68,
        rot: -0.3,
        palette: PINK_PALETTE,
    },
    Bloom {
        x: 24.0,
        y: -12.0,
        s: 0.7,
        rot: 0.32,
        palette: PINK_PALETTE,
    },
];

fn bloom_petals(palette: [u32; 5]) -> [BloomPetal; 5] {
    [
        BloomPetal {
            a: -0.35,
            dx: -10.0,
            dy: -6.0,
            rx: 13.0,
            ry: 17.0,
            c: palette[0],
        },
        BloomPetal {
            a: 0.4,
            dx: 11.0,
            dy: -4.0,
            rx: 12.0,
            ry: 16.0,
            c: palette[1],
        },
        BloomPetal {
            a: -0.9,
            dx: -4.0,
            dy: -15.0,
            rx: 11.0,
            ry: 15.0,
            c: palette[2],
        },
        BloomPetal {
            a: 0.95,
            dx: 5.0,
            dy: -14.0,
            rx: 11.0,
            ry: 15.0,
            c: palette[3],
        },
        BloomPetal {
            a: 0.05,
            dx: 0.0,
            dy: -7.0,
            rx: 10.0,
            ry: 13.0,
            c: palette[4],
        },
    ]
}

fn bloom_sway(t: f32, i: usize, amp: f32) -> (f32, f32, f32) {
    if amp <= 0.01 {
        return (0.0, 0.0, 0.0);
    }
    let i = i as f32;
    let slow = t * 0.0044 + i * 0.85;
    let fast = t * 0.014 + i * 1.35;
    (
        (slow.sin() * 4.8 + fast.sin() * 1.8) * amp,
        ((slow * 0.9).cos() * 1.6 + (fast * 1.1).sin() * 0.7) * amp,
        (slow.sin() * 0.11 + fast.sin() * 0.05) * amp,
    )
}

fn draw_bloom(window: &mut Window, xf: Xf, petals: &[BloomPetal; 5], center: u32, alpha: f32) {
    for petal in petals {
        fill_ellipse(
            window,
            xf,
            petal.dx,
            petal.dy,
            petal.rx,
            petal.ry,
            petal.a,
            hex(petal.c).opacity(alpha),
        );
    }
    fill_ellipse(
        window,
        xf,
        0.0,
        -5.0,
        5.5,
        5.5,
        0.0,
        hex(center).opacity(alpha),
    );
}

fn draw_bouquet(window: &mut Window, xf: Xf, scale: f32, t: f32, alpha: f32) {
    if scale <= 0.01 {
        return;
    }
    let xf = xf.scale_xy(scale * 1.5, scale * 1.5);
    let sway_amp = scale.min(1.0);
    let sways: [(f32, f32, f32); 9] = std::array::from_fn(|i| bloom_sway(t, i, sway_amp));
    let stem = hex(0x2f9e4f).opacity(alpha);
    for (i, bloom) in BLOOMS.iter().enumerate() {
        let sway = sways[i];
        let mut b = PathBuilder::stroke(px(3.5));
        b.move_to(xf.pt(bloom.x * 0.15, 62.0));
        b.curve_to(
            xf.pt(bloom.x + sway.0, bloom.y + 12.0 + sway.1),
            xf.pt(bloom.x * 0.55 + sway.0 * 0.45, 28.0),
        );
        paint_path(window, b, stem);
    }
    let leaf_wave = (t * 0.006).sin() * 0.12;
    let leaf = hex(0x3cb85c).opacity(alpha);
    fill_ellipse(
        window,
        xf,
        -18.0 + sways[1].0 * 0.3,
        36.0,
        16.0,
        7.0,
        -0.75 + leaf_wave,
        leaf,
    );
    fill_ellipse(
        window,
        xf,
        20.0 + sways[2].0 * 0.3,
        32.0,
        15.0,
        6.5,
        0.7 - leaf_wave,
        leaf,
    );
    fill_ellipse(
        window,
        xf,
        -8.0,
        20.0,
        12.0,
        5.5,
        -0.35 + leaf_wave * 0.6,
        leaf,
    );
    fill_ellipse(
        window,
        xf,
        10.0,
        24.0,
        11.0,
        5.0,
        0.4 - leaf_wave * 0.5,
        leaf,
    );
    for (i, bloom) in BLOOMS.iter().enumerate() {
        let sway = sways[i];
        let xf = xf
            .translate(bloom.x + sway.0, bloom.y + sway.1)
            .rotate(bloom.rot + sway.2)
            .scale_xy(bloom.s, bloom.s);
        draw_bloom(
            window,
            xf,
            &bloom_petals(bloom.palette),
            bloom.palette[1],
            alpha,
        );
    }
}

fn draw_body_ribbon(window: &mut Window, xf: Xf, body_left: f32, body_top: f32, alpha: f32) {
    let band_y = body_top + BOX_H * 0.42;
    let mouth_pad = 12.0;
    let gold = hex(GOLD_RIBBON).opacity(alpha);
    let dark = hex(GOLD_DARK).opacity(alpha);
    fill_round_rect(
        window,
        xf,
        -RIBBON_W / 2.0,
        body_top + mouth_pad,
        RIBBON_W,
        BOX_H - mouth_pad,
        5.0,
        gold,
    );
    fill_round_rect(window, xf, body_left, band_y, BOX_W, RIBBON_W, 5.0, gold);
    path_right_band(window, xf, body_left, band_y, BOX_W, RIBBON_W, dark);
    fill_round_rect(
        window,
        xf,
        -4.0,
        body_top + mouth_pad,
        8.0,
        BOX_H - mouth_pad,
        3.0,
        dark,
    );
}

fn draw_lid(
    window: &mut Window,
    xf: Xf,
    body_left: f32,
    body_top: f32,
    lift: f32,
    rot: f32,
    alpha: f32,
) {
    let lid_left = body_left - 4.0;
    let lid_w = BOX_W + 8.0;
    let xf = xf
        .translate(BOX_W / 2.0 + body_left, body_top)
        .rotate(rot)
        .translate(-(BOX_W / 2.0 + body_left), -body_top)
        .translate(0.0, -lift);
    fill_right_face(
        window,
        xf,
        lid_left,
        body_top - LID_LIP,
        lid_w,
        LID_LIP,
        hex(LID_DEPTH).opacity(alpha),
    );
    fill_top_face(
        window,
        xf,
        lid_left,
        body_top - LID_LIP,
        lid_w,
        hex(0xe45c5c).opacity(alpha),
    );
    fill_front_face(
        window,
        xf,
        lid_left,
        body_top - LID_LIP,
        lid_w,
        LID_LIP + 3.0,
        hex(LID_RED).opacity(alpha),
    );
    fill_round_rect(
        window,
        xf,
        -RIBBON_W / 2.0,
        body_top - LID_LIP,
        RIBBON_W,
        LID_LIP + 3.0,
        5.0,
        hex(GOLD_RIBBON).opacity(alpha),
    );
    fill_round_rect(
        window,
        xf,
        -4.0,
        body_top - LID_LIP,
        8.0,
        LID_LIP + 3.0,
        3.0,
        hex(GOLD_DARK).opacity(alpha),
    );
    draw_bow(window, xf.translate(0.0, body_top - LID_LIP + 8.0), alpha);
}

fn draw_gift_scene(window: &mut Window, scene: &GiftScene) {
    let alpha = scene.alpha.max(0.0);
    if alpha <= 0.0 {
        return;
    }
    let xf = Xf::unit()
        .translate(scene.x, scene.y)
        .scale_xy(scene.box_scale, scene.box_scale * scene.squash);
    let body_top = -BOX_H / 2.0;
    let body_left = -BOX_W / 2.0;
    fill_ellipse(
        window,
        xf,
        ISO_DX * 0.35,
        BOX_H / 2.0 + 16.0,
        BOX_W * 0.52,
        18.0,
        0.0,
        gpui::black().opacity(alpha * scene.shadow_alpha),
    );
    fill_right_face(
        window,
        xf,
        body_left,
        body_top,
        BOX_W,
        BOX_H,
        hex(0x8f1a1a).opacity(alpha),
    );
    fill_top_face(
        window,
        xf,
        body_left,
        body_top,
        BOX_W,
        hex(0x6e1414).opacity(alpha),
    );
    fill_round_rect(
        window,
        xf,
        body_left + 14.0,
        body_top - 6.0,
        BOX_W - 28.0,
        18.0,
        8.0,
        hex(0x5a1010).opacity(alpha),
    );
    draw_bouquet(
        window,
        xf.translate(ISO_DX * 0.5, body_top + 22.0 - scene.flower_scale * 58.0),
        scene.flower_scale,
        scene.t,
        alpha,
    );
    fill_front_face(
        window,
        xf,
        body_left,
        body_top,
        BOX_W,
        BOX_H,
        hex(BOX_RED).opacity(alpha),
    );
    fill_front_face(
        window,
        xf,
        body_left + 12.0,
        body_top + 16.0,
        BOX_W * 0.36,
        16.0,
        gpui::white().opacity(alpha * 0.14),
    );
    draw_body_ribbon(window, xf, body_left, body_top, alpha);
    draw_lid(
        window,
        xf,
        body_left,
        body_top,
        scene.lid_lift,
        scene.lid_rot,
        alpha,
    );
}

fn draw_star(window: &mut Window, xf: Xf, outer: f32, color: Hsla) {
    let inner = outer * 0.42;
    let mut b = PathBuilder::fill();
    for i in 0..10 {
        let r = if i % 2 == 0 { outer } else { inner };
        let a = (i as f32 * std::f32::consts::PI) / 5.0 - std::f32::consts::FRAC_PI_2;
        let p = xf.pt(a.cos() * r, a.sin() * r);
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close();
    paint_path(window, b, color);
}

fn draw_ribbon_stroke(window: &mut Window, xf: Xf, p: &Particle, color: Hsla) {
    let mut b = PathBuilder::stroke(px(p.size));
    let half = p.length / 2.0;
    let wave = p.flutter.sin() * p.amp;
    let wave2 = (p.flutter + 1.35).sin() * p.amp * 0.85;
    b.move_to(xf.pt(-half, 0.0));
    b.cubic_bezier_to(
        xf.pt(half, (p.flutter + 2.1).sin() * p.amp * 0.4),
        xf.pt(-half * 0.35, wave),
        xf.pt(half * 0.2, wave2),
    );
    paint_path(window, b, color);
}

fn update_and_draw_particles(
    window: &mut Window,
    particles: &mut Vec<Particle>,
    height: f32,
    dt_ms: f32,
) {
    let scale = (dt_ms / REF_FRAME_MS).max(0.0);
    let mut i = particles.len();
    while i > 0 {
        i -= 1;
        let p = &mut particles[i];
        p.pop = (p.pop + 0.08 * scale).min(1.0);
        p.vx *= p.drag.powf(scale);
        p.vy *= p.drag.powf(scale);
        if p.vy < 0.0 {
            p.vy += p.gravity * scale;
        } else {
            p.vy += p.fall_gravity * scale;
            p.vy *= 0.986f32.powf(scale);
        }
        p.flutter += p.flutter_speed * scale;
        let flutter_x = p.flutter.sin()
            * match p.shape {
                ParticleShape::Ribbon | ParticleShape::Streamer => 3.2,
                _ => 1.2,
            };
        p.x += (p.vx + flutter_x) * scale;
        p.y += p.vy * scale;
        p.rotation += p.rot_speed * scale;
        p.life -= p.decay * scale;
        if p.life <= 0.0 || p.y > height + 80.0 {
            particles.swap_remove(i);
            continue;
        }
        let age = 1.0 - p.life;
        let start_scale = 0.5 + 0.5 * (age / 0.12).min(1.0);
        let end_scale = if p.life < 0.28 {
            lerp(0.45, 1.0, p.life / 0.28)
        } else {
            1.0
        };
        let draw_scale = start_scale * end_scale * p.pop;
        let color = p.color.opacity(p.life.max(0.0) * p.opacity);
        let xf = Xf::unit()
            .translate(p.x, p.y)
            .rotate(p.rotation)
            .scale_xy(draw_scale, draw_scale);
        match p.shape {
            ParticleShape::Ribbon | ParticleShape::Streamer => {
                draw_ribbon_stroke(window, xf, p, color);
            }
            ParticleShape::Star => draw_star(window, xf, p.size, color),
            ParticleShape::Circle => {
                fill_ellipse(window, xf, 0.0, 0.0, p.size / 2.1, p.size / 2.1, 0.0, color);
            }
            ParticleShape::Square => {
                fill_round_rect(
                    window,
                    xf,
                    -p.size / 2.0,
                    -p.size / 2.0,
                    p.size,
                    p.size,
                    0.0,
                    color,
                );
            }
            ParticleShape::Rect => {
                fill_round_rect(
                    window,
                    xf,
                    -p.size * 0.7,
                    -p.size / 3.2,
                    p.size * 1.4,
                    p.size * 0.55,
                    0.0,
                    color,
                );
            }
        }
    }
}

fn step_runtime(runtime: &mut CelebrationRuntime, bounds: Bounds<Pixels>, window: &mut Window) {
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let now = Instant::now();
    let dt = runtime
        .last_frame
        .map(|last| (now.duration_since(last).as_secs_f32() * 1000.0).min(32.0))
        .unwrap_or(16.0);
    runtime.last_frame = Some(now);

    if let Some(scene) = runtime.scene.as_mut() {
        scene.x = f32::from(bounds.center().x);
        scene.y = f32::from(bounds.center().y);
        let (burst, wave2) = update_gift_scene(scene, dt);
        if burst {
            spawn_main_burst(
                &mut runtime.particles,
                &mut runtime.rng,
                origin_x,
                origin_y,
                scene.x,
                scene.y,
                width,
                height,
            );
        }
        if wave2 {
            spawn_wave2_burst(
                &mut runtime.particles,
                &mut runtime.rng,
                origin_x,
                origin_y,
                scene.x,
                scene.y,
                width,
                height,
            );
        }
        if scene.alpha > 0.0 {
            draw_gift_scene(window, scene);
        } else {
            runtime.scene = None;
        }
    }
    update_and_draw_particles(window, &mut runtime.particles, origin_y + height, dt);
}

fn paint_caption(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    appeared_at: Instant,
    label: &SharedString,
) {
    if label.is_empty() {
        return;
    }
    let slide = ease_out_cubic(clamp01(
        appeared_at.elapsed().as_secs_f32() * 1000.0 / 280.0,
    ));
    let font_size = px(14.);
    let line_height = px(20.);
    let pad_x = px(12.);
    let pad_y = px(8.);
    let max_pill_w = bounds.size.width * CAPTION_MAX_WIDTH_FRACTION;
    let max_text_w = (max_pill_w - pad_x * 2.).max(px(0.));
    let mut measure_run = window.text_style().to_run(label.len());
    measure_run.color = white().opacity(slide);
    measure_run.font.weight = FontWeight::SEMIBOLD;
    let (display, _) = cx
        .text_system()
        .line_wrapper(measure_run.font.clone(), font_size)
        .truncate_line(
            label.clone(),
            max_text_w,
            CAPTION_ELLIPSIS,
            std::slice::from_ref(&measure_run),
            TruncateFrom::End,
        );
    if display.is_empty() {
        return;
    }
    let mut run = window.text_style().to_run(display.len());
    run.color = white().opacity(slide);
    run.font.weight = FontWeight::SEMIBOLD;
    let line =
        window
            .text_system()
            .shape_line(display, font_size, std::slice::from_ref(&run), None);
    let pill_w = (line.width() + pad_x * 2.).min(max_pill_w);
    let pill_h = line_height + pad_y * 2.;
    let x = bounds.origin.x + (bounds.size.width - pill_w) / 2.;
    let rest_y = bounds.origin.y + bounds.size.height - px(70.) - pill_h;
    let y = rest_y + px(24.) * (1.0 - slide);
    window.paint_quad(
        fill(
            Bounds {
                origin: point(x, y),
                size: size(pill_w, pill_h),
            },
            black().opacity(slide),
        )
        .corner_radii(Corners::all(pill_h / 2.)),
    );
    let _ = line.paint(
        point(x + pad_x, y + pad_y),
        line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

pub struct FlowerCelebrationElement {
    id: ElementId,
    started_at: Instant,
    caption: Option<SharedString>,
}

impl FlowerCelebrationElement {
    pub fn new(key: &str, started_at: Instant, caption: Option<SharedString>) -> Self {
        Self {
            id: ElementId::Name(SharedString::from(format!("flower-celeb-{key}"))),
            started_at,
            caption,
        }
    }
}

impl IntoElement for FlowerCelebrationElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FlowerCelebrationElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        window.request_animation_frame();
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(global_id) = global_id else {
            return;
        };
        let started_at = self.started_at;
        let caption = self.caption.clone();
        window.with_element_state::<CelebrationRuntime, _>(global_id, |state, window| {
            let mut runtime = state.unwrap_or_default();
            if runtime.started_at != Some(started_at) {
                runtime.started_at = Some(started_at);
                runtime.last_frame = None;
                runtime.particles.clear();
                runtime.rng = started_at.elapsed().as_nanos() as u64 | 1;
                runtime.scene = Some(create_gift_scene(
                    f32::from(bounds.center().x),
                    f32::from(bounds.center().y),
                ));
                runtime.caption_key = None;
                runtime.caption_appeared_at = None;
            }
            step_runtime(&mut runtime, bounds, window);
            if let Some(label) = caption.as_ref() {
                if runtime.caption_key.as_ref() != Some(label) {
                    runtime.caption_key = Some(label.clone());
                    runtime.caption_appeared_at = Some(Instant::now());
                }
                if let Some(appeared_at) = runtime.caption_appeared_at {
                    paint_caption(window, cx, bounds, appeared_at, label);
                }
            } else {
                runtime.caption_key = None;
                runtime.caption_appeared_at = None;
            }
            ((), runtime)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BURST_MS, FADE_END_MS, WAVE2_MS, create_gift_scene, spawn_all_cannons, update_gift_scene,
    };

    #[test]
    fn gift_scene_fires_burst_then_wave2_then_fades() {
        let mut scene = create_gift_scene(100.0, 80.0);
        let (burst, wave2) = update_gift_scene(&mut scene, BURST_MS - 1.0);
        assert!(!burst);
        assert!(!wave2);
        assert!(scene.alpha > 0.99);
        let (burst, wave2) = update_gift_scene(&mut scene, 2.0);
        assert!(burst);
        assert!(!wave2);
        let to_wave2 = WAVE2_MS - scene.t + 1.0;
        let (burst, wave2) = update_gift_scene(&mut scene, to_wave2);
        assert!(!burst);
        assert!(wave2);
        let to_fade = FADE_END_MS - scene.t;
        update_gift_scene(&mut scene, to_fade);
        assert!(scene.alpha <= 0.001);
        assert!(scene.burst_fired);
        assert!(scene.wave2_fired);
    }

    #[test]
    fn cannons_spawn_in_overlay_space() {
        let mut particles = Vec::new();
        let mut rng = 1u64;
        let origin_x = 320.0;
        let origin_y = 90.0;
        let width = 800.0;
        let height = 500.0;
        let x = origin_x + width / 2.0;
        let y = origin_y + height / 2.0;
        spawn_all_cannons(
            &mut particles,
            &mut rng,
            origin_x,
            origin_y,
            x,
            y,
            width,
            height,
            1.0,
        );
        let left = origin_x + width * 0.12;
        let right = origin_x + width * 0.88;
        let mid_x = origin_x + width * 0.5;
        let floor_y = origin_y + height * 0.78;
        assert!(particles.iter().any(|p| (p.x - left).abs() < 0.01));
        assert!(particles.iter().any(|p| (p.x - right).abs() < 0.01));
        assert!(particles.iter().any(|p| (p.x - mid_x).abs() < 0.01));
        assert!(particles.iter().any(|p| (p.y - floor_y).abs() < 0.01));
        assert!(particles.iter().any(|p| (p.x - x).abs() < 0.01));
        assert!(
            particles
                .iter()
                .all(|p| p.x >= origin_x && p.x <= origin_x + width)
        );
        assert!(
            particles
                .iter()
                .all(|p| p.y >= origin_y && p.y <= origin_y + height)
        );
    }
}
