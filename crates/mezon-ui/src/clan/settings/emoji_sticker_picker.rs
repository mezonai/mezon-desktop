use std::path::PathBuf;

use gpui::{
    App, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions,
    SharedString, Subscription, Task, Window, div, img, prelude::*, px,
};
use mezon_store::{
    ClanId, EMOJI_SHORTNAME_MAX, EMOTICON_SHORTNAME_MIN, EmojiStore, EmoticonErrorKind,
    MAX_EMOJI_BYTES, MAX_STICKER_BYTES, Settings, StickerStore, is_valid_emoticon_shortname,
    strip_emoji_colons, validate_emoticon_file,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Checkbox, Icon, IconName, Input, InputEvent, InputState, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

const FORM_CONTROL_H: f32 = 34.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EmoticonKind {
    Emoji,
    Sticker,
}

#[derive(Clone)]
pub struct EmoticonEditTarget {
    pub id: String,
    pub shortname: String,
    pub source: String,
}

pub enum EmojiStickerPickerEvent {
    Saved,
    Cancelled,
}

#[derive(Clone)]
enum EmoticonPreview {
    Local(PathBuf),
    Remote(SharedString),
}

pub struct EmojiStickerPicker {
    kind: EmoticonKind,
    clan_id: ClanId,
    editing: Option<EmoticonEditTarget>,
    settings: Entity<Settings>,
    focus_handle: FocusHandle,
    name_input: Entity<InputState>,
    picked_path: Option<PathBuf>,
    file_label: SharedString,
    preview: Option<EmoticonPreview>,
    is_for_sale: bool,
    submitting: bool,
    _name_sub: Subscription,
    _pick_task: Option<Task<()>>,
    _submit_task: Option<Task<()>>,
}

impl EventEmitter<EmojiStickerPickerEvent> for EmojiStickerPicker {}

impl Focusable for EmojiStickerPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EmojiStickerPicker {
    pub fn new(
        kind: EmoticonKind,
        clan_id: ClanId,
        editing: Option<EmoticonEditTarget>,
        settings: Entity<Settings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = settings.read(cx).language.clone();
        let placeholder_key = match kind {
            EmoticonKind::Emoji => "clanEmojiSetting.modal.exCatHug",
            EmoticonKind::Sticker => "clanStickerSetting.modal.exCatHug",
        };
        let placeholder: SharedString = mezon_i18n::t(&locale, placeholder_key).to_string().into();
        let initial_name = editing
            .as_ref()
            .map(|e| strip_emoji_colons(&e.shortname))
            .unwrap_or_default();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(FORM_CONTROL_H))
        });
        if !initial_name.is_empty() {
            name_input.update(cx, |state, cx| {
                state.set_value(initial_name, window, cx);
            });
        }
        let name_sub = cx.subscribe(&name_input, |_, _, _: &InputEvent, cx| cx.notify());
        let file_label = editing
            .as_ref()
            .and_then(|e| e.source.split('/').next_back())
            .map(SharedString::from)
            .unwrap_or_else(|| {
                mezon_i18n::t(&locale, "clanEmojiSetting.modal.chooseAFile")
                    .to_string()
                    .into()
            });
        let preview = editing.as_ref().map(|e| {
            let proxied = match kind {
                EmoticonKind::Emoji => {
                    crate::util::imgproxy::proxied(cx, &e.source, 128, 128, "fit")
                }
                EmoticonKind::Sticker => {
                    crate::util::imgproxy::proxied(cx, &e.source, 160, 160, "fit")
                }
            };
            let url = if proxied.is_empty() {
                e.source.clone()
            } else {
                proxied
            };
            EmoticonPreview::Remote(url.into())
        });
        Self {
            kind,
            clan_id,
            editing,
            settings,
            focus_handle: cx.focus_handle(),
            name_input,
            picked_path: None,
            file_label,
            preview,
            is_for_sale: false,
            submitting: false,
            _name_sub: name_sub,
            _pick_task: None,
            _submit_task: None,
        }
    }

    fn max_bytes(&self) -> u64 {
        match self.kind {
            EmoticonKind::Emoji => MAX_EMOJI_BYTES,
            EmoticonKind::Sticker => MAX_STICKER_BYTES,
        }
    }

    fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    fn can_submit(&self, cx: &App) -> bool {
        if self.submitting {
            return false;
        }
        let name = strip_emoji_colons(self.name_input.read(cx).value());
        if !is_valid_emoticon_shortname(&name) || name.chars().count() > EMOJI_SHORTNAME_MAX {
            return false;
        }
        if let Some(editing) = &self.editing {
            return strip_emoji_colons(&editing.shortname) != name;
        }
        self.picked_path.is_some()
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        if self.is_editing() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let prompt_key = match self.kind {
            EmoticonKind::Emoji => "clanEmojiSetting.modal.chooseAFile",
            EmoticonKind::Sticker => "clanStickerSetting.modal.chooseAFile",
        };
        let prompt: SharedString = mezon_i18n::t(&locale, prompt_key).to_string().into();
        let max_bytes = self.max_bytes();
        let kind = self.kind;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        self._pick_task = Some(cx.spawn(async move |this, cx| {
            let finish = |this: &mut EmojiStickerPicker| {
                this._pick_task = None;
            };
            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    cx.notify();
                });
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                let _ = this.update(cx, |this, _| finish(this));
                return;
            };
            let path_buf = path.clone();
            let validated = cx
                .background_spawn(async move { validate_emoticon_file(&path_buf, max_bytes) })
                .await;
            if let Err(error) = validated {
                let empty_file_label: SharedString =
                    mezon_i18n::t(&locale, prompt_key).to_string().into();
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    this.picked_path = None;
                    this.file_label = empty_file_label;
                    this.preview = None;
                    cx.notify();
                });
                let feedback = this.update_in(cx, |_, window, cx| {
                    show_emoticon_error(kind, &locale, error, max_bytes, window, cx);
                });
                if let Err(err) = feedback {
                    tracing::warn!("failed to show emoticon validation feedback: {err}");
                }
                return;
            }
            let label = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let _ = this.update(cx, |this, cx| {
                finish(this);
                this.picked_path = Some(path.clone());
                this.file_label = label.into();
                this.preview = Some(EmoticonPreview::Local(path));
                cx.notify();
            });
        }));
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let name = strip_emoji_colons(self.name_input.read(cx).value());
        if !is_valid_emoticon_shortname(&name) || name.chars().count() > EMOJI_SHORTNAME_MAX {
            let message = match self.kind {
                EmoticonKind::Emoji => {
                    mezon_i18n::t(&locale, "clanEmojiSetting.toast.validateName")
                }
                EmoticonKind::Sticker => {
                    mezon_i18n::t(&locale, "clanStickerSetting.toast.validateName")
                }
            }
            .replace("{{min}}", &EMOTICON_SHORTNAME_MIN.to_string())
            .replace("{{max}}", &EMOJI_SHORTNAME_MAX.to_string());
            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            return;
        }

        self.submitting = true;
        cx.notify();
        let clan_id = self.clan_id;
        let kind = self.kind;
        let is_for_sale = self.is_for_sale;
        let editing = self.editing.clone();
        let picked_path = self.picked_path.clone();

        self._submit_task = Some(cx.spawn(async move |this, cx| {
            let task = match (editing.as_ref(), picked_path.as_ref()) {
                (Some(target), _) => cx.update(|cx| match kind {
                    EmoticonKind::Emoji => EmojiStore::global(cx).update(cx, |store, cx| {
                        store.update_emoji(&target.id, clan_id, &name, cx)
                    }),
                    EmoticonKind::Sticker => StickerStore::global(cx).update(cx, |store, cx| {
                        store.update_sticker(&target.id, clan_id, &target.source, &name, cx)
                    }),
                }),
                (None, Some(path)) => cx.update(|cx| match kind {
                    EmoticonKind::Emoji => EmojiStore::global(cx).update(cx, |store, cx| {
                        store.create_emoji(clan_id, path, &name, is_for_sale, cx)
                    }),
                    EmoticonKind::Sticker => StickerStore::global(cx).update(cx, |store, cx| {
                        store.create_sticker(clan_id, path, &name, is_for_sale, cx)
                    }),
                }),
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    show_error(cx, "missing file".into());
                    return;
                }
            };
            let result = task.await;

            match result {
                Ok(()) => {
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.emit(EmojiStickerPickerEvent::Saved);
                        cx.notify();
                    });
                    cx.update(|cx| {
                        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
                    });
                }
                Err(e) => {
                    tracing::error!("emoticon save failed: {e}");
                    let error = e.kind();
                    let max_bytes = match kind {
                        EmoticonKind::Emoji => MAX_EMOJI_BYTES,
                        EmoticonKind::Sticker => MAX_STICKER_BYTES,
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.submitting = false;
                        cx.notify();
                    });
                    let feedback = this.update_in(cx, |_, window, cx| {
                        show_emoticon_error(kind, &locale, error, max_bytes, window, cx);
                    });
                    if let Err(err) = feedback {
                        tracing::warn!("failed to show emoticon upload feedback: {err}");
                    }
                }
            }
        }));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(EmojiStickerPickerEvent::Cancelled);
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }

    fn render_file_field(
        &self,
        theme: &Theme,
        file_label_title: SharedString,
        browse: SharedString,
        is_editing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(file_label_title.to_uppercase()),
                    )
                    .child(div().text_xs().text_color(theme.danger_text).child("*")),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(FORM_CONTROL_H))
                            .flex()
                            .items_center()
                            .pl(px(12.))
                            .pr(px(16.))
                            .rounded_md()
                            .border_2()
                            .border_color(theme.border)
                            .bg(theme.bg_secondary)
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(theme.text_primary)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(self.file_label.clone()),
                            ),
                    )
                    .child({
                        let mut browse_btn = Button::new("emoticon-browse").label(browse);
                        if is_editing {
                            browse_btn = browse_btn.disabled(true);
                        }
                        browse_btn.on_click(cx.listener(|this, _, _, cx| this.pick_file(cx)))
                    }),
            )
    }

    fn render_name_field(
        &self,
        theme: &Theme,
        name_label: SharedString,
        name_len: usize,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.text_secondary)
                                    .child(name_label.to_uppercase()),
                            )
                            .child(div().text_xs().text_color(theme.danger_text).child("*")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if name_len > 40 {
                                theme.danger_text
                            } else {
                                theme.text_muted
                            })
                            .child(format!("{name_len}/{EMOJI_SHORTNAME_MAX}")),
                    ),
            )
            .child(Input::new(&self.name_input))
    }
}

fn emoticon_error_message(kind: EmoticonKind, locale: &str, error: EmoticonErrorKind) -> String {
    match error {
        EmoticonErrorKind::SizeLimit | EmoticonErrorKind::ImageTooLarge => match kind {
            EmoticonKind::Emoji => mezon_i18n::t(locale, "clanEmojiSetting.toast.errorSizeLimit"),
            EmoticonKind::Sticker => {
                mezon_i18n::t(locale, "clanStickerSetting.toast.errorSizeLimit")
            }
        }
        .to_string(),
        EmoticonErrorKind::InvalidName => match kind {
            EmoticonKind::Emoji => mezon_i18n::t(locale, "clanEmojiSetting.toast.validateName"),
            EmoticonKind::Sticker => mezon_i18n::t(locale, "clanStickerSetting.toast.validateName"),
        }
        .replace("{{min}}", &EMOTICON_SHORTNAME_MIN.to_string())
        .replace("{{max}}", &EMOJI_SHORTNAME_MAX.to_string()),
        EmoticonErrorKind::UnsupportedType
        | EmoticonErrorKind::Empty
        | EmoticonErrorKind::InvalidImage => {
            mezon_i18n::t(locale, "common.onlyImageFiles").to_string()
        }
        EmoticonErrorKind::Other => mezon_i18n::t(locale, "common.somethingWentWrong").to_string(),
    }
}

fn show_error(cx: &mut AsyncApp, message: String) {
    cx.update(|cx| {
        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
    });
}

fn show_emoticon_error(
    kind: EmoticonKind,
    locale: &str,
    error: EmoticonErrorKind,
    max_bytes: u64,
    window: &mut Window,
    cx: &mut App,
) {
    if matches!(
        error,
        EmoticonErrorKind::SizeLimit | EmoticonErrorKind::ImageTooLarge
    ) {
        show_emoticon_upload_limit(locale, max_bytes, window, cx);
    } else {
        let message = emoticon_error_message(kind, locale, error);
        Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
    }
}

fn show_emoticon_upload_limit(locale: &str, max_bytes: u64, window: &mut Window, cx: &mut App) {
    let title = mezon_i18n::t(locale, "common.filesTooPowerful");
    let size_limit = format!("{} KB", max_bytes / 1024);
    let content = mezon_i18n::t(locale, "common.maxFileSize").replace("{{sizeLimit}}", &size_limit);
    Shell::global(cx).update(cx, |shell, cx| {
        shell.show_upload_limit(title, content, window, cx);
    });
}

impl Render for EmojiStickerPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let is_emoji = self.kind == EmoticonKind::Emoji;
        let title = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.uploadAFile"
            } else {
                "clanStickerSetting.modal.uploadAFile"
            },
        );
        let subtitle = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.fileShouldBeAPNGPNGOrGIF256KBMax"
            } else {
                "clanStickerSetting.modal.fileShouldBeAPNGPNGOrGIF256KBMax"
            },
        );
        let name_label = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.stickerName"
            } else {
                "clanStickerSetting.modal.stickerName"
            },
        );
        let browse = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.browse"
            } else {
                "clanStickerSetting.modal.browse"
            },
        );
        let never_mind = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.neverMind"
            } else {
                "clanStickerSetting.modal.neverMind"
            },
        );
        let upload_label = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.button.upload"
            } else {
                "clanStickerSetting.btn.upload"
            },
        );
        let for_sale = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.thisIsForSale"
            } else {
                "clanStickerSetting.modal.thisIsForSale"
            },
        );
        let preview_label = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.preview"
            } else {
                "clanStickerSetting.modal.preview"
            },
        );
        let file_label_title = mezon_i18n::t(
            &locale,
            if is_emoji {
                "clanEmojiSetting.modal.file"
            } else {
                "clanStickerSetting.modal.file"
            },
        );
        let can_submit = self.can_submit(cx);
        let name_len = strip_emoji_colons(self.name_input.read(cx).value())
            .chars()
            .count();
        let is_editing = self.is_editing();

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|this, _: &::menu::Cancel, _window, cx| this.close(cx)))
            .w(px(684.))
            .max_h(px(760.))
            .gap(px(20.))
            .px(px(24.))
            .pt(px(20.))
            .pb(px(24.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg_secondary)
            .shadow_lg()
            .child(
                h_flex()
                    .relative()
                    .w_full()
                    .justify_center()
                    .items_center()
                    .pb(px(4.))
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_primary)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.text_secondary)
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        div()
                            .id("emoticon-picker-close")
                            .absolute()
                            .right_0()
                            .top_0()
                            .p_1()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.bg_hover))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(16.))
                                    .text_color(theme.text_secondary),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.text_secondary)
                            .child(preview_label.to_uppercase()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(224.))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .flex_1()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme.tokens.bg_tertiary)
                                    .child(preview_image(
                                        self.preview.as_ref(),
                                        theme.tokens.text_theme_primary,
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(preview_image(
                                        self.preview.as_ref(),
                                        theme.tokens.text_theme_primary,
                                    )),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(self.render_file_field(
                        &theme,
                        file_label_title.into(),
                        browse.into(),
                        is_editing,
                        cx,
                    ))
                    .child(self.render_name_field(&theme, name_label.into(), name_len)),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        Checkbox::new("emoticon-for-sale")
                            .checked(self.is_for_sale)
                            .label(for_sale)
                            .disabled(is_editing)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.is_for_sale = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("emoticon-cancel")
                                    .label(never_mind)
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                            .child({
                                let mut upload_btn =
                                    Button::new("emoticon-upload").label(upload_label).primary();
                                if !can_submit {
                                    upload_btn = upload_btn.disabled(true);
                                }
                                upload_btn.on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                            }),
                    ),
            )
    }
}

fn preview_image(
    preview: Option<&EmoticonPreview>,
    placeholder_color: impl Into<gpui::Hsla>,
) -> gpui::AnyElement {
    match preview {
        Some(EmoticonPreview::Local(path)) => img(path.clone())
            .size(px(72.))
            .object_fit(gpui::ObjectFit::Contain)
            .rounded_md()
            .into_any_element(),
        Some(EmoticonPreview::Remote(url)) => img(url.clone())
            .size(px(72.))
            .object_fit(gpui::ObjectFit::Contain)
            .rounded_md()
            .into_any_element(),
        None => Icon::new(IconName::UploadImage)
            .size(px(50.))
            .text_color(placeholder_color)
            .into_any_element(),
    }
}
