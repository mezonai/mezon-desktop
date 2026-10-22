use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Context, CursorStyle, Div, Entity, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, PathBuilder, Pixels, Point, Rgba, SharedString,
    Stateful, Subscription, TextAlign, WeakEntity, Window, canvas, div, fill, outline, point,
    prelude::*, px, size,
};
use mezon_store::{
    ContentViewport, DesktopDraw, DrawAnnotation, DrawTool, NormalizedPoint, PRESET_COLORS,
    PRESET_WIDTHS, STROKE_WIDTH_MEDIUM, STROKE_WIDTH_THIN, ScreenDrawState, ShapeAnnotation,
    ShapeKind, TEXT_HANDLE_SIZE, TEXT_LINE_HEIGHT, TextAnnotation, VoiceStore, scale_alpha,
    text_font_px,
};
use mezon_theme::Theme;
use ui::Tooltip;

use crate::components::primitives::{Icon, IconName, Input, InputEvent, InputState, h_flex};

const ACCENT_BLUE: u32 = 0x5865f2;
const CLOSE_RED: u32 = 0xef4444;
const ELLIPSE_SEGMENTS: usize = 64;

const DESKTOP_TOOLS: &[DrawTool] = &[
    DrawTool::Pointer,
    DrawTool::Pen,
    DrawTool::Text,
    DrawTool::Eraser,
];

const IN_APP_TOOLS: &[DrawTool] = &[
    DrawTool::Pointer,
    DrawTool::Pen,
    DrawTool::Text,
    DrawTool::Eraser,
];

#[derive(Clone)]
pub(crate) enum DrawTarget {
    InApp(Entity<VoiceStore>),
    Desktop {
        voice: Entity<VoiceStore>,
        draw: Entity<DesktopDraw>,
    },
}

impl DrawTarget {
    pub(crate) fn state<'a>(&self, cx: &'a App) -> &'a ScreenDrawState {
        match self {
            Self::InApp(voice) => voice.read(cx).screen_draw(),
            Self::Desktop { draw, .. } => draw.read(cx).state(),
        }
    }

    pub(crate) fn edit<R>(&self, cx: &mut App, f: impl FnOnce(&mut ScreenDrawState) -> R) -> R {
        match self {
            Self::InApp(voice) => voice.update(cx, |store, cx| {
                let result = f(store.screen_draw_mut());
                cx.notify();
                result
            }),
            Self::Desktop { draw, .. } => draw.update(cx, |draw, cx| draw.edit(cx, f)),
        }
    }

    pub(crate) fn close(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::InApp(voice) => {
                voice.update(cx, |store, cx| store.set_screen_draw_active(false, cx))
            }
            Self::Desktop { voice, draw } => {
                voice.update(cx, |store, cx| store.end_desktop_draw_for(draw, cx));
                window.remove_window();
            }
        }
    }

    fn is_desktop(&self) -> bool {
        matches!(self, Self::Desktop { .. })
    }
}

pub(crate) fn cursor_for_tool(tool: DrawTool) -> CursorStyle {
    match tool {
        DrawTool::Pointer => CursorStyle::Arrow,
        DrawTool::Text => CursorStyle::IBeam,
        _ => CursorStyle::Crosshair,
    }
}

fn tool_icon(tool: DrawTool) -> IconName {
    match tool {
        DrawTool::Pointer => IconName::AnnotatePointer,
        DrawTool::Pen => IconName::PenEdit,
        DrawTool::Highlighter => IconName::AnnotateHighlighter,
        DrawTool::Laser => IconName::AnnotateLaser,
        DrawTool::Arrow => IconName::AnnotateArrow,
        DrawTool::Rectangle => IconName::AnnotateRectangle,
        DrawTool::Ellipse => IconName::AnnotateEllipse,
        DrawTool::Text => IconName::TextTool,
        DrawTool::Eraser => IconName::Eraser,
    }
}

fn tool_label(tool: DrawTool) -> &'static str {
    match tool {
        DrawTool::Pointer => "Select and move",
        DrawTool::Pen => "Pen",
        DrawTool::Highlighter => "Highlighter",
        DrawTool::Laser => "Vanishing pen",
        DrawTool::Arrow => "Arrow",
        DrawTool::Rectangle => "Rectangle",
        DrawTool::Ellipse => "Ellipse",
        DrawTool::Text => "Text",
        DrawTool::Eraser => "Eraser",
    }
}

fn tool_group(tool: DrawTool) -> u8 {
    match tool {
        DrawTool::Pointer => 0,
        DrawTool::Pen | DrawTool::Highlighter | DrawTool::Laser => 1,
        DrawTool::Arrow | DrawTool::Rectangle | DrawTool::Ellipse => 2,
        DrawTool::Text | DrawTool::Eraser => 3,
    }
}

pub(crate) fn render_draw_toolbar(
    target: &DrawTarget,
    leading: Option<AnyElement>,
    cx: &App,
) -> Stateful<Div> {
    let state = target.state(cx);
    let active_tool = state.active_tool;
    let stroke_color = state.stroke_color;
    let stroke_width = state.stroke_width;
    let can_undo = state.can_undo();
    let can_redo = state.can_redo();
    let can_clear = state.has_content();
    let tooltips = !target.is_desktop();
    let tools = if target.is_desktop() {
        DESKTOP_TOOLS
    } else {
        IN_APP_TOOLS
    };

    let mut bar = h_flex()
        .id("screen-draw-toolbox")
        .items_center()
        .gap_1()
        .px_2()
        .py(px(6.))
        .rounded_xl()
        .bg(gpui::rgba(0x18181bf2))
        .border_1()
        .border_color(gpui::rgba(0xffffff26))
        .shadow_xl();

    if let Some(leading) = leading {
        bar = bar.child(leading);
    }

    let mut previous_group = None;
    for &tool in tools {
        let group = tool_group(tool);
        if previous_group.is_some_and(|previous| previous != group) {
            bar = bar.child(vertical_divider());
        }
        previous_group = Some(group);
        bar = bar.child(tool_button(target, tool, active_tool == tool, tooltips));
    }

    let undo = {
        let target = target.clone();
        action_button(
            "draw-action-undo",
            IconName::RotateLeftIcon,
            "Undo",
            can_undo,
            tooltips,
            move |_, _, cx| {
                target.edit(cx, |state| state.undo());
            },
        )
    };
    let redo = {
        let target = target.clone();
        action_button(
            "draw-action-redo",
            IconName::RotateRightIcon,
            "Redo",
            can_redo,
            tooltips,
            move |_, _, cx| {
                target.edit(cx, |state| state.redo());
            },
        )
    };
    let clear = {
        let target = target.clone();
        action_button(
            "draw-action-clear",
            IconName::TrashIcon,
            "Clear all drawings",
            can_clear,
            tooltips,
            move |_, _, cx| {
                target.edit(cx, |state| state.clear());
            },
        )
    };
    let close = {
        let target = target.clone();
        close_button(
            if target.is_desktop() {
                "Stop annotating"
            } else {
                "Close drawing"
            },
            tooltips,
            move |_, window, cx| target.close(window, cx),
        )
    };

    bar.child(vertical_divider())
        .child(color_row(target, stroke_color))
        .child(vertical_divider())
        .child(width_row(
            target,
            stroke_color,
            stroke_width,
            active_tool.uses_width(),
        ))
        .child(vertical_divider())
        .child(undo)
        .child(redo)
        .child(clear)
        .child(vertical_divider())
        .child(close)
}

pub(crate) fn render_draw_toolbox(
    voice: &Entity<VoiceStore>,
    _theme: &Theme,
    cx: &App,
) -> AnyElement {
    let target = DrawTarget::InApp(voice.clone());
    div()
        .id("screen-draw-toolbox-container")
        .absolute()
        .top(px(16.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            render_draw_toolbar(&target, None, cx)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation()),
        )
        .into_any_element()
}

fn color_row(target: &DrawTarget, stroke_color: u32) -> AnyElement {
    h_flex()
        .items_center()
        .gap_1()
        .children(PRESET_COLORS.iter().map(|&color| {
            let target = target.clone();
            let is_active = stroke_color == color;
            div()
                .id(SharedString::from(format!("draw-color-{color:x}")))
                .size(px(18.))
                .rounded_full()
                .bg(gpui::rgba(color))
                .border_1()
                .border_color(gpui::rgba(0xffffff40))
                .cursor_pointer()
                .when(is_active, |s| {
                    s.border_2().border_color(gpui::rgb(0xffffff)).shadow_sm()
                })
                .hover(|s| s.opacity(0.85))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    target.edit(cx, |state| state.set_color(color));
                })
        }))
        .into_any_element()
}

fn width_row(
    target: &DrawTarget,
    stroke_color: u32,
    stroke_width: f32,
    enabled: bool,
) -> AnyElement {
    h_flex()
        .items_center()
        .gap(px(2.))
        .opacity(if enabled { 1.0 } else { 0.35 })
        .children(PRESET_WIDTHS.iter().map(|&w| {
            let target = target.clone();
            let is_active = (stroke_width - w).abs() < 0.5;
            let dot_size = if (w - STROKE_WIDTH_THIN).abs() < 0.5 {
                px(4.)
            } else if (w - STROKE_WIDTH_MEDIUM).abs() < 0.5 {
                px(7.)
            } else {
                px(10.)
            };
            div()
                .id(SharedString::from(format!("draw-width-{w:.0}")))
                .size(px(24.))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .when(enabled, |s| {
                    s.cursor_pointer().hover(|s| s.bg(gpui::rgba(0xffffff20)))
                })
                .when(is_active, |s| s.bg(gpui::rgba(0xffffff30)))
                .child(
                    div()
                        .size(dot_size)
                        .rounded_full()
                        .bg(gpui::rgba(stroke_color)),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    if enabled {
                        target.edit(cx, |state| state.set_width(w));
                    }
                })
        }))
        .into_any_element()
}

fn vertical_divider() -> AnyElement {
    div()
        .w(px(1.))
        .h(px(18.))
        .mx(px(2.))
        .bg(gpui::rgba(0xffffff26))
        .into_any_element()
}

fn tool_button(target: &DrawTarget, tool: DrawTool, selected: bool, tooltip: bool) -> AnyElement {
    let target = target.clone();
    div()
        .id(SharedString::from(format!("draw-tool-{tool:?}")))
        .size(px(30.))
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(selected, |s| {
            s.bg(gpui::rgba(0x5865f259))
                .border_1()
                .border_color(gpui::rgb(ACCENT_BLUE))
        })
        .when(!selected, |s| s.hover(|h| h.bg(gpui::rgba(0xffffff1a))))
        .when(tooltip, |s| s.tooltip(Tooltip::text(tool_label(tool))))
        .child(
            Icon::new(tool_icon(tool))
                .size(px(18.))
                .text_color(if selected {
                    gpui::rgb(0xffffff)
                } else {
                    gpui::rgba(0xffffffb3)
                }),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            target.edit(cx, |state| state.set_tool(tool));
        })
        .into_any_element()
}

fn action_button(
    id: &'static str,
    icon: IconName,
    tooltip_text: &'static str,
    enabled: bool,
    tooltip: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .size(px(30.))
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .opacity(if enabled { 1.0 } else { 0.35 })
        .when(enabled, |s| s.hover(|h| h.bg(gpui::rgba(0xffffff1a))))
        .when(tooltip, |s| s.tooltip(Tooltip::text(tooltip_text)))
        .child(
            Icon::new(icon)
                .size(px(18.))
                .text_color(gpui::rgba(0xffffffb3)),
        )
        .on_click(move |e, window, cx| {
            cx.stop_propagation();
            if enabled {
                on_click(e, window, cx);
            }
        })
        .into_any_element()
}

fn close_button(
    tooltip_text: &'static str,
    tooltip: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id("draw-action-close")
        .size(px(30.))
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(gpui::rgba(0xef444433))
        .hover(|h| h.bg(gpui::rgb(CLOSE_RED)))
        .when(tooltip, |s| s.tooltip(Tooltip::text(tooltip_text)))
        .child(
            Icon::new(IconName::Close)
                .size(px(16.))
                .text_color(gpui::rgb(0xffffff)),
        )
        .on_click(move |e, window, cx| {
            cx.stop_propagation();
            on_click(e, window, cx);
        })
        .into_any_element()
}

pub(crate) fn render_screen_draw_overlay(
    voice: &Entity<VoiceStore>,
    _theme: &Theme,
    frame_size: Option<(u32, u32)>,
    cx: &App,
) -> AnyElement {
    let draw_state = voice.read(cx).screen_draw().clone();
    let is_active = draw_state.is_active;
    let cursor = cursor_for_tool(draw_state.active_tool);

    let bounds_ref = Arc::new(Mutex::new(None::<Bounds<Pixels>>));
    let bounds_for_canvas = bounds_ref.clone();
    let bounds_for_down = bounds_ref.clone();
    let bounds_for_move = bounds_ref;

    let (content_w, content_h) = frame_size.unwrap_or((1920, 1080));
    let vp_w = content_w as f32;
    let vp_h = content_h as f32;
    let viewport_for = move |bounds: Bounds<Pixels>| {
        ContentViewport::compute(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            vp_w,
            vp_h,
        )
    };
    let normalize = move |bounds: Bounds<Pixels>, position: Point<Pixels>| {
        let vp = viewport_for(bounds);
        let point = vp.to_normalized(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        );
        (point, (vp.width, vp.height))
    };

    let draw_state_for_canvas = draw_state.clone();
    let draw_canvas = canvas(
        move |bounds, _, _| {
            if let Ok(mut g) = bounds_for_canvas.lock() {
                *g = Some(bounds);
            }
            bounds
        },
        move |bounds, _, window, cx| {
            paint_draw_state(
                window,
                cx,
                &draw_state_for_canvas,
                &viewport_for(bounds),
                bounds.origin,
                Instant::now(),
            );
        },
    )
    .absolute()
    .size_full();

    let mut overlay = div()
        .id("screen-draw-overlay")
        .absolute()
        .inset_0()
        .child(draw_canvas);

    if is_active {
        let voice_down = voice.clone();
        let voice_move = voice.clone();
        let voice_up = voice.clone();

        overlay = overlay
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                cx.stop_propagation();
                let Some(bounds) = bounds_for_down.lock().ok().and_then(|g| *g) else {
                    return;
                };
                let (point, canvas) = normalize(bounds, e.position);
                let before = voice_down.read(cx).screen_draw().editing_text_id;
                let measure = |text: &TextAnnotation| text_width(window, text);
                voice_down.update(cx, |store, cx| {
                    if store
                        .screen_draw_mut()
                        .pointer_down(point, canvas, &measure)
                    {
                        cx.notify();
                    }
                });
                if open_in_app_editor(&voice_down, before, window, cx) {
                    window.prevent_default();
                }
            })
            .on_mouse_move(move |e, _, cx| {
                let Some(bounds) = bounds_for_move.lock().ok().and_then(|g| *g) else {
                    return;
                };
                let (point, _) = normalize(bounds, e.position);
                let pressed = e.pressed_button == Some(MouseButton::Left);
                voice_move.update(cx, |store, cx| {
                    if store.screen_draw_mut().pointer_move(point, pressed) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                let before = voice_up.read(cx).screen_draw().editing_text_id;
                let released = voice_up.update(cx, |store, cx| {
                    let released = store.screen_draw_mut().pointer_up();
                    if released {
                        cx.notify();
                    }
                    released
                });
                if released {
                    open_in_app_editor(&voice_up, before, window, cx);
                }
            });

        if let (Some(text), Some(editor)) = (
            draw_state.editing_text(),
            voice.read(cx).screen_draw_editor(),
        ) {
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(gpui::relative(text.position.x))
                    .top(gpui::relative(text.position.y))
                    .child(editor),
            );
        }
    }

    overlay.into_any_element()
}

fn open_in_app_editor(
    voice: &Entity<VoiceStore>,
    before: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let started = {
        let state = voice.read(cx).screen_draw();
        state
            .editing_text()
            .filter(|text| Some(text.id) != before)
            .map(|text| (text.clone(), state.editing_text_content.clone()))
    };
    if let Some((text, content)) = started {
        let editor_voice = voice.clone();
        let editor = cx.new(|cx| DrawTextEditor::new(editor_voice, &text, content, window, cx));
        voice.update(cx, |store, _| {
            store.set_screen_draw_editor(text.id, editor.into())
        });
    }
    voice.read(cx).screen_draw().editing_text_id.is_some()
}

pub(crate) struct DrawTextEditor {
    voice: WeakEntity<VoiceStore>,
    input: Entity<InputState>,
    color: u32,
    _subscription: Subscription,
}

impl DrawTextEditor {
    fn new(
        voice: Entity<VoiceStore>,
        text: &TextAnnotation,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder("Type a note, Enter to place")
                .borderless()
                .text_size(px(14.))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::rgba(text.color));
            if !content.is_empty() {
                input.set_value(content, window, cx);
            }
            input
        });
        let subscription =
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().to_string();
                    this.edit(cx, |state| state.set_editing_text_content(value));
                }
                InputEvent::PressEnter => this.edit(cx, |state| state.commit_editing_text()),
            });
        input.update(cx, |input, cx| input.focus(window, cx));
        Self {
            voice: voice.downgrade(),
            input,
            color: text.color,
            _subscription: subscription,
        }
    }

    fn edit(&self, cx: &mut App, f: impl FnOnce(&mut ScreenDrawState)) {
        let _ = self.voice.update(cx, |store, cx| {
            f(store.screen_draw_mut());
            cx.notify();
        });
    }
}

impl Render for DrawTextEditor {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let voice_commit = self.voice.clone();
        let voice_cancel = self.voice.clone();
        let voice_escape = self.voice.clone();

        h_flex()
            .id("screen-draw-text-editor-card")
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(gpui::rgba(0x18181bf5))
            .border_2()
            .border_color(gpui::rgba(self.color))
            .shadow_xl()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                cx.stop_propagation();
                let _ = voice_escape.update(cx, |store, cx| {
                    store.screen_draw_mut().cancel_editing_text();
                    cx.notify();
                });
            })
            .child(
                div()
                    .min_w(px(180.))
                    .max_w(px(320.))
                    .child(Input::new(&self.input)),
            )
            .child(
                div()
                    .id("screen-draw-text-commit")
                    .size(px(24.))
                    .rounded_md()
                    .bg(gpui::rgb(0x10b981))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .tooltip(Tooltip::text("Commit (Enter)"))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::rgb(0xffffff))
                            .child("✓"),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = voice_commit.update(cx, |store, cx| {
                            store.screen_draw_mut().commit_editing_text();
                            cx.notify();
                        });
                    }),
            )
            .child(
                div()
                    .id("screen-draw-text-cancel")
                    .size(px(24.))
                    .rounded_md()
                    .bg(gpui::rgb(CLOSE_RED))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .tooltip(Tooltip::text("Cancel (Esc)"))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::rgb(0xffffff))
                            .child("✕"),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = voice_cancel.update(cx, |store, cx| {
                            store.screen_draw_mut().cancel_editing_text();
                            cx.notify();
                        });
                    }),
            )
    }
}

pub(crate) fn paint_draw_state(
    window: &mut Window,
    cx: &mut App,
    state: &ScreenDrawState,
    vp: &ContentViewport,
    origin: Point<Pixels>,
    now: Instant,
) {
    for ann in &state.annotations {
        match ann {
            DrawAnnotation::Stroke(s) => {
                let points = screen_points(vp, origin, &s.points);
                paint_polyline(window, &points, s.render_color(), s.render_width());
            }
            DrawAnnotation::Shape(s) => paint_shape(window, vp, origin, s),
            DrawAnnotation::Text(t) => {
                if state.editing_text_id != Some(t.id) {
                    paint_text_annotation(window, cx, vp, origin, t);
                }
            }
        }
    }
    for vanishing in &state.vanishing {
        let points = screen_points(vp, origin, &vanishing.stroke.points);
        let color = scale_alpha(vanishing.stroke.render_color(), vanishing.alpha(now));
        paint_polyline(window, &points, color, vanishing.stroke.render_width());
    }
    if let Some(s) = &state.current_stroke {
        let points = screen_points(vp, origin, &s.points);
        paint_polyline(window, &points, s.render_color(), s.render_width());
    }
    if let Some(s) = &state.current_shape {
        paint_shape(window, vp, origin, s);
    }
    if matches!(state.active_tool, DrawTool::Text | DrawTool::Pointer)
        && let Some(selected) = state.selected_annotation()
        && state.editing_text_id != Some(selected.id())
    {
        paint_selection(window, state, vp, origin, selected);
    }
}

pub(crate) fn text_width(window: &Window, text: &TextAnnotation) -> f32 {
    if text.content.is_empty() {
        return 0.0;
    }
    f32::from(shape_text(window, text).width())
}

fn shape_text(window: &Window, text: &TextAnnotation) -> gpui::ShapedLine {
    let mut run = window.text_style().to_run(text.content.len());
    run.color = gpui::rgba(text.color).into();
    run.font.weight = FontWeight::BOLD;
    window.text_system().shape_line(
        SharedString::from(text.content.clone()),
        px(text_font_px(text.font_size)),
        std::slice::from_ref(&run),
        None,
    )
}

fn paint_selection(
    window: &mut Window,
    state: &ScreenDrawState,
    vp: &ContentViewport,
    origin: Point<Pixels>,
    selected: &DrawAnnotation,
) {
    let canvas = (vp.width, vp.height);
    let measure = |text: &TextAnnotation| text_width(window, text);
    let Some(frame) = state.annotation_bounds(selected, canvas, &measure) else {
        return;
    };
    let left = origin.x + px(vp.offset_x + frame.left);
    let top = origin.y + px(vp.offset_y + frame.top);
    let accent = gpui::rgb(ACCENT_BLUE);
    window.paint_quad(outline(
        Bounds {
            origin: point(left - px(2.0), top - px(2.0)),
            size: size(
                px(frame.right - frame.left + 4.0),
                px(frame.bottom - frame.top + 4.0),
            ),
        },
        accent,
        BorderStyle::Dashed,
    ));
    if !matches!(selected, DrawAnnotation::Text(_)) {
        return;
    }
    let handle = px(TEXT_HANDLE_SIZE);
    let center = point(
        origin.x + px(vp.offset_x + frame.right),
        origin.y + px(vp.offset_y + frame.bottom),
    );
    window.paint_quad(
        fill(
            Bounds {
                origin: point(center.x - handle / 2.0, center.y - handle / 2.0),
                size: size(handle, handle),
            },
            accent,
        )
        .corner_radii(px(2.0)),
    );
    let inner = handle - px(4.0);
    window.paint_quad(
        fill(
            Bounds {
                origin: point(center.x - inner / 2.0, center.y - inner / 2.0),
                size: size(inner, inner),
            },
            gpui::rgb(0xffffff),
        )
        .corner_radii(px(1.0)),
    );
}

fn to_screen(vp: &ContentViewport, origin: Point<Pixels>, pt: &NormalizedPoint) -> Point<Pixels> {
    let (x, y) = vp.to_pixels(pt);
    point(origin.x + px(x), origin.y + px(y))
}

fn screen_points(
    vp: &ContentViewport,
    origin: Point<Pixels>,
    points: &[NormalizedPoint],
) -> Vec<Point<Pixels>> {
    points.iter().map(|pt| to_screen(vp, origin, pt)).collect()
}

fn paint_dot(window: &mut Window, center: Point<Pixels>, radius: Pixels, color: Rgba) {
    window.paint_quad(
        fill(
            Bounds {
                origin: point(center.x - radius, center.y - radius),
                size: size(radius * 2.0, radius * 2.0),
            },
            color,
        )
        .corner_radii(radius),
    );
}

fn paint_polyline(window: &mut Window, points: &[Point<Pixels>], color: u32, width: f32) {
    let Some(&first) = points.first() else {
        return;
    };
    let rgba = gpui::rgba(color);
    let radius = px((width * 0.5).max(1.0));
    if points.len() == 1 {
        paint_dot(window, first, radius, rgba);
        return;
    }

    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(first);
    for pt in &points[1..] {
        builder.line_to(*pt);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba);
    }

    if color & 0xFF == 0xFF {
        paint_dot(window, first, radius, rgba);
        if let Some(&last) = points.last() {
            paint_dot(window, last, radius, rgba);
        }
    }
}

fn paint_closed_outline(window: &mut Window, points: &[Point<Pixels>], color: u32, width: f32) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.add_polygon(points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, gpui::rgba(color));
    }
}

fn paint_shape(
    window: &mut Window,
    vp: &ContentViewport,
    origin: Point<Pixels>,
    shape: &ShapeAnnotation,
) {
    let start = to_screen(vp, origin, &shape.start);
    let end = to_screen(vp, origin, &shape.end);
    match shape.kind {
        ShapeKind::Arrow => {
            let from = (f32::from(start.x), f32::from(start.y));
            let to = (f32::from(end.x), f32::from(end.y));
            let Some(head) = mezon_voice::arrow_head(from, to, shape.width) else {
                return;
            };
            let shaft_end = point(
                px((head[1].0 + head[2].0) / 2.0),
                px((head[1].1 + head[2].1) / 2.0),
            );
            paint_polyline(window, &[start, shaft_end], shape.color, shape.width);
            let mut builder = PathBuilder::fill();
            builder.add_polygon(&head.map(|(x, y)| point(px(x), px(y))), true);
            if let Ok(path) = builder.build() {
                window.paint_path(path, gpui::rgba(shape.color));
            }
        }
        ShapeKind::Rectangle => {
            let corners = [start, point(end.x, start.y), end, point(start.x, end.y)];
            paint_closed_outline(window, &corners, shape.color, shape.width);
        }
        ShapeKind::Ellipse => {
            let cx = (f32::from(start.x) + f32::from(end.x)) / 2.0;
            let cy = (f32::from(start.y) + f32::from(end.y)) / 2.0;
            let rx = (f32::from(end.x) - f32::from(start.x)).abs() / 2.0;
            let ry = (f32::from(end.y) - f32::from(start.y)).abs() / 2.0;
            let outline: Vec<Point<Pixels>> = (0..ELLIPSE_SEGMENTS)
                .map(|i| {
                    let angle = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                    point(px(cx + rx * angle.cos()), px(cy + ry * angle.sin()))
                })
                .collect();
            paint_closed_outline(window, &outline, shape.color, shape.width);
        }
    }
}

fn paint_text_annotation(
    window: &mut Window,
    cx: &mut App,
    vp: &ContentViewport,
    origin: Point<Pixels>,
    text: &TextAnnotation,
) {
    if text.content.trim().is_empty() {
        return;
    }

    let anchor = to_screen(vp, origin, &text.position);
    let line_height = px(text_font_px(text.font_size)) * TEXT_LINE_HEIGHT;
    let line = shape_text(window, text);
    let _ = line.paint(anchor, line_height, TextAlign::Left, None, window, cx);
}
