use std::{
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{
    App, Bounds, Context, Corners, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, RenderImage, SharedString, Size, Subscription, Window, canvas, div, point, prelude::*,
    px, rgb, size,
};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use mezon_store::Settings;

use crate::{
    app::shell::Shell,
    components::primitives::{
        Button, ButtonVariants, Icon, IconName, Slider, SliderEvent, SliderState, Spinner, h_flex,
        v_flex,
    },
    theme::ActiveTheme,
};

type ApplyHandler = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

pub struct EditAvatar {
    source: Arc<DynamicImage>,
    render_images: Vec<Arc<RenderImage>>,
    crop_mask: Arc<RenderImage>,
    zoom: f32,
    rotation: u16,
    pan: Point<Pixels>,
    drag_from: Option<Point<Pixels>>,
    slider: Entity<SliderState>,
    error: Option<SharedString>,
    loading: bool,
    on_apply: ApplyHandler,
    _slider_subscription: Subscription,
    _release_subscription: Subscription,
}

impl EditAvatar {
    pub fn open(
        source: PathBuf,
        on_apply: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| Self::new(on_apply, cx));
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(view.clone().into(), cx));

        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let prepared = executor.spawn(async move { prepare_editor(source) }).await;
            view.update(cx, |this, cx| {
                this.loading = false;
                match prepared {
                    Ok((source, render_images, crop_mask)) => {
                        this.source = source;
                        this.render_images = render_images;
                        this.crop_mask = crop_mask;
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new(
        on_apply: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(2.0)
                .step(0.1)
                .default_value(1.0)
        });
        let slider_subscription = cx.subscribe(&slider, |this, _, event, cx| {
            let SliderEvent::Change(value) = event;
            this.zoom = value.start();
            cx.notify();
        });
        let release_subscription = cx.on_release(|this, cx| {
            for image in std::mem::take(&mut this.render_images) {
                cx.drop_image(image, None);
            }
            cx.drop_image(this.crop_mask.clone(), None);
        });

        Self {
            source: Arc::new(DynamicImage::new_rgba8(1, 1)),
            render_images: Vec::new(),
            crop_mask: render_image(DynamicImage::new_rgba8(1, 1)),
            zoom: 1.0,
            rotation: 0,
            pan: point(px(0.), px(0.)),
            drag_from: None,
            slider,
            error: None,
            loading: true,
            on_apply: Rc::new(on_apply),
            _slider_subscription: slider_subscription,
            _release_subscription: release_subscription,
        }
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for EditAvatar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = Settings::try_global(cx)
            .map(|settings| settings.read(cx).language.clone())
            .unwrap_or_default();
        let apply = self.on_apply.clone();
        let source = self.source.clone();
        let zoom = self.zoom;
        let rotation = self.rotation;
        let image = self.render_images.get((rotation / 90) as usize).cloned();
        let crop_mask = self.crop_mask.clone();
        let canvas_size = (f32::from(window.viewport_size().height) - 330.0).clamp(300.0, 500.0);
        let card_width = canvas_size + 64.0;
        let pan = self.pan;
        let view = cx.entity().clone();
        let executor = cx.background_executor().clone();

        v_flex()
            .id("edit-avatar-modal")
            .w(px(card_width))
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_floating)
            .shadow_lg()
            .child(
                h_flex()
                    .h(px(64.))
                    .px_5()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(&locale, "profileSetting.editImage")),
                    )
                    .child(
                        div()
                            .id("edit-avatar-close")
                            .size(px(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(22.))
                                    .text_color(theme.text_muted),
                            )
                            .on_click(|_, _, cx| Self::close(cx)),
                    ),
            )
            .child(
                v_flex()
                    .px_8()
                    .pb_3()
                    .items_center()
                    .child(
                        div()
                            .id("edit-avatar-canvas")
                            .relative()
                            .size(px(canvas_size))
                            .overflow_hidden()
                            .bg(rgb(0x111214))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                canvas(
                                    move |_, _, _| {
                                        image.clone().map(|image| (image, crop_mask.clone()))
                                    },
                                    move |bounds, layers, window, _| {
                                        let Some((image, crop_mask)) = layers else {
                                            return;
                                        };
                                        let raw = image.size(0);
                                        let fitted = editor_image_size(
                                            bounds.size,
                                            size(px(raw.width.0 as f32), px(raw.height.0 as f32)),
                                            zoom,
                                        );
                                        let width = fitted.width;
                                        let height = fitted.height;
                                        let x = bounds.origin.x
                                            + (bounds.size.width - width) / 2.0
                                            + pan.x;
                                        let y = bounds.origin.y
                                            + (bounds.size.height - height) / 2.0
                                            + pan.y;
                                        let _ = window.paint_image(
                                            bounds,
                                            Bounds::from_corners(
                                                point(x, y),
                                                point(x + width, y + height),
                                            ),
                                            Corners::default(),
                                            image,
                                            0,
                                            false,
                                        );
                                        let _ = window.paint_image(
                                            bounds,
                                            bounds,
                                            Corners::default(),
                                            crop_mask,
                                            0,
                                            false,
                                        );
                                    },
                                )
                                .size_full(),
                            )
                            .when(self.loading, |element| {
                                element.child(div().absolute().child(Spinner::new()))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, _| {
                                    this.drag_from = Some(event.position);
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                if let Some(from) = this.drag_from {
                                    this.pan.x += event.position.x - from.x;
                                    this.pan.y += event.position.y - from.y;
                                    this.drag_from = Some(event.position);
                                    cx.notify();
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &gpui::MouseUpEvent, _, _| {
                                    this.drag_from = None;
                                }),
                            )
                            .cursor(gpui::CursorStyle::OpenHand),
                    )
                    .child(
                        h_flex()
                            .h(px(56.))
                            .gap_3()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::ImageThumbnail)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            )
                            .child(div().w(px(190.)).child(Slider::new(&self.slider)))
                            .child(
                                Icon::new(IconName::ImageThumbnail)
                                    .size(px(22.))
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                div()
                                    .id("edit-avatar-rotate")
                                    .size(px(34.))
                                    .ml_2()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|el| el.bg(theme.bg_hover))
                                    .child(
                                        Icon::new(IconName::RotateRightIcon)
                                            .size(px(22.))
                                            .text_color(theme.text_muted),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.rotation = (this.rotation + 90) % 360;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(div().text_sm().text_color(theme.danger_text).child(error))
                    }),
            )
            .child(
                h_flex()
                    .h(px(76.))
                    .px_5()
                    .justify_between()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.bg_primary)
                    .child(
                        Button::new("edit-avatar-reset")
                            .label(mezon_i18n::t(&locale, "profileSetting.reset"))
                            .text_color(theme.text_muted)
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom = 1.0;
                                this.rotation = 0;
                                this.pan = point(px(0.), px(0.));
                                this.slider
                                    .update(cx, |slider, cx| slider.set_value(1.0, window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("edit-avatar-cancel")
                                    .label(mezon_i18n::t(&locale, "profileSetting.cancel"))
                                    .ghost()
                                    .on_click(|_, _, cx| Self::close(cx)),
                            )
                            .child(
                                Button::new("edit-avatar-apply")
                                    .label(mezon_i18n::t(&locale, "profileSetting.apply"))
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        let output = temp_png("cropped");
                                        let source = source.clone();
                                        let output_for_crop = output.clone();
                                        let apply = apply.clone();
                                        let view = view.clone();
                                        let executor = executor.clone();
                                        let window_handle = window.window_handle();
                                        view.update(cx, |this, cx| {
                                            this.loading = true;
                                            this.error = None;
                                            cx.notify();
                                        });
                                        cx.spawn(async move |cx| {
                                            let result = executor
                                                .spawn(async move {
                                                    crop_avatar(
                                                        &source,
                                                        &output_for_crop,
                                                        zoom,
                                                        rotation,
                                                        pan,
                                                        canvas_size,
                                                    )
                                                })
                                                .await;
                                            let _ =
                                                cx.update_window(window_handle, |_, window, cx| {
                                                    view.update(cx, |this, cx| {
                                                        this.loading = false;
                                                        match result {
                                                            Ok(()) => {
                                                                apply(output, window, cx);
                                                                Self::close(cx);
                                                            }
                                                            Err(error) => {
                                                                tracing::error!(
                                                                    "failed to crop avatar: {error}"
                                                                );
                                                                this.error =
                                                                    Some(error.to_string().into());
                                                                cx.notify();
                                                            }
                                                        }
                                                    });
                                                });
                                        })
                                        .detach();
                                    }),
                            ),
                    ),
            )
    }
}

fn render_image(image: DynamicImage) -> Arc<RenderImage> {
    let mut buffer = image.to_rgba8();
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

fn prepare_editor(
    source_path: PathBuf,
) -> anyhow::Result<(Arc<DynamicImage>, Vec<Arc<RenderImage>>, Arc<RenderImage>)> {
    let source = image::open(source_path)?.thumbnail(1024, 1024);
    let render_images = [
        source.clone(),
        source.rotate90(),
        source.rotate180(),
        source.rotate270(),
    ]
    .into_iter()
    .map(render_image)
    .collect();
    Ok((Arc::new(source), render_images, render_image(crop_mask())))
}

fn editor_image_size(container: Size<Pixels>, content: Size<Pixels>, zoom: f32) -> Size<Pixels> {
    let scale = (f32::from(container.width) / f32::from(content.width))
        .min(f32::from(container.height) / f32::from(content.height))
        * zoom;
    let mut width = f32::from(content.width) * scale;
    let mut height = f32::from(content.height) * scale;
    let crop_diameter = f32::from(container.width) * 0.8;

    if width > height && height <= crop_diameter {
        height = crop_diameter;
        width = height * f32::from(content.width) / f32::from(content.height);
    } else if width <= height && width <= crop_diameter {
        width = crop_diameter;
        height = width * f32::from(content.height) / f32::from(content.width);
    }
    size(px(width), px(height))
}

fn crop_mask() -> DynamicImage {
    const SIDE: u32 = 1000;
    const CENTER: f32 = 500.0;
    const RADIUS: f32 = 400.0;
    const HALF_STROKE: f32 = 3.0;
    let mut mask = image::RgbaImage::new(SIDE, SIDE);
    for (x, y, pixel) in mask.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - CENTER;
        let dy = y as f32 + 0.5 - CENTER;
        let distance = (dx * dx + dy * dy).sqrt();
        let stroke_coverage = (HALF_STROKE + 1.0 - (distance - RADIUS).abs()).clamp(0.0, 1.0);
        if stroke_coverage > 0.0 {
            let alpha = (stroke_coverage * 255.0).round() as u8;
            *pixel = image::Rgba([255, 255, 255, alpha]);
        } else {
            let outside_coverage = (distance - RADIUS + 0.5).clamp(0.0, 1.0);
            *pixel = image::Rgba([0, 0, 0, (outside_coverage * 128.0).round() as u8]);
        }
    }
    DynamicImage::ImageRgba8(mask)
}

fn temp_png(kind: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mezon-avatar-{kind}-{nonce}.png"))
}

fn crop_avatar(
    source: &DynamicImage,
    output: &PathBuf,
    zoom: f32,
    rotation: u16,
    pan: Point<Pixels>,
    canvas_size: f32,
) -> anyhow::Result<()> {
    let rotated = match rotation % 360 {
        90 => source.rotate90(),
        180 => source.rotate180(),
        270 => source.rotate270(),
        _ => source.clone(),
    };
    let (width, height) = rotated.dimensions();
    let crop_display = canvas_size * 0.8;
    let displayed = editor_image_size(
        size(px(canvas_size), px(canvas_size)),
        size(px(width as f32), px(height as f32)),
        zoom,
    );
    let display_width = f32::from(displayed.width);
    let display_height = f32::from(displayed.height);
    let image_left = (canvas_size - display_width) / 2.0 + f32::from(pan.x);
    let image_top = (canvas_size - display_height) / 2.0 + f32::from(pan.y);
    let crop_left = (canvas_size - crop_display) / 2.0;
    let crop_top = (canvas_size - crop_display) / 2.0;
    let output_scale = 400.0 / crop_display;
    let resized = rotated.resize_exact(
        (display_width * output_scale).round().max(1.0) as u32,
        (display_height * output_scale).round().max(1.0) as u32,
        FilterType::Triangle,
    );
    let mut cropped = image::RgbaImage::new(400, 400);
    let output_x = ((image_left - crop_left) * output_scale).round() as i64;
    let output_y = ((image_top - crop_top) * output_scale).round() as i64;
    image::imageops::overlay(&mut cropped, &resized.to_rgba8(), output_x, output_y);
    DynamicImage::ImageRgba8(cropped).save_with_format(output, image::ImageFormat::Png)?;
    Ok(())
}
