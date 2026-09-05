use crate::app::shell::Shell;
use crate::components::primitives::{
    Avatar, Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Spinner, h_flex,
    v_flex,
};
use crate::theme::ActiveTheme;
use crate::util::imgproxy;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, PathPromptOptions, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use mezon_store::{
    ChannelId, ClanImageMimeType, ClanList, DirectMessageStore, validate_channel_name,
};

const MAX_GROUP_AVATAR_BYTES: u64 = 8 * 1024 * 1024;

pub struct EditGroupModal {
    focus_handle: FocusHandle,
    channel_id: ChannelId,
    locale: String,
    original_label: String,
    name_input: Entity<InputState>,
    avatar_current: String,
    avatar_dirty: bool,
    uploading: bool,
    saving: bool,
    _input_sub: Subscription,
    _upload_task: Option<Task<()>>,
    _save_task: Option<Task<()>>,
}

impl Focusable for EditGroupModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditGroupModal {
    pub fn new(
        channel_id: ChannelId,
        label: String,
        avatar: String,
        locale: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "directMessage.editGroup.groupNamePlaceholder")
                .to_string()
                .into();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(40.))
                .radius(px(4.))
        });
        let input_sub = cx.subscribe(&name_input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter => this.save(cx),
        });
        name_input.update(cx, |input, cx| {
            input.set_value(&label, window, cx);
            input.focus(window, cx);
        });

        Self {
            focus_handle: cx.focus_handle(),
            channel_id,
            locale,
            original_label: label,
            name_input,
            avatar_current: avatar,
            avatar_dirty: false,
            uploading: false,
            saving: false,
            _input_sub: input_sub,
            _upload_task: None,
            _save_task: None,
        }
    }

    /// Releases the picker/upload guard. Always notifies: the guard is claimed
    /// before the native dialog opens, so cancelling it has to clear the spinner.
    fn finish_upload(&mut self, cx: &mut Context<Self>) {
        self._upload_task = None;
        self.uploading = false;
        cx.notify();
    }

    fn pick_avatar(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading || self.saving {
            return;
        }
        let prompt: SharedString = mezon_i18n::t(&self.locale, "setting.profile.chooseAvatar")
            .to_string()
            .into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        // Claim the guard before awaiting the picker, otherwise a second click
        // opens a second native dialog while the first is still up.
        self.uploading = true;
        cx.notify();
        self._upload_task.take();
        self._upload_task = Some(cx.spawn(async move |this, cx| {
            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                return;
            };
            let path = match paths.into_iter().next() {
                Some(path) => path,
                None => {
                    let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                    return;
                }
            };

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !ClanImageMimeType::is_allowed_extension(&ext) {
                tracing::warn!("group avatar pick rejected: unsupported file type .{ext}");
                let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                return;
            }

            let path_buf = path.clone();
            let file_size = match cx
                .background_spawn(async move { std::fs::metadata(&path_buf).ok().map(|m| m.len()) })
                .await
            {
                Some(size) => size,
                None => {
                    tracing::warn!("group avatar pick: cannot read file metadata");
                    let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                    return;
                }
            };
            if file_size > MAX_GROUP_AVATAR_BYTES {
                tracing::warn!(
                    "group avatar pick rejected: file size {} > {} bytes",
                    file_size,
                    MAX_GROUP_AVATAR_BYTES
                );
                let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                return;
            }

            let task = match this.update(cx, |_, cx| {
                ClanList::global(cx).update(cx, |store, cx| {
                    store.upload_clan_image(&path, MAX_GROUP_AVATAR_BYTES, cx)
                })
            }) {
                Ok(task) => task,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                    return;
                }
            };
            match task.await {
                Ok(url) => {
                    let _ = this.update(cx, |modal, cx| {
                        modal.avatar_current = url;
                        modal.avatar_dirty = true;
                        modal.finish_upload(cx);
                    });
                }
                Err(err) => {
                    tracing::error!("group avatar upload failed: {err}");
                    let _ = this.update(cx, |this, cx| this.finish_upload(cx));
                }
            }
        }));
    }

    fn remove_avatar(&mut self, cx: &mut Context<Self>) {
        if self.uploading || self.saving {
            return;
        }
        self.avatar_current.clear();
        self.avatar_dirty = true;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.uploading {
            return;
        }
        let value = self.name_input.read(cx).value().to_string();
        let Ok(label) = validate_channel_name(&value) else {
            cx.notify();
            return;
        };
        let label_opt = (label != self.original_label).then_some(label);
        let avatar_opt = self.avatar_dirty.then(|| self.avatar_current.clone());
        if label_opt.is_none() && avatar_opt.is_none() {
            Self::close(cx);
            return;
        }
        self.saving = true;
        cx.notify();

        let channel_id = self.channel_id;
        let locale = self.locale.clone();
        let task = DirectMessageStore::global(cx).update(cx, |store, cx| {
            store.update_group(channel_id, label_opt, avatar_opt, cx)
        });
        self._save_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                let alive = this.update(cx, |_, _| {}).is_ok();
                let saved = mezon_i18n::t(&locale, "directMessage.editGroup.saved").to_string();
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.success(saved, cx);
                        if alive {
                            shell.close_modal(cx);
                        }
                    });
                });
            }
            Err(err) => {
                // The transport error is for the log; the toast gets the localized
                // message so users never see raw wire text.
                tracing::error!("update group failed: {err}");
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                });
                let failed =
                    mezon_i18n::t(&locale, "directMessage.editGroup.saveFailed").to_string();
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(failed, cx));
                });
            }
        }));
    }

    fn close(cx: &mut App) {
        Shell::global(cx).update(cx, |shell, cx| shell.close_modal(cx));
    }
}

impl Render for EditGroupModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = self.locale.clone();

        let title: SharedString = mezon_i18n::t(&locale, "directMessage.editGroup.title")
            .to_string()
            .into();
        let upload_text: SharedString =
            mezon_i18n::t(&locale, "directMessage.editGroup.uploadImageText")
                .to_string()
                .into();
        let remove_label: SharedString =
            mezon_i18n::t(&locale, "directMessage.editGroup.removeAvatar")
                .to_string()
                .into();
        let name_label: SharedString =
            mezon_i18n::t(&locale, "directMessage.editGroup.groupNameLabel")
                .to_string()
                .to_uppercase()
                .into();
        let validation_msg: SharedString =
            mezon_i18n::t(&locale, "directMessage.editGroup.validationError")
                .to_string()
                .into();
        let cancel_label: SharedString = mezon_i18n::t(&locale, "directMessage.editGroup.cancel")
            .to_string()
            .into();
        let save_label: SharedString = if self.saving {
            mezon_i18n::t(&locale, "directMessage.editGroup.saving")
                .to_string()
                .into()
        } else {
            mezon_i18n::t(&locale, "directMessage.editGroup.save")
                .to_string()
                .into()
        };

        let value = self.name_input.read(cx).value().to_string();
        let char_count = value.chars().count();
        let valid = validate_channel_name(&value).is_ok();
        let show_error = !value.is_empty() && !valid;
        let can_save = !value.trim().is_empty() && valid && !self.saving && !self.uploading;

        let has_custom_avatar = !self.avatar_current.is_empty();
        let avatar_src = if has_custom_avatar {
            imgproxy::avatar_url(cx, &self.avatar_current)
        } else {
            String::new()
        };
        let mut avatar = Avatar::new()
            .name(self.original_label.clone())
            .size_px(px(80.))
            .group_default(!has_custom_avatar);
        if !avatar_src.is_empty() {
            avatar = avatar.src(avatar_src.clone());
            if self.avatar_current != avatar_src {
                avatar = avatar.fallback_src(self.avatar_current.clone());
            }
        }

        let uploading = self.uploading;

        v_flex()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| Self::close(cx)))
            .w(px(440.))
            .max_w(px(440.))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(theme.tokens.theme_setting_primary)
            .shadow_lg()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_4()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.tokens.text_theme_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .id("edit-group-close")
                            .size(px(32.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .opacity(0.65)
                            .hover(|s| s.opacity(1.0))
                            .on_click(|_, _, cx| Self::close(cx))
                            .child(
                                Icon::new(IconName::Close)
                                    .size(px(20.))
                                    .text_color(theme.text_secondary),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .px_4()
                    .pt(px(16.))
                    .pb_4()
                    .gap_5()
                    .child(
                        v_flex()
                            .w_full()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("edit-group-avatar")
                                    .group("edit-group-avatar-zone")
                                    .relative()
                                    .size(px(80.))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_avatar(window, cx)
                                    }))
                                    .child(avatar)
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .size(px(80.))
                                            .rounded_full()
                                            .bg(gpui::hsla(0., 0., 0., 0.6))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(!uploading, |el| {
                                                el.invisible()
                                                    .group_hover("edit-group-avatar-zone", |s| {
                                                        s.visible()
                                                    })
                                                    .child(
                                                        Icon::new(IconName::UploadImage)
                                                            .size(px(20.))
                                                            .text_color(gpui::white()),
                                                    )
                                            })
                                            .when(uploading, |el| el.child(Spinner::new())),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(upload_text),
                            )
                            .children(has_custom_avatar.then(|| {
                                div()
                                    .id("edit-group-remove-avatar")
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(gpui::rgb(0xf2_3f_42))
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.8))
                                    .on_click(
                                        cx.listener(|this, _, _window, cx| this.remove_avatar(cx)),
                                    )
                                    .child(remove_label.clone())
                            })),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(name_label),
                            )
                            .child(Input::new(&self.name_input))
                            .children(show_error.then(|| {
                                div()
                                    .text_xs()
                                    .text_color(gpui::rgb(0xf2_3f_42))
                                    .child(validation_msg.clone())
                            }))
                            .child(
                                h_flex().w_full().justify_end().child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.tokens.text_theme_primary)
                                        .child(format!("{char_count}/64")),
                                ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_3()
                    .px_4()
                    .py_4()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        Button::new("edit-group-cancel")
                            .label(cancel_label)
                            .ghost()
                            .text_color(gpui::Hsla::from(theme.text_secondary))
                            .on_click(|_, _, cx| Self::close(cx)),
                    )
                    .child(
                        Button::new("edit-group-save")
                            .label(save_label)
                            .primary()
                            .min_w(px(96.))
                            .disabled(!can_save)
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, _window, cx| this.save(cx))),
                    ),
            )
    }
}
