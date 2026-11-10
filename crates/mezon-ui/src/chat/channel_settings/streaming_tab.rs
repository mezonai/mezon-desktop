use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    App, ClickEvent, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    FontWeight, ImageSource, ObjectFit, PathPromptOptions, SharedString, Subscription, Task,
    WeakEntity, Window, div, img, linear_color_stop, linear_gradient, prelude::*, px,
};
use mezon_store::{
    ChannelId, ChannelList, ClanId, ClanList, STREAM_QUALITIES, Settings, can_manage_channel,
    channel::MAX_STREAM_THUMBNAIL_BYTES as MAX_BYTES, validate_clan_image_file,
};

use crate::{
    app::{main_window::handle as main_window_handle, shell::Shell},
    components::primitives::{
        Button, ButtonVariants, Icon, IconName, Label, Sizable, Size, Spinner, h_flex, v_flex,
    },
    image_cache::{MAX_DECODE_PIXELS, MAX_DECODER_ALLOC_BYTES, MAX_IMAGE_DECODE_DIMENSION},
    theme::{ActiveTheme, Theme},
    util::{file_dialog::resolve as resolve_file_dialog, imgproxy::stream_cover_url},
};

const THUMBNAIL_ASPECT_RATIO: f32 = 16. / 9.;
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

#[derive(Clone)]
struct StreamKeyData {
    whip_url: String,
    token: String,
    expires_at: i64,
}

#[derive(Clone)]
struct KeyResult {
    key: Option<StreamKeyData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTarget {
    Server,
    Token,
}

pub struct StreamingTab {
    clan_id: ClanId,
    channel_id: ChannelId,
    settings: Entity<Settings>,
    // Thumbnail state
    phase: ThumbnailPhase,
    _picker_task: Option<Task<()>>,
    _operation_task: Option<Task<()>>,
    // Stream key state
    result: Option<KeyResult>,
    loading_key: bool,
    revealed: bool,
    request_counter: u64,
    copied_target: Option<CopyTarget>,
    _subs: Vec<Subscription>,
    _key_task: Option<Task<()>>,
    _countdown_task: Option<Task<()>>,
    _copy_reset_task: Option<Task<()>>,
}

impl StreamingTab {
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
            result: None,
            loading_key: false,
            revealed: false,
            request_counter: 0,
            copied_target: None,
            _subs: subs,
            _key_task: None,
            _countdown_task: None,
            _copy_reset_task: None,
        }
    }

    fn t(&self, key: &'static str, cx: &App) -> SharedString {
        mezon_i18n::t(&self.settings.read(cx).language, key)
            .to_string()
            .into()
    }

    // --- Stream Key Methods ---

    fn fetch_key(&mut self, cx: &mut Context<Self>) {
        if self.channel_id.is_zero() || self.loading_key {
            return;
        }
        self.request_counter = self.request_counter.wrapping_add(1);
        let req_id = self.request_counter;
        let channel_id = self.channel_id;
        self.loading_key = true;
        self.revealed = false;
        self._countdown_task = None;
        cx.notify();

        let Some(api) = crate::chat::channel_settings::channel_acl::api(cx) else {
            self.loading_key = false;
            self.result = Some(KeyResult { key: None });
            cx.notify();
            return;
        };

        self._key_task = Some(cx.spawn(async move |this, cx| {
            let resp = api.get_stream_playback(channel_id.get(), true).await;
            this.update(cx, |this, cx| {
                if this.request_counter != req_id {
                    return;
                }
                this.loading_key = false;
                let key = match resp {
                    Ok(pb) if !pb.token.is_empty() && !pb.whip_url.is_empty() => {
                        Some(StreamKeyData {
                            whip_url: pb.whip_url,
                            token: pb.token,
                            expires_at: pb.expires_at,
                        })
                    }
                    _ => None,
                };
                let expires_at = key.as_ref().map(|k| k.expires_at);
                this.result = Some(KeyResult { key });
                if let Some(exp) = expires_at {
                    this.start_countdown(exp, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn start_countdown(&mut self, expires_at: i64, cx: &mut Context<Self>) {
        let expires_at_ms = expires_at.saturating_mul(1000);
        self._countdown_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let active = this
                            .result
                            .as_ref()
                            .and_then(|r| r.key.as_ref())
                            .is_some_and(|k| k.expires_at == expires_at);
                        if !active {
                            return false;
                        }
                        cx.notify();
                        now_unix_ms() < expires_at_ms
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    fn copy_text(&mut self, text: String, target: CopyTarget, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_target = Some(target);
        cx.notify();
        self._copy_reset_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| {
                if this.copied_target == Some(target) {
                    this.copied_target = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    // --- Thumbnail Methods ---

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

    // --- Render Helpers ---

    fn render_copy_button(
        id: &'static str,
        locale: &str,
        theme: &Theme,
        copied: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(px(16.0))
                .text_color(if copied {
                    theme.status_online
                } else {
                    theme.text_primary
                }),
            )
            .child(
                Label::new(mezon_i18n::t(locale, "setting.streaming.copy"))
                    .text_sm()
                    .text_color(theme.text_primary),
            )
            .on_click(on_click)
    }

    fn render_key_section(
        &self,
        locale: &str,
        theme: &Theme,
        channel_name: String,
        can_manage: bool,
        entity: Entity<Self>,
    ) -> impl IntoElement {
        let stream_key = self
            .result
            .as_ref()
            .and_then(|r| r.key.as_ref())
            .cloned();
        let is_error = self.result.as_ref().is_some_and(|r| r.key.is_none());
        let loading = self.loading_key;
        let revealed = self.revealed;
        let server_copied = self.copied_target == Some(CopyTarget::Server);
        let token_copied = self.copied_target == Some(CopyTarget::Token);

        let button_disabled = loading || !can_manage;
        let button_label = mezon_i18n::t(
            locale,
            if stream_key.is_some() {
                "setting.streaming.regenerate"
            } else {
                "setting.streaming.getKey"
            },
        );

        let mut key_section = v_flex()
            .w_full()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(locale, "setting.streaming.keyTitle"))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(locale, "setting.streaming.keyDescription"))
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Label::new(
                            mezon_i18n::t(locale, "setting.streaming.channel").to_uppercase(),
                        )
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text_secondary),
                    )
                    .child(
                        h_flex()
                            .h(px(40.0))
                            .w_full()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .rounded_md()
                            .bg(theme.tokens.theme_setting_primary)
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                Icon::new(IconName::Live)
                                    .size(px(16.0))
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .child(channel_name),
                            ),
                    ),
            );

        if let Some(key) = &stream_key {
            let whip_url = key.whip_url.clone();
            let whip_for_copy = key.whip_url.clone();
            let token_for_copy = key.token.clone();
            let displayed_token = if revealed {
                key.token.clone()
            } else {
                "•".repeat(32)
            };
            let remaining_ms = key.expires_at.saturating_mul(1000) - now_unix_ms();
            let expiry_el = if remaining_ms > 0 {
                let text = mezon_i18n::t(locale, "setting.streaming.expiresIn")
                    .replace("{{time}}", &format_remaining_ms(remaining_ms));
                Label::new(text)
                    .text_xs()
                    .text_color(theme.tokens.text_theme_primary)
                    .into_any_element()
            } else {
                Label::new(mezon_i18n::t(locale, "setting.streaming.expired"))
                    .text_xs()
                    .text_color(theme.danger_text)
                    .into_any_element()
            };

            key_section = key_section.child(
                v_flex()
                    .gap_4()
                    .rounded_md()
                    .bg(theme.tokens.theme_setting_primary)
                    .border_1()
                    .border_color(theme.border)
                    .p_4()
                    .child(
                        h_flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            mezon_i18n::t(locale, "setting.streaming.server")
                                                .to_uppercase(),
                                        )
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_primary)
                                            .whitespace_normal()
                                            .child(whip_url),
                                    ),
                            )
                            .child(Self::render_copy_button(
                                "stream-key-copy-server",
                                locale,
                                theme,
                                server_copied,
                                {
                                    let entity = entity.clone();
                                    move |_, _, cx| {
                                        let text = whip_for_copy.clone();
                                        entity.update(cx, |this, cx| {
                                            this.copy_text(text, CopyTarget::Server, cx);
                                        });
                                    }
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            mezon_i18n::t(locale, "setting.streaming.token")
                                                .to_uppercase(),
                                        )
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text_secondary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_primary)
                                            .whitespace_normal()
                                            .child(displayed_token),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("stream-key-reveal")
                                            .px_3()
                                            .py_1()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_secondary)
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .child(
                                                Label::new(mezon_i18n::t(
                                                    locale,
                                                    if revealed {
                                                        "setting.streaming.hide"
                                                    } else {
                                                        "setting.streaming.show"
                                                    },
                                                ))
                                                .text_sm()
                                                .text_color(theme.text_primary),
                                            )
                                            .on_click({
                                                let entity = entity.clone();
                                                move |_, _, cx| {
                                                    entity.update(cx, |this, cx| {
                                                        this.revealed = !this.revealed;
                                                        cx.notify();
                                                    });
                                                }
                                            }),
                                    )
                                    .child(Self::render_copy_button(
                                        "stream-key-copy-token",
                                        locale,
                                        theme,
                                        token_copied,
                                        {
                                            let entity = entity.clone();
                                            move |_, _, cx| {
                                                let text = token_for_copy.clone();
                                                entity.update(cx, |this, cx| {
                                                    this.copy_text(text, CopyTarget::Token, cx);
                                                });
                                            }
                                        },
                                    )),
                            ),
                    )
                    .child(expiry_el),
            );
        }

        if is_error {
            key_section = key_section.child(
                Label::new(mezon_i18n::t(locale, "setting.streaming.error"))
                    .text_xs()
                    .text_color(theme.danger_text),
            );
        }

        let mut get_key_btn = div()
            .id("stream-get-key-btn")
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .rounded_lg()
            .bg(theme.brand)
            .when(button_disabled, |el| el.opacity(0.5))
            .when(!button_disabled, |el| el.cursor_pointer())
            .when(loading, |el| {
                el.child(Spinner::new().with_size(Size::Small).color(gpui::white()))
            })
            .child(
                Label::new(button_label)
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(gpui::white()),
            );

        if !button_disabled {
            let entity = entity.clone();
            get_key_btn = get_key_btn.on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| {
                    this.fetch_key(cx);
                });
            });
        }

        key_section.child(h_flex().child(get_key_btn))
    }

    fn render_thumbnail_section(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let brand = theme.brand;
        let avatar = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|channel| channel.avatar_url.clone())
            .filter(|url| !url.trim().is_empty() && url != "0");
        let disabled = self.phase != ThumbnailPhase::Idle
            || !can_manage_channel(self.clan_id, self.channel_id, cx);
        let mut content = v_flex().w_full().min_w_0().gap_3();

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
                                .icon(Icon::new(IconName::Edit).text_color(theme.text_primary))
                                .primary()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                        )
                        .child(
                            Button::new("thumbnail-remove")
                                .large()
                                .label(self.t("streamThumbnail.buttons.remove", cx))
                                .icon(Icon::new(IconName::Trash).text_color(theme.text_primary))
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
                                    Icon::new(IconName::Camera)
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
                                    Icon::new(IconName::Camera).text_color(theme.text_primary),
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
                IconName::FileImage,
            ),
            (
                "streamThumbnail.requirements.resolution.title",
                "streamThumbnail.requirements.resolution.value",
                IconName::ImageAdd,
            ),
            (
                "streamThumbnail.requirements.sizeLimit.title",
                "streamThumbnail.requirements.sizeLimit.value",
                IconName::File,
            ),
        ] {
            content = content.child(
                h_flex()
                    .p_4()
                    .gap_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.theme_setting_primary)
                    .items_center()
                    .child(
                        div()
                            .size(px(40.))
                            .rounded_lg()
                            .bg(theme.bg_hover)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if icon == IconName::ImageAdd {
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
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(locale, "streamThumbnail.title"))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(
                            locale,
                            "streamThumbnail.confirmModal.uploadMessage",
                        ))
                        .text_xs()
                        .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(content)
    }

    fn render_quality_section(
        &self,
        locale: &str,
        theme: &Theme,
        default_quality: mezon_store::StreamQuality,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_4()
            .rounded_lg()
            .bg(theme.tokens.theme_setting_nav)
            .border_1()
            .border_color(theme.border)
            .p_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(mezon_i18n::t(locale, "setting.streaming.qualityTitle"))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_primary),
                    )
                    .child(
                        Label::new(mezon_i18n::t(locale, "setting.streaming.qualityDescription"))
                            .text_xs()
                            .text_color(theme.tokens.text_theme_primary),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .children(STREAM_QUALITIES.iter().enumerate().map(|(idx, &quality)| {
                        let is_selected = default_quality == quality;
                        let label_key = quality.i18n_key();
                        h_flex()
                            .id(("stream-default-quality", idx))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(if is_selected {
                                theme.brand
                            } else {
                                theme.border
                            })
                            .when(is_selected, |el| el.bg(theme.bg_hover))
                            .when(!is_selected, |el| el.hover(|s| s.bg(theme.bg_hover)))
                            .cursor_pointer()
                            .child(
                                Label::new(mezon_i18n::t(locale, label_key))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary),
                            )
                            .child(
                                div()
                                    .size(px(16.0))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if is_selected {
                                        theme.brand
                                    } else {
                                        theme.text_muted
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(is_selected, |el| {
                                        el.child(div().size(px(8.0)).rounded_full().bg(theme.brand))
                                    }),
                            )
                            .on_click(move |_, _, cx| {
                                mezon_store::set_stream_default_quality(quality, cx);
                            })
                    })),
            )
    }
}

impl Render for StreamingTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let default_quality = self.settings.read(cx).stream_default_quality;
        let can_manage = can_manage_channel(self.clan_id, self.channel_id, cx);
        let entity = cx.entity();

        let channel_name = ChannelList::global(cx)
            .read(cx)
            .channel(self.clan_id, self.channel_id)
            .map(|ch| ch.name.clone())
            .unwrap_or_default();

        let page_title = div()
            .text_xl()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text_primary)
            .child(mezon_i18n::t(&locale, "setting.appSettings.streaming"));

        let key_section =
            self.render_key_section(&locale, &theme, channel_name, can_manage, entity);
        let thumbnail_section = self.render_thumbnail_section(&locale, &theme, cx);
        let quality_section = self.render_quality_section(&locale, &theme, default_quality);

        v_flex()
            .id("channel-streaming-tab")
            .w_full()
            .gap_6()
            .text_color(theme.text_primary)
            .child(page_title)
            .child(key_section)
            .child(thumbnail_section)
            .child(quality_section)
            .child(div().h(px(80.0)))
    }
}

struct ThumbnailConfirmation {
    tab: WeakEntity<StreamingTab>,
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
                button.icon(Icon::new(IconName::Trash).text_color(theme.text_primary))
            })
            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)));
        v_flex()
            .id("stream-thumbnail-modal")
            .track_focus(&self.focus_handle)
            .w(px(modal_width))
            .max_h(px(560.))
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.theme_setting_primary)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .p_6()
                    .border_b_1()
                    .border_color(theme.border)
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
                            .child(
                                div().text_sm().text_color(theme.text_muted).child(t(
                                    if upload {
                                        "streamThumbnail.confirmModal.uploadDescription"
                                    } else {
                                        "streamThumbnail.confirmModal.removeQuestion"
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .id("thumbnail-modal-close")
                            .cursor_pointer()
                            .p_1()
                            .rounded_md()
                            .hover(|style| style.bg(theme.bg_hover))
                            .child(Icon::new(IconName::Close).text_color(theme.text_muted))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx))
                            }),
                    ),
            )
            .child(body)
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .items_center()
                    .gap_3()
                    .p_6()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        Button::new("thumbnail-cancel")
                            .large()
                            .label(t("streamThumbnail.buttons.cancel"))
                            .on_click(|_, _, cx| {
                                Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx))
                            }),
                    )
                    .child(if upload { confirm.primary() } else { confirm.danger() }),
            )
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn format_remaining_ms(remaining_ms: i64) -> String {
    if remaining_ms <= 0 {
        return "0:00".to_string();
    }
    let seconds = (remaining_ms + 999) / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
