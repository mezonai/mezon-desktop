use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Bounds, Global, IntoElement, Pixels, Size, Window, canvas, prelude::*};

use crate::clan::settings::ClanSettingsPage;

static PROBING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TourAnchor {
    ClanRail,
    ClanHeader,
    ChannelList,
    DirectList,
    UserInfoBar,
    ChannelHeaderTools,
    MessageTimeline,
    Composer,
    ComposerTools,
    MemberList,
    VoiceControls,
    ClanSettingsNav,
    ChannelHeaderSearch,
    CreateChannel,
    FriendsButton,
    FriendsPage,
    ClanMembersRow,
    AddFriendButton,
    ClanSettingsRow(ClanSettingsPage),
}

fn area(bounds: Bounds<Pixels>) -> f32 {
    bounds.size.width.as_f32() * bounds.size.height.as_f32()
}

fn is_usable(bounds: Bounds<Pixels>, viewport: Size<Pixels>, clip: Bounds<Pixels>) -> bool {
    let visible = bounds.intersect(&clip);
    visible.size.width > Pixels::ZERO
        && visible.size.height > Pixels::ZERO
        && visible.right() > Pixels::ZERO
        && visible.bottom() > Pixels::ZERO
        && visible.left() < viewport.width
        && visible.top() < viewport.height
}

fn outranks(candidate: Bounds<Pixels>, existing: Bounds<Pixels>) -> bool {
    let (candidate_area, existing_area) = (area(candidate), area(existing));
    if candidate_area != existing_area {
        return candidate_area > existing_area;
    }
    (candidate.top(), candidate.left()) < (existing.top(), existing.left())
}

#[derive(Debug, Clone, Copy)]
struct AnchorRecord {
    bounds: Bounds<Pixels>,
    epoch: u64,
}

#[derive(Default)]
pub struct TourAnchors {
    records: RefCell<HashMap<TourAnchor, AnchorRecord>>,
    epoch: u64,
}

impl Global for TourAnchors {}

impl TourAnchors {
    pub fn is_probing() -> bool {
        PROBING.load(Ordering::Relaxed)
    }

    pub fn set_probing(cx: &mut App, probing: bool) {
        PROBING.store(probing, Ordering::Relaxed);
        if !probing {
            cx.default_global::<Self>().records.borrow_mut().clear();
        }
    }

    pub fn begin_epoch(cx: &mut App) -> u64 {
        let this = cx.default_global::<Self>();
        this.epoch = this.epoch.wrapping_add(1);
        this.records.borrow_mut().clear();
        this.epoch
    }

    pub fn live(cx: &App, anchor: TourAnchor, epoch: u64) -> Option<Bounds<Pixels>> {
        let this = cx.try_global::<Self>()?;
        let records = this.records.borrow();
        let record = records.get(&anchor)?;
        (record.epoch == epoch).then_some(record.bounds)
    }

    fn record(cx: &App, anchor: TourAnchor, bounds: Bounds<Pixels>, window: &Window) {
        if !Self::is_probing() {
            return;
        }
        let Some(this) = cx.try_global::<Self>() else {
            return;
        };
        if !is_usable(bounds, window.viewport_size(), window.content_mask().bounds) {
            return;
        }
        let epoch = this.epoch;
        let mut records = this.records.borrow_mut();
        if let Some(existing) = records.get(&anchor)
            && existing.epoch == epoch
            && !outranks(bounds, existing.bounds)
        {
            return;
        }
        records.insert(anchor, AnchorRecord { bounds, epoch });
    }
}

pub fn probe(anchor: TourAnchor) -> Option<impl IntoElement + use<>> {
    if !TourAnchors::is_probing() {
        return None;
    }
    Some(
        canvas(
            move |bounds, window, cx| TourAnchors::record(cx, anchor, bounds, window),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{bounds, point, px, size};

    fn viewport() -> Size<Pixels> {
        size(px(1200.), px(800.))
    }

    fn unclipped() -> Bounds<Pixels> {
        bounds(point(Pixels::ZERO, Pixels::ZERO), viewport())
    }

    #[test]
    fn a_row_clipped_away_by_its_scroll_container_is_not_usable() {
        let clip = bounds(point(px(0.), px(300.)), size(px(220.), px(400.)));
        let scrolled_out = bounds(point(px(0.), px(120.)), size(px(220.), px(40.)));
        assert!(!is_usable(scrolled_out, viewport(), clip));
    }

    #[test]
    fn a_row_inside_its_scroll_container_is_usable() {
        let clip = bounds(point(px(0.), px(300.)), size(px(220.), px(400.)));
        let visible = bounds(point(px(0.), px(360.)), size(px(220.), px(40.)));
        assert!(is_usable(visible, viewport(), clip));
    }

    #[test]
    fn an_offscreen_box_is_not_usable() {
        let above = bounds(point(px(10.), px(-80.)), size(px(100.), px(40.)));
        assert!(!is_usable(above, viewport(), unclipped()));
    }

    #[test]
    fn an_empty_box_is_not_usable() {
        let empty = bounds(point(px(10.), px(10.)), size(px(120.), Pixels::ZERO));
        assert!(!is_usable(empty, viewport(), unclipped()));
    }

    #[test]
    fn the_larger_box_wins_a_run() {
        let small = bounds(point(px(10.), px(10.)), size(px(20.), px(20.)));
        let large = bounds(point(px(10.), px(10.)), size(px(200.), px(40.)));
        assert!(outranks(large, small));
        assert!(!outranks(small, large));
    }

    #[test]
    fn equal_boxes_resolve_to_the_topmost_one() {
        let lower = bounds(point(px(10.), px(400.)), size(px(22.), px(22.)));
        let upper = bounds(point(px(10.), px(120.)), size(px(22.), px(22.)));
        assert!(outranks(upper, lower));
        assert!(!outranks(lower, upper));
    }
}
