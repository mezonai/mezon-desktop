use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Point, Render, SharedString, Styled,
    Window, div, px,
};
use mezon_store::{ChannelId, ClanId, SidebarOrderKey};

use crate::theme::ActiveTheme;

/// Colour of the line that shows where a dragged row would land.
pub(super) const DRAG_INDICATOR_COLOR: u32 = 0x3b82f6;

/// A category being dragged in the sidebar. `index` counts only the categories a user can
/// reorder, in the order they are drawn — Favourites is pinned and never takes part.
#[derive(Clone)]
pub(super) struct CategoryReorderDrag {
    pub(super) index: usize,
    pub(super) name: SharedString,
}

/// A channel or thread row being dragged in the sidebar, carried on the row itself so a drop
/// target can tell at a glance whether the two belong together.
#[derive(Clone, PartialEq)]
pub(super) struct ChannelReorderDrag {
    /// The list the row sits in. A drop only counts inside the same one: moving a channel to
    /// another category, or a thread to another channel, is the server's to record and this
    /// order is nobody's but this machine's.
    pub(super) key: SidebarOrderKey,
    /// Where the row sits among the rows of that list as drawn, which is what decides the
    /// edge the drop indicator goes on.
    pub(super) index: usize,
    pub(super) channel_id: ChannelId,
}

#[derive(Clone)]
pub(super) struct CategorySortView {
    pub(super) from: usize,
    pub(super) lifted: bool,
    shifts: Rc<HashMap<usize, Shift>>,
}

impl CategorySortView {
    pub(super) fn shift(&self, index: usize) -> Option<Shift> {
        self.shifts.get(&index).copied()
    }
}

pub(super) const SHIFT_DURATION: Duration = Duration::from_millis(160);

pub(super) fn shift_easing(delta: f32) -> f32 {
    gpui::ease_out_quint()(delta)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Shift {
    pub(super) from: f32,
    pub(super) to: f32,
    started: Instant,
    pub(super) id: u64,
}

impl Shift {
    fn at(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let delta = (elapsed / SHIFT_DURATION.as_secs_f32()).min(1.0);
        self.from + (self.to - self.from) * shift_easing(delta)
    }
}

pub(super) const LIFT_DISTANCE: f32 = 8.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Settle {
    Wait,
    Align(Option<Pixels>),
    Track,
}

pub(super) struct Over<'a> {
    pub(super) index: usize,
    pub(super) id: &'a str,
    pub(super) bounds: Bounds<Pixels>,
}

pub(super) struct CategorySort {
    pub(super) clan_id: ClanId,
    pub(super) from: usize,
    pub(super) from_id: String,
    pub(super) over: usize,
    pub(super) over_id: String,
    pub(super) pointer: Point<Pixels>,
    phase: Phase,
    from_bounds: Option<Bounds<Pixels>>,
    over_bounds: Option<Bounds<Pixels>>,
    shifts: Rc<HashMap<usize, Shift>>,
    next_id: u64,
}

#[derive(Clone, Copy)]
enum Phase {
    Pressed {
        row_top: Option<Pixels>,
        press: Point<Pixels>,
    },
    Settling {
        row_top: Option<Pixels>,
        laid_out: bool,
    },
    Aligned,
    Sorting,
}

impl CategorySort {
    pub(super) fn new(
        clan_id: ClanId,
        from: usize,
        from_id: String,
        row_top: Option<Pixels>,
        press: Point<Pixels>,
    ) -> Self {
        Self {
            clan_id,
            from,
            over: from,
            over_id: from_id.clone(),
            from_id,
            pointer: press,
            phase: Phase::Pressed { row_top, press },
            from_bounds: None,
            over_bounds: None,
            shifts: Rc::default(),
            next_id: 0,
        }
    }

    pub(super) fn lift(&mut self, pointer: Point<Pixels>) -> bool {
        self.pointer = pointer;
        let Phase::Pressed { row_top, press } = self.phase else {
            return false;
        };
        let dx = f32::from(pointer.x - press.x);
        let dy = f32::from(pointer.y - press.y);
        if dx.hypot(dy) < LIFT_DISTANCE {
            return false;
        }
        self.phase = Phase::Settling {
            row_top,
            laid_out: false,
        };
        true
    }

    pub(super) fn lifted(&self) -> bool {
        !matches!(self.phase, Phase::Pressed { .. })
    }

    pub(super) fn sorting(&self) -> bool {
        matches!(self.phase, Phase::Sorting)
    }

    pub(super) fn settle(&mut self) -> Option<Settle> {
        let (phase, step) = match self.phase {
            Phase::Settling {
                row_top,
                laid_out: false,
            } => (
                Phase::Settling {
                    row_top,
                    laid_out: true,
                },
                Settle::Wait,
            ),
            Phase::Settling { row_top, .. } => (Phase::Aligned, Settle::Align(row_top)),
            Phase::Aligned => (Phase::Sorting, Settle::Track),
            Phase::Pressed { .. } | Phase::Sorting => return None,
        };
        self.phase = phase;
        Some(step)
    }

    pub(super) fn track(
        &mut self,
        from_bounds: Option<Bounds<Pixels>>,
        over: Option<Over<'_>>,
        now: Instant,
    ) -> bool {
        if !self.sorting() {
            return false;
        }
        let previous = self.over;
        let mut moved = false;
        if let Some(bounds) = from_bounds {
            moved |= self.from_bounds.replace(bounds) != Some(bounds);
        }
        if let Some(over) = over {
            if over.index != self.over {
                self.over = over.index;
                self.over_id = over.id.to_owned();
            }
            moved |= self.over_bounds.replace(over.bounds) != Some(over.bounds);
        }
        if !moved && previous == self.over {
            return false;
        }
        let low = self.from.min(previous).min(self.over);
        let high = self.from.max(previous).max(self.over);
        self.retarget(low..=high, now)
    }

    fn retarget(&mut self, range: std::ops::RangeInclusive<usize>, now: Instant) -> bool {
        let mut changed = false;
        for index in range {
            let to = self.target(index);
            let current = self.shifts.get(&index).copied();
            if (current.map_or(0.0, |shift| shift.to) - to).abs() < 0.5 {
                continue;
            }
            let from = current.map_or(0.0, |shift| shift.at(now));
            Rc::make_mut(&mut self.shifts).insert(
                index,
                Shift {
                    from,
                    to,
                    started: now,
                    id: self.next_id,
                },
            );
            self.next_id += 1;
            changed = true;
        }
        changed
    }

    pub(super) fn view(&self) -> CategorySortView {
        CategorySortView {
            from: self.from,
            lifted: self.lifted(),
            shifts: self.shifts.clone(),
        }
    }

    fn target(&self, index: usize) -> f32 {
        let Some(from) = self.from_bounds else {
            return 0.0;
        };
        if index == self.from {
            let Some(over) = self.over_bounds else {
                return 0.0;
            };
            let offset = if self.over > self.from {
                over.bottom() - from.bottom()
            } else {
                over.top() - from.top()
            };
            return f32::from(offset);
        }
        let lifted = f32::from(from.size.height);
        if self.from < index && index <= self.over {
            -lifted
        } else if self.over <= index && index < self.from {
            lifted
        } else {
            0.0
        }
    }
}

pub(super) struct RowDragPreview {
    pub(super) name: SharedString,
}

impl Render for RowDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .bg(theme.bg_tertiary)
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_primary)
            .child(self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use gpui::{Bounds, point, px, size};

    use super::{CategorySort, Over, SHIFT_DURATION, Settle, shift_easing};
    use mezon_store::ClanId;

    fn header(index: usize) -> Bounds<gpui::Pixels> {
        Bounds {
            origin: point(px(0.), px(100. + 40. * index as f32)),
            size: size(px(200.), px(40.)),
        }
    }

    fn move_pointer(sort: &mut CategorySort, y: f32, now: Instant) -> bool {
        let pointer = point(px(20.), px(y));
        let ids = ["c0", "c1", "c2", "c3", "c4", "c5"];
        let over = (0..ids.len())
            .find(|&index| header(index).contains(&pointer))
            .map(|index| Over {
                index,
                id: ids[index],
                bounds: header(index),
            });
        sort.track(Some(header(sort.from)), over, now)
    }

    fn sorting(from: usize) -> CategorySort {
        let row = header(from);
        let press = point(px(20.), row.top() + px(20.));
        let mut sort =
            CategorySort::new(ClanId(1), from, format!("c{from}"), Some(row.top()), press);
        assert!(sort.lift(point(press.x, press.y + px(40.))));
        while sort.settle().is_some() {}
        assert!(sort.sorting());
        sort
    }

    fn target(sort: &CategorySort, index: usize) -> Option<f32> {
        sort.view().shift(index).map(|shift| shift.to)
    }

    #[test]
    fn nothing_moves_while_the_pointer_stays_on_the_dragged_header() {
        let mut sort = sorting(2);
        assert!(!move_pointer(&mut sort, 195., Instant::now()));
        assert_eq!(sort.over, 2);
        assert_eq!(sort.view().shift(2), None);
    }

    #[test]
    fn a_small_wobble_is_still_a_click() {
        let row = header(2);
        let press = point(px(20.), row.top() + px(20.));
        let mut sort = CategorySort::new(ClanId(1), 2, "c2".into(), Some(row.top()), press);
        assert!(!sort.lift(point(press.x + px(4.), press.y + px(5.))));
        assert!(!sort.lifted());
        assert!(!move_pointer(
            &mut sort,
            100. + 40. * 3. + 5.,
            Instant::now()
        ));
        assert_eq!(sort.over, 2);
    }

    #[test]
    fn a_sideways_drag_lifts_the_category_too() {
        let row = header(2);
        let press = point(px(20.), row.top() + px(20.));
        let mut sort = CategorySort::new(ClanId(1), 2, "c2".into(), Some(row.top()), press);
        assert!(sort.lift(point(press.x + px(30.), press.y)));
        assert!(sort.lifted());
    }

    #[test]
    fn dragging_down_lifts_the_headers_passed_and_lands_in_the_last_ones_place() {
        let mut sort = sorting(1);
        assert!(move_pointer(
            &mut sort,
            100. + 40. * 3. + 5.,
            Instant::now()
        ));
        assert_eq!((sort.over, sort.over_id.as_str()), (3, "c3"));
        assert_eq!(target(&sort, 2), Some(-40.));
        assert_eq!(target(&sort, 3), Some(-40.));
        assert_eq!(target(&sort, 1), Some(80.));
        assert_eq!(sort.view().shift(0), None);
        assert_eq!(sort.view().shift(4), None);
    }

    #[test]
    fn dragging_up_pushes_the_headers_passed_down() {
        let mut sort = sorting(4);
        move_pointer(&mut sort, 100. + 40. + 5., Instant::now());
        assert_eq!((sort.over, sort.over_id.as_str()), (1, "c1"));
        assert_eq!(target(&sort, 1), Some(40.));
        assert_eq!(target(&sort, 3), Some(40.));
        assert_eq!(target(&sort, 4), Some(-120.));
    }

    #[test]
    fn a_header_sliding_under_a_still_pointer_changes_nothing() {
        let mut sort = sorting(1);
        let now = Instant::now();
        move_pointer(&mut sort, 100. + 40. * 3. + 5., now);
        assert!(!move_pointer(&mut sort, 100. + 40. * 3. + 5., now));
        assert_eq!(sort.over, 3);
    }

    #[test]
    fn a_pointer_over_no_header_keeps_the_last_one() {
        let mut sort = sorting(1);
        let now = Instant::now();
        move_pointer(&mut sort, 100. + 40. * 3. + 5., now);
        assert!(!sort.track(Some(header(1)), None, now));
        assert_eq!((sort.over, sort.over_id.as_str()), (3, "c3"));
    }

    #[test]
    fn coming_back_sends_the_headers_home() {
        let mut sort = sorting(1);
        let now = Instant::now();
        move_pointer(&mut sort, 100. + 40. * 3. + 5., now);
        move_pointer(&mut sort, 100. + 40. + 5., now + SHIFT_DURATION);
        assert_eq!((sort.over, sort.over_id.as_str()), (1, "c1"));
        for index in 1..=3 {
            assert_eq!(target(&sort, index), Some(0.));
        }
    }

    #[test]
    fn a_slide_cut_short_carries_on_from_where_the_header_is() {
        let mut sort = sorting(1);
        let start = Instant::now();
        move_pointer(&mut sort, 100. + 40. * 2. + 5., start);
        let halfway = start + SHIFT_DURATION / 2;
        move_pointer(&mut sort, 100. + 40. + 5., halfway);
        let shift = sort.view().shift(2).expect("header 2 slides back");
        let drawn = -40. * shift_easing(0.5);
        assert!(
            (shift.from - drawn).abs() < 0.01,
            "{} vs {drawn}",
            shift.from
        );
        assert_eq!(shift.to, 0.);
    }

    #[test]
    fn the_pointer_counts_only_once_the_collapsed_list_is_aligned() {
        let row = header(5);
        let press = point(px(20.), row.top() + px(20.));
        let mut sort = CategorySort::new(ClanId(1), 5, "c5".into(), Some(row.top()), press);
        assert_eq!(sort.settle(), None);
        assert!(sort.lift(point(press.x, press.y + px(40.))));

        assert_eq!(sort.settle(), Some(Settle::Wait));
        assert!(!move_pointer(&mut sort, 150., Instant::now()));
        assert_eq!(sort.settle(), Some(Settle::Align(Some(row.top()))));
        assert!(!move_pointer(&mut sort, 150., Instant::now()));
        assert_eq!(sort.settle(), Some(Settle::Track));
        assert!(sort.sorting());
        assert_eq!(sort.settle(), None);

        assert!(move_pointer(&mut sort, 150., Instant::now()));
        assert_eq!(sort.over, 1);
    }
}
