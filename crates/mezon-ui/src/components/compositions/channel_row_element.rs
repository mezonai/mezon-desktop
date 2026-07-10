use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, BorderStyle, Bounds, ContentMask, Corners, CursorStyle, DispatchPhase,
    Edges, Element, ElementId, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, SharedString, Style, TextAlign, TransformationMatrix, Window,
    fill, point, px, quad, size, transparent_black,
};

use crate::components::primitives::IconName;

const ROW_VERTICAL_PADDING: Pixels = px(8.);
const BG_HORIZONTAL_INSET: Pixels = px(8.);
const ICON_LEFT_INSET: Pixels = px(16.);
const ICON_SIZE: Pixels = px(16.);
const NAME_LEFT_INSET: Pixels = px(40.);
const NAME_RIGHT_RESERVE: Pixels = px(24.);
const NAME_FONT_SIZE: Pixels = px(16.);
const BG_CORNER_RADIUS: Pixels = px(8.);
const NUB_TOP: Pixels = px(12.);
const NUB_WIDTH: Pixels = px(4.);
const NUB_HEIGHT: Pixels = px(8.);
const NUB_CORNER_RADIUS: Pixels = px(4.);
const BADGE_HEIGHT: Pixels = px(16.);
const BADGE_WIDTH_NARROW: Pixels = px(16.);
const BADGE_WIDTH_WIDE: Pixels = px(22.);
const BADGE_RIGHT_GAP: Pixels = px(12.);
const BADGE_FONT_SIZE: Pixels = px(12.);
const FALLBACK_WIDTH: Pixels = px(240.);
const THREAD_ROW_HEIGHT: Pixels = px(34.);
const THREAD_CONNECTOR_X: Pixels = px(24.);
const THREAD_ELBOW_TOP: Pixels = px(12.);
const THREAD_ELBOW_SIZE: Pixels = px(10.);
const THREAD_ELBOW_RADIUS: Pixels = px(2.);
const THREAD_NAME_LEFT: Pixels = px(36.);

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type RightClickHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

pub struct ChannelRowBadge {
    pub count: u32,
    pub bg: Hsla,
    pub text_color: Hsla,
}

pub struct ThreadConnector {
    pub line_above: bool,
    pub line_below: bool,
    pub color: Hsla,
}

#[derive(Default)]
struct ChannelRowElementState {
    hovered: Rc<Cell<bool>>,
    mouse_down: Rc<Cell<bool>>,
}

pub struct ChannelRowElement {
    element_id: ElementId,
    icon: IconName,
    name: SharedString,
    icon_base: Hsla,
    icon_hover: Hsla,
    name_base: Hsla,
    name_hover: Hsla,
    name_weight: FontWeight,
    selected_bg: Option<Hsla>,
    hover_bg: Option<Hsla>,
    muted: bool,
    unread_nub: Option<Hsla>,
    badge: Option<ChannelRowBadge>,
    connector: Option<ThreadConnector>,
    on_click: Option<ClickHandler>,
    on_right_click: Option<RightClickHandler>,
}

impl ChannelRowElement {
    pub fn new(id: impl Into<ElementId>, icon: IconName, name: SharedString) -> Self {
        Self {
            element_id: id.into(),
            icon,
            name,
            icon_base: gpui::black(),
            icon_hover: gpui::black(),
            name_base: gpui::black(),
            name_hover: gpui::black(),
            name_weight: FontWeight::MEDIUM,
            selected_bg: None,
            hover_bg: None,
            muted: false,
            unread_nub: None,
            badge: None,
            connector: None,
            on_click: None,
            on_right_click: None,
        }
    }

    pub fn icon_color(mut self, base: Hsla, hover: Hsla) -> Self {
        self.icon_base = base;
        self.icon_hover = hover;
        self
    }

    pub fn name_style(mut self, base: Hsla, hover: Hsla, weight: FontWeight) -> Self {
        self.name_base = base;
        self.name_hover = hover;
        self.name_weight = weight;
        self
    }

    pub fn selected_bg(mut self, bg: Option<Hsla>) -> Self {
        self.selected_bg = bg;
        self
    }

    pub fn hover_bg(mut self, bg: Option<Hsla>) -> Self {
        self.hover_bg = bg;
        self
    }

    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    pub fn unread_nub(mut self, color: Option<Hsla>) -> Self {
        self.unread_nub = color;
        self
    }

    pub fn badge(mut self, badge: Option<ChannelRowBadge>) -> Self {
        self.badge = badge;
        self
    }

    pub fn connector(mut self, connector: Option<ThreadConnector>) -> Self {
        self.connector = connector;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
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

fn line_height(window: &Window) -> Pixels {
    let text_style = window.text_style();
    text_style
        .line_height
        .to_pixels(NAME_FONT_SIZE.into(), window.rem_size())
}

fn muted_color(mut color: Hsla, muted: bool) -> Hsla {
    if muted {
        color.a *= 0.7;
    }
    color
}

impl Element for ChannelRowElement {
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
        let is_thread = self.connector.is_some();
        let layout_id = window.request_measured_layout(
            Style::default(),
            move |known, available, window, _cx| {
                let width = known
                    .width
                    .or(match available.width {
                        AvailableSpace::Definite(w) => Some(w),
                        _ => None,
                    })
                    .unwrap_or(FALLBACK_WIDTH);
                let height = if is_thread {
                    THREAD_ROW_HEIGHT
                } else {
                    line_height(window) + ROW_VERTICAL_PADDING
                };
                size(width, height)
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
        _cx: &mut App,
    ) -> Hitbox {
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
        window.with_element_state::<ChannelRowElementState, _>(
            global_id.unwrap(),
            |state, window| {
                let state = state.unwrap_or_default();

                let hovered = hitbox.is_hovered(window);
                state.hovered.set(hovered);

                let row_height = bounds.size.height;
                let content_line_height = line_height(window);
                let left = bounds.origin.x;
                let top = bounds.origin.y;
                let width = bounds.size.width;

                let is_thread = self.connector.is_some();
                let background = self
                    .selected_bg
                    .or(if hovered { self.hover_bg } else { None });
                if let Some(background) = background {
                    let (bg_left, bg_width, bg_radius) = if is_thread {
                        (left, width, px(0.))
                    } else {
                        (
                            left + BG_HORIZONTAL_INSET,
                            width - BG_HORIZONTAL_INSET * 2.,
                            BG_CORNER_RADIUS,
                        )
                    };
                    let bg_bounds = Bounds {
                        origin: point(bg_left, top),
                        size: size(bg_width, row_height),
                    };
                    window.paint_quad(
                        fill(bg_bounds, background).corner_radii(Corners::all(bg_radius)),
                    );
                }

                if let Some(nub_color) = self.unread_nub {
                    let nub_bounds = Bounds {
                        origin: point(left, top + NUB_TOP),
                        size: size(NUB_WIDTH, NUB_HEIGHT),
                    };
                    window.paint_quad(fill(nub_bounds, nub_color).corner_radii(Corners {
                        top_left: px(0.),
                        top_right: NUB_CORNER_RADIUS,
                        bottom_right: NUB_CORNER_RADIUS,
                        bottom_left: px(0.),
                    }));
                }

                if let Some(conn) = &self.connector {
                    let line_x = left + THREAD_CONNECTOR_X;
                    if conn.line_above || conn.line_below {
                        let connector_height = if conn.line_below {
                            row_height
                        } else {
                            THREAD_ELBOW_TOP
                        };
                        window.paint_quad(fill(
                            Bounds {
                                origin: point(line_x, top),
                                size: size(px(1.), connector_height),
                            },
                            conn.color,
                        ));
                    }
                    let elbow_bounds = Bounds {
                        origin: point(line_x, top + THREAD_ELBOW_TOP),
                        size: size(THREAD_ELBOW_SIZE, THREAD_ELBOW_SIZE),
                    };
                    window.paint_quad(quad(
                        elbow_bounds,
                        Corners {
                            top_left: px(0.),
                            top_right: px(0.),
                            bottom_right: px(0.),
                            bottom_left: THREAD_ELBOW_RADIUS,
                        },
                        transparent_black(),
                        Edges {
                            top: px(0.),
                            right: px(0.),
                            bottom: px(1.),
                            left: px(1.),
                        },
                        conn.color,
                        BorderStyle::Solid,
                    ));
                } else {
                    let icon_color = muted_color(
                        if hovered {
                            self.icon_hover
                        } else {
                            self.icon_base
                        },
                        self.muted,
                    );
                    let icon_bounds = Bounds {
                        origin: point(left + ICON_LEFT_INSET, top + (row_height - ICON_SIZE) / 2.),
                        size: size(ICON_SIZE, ICON_SIZE),
                    };
                    let _ = window.paint_svg(
                        icon_bounds,
                        self.icon.path().into(),
                        None,
                        TransformationMatrix::default(),
                        icon_color,
                        cx,
                    );
                }

                let name_color = muted_color(
                    if hovered {
                        self.name_hover
                    } else {
                        self.name_base
                    },
                    self.muted,
                );
                let name_left = if is_thread {
                    THREAD_NAME_LEFT
                } else {
                    NAME_LEFT_INSET
                };
                let name_max_width = (width - name_left - NAME_RIGHT_RESERVE).max(px(0.));
                let text_system = window.text_system().clone();
                let mut name_run = window.text_style().to_run(self.name.len());
                name_run.color = name_color;
                name_run.font.weight = self.name_weight;
                let name_line =
                    text_system.shape_line(self.name.clone(), NAME_FONT_SIZE, &[name_run], None);
                let name_origin = point(
                    left + name_left,
                    top + (row_height - content_line_height) / 2.,
                );
                let name_clip = ContentMask {
                    bounds: Bounds {
                        origin: point(left + name_left, top),
                        size: size(name_max_width, row_height),
                    },
                };
                window.with_content_mask(Some(name_clip), |window| {
                    let _ = name_line.paint(
                        name_origin,
                        content_line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                });

                if !hovered && let Some(badge) = &self.badge {
                    let wide = badge.count >= 10;
                    let label = if badge.count >= 100 {
                        SharedString::from("99+")
                    } else {
                        SharedString::from(badge.count.to_string())
                    };
                    let badge_width = if wide {
                        BADGE_WIDTH_WIDE
                    } else {
                        BADGE_WIDTH_NARROW
                    };
                    let badge_x = left + width - BADGE_RIGHT_GAP - badge_width;
                    let badge_y = top + (row_height - BADGE_HEIGHT) / 2.;
                    let badge_bounds = Bounds {
                        origin: point(badge_x, badge_y),
                        size: size(badge_width, BADGE_HEIGHT),
                    };
                    window.paint_quad(
                        fill(badge_bounds, badge.bg).corner_radii(Corners::all(BADGE_HEIGHT / 2.)),
                    );
                    let mut badge_run = window.text_style().to_run(label.len());
                    badge_run.color = badge.text_color;
                    badge_run.font.weight = FontWeight::BOLD;
                    let badge_line =
                        text_system.shape_line(label, BADGE_FONT_SIZE, &[badge_run], None);
                    let badge_text_x = badge_x + (badge_width - badge_line.width()) / 2.;
                    let _ = badge_line.paint(
                        point(badge_text_x, badge_y),
                        BADGE_HEIGHT,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }

                if hovered {
                    window.set_cursor_style(CursorStyle::PointingHand, hitbox);
                }

                if self.on_click.is_some() || self.on_right_click.is_some() {
                    let hitbox_down = hitbox.clone();
                    let mouse_down = state.mouse_down.clone();
                    let on_right_click = self.on_right_click.clone();
                    window.on_mouse_event(
                        move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble || !hitbox_down.is_hovered(window) {
                                return;
                            }
                            match event.button {
                                MouseButton::Left => mouse_down.set(true),
                                MouseButton::Right => {
                                    if let Some(on_right_click) = on_right_click.as_ref() {
                                        on_right_click(event.position, window, cx);
                                    }
                                }
                                _ => {}
                            }
                        },
                    );

                    let hitbox_up = hitbox.clone();
                    let mouse_up = state.mouse_down.clone();
                    let on_click = self.on_click.clone();
                    window.on_mouse_event(
                        move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            if event.button == MouseButton::Left && mouse_up.get() {
                                mouse_up.set(false);
                                if hitbox_up.is_hovered(window)
                                    && let Some(on_click) = on_click.as_ref()
                                {
                                    on_click(window, cx);
                                }
                            }
                        },
                    );
                }

                let hitbox_move = hitbox.clone();
                let hovered_cell = state.hovered.clone();
                let view = window.current_view();
                window.on_mouse_event(
                    move |_: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let now = hitbox_move.is_hovered(window);
                        if now != hovered_cell.get() {
                            hovered_cell.set(now);
                            cx.notify(view);
                        }
                    },
                );

                ((), state)
            },
        );
    }
}

impl IntoElement for ChannelRowElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
