use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, MouseButton, ObjectFit, Window, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions, div, img, prelude::*, px, rgb, rgba, size,
};
use mezon_store::{VoiceRenderFrame, VoiceStore};

use crate::app::window_controls::linux_app_id;
use crate::components::primitives::{Icon, IconName};

pub struct ScreenSharePipView {
    voice: Entity<VoiceStore>,
    key: u64,
    _frame_pump: Option<gpui::Task<()>>,
}

impl ScreenSharePipView {
    fn new(voice: Entity<VoiceStore>, key: u64, cx: &mut Context<Self>) -> Self {
        cx.observe(&voice, |_, _, cx| cx.notify()).detach();
        let release_voice = voice.clone();
        cx.on_release(move |_, cx| {
            release_voice.update(cx, |store, cx| store.on_pip_closed(key, cx));
        })
        .detach();
        let mut this = Self {
            voice,
            key,
            _frame_pump: None,
        };
        this.start_frame_pump(cx);
        this
    }

    fn start_frame_pump(&mut self, cx: &mut Context<Self>) {
        const PIP_FRAME_FALLBACK: std::time::Duration = std::time::Duration::from_millis(200);
        self._frame_pump = Some(cx.spawn(async move |this, cx| {
            let mut last_seq = 0u64;
            loop {
                let store = match this.update(cx, |this, cx| this.voice.read(cx).frame_store()) {
                    Ok(store) => store,
                    Err(_) => break,
                };
                let Some(store) = store else {
                    cx.background_executor().timer(PIP_FRAME_FALLBACK).await;
                    continue;
                };
                {
                    let mut rx = store.frame_watch();
                    let changed = std::pin::pin!(rx.changed());
                    let fallback =
                        std::pin::pin!(cx.background_executor().timer(PIP_FRAME_FALLBACK));
                    let _ = futures::future::select(changed, fallback).await;
                }
                let seq = store.publish_seq();
                if seq != last_seq {
                    last_seq = seq;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        }));
    }
}

impl Render for ScreenSharePipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.voice.read(cx).render_frame(self.key);
        self.voice
            .update(cx, |store, cx| store.flush_texture_drops(Some(window), cx));
        let content = match frame {
            Some(VoiceRenderFrame::Image(image)) => img(image)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            #[cfg(target_os = "macos")]
            Some(VoiceRenderFrame::Surface(surface)) => gpui::surface(surface.into_inner())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => Icon::new(IconName::VoiceScreenShareIcon)
                .size(px(40.))
                .text_color(rgb(0x9a9a9a))
                .into_any_element(),
        };

        let close = div()
            .id("pip-close")
            .absolute()
            .top_2()
            .right_2()
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.))
            .h(px(28.))
            .rounded_full()
            .bg(rgba(0x00000099))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0x000000cc)))
            .when(cfg!(target_os = "windows"), |el| {
                el.window_control_area(WindowControlArea::Close)
            })
            .child(
                Icon::new(IconName::Close)
                    .size(px(14.))
                    .text_color(rgb(0xffffff)),
            )
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                cx.stop_propagation();
                window.remove_window();
            });

        div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x000000))
            .when(cfg!(target_os = "windows"), |el| {
                el.window_control_area(WindowControlArea::Drag)
            })
            .when(cfg!(not(target_os = "windows")), |el| {
                el.on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.start_window_move();
                })
            })
            .child(content)
            .child(close)
    }
}

pub fn open_screen_share_pip(
    voice: Entity<VoiceStore>,
    key: u64,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(480.), px(290.)),
            cx,
        ))),
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: true,
        window_min_size: Some(size(px(240.), px(150.))),
        focus: false,
        show: true,
        app_id: linux_app_id(),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        keep_pip_above_other_windows(window);
        cx.new(|cx| ScreenSharePipView::new(voice, key, cx))
    })
    .ok()
    .map(Into::into)
}

#[cfg(target_os = "windows")]
fn keep_pip_above_other_windows(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut _);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn keep_pip_above_other_windows(_window: &Window) {}
