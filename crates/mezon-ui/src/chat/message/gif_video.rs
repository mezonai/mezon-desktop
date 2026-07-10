use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, Context, ObjectFit, SharedString, Window, div, img, prelude::*, px};
use mezon_video::{VideoFrame, VideoPlayer};

use crate::theme::ActiveTheme;

const LOOP_EPSILON_SECONDS: f64 = 0.08;
const THUMB_DECODE_MAX_PX: u32 = 120;

#[derive(Default)]
struct GifPlayback {
    frame: Option<VideoFrame>,
    failed: bool,
}

pub struct VideoThumbView {
    player: Option<Rc<VideoPlayer>>,
    shared: Rc<RefCell<GifPlayback>>,
    frozen: bool,
}

impl VideoThumbView {
    pub fn new(url: SharedString, cx: &mut Context<Self>) -> Self {
        let player = VideoPlayer::open(
            url.as_ref(),
            Some((THUMB_DECODE_MAX_PX, THUMB_DECODE_MAX_PX)),
        )
        .ok()
        .map(Rc::new);
        if let Some(player) = player.as_ref() {
            player.set_muted(true);
            player.play();
        }
        let shared = Rc::new(RefCell::new(GifPlayback::default()));
        Self::register_teardown(cx);
        Self {
            player,
            shared,
            frozen: false,
        }
    }

    pub fn decoding(&self) -> bool {
        self.player.is_some() && !self.frozen
    }

    fn poll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let failed = player.failed();
        let new_frame = player.copy_frame();
        let captured = new_frame.is_some();
        let previous = {
            let mut shared = self.shared.borrow_mut();
            shared.failed = failed;
            new_frame.and_then(|frame| shared.frame.replace(frame))
        };
        Self::release_frame(previous, window, cx);
        if captured && !self.frozen {
            player.pause();
            self.frozen = true;
            self.player = None;
            cx.notify();
        } else if failed {
            self.frozen = true;
            self.player = None;
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn release_frame(_previous: Option<VideoFrame>, _window: &mut Window, _cx: &mut Context<Self>) {
    }

    #[cfg(not(target_os = "macos"))]
    fn release_frame(previous: Option<VideoFrame>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = previous {
            cx.drop_image(previous, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn register_teardown(_cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    fn register_teardown(cx: &mut Context<Self>) {
        cx.on_release(|view, cx| {
            if let Some(frame) = view.shared.borrow_mut().frame.take() {
                crate::image_cache::queue_atlas_drop(cx, frame);
            }
        })
        .detach();
    }

    #[cfg(target_os = "macos")]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::surface(frame).size_full().into_any_element())
    }

    #[cfg(not(target_os = "macos"))]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared.borrow().frame.clone().map(|frame| {
            gpui::img(frame)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        })
    }
}

impl Render for VideoThumbView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_frame(window, cx);
        let has_frame = self.shared.borrow().frame.is_some();
        if !self.frozen && self.player.is_some() && !self.shared.borrow().failed {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .children(if has_frame { self.frame_child() } else { None })
    }
}

pub struct GifVideoView {
    fallback_gif: SharedString,
    width: f32,
    height: f32,
    player: Option<Rc<VideoPlayer>>,
    shared: Rc<RefCell<GifPlayback>>,
    playing: bool,
}

impl GifVideoView {
    pub fn new(
        mp4_url: SharedString,
        fallback_gif: SharedString,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = VideoPlayer::open(mp4_url.as_ref(), decode_size(width, height))
            .ok()
            .map(Rc::new);
        if let Some(player) = player.as_ref() {
            player.set_muted(true);
            player.play();
        }
        let playing = player.is_some();
        let shared = Rc::new(RefCell::new(GifPlayback::default()));
        Self::register_teardown(cx);
        Self {
            fallback_gif,
            width,
            height,
            player,
            shared,
            playing,
        }
    }

    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        if playing {
            player.play();
        } else {
            player.pause();
        }
        self.playing = playing;
        cx.notify();
    }

    fn poll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let failed = player.failed();
        let duration = player.duration();
        let current_time = player.current_time();
        if duration > 0.0 && current_time >= duration - LOOP_EPSILON_SECONDS {
            player.seek(0.0);
            player.play();
        }
        let new_frame = player.copy_frame();
        let previous = {
            let mut shared = self.shared.borrow_mut();
            if failed != shared.failed {
                shared.failed = failed;
            }
            new_frame.and_then(|frame| shared.frame.replace(frame))
        };
        Self::release_frame(previous, window, cx);
    }

    #[cfg(target_os = "macos")]
    fn release_frame(_previous: Option<VideoFrame>, _window: &mut Window, _cx: &mut Context<Self>) {
    }

    #[cfg(not(target_os = "macos"))]
    fn release_frame(previous: Option<VideoFrame>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = previous {
            cx.drop_image(previous, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn register_teardown(_cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    fn register_teardown(cx: &mut Context<Self>) {
        cx.on_release(|view, cx| {
            if let Some(frame) = view.shared.borrow_mut().frame.take() {
                crate::image_cache::queue_atlas_drop(cx, frame);
            }
        })
        .detach();
    }

    #[cfg(target_os = "macos")]
    pub fn release_textures(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    pub fn release_textures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(frame) = self.shared.borrow_mut().frame.take() {
            cx.drop_image(frame, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::surface(frame).size_full().into_any_element())
    }

    #[cfg(not(target_os = "macos"))]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared
            .borrow()
            .frame
            .clone()
            .map(|frame| gpui::img(frame).size_full().into_any_element())
    }
}

impl Render for GifVideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_frame(window, cx);
        let theme = cx.theme();
        let failed = self.shared.borrow().failed;
        let root = div()
            .w(px(self.width))
            .h(px(self.height))
            .max_w_full()
            .rounded_md()
            .overflow_hidden()
            .bg(theme.bg_tertiary);

        if (self.player.is_none() || failed) && !self.fallback_gif.is_empty() {
            return root.child(
                img(self.fallback_gif.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            );
        }

        if self.playing && !failed && self.player.is_some() && window.is_window_active() {
            window.request_animation_frame();
        }

        root.children(if failed { None } else { self.frame_child() })
    }
}

fn decode_size(width: f32, height: f32) -> Option<(u32, u32)> {
    let width = width.round();
    let height = height.round();
    if width >= 1.0 && height >= 1.0 {
        Some((width as u32, height as u32))
    } else {
        None
    }
}
