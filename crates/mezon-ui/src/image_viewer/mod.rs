use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::AsyncReadExt as _;
use gpui::Size as GpuiSize;
use gpui::http_client::HttpClient;
use gpui::{
    App, AppContext, BackgroundExecutor, Bounds, Context, Corners, Entity, FocusHandle, Focusable,
    FontWeight, ImageCache, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit,
    Pixels, Point, Render, RenderImage, Resource, ScrollDelta, ScrollWheelEvent, SharedString,
    SharedUri, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, canvas, div, img, point, prelude::*, px, relative, size,
    uniform_list,
};
use mezon_store::{
    AppConfig, ChannelAttachment, ChannelId, ChannelList, ClanId, DirectMessageStore, GalleryStore,
    PlatformStore, Settings, UploaderInfo, fetch_channel_attachments, resolve_attachment_uploader,
};

use crate::app::main_window::{activate_main_window, main_window_bounds};
use crate::app::title_bar::TitleBar;
use crate::app::window_controls::{self, APP_NAME};
use crate::chat::message::{VideoActivation, VideoFullscreenMode, VideoLayout, VideoPlayerView};
use crate::components::primitives::{Avatar, Icon, IconName, Spinner};
use crate::components::primitives::{ContextMenu, context_menu_at};
use crate::image_cache::{
    LruImageCache, VIEWER_IMAGE_CACHE_BYTES, VIEWER_IMAGE_CACHE_CAPACITY,
    VIEWER_IMAGE_ENTRY_MAX_BYTES,
};
use crate::theme::{ActiveTheme, Theme};

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 5.0;
const ZOOM_STEP: f32 = 0.25;
const THUMB_ROW_HEIGHT: f32 = 72.0;
const SIDEBAR_WIDTH: f32 = 96.0;
const VIEWER_FETCH_LIMIT: i32 = 50;
const LOAD_MORE_THRESHOLD: usize = 3;
const VIEWER_VIDEO_MAX_W: f32 = 900.0;
const VIEWER_VIDEO_MAX_H: f32 = 580.0;

pub struct OpenViewerRequest {
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub channel_label: SharedString,
    pub settings: Entity<Settings>,
    /// Pre-loaded playlist (gallery path). Empty for message-click, which fetches.
    pub attachments: Vec<ChannelAttachment>,
    pub selected_index: usize,
    /// Select this url once the playlist is loaded (message-click path).
    pub selected_url: Option<SharedString>,
    /// Fetch anchor (unix seconds, message create_time + 1 day) for message-click.
    pub anchor_before: Option<u32>,
}

struct GlobalImageViewer(WindowHandle<ImageViewer>);
impl gpui::Global for GlobalImageViewer {}

fn clear_image_viewer_global(cx: &mut App) {
    if cx.try_global::<GlobalImageViewer>().is_some() {
        cx.remove_global::<GlobalImageViewer>();
    }
}

/// static frame. Only GIFs are treated as animated
fn is_animated_image(att: &ChannelAttachment) -> bool {
    if !att.is_image {
        return false;
    }
    att.filetype.eq_ignore_ascii_case("image/gif")
        || att
            .url
            .rsplit('?')
            .next()
            .is_some_and(|path| path.to_ascii_lowercase().ends_with(".gif"))
}

/// Return freed heap pages back to the OS after tearing down the viewer.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn trim_process_memory() {
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn trim_process_memory() {}

fn close_image_viewer_window(handle: WindowHandle<ImageViewer>, cx: &mut App) {
    let _ = handle.update(cx, |viewer, window, cx| {
        viewer.release_resources(window, cx)
    });
    clear_image_viewer_global(cx);
    let _ = handle.update(cx, |_, window, _| window.remove_window());
    trim_process_memory();
}

fn prior_viewer_bounds(cx: &mut App) -> Option<Bounds<Pixels>> {
    let handle = cx.try_global::<GlobalImageViewer>().map(|g| g.0)?;
    match handle.update(cx, |_, window, _| window.window_bounds()) {
        Ok(WindowBounds::Windowed(bounds)) => Some(bounds),
        _ => None,
    }
}

/// Open the image viewer, replacing any existing viewer window.
pub fn open_image_viewer(request: OpenViewerRequest, cx: &mut App) {
    cx.defer(move |cx| open_image_viewer_now(request, cx));
}

fn open_image_viewer_now(request: OpenViewerRequest, cx: &mut App) {
    let prior_bounds = prior_viewer_bounds(cx);
    if let Some(handle) = cx.try_global::<GlobalImageViewer>().map(|g| g.0) {
        close_image_viewer_window(handle, cx);
        cx.defer(move |cx| spawn_image_viewer_window(request, prior_bounds, cx));
        return;
    }
    spawn_image_viewer_window(request, prior_bounds, cx);
}

fn default_viewer_bounds(cx: &mut App) -> Bounds<Pixels> {
    const MIN_W: f32 = 640.0;
    const MIN_H: f32 = 480.0;
    if let Some(main) = main_window_bounds(cx) {
        let w = (f32::from(main.size.width) * 0.8).max(MIN_W);
        let h = (f32::from(main.size.height) * 0.8).max(MIN_H);
        return Bounds::centered(None, size(px(w), px(h)), cx);
    }
    Bounds::centered(None, size(px(1100.0), px(740.0)), cx)
}

fn spawn_image_viewer_window(
    request: OpenViewerRequest,
    prior_bounds: Option<Bounds<Pixels>>,
    cx: &mut App,
) {
    let bounds = prior_bounds.unwrap_or_else(|| default_viewer_bounds(cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(640.0), px(480.0))),
        kind: WindowKind::Normal,
        focus: true,
        show: true,
        titlebar: Some(window_controls::window_title_options()),
        window_decorations: window_controls::main_window_decorations(),
        ..Default::default()
    };

    match cx.open_window(options, |window, cx| {
        cx.new(|cx| ImageViewer::new(request, window, cx))
    }) {
        Ok(handle) => cx.set_global(GlobalImageViewer(handle)),
        Err(e) => tracing::error!("failed to open image viewer window: {e}"),
    }
}

pub struct ImageViewer {
    focus_handle: FocusHandle,
    clan_id: ClanId,
    channel_id: ChannelId,
    channel_label: SharedString,
    title_bar: Entity<TitleBar>,
    settings: Entity<Settings>,
    attachments: Vec<ChannelAttachment>,
    index: usize,
    zoom: f32,
    pan: Point<Pixels>,
    drag_from: Option<Point<Pixels>>,
    show_thumbnails: bool,
    context_menu: Option<Point<Pixels>>,
    has_more_before: bool,
    has_more_after: bool,
    loading: bool,
    rotation_deg: i32,
    rotated_image: Option<Arc<RenderImage>>,
    rotation_loading: bool,
    image_cache: Entity<LruImageCache>,
    list_scroll: UniformListScrollHandle,
    active_video: Option<Entity<VideoPlayerView>>,
    active_video_url: Option<String>,
    session: u64,
    list_generation: u64,
    video_sync_token: Option<(u64, usize)>,
    video_sync_scheduled: bool,
    last_trim: Option<Instant>,
    closing: bool,
    _release: Subscription,
}

impl ImageViewer {
    fn new(request: OpenViewerRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, app| {
            let _ = weak.update(app, |viewer, cx| {
                viewer.release_resources(window, cx);
            });
            clear_image_viewer_global(app);
            activate_main_window(app);
            true
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::viewer(
                "viewer",
                VIEWER_IMAGE_CACHE_CAPACITY,
                VIEWER_IMAGE_CACHE_BYTES,
                VIEWER_IMAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let cache_for_release = image_cache.clone();
        let release = cx.on_release(move |viewer, cx| {
            viewer.session = viewer.session.wrapping_add(1);
            viewer.list_generation = viewer.list_generation.wrapping_add(1);
            if let Some(view) = viewer.active_video.take() {
                view.update(cx, |view, cx| view.shutdown(None, cx));
                drop(view);
            }
            viewer.active_video_url = None;
            viewer.clear_rotated_image(None, cx);
            viewer.rotation_loading = false;
            viewer.attachments.clear();
            cache_for_release.update(cx, |cache, cx| cache.clear_app(cx));
            trim_process_memory();
        });
        let channel_label = resolve_channel_label(
            request.clan_id,
            request.channel_id,
            request.channel_label.clone(),
            cx,
        );
        let title_bar = cx.new(|cx| TitleBar::new(request.settings.clone(), cx));
        let mut this = Self {
            focus_handle,
            clan_id: request.clan_id,
            channel_id: request.channel_id,
            channel_label,
            title_bar,
            settings: request.settings.clone(),
            attachments: Vec::new(),
            index: 0,
            zoom: MIN_ZOOM,
            pan: point(px(0.), px(0.)),
            drag_from: None,
            show_thumbnails: true,
            context_menu: None,
            has_more_before: true,
            has_more_after: false,
            loading: false,
            rotation_deg: 0,
            rotated_image: None,
            rotation_loading: false,
            image_cache,
            list_scroll: UniformListScrollHandle::new(),
            active_video: None,
            active_video_url: None,
            session: 0,
            list_generation: 0,
            video_sync_token: None,
            video_sync_scheduled: false,
            last_trim: None,
            closing: false,
            _release: release,
        };
        this.set_request(request, window, cx);
        this
    }

    fn release_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.session = self.session.wrapping_add(1);
        self.list_generation = self.list_generation.wrapping_add(1);
        self.loading = false;
        self.context_menu = None;
        self.drop_video_player(Some(window), cx);
        self.clear_rotated_image(Some(&mut *window), cx);
        self.rotation_loading = false;
        self.rotation_deg = 0;
        self.video_sync_token = None;
        self.video_sync_scheduled = false;
        self.attachments.clear();
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        trim_process_memory();
    }

    fn clear_rotated_image(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if let Some(old) = self.rotated_image.take() {
            cx.drop_image(old, window);
        }
    }

    fn trim_memory_throttled(&mut self) {
        const MIN_TRIM_INTERVAL: Duration = Duration::from_millis(300);
        let now = Instant::now();
        if self
            .last_trim
            .is_none_or(|last| now.duration_since(last) >= MIN_TRIM_INTERVAL)
        {
            self.last_trim = Some(now);
            trim_process_memory();
        }
    }

    fn drop_video_player(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let had_player = self.active_video.is_some();
        if let Some(view) = self.active_video.take() {
            match window {
                Some(window) => {
                    view.update(cx, |view, cx| view.shutdown(Some(window), cx));
                }
                None => {
                    view.update(cx, |view, cx| view.shutdown(None, cx));
                }
            }
            drop(view);
        }
        self.active_video_url = None;
        self.video_sync_token = None;
        if had_player {
            self.trim_memory_throttled();
        }
    }

    fn ensure_video_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(att) = self.current().filter(|a| a.is_video) else {
            self.drop_video_player(Some(window), cx);
            self.video_sync_token = Some((self.session, self.index));
            return;
        };
        let url = att.url.clone();
        if self.active_video_url.as_deref() == Some(url.as_str()) {
            self.video_sync_token = Some((self.session, self.index));
            return;
        }
        let (width, height) = video_display_size(att);
        let activation = VideoActivation {
            url: url.clone().into(),
            poster: SharedString::default(),
            width,
            height,
            fullscreen_mode: VideoFullscreenMode::InPlaceTheater,
            layout: VideoLayout::FillContainer,
            decode_max_size: Some(video_decode_max_size(window)),
        };
        if let Some(view) = self.active_video.clone() {
            view.update(cx, |player, cx| player.reopen(activation, window, cx));
        } else {
            self.active_video = Some(cx.new(|cx| VideoPlayerView::new(activation, window, cx)));
        }
        self.active_video_url = Some(url);
        self.video_sync_token = Some((self.session, self.index));
    }

    fn schedule_video_player_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || self.video_sync_scheduled {
            return;
        }
        let token = (self.session, self.index);
        if self.video_sync_token == Some(token) {
            return;
        }
        self.video_sync_scheduled = true;
        let session = self.session;
        let index = self.index;
        cx.defer_in(window, move |this, window, cx| {
            this.video_sync_scheduled = false;
            if this.closing || this.session != session || this.index != index {
                return;
            }
            let token = (session, index);
            if this.video_sync_token == Some(token) {
                return;
            }
            this.ensure_video_player(window, cx);
        });
    }

    fn set_request(
        &mut self,
        request: OpenViewerRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session = self.session.wrapping_add(1);
        self.list_generation = self.list_generation.wrapping_add(1);
        self.clan_id = request.clan_id;
        self.channel_id = request.channel_id;
        self.channel_label = resolve_channel_label(
            request.clan_id,
            request.channel_id,
            request.channel_label,
            cx,
        );
        self.settings = request.settings;
        self.zoom = MIN_ZOOM;
        self.pan = point(px(0.), px(0.));
        self.rotation_deg = 0;
        self.clear_rotated_image(Some(&mut *window), cx);
        self.rotation_loading = false;
        self.context_menu = None;
        self.drop_video_player(Some(window), cx);
        self.video_sync_token = None;
        self.attachments = request.attachments;
        self.has_more_before = true;
        self.has_more_after = false;

        if let Some(url) = &request.selected_url {
            self.index = self
                .attachments
                .iter()
                .position(|a| a.viewer_src == *url || a.url.as_str() == url.as_ref())
                .unwrap_or(0);
        } else {
            self.index = request
                .selected_index
                .min(self.attachments.len().saturating_sub(1));
        }

        if self.attachments.is_empty() {
            self.loading = true;
            self.fetch_initial(request.anchor_before, request.selected_url, cx);
        } else {
            self.loading = false;
            resolve_uploaders(&mut self.attachments, self.clan_id, self.channel_id, cx);
            self.schedule_video_player_sync(window, cx);
        }
        cx.notify();
    }

    fn clan(&self) -> ClanId {
        self.clan_id
    }

    fn channel(&self) -> ChannelId {
        self.channel_id
    }

    fn current(&self) -> Option<&ChannelAttachment> {
        self.attachments.get(self.index)
    }

    fn fetch_initial(
        &mut self,
        anchor_before: Option<u32>,
        select_url: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let (Some(gallery), Some(cfg)) = (
            GalleryStore::try_global(cx),
            AppConfig::try_global(cx).cloned(),
        ) else {
            self.loading = false;
            return;
        };
        let api = gallery.read(cx).api();
        let clan = self.clan();
        let channel = self.channel();
        let before = anchor_before.unwrap_or(0);
        let generation = self.list_generation;
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_attachments(api, cfg, clan, channel, before, 0, VIEWER_FETCH_LIMIT)
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(mut mapped) => {
                        resolve_uploaders(&mut mapped, clan, channel, cx);
                        this.attachments = mapped;
                        this.has_more_before = this.attachments.len() as i32 >= VIEWER_FETCH_LIMIT;
                        if let Some(url) = &select_url {
                            this.index = this
                                .attachments
                                .iter()
                                .position(|a| a.url.as_str() == url.as_ref())
                                .unwrap_or(0);
                        }
                        this.video_sync_token = None;
                        this.has_more_after = true;
                        this.fetch_newer(cx);
                    }
                    Err(e) => tracing::error!("image viewer fetch failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_newer(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.has_more_after {
            return;
        }
        let Some(newest) = self.attachments.first() else {
            return;
        };
        let after = newest.create_time_seconds;
        let (Some(gallery), Some(cfg)) = (
            GalleryStore::try_global(cx),
            AppConfig::try_global(cx).cloned(),
        ) else {
            return;
        };
        let api = gallery.read(cx).api();
        let clan = self.clan();
        let channel = self.channel();
        self.loading = true;
        let generation = self.list_generation;
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_attachments(api, cfg, clan, channel, 0, after, VIEWER_FETCH_LIMIT)
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(mut mapped) => {
                        resolve_uploaders(&mut mapped, clan, channel, cx);
                        let existing: std::collections::HashSet<i64> =
                            this.attachments.iter().map(|a| a.id).collect();
                        let fresh: Vec<ChannelAttachment> = mapped
                            .into_iter()
                            .filter(|a| !existing.contains(&a.id))
                            .collect();
                        let added = fresh.len();
                        if added > 0 {
                            let selected_id = this.attachments.get(this.index).map(|a| a.id);
                            let mut merged = fresh;
                            merged.extend(std::mem::take(&mut this.attachments));
                            merged.sort_by_key(|b| std::cmp::Reverse(b.create_time_seconds));
                            this.index = selected_id
                                .and_then(|id| merged.iter().position(|a| a.id == id))
                                .unwrap_or(this.index);
                            this.attachments = merged;
                            this.video_sync_token = None;
                        }
                        this.has_more_after = added as i32 >= VIEWER_FETCH_LIMIT;
                        cx.notify();
                    }
                    Err(e) => tracing::error!("image viewer newer fetch failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn fetch_older(&mut self, cx: &mut Context<Self>) {
        if self.loading || !self.has_more_before {
            return;
        }
        let Some(oldest) = self.attachments.last() else {
            return;
        };
        let before = oldest.create_time_seconds;
        let (Some(gallery), Some(cfg)) = (
            GalleryStore::try_global(cx),
            AppConfig::try_global(cx).cloned(),
        ) else {
            return;
        };
        let api = gallery.read(cx).api();
        let clan = self.clan();
        let channel = self.channel();
        self.loading = true;
        let generation = self.list_generation;
        cx.spawn(async move |this, cx| {
            let result =
                fetch_channel_attachments(api, cfg, clan, channel, before, 0, VIEWER_FETCH_LIMIT)
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.list_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(mut mapped) => {
                        resolve_uploaders(&mut mapped, clan, channel, cx);
                        let existing: std::collections::HashSet<i64> =
                            this.attachments.iter().map(|a| a.id).collect();
                        let before_len = this.attachments.len();
                        for att in mapped {
                            if !existing.contains(&att.id) {
                                this.attachments.push(att);
                            }
                        }
                        let added = this.attachments.len() - before_len;
                        this.has_more_before = added > 0;
                        cx.notify();
                    }
                    Err(e) => tracing::error!("image viewer page fetch failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn refresh_uploader_at(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(att) = self.attachments.get(index) else {
            return;
        };
        if !att.uploader_avatar.is_empty() {
            return;
        }
        let clan = self.clan_id;
        let channel = self.channel_id;
        let uploader_id = att.uploader_id;
        let message_id = att.message_id;
        let cfg = AppConfig::try_global(cx);
        let info = resolve_attachment_uploader(clan, channel, uploader_id, message_id, cfg, cx);
        if let Some(att) = self.attachments.get_mut(index) {
            apply_uploader_info(att, info);
        }
    }

    fn go_to(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.attachments.len() || index == self.index {
            return;
        }
        self.session = self.session.wrapping_add(1);
        self.video_sync_token = None;
        self.video_sync_scheduled = false;
        self.index = index;
        self.zoom = MIN_ZOOM;
        self.pan = point(px(0.), px(0.));
        self.rotation_deg = 0;
        self.clear_rotated_image(Some(&mut *window), cx);
        self.rotation_loading = false;
        self.context_menu = None;
        self.refresh_uploader_at(index, cx);
        self.schedule_video_player_sync(window, cx);
        if self.index + LOAD_MORE_THRESHOLD >= self.attachments.len() {
            self.fetch_older(cx);
        }
        if self.index <= LOAD_MORE_THRESHOLD {
            self.fetch_newer(cx);
        }
        self.list_scroll
            .scroll_to_item(self.index, gpui::ScrollStrategy::Center);
        cx.notify();
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.index + 1 < self.attachments.len() {
            self.go_to(self.index + 1, window, cx);
        }
    }

    fn prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.index > 0 {
            self.go_to(self.index - 1, window, cx);
        }
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (self.zoom - MIN_ZOOM).abs() < f32::EPSILON {
            self.pan = point(px(0.), px(0.));
        }
        cx.notify();
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(MIN_ZOOM, cx);
    }

    fn rotate_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rotation_deg = (self.rotation_deg - 90).rem_euclid(360);
        self.pan = point(px(0.), px(0.));
        self.refresh_rotation(window, cx);
    }

    fn rotate_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rotation_deg = (self.rotation_deg + 90).rem_euclid(360);
        self.pan = point(px(0.), px(0.));
        self.refresh_rotation(window, cx);
    }

    fn refresh_rotation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rotation_deg == 0 {
            self.clear_rotated_image(Some(&mut *window), cx);
            self.rotation_loading = false;
            cx.notify();
            return;
        }
        let Some(att) = self.current().filter(|a| a.is_image) else {
            self.rotation_deg = 0;
            self.clear_rotated_image(Some(&mut *window), cx);
            self.rotation_loading = false;
            cx.notify();
            return;
        };
        let url = att.viewer_src.to_string();
        let degrees = self.rotation_deg;
        self.rotation_loading = true;
        self.session = self.session.wrapping_add(1);
        let session = self.session;
        let client = cx.http_client();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = fetch_rotated_image(&url, degrees, client, executor).await;
            let _ = this.update(cx, |this, cx| {
                if this.session != session {
                    return;
                }
                this.rotation_loading = false;
                match result {
                    Ok(image) => {
                        this.clear_rotated_image(None, cx);
                        this.rotated_image = Some(image);
                    }
                    Err(e) => {
                        tracing::warn!("image rotation failed: {e}");
                        this.rotation_deg = 0;
                        this.clear_rotated_image(None, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_resources(window, cx);
        clear_image_viewer_global(cx);
        activate_main_window(cx);
        window.remove_window();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.context_menu.take().is_some() {
                    cx.notify();
                } else {
                    self.close_window(window, cx);
                }
            }
            "left" | "up" => self.prev(window, cx),
            "right" | "down" => self.next(window, cx),
            "+" | "=" => self.set_zoom(self.zoom + ZOOM_STEP, cx),
            "-" | "_" => self.set_zoom(self.zoom - ZOOM_STEP, cx),
            "0" => self.reset_zoom(cx),
            _ => {}
        }
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dy = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
        };
        if dy.abs() > f32::EPSILON {
            self.set_zoom(self.zoom + dy * ZOOM_STEP, cx);
        }
    }

    fn copy_link(&self, cx: &mut App) {
        if let Some(att) = self.current() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(att.url.clone()));
        }
    }

    fn open_in_browser(&self, cx: &mut App) {
        if let (Some(att), Some(platform)) = (self.current(), PlatformStore::try_global(cx)) {
            let url = att.url.clone();
            let _ = platform.read(cx).open_url_external(&url);
        }
    }

    fn save_image(&mut self, cx: &mut Context<Self>) {
        let Some(att) = self.current() else {
            return;
        };
        mezon_store::download_url_with_dialog(
            SharedString::from(att.url.clone()),
            SharedString::from(att.filename.clone()),
            cx,
        );
    }
}

fn apply_uploader_info(att: &mut ChannelAttachment, info: UploaderInfo) {
    if info.name.is_empty() {
        att.uploader_name = "Anonymous".into();
        att.uploader_avatar = SharedString::default();
        att.uploader_avatar_raw = SharedString::default();
    } else {
        att.uploader_name = info.name.into();
        att.uploader_avatar = info.avatar.into();
        att.uploader_avatar_raw = info.avatar_raw.into();
    }
}

fn resolve_uploaders(
    attachments: &mut [ChannelAttachment],
    clan: ClanId,
    channel: ChannelId,
    cx: &App,
) {
    let cfg = AppConfig::try_global(cx);
    for att in attachments.iter_mut() {
        let info =
            resolve_attachment_uploader(clan, channel, att.uploader_id, att.message_id, cfg, cx);
        apply_uploader_info(att, info);
    }
}

pub fn resolve_channel_label(
    clan_id: ClanId,
    channel_id: ChannelId,
    hint: SharedString,
    cx: &App,
) -> SharedString {
    if !hint.is_empty() {
        return hint;
    }
    if clan_id.0 != 0 {
        ChannelList::try_global(cx)
            .and_then(|store| {
                store
                    .read(cx)
                    .channel(clan_id, channel_id)
                    .map(|ch| ch.name.clone())
            })
            .map(SharedString::from)
            .unwrap_or_default()
    } else {
        DirectMessageStore::try_global(cx)
            .and_then(|store| store.read(cx).find(channel_id).map(|dm| dm.label.clone()))
            .map(SharedString::from)
            .unwrap_or_default()
    }
}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.closing {
            self.image_cache
                .update(cx, |cache, cx| cache.sweep(window, cx));
            self.schedule_video_player_sync(window, cx);
        }
        let theme = cx.theme().clone();
        let locale = self.locale(cx);

        div()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("ImageViewer")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg_tertiary)
            .text_color(theme.text_primary)
            .on_key_down(cx.listener(Self::on_key_down))
            .when(window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(self.title_bar.clone())
            })
            .when(!window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(self.render_app_title_strip(&theme))
            })
            .when(!self.channel_label.is_empty(), |el| {
                el.child(self.render_channel_header(&theme))
            })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .min_h_0()
                    .child(self.render_main_area(&theme, &locale, cx))
                    .when(self.show_thumbnails, |el| {
                        el.child(self.render_sidebar(&theme, cx))
                    }),
            )
            .child(self.render_bottom_bar(&theme, &locale, window, cx))
            .when_some(self.context_menu, |el, pos| {
                el.child(context_menu_at(pos, self.build_context_menu(&locale, cx)))
            })
            .when(window_controls::is_edge_resizable(), |el| {
                el.child(window_controls::render_resize_edges(window))
            })
    }
}

impl ImageViewer {
    fn render_app_title_strip(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .h_8()
            .w_full()
            .flex()
            .items_center()
            .bg(theme.title_bar_bg)
            .child(
                div()
                    .px_3()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(APP_NAME),
            )
    }

    fn render_channel_header(&self, theme: &Theme) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .h(px(30.))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.bg_secondary)
            .text_color(theme.text_primary)
            .text_size(px(13.))
            .child(self.channel_label.clone())
    }

    fn render_main_area(
        &self,
        theme: &Theme,
        _locale: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = if self.loading && self.attachments.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element()
        } else if let Some(att) = self.current() {
            if att.is_video {
                self.render_video()
            } else if is_animated_image(att) {
                self.render_animated_image(att)
            } else {
                self.render_image(att, cx)
            }
        } else {
            div().size_full().into_any_element()
        };

        let can_prev = self.index > 0;
        let can_next = self.index + 1 < self.attachments.len();

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .image_cache(self.image_cache.clone())
            .child(content)
            .when(can_prev, |el| {
                el.child(nav_button(IconName::ArrowLeft, theme, true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| this.prev(window, cx)),
                ))
            })
            .when(can_next, |el| {
                el.child(
                    nav_button(IconName::ChevronRight, theme, false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| this.next(window, cx)),
                    ),
                )
            })
    }

    /// Render an animated GIF by playing the original file frame-by-frame.
    fn render_animated_image(&self, att: &ChannelAttachment) -> gpui::AnyElement {
        let url = SharedUri::from(&att.url);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(
                img(url)
                    .image_cache(&self.image_cache)
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .id("viewer-animated-image"),
            )
            .into_any_element()
    }

    fn render_image(&self, att: &ChannelAttachment, cx: &mut Context<Self>) -> gpui::AnyElement {
        let zoom = self.zoom;
        let pan = self.pan;
        let rotated_image = self.rotated_image.clone();
        let rotation_loading = self.rotation_loading;
        let viewer_src = att.viewer_src.clone();
        let image_cache = self.image_cache.clone();

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .when(rotation_loading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Spinner::new()),
                )
            })
            .child(
                canvas(
                    move |_bounds, window, cx| {
                        if rotation_loading {
                            return None;
                        }
                        if let Some(image) = rotated_image {
                            return Some(image);
                        }
                        let resource = Resource::Uri(SharedUri::from(viewer_src.as_ref()));
                        image_cache.update(cx, |cache, cx| {
                            ImageCache::load(cache, &resource, window, cx).and_then(|r| r.ok())
                        })
                    },
                    move |bounds, image, window, _cx| {
                        let Some(image) = image else {
                            return;
                        };
                        let raw = image.size(0);
                        let content = size(px(raw.width.0 as f32), px(raw.height.0 as f32));
                        let fitted = fit_contain(bounds.size, content);
                        if fitted.width <= px(0.) || fitted.height <= px(0.) {
                            return;
                        }
                        let w = px(f32::from(fitted.width) * zoom);
                        let h = px(f32::from(fitted.height) * zoom);
                        let x = bounds.origin.x + (bounds.size.width - w) / 2.0 + pan.x;
                        let y = bounds.origin.y + (bounds.size.height - h) / 2.0 + pan.y;
                        let _ = window.paint_image(
                            Bounds::from_corners(point(x, y), point(x + w, y + h)),
                            Corners::default(),
                            image,
                            0,
                            false,
                        );
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if ev.click_count == 2 {
                        if this.zoom > MIN_ZOOM {
                            this.reset_zoom(cx);
                        } else {
                            this.set_zoom(2.0, cx);
                        }
                    } else if this.zoom > MIN_ZOOM {
                        this.drag_from = Some(ev.position);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(from) = this.drag_from {
                    this.pan.x += ev.position.x - from.x;
                    this.pan.y += ev.position.y - from.y;
                    this.drag_from = Some(ev.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, _| {
                    this.drag_from = None;
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.context_menu = Some(ev.position);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .when(zoom > MIN_ZOOM, |el| el.cursor(gpui::CursorStyle::OpenHand))
            .into_any_element()
    }

    fn render_video(&self) -> gpui::AnyElement {
        let Some(view) = &self.active_video else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element();
        };
        div()
            .size_full()
            .min_w_0()
            .child(view.clone())
            .into_any_element()
    }

    fn render_sidebar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.attachments.len();
        let entity = cx.entity();
        let active = self.index;
        let cache = self.image_cache.clone();
        let border = theme.border;
        let brand = theme.brand;
        let bg_tertiary = theme.bg_tertiary;

        let list = uniform_list("viewer-thumbnails", count, move |range, _window, cx| {
            range
                .map(|ix| {
                    let info = entity
                        .read(cx)
                        .attachments
                        .get(ix)
                        .map(|att| (att.is_video, SharedUri::from(&att.thumb_src)));
                    let Some((is_video, src)) = info else {
                        return div().into_any_element();
                    };
                    let is_active = ix == active;
                    let media = if is_video {
                        div().size_full().bg(bg_tertiary).into_any_element()
                    } else {
                        img(src)
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover)
                            .id(("viewer-thumb-img", ix))
                            .into_any_element()
                    };
                    div()
                        .id(("viewer-thumb", ix))
                        .h(px(THUMB_ROW_HEIGHT))
                        .w_full()
                        .p_1()
                        .child(
                            div()
                                .size_full()
                                .rounded(px(6.))
                                .overflow_hidden()
                                .border_2()
                                .border_color(if is_active { brand } else { gpui::rgba(0) })
                                .relative()
                                .child(media)
                                .when(is_video, |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(IconName::PlayButton)
                                                    .size(px(16.))
                                                    .text_color(gpui::white()),
                                            ),
                                    )
                                }),
                        )
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, window, cx| {
                                entity.update(cx, |this, cx| this.go_to(ix, window, cx));
                            }
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.bg_secondary)
            .border_l_1()
            .border_color(border)
            .image_cache(cache)
            .child(list)
    }

    fn render_bottom_bar(
        &self,
        theme: &Theme,
        locale: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let att = self.current();
        let uploader = att.map(|a| a.uploader_name.clone()).unwrap_or_default();
        let avatar_widget = {
            let mut avatar = Avatar::new()
                .name(uploader.clone())
                .size_px(px(32.))
                .image_cache(self.image_cache.clone());
            if let Some(att) = att {
                let proxied = att.uploader_avatar.clone();
                let raw = att.uploader_avatar_raw.clone();
                if !proxied.is_empty() {
                    let use_fallback = !raw.is_empty() && raw.as_ref() != proxied.as_ref();
                    avatar = avatar.src(proxied);
                    if use_fallback {
                        avatar = avatar.fallback_src(raw);
                    }
                } else if !raw.is_empty() {
                    avatar = avatar.src(raw);
                }
            }
            avatar
        };
        let timestamp = att.map(|a| a.viewer_timestamp.clone()).unwrap_or_default();
        let counter = if self.attachments.is_empty() {
            SharedString::default()
        } else {
            format!("{} / {}", self.index + 1, self.attachments.len()).into()
        };
        let _ = locale;

        let is_image = self.current().is_some_and(|a| a.is_image);

        let user_block = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(avatar_widget)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(div().text_size(px(13.)).child(uploader))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(timestamp),
                    ),
            );

        div()
            .flex_shrink_0()
            .h(px(56.))
            .w_full()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .bg(theme.bg_secondary)
            .border_t_1()
            .border_color(theme.border)
            .child(user_block)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .child(counter),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(tool_button(IconName::Download, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| this.save_image(cx)),
                    ))
                    .when(is_image, |el| {
                        el.child(tool_divider(theme))
                            .child(tool_button(IconName::RotateLeftIcon, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.rotate_left(window, cx)
                                }),
                            ))
                            .child(tool_button(IconName::RotateRightIcon, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.rotate_right(window, cx)
                                }),
                            ))
                            .child(tool_divider(theme))
                            .child(tool_button(IconName::MinusCircleIcon, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    this.set_zoom(this.zoom - ZOOM_STEP, cx)
                                }),
                            ))
                            .child(tool_button(IconName::Plus, theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    this.set_zoom(this.zoom + ZOOM_STEP, cx)
                                }),
                            ))
                    })
                    .child(tool_button(IconName::ImageThumbnail, theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.show_thumbnails = !this.show_thumbnails;
                            cx.notify();
                        }),
                    )),
            )
    }

    fn build_context_menu(&self, locale: &str, cx: &Context<Self>) -> ContextMenu {
        let entity = cx.entity();
        let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
        let dismiss = {
            let entity = entity.downgrade();
            move |_window: &mut Window, cx: &mut App| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        this.context_menu = None;
                        cx.notify();
                    });
                }
            }
        };
        ContextMenu::new()
            .on_dismiss(dismiss)
            .item_icon(t("contextMenu.copyLink"), IconName::CopyIcon, {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.copy_link(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
            .item_icon(t("contextMenu.saveImage"), IconName::Download, {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.save_image(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
            .separator()
            .item(t("contextMenu.openLink"), {
                let entity = entity.downgrade();
                move |_w, cx| {
                    if let Some(this) = entity.upgrade() {
                        this.update(cx, |this, cx| {
                            this.open_in_browser(cx);
                            this.context_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
    }
}

fn nav_button(icon: IconName, theme: &Theme, left: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(if left { "viewer-prev" } else { "viewer-next" })
        .absolute()
        .top(relative(0.5))
        .when(left, |el| el.left(px(16.)))
        .when(!left, |el| el.right(px(16.)))
        .size(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(gpui::hsla(0., 0., 0., 0.5))
        .cursor_pointer()
        .hover(|el| el.bg(gpui::hsla(0., 0., 0., 0.7)))
        .child(Icon::new(icon).size(px(20.)).text_color(theme.text_primary))
}

fn tool_button(icon: IconName, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("viewer-tool", icon as usize))
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|el| el.bg(theme.bg_hover))
        .child(
            Icon::new(icon)
                .size(px(18.))
                .text_color(theme.text_secondary),
        )
}

fn tool_divider(theme: &Theme) -> impl IntoElement {
    div().w(px(1.)).h(px(20.)).mx_1().bg(theme.border)
}

async fn fetch_rotated_image(
    url: &str,
    degrees: i32,
    client: Arc<dyn HttpClient>,
    executor: BackgroundExecutor,
) -> anyhow::Result<Arc<RenderImage>> {
    if !url.starts_with("https://") {
        anyhow::bail!("rotation fetch rejected: only https scheme is allowed");
    }
    let mut response = client.get(url, ().into(), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("rotation fetch failed with status {}", response.status());
    }
    let mut bytes = Vec::new();
    response.body_mut().read_to_end(&mut bytes).await?;
    executor
        .spawn(async move {
            let decoded = image::load_from_memory(&bytes)?;
            let rotated = match degrees.rem_euclid(360) {
                90 => decoded.rotate90(),
                180 => decoded.rotate180(),
                270 => decoded.rotate270(),
                _ => decoded,
            };
            let mut rgba = rotated.to_rgba8();
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            anyhow::Ok(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
        })
        .await
}

fn fit_contain(container: GpuiSize<Pixels>, content: GpuiSize<Pixels>) -> GpuiSize<Pixels> {
    let cw = f32::from(container.width);
    let ch = f32::from(container.height);
    let iw = f32::from(content.width);
    let ih = f32::from(content.height);
    if iw <= f32::EPSILON || ih <= f32::EPSILON {
        return size(px(0.), px(0.));
    }
    let scale = (cw / iw).min(ch / ih);
    size(px(iw * scale), px(ih * scale))
}

fn video_decode_max_size(window: &Window) -> (u32, u32) {
    let viewport = window.viewport_size();
    let w = f32::from(viewport.width).clamp(1.0, VIEWER_VIDEO_MAX_W) as u32;
    let h = f32::from(viewport.height).clamp(1.0, VIEWER_VIDEO_MAX_H) as u32;
    (w, h)
}

fn video_display_size(att: &ChannelAttachment) -> (f32, f32) {
    let mut w = att.width as f32;
    let mut h = att.height as f32;
    if w <= f32::EPSILON || h <= f32::EPSILON {
        return (VIEWER_VIDEO_MAX_W, VIEWER_VIDEO_MAX_H);
    }
    let scale = (VIEWER_VIDEO_MAX_W / w)
        .min(VIEWER_VIDEO_MAX_H / h)
        .min(1.0);
    w *= scale;
    h *= scale;
    (w, h)
}
