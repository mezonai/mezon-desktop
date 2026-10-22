use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, CursorStyle, Entity, FocusHandle,
    FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, canvas, div, prelude::*, px, size,
};
use mezon_store::{
    ContentViewport, DesktopDraw, DrawTool, NormalizedPoint, TextAnnotation, VoiceStore,
};

use super::native::{NativeRect, NativeWindow, ShareTarget, native_window};
use crate::app::window_controls::linux_app_id;
use crate::chat::screen_draw::{cursor_for_tool, paint_draw_state, text_width};
use crate::components::primitives::{Input, InputEvent, InputState};

const WINDOW_TRACK_INTERVAL: Duration = Duration::from_millis(120);
const DISPLAY_TRACK_INTERVAL: Duration = Duration::from_millis(1000);
const HOVER_POLL_INTERVAL: Duration = Duration::from_millis(33);
const TEXT_EDITOR_MIN_WIDTH: f32 = 220.;
const TEXT_EDITOR_BACKGROUND: u32 = 0x000000b3;

pub(super) struct OpenedOverlay {
    pub handle: AnyWindowHandle,
    pub native: NativeWindow,
    pub rect: NativeRect,
}

struct TextEditor {
    id: u64,
    input: Entity<InputState>,
    _subscription: Subscription,
}

pub(super) struct AnnotationOverlay {
    draw: Entity<DesktopDraw>,
    target: ShareTarget,
    native: Option<NativeWindow>,
    focus: FocusHandle,
    text_editor: Option<TextEditor>,
    click_through: Option<bool>,
    hovering: bool,
    native_scale: f64,
    visible: bool,
    _subscriptions: Vec<Subscription>,
    _tracker: Option<Task<()>>,
    _hover_poll: Option<Task<()>>,
}

pub(super) fn open(
    voice: Entity<VoiceStore>,
    draw: Entity<DesktopDraw>,
    target: ShareTarget,
    cx: &mut App,
) -> Option<OpenedOverlay> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(640.), px(360.)),
            cx,
        ))),
        titlebar: None,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus: false,
        show: true,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: linux_app_id(),
        ..Default::default()
    };

    let mut native = None;
    let handle = cx
        .open_window(options, |window, cx| {
            native = native_window(window);
            cx.new(|cx| AnnotationOverlay::new(voice, draw, target, native, window, cx))
        })
        .ok()?;
    let handle: AnyWindowHandle = handle.into();

    let placed = native.and_then(|native| {
        native.configure_overlay();
        let rect = target.rect(native)?;
        native.place(rect);
        Some((native, rect))
    });
    let Some((native, rect)) = placed else {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return None;
    };
    let _ = handle.update(cx, |_, window, cx| window.bounds_changed(cx));
    Some(OpenedOverlay {
        handle,
        native,
        rect,
    })
}

impl AnnotationOverlay {
    fn new(
        voice: Entity<VoiceStore>,
        draw: Entity<DesktopDraw>,
        target: ShareTarget,
        native: Option<NativeWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&draw, |this, _, cx| this.on_draw_changed(cx)),
            cx.observe_window_bounds(window, |this, window, cx| this.report_canvas(window, cx)),
        ];
        let release_draw = draw.clone();
        cx.on_release(move |_, cx| {
            voice.update(cx, |store, cx| {
                store.end_desktop_draw_for(&release_draw, cx)
            });
        })
        .detach();

        let mut this = Self {
            draw,
            target,
            native,
            focus: cx.focus_handle(),
            text_editor: None,
            click_through: None,
            hovering: false,
            native_scale: 1.0,
            visible: true,
            _subscriptions: subscriptions,
            _tracker: None,
            _hover_poll: None,
        };
        this.report_canvas(window, cx);
        this.sync_click_through(cx);
        this.start_tracker(window, cx);
        this.start_hover_poll(window, cx);
        this
    }

    fn report_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(native) = self.native {
            self.native_scale = native.scale(window);
        }
        let viewport = window.viewport_size();
        self.draw.update(cx, |draw, cx| {
            draw.set_canvas(f32::from(viewport.width), f32::from(viewport.height), cx)
        });
    }

    fn on_draw_changed(&mut self, cx: &mut Context<Self>) {
        let editing = self.draw.read(cx).state().editing_text_id;
        if self
            .text_editor
            .as_ref()
            .is_some_and(|editor| Some(editor.id) != editing)
        {
            self.text_editor = None;
        }
        self.sync_click_through(cx);
        cx.notify();
    }

    fn sync_click_through(&mut self, cx: &mut Context<Self>) {
        let state = self.draw.read(cx).state();
        let pass_through =
            state.active_tool == DrawTool::Pointer && !self.hovering && !state.is_drawing();
        if self.click_through == Some(pass_through) {
            return;
        }
        self.click_through = Some(pass_through);
        if let Some(native) = self.native {
            native.set_click_through(pass_through);
        }
    }

    fn start_tracker(&mut self, window: &Window, cx: &mut Context<Self>) {
        let interval = if self.target.is_window() {
            WINDOW_TRACK_INTERVAL
        } else {
            DISPLAY_TRACK_INTERVAL
        };
        let handle = window.window_handle();
        self._tracker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let Ok(changed) = this.update(cx, |this, _| this.follow_target()) else {
                    break;
                };
                if changed {
                    let _ = handle.update(cx, |_, window, cx| window.bounds_changed(cx));
                }
            }
        }));
    }

    fn start_hover_poll(&mut self, window: &Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        self._hover_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(HOVER_POLL_INTERVAL).await;
                let alive = handle
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| this.poll_hover(window, cx))
                            .is_ok()
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    fn poll_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.draw.read(cx).state();
        let hovering = state.active_tool == DrawTool::Pointer
            && self
                .native
                .and_then(|native| native.cursor_position())
                .is_some_and(|(x, y)| {
                    let viewport = window.viewport_size();
                    let canvas = (f32::from(viewport.width), f32::from(viewport.height));
                    let point = NormalizedPoint {
                        x: (x / self.native_scale) as f32 / canvas.0.max(1.0),
                        y: (y / self.native_scale) as f32 / canvas.1.max(1.0),
                    };
                    let inside = (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y);
                    let measure = |text: &TextAnnotation| text_width(window, text);
                    inside && state.hit_test(&point, canvas, &measure)
                });
        if hovering == self.hovering {
            return;
        }
        self.hovering = hovering;
        self.sync_click_through(cx);
        cx.notify();
    }

    fn follow_target(&mut self) -> bool {
        let Some(native) = self.native else {
            return false;
        };
        match self.target.rect(native) {
            Some(rect) => {
                let reveal = !self.visible;
                if reveal {
                    native.set_visible(true);
                    self.visible = true;
                }
                let moved = native.frame().differs_from(&rect);
                if moved {
                    native.place(rect);
                }
                reveal || moved
            }
            None => {
                if self.visible {
                    native.set_visible(false);
                    self.visible = false;
                }
                false
            }
        }
    }

    fn normalize(&self, position: Point<Pixels>, window: &Window) -> (NormalizedPoint, (f32, f32)) {
        let viewport = window.viewport_size();
        let canvas = (f32::from(viewport.width), f32::from(viewport.height));
        let point = ContentViewport::fill(canvas.0, canvas.1)
            .to_normalized(f32::from(position.x), f32::from(position.y));
        (point, canvas)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (point, canvas) = self.normalize(event.position, window);
        let measure = |text: &TextAnnotation| text_width(window, text);
        let editing = self.draw.update(cx, |draw, cx| {
            draw.edit(cx, |state| {
                state.pointer_down(point, canvas, &measure);
                state.editing_text_id
            })
        });
        self.after_pointer(editing, window, cx);
    }

    fn after_pointer(&mut self, editing: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_text_editor(editing, window, cx);
        if self.text_editor.is_some() {
            if let Some(native) = self.native {
                native.focus_keyboard();
            }
            window.prevent_default();
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn on_right_mouse_down(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.draw.update(cx, |draw, cx| {
            draw.edit(cx, |state| {
                state.cancel_stroke();
                state.set_tool(DrawTool::Pointer);
            })
        });
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pressed = event.pressed_button == Some(MouseButton::Left);
        let state = self.draw.read(cx).state();
        let erasing = pressed && state.active_tool == DrawTool::Eraser;
        if !state.is_drawing() && !erasing {
            return;
        }
        let (point, _) = self.normalize(event.position, window);
        self.draw.update(cx, |draw, cx| {
            draw.edit(cx, |state| state.pointer_move(point, pressed))
        });
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.draw.read(cx).state().is_drawing() {
            return;
        }
        let editing = self.draw.update(cx, |draw, cx| {
            draw.edit(cx, |state| {
                state.pointer_up();
                state.editing_text_id
            })
        });
        self.after_pointer(editing, window, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.text_editor.is_some() {
            if key == "escape" {
                self.draw.update(cx, |draw, cx| {
                    draw.edit(cx, |state| state.cancel_editing_text())
                });
                self.text_editor = None;
                self.focus.focus(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let modifiers = &event.keystroke.modifiers;
        let secondary = modifiers.secondary();
        let shift = modifiers.shift;
        let handled = match key {
            "z" if secondary && shift => self.edit(cx, |state| state.redo()),
            "z" if secondary => self.edit(cx, |state| state.undo()),
            "y" if modifiers.control => self.edit(cx, |state| state.redo()),
            "escape" => {
                self.edit(cx, |state| state.set_tool(DrawTool::Pointer));
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn edit<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut mezon_store::ScreenDrawState) -> R,
    ) -> R {
        self.draw.update(cx, |draw, cx| draw.edit(cx, f))
    }

    fn sync_text_editor(
        &mut self,
        editing: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = editing else {
            self.text_editor = None;
            return;
        };
        if self
            .text_editor
            .as_ref()
            .is_some_and(|editor| editor.id == id)
        {
            return;
        }
        let state = self.draw.read(cx).state();
        let content = state.editing_text_content.clone();
        let (color, font_size) = state
            .editing_text()
            .map(|text| (text.color, text.font_size))
            .unwrap_or((state.stroke_color, state.text_font_size));

        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder("Type a note, Enter to place")
                .borderless()
                .text_size(px(font_size))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::rgba(color))
                .bg(gpui::rgba(TEXT_EDITOR_BACKGROUND));
            if !content.is_empty() {
                input.set_value(content, window, cx);
            }
            input
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().to_string();
                    this.edit(cx, |state| state.set_editing_text_content(value));
                }
                InputEvent::PressEnter => {
                    this.edit(cx, |state| state.commit_editing_text());
                    this.text_editor = None;
                    this.focus.focus(window, cx);
                }
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.text_editor = Some(TextEditor {
            id,
            input,
            _subscription: subscription,
        });
    }

    fn render_text_editor(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        let editor = self.text_editor.as_ref()?;
        let text = self.draw.read(cx).state().editing_text()?;
        let viewport = window.viewport_size();
        Some(
            div()
                .id("desktop-annotation-text-editor")
                .absolute()
                .left(px(text.position.x * f32::from(viewport.width) - 4.))
                .top(px(text.position.y * f32::from(viewport.height) - 2.))
                .min_w(px(TEXT_EDITOR_MIN_WIDTH))
                .rounded(px(4.))
                .border_1()
                .border_color(gpui::rgba(text.color))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Input::new(&editor.input))
                .into_any_element(),
        )
    }
}

impl Render for AnnotationOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.draw.read(cx).state().clone();
        let cursor = match state.active_tool {
            DrawTool::Pointer if state.is_drawing() => CursorStyle::ClosedHand,
            DrawTool::Pointer if self.hovering => CursorStyle::OpenHand,
            tool => cursor_for_tool(tool),
        };
        let text_editor = self.render_text_editor(window, cx);

        div()
            .id("desktop-annotation-overlay")
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let viewport = ContentViewport::fill(
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                        );
                        paint_draw_state(
                            window,
                            cx,
                            &state,
                            &viewport,
                            bounds.origin,
                            Instant::now(),
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(text_editor)
    }
}
