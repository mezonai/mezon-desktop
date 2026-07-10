use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Corners, CursorStyle, DispatchPhase,
    Element, ElementId, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString, Style, TextAlign, TransformationMatrix, Window, fill, point, px, size,
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
const STATUS_MAX_WIDTH: Pixels = px(100.);
const FALLBACK_WIDTH: Pixels = px(245.);

type ClickHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;
type RightClickHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

pub struct MemberRowElement {
    element_id: ElementId,
    name: SharedString,
    avatar: Option<AnyElement>,
    name_color: Hsla,
    dot_fill: Hsla,
    dot_border: Hsla,
    owner_icon: Option<Hsla>,
    status: Option<(SharedString, Hsla)>,
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
            dot_fill: gpui::black(),
            dot_border: gpui::black(),
            owner_icon: None,
            status: None,
            on_click: None,
            on_right_click: None,
        }
    }

    pub fn name_color(mut self, color: Hsla) -> Self {
        self.name_color = color;
        self
    }

    pub fn dot(mut self, fill: Hsla, border: Hsla) -> Self {
        self.dot_fill = fill;
        self.dot_border = border;
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
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let left = bounds.origin.x;
        let top = bounds.origin.y;
        let width = bounds.size.width;
        let text_x = H_PADDING + AVATAR_SIZE + GAP;

        window.set_cursor_style(CursorStyle::PointingHand, hitbox);

        if let Some(handler) = self.on_click.clone() {
            let hitbox_down = hitbox.clone();
            window.on_mouse_event(
                move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase == DispatchPhase::Bubble
                        && hitbox_down.is_hovered(window)
                        && event.button == MouseButton::Left
                    {
                        handler(event.position, window, cx);
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
        window
            .paint_quad(fill(dot_outer, self.dot_border).corner_radii(Corners::all(DOT_SIZE / 2.)));
        let dot_inner = Bounds {
            origin: point(dot_x + DOT_BORDER, dot_y + DOT_BORDER),
            size: size(DOT_INNER, DOT_INNER),
        };
        window
            .paint_quad(fill(dot_inner, self.dot_fill).corner_radii(Corners::all(DOT_INNER / 2.)));

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

        let mut name_run = window.text_style().to_run(self.name.len());
        name_run.color = self.name_color;
        name_run.font.weight = FontWeight::MEDIUM;
        let name_line =
            text_system.shape_line(self.name.clone(), NAME_FONT_SIZE, &[name_run], None);
        let name_clip = ContentMask {
            bounds: Bounds {
                origin: point(left + text_x, top),
                size: size((width - text_x - H_PADDING).max(px(0.)), ROW_HEIGHT),
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
            let mut status_run = window.text_style().to_run(text.len());
            status_run.color = color;
            let status_line = text_system.shape_line(text, STATUS_FONT_SIZE, &[status_run], None);
            let status_clip = ContentMask {
                bounds: Bounds {
                    origin: point(left + text_x, top),
                    size: size(STATUS_MAX_WIDTH, ROW_HEIGHT),
                },
            };
            window.with_content_mask(Some(status_clip), |window| {
                let _ = status_line.paint(
                    point(left + text_x, status_y),
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
