use gpui::{Bounds, Pixels, Point, Size, bounds, point, px, size};

pub const HOLE_PADDING: Pixels = px(2.);
pub const MIN_HOLE_SIZE: Pixels = px(24.);
pub const BUBBLE_WIDTH: Pixels = px(320.);
pub const BUBBLE_HEIGHT: Pixels = px(248.);
pub const BUBBLE_GAP: Pixels = px(12.);
pub const VIEWPORT_MARGIN: Pixels = px(16.);
pub const RING_RADIUS: Pixels = px(6.);
pub const GLOW_INSET: Pixels = px(2.);
pub const SCRIM_ALPHA: f32 = 0.62;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Below,
    Above,
    Right,
    Left,
    Center,
}

fn clamp(value: Pixels, low: Pixels, high: Pixels) -> Pixels {
    if high < low {
        return low;
    }
    value.max(low).min(high)
}

fn fit_span(low: Pixels, high: Pixels, limit: Pixels) -> (Pixels, Pixels) {
    let mut low = clamp(low, Pixels::ZERO, limit);
    let mut high = clamp(high, low, limit);
    let want = MIN_HOLE_SIZE.min(limit);
    if high - low >= want {
        return (low, high);
    }
    let room_below = low;
    let take_below = ((want - (high - low)) / 2.).min(room_below);
    low -= take_below;
    high = (low + want).min(limit);
    if high - low < want {
        low = (high - want).max(Pixels::ZERO);
    }
    (low, high)
}

pub fn hole_for(target: Bounds<Pixels>, viewport: Size<Pixels>) -> Bounds<Pixels> {
    let (left, right) = fit_span(
        target.left() - HOLE_PADDING,
        target.right() + HOLE_PADDING,
        viewport.width,
    );
    let (top, bottom) = fit_span(
        target.top() - HOLE_PADDING,
        target.bottom() + HOLE_PADDING,
        viewport.height,
    );
    bounds(point(left, top), size(right - left, bottom - top))
}

pub fn bands(hole: Bounds<Pixels>, viewport: Size<Pixels>) -> [Bounds<Pixels>; 4] {
    let left = clamp(hole.left(), Pixels::ZERO, viewport.width);
    let top = clamp(hole.top(), Pixels::ZERO, viewport.height);
    let right = clamp(hole.right(), left, viewport.width);
    let bottom = clamp(hole.bottom(), top, viewport.height);

    [
        bounds(point(Pixels::ZERO, Pixels::ZERO), size(viewport.width, top)),
        bounds(
            point(Pixels::ZERO, bottom),
            size(viewport.width, viewport.height - bottom),
        ),
        bounds(point(Pixels::ZERO, top), size(left, bottom - top)),
        bounds(
            point(right, top),
            size(viewport.width - right, bottom - top),
        ),
    ]
}

pub fn place(
    hole: Bounds<Pixels>,
    bubble: Size<Pixels>,
    viewport: Size<Pixels>,
) -> (Point<Pixels>, Side) {
    let fits_below =
        hole.bottom() + BUBBLE_GAP + bubble.height + VIEWPORT_MARGIN <= viewport.height;
    let fits_above = hole.top() - BUBBLE_GAP - bubble.height - VIEWPORT_MARGIN >= Pixels::ZERO;
    let fits_right = hole.right() + BUBBLE_GAP + bubble.width + VIEWPORT_MARGIN <= viewport.width;
    let fits_left = hole.left() - BUBBLE_GAP - bubble.width - VIEWPORT_MARGIN >= Pixels::ZERO;

    let side = if fits_below {
        Side::Below
    } else if fits_above {
        Side::Above
    } else if fits_right {
        Side::Right
    } else if fits_left {
        Side::Left
    } else {
        Side::Center
    };

    let origin = match side {
        Side::Below => point(
            hole.center().x - bubble.width / 2.,
            hole.bottom() + BUBBLE_GAP,
        ),
        Side::Above => point(
            hole.center().x - bubble.width / 2.,
            hole.top() - BUBBLE_GAP - bubble.height,
        ),
        Side::Right => point(
            hole.right() + BUBBLE_GAP,
            hole.center().y - bubble.height / 2.,
        ),
        Side::Left => point(
            hole.left() - BUBBLE_GAP - bubble.width,
            hole.center().y - bubble.height / 2.,
        ),
        Side::Center => point(
            (viewport.width - bubble.width) / 2.,
            (viewport.height - bubble.height) / 2.,
        ),
    };

    (clamp_to_viewport(origin, bubble, viewport), side)
}

pub fn center_origin(bubble: Size<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
    clamp_to_viewport(
        point(
            (viewport.width - bubble.width) / 2.,
            (viewport.height - bubble.height) / 2.,
        ),
        bubble,
        viewport,
    )
}

fn clamp_to_viewport(
    origin: Point<Pixels>,
    bubble: Size<Pixels>,
    viewport: Size<Pixels>,
) -> Point<Pixels> {
    point(
        clamp(
            origin.x,
            VIEWPORT_MARGIN,
            viewport.width - bubble.width - VIEWPORT_MARGIN,
        ),
        clamp(
            origin.y,
            VIEWPORT_MARGIN,
            viewport.height - bubble.height - VIEWPORT_MARGIN,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Size<Pixels> {
        size(px(1200.), px(800.))
    }

    #[test]
    fn bands_cover_the_viewport_except_the_hole() {
        let view = viewport();
        let hole = bounds(point(px(300.), px(200.)), size(px(180.), px(60.)));
        let total = view.width.as_f32() * view.height.as_f32();
        let hole_area = hole.size.width.as_f32() * hole.size.height.as_f32();
        let covered: f32 = bands(hole, view)
            .iter()
            .map(|band| band.size.width.as_f32() * band.size.height.as_f32())
            .sum();
        assert!((covered - (total - hole_area)).abs() < 0.5);
    }

    #[test]
    fn bands_do_not_overlap_each_other() {
        let view = viewport();
        let hole = bounds(point(px(300.), px(200.)), size(px(180.), px(60.)));
        let all = bands(hole, view);
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                let overlap = a.intersect(b);
                assert!(overlap.size.width <= Pixels::ZERO || overlap.size.height <= Pixels::ZERO);
            }
        }
    }

    #[test]
    fn bands_survive_a_hole_touching_every_edge() {
        let view = viewport();
        let hole = bounds(point(Pixels::ZERO, Pixels::ZERO), view);
        let all = bands(hole, view);
        let covered: f32 = all
            .iter()
            .map(|band| band.size.width.as_f32().max(0.) * band.size.height.as_f32().max(0.))
            .sum();
        assert!(covered.abs() < 0.5);
    }

    #[test]
    fn bands_clamp_a_hole_larger_than_the_viewport() {
        let view = viewport();
        let hole = bounds(point(px(-200.), px(-200.)), size(px(4000.), px(4000.)));
        for band in bands(hole, view) {
            assert!(band.size.width >= Pixels::ZERO);
            assert!(band.size.height >= Pixels::ZERO);
        }
    }

    #[test]
    fn hole_grows_a_tiny_target_to_the_minimum() {
        let view = viewport();
        let target = bounds(point(px(100.), px(100.)), size(px(8.), px(8.)));
        let hole = hole_for(target, view);
        assert!(hole.size.width >= MIN_HOLE_SIZE);
        assert!(hole.size.height >= MIN_HOLE_SIZE);
    }

    #[test]
    fn the_minimum_still_holds_for_a_target_pinned_to_every_edge() {
        let view = viewport();
        for target in [
            bounds(point(px(0.), px(100.)), size(px(8.), px(8.))),
            bounds(point(px(100.), px(0.)), size(px(8.), px(8.))),
            bounds(point(view.width - px(8.), px(100.)), size(px(8.), px(8.))),
            bounds(point(px(100.), view.height - px(8.)), size(px(8.), px(8.))),
        ] {
            let hole = hole_for(target, view);
            assert!(
                hole.size.width >= MIN_HOLE_SIZE,
                "width {:?} for {target:?}",
                hole.size.width
            );
            assert!(
                hole.size.height >= MIN_HOLE_SIZE,
                "height {:?} for {target:?}",
                hole.size.height
            );
            assert!(hole.left() >= Pixels::ZERO && hole.right() <= view.width);
            assert!(hole.top() >= Pixels::ZERO && hole.bottom() <= view.height);
        }
    }

    #[test]
    fn a_hole_never_exceeds_a_viewport_smaller_than_the_minimum() {
        let tiny = size(px(16.), px(16.));
        let hole = hole_for(bounds(point(px(4.), px(4.)), size(px(2.), px(2.))), tiny);
        assert!(hole.right() <= tiny.width && hole.bottom() <= tiny.height);
    }

    #[test]
    fn hole_pads_a_normal_target_and_stays_inside_the_viewport() {
        let view = viewport();
        let target = bounds(point(px(0.), px(0.)), size(px(200.), px(40.)));
        let hole = hole_for(target, view);
        assert_eq!(hole.left(), Pixels::ZERO);
        assert_eq!(hole.top(), Pixels::ZERO);
        assert_eq!(hole.right(), px(202.));
    }

    #[test]
    fn placement_prefers_below() {
        let view = viewport();
        let hole = bounds(point(px(300.), px(100.)), size(px(180.), px(60.)));
        let (origin, side) = place(hole, size(BUBBLE_WIDTH, px(160.)), view);
        assert_eq!(side, Side::Below);
        assert_eq!(origin.y, hole.bottom() + BUBBLE_GAP);
    }

    #[test]
    fn placement_falls_back_to_above_then_right_then_left() {
        let view = viewport();
        let tall = size(BUBBLE_WIDTH, px(500.));

        let low = bounds(point(px(300.), px(600.)), size(px(180.), px(60.)));
        assert_eq!(place(low, tall, view).1, Side::Above);

        let mid = bounds(point(px(300.), px(300.)), size(px(180.), px(200.)));
        assert_eq!(place(mid, tall, view).1, Side::Right);

        let mid_right = bounds(point(px(900.), px(300.)), size(px(180.), px(200.)));
        assert_eq!(place(mid_right, tall, view).1, Side::Left);
    }

    #[test]
    fn placement_falls_back_to_center_when_nothing_fits() {
        let view = size(px(400.), px(300.));
        let hole = bounds(point(px(20.), px(20.)), size(px(360.), px(260.)));
        assert_eq!(
            place(hole, size(BUBBLE_WIDTH, px(260.)), view).1,
            Side::Center
        );
    }

    #[test]
    fn placement_clamps_a_bubble_that_would_leave_the_viewport() {
        let view = viewport();
        let hole = bounds(point(px(0.), px(100.)), size(px(40.), px(40.)));
        let (origin, _) = place(hole, size(BUBBLE_WIDTH, px(160.)), view);
        assert!(origin.x >= VIEWPORT_MARGIN);
        assert!(origin.x + BUBBLE_WIDTH <= view.width - VIEWPORT_MARGIN);
    }
}
