use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, DragMoveEvent, Empty, Entity, EntityId,
    FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit, Pixels, Rgba, SharedString,
    Task, Window, canvas, div, img, prelude::*, px, relative,
};
use mezon_store::open_media_url_external;
#[cfg(any(windows, target_os = "macos"))]
use mezon_video::{PreparedWebm, is_webm_url};
use mezon_video::{VideoFrame, VideoPlayer};

use crate::app::shell::Shell;
use crate::components::primitives::{Icon, IconName, h_flex};
use crate::image_cache::LruImageCache;
use crate::theme::ActiveTheme;

const SEEK_STEP_SECONDS: f64 = 5.0;
const SEEK_MIN_INTERVAL: Duration = Duration::from_millis(80);
const REPLAY_THRESHOLD_SECONDS: f64 = 0.05;
const STUCK_PLAYING_END_SECONDS: f64 = 0.02;
const THEATER_FILL: f32 = 0.92;
const CONTROL_TINT: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.16,
};
const SCRIM: Rgba = Rgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.55,
};
const TRACK_BG: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.3,
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoFullscreenMode {
    #[default]
    ShellModal,
    InPlaceTheater,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoLayout {
    #[default]
    Fixed,
    FillContainer,
}

pub struct VideoActivation {
    pub url: SharedString,
    pub filename: SharedString,
    pub poster: SharedString,
    pub width: f32,
    pub height: f32,
    pub fullscreen_mode: VideoFullscreenMode,
    pub layout: VideoLayout,
    pub decode_max_size: Option<(u32, u32)>,
    /// Locale for the "cannot play" card — the view has no settings entity of
    /// its own, so the caller hands its own locale down.
    pub locale: SharedString,
}

#[derive(Default)]
struct SharedPlayback {
    frame: Option<VideoFrame>,
    #[cfg(not(target_os = "macos"))]
    stream_image_id: Option<gpui::ImageId>,
    current_time: f64,
    duration: f64,
    playing: bool,
    muted: bool,
    failed: bool,
}

type Shared = Rc<RefCell<SharedPlayback>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoLoadState {
    Loading,
    Ready,
    Failed,
}

#[derive(Clone)]
struct SeekDrag(EntityId);

impl Render for SeekDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct VideoPlayerView {
    theater: bool,
    fullscreen_mode: VideoFullscreenMode,
    layout: VideoLayout,
    focus_handle: FocusHandle,
    url: SharedString,
    filename: SharedString,
    poster: SharedString,
    locale: SharedString,
    width: f32,
    height: f32,
    player: Option<Rc<VideoPlayer>>,
    load_state: VideoLoadState,
    decode_max_size: Option<(u32, u32)>,
    autoplay_when_ready: bool,
    shared: Shared,
    track_bounds: Bounds<Pixels>,
    time_label: SharedString,
    last_label_seconds: (u64, u64),
    image_cache: Entity<LruImageCache>,
    _open_task: Option<Task<()>>,
    last_seek_at: Option<Instant>,
    pending_seek: Option<f64>,
    signed: Option<mezon_client::cdn_signature::SignedUrl>,
    resigned: bool,
}

impl VideoPlayerView {
    pub fn new(activation: VideoActivation, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let VideoActivation {
            url,
            filename,
            poster,
            width,
            height,
            fullscreen_mode,
            layout,
            decode_max_size,
            locale,
        } = activation;
        let shared = Rc::new(RefCell::new(SharedPlayback::default()));
        Self::register_teardown(cx);
        let mut view = Self {
            theater: false,
            fullscreen_mode,
            layout,
            focus_handle: cx.focus_handle(),
            url,
            filename,
            poster,
            locale,
            width,
            height,
            player: None,
            load_state: VideoLoadState::Loading,
            decode_max_size,
            autoplay_when_ready: true,
            shared,
            track_bounds: Bounds::default(),
            time_label: SharedString::new_static("00:00 / 00:00"),
            last_label_seconds: (0, 0),
            image_cache: cx.new(|cx| {
                LruImageCache::message("video-poster", 2, 16 * 1024 * 1024, 16 * 1024 * 1024, cx)
            }),
            _open_task: None,
            last_seek_at: None,
            pending_seek: None,
            signed: None,
            resigned: false,
        };
        view.start_open(cx);
        view
    }

    fn finish_open(
        &mut self,
        opened: Result<VideoPlayer, mezon_video::PlayerError>,
        cx: &mut Context<Self>,
    ) {
        self._open_task = None;
        match opened {
            Ok(player) => {
                let duration = player.duration();
                let player = Rc::new(player);
                let should_play = self.autoplay_when_ready && duration > 0.0;
                if should_play {
                    player.play();
                }
                self.player = Some(player);
                self.load_state = VideoLoadState::Ready;
                let mut shared = self.shared.borrow_mut();
                shared.playing = should_play;
                shared.failed = false;
                shared.duration = duration;
            }
            Err(_) => {
                if self.open_with_fresh_signature(cx) {
                    return;
                }
                self.player = None;
                self.load_state = VideoLoadState::Failed;
                let mut shared = self.shared.borrow_mut();
                shared.playing = false;
                shared.failed = true;
            }
        }
        cx.notify();
    }

    fn start_open(&mut self, cx: &mut Context<Self>) {
        self.load_state = VideoLoadState::Loading;
        self._open_task = None;
        let url = self.url.clone();
        let decode_max_size = self.decode_max_size;
        self._open_task = Some(cx.spawn(async move |this, cx| {
            let signed = mezon_client::cdn_signature::sign(url.as_ref()).await;
            let url = signed
                .as_ref()
                .map_or(url, |signed| SharedString::from(signed.url.clone()));
            let _ = this.update(cx, |this, _| this.signed = signed);
            #[cfg(any(windows, target_os = "macos"))]
            if is_webm_url(url.as_ref()) {
                let open_url = url.clone();
                let prepared = cx
                    .background_executor()
                    .spawn(async move { PreparedWebm::open(open_url.as_ref(), decode_max_size) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.finish_open(prepared.map(VideoPlayer::from_prepared_webm), cx);
                });
                return;
            }

            let _ = this.update(cx, |this, cx| {
                this.finish_open(VideoPlayer::open(url.as_ref(), decode_max_size), cx);
            });
        }));
    }

    fn open_with_fresh_signature(&mut self, cx: &mut Context<Self>) -> bool {
        if self.resigned {
            return false;
        }
        let Some(signed) = self.signed.take() else {
            return false;
        };
        self.resigned = true;
        if !mezon_client::cdn_signature::forget(&signed) {
            return false;
        }
        self.player = None;
        self.shared.borrow_mut().failed = false;
        self.start_open(cx);
        cx.notify();
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn open_theater(
        player: Rc<VideoPlayer>,
        shared: Shared,
        url: SharedString,
        filename: SharedString,
        poster: SharedString,
        locale: SharedString,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| Self {
            theater: true,
            fullscreen_mode: VideoFullscreenMode::ShellModal,
            layout: VideoLayout::Fixed,
            focus_handle: cx.focus_handle(),
            url,
            filename,
            poster,
            locale,
            width,
            height,
            player: Some(player),
            load_state: VideoLoadState::Ready,
            decode_max_size: None,
            autoplay_when_ready: false,
            shared,
            track_bounds: Bounds::default(),
            time_label: SharedString::new_static("00:00 / 00:00"),
            last_label_seconds: (0, 0),
            image_cache: cx.new(|cx| {
                LruImageCache::message("video-poster", 2, 16 * 1024 * 1024, 16 * 1024 * 1024, cx)
            }),
            _open_task: None,
            last_seek_at: None,
            pending_seek: None,
            signed: None,
            resigned: false,
        });
        let focus_handle = view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.into(), cx));
    }

    fn poll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.pending_seek.take() {
            self.apply_seek_target(target, false, window, cx);
        }
        let Some(player) = self.player.clone() else {
            return;
        };
        let was_playing = self.shared.borrow().playing;
        let mut playing = player.is_playing();
        let current_time = player.current_time();
        let duration = player.duration();
        let muted = player.is_muted();
        let failed = player.failed();
        if failed && self.open_with_fresh_signature(cx) {
            return;
        }
        if playing && duration > 0.0 && current_time >= duration - STUCK_PLAYING_END_SECONDS {
            player.pause();
            playing = false;
        }
        let new_frame = if playing { player.copy_frame() } else { None };
        let new_frame = new_frame.map(|frame| Self::adopt_frame(&self.shared, frame, window, cx));
        let previous = {
            let mut shared = self.shared.borrow_mut();
            shared.failed = failed;
            shared.playing = playing;
            shared.current_time = current_time;
            shared.duration = duration;
            shared.muted = muted;
            new_frame.and_then(|frame| shared.frame.replace(frame))
        };
        Self::release_stale_frame(previous, &self.shared, window, cx);
        self.refresh_time_label(playing, current_time, duration);
        if was_playing && !playing {
            cx.notify();
        }
    }

    fn refresh_time_label(&mut self, playing: bool, current_time: f64, duration: f64) {
        let display_time = display_playhead(playing, current_time, duration);
        let seconds = (whole_seconds(display_time), whole_seconds(duration));
        if seconds != self.last_label_seconds {
            self.last_label_seconds = seconds;
            self.time_label = SharedString::from(format!(
                "{} / {}",
                format_seconds(display_time),
                format_seconds(duration)
            ));
        }
    }

    pub fn reopen(
        &mut self,
        activation: VideoActivation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.url.as_ref() == activation.url.as_ref() {
            return;
        }
        self.shutdown(Some(window), cx);
        let VideoActivation {
            url,
            filename,
            poster,
            width,
            height,
            fullscreen_mode,
            layout,
            decode_max_size,
            locale,
        } = activation;
        self.url = url;
        self.filename = filename;
        self.poster = poster;
        self.locale = locale;
        self.width = width;
        self.height = height;
        self.fullscreen_mode = fullscreen_mode;
        self.layout = layout;
        self.theater = false;
        self.track_bounds = Bounds::default();
        self.time_label = SharedString::new_static("00:00 / 00:00");
        self.last_label_seconds = (0, 0);
        self.last_seek_at = None;
        self.pending_seek = None;
        {
            let mut shared = self.shared.borrow_mut();
            shared.current_time = 0.0;
            shared.duration = 0.0;
        }
        self.decode_max_size = decode_max_size;
        self.autoplay_when_ready = true;
        self.start_open(cx);
        cx.notify();
    }

    pub fn shutdown(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self._open_task = None;
        self.pending_seek = None;
        self.load_state = VideoLoadState::Loading;
        self.autoplay_when_ready = false;
        if let Some(player) = self.player.take() {
            player.pause();
            drop(player);
        }
        let frame = self.shared.borrow_mut().frame.take();
        Self::release_frame(frame, window, cx);
        {
            let mut shared = self.shared.borrow_mut();
            shared.playing = false;
            shared.failed = false;
            shared.current_time = 0.0;
            shared.duration = 0.0;
        }
    }

    pub fn pause_for_background(&mut self, cx: &mut Context<Self>) {
        self.autoplay_when_ready = false;
        let Some(player) = self.player.clone() else {
            return;
        };
        if self.shared.borrow().playing {
            player.pause();
            self.shared.borrow_mut().playing = false;
            cx.notify();
        }
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        {
            let mut shared = self.shared.borrow_mut();
            if shared.playing {
                player.pause();
                shared.playing = false;
            } else {
                if should_replay_from_start(shared.playing, shared.current_time, shared.duration) {
                    player.seek(0.0);
                    shared.current_time = 0.0;
                }
                player.play();
                shared.playing = true;
            }
        }
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.player.clone() else {
            return;
        };
        let next = !self.shared.borrow().muted;
        player.set_muted(next);
        self.shared.borrow_mut().muted = next;
        cx.notify();
    }

    fn seek_relative(&mut self, delta: f64, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let shared = self.shared.borrow();
            if shared.duration <= 0.0 {
                return;
            }
            (shared.current_time + delta).clamp(0.0, shared.duration)
        };
        self.apply_seek_target(target, true, window, cx);
    }

    fn seek_to_x(&mut self, x: Pixels, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.track_bounds;
        let target = {
            let shared = self.shared.borrow();
            if shared.duration <= 0.0 {
                return;
            }
            fraction_from_position(bounds, x) as f64 * shared.duration
        };
        self.apply_seek_target(target, force, window, cx);
    }

    fn apply_seek_target(
        &mut self,
        target: f64,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(player) = self.player.clone() else {
            return;
        };
        {
            let mut shared = self.shared.borrow_mut();
            if shared.duration <= 0.0 {
                return;
            }
            shared.current_time = target.clamp(0.0, shared.duration);
        }
        let now = Instant::now();
        if !force
            && self
                .last_seek_at
                .is_some_and(|prev| now.duration_since(prev) < SEEK_MIN_INTERVAL)
        {
            self.pending_seek = Some(target);
            let (current_time, duration) = {
                let shared = self.shared.borrow();
                (shared.current_time, shared.duration)
            };
            self.refresh_time_label(player.is_playing(), current_time, duration);
            cx.notify();
            return;
        }
        self.last_seek_at = Some(now);
        self.pending_seek = None;
        let was_playing = player.is_playing();
        player.seek(target);
        if was_playing {
            player.play();
            self.shared.borrow_mut().playing = true;
        }
        if let Some(frame) = player.copy_frame() {
            let frame = Self::adopt_frame(&self.shared, frame, window, cx);
            let previous = self.shared.borrow_mut().frame.replace(frame);
            Self::release_stale_frame(previous, &self.shared, window, cx);
        }
        let current_time = player.current_time();
        let duration = player.duration();
        self.shared.borrow_mut().current_time = current_time;
        self.refresh_time_label(player.is_playing(), current_time, duration);
        cx.notify();
    }

    fn open_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.fullscreen_mode {
            VideoFullscreenMode::ShellModal => {
                if let Some(player) = self.player.clone() {
                    Self::open_theater(
                        player,
                        self.shared.clone(),
                        self.url.clone(),
                        self.filename.clone(),
                        self.poster.clone(),
                        self.locale.clone(),
                        self.width,
                        self.height,
                        window,
                        cx,
                    );
                }
            }
            VideoFullscreenMode::InPlaceTheater => {
                self.theater = true;
                cx.notify();
            }
        }
    }

    fn exit_theater(&mut self, cx: &mut Context<Self>) {
        match self.fullscreen_mode {
            VideoFullscreenMode::ShellModal => {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }
            VideoFullscreenMode::InPlaceTheater => {
                self.theater = false;
                cx.notify();
            }
        }
    }

    fn open_external(&self, cx: &mut App) {
        open_media_url_external(self.url.to_string(), cx);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let plain = !event.keystroke.modifiers.modified();
        match event.keystroke.key.as_str() {
            "space" => self.toggle_play(cx),
            "left" if plain => self.seek_relative(-SEEK_STEP_SECONDS, window, cx),
            "right" if plain => self.seek_relative(SEEK_STEP_SECONDS, window, cx),
            "f" if !self.theater => self.open_fullscreen(window, cx),
            "escape" if self.theater => self.exit_theater(cx),
            _ => {}
        }
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
    pub fn release_textures(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    #[cfg(not(target_os = "macos"))]
    pub fn release_textures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(frame) = self.shared.borrow_mut().frame.take() {
            cx.drop_image(frame, Some(window));
        }
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

    fn render_seek(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity_id = cx.entity_id();
        let fraction = {
            let shared = self.shared.borrow();
            if shared.duration > 0.0 {
                let display_time =
                    display_playhead(shared.playing, shared.current_time, shared.duration);
                (display_time / shared.duration).clamp(0.0, 1.0) as f32
            } else {
                0.0
            }
        };
        let brand = cx.theme().brand;
        div()
            .id("video-seek")
            .relative()
            .flex()
            .items_center()
            .h(px(14.))
            .w_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.seek_to_x(event.position.x, true, window, cx);
                }),
            )
            .on_drag(SeekDrag(entity_id), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(
                cx.listener(move |view, event: &DragMoveEvent<SeekDrag>, window, cx| {
                    let SeekDrag(id) = event.drag(cx);
                    if *id != entity_id {
                        return;
                    }
                    view.seek_to_x(event.event.position.x, false, window, cx);
                }),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(TRACK_BG)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(relative(fraction))
                            .bg(brand)
                            .rounded_full(),
                    ),
            )
            .child({
                let view = cx.entity();
                canvas(
                    move |bounds, _, cx| view.update(cx, |this, _| this.track_bounds = bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
    }

    /// Shown when the platform player cannot open the file at all. Without it
    /// the failed state renders exactly like the untouched poster — same play
    /// circle, same overlay — so a video the demuxer rejects just looks frozen
    /// and clicking it appears to do nothing.
    fn unplayable_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (title_color, body_color, button_bg) = {
            let theme = cx.theme();
            (theme.text_primary, theme.text_secondary, theme.bg_floating)
        };
        let locale = self.locale.as_ref();
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .bg(SCRIM)
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size(px(22.))
                    .text_color(body_color),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(title_color)
                    .child(mezon_i18n::t(locale, "media.video.error.title")),
            )
            .child(
                div()
                    .max_w(px(240.))
                    .text_size(px(12.))
                    .text_color(body_color)
                    .child(mezon_i18n::t(
                        locale,
                        "media.video.error.formatNotSupported",
                    )),
            )
            .child(
                h_flex()
                    .id("video-unplayable-download")
                    .gap_1p5()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(button_bg)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|view, _, _window, cx| {
                        cx.stop_propagation();
                        crate::util::download::save_with_progress_toast(
                            view.url.clone(),
                            view.filename.clone(),
                            cx,
                        );
                    }))
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(14.))
                            .text_color(title_color),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(title_color)
                            .child(mezon_i18n::t(locale, "media.video.error.downloadButton")),
                    ),
            )
    }

    fn download_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (bg, icon_color) = {
            let theme = cx.theme();
            (theme.bg_secondary, theme.text_secondary)
        };
        div()
            .id("video-download")
            .absolute()
            .top(px(8.))
            .right(px(4.))
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(bg)
            .cursor_pointer()
            .opacity(0.0)
            .group_hover("video-player", |s| s.opacity(1.0))
            .on_click(cx.listener(|view, _, _window, cx| {
                crate::util::download::save_with_progress_toast(
                    view.url.clone(),
                    view.filename.clone(),
                    cx,
                );
            }))
            .child(
                Icon::new(IconName::Download)
                    .size(px(16.))
                    .text_color(icon_color),
            )
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theater = self.theater;
        let (playing, muted) = {
            let shared = self.shared.borrow();
            (shared.playing, shared.muted)
        };
        let label = self.time_label.clone();
        let play_icon = if playing {
            IconName::PauseButton
        } else {
            IconName::PlayButton
        };
        let mute_icon = if muted {
            IconName::MutedVolume
        } else {
            IconName::LoudVolume
        };
        let last_icon = if theater {
            IconName::ExitFullScreen
        } else {
            IconName::FullScreen
        };
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1p5()
            .bg(SCRIM)
            .opacity(0.0)
            .group_hover("video-player", |s| s.opacity(1.0))
            .child(self.render_seek(cx))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(control_button(
                        "video-playpause",
                        play_icon,
                        cx.listener(|view, _, _window, cx| view.toggle_play(cx)),
                    ))
                    .child(div().text_xs().text_color(gpui::white()).child(label))
                    .child(div().flex_1())
                    .child(control_button(
                        "video-mute",
                        mute_icon,
                        cx.listener(|view, _, _window, cx| view.toggle_mute(cx)),
                    ))
                    .child(control_button(
                        "video-fullscreen",
                        last_icon,
                        cx.listener(move |view, _, window, cx| {
                            if theater {
                                view.exit_theater(cx);
                            } else {
                                view.open_fullscreen(window, cx);
                            }
                        }),
                    )),
            )
    }
}

impl Render for VideoPlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.poll_frame(window, cx);
        let theme = cx.theme();
        let has_frame = self.has_frame();
        let (playing, failed) = {
            let shared = self.shared.borrow();
            (shared.playing, shared.failed)
        };
        let mut root = div()
            .image_cache(self.image_cache.clone())
            .id("video-player")
            .group("video-player")
            .relative()
            .overflow_hidden()
            .bg(theme.bg_tertiary)
            .key_context(crate::VIDEO_PLAYER_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                view.on_key(event, window, cx);
            }));
        root = if self.layout == VideoLayout::FillContainer
            || (self.theater && self.fullscreen_mode == VideoFullscreenMode::InPlaceTheater)
        {
            root.w_full().h_full()
        } else if self.theater {
            let viewport = window.viewport_size();
            let (w, h) = theater_box(
                viewport.width * THEATER_FILL,
                viewport.height * THEATER_FILL,
                self.width,
                self.height,
            );
            root.w(w).h(h)
        } else {
            root.w(px(self.width))
                .h(px(self.height))
                .max_w_full()
                .max_h_full()
                .rounded_lg()
        };

        if self.load_state == VideoLoadState::Loading {
            let poster_fit = if self.layout == VideoLayout::FillContainer {
                ObjectFit::Contain
            } else {
                ObjectFit::Cover
            };
            return root
                .when(!self.poster.is_empty(), |d| {
                    d.child(img(self.poster.clone()).size_full().object_fit(poster_fit))
                })
                .into_any_element();
        }

        if self.load_state == VideoLoadState::Failed || failed || self.player.is_none() {
            let poster_fit = if self.layout == VideoLayout::FillContainer {
                ObjectFit::Contain
            } else {
                ObjectFit::Cover
            };
            return root
                .cursor_pointer()
                .when(!self.poster.is_empty(), |d| {
                    d.child(img(self.poster.clone()).size_full().object_fit(poster_fit))
                })
                .child(self.unplayable_card(cx))
                .on_click(cx.listener(|view, _, _window, cx| view.open_external(cx)))
                .into_any_element();
        }

        if playing && window.is_window_active() {
            window.request_animation_frame();
        }

        root.children(self.frame_child())
            .when(!has_frame && !self.poster.is_empty(), |d| {
                let poster_fit = if self.layout == VideoLayout::FillContainer {
                    ObjectFit::Contain
                } else {
                    ObjectFit::Cover
                };
                d.child(img(self.poster.clone()).size_full().object_fit(poster_fit))
            })
            .child(self.render_controls(cx))
            .child(self.download_button(cx))
            .into_any_element()
    }
}

fn theater_box(
    max_width: Pixels,
    max_height: Pixels,
    media_width: f32,
    media_height: f32,
) -> (Pixels, Pixels) {
    if media_width <= 0.0 || media_height <= 0.0 {
        return (max_width, max_height);
    }
    let scale = (f32::from(max_width) / media_width).min(f32::from(max_height) / media_height);
    (px(media_width * scale), px(media_height * scale))
}

fn control_button(
    id: &'static str,
    icon: IconName,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(CONTROL_TINT))
        .on_click(on_click)
        .child(Icon::new(icon).size(px(16.)).text_color(gpui::white()))
}

fn should_replay_from_start(playing: bool, current_time: f64, duration: f64) -> bool {
    !playing
        && duration > 0.0
        && (current_time >= duration - REPLAY_THRESHOLD_SECONDS || current_time / duration >= 0.98)
}

fn display_playhead(playing: bool, current_time: f64, duration: f64) -> f64 {
    if should_replay_from_start(playing, current_time, duration) {
        duration
    } else {
        current_time
    }
}

fn fraction_from_position(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    let width = bounds.size.width;
    if width <= px(0.0) {
        return 0.0;
    }
    ((x - bounds.left()) / width).clamp(0.0, 1.0)
}

fn whole_seconds(total: f64) -> u64 {
    if total.is_finite() && total > 0.0 {
        total as u64
    } else {
        0
    }
}

fn format_seconds(total: f64) -> String {
    let seconds = whole_seconds(total);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::{THEATER_FILL, theater_box};
    use gpui::px;

    #[test]
    fn a_portrait_video_gets_a_portrait_theater_box() {
        let (w, h) = theater_box(
            px(1920. * THEATER_FILL),
            px(1080. * THEATER_FILL),
            576.,
            1024.,
        );
        assert_eq!(h, px(1080. * THEATER_FILL));
        assert!(w < px(700.));
    }

    #[test]
    fn a_landscape_video_fills_the_available_width() {
        let (w, h) = theater_box(
            px(1920. * THEATER_FILL),
            px(1080. * THEATER_FILL),
            1920.,
            1080.,
        );
        assert_eq!(w, px(1920. * THEATER_FILL));
        assert_eq!(h, px(1080. * THEATER_FILL));
    }

    #[test]
    fn a_small_video_is_scaled_up_but_keeps_its_ratio() {
        let (w, h) = theater_box(px(1000.), px(1000.), 100., 200.);
        assert_eq!(w, px(500.));
        assert_eq!(h, px(1000.));
    }

    #[test]
    fn unknown_dimensions_fall_back_to_the_whole_box() {
        let (w, h) = theater_box(px(800.), px(600.), 0., 0.);
        assert_eq!(w, px(800.));
        assert_eq!(h, px(600.));
    }
}
