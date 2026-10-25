use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, DispatchPhase, ListOffset, ListState, ScrollWheelEvent, TouchPhase,
    Window, canvas, div, prelude::*, px,
};

use super::{ClanRow, ClanSidebar};
use crate::components::primitives::mention_float_button;

const MENTION_SCROLL_DURATION: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(super) struct RailWheel {
    phased: bool,
    suppress_continuation: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum WheelAction {
    Pass,
    Ignore,
    Cancel,
}

impl RailWheel {
    fn begin_reveal(&mut self) {
        self.suppress_continuation = self.phased;
    }

    fn action(&mut self, event: &ScrollWheelEvent, revealing: bool) -> WheelAction {
        if event.touch_phase == TouchPhase::Started {
            self.phased = true;
            self.suppress_continuation = false;
        }
        if self.suppress_continuation && event.delta.precise() {
            return WheelAction::Ignore;
        }
        if !event.delta.precise() {
            self.suppress_continuation = false;
        }
        if revealing && event.delta.pixel_delta(px(1.)).y != px(0.) {
            WheelAction::Cancel
        } else {
            WheelAction::Pass
        }
    }
}

pub(super) struct MentionScroll {
    start: f32,
    target_ix: usize,
    started_at: Instant,
    viewport_height: gpui::Pixels,
}

fn logical_position(list: &ListState) -> f32 {
    let offset = list.logical_scroll_top();
    offset.item_ix as f32 + f32::from(offset.offset_in_item) / super::CLAN_ROW_HEIGHT
}

impl MentionScroll {
    fn new(list: &ListState, target_ix: usize, now: Instant) -> Self {
        Self {
            start: logical_position(list),
            target_ix,
            started_at: now,
            viewport_height: list.viewport_bounds().size.height,
        }
    }

    fn advance(&mut self, list: &ListState, now: Instant) -> bool {
        let height = list.viewport_bounds().size.height;
        if height <= px(0.) || self.target_ix >= list.item_count() {
            return false;
        }
        if height != self.viewport_height {
            self.start = logical_position(list);
            self.started_at = now;
            self.viewport_height = height;
        }
        let target = (self.target_ix as f32 + 0.5
            - f32::from(height) / (2. * super::CLAN_ROW_HEIGHT))
            .max(0.);
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f32()
            / MENTION_SCROLL_DURATION.as_secs_f32())
        .min(1.);
        let position = self.start + (target - self.start) * (1. - (1. - progress).powi(3));
        let item_ix = (position.floor() as usize).min(list.item_count().saturating_sub(1));
        list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px((position - item_ix as f32) * super::CLAN_ROW_HEIGHT),
        });
        progress < 1.
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) struct MentionTargets {
    above: Option<usize>,
    below: Option<usize>,
}

impl MentionTargets {
    fn visibility(self) -> (bool, bool) {
        (self.above.is_some(), self.below.is_some())
    }
}

pub(super) fn mentioned_clan_indices(rows: &[ClanRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.shows_mention_badge())
        .map(|(ix, _)| ix)
        .collect()
}

fn mention_targets(mentions: &[usize], visible: Range<usize>) -> MentionTargets {
    let above = mentions.partition_point(|ix| *ix < visible.start);
    let below = mentions.partition_point(|ix| *ix < visible.end);
    MentionTargets {
        above: above.checked_sub(1).map(|ix| mentions[ix]),
        below: mentions.get(below).copied(),
    }
}

impl ClanSidebar {
    pub(super) fn cancel_mention_reveal(&mut self) {
        self.mention_scroll = None;
        self.rail_wheel.suppress_continuation = false;
    }

    pub(super) fn current_mention_targets(&self) -> MentionTargets {
        if self.clan_drag_active {
            MentionTargets::default()
        } else {
            list_mention_targets(&self.mention_indices, &self.list_state)
        }
    }

    pub(super) fn mention_layout_observer(
        &self,
        rendered: MentionTargets,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity().downgrade();
        let input_view = view.clone();
        canvas(
            move |_, _, cx| {
                let Some(view) = view.upgrade() else { return };
                let sidebar = view.read(cx);
                if sidebar.current_mention_targets().visibility() != rendered.visibility()
                    || sidebar.clan_drag_active && !cx.has_active_drag()
                {
                    cx.defer(move |cx| {
                        view.update(cx, |this, cx| {
                            if this.clan_drag_active && !cx.has_active_drag() {
                                this.clan_drag_active = false;
                            }
                            if this.current_mention_targets().visibility() != rendered.visibility()
                            {
                                cx.notify();
                            }
                        });
                    });
                }
            },
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                    let inside = bounds.contains(&event.position);
                    if phase == DispatchPhase::Capture
                        && (inside || event.touch_phase == TouchPhase::Started)
                    {
                        let _ = input_view.update(cx, |this, cx| {
                            this.handle_rail_wheel(event, inside, cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    fn handle_rail_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        inside: bool,
        cx: &mut Context<Self>,
    ) {
        match self
            .rail_wheel
            .action(event, inside && self.mention_scroll.is_some())
        {
            WheelAction::Ignore => cx.stop_propagation(),
            WheelAction::Cancel => self.cancel_mention_reveal(),
            WheelAction::Pass => {}
        }
    }

    pub(super) fn render_mention_indicator(
        &self,
        targets: MentionTargets,
        above: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        (if above { targets.above } else { targets.below })?;
        Some(
            div()
                .absolute()
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .when(above, |el| el.top_0())
                .when(!above, |el| el.bottom_0())
                .child(
                    mention_float_button(
                        if above {
                            "clan-mention-above"
                        } else {
                            "clan-mention-below"
                        },
                        &self.mention_labels,
                        true,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let targets = this.current_mention_targets();
                        if let Some(ix) = if above { targets.above } else { targets.below } {
                            this.start_mention_scroll(ix, window, cx);
                        }
                    })),
                )
                .into_any_element(),
        )
    }

    fn start_mention_scroll(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.rail_wheel.begin_reveal();
        self.list_state
            .scroll_to(self.list_state.logical_scroll_top());
        self.mention_scroll = Some(MentionScroll::new(&self.list_state, ix, Instant::now()));
        self.schedule_mention_scroll(window, cx);
    }

    fn schedule_mention_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention_scroll_queued {
            return;
        }
        self.mention_scroll_queued = true;
        let view = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = view.update(cx, |this, cx| {
                this.mention_scroll_queued = false;
                this.advance_mention_scroll(window, cx);
            });
        });
    }

    fn advance_mention_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut animation) = self.mention_scroll.take() else {
            return;
        };
        if animation.advance(&self.list_state, Instant::now()) {
            self.mention_scroll = Some(animation);
            self.schedule_mention_scroll(window, cx);
        }
        cx.notify();
    }
}

fn list_mention_targets(mentions: &[usize], list: &ListState) -> MentionTargets {
    let height = list.viewport_bounds().size.height;
    if height <= px(0.) {
        return MentionTargets::default();
    }
    let start = logical_position(list);
    let end = start + f32::from(height) / super::CLAN_ROW_HEIGHT;
    mention_targets(
        mentions,
        start.ceil() as usize..(end.floor() as usize).max(start.ceil() as usize),
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    struct TestClanRail(ListState);

    impl gpui::Render for TestClanRail {
        fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
            let count = self.0.item_count();
            gpui::list(self.0.clone(), move |ix, _, _| {
                if ix + 1 == count {
                    let button = || div().size(px(super::super::CLAN_FOOTER_BUTTON_SIZE));
                    super::super::clan_footer_layout()
                        .child(button())
                        .child(button())
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .h(px(super::super::CLAN_ROW_HEIGHT))
                        .into_any_element()
                }
            })
            .size_full()
        }
    }
    fn draw_rail(cx: &mut gpui::VisualTestContext, state: &ListState, height: f32) {
        draw_rail_at_width(cx, state, 72., height);
    }

    fn draw_rail_at_width(
        cx: &mut gpui::VisualTestContext,
        state: &ListState,
        width: f32,
        height: f32,
    ) {
        let state = state.clone();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(width), px(height)),
            move |_, cx| cx.new(|_| TestClanRail(state)).into_any_element(),
        );
    }

    fn lazy_state(count: usize) -> ListState {
        let state = super::super::new_clan_list_state();
        super::super::reset_clan_list(&state, count);
        state
    }

    #[gpui::test]
    fn deep_unmeasured_clans_are_reached_in_both_directions(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let state = lazy_state(1000);
        draw_rail(cx, &state, 224.);
        assert!(state.bounds_for_item(900).is_none());
        let now = Instant::now();
        let mut animation = MentionScroll::new(&state, 900, now);
        assert!(animation.advance(&state, now + MENTION_SCROLL_DURATION / 2));
        draw_rail_at_width(cx, &state, 64., 224.);
        let middle = logical_position(&state);
        assert!(middle > 0. && middle < 900.);
        assert!(!animation.advance(&state, now + MENTION_SCROLL_DURATION));
        draw_rail_at_width(cx, &state, 64., 224.);
        assert_eq!(
            list_mention_targets(&[900], &state).visibility(),
            (false, false)
        );
        assert!(state.bounds_for_item(900).is_some());
        let mut above = MentionScroll::new(&state, 1, now);
        assert!(!above.advance(&state, now + MENTION_SCROLL_DURATION));
        draw_rail(cx, &state, 224.);
        assert!(state.bounds_for_item(1).is_some());
        MentionScroll::new(&state, 999, now).advance(&state, now + MENTION_SCROLL_DURATION);
        draw_rail(cx, &state, 224.);
        let last = state.bounds_for_item(999).expect("last clan was revealed");
        assert!(last.bottom() <= state.viewport_bounds().bottom());
    }

    #[gpui::test]
    fn layout_changes_retarget_the_scroll_instead_of_aborting(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let state = lazy_state(100);
        draw_rail(cx, &state, 224.);
        let now = Instant::now();
        let mut animation = MentionScroll::new(&state, 90, now);
        assert!(animation.advance(&state, now + MENTION_SCROLL_DURATION / 2));
        draw_rail(cx, &state, 112.);
        assert!(animation.advance(&state, now + MENTION_SCROLL_DURATION));
        assert!(!animation.advance(&state, now + MENTION_SCROLL_DURATION * 2));
        draw_rail(cx, &state, 112.);
        assert_eq!(
            list_mention_targets(&[90], &state).visibility(),
            (false, false)
        );
    }

    #[test]
    fn new_scroll_gestures_interrupt_but_old_momentum_does_not() {
        let mut wheel = RailWheel::default();
        let event = |phase, delta| ScrollWheelEvent {
            touch_phase: phase,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(delta))),
            ..Default::default()
        };
        assert_eq!(
            wheel.action(&event(TouchPhase::Started, 1.), false),
            WheelAction::Pass
        );
        wheel.action(&event(TouchPhase::Ended, 0.), false);
        wheel.begin_reveal();
        assert_eq!(
            wheel.action(&event(TouchPhase::Moved, 10.), true),
            WheelAction::Ignore
        );
        assert_eq!(
            wheel.action(&event(TouchPhase::Ended, 0.), false),
            WheelAction::Ignore
        );
        assert_eq!(
            wheel.action(&event(TouchPhase::Started, 10.), true),
            WheelAction::Cancel
        );
        let mut unphased = RailWheel::default();
        unphased.begin_reveal();
        assert_eq!(
            unphased.action(&event(TouchPhase::Moved, 10.), true),
            WheelAction::Cancel
        );
        let lines = ScrollWheelEvent {
            delta: gpui::ScrollDelta::Lines(gpui::point(0., 1.)),
            ..Default::default()
        };
        wheel.begin_reveal();
        assert_eq!(wheel.action(&lines, true), WheelAction::Cancel);
        assert_eq!(
            wheel.action(&event(TouchPhase::Moved, 0.), true),
            WheelAction::Pass
        );
    }
    fn row(id: i64, badge_count: u32, muted: bool) -> ClanRow {
        ClanRow {
            id: id.to_string().into(),
            id_num: mezon_store::ClanId(id),
            row_id: format!("clan-{id}").into(),
            group_name: format!("clan-group-{id}").into(),
            name: "Clan".into(),
            proxied_avatar_url: None,
            avatar_id: format!("clan-avatar-{id}").into(),
            badge_count,
            has_unread: true,
            muted,
            active: false,
        }
    }

    #[test]
    fn targets_follow_badges_mute_read_reorder_and_removal() {
        let mut rows = vec![row(1, 0, false), row(2, 2, true), row(3, 1, false)];
        assert_eq!(mentioned_clan_indices(&rows), vec![2]);
        rows[1].muted = false;
        assert_eq!(mentioned_clan_indices(&rows), vec![1, 2]);
        rows.swap(0, 2);
        assert_eq!(mentioned_clan_indices(&rows), vec![0, 1]);
        rows[0].badge_count = 0;
        assert_eq!(mentioned_clan_indices(&rows), vec![1]);
        rows.remove(1);
        assert!(mentioned_clan_indices(&rows).is_empty());
        rows[1].badge_count = 1;
        assert_eq!(mentioned_clan_indices(&rows), vec![1]);
        rows.clear();
        assert!(mentioned_clan_indices(&rows).is_empty());
    }

    #[test]
    fn mentions_outside_viewport_have_directional_targets() {
        assert_eq!(
            mention_targets(&[0, 2, 4, 6, 9], 3..6),
            MentionTargets {
                above: Some(2),
                below: Some(6)
            },
        );
    }

    #[test]
    fn visible_mentions_do_not_hide_mentions_further_down() {
        assert_eq!(
            mention_targets(&[2, 9], 1..4),
            MentionTargets {
                above: None,
                below: Some(9)
            },
        );
    }

    #[test]
    fn empty_or_fully_visible_lists_have_no_indicators() {
        assert_eq!(mention_targets(&[], 0..5), MentionTargets::default());
        assert_eq!(mention_targets(&[1, 2], 0..3), MentionTargets::default());
    }

    #[gpui::test]
    fn layout_resize_and_reveal_update_the_production_targets(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let state = lazy_state(19);
        assert_eq!(
            list_mention_targets(&[15], &state),
            MentionTargets::default()
        );

        draw_rail(cx, &state, 196.);
        assert_eq!(list_mention_targets(&[3], &state).below, Some(3));
        state.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(28.),
        });
        draw_rail(cx, &state, 224.);
        assert_eq!(list_mention_targets(&[0], &state).above, Some(0));
        assert_eq!(list_mention_targets(&[1, 15], &state).below, Some(15));
        let now = Instant::now();
        MentionScroll::new(&state, 15, now).advance(&state, now + MENTION_SCROLL_DURATION);
        draw_rail(cx, &state, 224.);
        assert_eq!(list_mention_targets(&[1, 15], &state).below, None);
        assert_eq!(list_mention_targets(&[1, 15], &state).above, Some(1));
        assert_eq!(list_mention_targets(&[], &state), MentionTargets::default());
        state.scroll_to(ListOffset::default());
        draw_rail(cx, &state, 1120.);
        assert_eq!(
            list_mention_targets(&[1, 15], &state),
            MentionTargets::default()
        );
    }
}
