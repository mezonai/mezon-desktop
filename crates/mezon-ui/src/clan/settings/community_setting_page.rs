use gpui::{
    App, ClipboardItem, Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription,
    Task, Window, div, img, prelude::*, px,
};
use mezon_store::{
    AppConfig, ClanId, ClanImageMimeType, ClanList, CommunityDraft, CommunityInfo,
    MAX_COMMUNITY_ABOUT_CHARS, MAX_COMMUNITY_BANNER_BYTES, MAX_COMMUNITY_DESCRIPTION_CHARS,
    MAX_COMMUNITY_SHORT_URL_CHARS, Settings, sanitize_community_short_url, truncate_chars,
};
use std::time::Duration;

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Sizable, Size, TextArea,
    TextAreaEvent, TextAreaField, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::assets::MEZON_COMMUNITY;
use crate::util::imgproxy;

const ABOUT_MAX_LEN: usize = MAX_COMMUNITY_ABOUT_CHARS;
const DESCRIPTION_MAX_LEN: usize = MAX_COMMUNITY_DESCRIPTION_CHARS;
const VANITY_URL_MAX_LEN: usize = MAX_COMMUNITY_SHORT_URL_CHARS;
const COMMUNITY_BANNER_PREVIEW_W: u32 = 700;
const COMMUNITY_BANNER_PREVIEW_H: u32 = 200;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FieldErrors {
    banner: bool,
    about: bool,
    description: bool,
    vanity_url: bool,
}

pub struct CommunitySettingPage {
    clan_id: ClanId,
    clan_list: Entity<ClanList>,
    settings: Entity<Settings>,
    loading: bool,
    is_community: bool,
    setup_mode: bool,
    draft: CommunityDraft,
    saved_draft: CommunityDraft,
    saving: bool,
    banner_uploading: bool,
    field_errors: FieldErrors,
    about_input: Option<Entity<TextArea>>,
    description_input: Option<Entity<TextArea>>,
    vanity_input: Option<Entity<InputState>>,
    _about_sub: Option<Subscription>,
    _description_sub: Option<Subscription>,
    _vanity_sub: Option<Subscription>,
    _fetch_task: Option<Task<()>>,
    _banner_upload_task: Option<Task<()>>,
    _copy_reset_task: Option<Task<()>>,
    url_copied: bool,
}

impl CommunitySettingPage {
    pub fn new(
        clan_id: ClanId,
        clan_list: Entity<ClanList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            clan_id,
            clan_list,
            settings,
            loading: true,
            is_community: false,
            setup_mode: false,
            draft: CommunityDraft::default(),
            saved_draft: CommunityDraft::default(),
            saving: false,
            banner_uploading: false,
            field_errors: FieldErrors::default(),
            about_input: None,
            description_input: None,
            vanity_input: None,
            _about_sub: None,
            _description_sub: None,
            _vanity_sub: None,
            _fetch_task: None,
            _banner_upload_task: None,
            _copy_reset_task: None,
            url_copied: false,
        };
        this.fetch_community_info(cx);
        this
    }

    pub fn release(&mut self, _cx: &mut Context<Self>) {
        self._fetch_task.take();
        self._banner_upload_task.take();
        self._copy_reset_task.take();
        self._about_sub.take();
        self._description_sub.take();
        self._vanity_sub.take();
        self.about_input = None;
        self.description_input = None;
        self.vanity_input = None;
        self.url_copied = false;
    }

    pub fn should_show_save_bar(&self, _cx: &App) -> bool {
        self.is_community && self.draft.has_changes(&self.saved_draft) && !self.saving
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    fn fetch_community_info(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let clan_id = self.clan_id;
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |this, cx| {
                    this.clan_list
                        .update(cx, |store, cx| store.fetch_community_info(clan_id, cx))
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let fetched = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match fetched {
                    Ok(info) => this.apply_info(info),
                    Err(err) => tracing::error!("fetch community info failed: {err}"),
                }
                cx.notify();
            });
        }));
    }

    fn apply_info(&mut self, info: CommunityInfo) {
        self.is_community = info.is_community;
        self.draft = CommunityDraft::from_info(&info);
        self.saved_draft = self.draft.clone();
        self.setup_mode = false;
        self.field_errors = FieldErrors::default();
    }

    fn reset_inputs(&mut self) {
        self.about_input = None;
        self.description_input = None;
        self.vanity_input = None;
        self._about_sub = None;
        self._description_sub = None;
        self._vanity_sub = None;
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme().clone();
        let input_bg = theme.tokens.bg_tertiary;

        if self.about_input.is_none() {
            let placeholder: SharedString = mezon_i18n::t(
                &locale,
                "onBoardingClan.communitySettings.about.placeholder",
            )
            .into();
            let initial = self.draft.about.clone();
            let about_input = cx.new(|cx| {
                TextArea::new(window, cx)
                    .placeholder(placeholder)
                    .min_height(px(96.0))
                    .max_visible_lines(6)
                    .bg(input_bg)
                    .text_color(theme.text_primary)
                    .radius(px(8.0))
            });
            about_input.update(cx, |state, cx| {
                state.set_value(initial, cx);
            });
            let sub = cx.subscribe(&about_input, |this, _, event: &TextAreaEvent, cx| {
                if *event == TextAreaEvent::Change {
                    this.sync_about_from_input(cx);
                }
            });
            self.about_input = Some(about_input);
            self._about_sub = Some(sub);
        }

        if self.description_input.is_none() {
            let placeholder: SharedString = mezon_i18n::t(
                &locale,
                "onBoardingClan.communitySettings.description.placeholder",
            )
            .into();
            let initial = self.draft.description.clone();
            let description_input = cx.new(|cx| {
                TextArea::new(window, cx)
                    .placeholder(placeholder)
                    .min_height(px(120.0))
                    .max_visible_lines(8)
                    .bg(input_bg)
                    .text_color(theme.text_primary)
                    .radius(px(8.0))
            });
            description_input.update(cx, |state, cx| {
                state.set_value(initial, cx);
            });
            let sub = cx.subscribe(&description_input, |this, _, event: &TextAreaEvent, cx| {
                if *event == TextAreaEvent::Change {
                    this.sync_description_from_input(cx);
                }
            });
            self.description_input = Some(description_input);
            self._description_sub = Some(sub);
        }

        if self.vanity_input.is_none() {
            let placeholder: SharedString = mezon_i18n::t(
                &locale,
                "onBoardingClan.communitySettings.vanityUrl.placeholder",
            )
            .into();
            let initial = self.draft.short_url.clone();
            let vanity_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .height(px(40.0))
                    .text_size(px(14.0))
                    .borderless()
                    .bg(input_bg)
            });
            vanity_input.update(cx, |state, cx| {
                state.set_value(&initial, window, cx);
            });
            let sub = cx.subscribe_in(&vanity_input, window, |this, input, evt, window, cx| {
                if *evt == InputEvent::Change {
                    let raw = input.read(cx).value().to_string();
                    let value = sanitize_community_short_url(&raw);
                    if value != raw {
                        input.update(cx, |state, cx| {
                            state.set_value(&value, window, cx);
                        });
                    }
                    this.draft.short_url = value;
                    this.field_errors.vanity_url = false;
                    this.url_copied = false;
                    this._copy_reset_task.take();
                    cx.notify();
                }
            });
            self.vanity_input = Some(vanity_input);
            self._vanity_sub = Some(sub);
        }
    }

    fn sync_about_from_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.about_input.clone() else {
            return;
        };
        let raw = input.read(cx).value().to_string();
        let value = truncate_chars(&raw, ABOUT_MAX_LEN);
        if value != raw {
            input.update(cx, |state, cx| state.set_value(value.clone(), cx));
        }
        self.draft.about = value;
        self.field_errors.about = false;
        cx.notify();
    }

    fn sync_description_from_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.description_input.clone() else {
            return;
        };
        let raw = input.read(cx).value().to_string();
        let value = truncate_chars(&raw, DESCRIPTION_MAX_LEN);
        if value != raw {
            input.update(cx, |state, cx| state.set_value(value.clone(), cx));
        }
        self.draft.description = value;
        self.field_errors.description = false;
        cx.notify();
    }

    fn start_setup(&mut self, cx: &mut Context<Self>) {
        if self.draft == CommunityDraft::default()
            && let Some(clan) = self
                .clan_list
                .read(cx)
                .clans
                .iter()
                .find(|c| c.id == self.clan_id)
        {
            let info = CommunityInfo::from(clan);
            let restored = CommunityDraft::from_info(&info);
            if restored != CommunityDraft::default() {
                self.draft = restored.clone();
                self.saved_draft = restored;
            }
        }
        self.setup_mode = true;
        self.field_errors = FieldErrors::default();
        self.reset_inputs();
        cx.notify();
    }

    fn cancel_setup(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.setup_mode = false;
        self.draft = self.saved_draft.clone();
        self.field_errors = FieldErrors::default();
        self.reset_inputs();
        cx.notify();
    }

    fn reset(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.draft = self.saved_draft.clone();
        self.field_errors = FieldErrors::default();
        self.reset_inputs();
        cx.notify();
    }

    fn validate(&mut self) -> bool {
        let draft = self.draft.clone().sanitized();
        let mut errors = FieldErrors::default();
        if draft.community_banner.is_empty() {
            errors.banner = true;
        }
        if draft.about.is_empty() {
            errors.about = true;
        }
        if draft.description.is_empty() {
            errors.description = true;
        }
        if draft.short_url.is_empty() {
            errors.vanity_url = true;
        }
        self.field_errors = errors;
        !self.field_errors.banner
            && !self.field_errors.about
            && !self.field_errors.description
            && !self.field_errors.vanity_url
    }

    fn save(&mut self, enable: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        if !self.validate() {
            let locale = self.settings.read(cx).language.clone();
            let message = mezon_i18n::t(
                &locale,
                "onBoardingClan.communitySettings.messages.fillAllRequiredFields",
            )
            .to_string();
            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
            cx.notify();
            return;
        }

        self.saving = true;
        cx.notify();

        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        let draft = self.draft.clone().sanitized();
        let save_task = self.clan_list.update(cx, |store, cx| {
            store.save_community_fields(clan_id, draft.clone(), cx)
        });

        cx.spawn(async move |this, cx| match save_task.await {
            Ok(()) => {
                let success_key = if enable {
                    "onBoardingClan.communitySettings.messages.communityEnabledAndSaved"
                } else {
                    "onBoardingClan.communitySettings.messages.changesSaved"
                };
                let success = mezon_i18n::t(&locale, success_key).to_string();
                let _ = this.update(cx, |this, cx| {
                    this.is_community = true;
                    this.setup_mode = false;
                    this.draft = draft.clone();
                    this.saved_draft = draft;
                    this.saving = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.success(success, cx));
                });
            }
            Err(err) => {
                let message_key = if enable {
                    "onBoardingClan.errors.failedToEnableCommunity"
                } else {
                    "onBoardingClan.communitySettings.messages.saveFailed"
                };
                let message = mezon_i18n::t(&locale, message_key).to_string();
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
                tracing::error!("save community failed: {err}");
            }
        })
        .detach();
    }

    pub(crate) fn disable_community(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        cx.notify();

        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        let task = self
            .clan_list
            .update(cx, |store, cx| store.disable_community(clan_id, cx));

        cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                let success = mezon_i18n::t(
                    &locale,
                    "onBoardingClan.communitySettings.messages.communityDisabled",
                )
                .to_string();
                let _ = this.update(cx, |this, cx| {
                    this.is_community = false;
                    this.setup_mode = false;
                    this.field_errors = FieldErrors::default();
                    this.reset_inputs();
                    this.saving = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.info(success, cx));
                });
            }
            Err(err) => {
                let message = mezon_i18n::t(
                    &locale,
                    "onBoardingClan.communitySettings.messages.disableFailed",
                )
                .to_string();
                let _ = this.update(cx, |this, cx| {
                    this.saving = false;
                    cx.notify();
                });
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
                tracing::error!("disable community failed: {err}");
            }
        })
        .detach();
    }

    fn request_disable_community(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let page = cx.weak_entity();
        Shell::global(cx).update(cx, |shell, cx| {
            shell.confirm_disable_clan_community(
                move |cx| {
                    let _ = page.update(cx, |page, cx| {
                        page.disable_community(cx);
                    });
                },
                &locale,
                window,
                cx,
            );
        });
    }

    fn pick_banner(&mut self, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let prompt: SharedString = mezon_i18n::t(
            &locale,
            "onBoardingClan.communitySettings.banner.uploadTitle",
        )
        .into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        let task = cx.spawn(async move |this, cx| {
            let finish = |this: &mut CommunitySettingPage| {
                this.banner_uploading = false;
                this._banner_upload_task = None;
            };
            let show_error = |cx: &mut gpui::AsyncApp, message: String| {
                cx.update(|cx| {
                    Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                });
            };

            let Some(paths) = crate::util::file_dialog::resolve(rx, cx).await else {
                let _ = this.update(cx, |this, cx| {
                    this._banner_upload_task = None;
                    cx.notify();
                });
                return;
            };
            let path = match paths.into_iter().next() {
                Some(path) => path,
                None => {
                    let _ = this.update(cx, |this, cx| {
                        this._banner_upload_task = None;
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.banner_uploading = true;
                cx.notify();
            });
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !ClanImageMimeType::is_allowed_extension(&ext) {
                let message =
                    mezon_i18n::t(&locale, "clanSettings.clanBanner.modal.content").to_string();
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    cx.notify();
                });
                show_error(cx, message);
                return;
            }
            let path_buf = path.clone();
            let file_size = match cx
                .background_spawn(async move { std::fs::metadata(&path_buf).ok().map(|m| m.len()) })
                .await
            {
                Some(size) => size,
                None => {
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    return;
                }
            };
            if file_size > MAX_COMMUNITY_BANNER_BYTES {
                let message =
                    mezon_i18n::t(&locale, "clanSoundSetting.toast.errorSizeLimit").to_string();
                let _ = this.update(cx, |this, cx| {
                    finish(this);
                    cx.notify();
                });
                show_error(cx, message);
                return;
            }
            let upload = this
                .update(cx, |this, cx| {
                    this.clan_list.update(cx, |store, cx| {
                        store.upload_clan_image(&path, MAX_COMMUNITY_BANNER_BYTES, cx)
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
                        this.draft.community_banner = url;
                        this.field_errors.banner = false;
                        finish(this);
                        cx.notify();
                    });
                }
                Err(err) => {
                    tracing::error!("community banner upload failed: {err}");
                    let message = mezon_i18n::t(
                        &locale,
                        "onBoardingClan.communitySettings.messages.bannerUpdateFailed",
                    )
                    .to_string();
                    let _ = this.update(cx, |this, cx| {
                        finish(this);
                        cx.notify();
                    });
                    show_error(cx, message);
                }
            }
        });
        self._banner_upload_task = Some(task);
    }

    fn remove_banner(&mut self, cx: &mut Context<Self>) {
        self.draft.community_banner.clear();
        self.field_errors.banner = false;
        cx.notify();
    }

    fn preview_url(&self, cx: &App) -> String {
        AppConfig::global(cx).community_clan_url(&self.draft.short_url)
    }

    fn vanity_url_prefix(&self, cx: &App) -> String {
        AppConfig::global(cx).community_clan_url_prefix()
    }

    fn copy_preview_url(&mut self, cx: &mut Context<Self>) {
        if self.draft.short_url.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.preview_url(cx)));
        self.url_copied = true;
        cx.notify();
        let executor = cx.background_executor().clone();
        self._copy_reset_task = Some(cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                this.url_copied = false;
                this._copy_reset_task = None;
                cx.notify();
            });
        }));
    }

    fn render_loading(locale: &str, theme: &Theme) -> impl IntoElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .min_h(px(320.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(mezon_i18n::t(
                        locale,
                        "onBoardingClan.communitySettings.loading",
                    )),
            )
    }

    fn render_enable_landing(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex().w_full().gap_6().child(
            div()
                .w_full()
                .p_6()
                .rounded_lg()
                .bg(theme.surfaces.secondary)
                .border_1()
                .border_color(theme.border)
                .child(
                    v_flex()
                        .items_center()
                        .gap_5()
                        .child(
                            div()
                                .p_4()
                                .rounded_lg()
                                .bg(theme.tokens.bg_tertiary)
                                .child(img(MEZON_COMMUNITY).max_w(px(320.0)).rounded_md()),
                        )
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.enableCommunity.title",
                                )),
                        )
                        .child(
                            div()
                                .max_w(px(520.0))
                                .text_center()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.enableCommunity.description",
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.enableCommunity.subtitle",
                                )),
                        )
                        .child(
                            Button::new("community-enable-start")
                                .label(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.enableCommunity.button",
                                ))
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_setup(cx);
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.enableCommunity.note",
                                )),
                        ),
                ),
        )
    }

    fn render_field_label(text: SharedString, required: bool, theme: &Theme) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(text),
            )
            .when(required, |el| {
                el.child(div().text_sm().text_color(theme.status_dnd).child("*"))
            })
    }

    fn render_char_counter(current: usize, max: usize, theme: &Theme) -> impl IntoElement {
        div()
            .text_xs()
            .text_color(if current > max * 8 / 10 {
                theme.status_idle
            } else {
                theme.text_muted
            })
            .child(format!("{current}/{max}"))
    }

    fn render_banner_preview(
        &self,
        locale: &str,
        theme: &Theme,
        banner_error: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let banner = self.draft.community_banner.clone();
        let has_banner = !banner.is_empty();
        let banner_src = if has_banner {
            imgproxy::proxied(
                cx,
                &banner,
                COMMUNITY_BANNER_PREVIEW_W,
                COMMUNITY_BANNER_PREVIEW_H,
                "force",
            )
        } else {
            String::new()
        };
        let uploading = self.banner_uploading;
        let radius = px(8.0);

        div()
            .relative()
            .w_full()
            .h(px(200.0))
            .rounded(radius)
            .border_1()
            .border_color(if banner_error {
                theme.status_dnd
            } else {
                theme.border
            })
            .bg(theme.tokens.theme_setting_nav)
            .overflow_hidden()
            .when(has_banner, |el| {
                el.child(
                    img(banner_src)
                        .absolute()
                        .inset_0()
                        .w_full()
                        .h_full()
                        .rounded(radius)
                        .object_fit(gpui::ObjectFit::Cover),
                )
            })
            .when(!has_banner, |el| {
                el.flex().items_center().justify_center().child(
                    Icon::new(IconName::UploadImage)
                        .size(px(32.0))
                        .text_color(theme.text_secondary),
                )
            })
            .child(
                div()
                    .id("community-banner-action")
                    .absolute()
                    .top(px(16.0))
                    .right(px(16.0))
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_floating)
                    .cursor_pointer()
                    .occlude()
                    .when(has_banner, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.remove_banner(cx)))
                    })
                    .when(!has_banner, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            if !this.banner_uploading {
                                this.pick_banner(cx);
                            }
                        }))
                    })
                    .child(
                        Icon::new(if has_banner {
                            IconName::Close
                        } else {
                            IconName::SelectFileIcon
                        })
                        .size(px(16.0))
                        .text_color(if has_banner {
                            theme.status_dnd
                        } else {
                            theme.text_secondary
                        }),
                    ),
            )
            .when(uploading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(radius)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::hsla(0., 0., 0., 0.45))
                        .child(
                            div()
                                .text_sm()
                                .text_color(gpui::white())
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.buttons.saving",
                                )),
                        ),
                )
            })
    }

    fn render_banner_section(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let banner_error = self.field_errors.banner;

        h_flex()
            .gap(px(20.0))
            .items_start()
            .w_full()
            .min_w(px(0.0))
            .child(
                div().flex_1().min_w(px(0.0)).overflow_hidden().child(
                    v_flex()
                        .w_full()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .mb(px(8.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text_primary)
                                        .child(mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.banner.title",
                                        )),
                                )
                                .child(div().text_xs().text_color(theme.status_dnd).child("*")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(theme.text_secondary)
                                .mb(px(8.0))
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.banner.uploadDescription",
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "clanSettings.clanBanner.recommendedSize",
                                )),
                        )
                        .child(
                            div().mt(px(16.0)).child(
                                Button::new("community-banner-upload")
                                    .label(mezon_i18n::t(
                                        locale,
                                        "onBoardingClan.communitySettings.banner.uploadTitle",
                                    ))
                                    .primary()
                                    .with_size(Size::Large)
                                    .disabled(self.banner_uploading)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pick_banner(cx);
                                    })),
                            ),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.render_banner_preview(locale, theme, banner_error, cx)),
            )
    }

    fn render_form(
        &self,
        locale: &str,
        theme: &Theme,
        setup_mode: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let about_len = self.draft.about.chars().count();
        let description_len = self.draft.description.chars().count();
        let vanity_len = self.draft.short_url.chars().count();
        let show_preview = !self.draft.short_url.is_empty();
        let saving = self.saving;
        let vanity_prefix = self.vanity_url_prefix(cx);
        let preview_url = self.preview_url(cx);

        v_flex()
            .w_full()
            .min_w(px(0.0))
            .gap_8()
            .when(self.is_community && !setup_mode, |el| {
                el.child(
                    div()
                        .text_base()
                        .text_color(theme.text_secondary)
                        .child(mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.subtitle",
                        )),
                )
            })
            .when(setup_mode, |el| {
                el.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_primary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.enableTitle",
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.enableSubtitle",
                                )),
                        ),
                )
            })
            .child(self.render_banner_section(locale, theme, cx))
            .child(
                v_flex()
                    .gap_2()
                    .child(Self::render_field_label(
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.description.title",
                        )
                        .into(),
                        true,
                        theme,
                    ))
                    .child(
                        div()
                            .w_full()
                            .when(self.field_errors.description, |el| {
                                el.border_2().border_color(theme.status_dnd).rounded_lg()
                            })
                            .when_some(self.description_input.as_ref(), |el, input| {
                                el.child(TextAreaField::new(input))
                            }),
                    )
                    .child(h_flex().justify_end().child(Self::render_char_counter(
                        description_len,
                        DESCRIPTION_MAX_LEN,
                        theme,
                    ))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Self::render_field_label(
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.about.title",
                        )
                        .into(),
                        true,
                        theme,
                    ))
                    .child(
                        div()
                            .w_full()
                            .when(self.field_errors.about, |el| {
                                el.border_2().border_color(theme.status_dnd).rounded_lg()
                            })
                            .when_some(self.about_input.as_ref(), |el, input| {
                                el.child(TextAreaField::new(input))
                            }),
                    )
                    .child(h_flex().justify_end().child(Self::render_char_counter(
                        about_len,
                        ABOUT_MAX_LEN,
                        theme,
                    ))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Self::render_field_label(
                        mezon_i18n::t(
                            locale,
                            "onBoardingClan.communitySettings.vanityUrl.title",
                        )
                        .into(),
                        true,
                        theme,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_secondary)
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.communitySettings.vanityUrl.description",
                            )),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .rounded_lg()
                            .border_1()
                            .when(self.field_errors.vanity_url, |el| {
                                el.border_color(theme.status_dnd)
                            })
                            .when(!self.field_errors.vanity_url, |el| {
                                el.border_color(theme.border)
                            })
                            .overflow_hidden()
                            .bg(theme.tokens.bg_tertiary)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px_3()
                                    .h(px(40.0))
                                    .flex()
                                    .items_center()
                                    .border_r_1()
                                    .border_color(theme.border)
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(vanity_prefix),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .when_some(self.vanity_input.as_ref(), |el, input| {
                                        el.child(Input::new(input))
                                    }),
                            ),
                    )
                    .child(h_flex().justify_end().child(Self::render_char_counter(
                        vanity_len,
                        VANITY_URL_MAX_LEN,
                        theme,
                    )))
                    .when(show_preview, |el| {
                        let copied = self.url_copied;
                        el.child(
                            div()
                                .p_3()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surfaces.secondary)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(theme.text_secondary)
                                                        .child(mezon_i18n::t(
                                                            locale,
                                                            "onBoardingClan.communitySettings.vanityUrl.previewLabel",
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(theme.text_primary)
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .child(preview_url),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("community-preview-url-copy")
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .size(px(28.0))
                                                .rounded_md()
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.bg_hover))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.copy_preview_url(cx);
                                                }))
                                                .child(
                                                    Icon::new(if copied {
                                                        IconName::Check
                                                    } else {
                                                        IconName::CopyIcon
                                                    })
                                                    .size(px(16.0))
                                                    .text_color(if copied {
                                                        theme.status_online
                                                    } else {
                                                        theme.text_secondary
                                                    }),
                                                ),
                                        ),
                                ),
                        )
                    }),
            )
            .when(setup_mode, |el| {
                el.child(
                    h_flex()
                        .justify_end()
                        .gap_3()
                        .pt_4()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            Button::new("community-setup-cancel")
                                .label(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.buttons.cancel",
                                ))
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_setup(window, cx);
                                })),
                        )
                        .child(
                            Button::new("community-setup-save")
                                .label(mezon_i18n::t(
                                    locale,
                                    "onBoardingClan.communitySettings.buttons.enableAndSave",
                                ))
                                .primary()
                                .disabled(saving)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save(true, cx);
                                })),
                        ),
                )
            })
            .when(self.is_community && !setup_mode, |el| {
                el.child(
                    v_flex()
                        .gap_3()
                        .pt_4()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.status_dnd)
                                .child(
                                    mezon_i18n::t(
                                        locale,
                                        "onBoardingClan.communitySettings.dangerZone",
                                    )
                                    .to_uppercase(),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_4()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.text_secondary)
                                        .child(mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.buttons.disable",
                                        )),
                                )
                                .child(
                                    Button::new("community-disable")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.buttons.disable",
                                        ))
                                        .danger()
                                        .disabled(saving || self.draft.has_changes(&self.saved_draft))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.request_disable_community(window, cx);
                                        })),
                                ),
                        ),
                )
            })
    }
}

impl Render for CommunitySettingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let theme = cx.theme().clone();

        if self.loading {
            return Self::render_loading(&locale, &theme).into_any_element();
        }

        if !self.is_community && !self.setup_mode {
            return self
                .render_enable_landing(&locale, &theme, cx)
                .into_any_element();
        }

        self.ensure_inputs(window, cx);
        self.render_form(&locale, &theme, self.setup_mode, cx)
            .into_any_element()
    }
}

pub fn render_community_save_bar(
    page: Entity<CommunitySettingPage>,
    locale: &str,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let saving = page.read(cx).is_saving();
    div()
        .absolute()
        .bottom(px(20.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .occlude()
        .child(
            div()
                .w(px(700.0))
                .max_w(gpui::relative(0.9))
                .py(px(10.0))
                .pl_4()
                .pr(px(10.0))
                .rounded(px(5.0))
                .bg(theme.bg_floating)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(mezon_i18n::t(
                                    locale,
                                    "clanSettings.modalSaveChanges.title",
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_4()
                                .items_center()
                                .child(
                                    Button::new("community-reset")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.buttons.reset",
                                        ))
                                        .ghost()
                                        .on_click({
                                            let page = page.clone();
                                            move |_, window, cx| {
                                                page.update(cx, |page, cx| {
                                                    page.reset(window, cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new("community-save")
                                        .label(mezon_i18n::t(
                                            locale,
                                            "onBoardingClan.communitySettings.buttons.save",
                                        ))
                                        .primary()
                                        .disabled(saving)
                                        .on_click({
                                            let page = page.clone();
                                            move |_, _, cx| {
                                                page.update(cx, |page, cx| {
                                                    page.save(false, cx);
                                                });
                                            }
                                        }),
                                ),
                        ),
                ),
        )
}
