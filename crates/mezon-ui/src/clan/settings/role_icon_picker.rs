use gpui::{
    App, AsyncApp, Context, Entity, FontWeight, MouseButton, PathPromptOptions, SharedString,
    Window, deferred, div, hsla, img, prelude::*, px,
};

use mezon_store::{ClanList, EmojiStore, MAX_ROLE_ICON_BYTES, validate_clan_image_file};

use super::role_setting_page::RoleSettingPage;
use crate::app::shell::Shell;
use crate::chat::message::{ReactionPicker, ReactionPickerEvent};
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Sizable, Size, h_flex, v_flex,
};
use crate::theme::Theme;

const ROLE_ICON_PREVIEW_SIZE: f32 = 64.0;
const MODAL_W: f32 = 600.0;
const MODAL_H: f32 = 540.0;
const UPLOAD_TARGET: f32 = 150.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoleIconPickerTab {
    Image,
    Emoji,
}

pub fn render_role_icon_section(
    page: &mut RoleSettingPage,
    locale: &str,
    theme: &Theme,
    can_edit: bool,
    _window: &mut Window,
    cx: &mut Context<RoleSettingPage>,
) -> impl IntoElement {
    page.render_role_icon_controls(locale, theme, can_edit, cx)
}

pub fn render_role_icon_picker_modal(
    page: Entity<RoleSettingPage>,
    locale: &str,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let tab = page.read(cx).role_icon_picker_tab;
    let uploading = page.read(cx).icon_uploading;
    let focus_handle = page.read(cx).role_icon_picker_focus();
    let upload_label: SharedString = mezon_i18n::t(locale, "common.roleIcon.uploadImage").into();
    let emoji_label: SharedString = mezon_i18n::t(locale, "common.roleIcon.emoji").into();
    let emoji_picker = page.read(cx).role_icon_emoji_picker.clone();

    let viewport = window.viewport_size();
    let modal_w = f32::from(viewport.width).clamp(320.0, MODAL_W);
    let modal_h = (f32::from(viewport.height) - 48.0).clamp(360.0, MODAL_H);

    deferred(
        div()
            .id("role-icon-picker-backdrop")
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.8))
            .key_context("menu")
            .on_action({
                let page = page.clone();
                move |_: &::menu::Cancel, _, cx| {
                    page.update(cx, |this, cx| {
                        if !this.icon_uploading {
                            this.close_role_icon_picker(cx);
                        }
                    });
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let page = page.clone();
                move |_, _, cx| {
                    page.update(cx, |this, cx| {
                        if !this.icon_uploading {
                            this.close_role_icon_picker(cx);
                        }
                    });
                }
            })
            .child(
                div()
                    .id("role-icon-picker-modal")
                    .track_focus(&focus_handle)
                    .occlude()
                    .relative()
                    .w(px(modal_w))
                    .h(px(modal_h))
                    .max_w(gpui::relative(0.95))
                    .max_h(gpui::relative(0.9))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded_lg()
                    .bg(theme.bg_primary)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(render_role_icon_tab_button(
                                page.clone(),
                                theme,
                                RoleIconPickerTab::Image,
                                tab,
                                upload_label,
                                window,
                                cx,
                            ))
                            .child(render_role_icon_tab_button(
                                page.clone(),
                                theme,
                                RoleIconPickerTab::Emoji,
                                tab,
                                emoji_label,
                                window,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .when(tab == RoleIconPickerTab::Image, |panel| {
                                panel.items_center().justify_center().child(
                                    render_role_icon_upload_panel(page.clone(), locale, theme, cx),
                                )
                            })
                            .when(tab == RoleIconPickerTab::Emoji, |panel| {
                                panel.child(
                                    div()
                                        .w_full()
                                        .h_full()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .flex()
                                        .flex_col()
                                        .when_some(emoji_picker, |col, picker| col.child(picker)),
                                )
                            }),
                    )
                    .when(uploading, |modal| {
                        modal.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(hsla(0., 0., 0., 0.6))
                                .rounded_lg()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(gpui::white()),
                                ),
                        )
                    }),
            ),
    )
    .with_priority(1)
}

impl RoleSettingPage {
    fn render_role_icon_controls(
        &self,
        locale: &str,
        theme: &Theme,
        can_edit: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = self.draft_icon.clone();
        let has_icon = !icon.is_empty();
        let uploading = self.icon_uploading;

        h_flex()
            .gap_3()
            .items_start()
            .child(self.render_role_icon_preview(&icon, has_icon, theme))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(!can_edit, |row| row.opacity(0.5))
                    .child(
                        Button::new("role-icon-choose")
                            .label(mezon_i18n::t(
                                locale,
                                "clanRoles.roleManagement.chooseImage",
                            ))
                            .primary()
                            .with_size(Size::Medium)
                            .disabled(!can_edit || uploading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_role_icon_picker(window, cx);
                            })),
                    )
                    .when(has_icon, |row| {
                        row.child(
                            Button::new("role-icon-remove")
                                .label(mezon_i18n::t(locale, "clanRoles.roleManagement.removeIcon"))
                                .danger()
                                .with_size(Size::Medium)
                                .disabled(!can_edit || uploading)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remove_role_icon(cx);
                                })),
                        )
                    }),
            )
    }

    fn render_role_icon_preview(
        &self,
        icon: &str,
        has_icon: bool,
        theme: &Theme,
    ) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .size(px(ROLE_ICON_PREVIEW_SIZE))
            .rounded(px(4.0))
            .overflow_hidden()
            .bg(theme.tokens.bg_tertiary)
            .flex()
            .items_center()
            .justify_center()
            .when(has_icon, |el| {
                el.child(
                    img(icon.to_string())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
            })
            .when(!has_icon, |el| {
                el.child(
                    Icon::new(IconName::ImageUploadIcon)
                        .size(px(20.0))
                        .text_color(theme.tokens.text_theme_primary),
                )
            })
    }

    pub(super) fn open_role_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        EmojiStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        self.role_icon_picker_open = true;
        self.role_icon_picker_tab = RoleIconPickerTab::Image;
        window.focus(&self.role_icon_picker_focus, cx);
        cx.notify();
    }

    pub(super) fn close_role_icon_picker(&mut self, cx: &mut Context<Self>) {
        self.role_icon_picker_open = false;
        self.role_icon_emoji_picker = None;
        self._role_icon_emoji_picker_sub = None;
        cx.notify();
    }

    pub(super) fn set_role_icon_picker_tab(
        &mut self,
        tab: RoleIconPickerTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.role_icon_picker_tab != tab {
            self.role_icon_picker_tab = tab;
            if tab == RoleIconPickerTab::Emoji {
                self.ensure_role_icon_emoji_picker(window, cx);
            }
            cx.notify();
        }
    }

    fn ensure_role_icon_emoji_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.role_icon_emoji_picker.is_some() {
            return;
        }
        let picker = cx.new(|cx| ReactionPicker::new_in_container(window, cx));
        let sub = cx.subscribe(&picker, |this, _, event: &ReactionPickerEvent, cx| {
            let ReactionPickerEvent::Picked { emoji_id, .. } = event;
            let url = crate::util::imgproxy::emoji_url(cx, emoji_id);
            this.set_draft_icon(url, cx);
            this.close_role_icon_picker(cx);
        });
        self.role_icon_emoji_picker = Some(picker);
        self._role_icon_emoji_picker_sub = Some(sub);
    }

    pub(super) fn set_draft_icon(&mut self, icon: String, cx: &mut Context<Self>) {
        self.draft_icon = icon;
        cx.notify();
    }

    pub(super) fn remove_role_icon(&mut self, cx: &mut Context<Self>) {
        self.set_draft_icon(String::new(), cx);
    }

    pub(super) fn pick_role_icon(&mut self, cx: &mut Context<Self>) {
        if self.icon_uploading {
            return;
        }
        self.icon_uploading = true;
        cx.notify();

        let locale = self.settings.read(cx).language.clone();
        let prompt: SharedString =
            mezon_i18n::t(&locale, "clanRoles.roleManagement.chooseImage").into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        let task = cx.spawn(async move |this, cx| {
            let finish = |this: &mut RoleSettingPage| {
                this.icon_uploading = false;
                this._icon_upload_task = None;
            };
            let show_error = |cx: &mut AsyncApp, message: String| {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
            };

            let paths = match rx.await {
                Ok(Ok(Some(p))) => p,
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    return;
                }
            };
            let path = match paths.into_iter().next() {
                Some(p) => p,
                None => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    return;
                }
            };
            let path_for_validate = path.clone();
            let validation = cx
                .background_spawn(async move {
                    validate_clan_image_file(&path_for_validate, MAX_ROLE_ICON_BYTES)
                })
                .await;
            match validation {
                Ok(_) => {}
                Err(err) if err.contains("byte limit") => {
                    let message =
                        mezon_i18n::t(&locale, "clanEmojiSetting.toast.errorSizeLimit").to_string();
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    show_error(cx, message);
                    return;
                }
                Err(_) => {
                    let message = mezon_i18n::t(
                        &locale,
                        "clanEmojiSetting.modal.fileShouldBeAPNGPNGOrGIF256KBMax",
                    )
                    .to_string();
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    show_error(cx, message);
                    return;
                }
            }
            let upload = this
                .update(cx, |_, cx| {
                    ClanList::global(cx).update(cx, |store, cx| {
                        store.upload_clan_image(&path, MAX_ROLE_ICON_BYTES, cx)
                    })
                })
                .ok();
            let Some(task) = upload else {
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    cx.notify();
                });
                return;
            };
            match task.await {
                Ok(url) => {
                    let _ = this.update(cx, |this, cx| {
                        this.set_draft_icon(url, cx);
                        this.close_role_icon_picker(cx);
                        finish(this);
                        cx.notify();
                    });
                }
                Err(err) => {
                    tracing::error!("role icon upload failed: {err}");
                    let message =
                        mezon_i18n::t(&locale, "streamThumbnail.errors.uploadFailed").to_string();
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    show_error(cx, message);
                }
            }
        });
        self._icon_upload_task = Some(task);
    }
}

fn render_role_icon_tab_button(
    page: Entity<RoleSettingPage>,
    theme: &Theme,
    tab: RoleIconPickerTab,
    active: RoleIconPickerTab,
    label: SharedString,
    _window: &mut Window,
    _cx: &mut App,
) -> impl IntoElement {
    let selected = tab == active;
    div()
        .id(match tab {
            RoleIconPickerTab::Image => "role-icon-tab-upload",
            RoleIconPickerTab::Emoji => "role-icon-tab-emoji",
        })
        .px_5()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_primary)
        .when(selected, |el| el.bg(theme.tokens.bg_option_theme))
        .when(!selected, |el| {
            el.bg(theme.tokens.bg_item_theme_hover)
                .hover(|s| s.bg(theme.tokens.bg_option_theme))
        })
        .child(label)
        .on_click({
            let page = page.clone();
            move |_, window, cx| {
                page.update(cx, |this, cx| {
                    this.set_role_icon_picker_tab(tab, window, cx);
                });
            }
        })
}

fn render_role_icon_upload_panel(
    page: Entity<RoleSettingPage>,
    locale: &str,
    theme: &Theme,
    _cx: &mut App,
) -> impl IntoElement {
    v_flex()
        .gap_2()
        .items_center()
        .justify_center()
        .child(
            div()
                .id("role-icon-upload-target")
                .size(px(UPLOAD_TARGET))
                .rounded_full()
                .border_2()
                .border_dashed()
                .border_color(theme.border)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    Icon::new(IconName::ImageUploadIcon)
                        .size(px(70.0))
                        .text_color(theme.tokens.text_theme_primary),
                )
                .on_click({
                    let page = page.clone();
                    move |_, _, cx| {
                        page.update(cx, |this, cx| this.pick_role_icon(cx));
                    }
                }),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(mezon_i18n::t(locale, "common.roleIcon.chooseImageToUpload")),
        )
}
