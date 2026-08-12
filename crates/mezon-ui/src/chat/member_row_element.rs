use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Corners, CursorStyle, DispatchPhase,
    Element, ElementId, Font, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, TransformationMatrix, TruncateFrom, Window, fill,
    point, px, radians, size,
};

use crate::components::primitives::IconName;

const ROW_HEIGHT: Pixels = px(48.);
const H_PADDING: Pixels = px(16.);
const AVATAR_SIZE: Pixels = px(32.);
const GAP: Pixels = px(9.);
const NAME_FONT_SIZE: Pixels = px(16.);
const STATUS_FONT_SIZE: Pixels = px(12.);
const OWNER_ICON_SIZE: Pixels = px(14.);
const OWNER_GAP: Pixels = px(4.);
const DOT_SIZE: Pixels = px(12.);
const DOT_BORDER: Pixels = px(2.);
const DOT_INNER: Pixels = px(8.);
const DOT_ICON_SIZE: Pixels = px(10.);
const STATUS_MAX_WIDTH: Pixels = px(150.);
const ELLIPSIS: &str = "…";
const STATUS_ICON_SIZE: Pixels = px(12.);
const STATUS_ICON_GAP: Pixels = px(4.);
const FALLBACK_WIDTH: Pixels = px(245.);

type ClickHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;
type RightClickHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
pub enum RowDot {
    None,
    Fill { color: Hsla, ring: Hsla },
    Icon { icon: IconName, color: Hsla },
}

pub struct MemberRowElement {
    element_id: ElementId,
    name: SharedString,
    avatar: Option<AnyElement>,
    name_color: Hsla,
    dot: RowDot,
    owner_icon: Option<Hsla>,
    status: Option<(SharedString, Hsla)>,
    status_icon: Option<(IconName, Hsla)>,
    on_click: Option<ClickHandler>,
    on_right_click: Option<RightClickHandler>,
}

impl MemberRowElement {
    pub fn new(id: impl Into<ElementId>, name: SharedString, avatar: AnyElement) -> Self {
        Self {
            element_id: id.into(),
            name,
            avatar: Some(avatar),
            name_color: gpui::black(),
            dot: RowDot::None,
            owner_icon: None,
            status: None,
            status_icon: None,
            on_click: None,
            on_right_click: None,
        }
    }

    pub fn name_color(mut self, color: Hsla) -> Self {
        self.name_color = color;
        self
    }

    pub fn dot(mut self, dot: RowDot) -> Self {
        self.dot = dot;
        self
    }

    pub fn owner_icon(mut self, color: Option<Hsla>) -> Self {
        self.owner_icon = color;
        self
    }

    pub fn status(mut self, status: Option<(SharedString, Hsla)>) -> Self {
        self.status = status;
        self
    }

    pub fn status_icon(mut self, icon: Option<(IconName, Hsla)>) -> Self {
        self.status_icon = icon;
        self
    }

    pub fn on_click(mut self, f: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn on_right_click(
        mut self,
        f: impl Fn(Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_right_click = Some(Rc::new(f));
        self
    }
}

fn line_height(window: &Window, font_size: Pixels) -> Pixels {
    let text_style = window.text_style();
    text_style
        .line_height
        .to_pixels(font_size.into(), window.rem_size())
}

struct TruncatedLine {
    source: SharedString,
    max_width: Pixels,
    font: Font,
    text: SharedString,
}

#[derive(Default)]
struct RowTextCache {
    name: Option<TruncatedLine>,
    status: Option<TruncatedLine>,
}

fn truncate_to_width(
    slot: &mut Option<TruncatedLine>,
    source: &SharedString,
    max_width: Pixels,
    font_size: Pixels,
    font: &Font,
    cx: &App,
) -> SharedString {
    if let Some(cached) = slot.as_ref()
        && cached.source == *source
        && cached.max_width == max_width
        && cached.font == *font
    {
        return cached.text.clone();
    }

    let text = if max_width <= px(0.) {
        SharedString::default()
    } else {
        let run = TextRun {
            len: source.len(),
            font: font.clone(),
            ..Default::default()
        };
        cx.text_system()
            .line_wrapper(font.clone(), font_size)
            .truncate_line(
                source.clone(),
                max_width,
                ELLIPSIS,
                std::slice::from_ref(&run),
                TruncateFrom::End,
            )
            .0
    };

    *slot = Some(TruncatedLine {
        source: source.clone(),
        max_width,
        font: font.clone(),
        text: text.clone(),
    });
    text
}

impl Element for MemberRowElement {
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
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = window.request_measured_layout(
            Style::default(),
            move |known, available, _window, _cx| {
                let width = known
                    .width
                    .or(match available.width {
                        AvailableSpace::Definite(w) => Some(w),
                        _ => None,
                    })
                    .unwrap_or(FALLBACK_WIDTH);
                size(width, ROW_HEIGHT)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let avatar_origin = point(
            bounds.origin.x + H_PADDING,
            bounds.origin.y + (ROW_HEIGHT - AVATAR_SIZE) / 2.,
        );
        if let Some(avatar) = &mut self.avatar {
            avatar.prepaint_as_root(
                avatar_origin,
                size(
                    AvailableSpace::Definite(AVATAR_SIZE),
                    AvailableSpace::Definite(AVATAR_SIZE),
                ),
                window,
                cx,
            );
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let left = bounds.origin.x;
        let top = bounds.origin.y;

        window.set_cursor_style(CursorStyle::PointingHand, hitbox);

        if let Some(handler) = self.on_click.clone() {
            let hitbox_down = hitbox.clone();
            let avatar_anchor = bounds.origin;
            window.on_mouse_event(
                move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase == DispatchPhase::Bubble
                        && hitbox_down.is_hovered(window)
                        && event.button == MouseButton::Left
                    {
                        handler(avatar_anchor, window, cx);
                    }
                },
            );
        }

        if let Some(handler) = self.on_right_click.clone() {
            let hitbox_right = hitbox.clone();
            window.on_mouse_event(
                move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase == DispatchPhase::Bubble
                        && hitbox_right.is_hovered(window)
                        && event.button == MouseButton::Right
                    {
                        handler(event.position, window, cx);
                    }
                },
            );
        }

        if let Some(avatar) = &mut self.avatar {
            avatar.paint(window, cx);
        }

        let dot_x = left + H_PADDING + AVATAR_SIZE - DOT_SIZE + px(1.);
        let dot_y = top + (ROW_HEIGHT - AVATAR_SIZE) / 2. + AVATAR_SIZE - DOT_SIZE + px(1.);
        let dot_outer = Bounds {
            origin: point(dot_x, dot_y),
            size: size(DOT_SIZE, DOT_SIZE),
        };
        match self.dot {
            RowDot::None => {}
            RowDot::Fill { color, ring } => {
                window.paint_quad(fill(dot_outer, ring).corner_radii(Corners::all(DOT_SIZE / 2.)));
                let dot_inner = Bounds {
                    origin: point(dot_x + DOT_BORDER, dot_y + DOT_BORDER),
                    size: size(DOT_INNER, DOT_INNER),
                };
                window
                    .paint_quad(fill(dot_inner, color).corner_radii(Corners::all(DOT_INNER / 2.)));
            }
            RowDot::Icon { icon, color } => {
                let inset = (DOT_SIZE - DOT_ICON_SIZE) / 2.;
                let icon_bounds = Bounds {
                    origin: point(dot_x + inset, dot_y + inset),
                    size: size(DOT_ICON_SIZE, DOT_ICON_SIZE),
                };
                let scale = window.scale_factor();
                let center = icon_bounds.center();
                let transform = TransformationMatrix::unit()
                    .translate(center.scale(scale))
                    .rotate(radians(-std::f32::consts::FRAC_PI_2))
                    .translate(center.scale(-scale));
                let _ =
                    window.paint_svg(icon_bounds, icon.path().into(), None, transform, color, cx);
            }
        }

        match global_id {
            Some(global_id) => {
                window.with_element_state::<RowTextCache, _>(global_id, |state, window| {
                    let mut cache = state.unwrap_or_default();
                    self.paint_text(&mut cache, bounds, window, cx);
                    ((), cache)
                });
            }
            None => self.paint_text(&mut RowTextCache::default(), bounds, window, cx),
        }
    }
}

impl MemberRowElement {
    fn paint_text(
        &mut self,
        cache: &mut RowTextCache,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let left = bounds.origin.x;
        let top = bounds.origin.y;
        let width = bounds.size.width;
        let text_x = H_PADDING + AVATAR_SIZE + GAP;

        let name_line_height = line_height(window, NAME_FONT_SIZE);
        let status_line_height = line_height(window, STATUS_FONT_SIZE);
        let (name_y, status_y) = if self.status.is_some() {
            let total = name_line_height + status_line_height;
            let block_top = top + (ROW_HEIGHT - total) / 2.;
            (block_top, block_top + name_line_height)
        } else {
            (top + (ROW_HEIGHT - name_line_height) / 2., top)
        };

        let text_system = window.text_system().clone();

        let mut name_max_width = (width - text_x - H_PADDING).max(px(0.));
        if self.owner_icon.is_some() {
            name_max_width = (name_max_width - OWNER_GAP - OWNER_ICON_SIZE).max(px(0.));
        }
        let mut name_run = window.text_style().to_run(self.name.len());
        name_run.color = self.name_color;
        name_run.font.weight = FontWeight::MEDIUM;
        let name_text = truncate_to_width(
            &mut cache.name,
            &self.name,
            name_max_width,
            NAME_FONT_SIZE,
            &name_run.font,
            cx,
        );
        name_run.len = name_text.len();
        let name_line = text_system.shape_line(
            name_text,
            NAME_FONT_SIZE,
            std::slice::from_ref(&name_run),
            None,
        );
        let name_clip = ContentMask {
            bounds: Bounds {
                origin: point(left + text_x, top),
                size: size(name_max_width, ROW_HEIGHT),
            },
        };
        window.with_content_mask(Some(name_clip), |window| {
            let _ = name_line.paint(
                point(left + text_x, name_y),
                name_line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        });

        if let Some(color) = self.owner_icon {
            let owner_bounds = Bounds {
                origin: point(
                    left + text_x + name_line.width() + OWNER_GAP,
                    name_y + (name_line_height - OWNER_ICON_SIZE) / 2.,
                ),
                size: size(OWNER_ICON_SIZE, OWNER_ICON_SIZE),
            };
            let _ = window.paint_svg(
                owner_bounds,
                IconName::OwnerIcon.path().into(),
                None,
                TransformationMatrix::default(),
                color,
                cx,
            );
        }

        if let Some((text, color)) = self.status.clone() {
            let mut status_text_x = left + text_x;
            if let Some((icon, icon_color)) = self.status_icon {
                let icon_bounds = Bounds {
                    origin: point(
                        status_text_x,
                        status_y + (status_line_height - STATUS_ICON_SIZE) / 2.,
                    ),
                    size: size(STATUS_ICON_SIZE, STATUS_ICON_SIZE),
                };
                let _ = window.paint_svg(
                    icon_bounds,
                    icon.path().into(),
                    None,
                    TransformationMatrix::default(),
                    icon_color,
                    cx,
                );
                status_text_x += STATUS_ICON_SIZE + STATUS_ICON_GAP;
            }
            let status_area_width = STATUS_MAX_WIDTH.min((width - text_x - H_PADDING).max(px(0.)));
            let status_text_max_width = if self.status_icon.is_some() {
                (status_area_width - STATUS_ICON_SIZE - STATUS_ICON_GAP).max(px(0.))
            } else {
                status_area_width
            };
            let mut status_run = window.text_style().to_run(text.len());
            status_run.color = color;
            let status_text = truncate_to_width(
                &mut cache.status,
                &text,
                status_text_max_width,
                STATUS_FONT_SIZE,
                &status_run.font,
                cx,
            );
            status_run.len = status_text.len();
            let status_line = text_system.shape_line(
                status_text,
                STATUS_FONT_SIZE,
                std::slice::from_ref(&status_run),
                None,
            );
            let status_clip = ContentMask {
                bounds: Bounds {
                    origin: point(left + text_x, top),
                    size: size(status_area_width, ROW_HEIGHT),
                },
            };
            window.with_content_mask(Some(status_clip), |window| {
                let _ = status_line.paint(
                    point(status_text_x, status_y),
                    status_line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            });
        }
    }
}

impl IntoElement for MemberRowElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Font, FontWeight, SharedString, TestAppContext, font, px};

    use super::{ELLIPSIS, NAME_FONT_SIZE, TruncatedLine, truncate_to_width};

    const LONG_NAME: &str = "phuong.nguyenhonghang.wider";

    fn regular() -> Font {
        font("Helvetica")
    }

    fn medium() -> Font {
        Font {
            weight: FontWeight::MEDIUM,
            ..font("Helvetica")
        }
    }

    fn truncate(
        slot: &mut Option<TruncatedLine>,
        max_width: gpui::Pixels,
        f: &Font,
        cx: &mut TestAppContext,
    ) -> SharedString {
        let source: SharedString = LONG_NAME.into();
        cx.update(|cx| truncate_to_width(slot, &source, max_width, NAME_FONT_SIZE, f, cx))
    }

    #[gpui::test]
    fn a_generous_budget_leaves_the_text_untouched(cx: &mut TestAppContext) {
        let mut slot = None;
        assert_eq!(truncate(&mut slot, px(4000.), &regular(), cx), LONG_NAME);
    }

    #[gpui::test]
    fn a_tight_budget_appends_the_ellipsis(cx: &mut TestAppContext) {
        let mut slot = None;
        let text = truncate(&mut slot, px(30.), &regular(), cx);
        assert!(text.ends_with(ELLIPSIS));
        assert!(text.len() < LONG_NAME.len());
    }

    #[gpui::test]
    fn a_repeat_call_is_served_from_the_cache(cx: &mut TestAppContext) {
        let mut slot = None;
        let first = truncate(&mut slot, px(30.), &regular(), cx);
        assert_eq!(slot.as_ref().unwrap().max_width, px(30.));
        assert_eq!(truncate(&mut slot, px(30.), &regular(), cx), first);
    }

    #[gpui::test]
    fn a_different_width_or_weight_invalidates_the_cache(cx: &mut TestAppContext) {
        let mut slot = None;
        truncate(&mut slot, px(30.), &regular(), cx);

        truncate(&mut slot, px(4000.), &regular(), cx);
        assert_eq!(slot.as_ref().unwrap().max_width, px(4000.));

        truncate(&mut slot, px(4000.), &medium(), cx);
        assert_eq!(slot.as_ref().unwrap().font, medium());
    }
}
