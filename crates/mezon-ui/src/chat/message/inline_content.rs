use std::cell::Cell;
use std::mem;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FontWeight, GlobalElementId,
    HighlightStyle, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseDownEvent, MouseUpEvent, Pixels, SharedString, StyledText, TextLayout,
    TransformationMatrix, Window, point, px, size,
};

use super::selection::{SharedSelection, merge_selection_background};
use crate::components::primitives::IconName;

const INLINE_ICON_SIZE: Pixels = px(16.);
/// Icon height as a fraction of the em box. Matches the 11px the icon has always been
/// drawn at in a 16px body, which is what the reserve happened to give on macOS before
/// the placeholder was made deterministic.
const INLINE_ICON_EM_RATIO: f32 = 0.6875;

pub struct StyledRun {
    pub range: Range<usize>,
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
    /// Pins the run to one weight. The icon placeholder needs it: its advance is
    /// the box the icon is painted into, and gg sans widens `‰` from 1.004em at
    /// Normal to 1.092em at ExtraBold, which would resize the icon with the text.
    pub font_weight: Option<FontWeight>,
    /// Fades the run out, 1.0 being invisible. A transparent `color` cannot do this:
    /// `HighlightStyle` blends its color over the base one, so alpha 0 is a no-op.
    pub fade_out: Option<f32>,
}

pub struct IconOverlay {
    pub byte_index: usize,
    pub end_index: usize,
    pub icon: IconName,
    pub color: Hsla,
}

pub struct ClickRegion {
    pub range: Range<usize>,
    pub action: Box<dyn Fn(&mut Window, &mut App)>,
}

#[derive(Default)]
struct InlineContentState {
    mouse_down_index: Rc<Cell<Option<usize>>>,
}

pub struct InlineContent {
    element_id: ElementId,
    styled: StyledText,
    icons: Vec<IconOverlay>,
    clicks: Vec<ClickRegion>,
    selection_state: SharedSelection,
}

impl InlineContent {
    pub fn new(
        id: impl Into<ElementId>,
        text: SharedString,
        runs: Vec<StyledRun>,
        icons: Vec<IconOverlay>,
        clicks: Vec<ClickRegion>,
        body_color: Hsla,
        selection: Option<(Range<usize>, Hsla)>,
        selection_state: SharedSelection,
    ) -> Self {
        let mut highlights = build_highlights(&text, runs, body_color);
        if let Some((range, bg)) = selection {
            highlights = merge_selection_background(&highlights, range, bg);
        }
        let styled = StyledText::new(text).with_highlights(highlights);
        Self {
            element_id: id.into(),
            styled,
            icons,
            clicks,
            selection_state,
        }
    }

    pub fn text_layout(&self) -> TextLayout {
        self.styled.layout().clone()
    }
}

fn build_highlights(
    text: &str,
    mut runs: Vec<StyledRun>,
    body_color: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    runs.sort_by_key(|run| run.range.start);
    let body_style = HighlightStyle {
        color: Some(body_color),
        ..Default::default()
    };
    let mut highlights = Vec::with_capacity(runs.len() * 2 + 1);
    let mut cursor = 0;
    for run in runs {
        if run.range.start < cursor {
            continue;
        }
        if cursor < run.range.start {
            highlights.push((cursor..run.range.start, body_style));
        }
        highlights.push((
            run.range.clone(),
            HighlightStyle {
                color: run.color.or(Some(body_color)),
                background_color: run.background,
                font_weight: run.font_weight,
                fade_out: run.fade_out,
                ..Default::default()
            },
        ));
        cursor = run.range.end;
    }
    if cursor < text.len() {
        highlights.push((cursor..text.len(), body_style));
    }
    highlights
}

impl Element for InlineContent {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.element_id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.styled.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.styled
            .prepaint(None, inspector_id, bounds, state, window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text_layout = self.styled.layout().clone();
        let icons = mem::take(&mut self.icons);
        window.with_element_state::<InlineContentState, _>(global_id.unwrap(), |state, window| {
            let state = state.unwrap_or_default();

            let mouse_position = window.mouse_position();
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
            if let Some(Ok(ix)) = text_layout.try_index_for_position(mouse_position)
                && self.clicks.iter().any(|region| region.range.contains(&ix))
            {
                window.set_cursor_style(CursorStyle::PointingHand, hitbox);
            }

            let mouse_down = state.mouse_down_index.clone();
            let clicks = mem::take(&mut self.clicks);

            {
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let mouse_down = mouse_down.clone();
                window.on_mouse_event(
                    move |event: &MouseDownEvent, phase, window: &mut Window, _: &mut App| {
                        if phase == DispatchPhase::Bubble
                            && hitbox.is_hovered(window)
                            && let Some(Ok(index)) =
                                text_layout.try_index_for_position(event.position)
                        {
                            mouse_down.set(Some(index));
                        }
                    },
                );
            }

            {
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let selection_state = self.selection_state.clone();
                window.on_mouse_event(
                    move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let Some(down_index) = mouse_down.take() else {
                            return;
                        };
                        if selection_state.borrow().has_selection() {
                            return;
                        }
                        if !hitbox.is_hovered(window) {
                            return;
                        }
                        let Some(Ok(up_index)) = text_layout.try_index_for_position(event.position)
                        else {
                            return;
                        };
                        for region in &clicks {
                            if region.range.contains(&down_index)
                                && region.range.contains(&up_index)
                            {
                                (region.action)(window, cx);
                                break;
                            }
                        }
                    },
                );
            }

            self.styled
                .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

            let line_height = text_layout.line_height();
            for overlay in &icons {
                let Some(start) = text_layout.position_for_index(overlay.byte_index) else {
                    continue;
                };
                let (icon_x, icon_y, reserved) =
                    match text_layout.position_for_index(overlay.end_index) {
                        Some(end) if end.y == start.y && end.x > start.x => {
                            (start.x, start.y, end.x - start.x)
                        }
                        Some(end) if end.y != start.y && end.x > bounds.origin.x => {
                            (bounds.origin.x, end.y, end.x - bounds.origin.x)
                        }
                        _ => (start.x, start.y, INLINE_ICON_SIZE),
                    };
                // The placeholder reserves one em; the icon is drawn smaller inside it and
                // centred, so the slack reads as padding between the icon and the label.
                let line = text_layout.line_layout_for_index(overlay.byte_index);
                let em = line
                    .as_ref()
                    .map(|line| line.unwrapped_layout.font_size)
                    .unwrap_or(reserved);
                // `reserved` is the placeholder's advance on the common path, but the wrapped
                // arm above hands back the whole distance from the row's left edge, which must
                // not be treated as the icon's slot: centring in it would push the icon halfway
                // across the line.
                let slot = reserved.min(em);
                let icon_size = (em * INLINE_ICON_EM_RATIO).min(slot).min(line_height);
                // Sit the icon on the text baseline rather than centred in the line box: a line
                // is only as tall as its tallest run, so centring drifts whenever a chip shares
                // the line with something bigger. This is the baseline gpui itself draws to
                // (`text_system::line::paint`).
                let baseline = line
                    .map(|line| {
                        let ascent = line.unwrapped_layout.ascent;
                        let descent = line.unwrapped_layout.descent;
                        (line_height - ascent - descent) / 2. + ascent
                    })
                    .unwrap_or((line_height + icon_size) / 2.);
                let icon_bounds = Bounds {
                    origin: point(
                        icon_x + (slot - icon_size) / 2.,
                        icon_y + baseline - icon_size,
                    ),
                    size: size(icon_size, icon_size),
                };
                let _ = window.paint_svg(
                    icon_bounds,
                    overlay.icon.path().into(),
                    None,
                    TransformationMatrix::default(),
                    overlay.color,
                    cx,
                );
            }

            ((), state)
        });
    }
}

impl IntoElement for InlineContent {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
