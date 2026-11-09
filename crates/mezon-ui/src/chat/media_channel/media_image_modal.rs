use std::sync::Arc;

use gpui::http_client::HttpClient;
use gpui::{
    AnyView, App, AppContext, Bounds, Context, Corners, DisplayId, Entity, FocusHandle, Focusable,
    ImageCache, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, Pixels,
    Point, Render, RenderImage, Resource, ScrollDelta, ScrollWheelEvent, SharedString, SharedUri,
    Size as GpuiSize, StyleRefinement, Subscription, UniformListScrollHandle, Window, WindowBounds,
    WindowHandle, WindowOptions, canvas, div, img, point, prelude::*, px, size, uniform_list,
};
use mezon_store::{AppConfig, ChannelTimelineAttachment, Settings};
use ui::{ScrollAxes, Scrollbars, WithScrollbar};

use crate::app::main_window::{
    activate_main_window, apply_overlay_bounds, handle as main_window_handle,
    overlay_placement_from_window, overlay_window_kind, sync_overlay_to_main,
};
use crate::app::title_bar::TitleBar;
use crate::app::window_controls;
use crate::components::primitives::{Icon, IconName, Spinner};
use crate::image_cache::{
    GALLERY_IMAGE_CACHE_BYTES, GALLERY_IMAGE_CACHE_CAPACITY, LruImageCache, SHARED_ENTRY_MAX_BYTES,
    VIEWER_IMAGE_CACHE_BYTES, VIEWER_IMAGE_CACHE_CAPACITY, VIEWER_IMAGE_ENTRY_MAX_BYTES,
};
use crate::theme::ActiveTheme;

use super::media_image::render_media_image;

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 5.0;
const ZOOM_WHEEL_STEP: f32 = 0.05;
const ZOOM_BUTTON_STEP: f32 = 1.5;
const THUMB_SIDEBAR_WIDTH: f32 = 100.;
const THUMB_SCROLLBAR_GUTTER: f32 = 14.;
const THUMB_SIDEBAR_BG: u32 = 0x1a1a1a;
const THUMB_ROW_HEIGHT: f32 = 88.;

fn uploaded_attachment_index(
    attachments: &[ChannelTimelineAttachment],
    attachment_index: usize,
) -> Option<usize> {
    attachments
        .get(attachment_index)
        .filter(|a| a.is_uploaded())?;
    Some(
        attachments
            .iter()
            .take(attachment_index + 1)
            .filter(|a| a.is_uploaded())
            .count()
            .saturating_sub(1),
    )
}

fn is_video_attachment(att: &ChannelTimelineAttachment) -> bool {
    if att.file_type.starts_with("video/") {
        return true;
    }
    let url = att.file_url.to_ascii_lowercase();
    if url.contains(".avif") {
        return false;
    }
    [".mp4", ".webm", ".ogg", ".mov", ".avi", ".mkv", ".m4v"]
        .iter()
        .any(|ext| url.contains(ext))
}

struct GlobalMediaImageModal(WindowHandle<MediaImageModal>);
impl gpui::Global for GlobalMediaImageModal {}

fn clear_media_image_modal_global(cx: &mut App) {
    if cx.try_global::<GlobalMediaImageModal>().is_some() {
        cx.remove_global::<GlobalMediaImageModal>();
    }
}

pub fn close_media_image_modal(cx: &mut App) {
    let Some(handle) = cx.try_global::<GlobalMediaImageModal>().map(|g| g.0) else {
        return;
    };
    let _ = handle.update(cx, |modal, window, cx| {
        modal.release_resources(window, cx);
        window.remove_window();
    });
    clear_media_image_modal_global(cx);
}

pub fn open_media_image_modal(
    attachments: Vec<ChannelTimelineAttachment>,
    attachment_index: usize,
    window: &mut Window,
    cx: &mut App,
) {
    let uploaded: Vec<_> = attachments
        .iter()
        .filter(|a| a.is_uploaded())
        .cloned()
        .collect();
    if uploaded.is_empty() {
        return;
    }
    let Some(index) = uploaded_attachment_index(&attachments, attachment_index) else {
        return;
    };
    let Some(settings) = Settings::try_global(cx) else {
        return;
    };

    let placement = overlay_placement_from_window(window, cx);
    let mut pending = Some((uploaded, index));
    if let Some(handle) = cx.try_global::<GlobalMediaImageModal>().map(|g| g.0) {
        let _ = handle.update(cx, |modal, window, cx| {
            if let Some((uploaded, index)) = pending.take() {
                apply_overlay_bounds(window, placement.0);
                window.activate_window();
                window.focus(&modal.focus_handle, cx);
                modal.set_attachments(uploaded, index, window, cx);
            }
        });
        if pending.is_none() {
            return;
        }
        clear_media_image_modal_global(cx);
        let Some((uploaded, index)) = pending else {
            return;
        };
        spawn_media_image_modal_window(uploaded, index, settings, placement, cx);
        return;
    }

    let Some((uploaded, index)) = pending else {
        return;
    };

    spawn_media_image_modal_window(uploaded, index, settings, placement, cx);
}

fn spawn_media_image_modal_window(
    uploaded: Vec<ChannelTimelineAttachment>,
    index: usize,
    settings: Entity<Settings>,
    placement: (WindowBounds, Option<DisplayId>),
    cx: &mut App,
) {
    let main_app = main_window_handle(cx);
    let (window_bounds, display_id) = placement;
    let kind = overlay_window_kind();
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(640.0), px(480.0))),
        kind,
        focus: true,
        show: true,
        is_movable: true,
        display_id,
        titlebar: Some(window_controls::window_title_options()),
        window_decorations: window_controls::main_window_decorations(),
        app_id: window_controls::linux_app_id(),
        ..Default::default()
    };

    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| MediaImageModal::new(uploaded, index, settings, window, cx))
    }) {
        Ok(handle) => {
            #[cfg(target_os = "macos")]
            if let Err(error) = handle.update(cx, |_, window, _| {
                window_controls::macos::disable_window_fullscreen(window);
            }) {
                tracing::warn!("Failed to configure media image modal window: {error}");
            }
            cx.set_global(GlobalMediaImageModal(handle));
            let _ = handle.update(cx, |modal, window, cx| {
                window.activate_window();
                window.focus(&modal.focus_handle, cx);
            });
            let modal = handle;
            if let Some(main_app) = main_app {
                cx.defer(move |cx| {
                    sync_overlay_to_main(modal, main_app, cx);
                });
            }
        }
        Err(error) => tracing::error!("failed to open media image modal window: {error}"),
    }
}

fn render_title_bar(title_bar: Entity<TitleBar>) -> AnyView {
    AnyView::from(title_bar).cached(StyleRefinement::default().w_full().h_8())
}

struct MediaImageModal {
    focus_handle: FocusHandle,
    title_bar: Entity<TitleBar>,
    attachments: Vec<ChannelTimelineAttachment>,
    index: usize,
    config: AppConfig,
    zoom: f32,
    pan: Point<Pixels>,
    drag_from: Option<Point<Pixels>>,
    rotation_deg: i32,
    rotated_image: Option<Arc<RenderImage>>,
    decoded_rgba: Option<(usize, Arc<image::RgbaImage>)>,
    rotation_loading: bool,
    show_list: bool,
    pending_thumb_scroll: bool,
    list_scroll: UniformListScrollHandle,
    image_cache: Entity<LruImageCache>,
    thumb_image_cache: Entity<LruImageCache>,
    rotation_session: u64,
    _release: Subscription,
}

impl Focusable for MediaImageModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MediaImageModal {
    fn new(
        attachments: Vec<ChannelTimelineAttachment>,
        index: usize,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, app| {
            let _ = weak.update(app, |modal, cx| {
                modal.release_resources(window, cx);
            });
            clear_media_image_modal_global(app);
            activate_main_window(app);
            true
        });
        let image_cache = cx.new(|cx| {
            LruImageCache::viewer(
                "media-modal",
                VIEWER_IMAGE_CACHE_CAPACITY,
                VIEWER_IMAGE_CACHE_BYTES,
                VIEWER_IMAGE_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let thumb_image_cache = cx.new(|cx| {
            LruImageCache::gallery_thumbnail(
                "media-modal-thumbs",
                GALLERY_IMAGE_CACHE_CAPACITY,
                GALLERY_IMAGE_CACHE_BYTES,
                SHARED_ENTRY_MAX_BYTES,
                cx,
            )
        });
        let cache_for_release = image_cache.clone();
        let thumb_cache_for_release = thumb_image_cache.clone();
        let release = cx.on_release(move |modal, cx| {
            modal.clear_rotated_image(None, cx);
            modal.decoded_rgba = None;
            cache_for_release.update(cx, |cache, cx| cache.clear_app(cx));
            thumb_cache_for_release.update(cx, |cache, cx| cache.clear_app(cx));
            crate::image_cache::release_freed_memory_to_os(cx);
            clear_media_image_modal_global(cx);
        });
        let show_list = attachments.len() > 1;
        let title_bar = cx.new(|cx| TitleBar::new(settings.clone(), cx));
        Self {
            focus_handle,
            title_bar,
            attachments,
            index,
            config: AppConfig::global(cx).clone(),
            zoom: MIN_ZOOM,
            pan: point(px(0.), px(0.)),
            drag_from: None,
            rotation_deg: 0,
            rotated_image: None,
            decoded_rgba: None,
            rotation_loading: false,
            show_list,
            pending_thumb_scroll: show_list,
            list_scroll: UniformListScrollHandle::new(),
            image_cache,
            thumb_image_cache,
            rotation_session: 0,
            _release: release,
        }
    }

    fn set_attachments(
        &mut self,
        attachments: Vec<ChannelTimelineAttachment>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_rotated_image(Some(window), cx);
        self.decoded_rgba = None;
        self.attachments = attachments;
        self.index = index.min(self.attachments.len().saturating_sub(1));
        self.show_list = self.attachments.len() > 1;
        self.pending_thumb_scroll = self.show_list;
        self.reset_view(cx);
        cx.notify();
    }

    fn release_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_rotated_image(Some(window), cx);
        self.decoded_rgba = None;
        self.image_cache.update(cx, |cache, cx| cache.clear_app(cx));
        self.thumb_image_cache
            .update(cx, |cache, cx| cache.clear_app(cx));
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_resources(window, cx);
        clear_media_image_modal_global(cx);
        activate_main_window(cx);
        window.remove_window();
    }

    fn current(&self) -> Option<&ChannelTimelineAttachment> {
        self.attachments.get(self.index)
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.zoom = MIN_ZOOM;
        self.pan = point(px(0.), px(0.));
        self.drag_from = None;
        self.rotation_deg = 0;
        self.rotated_image = None;
        self.decoded_rgba = None;
        self.rotation_loading = false;
        cx.notify();
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.attachments.len() && index != self.index {
            self.clear_rotated_image(Some(window), cx);
            self.decoded_rgba = None;
            self.index = index;
            self.list_scroll
                .scroll_to_item(self.index, gpui::ScrollStrategy::Center);
            self.reset_view(cx);
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

    fn zoom_in(&mut self, cx: &mut Context<Self>) {
        if self.zoom < MAX_ZOOM {
            self.set_zoom(self.zoom + ZOOM_BUTTON_STEP, cx);
        }
    }

    fn clear_rotated_image(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if let Some(old) = self.rotated_image.take() {
            cx.drop_image(old, window);
        }
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
            self.clear_rotated_image(Some(window), cx);
            self.rotation_loading = false;
            cx.notify();
            return;
        }
        let Some(att) = self.current().filter(|a| !is_video_attachment(a)) else {
            self.rotation_deg = 0;
            self.clear_rotated_image(Some(window), cx);
            self.rotation_loading = false;
            cx.notify();
            return;
        };
        let url = self.viewer_source(att);
        let degrees = self.rotation_deg;
        let cached_rgba = self
            .decoded_rgba
            .as_ref()
            .filter(|(index, _)| *index == self.index)
            .map(|(_, rgba)| rgba.clone());
        self.rotation_loading = true;
        self.rotation_session = self.rotation_session.wrapping_add(1);
        let session = self.rotation_session;
        let client = cx.http_client();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<(Arc<RenderImage>, Option<Arc<image::RgbaImage>>)> =
                match cached_rgba {
                    Some(rgba) => {
                        executor
                            .spawn(async move {
                                render_rotated_image(&rgba, degrees).map(|image| (image, None))
                            })
                            .await
                    }
                    None => match fetch_decoded_rgba(&url, client.clone()).await {
                        Ok(rgba) => {
                            let rgba_for_cache = rgba.clone();
                            executor
                                .spawn(async move { render_rotated_image(&rgba, degrees) })
                                .await
                                .map(|image| (image, Some(rgba_for_cache)))
                        }
                        Err(error) => Err(error),
                    },
                };
            let _ = this.update(cx, |this, cx| {
                if this.rotation_session != session {
                    return;
                }
                this.rotation_loading = false;
                match result {
                    Ok((image, rgba)) => {
                        if let Some(rgba) = rgba {
                            this.decoded_rgba = Some((this.index, rgba));
                        }
                        this.clear_rotated_image(None, cx);
                        this.rotated_image = Some(image);
                    }
                    Err(_) => {
                        this.rotation_deg = 0;
                        this.decoded_rgba = None;
                        this.clear_rotated_image(None, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_image(&mut self, cx: &mut Context<Self>) {
        let Some(att) = self.current() else {
            return;
        };
        let url = if att.file_url.starts_with("http") {
            att.file_url.clone()
        } else {
            att.display_url().to_string()
        };
        crate::util::download::save_with_progress_toast(
            SharedString::from(url),
            SharedString::from(att.file_name.clone()),
            cx,
        );
    }

    fn viewer_source(&self, att: &ChannelTimelineAttachment) -> String {
        if att.file_url.starts_with("http") {
            att.file_url.clone()
        } else {
            att.display_url().to_string()
        }
    }

    fn viewer_url(&self, att: &ChannelTimelineAttachment) -> SharedString {
        let url = self.viewer_source(att);
        if url.starts_with("http") {
            self.config
                .viewer_proxy(&url, att.width.max(0) as u32, att.height.max(0) as u32)
                .into()
        } else {
            url.into()
        }
    }

    fn thumb_url(&self, att: &ChannelTimelineAttachment) -> SharedString {
        let url = att.display_url();
        if url.starts_with("http") {
            self.config.viewer_thumb_proxy(url).into()
        } else {
            url.into()
        }
    }

    fn render_thumb_sidebar(
        &self,
        theme: &crate::theme::Theme,
        bg: gpui::Rgba,
        muted: gpui::Rgba,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = self.attachments.len();
        let entity = cx.entity();
        let active_index = self.index;
        let sidebar_bg = gpui::rgb(THUMB_SIDEBAR_BG);
        let text_primary = theme.text_primary;
        let border = theme.border;

        let list = uniform_list("media-modal-thumbs", count, move |range, _window, cx| {
            range
                .map(|idx| {
                    let Some(att) = entity.read(cx).attachments.get(idx) else {
                        return div().into_any_element();
                    };
                    let is_active = idx == active_index;
                    let thumb = entity.read(cx).thumb_url(att);
                    div()
                        .id(SharedString::from(format!("media-modal-thumb-{idx}")))
                        .h(px(THUMB_ROW_HEIGHT))
                        .w_full()
                        .p_1()
                        .on_mouse_down(MouseButton::Left, {
                            let entity = entity.clone();
                            move |_: &MouseDownEvent, window, cx| {
                                entity.update(cx, |this, cx| this.select(idx, window, cx));
                            }
                        })
                        .child(
                            div()
                                .rounded_md()
                                .overflow_hidden()
                                .border_2()
                                .when(is_active, |el| el.border_color(text_primary))
                                .when(!is_active, |el| {
                                    el.border_color(border)
                                        .opacity(0.6)
                                        .hover(|el| el.opacity(0.9))
                                })
                                .child(render_media_image(bg, muted, thumb, px(92.), px(80.))),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .w_full()
        .flex_1()
        .h_full()
        .min_h_0();

        div()
            .id("media-modal-thumbs")
            .w(px(THUMB_SIDEBAR_WIDTH + THUMB_SCROLLBAR_GUTTER))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(sidebar_bg)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .image_cache(self.thumb_image_cache.clone())
                    .child(list)
                    .custom_scrollbars(
                        Scrollbars::always_visible(ScrollAxes::Vertical)
                            .tracked_scroll_handle(&self.list_scroll)
                            .with_stable_track_along(ScrollAxes::Vertical, sidebar_bg.into()),
                        window,
                        cx,
                    ),
            )
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close(window, cx),
            "left" | "up" if self.index > 0 => self.select(self.index - 1, window, cx),
            "right" | "down" if self.index + 1 < self.attachments.len() => {
                self.select(self.index + 1, window, cx);
            }
            _ => {}
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
        };
        if dy.abs() > f32::EPSILON {
            self.set_zoom(self.zoom - dy * ZOOM_WHEEL_STEP, cx);
        }
    }
}

impl Render for MediaImageModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        self.thumb_image_cache
            .update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
        if self.pending_thumb_scroll && self.show_list {
            self.list_scroll
                .scroll_to_item(self.index, gpui::ScrollStrategy::Center);
            self.pending_thumb_scroll = false;
        }
        let theme = cx.theme().clone();
        let modal_fg = gpui::rgb(0xffffff);
        let bg = theme.bg_tertiary;
        let muted = theme.text_muted;
        let att = self.attachments.get(self.index);
        let total = self.attachments.len();
        let has_prev = self.index > 0;
        let has_next = self.index + 1 < total;
        let is_video = att.is_some_and(is_video_attachment);
        let show_image_tools = att.is_some() && !is_video;

        div()
            .track_focus(&self.focus_handle)
            .key_context("MediaImageModal")
            .on_action(cx.listener(|this, _: &::menu::Cancel, window, cx| this.close(window, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(gpui::rgb(0x141414))
            .when(window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(render_title_bar(self.title_bar.clone()))
            })
            .when(!window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(window_controls::window_drag_handle(
                    div().flex_shrink_0().h_8().w_full().bg(theme.title_bar_bg),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .w_full()
                    .h(px(30.))
                    .bg(gpui::rgb(0x2e2e2e))
                    .child(div().text_sm().text_color(modal_fg).child(format!(
                        "{} / {}",
                        self.index + 1,
                        total
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .relative()
                            .px_5()
                            .py_3()
                            .overflow_hidden()
                            .on_scroll_wheel(cx.listener(|this, event, _, cx| {
                                this.on_scroll(event, cx);
                            }))
                            .when(has_prev, |row| {
                                row.child(
                                    div()
                                        .id("media-modal-prev")
                                        .absolute()
                                        .left_4()
                                        .cursor_pointer()
                                        .rounded_full()
                                        .p_2()
                                        .bg(theme.bg_tertiary)
                                        .hover(|el| el.opacity(0.85))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.select(this.index.saturating_sub(1), window, cx);
                                        }))
                                        .child(
                                            Icon::new(IconName::ArrowLeft)
                                                .size(px(20.))
                                                .text_color(modal_fg),
                                        ),
                                )
                            })
                            .when(has_next, |row| {
                                row.child(
                                    div()
                                        .id("media-modal-next")
                                        .absolute()
                                        .right_4()
                                        .cursor_pointer()
                                        .rounded_full()
                                        .p_2()
                                        .bg(theme.bg_tertiary)
                                        .hover(|el| el.opacity(0.85))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.select(this.index + 1, window, cx);
                                        }))
                                        .child(
                                            Icon::new(IconName::ArrowRight)
                                                .size(px(20.))
                                                .text_color(modal_fg),
                                        ),
                                )
                            })
                            .when_some(att, |col, att| {
                                if is_video_attachment(att) {
                                    let url = SharedString::from(self.viewer_source(att));
                                    col.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .size_full()
                                            .child(
                                                img(url)
                                                    .max_w_full()
                                                    .max_h_full()
                                                    .object_fit(ObjectFit::Contain),
                                            ),
                                    )
                                } else {
                                    col.child(self.render_image(att, cx))
                                }
                            }),
                    )
                    .when(self.show_list && total > 1, |row| {
                        row.child(self.render_thumb_sidebar(&theme, bg, muted, window, cx))
                    }),
            )
            .child(
                div()
                    .h(px(56.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .bg(gpui::rgb(0x2e2e2e))
                    .child(
                        div().flex_1().min_w_0().flex().items_center().child(
                            div()
                                .text_sm()
                                .text_color(modal_fg)
                                .truncate()
                                .max_w(px(200.))
                                .child(att.map(|a| a.file_name.clone()).unwrap_or_default()),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .child(
                                modal_tool_button(IconName::HomepageDownload, modal_fg)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_image(cx))),
                            )
                            .when(show_image_tools, |row| {
                                row.child(modal_tool_divider(modal_fg))
                                    .child(
                                        modal_tool_button(IconName::RotateLeftIcon, modal_fg)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.rotate_left(window, cx)
                                            })),
                                    )
                                    .child(
                                        modal_tool_button(IconName::RotateRightIcon, modal_fg)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.rotate_right(window, cx)
                                            })),
                                    )
                                    .child(modal_tool_divider(modal_fg))
                                    .child(
                                        modal_tool_button(IconName::ZoomIcon, modal_fg).on_click(
                                            cx.listener(|this, _, _, cx| this.zoom_in(cx)),
                                        ),
                                    )
                                    .child(
                                        modal_tool_button(IconName::AspectRatioIcon, modal_fg)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.reset_zoom(cx)),
                                            ),
                                    )
                            }),
                    )
                    .child(div().flex_1().flex().justify_end().when(total > 1, |row| {
                        row.child(
                            modal_tool_button(IconName::SideMenuIcon, modal_fg).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.show_list = !this.show_list;
                                    cx.notify();
                                }),
                            ),
                        )
                    })),
            )
            .child(window_controls::render_app_drag_header())
            .when(window_controls::is_edge_resizable(), |el| {
                el.child(window_controls::render_resize_edges(window))
            })
    }
}

impl MediaImageModal {
    fn render_image(
        &self,
        att: &ChannelTimelineAttachment,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zoom = self.zoom;
        let pan = self.pan;
        let rotated_image = self.rotated_image.clone();
        let rotation_loading = self.rotation_loading;
        let viewer_src = self.viewer_url(att);
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
                            bounds,
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
                cx.listener(|this, ev: &MouseDownEvent, _, _cx| {
                    if this.zoom > MIN_ZOOM {
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
            .when(zoom > MIN_ZOOM, |el| el.cursor(gpui::CursorStyle::OpenHand))
    }
}

fn modal_tool_button(icon: IconName, fg: gpui::Rgba) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("media-modal-tool", icon as usize))
        .p_2()
        .rounded_md()
        .cursor_pointer()
        .hover(|el| el.bg(gpui::rgb(0x434343)))
        .child(Icon::new(icon).size(px(20.)).text_color(fg))
}

fn modal_tool_divider(fg: gpui::Rgba) -> impl IntoElement {
    div()
        .w(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::StraightLineIcon)
                .size(px(20.))
                .text_color(fg),
        )
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

async fn fetch_decoded_rgba(
    url: &str,
    client: Arc<dyn HttpClient>,
) -> anyhow::Result<Arc<image::RgbaImage>> {
    if !url.starts_with("https://") {
        anyhow::bail!("rotation fetch rejected: only https scheme is allowed");
    }
    let mut response = client.get(url, ().into(), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("rotation fetch failed with status {}", response.status());
    }
    let bytes = crate::image_cache::read_body_limited(
        &mut response,
        crate::image_cache::VIEWER_FETCH_MAX_BYTES,
    )
    .await?;
    let rgba = image::load_from_memory(&bytes)?.to_rgba8();
    Ok(Arc::new(rgba))
}

fn render_rotated_image(rgba: &image::RgbaImage, degrees: i32) -> anyhow::Result<Arc<RenderImage>> {
    let decoded = image::DynamicImage::ImageRgba8(rgba.clone());
    let rotated = match degrees.rem_euclid(360) {
        90 => decoded.rotate90(),
        180 => decoded.rotate180(),
        270 => decoded.rotate270(),
        _ => decoded,
    };
    let mut buffer = rotated.to_rgba8();
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}
