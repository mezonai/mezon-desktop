use std::sync::Arc;

use gpui::http_client::HttpClient;
use gpui::{
    App, AppContext, Context, DisplayId, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, Pixels, Render, RenderImage, ScrollHandle, SharedString, Subscription, Task,
    Window, WindowBounds, WindowHandle, WindowOptions, div, img, prelude::*, px, size,
};
use mezon_pdf::{PdfDocument, fit_page_pixels};
use mezon_store::{PlatformStore, Settings};

use crate::app::main_window::{
    activate_main_window, apply_overlay_bounds, handle as main_window_handle,
    overlay_placement_from_window, overlay_window_kind, sync_overlay_to_main,
};
use crate::app::title_bar::TitleBar;
use crate::app::window_controls;
use crate::components::primitives::{Icon, IconName, Spinner};
use crate::theme::{ActiveTheme, Theme};

const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;
const ZOOM_STEP: f32 = 0.2;
const BASE_PAGE_WIDTH: f32 = 800.0;
const PDF_FETCH_MAX_BYTES: usize = 64 * 1024 * 1024;

pub struct OpenPdfRequest {
    pub url: SharedString,
    pub filename: SharedString,
    pub settings: Entity<Settings>,
}

struct GlobalPdfViewer(WindowHandle<PdfViewer>);
impl gpui::Global for GlobalPdfViewer {}

fn clear_pdf_viewer_global(cx: &mut App) {
    if cx.try_global::<GlobalPdfViewer>().is_some() {
        cx.remove_global::<GlobalPdfViewer>();
    }
}

pub fn close_pdf_viewer(cx: &mut App) {
    let Some(handle) = cx.try_global::<GlobalPdfViewer>().map(|g| g.0) else {
        return;
    };
    let _ = handle.update(cx, |viewer, window, cx| {
        viewer.release_resources(Some(window), cx);
        window.remove_window();
    });
    clear_pdf_viewer_global(cx);
}

pub fn open_pdf_viewer(request: OpenPdfRequest, window: &Window, cx: &mut App) {
    if request.url.is_empty() {
        return;
    }
    let placement = overlay_placement_from_window(window, cx);
    let mut pending = Some(request);
    if let Some(handle) = cx.try_global::<GlobalPdfViewer>().map(|g| g.0) {
        let _ = handle.update(cx, |viewer, window, cx| {
            if let Some(request) = pending.take() {
                apply_overlay_bounds(window, placement.0);
                window.activate_window();
                window.focus(&viewer.focus_handle, cx);
                viewer.set_request(request, window, cx);
            }
        });
        if pending.is_none() {
            return;
        }
        clear_pdf_viewer_global(cx);
    }
    let Some(request) = pending else {
        return;
    };
    spawn_pdf_viewer_window(request, placement, cx);
}

fn spawn_pdf_viewer_window(
    request: OpenPdfRequest,
    placement: (WindowBounds, Option<DisplayId>),
    cx: &mut App,
) {
    let main_app = main_window_handle(cx);
    let (window_bounds, display_id) = placement;
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(600.0), px(480.0))),
        kind: overlay_window_kind(),
        focus: true,
        show: true,
        is_movable: true,
        display_id,
        titlebar: Some(window_controls::window_title_options()),
        window_decorations: window_controls::main_window_decorations(),
        app_id: window_controls::linux_app_id(),
        ..Default::default()
    };

    match cx.open_window(options, |window, cx| {
        cx.new(|cx| PdfViewer::new(request, window, cx))
    }) {
        Ok(handle) => {
            #[cfg(target_os = "macos")]
            if let Err(error) = handle.update(cx, |_, window, _| {
                window_controls::macos::disable_window_fullscreen(window);
            }) {
                tracing::warn!("Failed to configure pdf viewer window: {error}");
            }
            cx.set_global(GlobalPdfViewer(handle));
            let _ = handle.update(cx, |viewer, window, cx| {
                window.activate_window();
                window.focus(&viewer.focus_handle, cx);
            });
            if let Some(main_app) = main_app {
                cx.defer(move |cx| {
                    sync_overlay_to_main(handle, main_app, cx);
                });
            }
        }
        Err(error) => tracing::error!("failed to open pdf viewer window: {error}"),
    }
}

enum LoadState {
    Loading,
    Ready,
    Failed {
        message: SharedString,
        detail: Option<SharedString>,
    },
}

pub struct PdfViewer {
    focus_handle: FocusHandle,
    title_bar: Entity<TitleBar>,
    settings: Entity<Settings>,
    url: SharedString,
    filename: SharedString,
    document: Option<Arc<PdfDocument>>,
    page_count: usize,
    page_index: usize,
    zoom: f32,
    state: LoadState,
    page_image: Option<Arc<RenderImage>>,
    page_display: gpui::Size<Pixels>,
    rendered_key: Option<(usize, u32)>,
    render_in_flight: bool,
    scroll: ScrollHandle,
    closing: bool,
    _load_task: Option<Task<()>>,
    _render_task: Option<Task<()>>,
    _release: Subscription,
}

impl PdfViewer {
    fn new(request: OpenPdfRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, app| {
            let _ = weak.update(app, |viewer, cx| {
                viewer.release_resources(Some(window), cx);
            });
            clear_pdf_viewer_global(app);
            activate_main_window(app);
            true
        });
        let release = cx.on_release(|viewer, cx| {
            viewer.clear_page_image(None, cx);
        });
        let title_bar = cx.new(|cx| TitleBar::new(request.settings.clone(), cx));
        let mut this = Self {
            focus_handle,
            title_bar,
            settings: request.settings.clone(),
            url: request.url.clone(),
            filename: request.filename.clone(),
            document: None,
            page_count: 0,
            page_index: 0,
            zoom: 1.0,
            state: LoadState::Loading,
            page_image: None,
            page_display: size(px(0.), px(0.)),
            rendered_key: None,
            render_in_flight: false,
            scroll: ScrollHandle::new(),
            closing: false,
            _load_task: None,
            _render_task: None,
            _release: release,
        };
        this.start_load(window, cx);
        this
    }

    fn set_request(
        &mut self,
        request: OpenPdfRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.url == request.url {
            return;
        }
        self.url = request.url;
        self.filename = request.filename;
        self.settings = request.settings;
        self.document = None;
        self.page_count = 0;
        self.page_index = 0;
        self.zoom = 1.0;
        self.rendered_key = None;
        self.render_in_flight = false;
        self._render_task = None;
        self.clear_page_image(Some(window), cx);
        self.start_load(window, cx);
    }

    fn locale(&self, cx: &App) -> String {
        self.settings.read(cx).language.clone()
    }

    fn start_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !mezon_pdf::is_supported() {
            let detail = mezon_pdf::unavailable_reason();
            if let Some(detail) = &detail {
                tracing::warn!("pdf viewer unavailable: {detail}");
            }
            self.state = LoadState::Failed {
                message: SharedString::from(mezon_i18n::t(
                    &self.locale(cx),
                    "media.pdf.unsupported",
                )),
                detail: detail.map(SharedString::from),
            };
            cx.notify();
            return;
        }
        self.state = LoadState::Loading;
        let url = self.url.to_string();
        let client = cx.http_client();
        self._load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let bytes = match fetch_pdf(client, url).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!("pdf download failed: {error}");
                    let _ = this.update(cx, |this, cx| this.fail(cx));
                    return;
                }
            };
            let opened = cx
                .background_executor()
                .spawn(async move { PdfDocument::from_bytes(bytes) })
                .await;
            let document = match opened {
                Ok(document) => Arc::new(document),
                Err(error) => {
                    tracing::warn!("pdf open failed: {error}");
                    let _ = this.update(cx, |this, cx| this.fail(cx));
                    return;
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.page_count = document.page_count();
                this.document = Some(document);
                this.state = LoadState::Ready;
                this.request_page_render(window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn fail(&mut self, cx: &mut Context<Self>) {
        self.state = LoadState::Failed {
            message: SharedString::from(mezon_i18n::t(&self.locale(cx), "media.pdf.loadFailed")),
            detail: None,
        };
        cx.notify();
    }

    fn request_page_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.document.clone() else {
            return;
        };
        let scale_factor = window.scale_factor().max(1.0);
        let logical_width = (BASE_PAGE_WIDTH * self.zoom).max(1.0);
        let device_width = (logical_width * scale_factor).round().max(1.0) as u32;
        let key = (self.page_index, device_width);
        if self.rendered_key == Some(key) || self.render_in_flight {
            return;
        }
        self.render_in_flight = true;
        let index = self.page_index;
        self._render_task = Some(cx.spawn_in(window, async move |this, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move {
                    let page = document.page_size(index)?;
                    let (width, height) = fit_page_pixels(&page, device_width as f32);
                    document.render_page(index, width, height)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.render_in_flight = false;
                match rendered {
                    Ok(bitmap) => {
                        let Some(buffer) =
                            image::RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)
                        else {
                            this.fail(cx);
                            return;
                        };
                        this.clear_page_image(Some(window), cx);
                        let aspect = bitmap.height as f32 / bitmap.width.max(1) as f32;
                        this.page_display =
                            size(px(logical_width), px((logical_width * aspect).max(1.0)));
                        this.page_image =
                            Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])));
                        this.rendered_key = Some(key);
                        cx.notify();
                        cx.defer_in(window, |this, window, cx| {
                            this.request_page_render(window, cx);
                        });
                    }
                    Err(error) => {
                        tracing::warn!("pdf page render failed: {error}");
                        this.fail(cx);
                    }
                }
            });
        }));
    }

    fn clear_page_image(&mut self, window: Option<&mut Window>, cx: &mut App) {
        self.rendered_key = None;
        let Some(image) = self.page_image.take() else {
            return;
        };
        match window {
            Some(window) => cx.drop_image(image, Some(window)),
            None => crate::image_cache::queue_atlas_drop(cx, image),
        }
    }

    fn release_resources(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.render_in_flight = false;
        self._load_task = None;
        self._render_task = None;
        self.document = None;
        self.clear_page_image(window, cx);
        crate::image_cache::release_freed_memory_to_os(cx);
    }

    fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.release_resources(Some(window), cx);
        clear_pdf_viewer_global(cx);
        activate_main_window(cx);
        window.remove_window();
    }

    fn go_to_page(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_count == 0 {
            return;
        }
        let index = index.min(self.page_count - 1);
        if index == self.page_index {
            return;
        }
        self.page_index = index;
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.request_page_render(window, cx);
        cx.notify();
    }

    fn prev_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_index > 0 {
            self.go_to_page(self.page_index - 1, window, cx);
        }
    }

    fn next_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_page(self.page_index + 1, window, cx);
    }

    fn set_zoom(&mut self, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (zoom - self.zoom).abs() < f32::EPSILON {
            return;
        }
        self.zoom = zoom;
        self.request_page_render(window, cx);
        cx.notify();
    }

    fn download(&self, cx: &mut App) {
        crate::util::download::save_with_progress_toast(
            self.url.clone(),
            self.filename.clone(),
            cx,
        );
    }

    fn open_externally(&self, cx: &mut App) {
        if let Some(store) = PlatformStore::try_global(cx) {
            let _ = store.read(cx).open_url_external(&self.url);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_window(window, cx),
            "left" => self.prev_page(window, cx),
            "right" => self.next_page(window, cx),
            "+" | "=" => self.set_zoom(self.zoom + ZOOM_STEP, window, cx),
            "-" => self.set_zoom(self.zoom - ZOOM_STEP, window, cx),
            "0" => self.set_zoom(1.0, window, cx),
            _ => {}
        }
    }
}

async fn fetch_pdf(client: Arc<dyn HttpClient>, url: String) -> anyhow::Result<Vec<u8>> {
    if !url.starts_with("https://") {
        anyhow::bail!("pdf fetch rejected: only https scheme is allowed");
    }
    let mut response = client.get(&url, ().into(), true).await?;
    if !response.status().is_success() {
        anyhow::bail!("pdf fetch failed with status {}", response.status());
    }
    let bytes = crate::image_cache::read_body_limited(&mut response, PDF_FETCH_MAX_BYTES).await?;
    Ok(bytes)
}

impl Focusable for PdfViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PdfViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale(cx);

        div()
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("PdfViewer")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg_tertiary)
            .text_color(theme.text_primary)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !this.focus_handle.is_focused(window) {
                        window.focus(&this.focus_handle, cx);
                    }
                }),
            )
            .when(window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(self.title_bar.clone())
            })
            .when(!window_controls::HAS_CUSTOM_TITLE_BAR, |el| {
                el.child(window_controls::window_drag_handle(
                    div().flex_shrink_0().h_8().w_full().bg(theme.title_bar_bg),
                ))
            })
            .child(self.render_header(&theme, cx))
            .child(self.render_controls(&theme, cx))
            .child(self.render_page(&theme, &locale, cx))
            .child(self.render_footer(&theme, &locale))
            .child(window_controls::render_app_drag_header())
            .when(window_controls::is_edge_resizable(), |el| {
                el.child(window_controls::render_resize_edges(window))
            })
    }
}

impl PdfViewer {
    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .h(px(40.))
            .w_full()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .bg(theme.bg_secondary)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.))
                    .child(self.filename.clone()),
            )
            .child(div().flex().flex_row().items_center().gap_1().child(
                tool_button("pdf-download", IconName::Download, theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| this.download(cx)),
                ),
            ))
    }

    fn render_controls(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let counter = SharedString::from(format!(
            "{} / {}",
            self.page_index + 1,
            self.page_count.max(1)
        ));
        let zoom_label = SharedString::from(format!("{}%", (self.zoom * 100.0).round() as i32));
        let at_first = self.page_index == 0;
        let at_last = self.page_index + 1 >= self.page_count.max(1);

        div()
            .flex_shrink_0()
            .h(px(44.))
            .w_full()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .bg(theme.bg_secondary)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        tool_button("pdf-prev", IconName::ArrowLeft, theme)
                            .when(at_first, |el| el.opacity(0.5))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.prev_page(window, cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.text_secondary)
                            .child(counter),
                    )
                    .child(
                        tool_button("pdf-next", IconName::ChevronRight, theme)
                            .when(at_last, |el| el.opacity(0.5))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.next_page(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        tool_button("pdf-zoom-out", IconName::MinusCircleIcon, theme)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.set_zoom(this.zoom - ZOOM_STEP, window, cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("pdf-zoom-reset")
                            .min_w(px(56.))
                            .text_size(px(13.))
                            .text_color(theme.text_secondary)
                            .cursor_pointer()
                            .flex()
                            .justify_center()
                            .child(zoom_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                    this.set_zoom(1.0, window, cx)
                                }),
                            ),
                    )
                    .child(
                        tool_button("pdf-zoom-in", IconName::Plus, theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                this.set_zoom(this.zoom + ZOOM_STEP, window, cx)
                            }),
                        ),
                    ),
            )
    }

    fn render_page(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.state {
            LoadState::Failed { message, detail } => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(32.))
                        .text_color(theme.danger_text),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.danger_text)
                        .child(message.clone()),
                )
                .when_some(detail.clone(), |el, detail| {
                    el.child(
                        div()
                            .max_w(px(520.))
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(detail),
                    )
                })
                .child(
                    div()
                        .id("pdf-open-external")
                        .px_3()
                        .py_1()
                        .rounded(px(6.))
                        .bg(theme.bg_hover)
                        .cursor_pointer()
                        .text_size(px(13.))
                        .child(SharedString::from(mezon_i18n::t(
                            locale,
                            "media.pdf.openExternally",
                        )))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| this.open_externally(cx)),
                        ),
                )
                .into_any_element(),
            _ => match self.page_image.clone() {
                Some(image) => div()
                    .min_w_full()
                    .flex()
                    .justify_center()
                    .p_4()
                    .child(
                        img(image)
                            .w(self.page_display.width)
                            .h(self.page_display.height),
                    )
                    .into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new())
                    .into_any_element(),
            },
        };

        div()
            .id("pdf-page-scroll")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .bg(theme.bg_tertiary)
            .child(body)
    }

    fn render_footer(&self, theme: &Theme, locale: &str) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .h(px(28.))
            .w_full()
            .px_4()
            .flex()
            .items_center()
            .bg(theme.bg_secondary)
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.text_muted)
            .child(SharedString::from(mezon_i18n::t(
                locale,
                "media.pdf.shortcutHint",
            )))
    }
}

fn tool_button(id: &'static str, icon: IconName, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
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
