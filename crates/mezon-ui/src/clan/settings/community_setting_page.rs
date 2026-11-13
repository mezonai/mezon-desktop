use gpui::{
    AnyElement, App, Context, Entity, Focusable, FontWeight, PathPromptOptions, SharedString,
    Subscription, Task, Window, div, img, prelude::*, px, relative, rgb,
};
use mezon_store::{
    AppConfig, ClanId, ClanImageMimeType, ClanList, CommunityDraft, CommunityFieldIssues,
    CommunityInfo, MAX_COMMUNITY_ABOUT_CHARS, MAX_COMMUNITY_BANNER_BYTES,
    MAX_COMMUNITY_DESCRIPTION_CHARS, MAX_COMMUNITY_SHORT_URL_CHARS, Settings,
    sanitize_community_short_url, truncate_chars,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, InputEvent, InputState, TextArea, TextAreaEvent, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};
use crate::util::assets::MEZON_COMMUNITY;

mod community_enable_modal;
mod community_overview_card;

const ABOUT_MAX_LEN: usize = MAX_COMMUNITY_ABOUT_CHARS;
const DESCRIPTION_MAX_LEN: usize = MAX_COMMUNITY_DESCRIPTION_CHARS;
const VANITY_URL_MAX_LEN: usize = MAX_COMMUNITY_SHORT_URL_CHARS;
const COMMUNITY_BANNER_PREVIEW_W: u32 = 1280;
const COMMUNITY_BANNER_PREVIEW_H: u32 = 720;
const ENABLE_CARD_BG: u32 = 0x4752c4;
const ENABLE_CARD_BUTTON_TEXT: u32 = 0x5865f2;
const ENABLE_CARD_BUTTON_HOVER_BG: u32 = 0xf9fafb;
const ENABLE_CARD_MIN_HEIGHT: f32 = 500.0;
const ENABLE_CARD_IMAGE_WIDTH: f32 = 350.0;
const TEXTAREA_MIN_HEIGHT: f32 = 120.0;
const TEXTAREA_MAX_VISIBLE_LINES: usize = 8;

fn without_url_scheme(url: &str) -> &str {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
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
    field_errors: CommunityFieldIssues,
    about_input: Option<Entity<TextArea>>,
    description_input: Option<Entity<TextArea>>,
    vanity_input: Option<Entity<InputState>>,
    _about_sub: Option<Subscription>,
    _description_sub: Option<Subscription>,
    _vanity_sub: Option<Subscription>,
    _fetch_task: Option<Task<()>>,
    _banner_upload_task: Option<Task<()>>,
    banner_hovered: Option<bool>,
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
            field_errors: CommunityFieldIssues::default(),
            about_input: None,
            description_input: None,
            vanity_input: None,
            _about_sub: None,
            _description_sub: None,
            _vanity_sub: None,
            _fetch_task: None,
            _banner_upload_task: None,
            banner_hovered: None,
        };
        this.fetch_community_info(cx);
        this
    }

    pub fn release(&mut self, _cx: &mut Context<Self>) {
        self._fetch_task.take();
        self._banner_upload_task.take();
        self._about_sub.take();
        self._description_sub.take();
        self._vanity_sub.take();
        self.about_input = None;
        self.description_input = None;
        self.vanity_input = None;
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
        self.field_errors = CommunityFieldIssues::default();
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
                    .min_height(px(TEXTAREA_MIN_HEIGHT))
                    .max_visible_lines(TEXTAREA_MAX_VISIBLE_LINES)
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
                    .min_height(px(TEXTAREA_MIN_HEIGHT))
                    .max_visible_lines(TEXTAREA_MAX_VISIBLE_LINES)
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
        self.draft.ensure_default_hashtag();
        self.setup_mode = true;
        self.field_errors = CommunityFieldIssues::default();
        self.reset_inputs();
        cx.notify();
    }

    fn cancel_setup(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.setup_mode = false;
        self.draft = self.saved_draft.clone();
        self.field_errors = CommunityFieldIssues::default();
        self.reset_inputs();
        cx.notify();
    }

    fn reset(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.draft = self.saved_draft.clone();
        self.field_errors = CommunityFieldIssues::default();
        self.reset_inputs();
        cx.notify();
    }

    fn validate(&mut self) -> bool {
        self.field_errors = self.draft.missing_fields();
        self.field_errors.is_empty()
    }

    pub fn fills_content_height(&self) -> bool {
        !self.loading && self.is_community
    }

    pub fn is_setup_open(&self) -> bool {
        self.setup_mode && !self.is_community
    }

    fn toggle_hashtag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if self.draft.toggle_hashtag(tag) {
            self.field_errors.hashtags = false;
            cx.notify();
        }
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
                    this.field_errors = CommunityFieldIssues::default();
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
        self.banner_hovered = None;
        self.field_errors.banner = false;
        cx.notify();
    }

    fn preview_url(&self, cx: &App) -> String {
        without_url_scheme(&AppConfig::global(cx).community_clan_url(&self.draft.short_url))
            .to_string()
    }

    fn vanity_url_prefix(&self, cx: &App) -> String {
        without_url_scheme(&AppConfig::global(cx).community_clan_url_prefix()).to_string()
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

    fn render_enable_card(page: Entity<Self>, locale: &str) -> AnyElement {
        let page = page.downgrade();
        v_flex()
            .relative()
            .overflow_hidden()
            .items_center()
            .justify_center()
            .min_h(px(ENABLE_CARD_MIN_HEIGHT))
            .p_8()
            .rounded(px(16.0))
            .bg(rgb(ENABLE_CARD_BG))
            .text_color(gpui::white())
            .shadow_lg()
            .child(
                div()
                    .absolute()
                    .top(px(-64.0))
                    .left(px(-64.0))
                    .size(px(128.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.05)),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(-96.0))
                    .right(px(-96.0))
                    .size(px(192.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.05)),
            )
            .child(
                div()
                    .absolute()
                    .top(relative(0.5))
                    .left(px(-48.0))
                    .size(px(96.0))
                    .rounded_full()
                    .bg(gpui::white().alpha(0.1)),
            )
            .child(
                v_flex()
                    .relative()
                    .items_center()
                    .w_full()
                    .child(
                        div()
                            .mb_8()
                            .p_4()
                            .rounded(px(16.0))
                            .bg(gpui::white().alpha(0.1))
                            .shadow_lg()
                            .child(
                                img(MEZON_COMMUNITY)
                                    .w(px(ENABLE_CARD_IMAGE_WIDTH))
                                    .max_w_full()
                                    .rounded(px(8.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(36.0))
                            .font_weight(FontWeight::BOLD)
                            .text_center()
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableCommunity.title",
                            )),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .h(px(4.0))
                            .mt_2()
                            .mb_6()
                            .rounded_full()
                            .bg(gpui::white().alpha(0.3)),
                    )
                    .child(
                        div()
                            .px_4()
                            .text_lg()
                            .text_center()
                            .text_color(gpui::white().alpha(0.9))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableCommunity.description",
                            )),
                    )
                    .child(
                        div()
                            .mt_2()
                            .px_4()
                            .text_base()
                            .text_center()
                            .text_color(gpui::white().alpha(0.8))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableCommunity.subtitle",
                            )),
                    )
                    .child(
                        div()
                            .id("community-enable-start")
                            .mt_8()
                            .px(px(40.0))
                            .py(px(16.0))
                            .rounded(px(12.0))
                            .bg(gpui::white())
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(ENABLE_CARD_BUTTON_TEXT))
                            .shadow_lg()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(ENABLE_CARD_BUTTON_HOVER_BG)))
                            .child(mezon_i18n::t(
                                locale,
                                "onBoardingClan.enableCommunity.button",
                            ))
                            .on_click(move |_, _, cx| {
                                let _ = page.update(cx, |this, cx| this.start_setup(cx));
                            }),
                    )
                    .child(
                        div()
                            .mt_6()
                            .text_sm()
                            .text_center()
                            .text_color(gpui::white().alpha(0.6))
                            .child(mezon_i18n::t(locale, "onBoardingClan.enableCommunity.note")),
                    ),
            )
            .into_any_element()
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

    fn input_focus_handles(&self, cx: &App) -> Vec<gpui::FocusHandle> {
        self.description_input
            .iter()
            .chain(&self.about_input)
            .map(|input| input.focus_handle(cx))
            .chain(self.vanity_input.iter().map(|input| input.focus_handle(cx)))
            .collect()
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
            return Self::render_enable_card(cx.entity(), &locale);
        }

        self.ensure_inputs(window, cx);
        if self.is_setup_open() {
            return self.render_enable_modal(&locale, &theme, cx);
        }
        self.render_overview_card(&locale, &theme, cx)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{TestAppContext, VisualTestContext};
    use mezon_client::transport::ApiClanDesc;
    use mezon_store::{Clan, DEFAULT_COMMUNITY_HASHTAG};

    use super::*;

    fn open_page(
        desc: ApiClanDesc,
        cx: &mut TestAppContext,
    ) -> (Entity<CommunitySettingPage>, &mut VisualTestContext) {
        let clan_id = ClanId(desc.clan_id);
        let (clan_list, settings) = cx.update(|cx| {
            mezon_theme::set_theme(mezon_theme::resolve_theme("dark"), cx);
            AppConfig::init_global(Arc::new(AppConfig::default()), cx);
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            mezon_store::RealtimeDispatch::init(api.clone(), cx);
            let clan_list = ClanList::init(api, cx);
            clan_list.update(cx, |list, cx| list.update_clans(vec![Clan::from(desc)], cx));
            (clan_list, cx.new(|_| Settings::default()))
        });
        let (page, cx) =
            cx.add_window_view(|_, cx| CommunitySettingPage::new(clan_id, clan_list, settings, cx));
        cx.run_until_parked();
        (page, cx)
    }

    #[gpui::test]
    fn enabling_opens_the_modal_with_the_default_hashtag_and_cancel_restores(
        cx: &mut TestAppContext,
    ) {
        let (page, cx) = open_page(
            ApiClanDesc {
                clan_id: 1,
                ..Default::default()
            },
            cx,
        );
        page.read_with(cx, |page, _| {
            assert!(!page.loading);
            assert!(!page.is_setup_open());
            assert!(page.draft.hashtags.is_empty());
        });

        page.update(cx, |page, cx| page.start_setup(cx));
        cx.run_until_parked();
        page.read_with(cx, |page, cx| {
            assert!(page.is_setup_open());
            assert!(!page.should_show_save_bar(cx));
            assert_eq!(page.draft.hashtags, vec![DEFAULT_COMMUNITY_HASHTAG]);
            assert!(page.description_input.is_some());
            assert!(page.about_input.is_some());
            assert!(page.vanity_input.is_some());
        });

        page.update(cx, |page, cx| {
            assert!(!page.validate());
            assert!(page.field_errors.banner && page.field_errors.about);
            assert!(page.field_errors.description && page.field_errors.vanity_url);
            assert!(!page.field_errors.hashtags);

            page.toggle_hashtag(DEFAULT_COMMUNITY_HASHTAG, cx);
            assert_eq!(page.draft.hashtags, vec![DEFAULT_COMMUNITY_HASHTAG]);
            page.toggle_hashtag("Gaming", cx);
            page.toggle_hashtag(DEFAULT_COMMUNITY_HASHTAG, cx);
            assert_eq!(page.draft.hashtags, vec!["Gaming"]);
        });

        cx.update(|window, cx| page.update(cx, |page, cx| page.cancel_setup(window, cx)));
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(!page.is_setup_open());
            assert!(page.draft.hashtags.is_empty());
            assert_eq!(page.field_errors, CommunityFieldIssues::default());
        });
    }

    #[gpui::test]
    fn enabled_community_loads_its_hashtags_and_flags_unsaved_hashtag_edits(
        cx: &mut TestAppContext,
    ) {
        let (page, cx) = open_page(
            ApiClanDesc {
                clan_id: 2,
                is_community: true,
                community_banner: "https://cdn.example/b.png".into(),
                about: "about".into(),
                description: "desc".into(),
                short_url: "vanity".into(),
                hashtags: "Music".into(),
                ..Default::default()
            },
            cx,
        );
        page.read_with(cx, |page, cx| {
            assert!(page.is_community);
            assert!(!page.is_setup_open());
            assert_eq!(page.draft.hashtags, vec!["Music"]);
            assert!(!page.should_show_save_bar(cx));
        });

        page.update(cx, |page, cx| page.toggle_hashtag("Gaming", cx));
        cx.run_until_parked();
        page.read_with(cx, |page, cx| {
            assert_eq!(page.draft.hashtags, vec!["Music", "Gaming"]);
            assert!(page.should_show_save_bar(cx));
        });

        cx.update(|window, cx| page.update(cx, |page, cx| page.reset(window, cx)));
        page.read_with(cx, |page, cx| {
            assert_eq!(page.draft.hashtags, vec!["Music"]);
            assert!(!page.should_show_save_bar(cx));
        });
    }
}
