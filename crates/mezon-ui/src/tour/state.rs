use gpui::{
    AnyElement, AnyView, App, Bounds, ClickEvent, Context, Entity, FocusHandle, FontWeight, Global,
    Hsla, IntoElement, Pixels, Point, RenderOnce, SharedString, Size, Window, deferred, div, hsla,
    prelude::*, px, relative, size, svg,
};
use mezon_store::Settings;

use super::anchor::TourAnchors;
use super::overlay::{
    BUBBLE_GAP, BUBBLE_HEIGHT, BUBBLE_WIDTH, GLOW_INSET, RING_RADIUS, SCRIM_ALPHA, Side,
    VIEWPORT_MARGIN, bands, center_origin, hole_for, place,
};
use super::tracks::{TOUR_VERSION, TRACKS, TourTrack, core_track_for, track};
use crate::app::shell::Shell;
use crate::components::primitives::{Button, ButtonVariants, ToastKind, h_flex, v_flex};
use crate::router::{Route, Router};
use crate::theme::ActiveTheme;

const CARET: Pixels = px(12.);

gpui::actions!(mezon_tour, [TourNext, TourBack]);

pub struct TourStatus {
    pub resolving: bool,
    pub hole: Option<(f32, f32, f32, f32)>,
    pub track: &'static str,
    pub index: usize,
    pub position: usize,
    pub total: usize,
    pub title_key: &'static str,
    pub anchor: Option<String>,
    pub has_hole: bool,
}

pub struct TourAdvance {
    pub moved: bool,
    pub still_active: bool,
}

enum Phase {
    Idle,
    Arming {
        track: &'static TourTrack,
        index: usize,
        forward: bool,
    },
    Showing {
        track: &'static TourTrack,
        index: usize,
        visible: Vec<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourTrigger {
    Auto,
    Manual,
}

pub struct TourState {
    phase: Phase,
    trigger: TourTrigger,
    epoch: u64,
    probed_viewport: Size<Pixels>,
    restore_focus: Option<FocusHandle>,
    focus_handle: FocusHandle,
}

struct GlobalTourState(Entity<TourState>);
impl Global for GlobalTourState {}

impl TourState {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self {
            phase: Phase::Idle,
            trigger: TourTrigger::Manual,
            epoch: 0,
            probed_viewport: Size::default(),
            restore_focus: None,
            focus_handle: cx.focus_handle(),
        });
        cx.set_global(GlobalTourState(entity.clone()));
        entity
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalTourState>()
            .map(|this| this.0.clone())
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    pub fn running_track(&self) -> Option<&'static str> {
        match &self.phase {
            Phase::Arming { track, .. } | Phase::Showing { track, .. } => Some(track.id),
            Phase::Idle => None,
        }
    }

    fn running(&self) -> Option<&'static TourTrack> {
        match &self.phase {
            Phase::Arming { track, .. } | Phase::Showing { track, .. } => Some(track),
            Phase::Idle => None,
        }
    }

    pub fn status(&self, cx: &App) -> Option<TourStatus> {
        if let Phase::Arming { track, index, .. } = &self.phase {
            return Some(TourStatus {
                resolving: true,
                hole: None,
                track: track.id,
                index: *index,
                position: 0,
                total: track.steps.len(),
                title_key: track.steps[*index].title_key,
                anchor: track.steps[*index].anchor.map(|a| format!("{a:?}")),
                has_hole: false,
            });
        }
        let Phase::Showing {
            track,
            index,
            visible,
        } = &self.phase
        else {
            return None;
        };
        let step = &track.steps[*index];
        let position = visible
            .iter()
            .position(|candidate| candidate == index)
            .map_or(1, |offset| offset + 1);
        let hole = step
            .anchor
            .and_then(|anchor| TourAnchors::live(cx, anchor, self.epoch));
        Some(TourStatus {
            resolving: false,
            hole: hole.map(|h| {
                (
                    h.origin.x.as_f32(),
                    h.origin.y.as_f32(),
                    h.size.width.as_f32(),
                    h.size.height.as_f32(),
                )
            }),
            track: track.id,
            index: *index,
            position,
            total: visible.len().max(position),
            title_key: step.title_key,
            anchor: step.anchor.map(|anchor| format!("{anchor:?}")),
            has_hole: hole.is_some(),
        })
    }

    pub fn advance(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TourAdvance {
        let before = match &self.phase {
            Phase::Showing { index, .. } => Some(*index),
            _ => None,
        };
        if forward {
            self.next(window, cx);
        } else {
            self.back(cx);
        }
        let after = match &self.phase {
            Phase::Showing { index, .. } => Some(*index),
            _ => None,
        };
        TourAdvance {
            moved: after.is_some() && after != before,
            still_active: self.is_active(),
        }
    }

    pub fn start_track(id: &str, window: &mut Window, cx: &mut App) {
        Self::start_track_restoring(id, TourTrigger::Manual, None, window, cx);
    }

    pub fn start_track_restoring(
        id: &str,
        trigger: TourTrigger,
        restore_focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(entity) = Self::try_global(cx) else {
            return;
        };
        let Some(track) = track(id) else {
            return;
        };
        if !track.precondition.is_met(&current_route(cx)) {
            return;
        }
        if entity.read(cx).is_active() {
            return;
        }
        entity.update(cx, |this, cx| {
            this.trigger = trigger;
            this.restore_focus = restore_focus.or_else(|| window.focused(cx));
            this.start(track, window, cx);
        });
    }

    fn start(&mut self, track: &'static TourTrack, window: &mut Window, cx: &mut Context<Self>) {
        self.arm(track, 0, true, window, cx);
        window.focus(&self.focus_handle, cx);
    }

    fn arm(
        &mut self,
        track: &'static TourTrack,
        index: usize,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        TourAnchors::set_probing(cx, true);
        self.epoch = TourAnchors::begin_epoch(cx);
        self.probed_viewport = window.viewport_size();
        self.phase = Phase::Arming {
            track,
            index,
            forward,
        };
        window.refresh();
        cx.notify();
    }

    fn show(
        &mut self,
        track: &'static TourTrack,
        index: usize,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let visible = self.visible_steps(track, cx);
        let Some(landed) = land(&visible, index, forward) else {
            return false;
        };
        self.phase = Phase::Showing {
            track,
            index: landed,
            visible,
        };
        cx.notify();
        true
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Showing { track, index, .. } = &self.phase else {
            return;
        };
        let (track, index) = (*track, *index);
        if index + 1 >= track.steps.len() || !self.show(track, index + 1, true, cx) {
            self.finish(window, cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        let Phase::Showing { track, index, .. } = &self.phase else {
            return;
        };
        let (track, index) = (*track, *index);
        let Some(previous) = index.checked_sub(1) else {
            return;
        };
        self.show(track, previous, false, cx);
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end(Ending::Dismissed, window, cx);
    }

    fn abandon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end(Ending::Interrupted, window, cx);
    }

    fn end(&mut self, ending: Ending, window: &mut Window, cx: &mut Context<Self>) {
        let Some(track) = self.running() else {
            return;
        };
        let id = track.id;
        self.phase = Phase::Idle;
        TourAnchors::set_probing(cx, false);
        match (ending, self.restore_focus.take()) {
            (Ending::Dismissed, Some(focus)) => window.focus(&focus, cx),
            _ => window.blur(),
        }
        if ending == Ending::Dismissed {
            mark_seen(id, cx);
        }
        cx.notify();
    }

    fn visible_steps(&self, track: &'static TourTrack, cx: &App) -> Vec<usize> {
        track
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| match step.anchor {
                None => true,
                Some(anchor) => TourAnchors::live(cx, anchor, self.epoch).is_some(),
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn resolve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Arming {
            track,
            index,
            forward,
        } = &self.phase
        else {
            return;
        };
        let (track, index, forward) = (*track, *index, *forward);
        if self.show(track, index, forward, cx) {
            return;
        }
        let announce = self.trigger == TourTrigger::Manual;
        self.abandon(window, cx);
        if announce {
            let message = mezon_i18n::t(&locale(cx), "tour.empty").to_string();
            Shell::global(cx).update(cx, |shell, cx| shell.toast(ToastKind::Info, message, cx));
        }
    }

    fn reprobe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Showing { track, index, .. } = &self.phase else {
            return;
        };
        let (track, index) = (*track, *index);
        self.arm(track, index, true, window, cx);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Dismissed,
    Interrupted,
}

fn land(visible: &[usize], index: usize, forward: bool) -> Option<usize> {
    if forward {
        visible
            .iter()
            .copied()
            .find(|candidate| *candidate >= index)
    } else {
        visible
            .iter()
            .copied()
            .rev()
            .find(|candidate| *candidate <= index)
    }
}

fn current_route(cx: &App) -> Route {
    Router::global(cx).read(cx).route()
}

fn locale(cx: &App) -> String {
    Settings::try_global(cx)
        .map(|settings| settings.read(cx).language.clone())
        .unwrap_or_else(|| "en".to_string())
}

fn mark_seen(track_id: &str, cx: &mut App) {
    let Some(settings) = Settings::try_global(cx) else {
        return;
    };
    let changed = settings.update(cx, |settings, cx| {
        let mut changed = false;
        if settings.tour_seen_version < TOUR_VERSION {
            settings.tour_seen_version = TOUR_VERSION;
            settings.tour_done_tracks.clear();
            changed = true;
        }
        if !settings.tour_done_tracks.iter().any(|id| id == track_id) {
            settings.tour_done_tracks.push(track_id.to_string());
            changed = true;
        }
        if changed {
            cx.notify();
        }
        changed
    });
    if changed {
        mezon_store::schedule_settings_save(&settings, cx);
    }
}

pub fn eligibility_undecided(cx: &App) -> bool {
    Settings::try_global(cx).is_some_and(|settings| settings.read(cx).tour_eligible.is_none())
}

fn tour_eligible(cx: &mut App) -> bool {
    let Some(settings) = Settings::try_global(cx) else {
        return false;
    };
    if let Some(decided) = settings.read(cx).tour_eligible {
        return decided;
    }
    let Some(clans) = mezon_store::ClanList::try_global(cx) else {
        return false;
    };
    let clans = clans.read(cx);
    if !clans.has_listed() {
        return false;
    }
    let eligible = clans.clans.is_empty();
    settings.update(cx, |settings, cx| {
        settings.tour_eligible = Some(eligible);
        cx.notify();
    });
    mezon_store::schedule_settings_save(&settings, cx);
    eligible
}

pub fn track_done(track_id: &str, cx: &App) -> bool {
    Settings::try_global(cx).is_some_and(|settings| {
        let settings = settings.read(cx);
        settings.tour_seen_version >= TOUR_VERSION
            && settings.tour_done_tracks.iter().any(|id| id == track_id)
    })
}

pub fn pending_core_track(cx: &mut App) -> Option<&'static str> {
    let route = current_route(cx);
    let track = core_track_for(&route)?;
    if !track.precondition.is_met(&route) {
        return None;
    }
    if !tour_eligible(cx) {
        return None;
    }
    if track_done(track.id, cx) {
        return None;
    }
    Some(track.id)
}

fn runs_a_track(route: &Route) -> bool {
    TRACKS.iter().any(|track| track.precondition.is_met(route))
}

fn pick_host_route<'a>(
    current: &'a Route,
    mut recent: impl Iterator<Item = &'a Route>,
) -> &'a Route {
    if runs_a_track(current) {
        return current;
    }
    recent.find(|route| runs_a_track(route)).unwrap_or(current)
}

pub fn host_route(cx: &App) -> Route {
    let router = Router::global(cx);
    let router = router.read(cx);
    pick_host_route(router.route_ref(), router.recently_visited()).clone()
}

pub fn available_tracks_for(route: &Route) -> Vec<&'static TourTrack> {
    TRACKS
        .iter()
        .filter(|track| track.precondition.is_met(route))
        .collect()
}

fn composer_is_busy(cx: &App) -> bool {
    crate::chat::mention_input::MentionInput::active_composer(cx).is_some_and(|composer| {
        let composer = composer.read(cx);
        if composer.is_composing(cx) {
            return true;
        }
        let (text, _, attachments) = composer.current_content(cx);
        !text.is_empty() || !attachments.is_empty()
    })
}

pub fn auto_start_core(window: &mut Window, cx: &mut App) -> bool {
    let Some(id) = pending_core_track(cx) else {
        return false;
    };
    if TourState::try_global(cx).is_some_and(|entity| entity.read(cx).is_active()) {
        return false;
    }
    if Shell::global(cx).read(cx).has_modal() {
        return false;
    }
    if composer_is_busy(cx) {
        return false;
    }
    TourState::start_track_restoring(id, TourTrigger::Auto, None, window, cx);
    TourState::try_global(cx).is_some_and(|entity| entity.read(cx).is_active())
}

pub fn shutdown(cx: &mut App) {
    TourAnchors::set_probing(cx, false);
    let Some(entity) = TourState::try_global(cx) else {
        return;
    };
    if !entity.read(cx).is_active() {
        return;
    }
    let handle = crate::app::main_window::handle(cx);
    entity.update(cx, |this, cx| {
        this.phase = Phase::Idle;
        this.restore_focus = None;
        cx.notify();
    });
    if let Some(handle) = handle {
        handle.update(cx, |_, window, _| window.blur()).ok();
    }
}

pub fn layer(cx: &App) -> Option<AnyView> {
    let entity = TourState::try_global(cx)?;
    entity.read(cx).is_active().then(|| AnyView::from(entity))
}

#[derive(IntoElement)]
struct Scrim {
    hole: Option<Bounds<Pixels>>,
    viewport: Size<Pixels>,
    ring: Hsla,
}

impl RenderOnce for Scrim {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let dim = hsla(0., 0., 0., SCRIM_ALPHA);
        let ring = self.ring;

        let Some(hole) = self.hole else {
            return div().absolute().top_0().left_0().size_full().bg(dim);
        };

        let mut root = div().absolute().top_0().left_0().size_full();
        for band in bands(hole, self.viewport) {
            root = root.child(
                div()
                    .absolute()
                    .left(band.origin.x)
                    .top(band.origin.y)
                    .w(band.size.width)
                    .h(band.size.height)
                    .bg(dim),
            );
        }
        root.child(
            div()
                .absolute()
                .left(hole.origin.x - GLOW_INSET)
                .top(hole.origin.y - GLOW_INSET)
                .w(hole.size.width + GLOW_INSET * 2.)
                .h(hole.size.height + GLOW_INSET * 2.)
                .rounded(RING_RADIUS + GLOW_INSET)
                .border_1()
                .border_color(hsla(ring.h, ring.s, ring.l, 0.3)),
        )
        .child(
            div()
                .absolute()
                .left(hole.origin.x)
                .top(hole.origin.y)
                .w(hole.size.width)
                .h(hole.size.height)
                .rounded(RING_RADIUS)
                .border_2()
                .border_color(ring),
        )
    }
}

impl Render for TourState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let router = Router::global(cx);
        let off_route = self
            .running()
            .is_some_and(|track| !track.precondition.is_met(router.read(cx).route_ref()));
        if off_route {
            cx.defer_in(window, |this, window, cx| this.abandon(window, cx));
            return div();
        }

        if matches!(self.phase, Phase::Arming { .. }) {
            cx.defer_in(window, |this, window, cx| this.resolve(window, cx));
            return div();
        }

        let viewport = window.viewport_size();
        if viewport != self.probed_viewport {
            cx.defer_in(window, |this, window, cx| this.reprobe(window, cx));
            return div();
        }

        let Phase::Showing {
            track,
            index,
            visible,
        } = &self.phase
        else {
            return div();
        };
        let (track, index) = (*track, *index);
        let visible = visible.clone();

        let hole = track.steps[index]
            .anchor
            .and_then(|anchor| TourAnchors::live(cx, anchor, self.epoch))
            .map(|target| hole_for(target, viewport));
        let theme = cx.theme();
        let ring: Hsla = theme.tokens.bg_button_primary.into();
        let locale = locale(cx);
        let step = &track.steps[index];
        let position = visible
            .iter()
            .position(|candidate| *candidate == index)
            .map_or(1, |offset| offset + 1);
        let total = visible.len().max(position);
        let is_last = position >= total;

        let (origin, side) = match hole {
            Some(hole) => place(hole, size(BUBBLE_WIDTH, BUBBLE_HEIGHT), viewport),
            None => (
                center_origin(size(BUBBLE_WIDTH, BUBBLE_HEIGHT), viewport),
                Side::Center,
            ),
        };

        let scrim = Scrim {
            hole,
            viewport,
            ring,
        };

        let bubble = v_flex()
            .absolute()
            .left(origin.x)
            .top(origin.y)
            .w(BUBBLE_WIDTH)
            .h(BUBBLE_HEIGHT)
            .p(px(16.))
            .gap(px(8.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .child(
                div()
                    .flex_none()
                    .w(px(56.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_md()
                    .bg(theme.bg_hover)
                    .text_xs()
                    .text_color(theme.tokens.text_secondary)
                    .child(format!("{position} / {total}")),
            )
            .child(
                div()
                    .flex_none()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tokens.text_theme_primary)
                    .child(SharedString::from(mezon_i18n::t(&locale, step.title_key))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_sm()
                    .text_color(theme.tokens.text_secondary)
                    .child(SharedString::from(mezon_i18n::t(&locale, step.body_key))),
            )
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .h(px(3.))
                    .rounded_full()
                    .bg(theme.bg_hover)
                    .child(
                        div()
                            .h_full()
                            .w(relative(position as f32 / total as f32))
                            .rounded_full()
                            .bg(ring),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("tour-skip")
                            .label(mezon_i18n::t(&locale, "tour.skip"))
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.finish(window, cx))),
                    )
                    .child(div().flex_1())
                    .when(position > 1, |el| {
                        el.child(
                            Button::new("tour-back")
                                .label(mezon_i18n::t(&locale, "tour.back"))
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .child(
                        Button::new("tour-next")
                            .label(mezon_i18n::t(
                                &locale,
                                if is_last { "tour.done" } else { "tour.next" },
                            ))
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            );

        let caret = hole.and_then(|hole| caret_for(side, hole, origin, theme.bg_floating.into()));

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(deferred(
                div()
                    .id("tour-scrim")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .track_focus(&self.focus_handle)
                    .key_context("tour")
                    .on_action(
                        cx.listener(|this, _: &::menu::Cancel, window, cx| this.finish(window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &TourNext, window, cx| this.next(window, cx)))
                    .on_action(cx.listener(|this, _: &TourBack, _, cx| this.back(cx)))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        if event.click_count() <= 1 {
                            this.next(window, cx);
                        }
                    }))
                    .child(scrim)
                    .children(caret)
                    .child(bubble),
            ))
    }
}

fn caret_for(
    side: Side,
    hole: Bounds<Pixels>,
    origin: Point<Pixels>,
    color: Hsla,
) -> Option<AnyElement> {
    let horizontal = (hole.center().x - CARET / 2.)
        .max(origin.x + BUBBLE_GAP)
        .min(origin.x + BUBBLE_WIDTH - BUBBLE_GAP - CARET);
    let vertical = (hole.center().y - CARET / 2.)
        .max(origin.y + VIEWPORT_MARGIN)
        .min(origin.y + BUBBLE_HEIGHT - VIEWPORT_MARGIN - CARET);

    let (path, left, top, width, height) = match side {
        Side::Below => (
            "icons/tour-caret-up.svg",
            horizontal,
            origin.y - CARET / 2.,
            CARET,
            CARET / 2.,
        ),
        Side::Above => (
            "icons/tour-caret-down.svg",
            horizontal,
            origin.y + BUBBLE_HEIGHT,
            CARET,
            CARET / 2.,
        ),
        Side::Right => (
            "icons/tour-caret-left.svg",
            origin.x - CARET / 2.,
            vertical,
            CARET / 2.,
            CARET,
        ),
        Side::Left => (
            "icons/tour-caret-right.svg",
            origin.x + BUBBLE_WIDTH,
            vertical,
            CARET / 2.,
            CARET,
        ),
        Side::Center => return None,
    };

    Some(
        svg()
            .absolute()
            .left(left)
            .top(top)
            .w(width)
            .h(height)
            .path(path)
            .text_color(color)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::{available_tracks_for, land, pick_host_route};
    use crate::clan::settings::ClanSettingsPage;
    use crate::router::Route;
    use crate::tour::tracks::{CLAN_SETTINGS_TRACK_ID, TrackPrecondition};
    use mezon_store::{ChannelId, ClanId};

    #[test]
    fn forward_lands_on_the_requested_step_when_it_is_visible() {
        assert_eq!(land(&[0, 1, 2, 3], 2, true), Some(2));
    }

    #[test]
    fn forward_skips_steps_whose_anchor_is_off_screen() {
        assert_eq!(land(&[0, 3, 5], 1, true), Some(3));
        assert_eq!(land(&[0, 3, 5], 4, true), Some(5));
    }

    #[test]
    fn forward_past_the_last_visible_step_ends_the_track() {
        assert_eq!(land(&[0, 3], 4, true), None);
    }

    #[test]
    fn back_skips_backwards_over_missing_steps() {
        assert_eq!(land(&[0, 3, 5], 4, false), Some(3));
        assert_eq!(land(&[2, 4], 3, false), Some(2));
    }

    #[test]
    fn back_before_the_first_visible_step_ends_the_track() {
        assert_eq!(land(&[2, 4], 1, false), None);
    }

    #[test]
    fn an_empty_visible_set_never_lands() {
        assert_eq!(land(&[], 0, true), None);
        assert_eq!(land(&[], 0, false), None);
    }

    #[test]
    fn a_channel_route_offers_more_than_just_the_core_track() {
        let channel = Route::Channel {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
        };
        let ids: Vec<_> = available_tracks_for(&channel)
            .iter()
            .map(|track| track.id)
            .collect();
        assert!(ids.len() > 1, "a channel offers only {ids:?}");
        assert!(ids.contains(&"messaging"));
        assert!(ids.contains(&"voice"));
    }

    #[test]
    fn no_settings_route_can_run_a_conversation_track() {
        for route in [
            Route::SettingsAccount,
            Route::SettingsAppearance,
            Route::SettingsLanguage,
        ] {
            assert!(
                available_tracks_for(&route).is_empty(),
                "{route:?} must fall back to a host route instead of listing tracks"
            );
        }
    }

    #[test]
    fn clan_settings_still_offers_only_its_own_track() {
        let route = Route::ClanSettings {
            clan_id: ClanId(1),
            page: ClanSettingsPage::Overview,
        };
        let ids: Vec<_> = available_tracks_for(&route)
            .iter()
            .map(|track| track.id)
            .collect();
        assert_eq!(ids, vec![CLAN_SETTINGS_TRACK_ID]);
        assert!(TrackPrecondition::ClanSettings.is_met(&route));
    }

    fn channel() -> Route {
        Route::Channel {
            clan_id: ClanId(1),
            channel_id: ChannelId(2),
        }
    }

    #[test]
    fn opening_the_launcher_from_settings_falls_back_to_the_conversation_behind_it() {
        let current = Route::SettingsAdvanced;
        let recent = [Route::SettingsAppearance, channel(), Route::Direct];

        let host = pick_host_route(&current, recent.iter());

        assert_eq!(
            host,
            &channel(),
            "walked past the settings route to {host:?}"
        );
        assert!(
            !available_tracks_for(host).is_empty(),
            "the launcher would still be empty"
        );
    }

    #[test]
    fn a_route_that_runs_a_track_is_its_own_host() {
        let current = channel();
        let recent = [Route::Direct];
        assert_eq!(pick_host_route(&current, recent.iter()), &current);

        let clan_settings = Route::ClanSettings {
            clan_id: ClanId(1),
            page: ClanSettingsPage::Overview,
        };
        assert_eq!(
            pick_host_route(&clan_settings, recent.iter()),
            &clan_settings,
            "clan settings must keep hosting its own track"
        );
    }

    #[test]
    fn with_no_runnable_history_the_host_stays_the_current_route() {
        let current = Route::SettingsAccount;
        let recent = [Route::SettingsVoice, Route::SettingsLanguage];
        assert_eq!(pick_host_route(&current, recent.iter()), &current);
    }
}
