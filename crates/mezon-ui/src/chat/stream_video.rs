use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, ObjectFit, Subscription, Task, Window, div, prelude::*, rgb,
};
use mezon_store::StreamStore;
use mezon_video::VideoFrame;

use crate::components::primitives::{Sizable, Size, Spinner};
use crate::image_cache::LruImageCache;

const FRAME_POLL: Duration = Duration::from_millis(8);
const HIDDEN_POLL: Duration = Duration::from_millis(250);
const HIDDEN_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
struct SharedPlayback {
    frame: Option<VideoFrame>,
    pending: Option<(u64, VideoFrame)>,
    pending_since: Option<Instant>,
    #[cfg(not(target_os = "macos"))]
    stream_image_id: Option<gpui::ImageId>,
}

pub struct StreamVideoView {
    stream: Entity<StreamStore>,
    shared: Rc<RefCell<SharedPlayback>>,
    generation: u64,
    image_cache: Entity<LruImageCache>,
    _frame_pump: Option<Task<()>>,
    _stream_observe: Subscription,
}

impl StreamVideoView {
    pub fn new(stream: Entity<StreamStore>, cx: &mut Context<Self>) -> Self {
        Self::register_teardown(cx);
        let stream_observe = cx.observe(&stream, |view, _, cx| {
            view.sync_frame_pump(cx);
            cx.notify();
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::message("stream-video", 2, 16 * 1024 * 1024, 16 * 1024 * 1024, cx)
        });
        let mut view = Self {
            stream,
            shared: Rc::new(RefCell::new(SharedPlayback::default())),
            generation: 0,
            image_cache,
            _frame_pump: None,
            _stream_observe: stream_observe,
        };
        view.sync_frame_pump(cx);
        view
    }

    fn sync_frame_pump(&mut self, cx: &mut Context<Self>) {
        let has_player = self.stream.read(cx).player().is_some();
        if !has_player {
            self._frame_pump = None;
            let mut shared = self.shared.borrow_mut();
            shared.pending = None;
            shared.pending_since = None;
        } else if self._frame_pump.is_none() {
            self._frame_pump = Some(cx.spawn(async move |this, cx| {
                loop {
                    let Ok(wait) = this.update(cx, |view, cx| view.pull_frame(cx)) else {
                        break;
                    };
                    cx.background_executor().timer(wait).await;
                }
            }));
        }
    }

    fn pull_frame(&mut self, cx: &mut Context<Self>) -> Duration {
        let hidden = self
            .shared
            .borrow()
            .pending_since
            .is_some_and(|at| at.elapsed() >= HIDDEN_AFTER);
        if hidden {
            return HIDDEN_POLL;
        }
        let (player, generation) = {
            let store = self.stream.read(cx);
            (store.player(), store.player_generation())
        };
        let Some(player) = player else {
            return HIDDEN_POLL;
        };
        if let Some(frame) = player.copy_frame() {
            let mut shared = self.shared.borrow_mut();
            shared.pending_since.get_or_insert_with(Instant::now);
            shared.pending = Some((generation, frame));
            drop(shared);
            cx.notify();
        }
        FRAME_POLL
    }

    fn take_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (has_player, current_gen) = {
            let store = self.stream.read(cx);
            (store.player().is_some(), store.player_generation())
        };
        if self.generation != current_gen || !has_player {
            self.generation = current_gen;
            let old = {
                let mut shared = self.shared.borrow_mut();
                #[cfg(not(target_os = "macos"))]
                {
                    shared.stream_image_id = None;
                }
                shared.frame.take()
            };
            Self::release_frame(old, Some(window), cx);
        }
        if !has_player {
            let mut shared = self.shared.borrow_mut();
            shared.pending = None;
            shared.pending_since = None;
            return false;
        }
        let pending = self.shared.borrow_mut().pending.take();
        if let Some((generation, frame)) = pending
            && generation == current_gen
        {
            let frame = Self::adopt_frame(&self.shared, frame, window, cx);
            let previous = self.shared.borrow_mut().frame.replace(frame);
            Self::release_stale_frame(previous, &self.shared, window, cx);
        }
        true
    }

    #[cfg(target_os = "macos")]
    fn adopt_frame(
        _shared: &Rc<RefCell<SharedPlayback>>,
        frame: VideoFrame,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> VideoFrame {
        frame
    }

    #[cfg(not(target_os = "macos"))]
    fn adopt_frame(
        shared: &Rc<RefCell<SharedPlayback>>,
        frame: VideoFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> VideoFrame {
        let stream_id = shared.borrow().stream_image_id;
        match stream_id {
            None => {
                shared.borrow_mut().stream_image_id = Some(frame.id);
                frame
            }
            Some(id) => match std::sync::Arc::try_unwrap(frame) {
                Ok(image) => {
                    let rebranded = std::sync::Arc::new(image.with_id(id));
                    cx.update_render_image(&rebranded, Some(window));
                    rebranded
                }
                Err(still_shared) => {
                    shared.borrow_mut().stream_image_id = Some(still_shared.id);
                    still_shared
                }
            },
        }
    }

    #[cfg(target_os = "macos")]
    fn release_stale_frame(
        _previous: Option<VideoFrame>,
        _shared: &Rc<RefCell<SharedPlayback>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    #[cfg(not(target_os = "macos"))]
    fn release_stale_frame(
        previous: Option<VideoFrame>,
        shared: &Rc<RefCell<SharedPlayback>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = previous
            && shared.borrow().stream_image_id != Some(previous.id)
        {
            cx.drop_image(previous, Some(window));
        }
    }

    #[cfg(target_os = "macos")]
    fn release_frame(
        _previous: Option<VideoFrame>,
        _window: Option<&mut Window>,
        _cx: &mut Context<Self>,
    ) {
    }

    #[cfg(not(target_os = "macos"))]
    fn release_frame(
        previous: Option<VideoFrame>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = previous {
            cx.drop_image(previous, window);
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
        self.shared.borrow().frame.clone().map(|frame| {
            gpui::surface(frame)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn frame_child(&self) -> Option<AnyElement> {
        self.shared.borrow().frame.clone().map(|frame| {
            gpui::img(frame)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        })
    }

    fn has_frame(&self) -> bool {
        self.shared.borrow().frame.is_some()
    }
}

impl Render for StreamVideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shared.borrow_mut().pending_since = None;
        let has_player = self.take_frame(window, cx);
        let has_frame = self.has_frame();
        let root = div()
            .image_cache(self.image_cache.clone())
            .id("stream-video-surface")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x000000));

        if !has_player {
            return root.into_any_element();
        }

        root.children(self.frame_child())
            .when(!has_frame, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Spinner::new().with_size(Size::Large).color(gpui::white())),
                )
            })
            .into_any_element()
    }
}
