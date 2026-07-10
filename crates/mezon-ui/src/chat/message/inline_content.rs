use std::cell::Cell;
use std::mem;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, HighlightStyle,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseDownEvent,
    MouseUpEvent, Pixels, SharedString, StyledText, TransformationMatrix, Window, point, px, size,
};

use crate::components::primitives::IconName;

const INLINE_ICON_SIZE: Pixels = px(16.);

pub struct StyledRun {
    pub range: Range<usize>,
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
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
}

impl InlineContent {
    pub fn new(
        id: impl Into<ElementId>,
        text: SharedString,
        runs: Vec<StyledRun>,
        icons: Vec<IconOverlay>,
        clicks: Vec<ClickRegion>,
        body_color: Hsla,
    ) -> Self {
        let highlights = build_highlights(&text, runs, body_color);
        let styled = StyledText::new(text).with_highlights(highlights);
        Self {
            element_id: id.into(),
            styled,
            icons,
            clicks,
        }
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
            if let Ok(ix) = text_layout.index_for_position(mouse_position)
                && self.clicks.iter().any(|region| region.range.contains(&ix))
            {
                window.set_cursor_style(CursorStyle::PointingHand, hitbox);
            }

            let mouse_down = state.mouse_down_index.clone();
            if let Some(mouse_down_index) = mouse_down.get() {
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                let clicks = mem::take(&mut self.clicks);
                window.on_mouse_event(
                    move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                        if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                            if let Ok(mouse_up_index) =
                                text_layout.index_for_position(event.position)
                            {
                                for region in &clicks {
                                    if region.range.contains(&mouse_down_index)
                                        && region.range.contains(&mouse_up_index)
                                    {
                                        (region.action)(window, cx);
                                    }
                                }
                            }
                            mouse_down.take();
                            window.refresh();
                        }
                    },
                );
            } else {
                let hitbox = hitbox.clone();
                let text_layout = text_layout.clone();
                window.on_mouse_event(
                    move |event: &MouseDownEvent, phase, window: &mut Window, _: &mut App| {
                        if phase == DispatchPhase::Bubble
                            && hitbox.is_hovered(window)
                            && let Ok(mouse_down_index) =
                                text_layout.index_for_position(event.position)
                        {
                            mouse_down.set(Some(mouse_down_index));
                            window.refresh();
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
                let icon_size = if reserved < line_height {
                    reserved
                } else {
                    line_height
                };
                let vertical_offset = (line_height - icon_size) * 0.5;
                let icon_bounds = Bounds {
                    origin: point(icon_x, icon_y + vertical_offset),
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
