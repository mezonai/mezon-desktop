#[cfg(not(target_os = "windows"))]
use gpui::MouseButton;
#[cfg(target_os = "windows")]
use gpui::WindowControlArea;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use gpui::{AnyElement, DispatchPhase, MouseMoveEvent, MouseUpEvent, canvas};
use gpui::{
    AnyWindowHandle, App, Bounds, Context, CursorStyle, Entity, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, size,
};
use mezon_store::{DesktopDraw, VoiceStore};

use super::native::{NativeRect, NativeWindow, native_window};
use crate::app::window_controls::linux_app_id;
use crate::chat::screen_draw::{DrawTarget, render_draw_toolbar};
use crate::components::primitives::{Icon, IconName};

const TOOLBAR_WIDTH: f32 = 590.;
const TOOLBAR_HEIGHT: f32 = 46.;
const WINDOW_TOP_GAP: f64 = 12.;
const DISPLAY_TOP_GAP: f64 = 56.;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[derive(Clone, Copy)]
struct DragAnchor {
    pointer: (f64, f64),
    origin: (f64, f64),
}

pub(super) struct AnnotationToolbar {
    target: DrawTarget,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    native: Option<NativeWindow>,
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
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(TOOLBAR_WIDTH), px(TOOLBAR_HEIGHT)),
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
            cx.new(|cx| AnnotationToolbar::new(voice, draw, cx).with_native(native))
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
    fn new(voice: Entity<VoiceStore>, draw: Entity<DesktopDraw>, cx: &mut Context<Self>) -> Self {
        cx.observe(&draw, |_, _, cx| cx.notify()).detach();
        let release_voice = voice.clone();
        let release_draw = draw.clone();
        cx.on_release(move |_, cx| {
            release_voice.update(cx, |store, cx| {
                store.end_desktop_draw_for(&release_draw, cx)
            });
        })
        .detach();
        Self {
            target: DrawTarget::Desktop { voice, draw },
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            native: None,
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            drag: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn with_native(self, native: Option<NativeWindow>) -> Self {
        Self { native, ..self }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn with_native(self, _native: Option<NativeWindow>) -> Self {
        self
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

        let bar = render_draw_toolbar(&self.target, Some(grip.into_any_element()), cx)
            .size_full()
            .justify_center();
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let bar = bar.children(self.drag.is_some().then(|| self.drag_listener(cx)));
        bar
    }
}
