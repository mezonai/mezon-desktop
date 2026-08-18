use gpui::{
    App, AsyncApp, Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription,
    Task, Window, deferred, div, img, prelude::*, px,
};
use mezon_store::{
    AppConfig, ChannelId, ChannelList, ChannelType, ClanId, ClanImageMimeType, ClanList,
    ClanOverviewDraft, ClanSystemMessage, MAX_CLAN_BANNER_BYTES, MAX_CLAN_LOGO_BYTES, Settings,
};

use crate::app::shell::Shell;
use crate::components::primitives::{
    Button, ButtonVariants, Icon, IconName, Input, InputEvent, InputState, Sizable, Size, Switch,
    UnsavedChangesBar, h_flex, v_flex,
};
use crate::theme::{ActiveTheme, Theme};

fn section_heading_xs(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    let text = text.into().to_string().to_uppercase();
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_primary)
        .mb(px(8.0))
        .child(text)
}

fn section_heading_sm(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    let text = text.into().to_string().to_uppercase();
    div()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text_primary)
        .mb(px(8.0))
        .child(text)
}

fn body_text(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_sm()
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.text_secondary)
        .child(text.into())
}

fn logo_helper_text(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .text_sm()
        .whitespace_normal()
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.text_secondary)
        .child(text.into())
}

fn flex_column() -> gpui::Div {
    div().flex_1().min_w(px(0.0))
}

fn section_divider(el: gpui::Div, theme: &Theme) -> gpui::Div {
    el.mt(px(40.0))
        .pt(px(40.0))
        .border_t_1()
        .border_color(theme.border)
}

fn half_column() -> gpui::Div {
    div()
        .w(gpui::relative(0.5))
        .flex_shrink_0()
        .min_w(px(0.0))
        .overflow_hidden()
}

fn is_valid_clan_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == ' '
}

fn is_valid_clan_name(s: &str) -> bool {
    if s.is_empty() || s.chars().count() > 64 {
        return false;
    }
    let first = s.chars().next().unwrap();
    if first == '_' || first == '-' || first == ' ' {
        return false;
    }
    s.chars().all(|c| is_valid_clan_name_char(c) && c != '\'')
}

#[derive(Clone, PartialEq)]
struct ChannelOption {
    id: ChannelId,
    channel_label: SharedString,
    category_name: SharedString,
}

pub struct OverviewSettingPage {
    clan_id: ClanId,
    clan_list: Entity<ClanList>,
    channel_list: Entity<ChannelList>,
    settings: Entity<Settings>,
    draft: ClanOverviewDraft,
    saved_draft: ClanOverviewDraft,
    system_message: Option<ClanSystemMessage>,
    saved_system_message: Option<ClanSystemMessage>,
    channel_options: Vec<ChannelOption>,
    selected_channel_index: Option<usize>,
    channel_menu_open: bool,
    name_input: Option<Entity<InputState>>,
    name_valid: bool,
    saving: bool,
    logo_uploading: bool,
    banner_uploading: bool,
    system_loading: bool,
    _name_sub: Option<Subscription>,
    _channel_sub: Subscription,
    _fetch_task: Option<Task<()>>,
    _logo_upload_task: Option<Task<()>>,
    _banner_upload_task: Option<Task<()>>,
}

#[derive(Clone, Copy)]
enum ImageUploadKind {
    Logo,
    Banner,
}

impl OverviewSettingPage {
    pub fn new(
        clan_id: ClanId,
        clan_list: Entity<ClanList>,
        channel_list: Entity<ChannelList>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let channel_options = Self::build_channel_options(&channel_list, clan_id, cx);

        let (draft, saved_draft, selected_channel) =
            Self::initial_draft(&clan_list, clan_id, &channel_options, cx);

        let name_valid = is_valid_clan_name(&draft.clan_name)
            || draft.clan_name.trim() == saved_draft.clan_name.trim();

        let mut this = Self {
            clan_id,
            clan_list,
            channel_list: channel_list.clone(),
            settings,
            draft,
            saved_draft,
            system_message: None,
            saved_system_message: None,
            channel_options,
            selected_channel_index: selected_channel,
            channel_menu_open: false,
            name_input: None,
            name_valid,
            saving: false,
            logo_uploading: false,
            banner_uploading: false,
            system_loading: true,
            _name_sub: None,
            _channel_sub: cx.observe(&channel_list, |this, _, cx| {
                this.rebuild_channel_options(cx);
            }),
            _fetch_task: None,
            _logo_upload_task: None,
            _banner_upload_task: None,
        };
        this.fetch_system_message(cx);
        this
    }

    pub fn release(&mut self, _cx: &mut Context<Self>) {
        self._fetch_task.take();
        self._logo_upload_task.take();
        self._banner_upload_task.take();
        self._name_sub.take();
        self.name_input = None;
    }

    fn ensure_name_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_input.is_some() {
            return;
        }
        let locale = self.settings.read(cx).language.clone();
        let placeholder: SharedString =
            mezon_i18n::t(&locale, "clanSettings.clanLogo.namePlaceholder")
                .to_string()
                .into();
        let initial_name = self.draft.clan_name.clone();
        let input_bg = cx.theme().tokens.bg_tertiary;
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .height(px(40.0))
                .text_size(px(16.0))
                .bg(input_bg)
        });
        name_input.update(cx, |input, cx| {
            input.set_value(&initial_name, window, cx);
        });
        let sub = cx.subscribe(&name_input, |this, _, evt: &InputEvent, cx| {
            if *evt == InputEvent::Change {
                this.sync_name_from_input(cx);
            }
        });
        self.name_input = Some(name_input);
        self._name_sub = Some(sub);
    }

    fn initial_draft(
        clan_list: &Entity<ClanList>,
        clan_id: ClanId,
        _channel_options: &[ChannelOption],
        cx: &App,
    ) -> (ClanOverviewDraft, ClanOverviewDraft, Option<usize>) {
        let clan = clan_list.read(cx).clan_by_id(clan_id);
        let draft = ClanOverviewDraft {
            clan_name: clan.map(|c| c.name.clone()).unwrap_or_default(),
            logo: clan.and_then(|c| c.avatar_url.clone()).unwrap_or_default(),
            banner: clan.and_then(|c| c.banner_url.clone()).unwrap_or_default(),
            welcome_channel_id: clan.and_then(|c| c.welcome_channel_id),
            prevent_anonymous: clan.map(|c| c.prevent_anonymous).unwrap_or(false),
        };
        let selected = None;
        (draft.clone(), draft, selected)
    }

    fn rebuild_channel_options(&mut self, cx: &mut Context<Self>) {
        let options = Self::build_channel_options(&self.channel_list, self.clan_id, cx);
        let selected_channel = self
            .system_message
            .as_ref()
            .map(|m| m.channel_id)
            .filter(|id| id.get() != 0)
            .and_then(|channel_id| options.iter().position(|o| o.id == channel_id));
        if options == self.channel_options && selected_channel == self.selected_channel_index {
            return;
        }
        self.channel_options = options;
        self.selected_channel_index = selected_channel;
        cx.notify();
    }

    fn build_channel_options(
        channel_list: &Entity<ChannelList>,
        clan_id: ClanId,
        cx: &App,
    ) -> Vec<ChannelOption> {
        channel_list
            .read(cx)
            .categories_for_clan(clan_id)
            .iter()
            .flat_map(|category| {
                category.channels.iter().filter_map(|channel| {
                    if channel.channel_type != ChannelType::Text || channel.private {
                        return None;
                    }
                    Some(ChannelOption {
                        id: channel.id,
                        channel_label: channel.name.clone().into(),
                        category_name: category.name.to_uppercase().into(),
                    })
                })
            })
            .collect()
    }

    fn sync_name_from_input(&mut self, cx: &mut Context<Self>) {
        let value = self
            .name_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        self.draft.clan_name = value.clone();
        self.name_valid = is_valid_clan_name(&value) || value == self.saved_draft.clan_name.trim();
        cx.notify();
    }

    fn set_system_channel(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(option) = self.channel_options.get(index) {
            self.selected_channel_index = Some(index);
            self.channel_menu_open = false;
            self.draft.welcome_channel_id = Some(option.id);
            if let Some(system) = self.system_message.as_mut() {
                system.channel_id = option.id;
            }
            cx.notify();
        }
    }

    fn fetch_system_message(&mut self, cx: &mut Context<Self>) {
        self.system_loading = true;
        let clan_id = self.clan_id;
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let task = this
                .update(cx, |this, cx| {
                    this.clan_list
                        .update(cx, |store, cx| store.fetch_system_message(clan_id, cx))
                })
                .ok();
            let Some(task) = task else {
                return;
            };
            let fetched = task.await;
            let _ = this.update(cx, |this, cx| {
                this.system_loading = false;
                match fetched {
                    Ok(msg) => {
                        if msg.channel_id.get() != 0
                            && let Some(index) = this
                                .channel_options
                                .iter()
                                .position(|o| o.id == msg.channel_id)
                        {
                            this.selected_channel_index = Some(index);
                        }
                        this.system_message = Some(msg.clone());
                        this.saved_system_message = Some(msg);
                    }
                    Err(err) => {
                        tracing::error!("fetch system message failed: {err}");
                    }
                }
                cx.notify();
            });
        }));
    }

    fn has_clan_changes(&self) -> bool {
        self.draft.clan_name.trim() != self.saved_draft.clan_name.trim()
            || self.draft.logo != self.saved_draft.logo
            || self.draft.banner != self.saved_draft.banner
            || self.draft.prevent_anonymous != self.saved_draft.prevent_anonymous
            || self.draft.welcome_channel_id != self.saved_draft.welcome_channel_id
    }

    fn has_system_changes(&self) -> bool {
        match (&self.system_message, &self.saved_system_message) {
            (Some(current), Some(saved)) => current != saved,
            (Some(_), None) => true,
            _ => false,
        }
    }

    fn has_unsaved_changes(&self) -> bool {
        self.has_clan_changes() || self.has_system_changes()
    }

    pub fn should_show_save_bar(&self, _cx: &App) -> bool {
        self.has_unsaved_changes()
            && self.name_valid
            && !self.logo_uploading
            && !self.banner_uploading
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = self.saved_draft.clone();
        self.system_message = self.saved_system_message.clone();
        if let Some(input) = &self.name_input {
            input.update(cx, |input, cx| {
                input.set_value(&self.draft.clan_name, window, cx);
            });
        }
        self.name_valid = is_valid_clan_name(&self.draft.clan_name)
            || self.draft.clan_name.trim() == self.saved_draft.clan_name.trim();
        if let Some(channel_id) = self
            .system_message
            .as_ref()
            .map(|m| m.channel_id)
            .filter(|id| id.get() != 0)
            && let Some(index) = self.channel_options.iter().position(|o| o.id == channel_id)
        {
            self.selected_channel_index = Some(index);
        }
        self.channel_menu_open = false;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.sync_name_from_input(cx);
        if self.saving
            || !self.name_valid
            || !self.has_unsaved_changes()
            || self.logo_uploading
            || self.banner_uploading
        {
            return;
        }

        self.saving = true;
        cx.notify();

        let locale = self.settings.read(cx).language.clone();
        let clan_id = self.clan_id;
        let draft = self.draft.clone();
        let has_clan = self.has_clan_changes();
        let has_system = self.has_system_changes();
        let system_draft = self.system_message.clone();

        let clan_task = has_clan.then(|| {
            self.clan_list.update(cx, |store, cx| {
                store.save_clan_overview(clan_id, draft.clone(), cx)
            })
        });
        let system_task = (has_system && system_draft.is_some()).then(|| {
            self.clan_list.update(cx, |store, cx| {
                store.save_system_message(clan_id, system_draft.clone().unwrap(), cx)
            })
        });

        cx.spawn(async move |this, cx| {
            if let Some(task) = clan_task {
                match task.await {
                    Ok(()) => {}
                    Err(_) => {
                        let message = mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveError")
                            .to_string();
                        let _ = this.update(cx, |this, cx| {
                            this.saving = false;
                            cx.notify();
                        });
                        cx.update(|cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        });
                        return;
                    }
                }
            }

            if let Some(task) = system_task {
                match task.await {
                    Ok(()) => {}
                    Err(err) => {
                        tracing::error!("save system message failed: {err}");
                        let message = mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveError")
                            .to_string();
                        let _ = this.update(cx, |this, cx| {
                            this.saving = false;
                            cx.notify();
                        });
                        cx.update(|cx| {
                            Shell::global(cx).update(cx, |shell, cx| shell.error(message, cx));
                        });
                        return;
                    }
                }
            }

            let _ = this.update(cx, |this, cx| {
                this.saved_draft = draft;
                if has_system {
                    this.saved_system_message = system_draft;
                }
                this.saving = false;
                cx.notify();
            });
            let success =
                mezon_i18n::t(&locale, "clanOverviewSetting.toast.saveSuccess").to_string();
            cx.update(|cx| {
                Shell::global(cx).update(cx, |shell, cx| shell.success(success, cx));
            });
        })
        .detach();
    }

    fn pick_image(
        &mut self,
        kind: ImageUploadKind,
        max_size: u64,
        prompt_key: &'static str,
        apply_url: fn(&mut Self, String),
        cx: &mut Context<Self>,
    ) {
        match kind {
            ImageUploadKind::Logo => self.logo_uploading = true,
            ImageUploadKind::Banner => self.banner_uploading = true,
        }
        cx.notify();

        let locale = self.settings.read(cx).language.clone();
        let prompt: SharedString = mezon_i18n::t(&locale, prompt_key).to_string().into();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt),
        });
        let task = cx.spawn(async move |this, cx| {
            let finish = |this: &mut OverviewSettingPage| match kind {
                ImageUploadKind::Logo => {
                    this.logo_uploading = false;
                    this._logo_upload_task = None;
                }
                ImageUploadKind::Banner => {
                    this.banner_uploading = false;
                    this._banner_upload_task = None;
                }
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
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !ClanImageMimeType::is_allowed_extension(&ext) {
                let message_key = match kind {
                    ImageUploadKind::Logo => "clanSettings.clanLogo.modal.content",
                    ImageUploadKind::Banner => "clanSettings.clanBanner.modal.content",
                };
                let message = mezon_i18n::t(&locale, message_key).to_string();
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
            if file_size > max_size {
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
                    this.clan_list
                        .update(cx, |store, cx| store.upload_clan_image(&path, max_size, cx))
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
                        apply_url(this, url);
                        finish(this);
                        cx.notify();
                    });
                }
                Err(err) => {
                    tracing::error!("clan image upload failed: {err}");
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
        match kind {
            ImageUploadKind::Logo => self._logo_upload_task = Some(task),
            ImageUploadKind::Banner => self._banner_upload_task = Some(task),
        }
    }

    fn toggle_system_flag(
        &mut self,
        update: impl FnOnce(&mut ClanSystemMessage),
        cx: &mut Context<Self>,
    ) {
        if let Some(system) = self.system_message.as_mut() {
            update(system);
            cx.notify();
        }
    }

    fn remove_clan_logo(&mut self, cx: &mut Context<Self>) {
        self.draft.logo.clear();
        cx.notify();
    }

    fn remove_clan_banner(&mut self, cx: &mut Context<Self>) {
        self.draft.banner.clear();
        cx.notify();
    }

    fn pick_clan_logo(&mut self, cx: &mut Context<Self>) {
        self.pick_image(
            ImageUploadKind::Logo,
            MAX_CLAN_LOGO_BYTES,
            "clanSettings.clanLogo.uploadImage",
            |this, url| {
                this.draft.logo = url;
            },
            cx,
        );
    }

    fn pick_clan_banner(&mut self, cx: &mut Context<Self>) {
        self.pick_image(
            ImageUploadKind::Banner,
            MAX_CLAN_BANNER_BYTES,
            "clanSettings.clanBanner.uploadBackground",
            |this, url| {
                this.draft.banner = url;
            },
            cx,
        );
    }

    fn render_logo_block(
        &self,
        _locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let logo = self.draft.logo.clone();
        let has_logo = !logo.is_empty();
        let logo_preview = AppConfig::try_global(cx)
            .map(|config| config.imgproxy_url(&logo, 200, 200, "fill"))
            .unwrap_or(logo);

        let avatar = div()
            .absolute()
            .top_0()
            .left_0()
            .size(px(100.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .when(has_logo, |el| {
                el.child(
                    img(logo_preview)
                        .size_full()
                        .rounded_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
            })
            .when(!has_logo, |el| {
                el.child(
                    Icon::new(IconName::UploadImage)
                        .size(px(32.0))
                        .text_color(theme.text_secondary),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border),
            );

        div()
            .relative()
            .flex_shrink_0()
            .w(px(115.0))
            .h(px(100.0))
            .child(avatar)
            .child(
                div()
                    .id("clan-logo-action")
                    .absolute()
                    .top(px(2.0))
                    .right(px(0.0))
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
                    .when(has_logo, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.remove_clan_logo(cx)))
                    })
                    .when(!has_logo, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.pick_clan_logo(cx)))
                    })
                    .child(
                        Icon::new(if has_logo {
                            IconName::Close
                        } else {
                            IconName::SelectFileIcon
                        })
                        .size(px(16.0))
                        .text_color(if has_logo {
                            theme.danger_text
                        } else {
                            theme.text_secondary
                        }),
                    ),
            )
    }

    fn render_logo_upload_section(
        &self,
        locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_start()
            .gap(px(10.0))
            .child(self.render_logo_block(locale, theme, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_start()
                    .ml(px(10.0))
                    .gap(px(8.0))
                    .child(logo_helper_text(
                        mezon_i18n::t(locale, "clanSettings.clanLogo.recommendedSize"),
                        theme,
                    ))
                    .child(
                        Button::new("clan-logo-upload")
                            .label(mezon_i18n::t(locale, "clanSettings.clanLogo.uploadImage"))
                            .primary()
                            .with_size(Size::Large)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_clan_logo(cx))),
                    ),
            )
    }

    fn render_banner_preview(
        &self,
        _locale: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let banner = self.draft.banner.clone();
        let has_banner = !banner.is_empty();

        div()
            .relative()
            .w_full()
            .aspect_ratio(16.0 / 9.0)
            .rounded_lg()
            .overflow_hidden()
            .when(has_banner, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(img(banner).size_full().object_fit(gpui::ObjectFit::Cover)),
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
                    .id("clan-banner-action")
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
                        el.on_click(cx.listener(|this, _, _, cx| this.remove_clan_banner(cx)))
                    })
                    .when(!has_banner, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.pick_clan_banner(cx)))
                    })
                    .child(
                        Icon::new(if has_banner {
                            IconName::Close
                        } else {
                            IconName::SelectFileIcon
                        })
                        .size(px(16.0))
                        .text_color(if has_banner {
                            theme.danger_text
                        } else {
                            theme.text_secondary
                        }),
                    ),
            )
    }

    fn render_channel_row_label(option: &ChannelOption, theme: &Theme) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1()
            .overflow_hidden()
            .child(
                Icon::new(IconName::Hashtag)
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(theme.text_secondary),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(theme.text_primary)
                    .child(option.channel_label.clone()),
            )
            .child(
                div()
                    .ml(px(20.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_primary)
                    .child(option.category_name.clone()),
            )
    }

    fn render_channel_picker(&mut self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .selected_channel_index
            .and_then(|index| self.channel_options.get(index));
        let open = self.channel_menu_open;
        let selected_index = self.selected_channel_index;

        let mut trigger = h_flex()
            .id("clan-system-channel")
            .w_full()
            .h(px(40.0))
            .items_center()
            .gap_2()
            .px(px(12.0))
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.bg_tertiary)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .when_some(selected, |el, option| {
                        el.child(Self::render_channel_row_label(option, theme))
                    })
                    .when(selected.is_none(), |el| {
                        el.text_sm().text_color(theme.text_muted).child("Select…")
                    }),
            )
            .child(
                Icon::new(IconName::ArrowDownFill)
                    .size(px(16.0))
                    .flex_shrink_0()
                    .text_color(theme.text_muted),
            );
        trigger
            .interactivity()
            .on_click(cx.listener(|this, _, _, cx| {
                this.channel_menu_open = !this.channel_menu_open;
                cx.notify();
            }));

        div().relative().w_full().child(trigger).when(open, |menu| {
            menu.child(deferred(
                div()
                    .absolute()
                    .top_full()
                    .left_0()
                    .right_0()
                    .mt(px(4.0))
                    .max_h(px(200.0))
                    .p(px(4.0))
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surfaces.input_primary)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.channel_menu_open {
                            this.channel_menu_open = false;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .id("clan-system-channel-menu")
                            .overflow_y_scroll()
                            .max_h(px(192.0))
                            .child(
                                v_flex().children(self.channel_options.iter().enumerate().map(
                                    |(index, option)| {
                                        let is_selected = selected_index == Some(index);
                                        let mut row = h_flex()
                                            .id(("clan-system-channel-item", index))
                                            .w_full()
                                            .items_center()
                                            .px(px(16.0))
                                            .py(px(8.0))
                                            .rounded(px(4.0))
                                            .text_sm()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.bg_hover))
                                            .child(Self::render_channel_row_label(option, theme));
                                        if !is_selected {
                                            row.interactivity().on_click(cx.listener(
                                                move |this, _, _, cx| {
                                                    this.set_system_channel(index, cx);
                                                },
                                            ));
                                        }
                                        row
                                    },
                                )),
                            ),
                    ),
            ))
        })
    }

    fn render_toggle_row(
        label: SharedString,
        checked: bool,
        id: &'static str,
        theme: &Theme,
        on_toggle: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .py(px(4.0))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme.text_primary)
                    .child(label),
            )
            .child(Switch::new(id).checked(checked).on_click(on_toggle))
    }
}

impl Render for OverviewSettingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_name_input(window, cx);
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let name_error = (!self.name_valid)
            .then(|| mezon_i18n::t(&locale, "clanSettings.clanLogo.validationError").to_string());
        let system = self.system_message.clone();

        v_flex()
            .relative()
            .w_full()
            .gap_0()
            .pb(px(80.0))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(10.0))
                    .items_start()
                    .child(
                        flex_column().child(self.render_logo_upload_section(&locale, &theme, cx)),
                    )
                    .child(
                        flex_column().child(
                            v_flex()
                                .w_full()
                                .child(section_heading_xs(
                                    mezon_i18n::t(&locale, "clanSettings.clanLogo.clanName"),
                                    &theme,
                                ))
                                .child(self.name_input.as_ref().map_or_else(
                                    || div().into_any_element(),
                                    |input| Input::new(input).w_full().into_any_element(),
                                ))
                                .when_some(name_error, |el, err| {
                                    el.child(
                                        div()
                                            .mt(px(4.0))
                                            .text_xs()
                                            .italic()
                                            .font_weight(FontWeight::NORMAL)
                                            .text_color(theme.danger_text)
                                            .child(err),
                                    )
                                }),
                        ),
                    ),
            )
            .child(
                section_divider(div(), &theme).child(
                    h_flex()
                        .gap(px(20.0))
                        .items_start()
                        .w_full()
                        .child(
                            half_column().child(
                                v_flex()
                                    .w_full()
                                    .child(section_heading_xs(
                                        mezon_i18n::t(&locale, "clanSettings.clanBanner.title"),
                                        &theme,
                                    ))
                                    .child(
                                        body_text(
                                            mezon_i18n::t(
                                                &locale,
                                                "clanSettings.clanBanner.description",
                                            ),
                                            &theme,
                                        )
                                        .mb(px(8.0)),
                                    )
                                    .child(body_text(
                                        mezon_i18n::t(
                                            &locale,
                                            "clanSettings.clanBanner.recommendedSize",
                                        ),
                                        &theme,
                                    ))
                                    .child(
                                        div().mt(px(16.0)).child(
                                            Button::new("clan-banner-upload")
                                                .label(mezon_i18n::t(
                                                    &locale,
                                                    "clanSettings.clanBanner.uploadBackground",
                                                ))
                                                .primary()
                                                .with_size(Size::Large)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.pick_clan_banner(cx);
                                                })),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            half_column().child(
                                div()
                                    .w_full()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.tokens.theme_setting_nav)
                                    .overflow_hidden()
                                    .child(self.render_banner_preview(&locale, &theme, cx)),
                            ),
                        ),
                ),
            )
            .when(!self.system_loading && system.is_some(), |el| {
                let system = system.unwrap();
                el.child(
                    section_divider(div(), &theme).child(
                        v_flex()
                            .gap_0()
                            .child(section_heading_sm(
                                mezon_i18n::t(&locale, "clanSettings.systemMessages.title"),
                                &theme,
                            ))
                            .child(self.render_channel_picker(&theme, cx))
                            .child(
                                body_text(
                                    mezon_i18n::t(
                                        &locale,
                                        "clanSettings.systemMessages.description",
                                    ),
                                    &theme,
                                )
                                .py(px(8.0)),
                            )
                            .child(Self::render_toggle_row(
                                mezon_i18n::t(&locale, "clanSettings.systemMessages.welcomeRandom")
                                    .into(),
                                system.welcome_random,
                                "system-welcome-random",
                                &theme,
                                cx.listener(|this, next: &bool, _, cx| {
                                    this.toggle_system_flag(|s| s.welcome_random = *next, cx);
                                }),
                            ))
                            .child(Self::render_toggle_row(
                                mezon_i18n::t(&locale, "clanSettings.systemMessages.setupTips")
                                    .into(),
                                system.setup_tips,
                                "system-setup-tips",
                                &theme,
                                cx.listener(|this, next: &bool, _, cx| {
                                    this.toggle_system_flag(|s| s.setup_tips = *next, cx);
                                }),
                            ))
                            .child(Self::render_toggle_row(
                                mezon_i18n::t(&locale, "clanSettings.systemMessages.auditLog")
                                    .into(),
                                !system.hide_audit_log,
                                "system-audit-log",
                                &theme,
                                cx.listener(|this, next: &bool, _, cx| {
                                    this.toggle_system_flag(|s| s.hide_audit_log = !*next, cx);
                                }),
                            )),
                    ),
                )
            })
            .child(
                section_divider(div(), &theme).child(
                    v_flex()
                        .gap_0()
                        .child(section_heading_sm(
                            mezon_i18n::t(&locale, "clanSettings.systemMessages.anoTitle"),
                            &theme,
                        ))
                        .child(Self::render_toggle_row(
                            mezon_i18n::t(&locale, "clanSettings.systemMessages.anoDesc").into(),
                            self.draft.prevent_anonymous,
                            "prevent-anonymous",
                            &theme,
                            cx.listener(|this, next: &bool, _, cx| {
                                this.draft.prevent_anonymous = *next;
                                cx.notify();
                            }),
                        )),
                ),
            )
    }
}

pub fn render_clan_overview_save_bar(
    overview: Entity<OverviewSettingPage>,
    locale: &str,
    _theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let saving = overview.read(cx).is_saving();
    let reset = overview.clone();
    let save = overview.clone();
    UnsavedChangesBar::new(
        mezon_i18n::t(locale, "clanSettings.modalSaveChanges.title"),
        Button::new("clan-overview-reset")
            .label(mezon_i18n::t(locale, "clanSettings.modalSaveChanges.reset"))
            .ghost()
            .on_click(move |_, window, cx| {
                reset.update(cx, |page, cx| page.reset(window, cx));
            }),
        Button::new("clan-overview-save")
            .label(mezon_i18n::t(
                locale,
                "clanSettings.modalSaveChanges.saveChanges",
            ))
            .primary()
            .disabled(saving)
            .on_click(move |_, _, cx| {
                save.update(cx, |page, cx| page.save(cx));
            }),
    )
}
