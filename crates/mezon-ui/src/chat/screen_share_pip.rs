use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, MouseButton, ObjectFit, Window, WindowBounds,
    WindowKind, WindowOptions, div, img, prelude::*, px, rgb, rgba, size,
};
use mezon_store::VoiceStore;

use crate::app::window_controls::linux_app_id;
use crate::components::primitives::{Icon, IconName};

pub struct ScreenSharePipView {
    voice: Entity<VoiceStore>,
    key: u64,
}

impl ScreenSharePipView {
    fn new(voice: Entity<VoiceStore>, key: u64, cx: &mut Context<Self>) -> Self {
        cx.observe(&voice, |_, _, cx| cx.notify()).detach();
        let release_voice = voice.clone();
        cx.on_release(move |_, cx| {
            release_voice.update(cx, |store, cx| store.on_pip_closed(key, cx));
        })
        .detach();
        Self { voice, key }
    }
}

impl Render for ScreenSharePipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let image = self.voice.read(cx).render_image(self.key);
        self.voice
            .update(cx, |store, cx| store.flush_texture_drops(None, cx));
        if image.is_some() {
            window.request_animation_frame();
        }
        let content = match image {
            Some(image) => img(image)
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
            .child(
                Icon::new(IconName::Close)
                    .size(px(14.))
                    .text_color(rgb(0xffffff)),
            )
            .on_mouse_down(MouseButton::Left, |_, window, _| window.remove_window());

        div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x000000))
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

    cx.open_window(options, |_window, cx| {
        cx.new(|cx| ScreenSharePipView::new(voice, key, cx))
    })
    .ok()
    .map(Into::into)
}
