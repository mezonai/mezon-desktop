//! Shared state for multi-event input gestures.

use std::time::{Duration, Instant};

use crate::{Axis, IsZero, Pixels, Point, TouchPhase, px};

const SCROLL_EVENT_SEPARATION: Duration = Duration::from_millis(28);

/// Tracks the dominant axis across the events in a scroll gesture.
#[derive(Clone, Copy, Debug, Default)]
pub struct OngoingScroll {
    last_event: Option<Instant>,
    axis: Option<Axis>,
}

impl OngoingScroll {
    /// Filters the given delta to the dominant axis of the current scroll gesture.
    ///
    /// Touch phases delimit gestures when available, with a timeout fallback for
    /// platforms that only emit moved events.
    pub fn filter(&mut self, delta: &mut Point<Pixels>, touch_phase: TouchPhase) {
        self.filter_at(delta, touch_phase, Instant::now());
    }

    fn filter_at(&mut self, delta: &mut Point<Pixels>, touch_phase: TouchPhase, now: Instant) {
        const UNLOCK_PERCENT: f32 = 1.9;
        const UNLOCK_LOWER_BOUND: Pixels = px(6.);

        if touch_phase == TouchPhase::Ended {
            self.last_event = None;
            self.axis = None;
            return;
        }

        let x = delta.x.abs();
        let y = delta.y.abs();
        if x.is_zero() && y.is_zero() {
            if touch_phase == TouchPhase::Started {
                self.last_event = None;
                self.axis = None;
            }
            return;
        }

        let starts_new_gesture = touch_phase == TouchPhase::Started
            || self
                .last_event
                .is_none_or(|last_event| now.duration_since(last_event) >= SCROLL_EVENT_SEPARATION);
        let mut axis = self.axis;
        if starts_new_gesture {
            axis = if x <= y {
                Some(Axis::Vertical)
            } else {
                Some(Axis::Horizontal)
            };
        } else if x.max(y) >= UNLOCK_LOWER_BOUND {
            match axis {
                Some(Axis::Vertical) if x > y && x >= y * UNLOCK_PERCENT => axis = None,
                Some(Axis::Horizontal) if y > x && y >= x * UNLOCK_PERCENT => axis = None,
                _ => {}
            }
        }

        self.last_event = Some(now);
        self.axis = axis;
        match axis {
            Some(Axis::Vertical) => delta.x = Pixels::ZERO,
            Some(Axis::Horizontal) => delta.y = Pixels::ZERO,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point;

    #[test]
    fn locks_to_the_dominant_axis_for_a_gesture() {
        let now = Instant::now();
        let mut scroll = OngoingScroll::default();
        let mut first = point(px(10.), px(2.));
        scroll.filter_at(&mut first, TouchPhase::Started, now);
        assert_eq!(first, point(px(10.), px(0.)));

        let mut continued = point(px(3.), px(2.));
        scroll.filter_at(
            &mut continued,
            TouchPhase::Moved,
            now + Duration::from_millis(1),
        );
        assert_eq!(continued, point(px(3.), px(0.)));
    }
}
