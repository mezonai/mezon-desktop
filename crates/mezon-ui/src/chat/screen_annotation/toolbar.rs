use std::time::Duration;

#[cfg(not(target_os = "windows"))]
use gpui::MouseButton;
#[cfg(target_os = "windows")]
use gpui::WindowControlArea;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use gpui::{DispatchPhase, MouseMoveEvent, MouseUpEvent, canvas};
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, CursorStyle, Div, Entity, Stateful, Task,
    Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, prelude::*,
    px, size,
};
use mezon_store::{DesktopDraw, VoiceStore};

use super::native::{NativeRect, NativeWindow, native_window};
use crate::app::window_controls::linux_app_id;
use crate::chat::screen_draw::{DrawTarget, render_draw_toolbar};
use crate::components::primitives::{Icon, IconName};

const TOOLBAR_WIDTH: f32 = 624.;
const TOOLBAR_HEIGHT: f32 = 46.;
const BUBBLE_WIDTH: f32 = 84.;
const BUBBLE_HEIGHT: f32 = 58.;
const BUBBLE_BUTTON_SIZE: f32 = 44.;
const BUBBLE_ICON_SIZE: f32 = 22.;
const BUBBLE_ACCENT: u32 = 0x5865f2;
const HINT_HEIGHT: f32 = 32.;
const HINT_DURATION: Duration = Duration::from_secs(4);
const WINDOW_TOP_GAP: f64 = 12.;
const DISPLAY_TOP_GAP: f64 = 56.;

fn toolbar_height(hint_visible: bool) -> f32 {
    if hint_visible {
        TOOLBAR_HEIGHT + HINT_HEIGHT
    } else {
        TOOLBAR_HEIGHT
    }
}

fn render_hint() -> Div {
    div()
        .mt(px(6.))
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .bg(gpui::rgba(0x18181bf2))
        .border_1()
        .border_color(gpui::rgba(0xffffff26))
        .text_xs()
        .text_color(gpui::rgb(0xffffff))
        .child("Press Esc to exit drawing mode")
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[derive(Clone, Copy)]
struct DragAnchor {
    pointer: (f64, f64),
    origin: (f64, f64),
}

pub(super) struct AnnotationToolbar {
    target: DrawTarget,
    draw: Entity<DesktopDraw>,
    overlay: NativeWindow,
    native: Option<NativeWindow>,
    minimized: bool,
    drawing: bool,
    hint_visible: bool,
    _hint_timer: Option<Task<()>>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    drag: Option<DragAnchor>,
}

pub(super) fn open(
    voice: Entity<VoiceStore>,
    draw: Entity<DesktopDraw>,
    target_rect: NativeRect,
    window_share: bool,
    overlay: NativeWindow,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let height = toolbar_height(draw.read(cx).in_drawing_mode());
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(TOOLBAR_WIDTH), px(height)),
            cx,
        ))),
        titlebar: None,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        focus: false,
        show: true,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: linux_app_id(),
        ..Default::default()
    };
    let top_gap = if window_share {
        WINDOW_TOP_GAP
    } else {
        DISPLAY_TOP_GAP
    };

    let mut native = None;
    let handle: AnyWindowHandle = cx
        .open_window(options, |window, cx| {
            native = native_window(window);
            cx.new(|cx| AnnotationToolbar::new(voice, draw, overlay, native, window, cx))
        })
        .ok()?
        .into();

    if let Some(native) = native {
        native.configure_toolbar(overlay);
        let scale = handle
            .update(cx, |_, window, _| native.scale(window))
            .unwrap_or(1.0);
        let width = TOOLBAR_WIDTH as f64 * scale;
        let x = target_rect.x + ((target_rect.width - width) / 2.0).max(0.0);
        let y = target_rect.y + top_gap * scale;
        native.move_to(x, y);
        let _ = handle.update(cx, |_, window, cx| window.bounds_changed(cx));
    }
    Some(handle)
}

impl AnnotationToolbar {
    fn new(
        voice: Entity<VoiceStore>,
        draw: Entity<DesktopDraw>,
        overlay: NativeWindow,
        native: Option<NativeWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_in(&draw, window, |this, _, window, cx| {
            this.sync_with_draw(window, cx)
        })
        .detach();
        let release_voice = voice.clone();
        let release_draw = draw.clone();
        cx.on_release(move |_, cx| {
            release_voice.update(cx, |store, cx| {
                store.end_desktop_draw_for(&release_draw, cx)
            });
        })
        .detach();
        let target = DrawTarget::Desktop { draw: draw.clone() };
        let drawing = draw.read(cx).in_drawing_mode();
        let mut this = Self {
            target,
            draw,
            overlay,
            native,
            minimized: false,
            drawing,
            hint_visible: drawing,
            _hint_timer: None,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            drag: None,
        };
        if drawing {
            this.schedule_hint_hide(window, cx);
        }
        this
    }

    fn sync_with_draw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draw = self.draw.read(cx);
        let minimized = draw.is_minimized();
        let drawing = draw.in_drawing_mode();
        let previous_size = self.window_size();
        self.minimized = minimized;
        if drawing && !self.drawing {
            self.hint_visible = true;
            self.schedule_hint_hide(window, cx);
        } else if !drawing {
            self.hint_visible = false;
            self._hint_timer = None;
        }
        self.drawing = drawing;
        if self.window_size() != previous_size {
            self.fit_window(window, cx);
        }
        cx.notify();
    }

    fn schedule_hint_hide(&mut self, window: &Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        self._hint_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HINT_DURATION).await;
            let _ = handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| this.hide_hint(window, cx))
            });
        }));
    }

    fn hide_hint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.hint_visible {
            return;
        }
        self.hint_visible = false;
        self.fit_window(window, cx);
        cx.notify();
    }

    fn window_size(&self) -> (f32, f32) {
        if self.minimized {
            (BUBBLE_WIDTH, BUBBLE_HEIGHT)
        } else {
            (TOOLBAR_WIDTH, toolbar_height(self.hint_visible))
        }
    }

    fn fit_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (width, height) = self.window_size();
        let Some(native) = self.native else {
            window.resize(size(px(width), px(height)));
            return;
        };
        let frame = native.frame();
        let overlay = self.overlay.frame();
        let scale = native.scale(window);
        let scaled_width = width as f64 * scale;
        let keep_right_edge = frame.x + frame.width / 2.0 > overlay.x + overlay.width / 2.0;
        let x = if keep_right_edge {
            frame.x + frame.width - scaled_width
        } else {
            frame.x
        };
        native.place(NativeRect {
            x,
            y: frame.y,
            width: scaled_width,
            height: height as f64 * scale,
        });
        window.bounds_changed(cx);
    }

    fn render_bubble(&self, grip: AnyElement) -> Stateful<Div> {
        let draw = self.draw.clone();
        let expand = div()
            .id("desktop-annotation-expand")
            .size(px(BUBBLE_BUTTON_SIZE))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(gpui::rgb(BUBBLE_ACCENT))
            .hover(|s| s.opacity(0.85))
            .child(
                Icon::new(IconName::PenEdit)
                    .size(px(BUBBLE_ICON_SIZE))
                    .text_color(gpui::rgb(0xffffff)),
            )
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                draw.update(cx, |draw, cx| draw.set_minimized(false, cx));
            });

        div()
            .id("desktop-annotation-bubble")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .p(px(4.))
                    .rounded_full()
                    .bg(gpui::rgba(0x18181bf2))
                    .border_1()
                    .border_color(gpui::rgba(0xffffff26))
                    .shadow_xl()
                    .child(grip)
                    .child(expand),
            )
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn begin_drag(&mut self, cx: &mut Context<Self>) {
        let Some(native) = self.native else {
            return;
        };
        let Some(pointer) = native.pointer_position() else {
            return;
        };
        let frame = native.frame();
        self.drag = Some(DragAnchor {
            pointer,
            origin: (frame.x, frame.y),
        });
        cx.notify();
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn drag_to(&mut self) {
        let (Some(native), Some(anchor)) = (self.native, self.drag) else {
            return;
        };
        if let Some((x, y)) = native.pointer_position() {
            native.move_to(
                anchor.origin.0 + x - anchor.pointer.0,
                anchor.origin.1 + y - anchor.pointer.1,
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        cx.notify();
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn drag_listener(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moving = entity.clone();
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = moving.update(cx, |this, _| this.drag_to());
                    }
                });
                let releasing = entity.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = releasing.update(cx, |this, cx| this.end_drag(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }
}

impl Render for AnnotationToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grip = div()
            .id("desktop-annotation-grip")
            .w(px(20.))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::OpenHand)
            .child(
                Icon::new(IconName::AnnotateGrip)
                    .size(px(16.))
                    .text_color(gpui::rgba(0xffffff99)),
            );
        #[cfg(target_os = "windows")]
        let grip = grip.window_control_area(WindowControlArea::Drag);
        #[cfg(target_os = "macos")]
        let grip = grip.on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        });
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let grip = grip.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.begin_drag(cx)),
        );

        let bar = if self.minimized {
            self.render_bubble(grip.into_any_element())
        } else {
            div()
                .id("desktop-annotation-toolbar")
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    render_draw_toolbar(&self.target, Some(grip.into_any_element()), cx)
                        .w_full()
                        .h(px(TOOLBAR_HEIGHT))
                        .justify_center(),
                )
                .children(self.hint_visible.then(render_hint))
        };
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let bar = bar.children(self.drag.is_some().then(|| self.drag_listener(cx)));
        bar
    }
}
