use crate::{
    app::{main_window::handle as main_window_handle, shell::Shell},
    components::primitives::{Button, ButtonVariants, Icon, IconName, Sizable, h_flex, v_flex},
    image_cache::{MAX_DECODE_PIXELS, MAX_DECODER_ALLOC_BYTES, MAX_IMAGE_DECODE_DIMENSION},
    theme::ActiveTheme,
    util::{file_dialog::resolve as resolve_file_dialog, imgproxy::stream_cover_url},
};
use gpui::{
    App, Context, Entity, ExternalPaths, FocusHandle, Focusable, FontWeight, ImageSource,
    ObjectFit, PathPromptOptions, SharedString, Subscription, Task, WeakEntity, Window, div, img,
    linear_color_stop, linear_gradient, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, Settings, can_manage_channel,
    channel::MAX_STREAM_THUMBNAIL_BYTES as MAX_BYTES, validate_clan_image_file,
};
use std::path::PathBuf;

const THUMBNAIL_ASPECT_RATIO: f32 = 16. / 9.;
const THUMBNAIL_CONTENT_WIDTH: f32 = 650.;
const UPLOAD_MODAL_WIDTH: f32 = 672.;
const REMOVE_MODAL_WIDTH: f32 = 448.;

fn thumbnail_frame(image_id: &'static str, source: impl Into<ImageSource>) -> gpui::Div {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .flex_none()
        .aspect_ratio(THUMBNAIL_ASPECT_RATIO)
        .rounded_xl()
        .overflow_hidden()
        .child(
            img(source)
                .id(image_id)
                .absolute()
                .inset_0()
                .size_full()
                .aspect_ratio(THUMBNAIL_ASPECT_RATIO)
                .object_fit(ObjectFit::Cover)
                .rounded_xl(),
        )
}

pub struct StreamThumbnailTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    phase: ThumbnailPhase,
    _picker_task: Option<Task<()>>,
    _operation_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThumbnailPhase {
    Idle,
    Picking,
    Validating,
    Uploading,
    Removing,
}

enum ThumbnailFileError {
    TooLarge,
    Empty,
    Invalid,
    Other(String),
}

impl StreamThumbnailTab {
    pub fn new(
        clan_id: ClanId,
        channel_id: ChannelId,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subs = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&ChannelList::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            clan_id,
            channel_id,
            settings,
            phase: ThumbnailPhase::Idle,
            _picker_task: None,
            _operation_task: None,
            _subs: subs,
        }
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        if self.phase != ThumbnailPhase::Idle
            || !can_manage_channel(self.clan_id, self.channel_id, cx)
        {
            return;
        }
        self.phase = ThumbnailPhase::Picking;
        cx.notify();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.t("streamThumbnail.buttons.selectFile", cx)),
        });
        self._picker_task = Some(cx.spawn(async move |this, cx| {
            let paths = resolve_file_dialog(rx, cx).await;
            let _ = this.update(cx, |this, cx| {
                this.phase = ThumbnailPhase::Idle;
                if let Some(path) = paths.and_then(|paths| paths.into_iter().next()) {
                    this.prepare(path, cx);
                }
                cx.notify();
            });
        }));
    }

    fn prepare(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.phase != ThumbnailPhase::Idle
            || !can_manage_channel(self.clan_id, self.channel_id, cx)
        {
            return;
        }
        self.phase = ThumbnailPhase::Validating;
        cx.notify();
        self._operation_task = Some(cx.spawn(async move |this, cx| {
            let validated = cx
                .background_spawn(async move {
                    let len = std::fs::metadata(&path)
                        .map_err(|error| ThumbnailFileError::Other(error.to_string()))?
                        .len();
                    if len > MAX_BYTES {
                        return Err(ThumbnailFileError::TooLarge);
                    }
                    if len == 0 {
                        return Err(ThumbnailFileError::Empty);
                    }
                    if validate_clan_image_file(&path, MAX_BYTES).is_err() {
                        return Err(ThumbnailFileError::Invalid);
                    }
                    let reader = image::ImageReader::open(&path)
                        .map_err(|error| ThumbnailFileError::Other(error.to_string()))?
                        .with_guessed_format()
                        .map_err(|error| ThumbnailFileError::Other(error.to_string()))?;
                    let (width, height) = reader
                        .into_dimensions()
                        .map_err(|_| ThumbnailFileError::Invalid)?;
                    if width == 0
                        || height == 0
                        || width > MAX_IMAGE_DECODE_DIMENSION
                        || height > MAX_IMAGE_DECODE_DIMENSION
                        || u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS
                        || u64::from(width) * u64::from(height) * 4 > MAX_DECODER_ALLOC_BYTES
                    {
                        return Err(ThumbnailFileError::Invalid);
                    }
                    Ok(path)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.phase = ThumbnailPhase::Idle;
                match validated {
                    Ok(path) => this.confirm(Some(path), cx),
                    Err(error) => {
                        let locale = this.settings.read(cx).language.clone();
                        if matches!(
                            error,
                            ThumbnailFileError::Empty | ThumbnailFileError::Other(_)
                        ) {
                            if let ThumbnailFileError::Other(message) = &error {
                                tracing::warn!("Stream thumbnail validation failed: {message}");
                            }
                            let message =
                                mezon_i18n::t(&locale, "streamThumbnail.errors.uploadFailed")
                                    .to_string();
                            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                            cx.notify();
                            return;
                        }
                        let oversized = matches!(error, ThumbnailFileError::TooLarge);
                        let title = mezon_i18n::t(
                            &locale,
                            if oversized {
                                "common.filesTooPowerful"
                            } else {
                                "common.onlyImageFiles"
                            },
                        );
                        let content = if oversized {
                            mezon_i18n::t(&locale, "common.maxFileSize").replace(
                                "{{sizeLimit}}",
                                &format!("{} MB", MAX_BYTES / (1024 * 1024)),
                            )
                        } else {
                            mezon_i18n::t(&locale, "streamThumbnail.requirements.format.value")
                                .to_string()
                        };
                        if let Some(handle) = main_window_handle(cx) {
                            let _ = cx.update_window(handle, |_, window, cx| {
                                Shell::global(cx).update(cx, |shell, cx| {
                                    shell.show_upload_limit(title, content, window, cx)
                                });
                            });
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn t(&self, key: &'static str, cx: &App) -> SharedString {
        mezon_i18n::t(&self.settings.read(cx).language, key)
            .to_string()
            .into()
    }

    fn confirm(&self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.phase != ThumbnailPhase::Idle {
            return;
        }
        let tab = cx.entity().downgrade();
        let modal = cx.new(|cx| ThumbnailConfirmation {
            tab,
            path,
            settings: self.settings.clone(),
            focus_handle: cx.focus_handle(),
        });
        if let Some(handle) = main_window_handle(cx) {
            let focus_handle = modal.read(cx).focus_handle.clone();
            let _ = cx.update_window(handle, |_, window, cx| window.focus(&focus_handle, cx));
        }
        Shell::global(cx).update(cx, |shell, cx| shell.show_modal(modal.into(), cx));
    }

    fn save(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.phase != ThumbnailPhase::Idle
            || !can_manage_channel(self.clan_id, self.channel_id, cx)
        {
            return;
        }
        self.phase = if path.is_some() {
            ThumbnailPhase::Uploading
        } else {
            ThumbnailPhase::Removing
        };
        cx.notify();
        let clan_id = self.clan_id;
        let channel_id = self.channel_id;
        let removing = path.is_none();
        let upload = path.map(|path| {
            ClanList::global(cx).update(cx, |store, cx| {
                store.upload_clan_image(&path, MAX_BYTES, cx)
            })
        });
        let store = ChannelList::global(cx);
        self._operation_task = Some(cx.spawn(async move |this, cx| {
            let result = async {
                let avatar = match upload {
                    Some(upload) => upload.await?,
                    None => String::new(),
                };
                let task = store.update(cx, |store, cx| {
                    store.update_stream_thumbnail(clan_id, channel_id, avatar, cx)
                });
                task.await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.phase = ThumbnailPhase::Idle;
                if let Err(error) = result {
                    tracing::error!("Stream thumbnail save failed: {error}");
                    let message = this
                        .t(
                            if removing {
                                "streamThumbnail.errors.removeFailed"
                            } else {
                                "streamThumbnail.errors.uploadFailed"
                            },
                            cx,
                        )
                        .to_string();
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                }
                cx.notify();
            });
        }));
    }
}

impl Render for StreamThumbnailTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let brand = theme.brand;
        let avatar = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| channel.avatar_url.clone())
            .filter(|url| !url.trim().is_empty() && url != "0");
        let disabled = self.phase != ThumbnailPhase::Idle
            || !can_manage_channel(self.clan_id, self.channel_id, cx);
        let mut content = v_flex()
            .w_full()
            .min_w_0()
            .max_w(px(THUMBNAIL_CONTENT_WIDTH))
            .gap_3();
        if let Some(avatar) = avatar {
            content = content.child(
                thumbnail_frame(
                    "stream-thumbnail-settings-image",
                    SharedString::from(stream_cover_url(cx, &avatar)),
                )
                .id("stream-thumbnail-preview")
                .group("thumbnail-preview")
                .child(
                    h_flex()
                        .absolute()
                        .inset_0()
                        .items_end()
                        .justify_center()
                        .gap_3()
                        .p_6()
                        .rounded_xl()
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(
                                gpui::Hsla::from(theme.tokens.bg_modal).opacity(0.),
                                0.,
                            ),
                            linear_color_stop(
                                gpui::Hsla::from(theme.tokens.bg_modal).opacity(0.8),
                                1.,
                            ),
                        ))
                        .opacity(0.)
                        .group_hover("thumbnail-preview", |style| style.opacity(1.))
                        .child(
                            Button::new("thumbnail-change")
                                .large()
                                .label(self.t("streamThumbnail.buttons.change", cx))
                                .icon(Icon::new(IconName::PenEdit).text_color(theme.text_primary))
                                .primary()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                        )
                        .child(
                            Button::new("thumbnail-remove")
                                .large()
                                .label(self.t("streamThumbnail.buttons.remove", cx))
                                .icon(Icon::new(IconName::TrashIcon).text_color(theme.text_primary))
                                .danger()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| this.confirm(None, cx))),
                        ),
                ),
            );
        } else {
            content = content
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.t("streamThumbnail.uploadThumbnail", cx)),
                )
                .child(
                    v_flex()
                        .id("stream-thumbnail-upload")
                        .h(px(320.))
                        .w_full()
                        .rounded_xl()
                        .border_2()
                        .border_dashed()
                        .border_color(theme.border)
                        .bg(theme.bg_secondary)
                        .items_center()
                        .justify_center()
                        .gap_4()
                        .cursor_pointer()
                        .hover(|style| style.border_color(brand))
                        .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(brand))
                        .child(
                            div()
                                .size(px(80.))
                                .rounded_full()
                                .bg(theme.bg_hover)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::UploadImage)
                                        .size(px(48.))
                                        .text_color(theme.brand),
                                ),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.t("streamThumbnail.upload.title", cx)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .child(self.t("streamThumbnail.upload.description", cx)),
                        )
                        .child(
                            Button::new("thumbnail-select")
                                .large()
                                .label(self.t("streamThumbnail.buttons.selectFile", cx))
                                .icon(
                                    Icon::new(IconName::UploadImage).text_color(theme.text_primary),
                                )
                                .primary()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.choose(cx);
                                })),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.choose(cx)))
                        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                            if let Some(path) = paths.paths().first() {
                                this.prepare(path.clone(), cx);
                            }
                        })),
                );
        }
        content = content.when(
            matches!(
                self.phase,
                ThumbnailPhase::Uploading | ThumbnailPhase::Removing
            ),
            |content| {
                content.child(div().text_sm().text_color(theme.text_muted).child(self.t(
                    if self.phase == ThumbnailPhase::Removing {
                        "streamThumbnail.buttons.removing"
                    } else {
                        "streamThumbnail.buttons.uploading"
                    },
                    cx,
                )))
            },
        );
        content = content.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(self.t("streamThumbnail.requirementsTitle", cx)),
        );
        for (title, value, icon) in [
            (
                "streamThumbnail.requirements.format.title",
                "streamThumbnail.requirements.format.value",
                IconName::ImageThumbnail,
            ),
            (
                "streamThumbnail.requirements.resolution.title",
                "streamThumbnail.requirements.resolution.value",
                IconName::ImageUploadIcon,
            ),
            (
                "streamThumbnail.requirements.sizeLimit.title",
                "streamThumbnail.requirements.sizeLimit.value",
                IconName::FileIcon,
            ),
        ] {
            content = content.child(
                h_flex()
                    .p_4()
                    .gap_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.theme_setting_nav)
                    .items_center()
                    .child(
                        div()
                            .size(px(40.))
                            .rounded_lg()
                            .bg(theme.bg_hover)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if icon == IconName::ImageUploadIcon {
                                img(icon.path())
                                    .size(px(20.))
                                    .flex_none()
                                    .into_any_element()
                            } else {
                                Icon::new(icon)
                                    .size(px(20.))
                                    .text_color(theme.text_muted)
                                    .into_any_element()
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.t(title, cx)),
                            )
                            .child(div().text_sm().child(self.t(value, cx))),
                    ),
            );
        }
        v_flex()
            .w_full()
            .gap(px(48.))
            .text_color(theme.text_primary)
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.t("streamThumbnail.title", cx)),
            )
            .child(div().px_6().child(content))
    }
}

struct ThumbnailConfirmation {
    tab: WeakEntity<StreamThumbnailTab>,
    path: Option<PathBuf>,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
}

impl Focusable for ThumbnailConfirmation {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ThumbnailConfirmation {
    fn submit(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab.clone();
        let path = self.path.clone();
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
        let _ = tab.update(cx, |tab, cx| tab.save(path, cx));
    }
}

impl Render for ThumbnailConfirmation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = &self.settings.read(cx).language;
        let t =
            |key: &'static str| -> SharedString { mezon_i18n::t(locale, key).to_string().into() };
        let upload = self.path.is_some();
        let modal_width = (if upload {
            UPLOAD_MODAL_WIDTH
        } else {
            REMOVE_MODAL_WIDTH
        })
        .min((f32::from(window.viewport_size().width) - 48.).max(0.));
        let mut body = v_flex()
            .id("thumbnail-confirmation-body")
            .w_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .text_sm()
            .bg(theme.tokens.theme_setting_primary);
        if let Some(path) = &self.path {
            body = body.child(thumbnail_frame(
                "stream-thumbnail-confirm-image",
                path.clone(),
            ));
        }
        body = body.child(t(if upload {
            "streamThumbnail.confirmModal.uploadMessage"
        } else {
            "streamThumbnail.confirmModal.removeMessage"
        }));
        if !upload {
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.bg_item_theme_hover)
                    .child(t("streamThumbnail.confirmModal.removeWarning")),
            );
        }
        let confirm = Button::new("thumbnail-confirm")
            .large()
            .label(t(if upload {
                "streamThumbnail.buttons.confirmUpload"
            } else {
                "streamThumbnail.buttons.removeThumbnail"
            }))
            .when(!upload, |button| {
                button.icon(Icon::new(IconName::TrashIcon).text_color(theme.text_primary))
            })
            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)));
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _, cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
            }))
            .on_action(cx.listener(|this, _: &::menu::Confirm, _, cx| this.submit(cx)))
            .w(px(modal_width))
            .max_h(px((f32::from(window.viewport_size().height) - 48.).max(0.)))
            .max_w_full()
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_nav)
            .text_color(theme.text_primary)
            .child(
                h_flex()
                    .flex_none()
                    .p_4()
                    .gap_3()
                    .bg(theme.tokens.theme_setting_nav)
                    .when(upload, |header| {
                        header.child(
                            div()
                                .size(px(40.))
                                .rounded_lg()
                                .bg(gpui::Hsla::from(theme.brand).opacity(0.15))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::UploadImage)
                                        .size(px(24.))
                                        .text_color(theme.brand),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(t(
                                if upload {
                                    "streamThumbnail.confirmModal.uploadTitle"
                                } else {
                                    "streamThumbnail.confirmModal.removeTitle"
                                },
                            )))
                            .child(div().text_sm().child(t(if upload {
                                "streamThumbnail.confirmModal.uploadDescription"
                            } else {
                                "streamThumbnail.confirmModal.removeQuestion"
                            }))),
                    ),
            )
            .child(body)
            .child(
                h_flex()
                    .flex_none()
                    .p_6()
                    .gap_3()
                    .bg(theme.tokens.theme_setting_nav)
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("thumbnail-cancel")
                            .large()
                            .label(t("streamThumbnail.buttons.cancel"))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                            }),
                    )
                    .child(if upload {
                        confirm.primary()
                    } else {
                        confirm.danger()
                    }),
            )
    }
}
