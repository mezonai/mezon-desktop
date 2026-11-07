use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, Global, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseMoveEvent, Pixels,
    Transformation, Window, point, px, radians, size,
};

#[derive(Clone, Copy, Default)]
pub struct IconMotionPreferences {
    pub reduced_motion: bool,
}

impl Global for IconMotionPreferences {}

impl IconMotionPreferences {
    pub(crate) fn enabled(cx: &App) -> bool {
        mezon_store::Settings::try_global(cx).is_none_or(|s| s.read(cx).icon_motion)
            && !cx.try_global::<Self>().is_some_and(|p| p.reduced_motion)
    }
}

#[derive(Default)]
struct MotionScopeState(Option<bool>);

impl Global for MotionScopeState {}

pub(crate) fn scoped_hover(cx: &App) -> Option<bool> {
    cx.try_global::<MotionScopeState>().and_then(|s| s.0)
}

pub struct IconMotionScope {
    element: AnyElement,
    enabled: bool,
}

impl IconMotionScope {
    pub fn new(element: impl IntoElement, enabled: bool) -> Self {
        Self {
            element: element.into_any_element(),
            enabled,
        }
    }
}

impl IntoElement for IconMotionScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for IconMotionScope {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.element.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.element.prepaint(window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hovered = self.enabled && scoped_hover(cx) != Some(false) && hitbox.is_hovered(window);
        let previous = cx.default_global::<MotionScopeState>().0.replace(hovered);
        self.element.paint(window, cx);
        cx.global_mut::<MotionScopeState>().0 = previous;
        if self.enabled {
            let hitbox = hitbox.clone();
            let view = window.current_view();
            let mut was_hovered = hovered;
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                let hovered = hitbox.is_hovered(window);
                if phase == DispatchPhase::Capture && hovered != was_hovered {
                    was_hovered = hovered;
                    cx.notify(view);
                }
            });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MotionKind {
    None,
    Hop,
    Typing,
    Live,
    Ring,
    Swing,
    Spin,
    Zoom,
}

#[derive(Clone, Copy)]
pub(crate) struct MotionSpec {
    anim: MotionKind,
    duration_ms: u64,
    turn_deg: Option<f32>,
}

impl MotionSpec {
    pub(crate) fn for_mz(icon: crate::MzIcon) -> Option<Self> {
        use crate::MzAnim;
        let (anim, duration_ms, turn_deg) = match icon.anim {
            MzAnim::None => return None,
            MzAnim::Hop => (MotionKind::Hop, 350, None),
            MzAnim::Ring => (MotionKind::Ring, 450, None),
            MzAnim::Swing => (MotionKind::Swing, 500, None),
            MzAnim::Spin { turn_deg } => (MotionKind::Spin, 400, Some(f32::from(turn_deg))),
            MzAnim::Zoom => (MotionKind::Zoom, 300, None),
            MzAnim::Typing => (MotionKind::Typing, 900, None),
            MzAnim::Live => (MotionKind::Live, 1600, None),
        };
        Some(Self {
            anim,
            duration_ms,
            turn_deg,
        })
    }

    pub(crate) fn translates_only(&self) -> bool {
        matches!(
            self.anim,
            MotionKind::Hop | MotionKind::Typing | MotionKind::Live
        )
    }

    pub(crate) fn has_hover_motion(&self) -> bool {
        !matches!(self.anim, MotionKind::None)
    }

    pub(crate) fn for_path(path: &str) -> Option<&'static Self> {
        static SPECS: &[(&str, MotionSpec)] = include!("../assets/icon-motion.rs");
        SPECS
            .binary_search_by_key(&path, |(path, _)| *path)
            .ok()
            .map(|index| &SPECS[index].1)
    }

    fn hover_duration_ms(&self) -> u64 {
        match self.anim {
            MotionKind::Typing | MotionKind::Live => HOP_DURATION_MS,
            _ => self.duration_ms,
        }
    }
}

const HOP_DURATION_MS: u64 = 350;
const HOP_HEIGHT: f32 = 0.0833;
const HOP_KEYFRAMES: &[(f32, f32)] = &[(0., 0.), (0.35, 1.), (1., 0.)];
const RING_KEYFRAMES: &[(f32, f32)] = &[
    (0., 0.),
    (0.2, -12.),
    (0.4, 10.),
    (0.6, -8.),
    (0.8, 5.),
    (1., 0.),
];
const SWING_KEYFRAMES: &[(f32, f32)] = &[(0., 0.), (0.2, 14.), (0.45, -10.), (0.7, 5.), (1., 0.)];
const ZOOM_KEYFRAMES: &[(f32, f32)] = &[(0., 1.), (0.5, 1.12), (1., 1.)];
const SWING_ORIGIN_Y: f32 = 0.12;
const ZOOM_ORIGIN: f32 = 0.46;

#[derive(Default)]
pub(crate) struct MotionPlayback {
    path: Option<&'static str>,
    hovered: bool,
    started: Option<Instant>,
}

impl MotionPlayback {
    pub(crate) fn frame(
        &mut self,
        path: &'static str,
        spec: Option<&MotionSpec>,
        hovered: bool,
        now: Instant,
    ) -> Option<MotionFrame> {
        if self.path != Some(path) {
            *self = Self {
                path: Some(path),
                ..Self::default()
            };
        }
        if hovered && !self.hovered {
            self.started = Some(now);
        } else if !hovered {
            self.started = None;
        }
        self.hovered = hovered;
        let spec = spec?;
        if !spec.has_hover_motion() {
            self.started = None;
            return None;
        }
        let started = self.started?;
        let duration = spec.hover_duration_ms();
        if duration == 0 {
            self.started = None;
            return None;
        }
        let progress =
            (now.duration_since(started).as_secs_f32() * 1000. / duration as f32).min(1.);
        Some(MotionFrame {
            kind: spec.anim,
            progress,
            turn_deg: spec.turn_deg.unwrap_or_default(),
            running: progress < 1.,
        })
    }
}

pub(crate) struct MotionFrame {
    kind: MotionKind,
    progress: f32,
    turn_deg: f32,
    pub(crate) running: bool,
}

impl MotionFrame {
    fn hop_offset(&self, height: Pixels) -> gpui::Point<Pixels> {
        point(
            px(0.),
            if self.running {
                -height * HOP_HEIGHT * keyframe(HOP_KEYFRAMES, self.progress)
            } else {
                px(0.)
            },
        )
    }

    pub(crate) fn translated_bounds(&self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        bounds.origin += self.hop_offset(bounds.size.height);
        bounds
    }

    pub(crate) fn transformation(&self, bounds: Bounds<Pixels>) -> Transformation {
        if !self.running {
            return Transformation::default();
        }
        match self.kind {
            MotionKind::Hop | MotionKind::Typing | MotionKind::Live => {
                Transformation::translate(self.hop_offset(bounds.size.height))
            }
            MotionKind::Ring => Transformation::rotate(radians(
                keyframe(RING_KEYFRAMES, self.progress).to_radians(),
            )),
            MotionKind::Swing => {
                let angle = keyframe(SWING_KEYFRAMES, self.progress).to_radians();
                let pivot_offset = bounds.size.height * (SWING_ORIGIN_Y - 0.5);
                Transformation::rotate(radians(angle)).with_translation(point(
                    pivot_offset * angle.sin(),
                    pivot_offset * (1. - angle.cos()),
                ))
            }
            MotionKind::Spin => Transformation::rotate(radians(
                self.turn_deg.to_radians() * cubic_bezier(self.progress, 0.2, 0., 0., 1.),
            )),
            MotionKind::Zoom => {
                let scale = keyframe(ZOOM_KEYFRAMES, self.progress);
                Transformation::scale(size(scale, scale)).with_translation(point(
                    bounds.size.width * (ZOOM_ORIGIN - 0.5) * (1. - scale),
                    bounds.size.height * (ZOOM_ORIGIN - 0.5) * (1. - scale),
                ))
            }
            MotionKind::None => Transformation::default(),
        }
    }
}

fn keyframe(frames: &[(f32, f32)], progress: f32) -> f32 {
    for pair in frames.windows(2) {
        let [(start, from), (end, to)] = pair else {
            continue;
        };
        if progress <= *end {
            let t = ((progress - start) / (end - start)).clamp(0., 1.);
            return from + (to - from) * cubic_bezier(t, 0.42, 0., 0.58, 1.);
        }
    }
    frames.last().map_or(0., |(_, value)| *value)
}

fn cubic_bezier(progress: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let sample = |t: f32, a: f32, b: f32| {
        3. * (1. - t).powi(2) * t * a + 3. * (1. - t) * t.powi(2) * b + t.powi(3)
    };
    let mut low = 0.;
    let mut high = 1.;
    const BEZIER_ITERATIONS: usize = 16;
    for _ in 0..BEZIER_ITERATIONS {
        let t = (low + high) * 0.5;
        if sample(t, x1, x2) < progress {
            low = t;
        } else {
            high = t;
        }
    }
    sample((low + high) * 0.5, y1, y2)
}

#[derive(Default)]
pub(crate) struct ColorPlayback {
    from: Option<gpui::Rgba>,
    target: Option<gpui::Rgba>,
    started: Option<Instant>,
}

impl ColorPlayback {
    pub(crate) fn frame(&mut self, target: gpui::Hsla, now: Instant) -> (gpui::Hsla, bool) {
        let target = gpui::Rgba::from(target);
        if self.target != Some(target) {
            self.from = Some(self.sample(now));
            if self.target.is_none() {
                self.from = Some(target);
            }
            self.target = Some(target);
            self.started = Some(now);
        }
        let running = self
            .started
            .is_some_and(|start| now.duration_since(start).as_millis() < 200)
            && self.from != self.target;
        (self.sample(now).into(), running)
    }

    fn sample(&self, now: Instant) -> gpui::Rgba {
        let to = self.target.unwrap_or_else(|| gpui::rgb(0));
        let from = self.from.unwrap_or(to);
        let t = self.started.map_or(1., |start| {
            cubic_bezier(
                (now.duration_since(start).as_secs_f32() / 0.2).min(1.),
                0.2,
                0.,
                0.,
                1.,
            )
        });
        gpui::Rgba {
            r: from.r + (to.r - from.r) * t,
            g: from.g + (to.g - from.g) * t,
            b: from.b + (to.b - from.b) * t,
            a: from.a + (to.a - from.a) * t,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MzTransform {
    pub translate_y_pct: f32,
    pub rotate_deg: f32,
    pub scale: f32,
    pub opacity: f32,
}

pub fn sample_keyframe(anim: crate::MzAnim, t: f32) -> MzTransform {
    use crate::MzAnim;
    let t = t.clamp(0., 1.);
    let mut transform = MzTransform {
        translate_y_pct: 0.,
        rotate_deg: 0.,
        scale: 1.,
        opacity: 1.,
    };
    match anim {
        MzAnim::Hop | MzAnim::Typing | MzAnim::Live => {
            transform.translate_y_pct = -8.33 * keyframe(HOP_KEYFRAMES, t);
        }
        MzAnim::Ring => transform.rotate_deg = keyframe(RING_KEYFRAMES, t),
        MzAnim::Swing => transform.rotate_deg = keyframe(SWING_KEYFRAMES, t),
        MzAnim::Spin { turn_deg } => {
            transform.rotate_deg = f32::from(turn_deg) * cubic_bezier(t, 0.2, 0., 0., 1.)
        }
        MzAnim::Zoom => transform.scale = keyframe(ZOOM_KEYFRAMES, t),
        MzAnim::None => {}
    }
    transform
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn color_transition_finishes_and_can_reverse_mid_flight() {
        let now = Instant::now();
        let black = gpui::rgb(0).into();
        let white = gpui::rgb(0xffffff).into();
        let mut playback = ColorPlayback::default();
        assert!(!playback.frame(black, now).1);
        assert!(playback.frame(white, now).1);
        let middle = playback.frame(white, now + Duration::from_millis(80));
        assert!(middle.1);
        assert!(gpui::Rgba::from(middle.0).r > 0.);
        let reversed = playback.frame(black, now + Duration::from_millis(80));
        assert!((gpui::Rgba::from(middle.0).r - gpui::Rgba::from(reversed.0).r).abs() < 0.001);
        let finished = playback.frame(black, now + Duration::from_millis(300));
        assert!(!finished.1);
        assert!(gpui::Rgba::from(finished.0).r < 0.001);
    }

    #[test]
    fn keyframes_end_at_identity_and_spin_uses_package_turn() {
        use crate::MzAnim;
        for anim in [
            MzAnim::None,
            MzAnim::Hop,
            MzAnim::Ring,
            MzAnim::Swing,
            MzAnim::Zoom,
            MzAnim::Typing,
            MzAnim::Live,
        ] {
            let frame = sample_keyframe(anim, 1.);
            assert!(frame.translate_y_pct.abs() < 0.001);
            assert!(frame.rotate_deg.abs() < 0.001);
            assert!((frame.scale - 1.).abs() < 0.001);
            assert_eq!(frame.opacity, 1.);
        }
        assert!((sample_keyframe(MzAnim::Hop, 0.35).translate_y_pct + 8.33).abs() < 0.001);
        assert!((sample_keyframe(crate::mz::SETTINGS_20.anim, 1.).rotate_deg - 45.).abs() < 0.001);
    }

    #[test]
    fn hover_hops_once_and_restarts_only_after_leaving() {
        let now = Instant::now();
        let spec = MotionSpec::for_path("icons/plus.svg");
        let mut playback = MotionPlayback::default();
        assert!(playback.frame("icons/plus.svg", spec, false, now).is_none());
        playback.frame("icons/plus.svg", spec, true, now);
        let peak = playback
            .frame(
                "icons/plus.svg",
                spec,
                true,
                now + Duration::from_micros(122500),
            )
            .unwrap();
        assert!((keyframe(HOP_KEYFRAMES, peak.progress) - 1.).abs() < 0.001);
        let done = playback
            .frame("icons/plus.svg", spec, true, now + Duration::from_secs(1))
            .unwrap();
        assert!(!done.running);
        assert_eq!(done.progress, 1.);
        assert!(
            playback
                .frame("icons/plus.svg", spec, false, now + Duration::from_secs(2))
                .is_none()
        );
        let restart = playback
            .frame("icons/plus.svg", spec, true, now + Duration::from_secs(3))
            .unwrap();
        assert_eq!(restart.progress, 0.);
    }

    #[test]
    fn changing_icon_does_not_inherit_animation_progress() {
        let now = Instant::now();
        let mut playback = MotionPlayback::default();
        playback.frame(
            "icons/plus.svg",
            MotionSpec::for_path("icons/plus.svg"),
            true,
            now,
        );
        let switched = playback
            .frame(
                "icons/search.svg",
                MotionSpec::for_path("icons/search.svg"),
                true,
                now + Duration::from_millis(100),
            )
            .unwrap();
        assert_eq!(switched.kind, MotionKind::Zoom);
        assert_eq!(switched.progress, 0.);
        assert!(
            playback
                .frame(
                    "icons/online-status.svg",
                    MotionSpec::for_path("icons/online-status.svg"),
                    true,
                    now
                )
                .is_none()
        );
        assert!(playback.frame("icons/apple.svg", None, true, now).is_none());
    }

    #[test]
    fn catalog_preserves_special_motion_and_static_statuses() {
        for (path, kind, duration) in [
            ("icons/bell.svg", MotionKind::Swing, 500),
            ("icons/incoming-call.svg", MotionKind::Ring, 450),
            ("icons/search.svg", MotionKind::Zoom, 300),
            ("icons/settings.svg", MotionKind::Spin, 400),
            ("icons/chat.svg", MotionKind::Typing, 350),
            ("icons/online-status.svg", MotionKind::None, 0),
        ] {
            let spec = MotionSpec::for_path(path).unwrap();
            assert_eq!(spec.anim, kind, "{path}");
            assert_eq!(spec.hover_duration_ms(), duration, "{path}");
        }
    }

    #[test]
    fn exit_cancels_in_flight_animation_and_transforms_scale_with_layout() {
        let now = Instant::now();
        let mut playback = MotionPlayback::default();
        let spec = MotionSpec::for_path("icons/plus.svg");
        playback.frame("icons/plus.svg", spec, true, now);
        assert!(
            playback
                .frame(
                    "icons/plus.svg",
                    spec,
                    false,
                    now + Duration::from_millis(30)
                )
                .is_none()
        );
        for diameter in [12., 16., 20., 24., 48.] {
            let frame = MotionFrame {
                kind: MotionKind::Hop,
                progress: 0.35,
                turn_deg: 0.,
                running: true,
            };
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(diameter), px(diameter)));
            assert_eq!(
                frame.transformation(bounds),
                Transformation::translate(point(px(0.), -bounds.size.height * HOP_HEIGHT))
            );
        }
    }

    #[test]
    fn finished_spin_returns_to_rest_instead_of_leaving_icon_rotated() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(20.), px(20.)));
        let frame = MotionFrame {
            kind: MotionKind::Spin,
            progress: 1.,
            turn_deg: 45.,
            running: false,
        };
        assert_eq!(frame.transformation(bounds), Transformation::default());
    }

    #[test]
    fn hop_translates_paint_bounds_without_changing_layout_size() {
        let bounds = Bounds::new(point(px(12.), px(30.)), size(px(20.), px(20.)));
        let mut frame = MotionFrame {
            kind: MotionKind::Hop,
            progress: 0.35,
            turn_deg: 0.,
            running: true,
        };
        let painted = frame.translated_bounds(bounds);
        assert_eq!(painted.size, bounds.size);
        assert_eq!(painted.origin.x, bounds.origin.x);
        assert!(painted.origin.y < bounds.origin.y);
        frame.running = false;
        assert_eq!(frame.translated_bounds(bounds), bounds);
    }

    #[test]
    fn static_catalog_is_sorted_and_every_entry_is_resolvable() {
        let catalog: &[(&str, MotionSpec)] = include!("../assets/icon-motion.rs");
        assert!(!catalog.is_empty());
        assert!(catalog.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for (path, expected) in catalog {
            let actual = MotionSpec::for_path(path).expect("catalog entry must resolve");
            assert_eq!(actual.anim, expected.anim, "{path}");
            assert_eq!(actual.duration_ms, expected.duration_ms, "{path}");
            assert_eq!(actual.turn_deg, expected.turn_deg, "{path}");
        }
    }

    #[test]
    fn parent_hover_reaches_children_and_disabled_scope_blocks_motion() {
        use gpui::{Context, Render, TestApp, canvas, div, prelude::*};
        use std::{cell::RefCell, rc::Rc};

        struct Fixture {
            enabled: bool,
            clicks: usize,
            observed: Rc<RefCell<Vec<Option<bool>>>>,
        }

        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let observed = self.observed.clone();
                IconMotionScope::new(
                    div()
                        .id("motion-scope-row")
                        .size(px(80.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.clicks += 1;
                            cx.notify();
                        }))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |_, _, _, cx| observed.borrow_mut().push(scoped_hover(cx)),
                            )
                            .size(px(16.)),
                        ),
                    self.enabled,
                )
            }
        }

        let mut app = TestApp::new();
        let observed = Rc::new(RefCell::new(Vec::new()));
        let mut window = app.open_window(|_, _| Fixture {
            enabled: true,
            clicks: 0,
            observed: observed.clone(),
        });
        window.draw();
        window.simulate_mouse_move(point(px(60.), px(60.)));
        window.draw();
        assert_eq!(observed.borrow().last(), Some(&Some(true)));
        window.simulate_click(point(px(60.), px(60.)), gpui::MouseButton::Left);
        assert_eq!(window.read(|view, _| view.clicks), 1);
        window.update(|view, _, cx| {
            view.enabled = false;
            cx.notify();
        });
        window.draw();
        assert_eq!(observed.borrow().last(), Some(&Some(false)));
        window.update(|view, _, cx| {
            view.enabled = true;
            cx.notify();
        });
        window.simulate_mouse_move(point(px(100.), px(100.)));
        window.draw();
        assert_eq!(observed.borrow().last(), Some(&Some(false)));
        assert!(app.read(scoped_hover).is_none());
    }
}
