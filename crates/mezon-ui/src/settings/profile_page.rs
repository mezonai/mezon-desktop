use crate::components::compositions::CustomStatusBubble;
use crate::components::primitives::{
    Avatar, Button as GpuiButton, ButtonVariants, Icon, IconName, Input, InputEvent, InputState,
    Label, TextArea, TextAreaEvent, TextAreaField, h_flex, v_flex,
};
use gpui::{
    Context, Entity, FontWeight, MouseButton, MouseDownEvent, PathPromptOptions, Pixels, Point,
    Rgba, SharedString, Subscription, Task, Window, anchored, deferred, div, img, prelude::*, px,
};
use mezon_store::{
    AccountEvent, AccountStore, AppConfig, ClanList, LoginStore, Settings, UserAccount,
};

use super::clan_profile_section::ClanProfileSection;
use super::edit_avatar::EditAvatar;
use crate::app::shell::Shell;
use crate::theme::{ActiveTheme, Theme};
use crate::{
    image_cache::LruImageCache,
    util::avatar_color::{spawn_banner_color_task, spawn_local_banner_color_task},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileTab {
    User,
    Clan,
}

struct ProfileState {
    username: SharedString,
    display_name: SharedString,
    about_me: SharedString,
    avatar_url: Option<SharedString>,
    logo_url: Option<SharedString>,
    status: SharedString,
    custom_status: SharedString,
    original_display_name: SharedString,
    original_about_me: SharedString,
    original_avatar_url: Option<SharedString>,
    original_logo_url: Option<SharedString>,
    loading: bool,
    saving: bool,
}

impl ProfileState {
    fn from_account(account: &UserAccount) -> Self {
        let display_name: SharedString = account.display_name.clone().into();
        let about_me: SharedString = account.about_me.clone().unwrap_or_default().into();
        let avatar_url: Option<SharedString> = account.avatar_url.clone().map(Into::into);
        Self {
            username: account.username.clone().into(),
            display_name: display_name.clone(),
            about_me: about_me.clone(),
            avatar_url: avatar_url.clone(),
            logo_url: account.logo.clone().map(Into::into),
            status: account.status.clone().into(),
            custom_status: account.user_status.clone().into(),
            original_display_name: display_name,
            original_about_me: about_me,
            original_avatar_url: avatar_url,
            original_logo_url: account.logo.clone().map(Into::into),
            loading: false,
            saving: false,
        }
    }
}

pub struct ProfilePage {
    settings: Entity<Settings>,
    clan_list: Entity<ClanList>,
    active_tab: ProfileTab,
    profile: Option<ProfileState>,
    display_name_input: Option<Entity<InputState>>,
    about_me_input: Option<Entity<TextArea>>,
    _subscriptions: Vec<Subscription>,
    fetch_error: bool,
    account_loaded: bool,
    clan_section: Option<Entity<ClanProfileSection>>,
    clan_tab_id: Option<mezon_store::ClanId>,
    avatar_local_preview: Option<std::path::PathBuf>,
    dm_icon_menu_position: Option<Point<Pixels>>,
    avatar_image_cache: Entity<LruImageCache>,
    banner_color: Option<Rgba>,
    banner_source: String,
    banner_task: Option<Task<()>>,
    discard_on_next_render: bool,
    custom_status_bubble: Entity<CustomStatusBubble>,
    #[allow(dead_code)]
    show_delete_confirm: bool,
}

impl ProfilePage {
    fn ensure_clan_section(&mut self, cx: &mut Context<Self>) -> Entity<ClanProfileSection> {
        if let Some(section) = &self.clan_section {
            return section.clone();
        }
        let section =
            cx.new(|cx| ClanProfileSection::new(self.settings.clone(), self.clan_list.clone(), cx));
        cx.observe(&section, |_, _, cx| cx.notify()).detach();
        self.clan_section = Some(section.clone());
        section
    }

    pub fn new(
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&clan_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |this, store, cx| {
            if let Some(account) = store.read(cx).account.as_ref() {
                if !this.account_loaded {
                    this.account_loaded = true;
                    this.profile = Some(ProfileState::from_account(account));
                    this.refresh_banner_color(cx);
                    cx.notify();
                } else if let Some(profile) = &mut this.profile
                    && (profile.status.as_ref() != account.status
                        || profile.custom_status.as_ref() != account.user_status)
                {
                    profile.status = account.status.clone().into();
                    profile.custom_status = account.user_status.clone().into();
                    cx.notify();
                }
            }
            this.refresh_banner_color(cx);
        })
        .detach();
        cx.subscribe(
            &AccountStore::global(cx),
            |this, store, event, cx| match event {
                AccountEvent::AccountLoaded => {
                    if !this.is_dirty()
                        && let Some(account) = store.read(cx).account.as_ref()
                    {
                        this.profile = Some(ProfileState::from_account(account));
                        this.display_name_input = None;
                        this.about_me_input = None;
                        this._subscriptions.clear();
                        this.avatar_local_preview = None;
                        this.refresh_banner_color(cx);
                        cx.notify();
                    }
                }
                AccountEvent::AccountLoadFailed => {
                    this.fetch_error = true;
                    cx.notify();
                }
                AccountEvent::AccountSaved => {
                    if let Some(state) = &mut this.profile {
                        state.original_display_name = state.display_name.clone();
                        state.original_about_me = state.about_me.clone();
                        state.original_avatar_url = state.avatar_url.clone();
                        state.original_logo_url = state.logo_url.clone();
                        state.saving = false;
                    }
                    Shell::global(cx).update(cx, |shell, cx| shell.success("Profile saved", cx));
                }
                AccountEvent::AccountSaveFailed(msg) => {
                    if let Some(state) = &mut this.profile {
                        state.saving = false;
                    }
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(format!("Failed to save: {}", msg), cx)
                    });
                }
                AccountEvent::UserAvatarUploaded(url) => {
                    if let Some(state) = &mut this.profile {
                        state.avatar_url = Some(url.clone().into());
                    }
                    this.refresh_banner_color(cx);
                    cx.notify();
                }
                AccountEvent::UserAvatarUploadFailed(msg) => {
                    this.avatar_local_preview = None;
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(format!("Failed to upload avatar: {}", msg), cx)
                    });
                }
                AccountEvent::DirectMessageIconUploaded(url) => {
                    if let Some(state) = &mut this.profile {
                        state.logo_url = Some(url.clone().into());
                    }
                    cx.notify();
                }
                AccountEvent::DirectMessageIconUploadFailed(msg) => {
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.error(format!("Failed to upload direct message icon: {}", msg), cx)
                    });
                }
                AccountEvent::AccountDeleted => {
                    let locale = this.settings.read(cx).language.clone();
                    let message =
                        mezon_i18n::t(&locale, "accountSetting.toast.deleteAccount.success");
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.close_modal(cx);
                        shell.success(message, cx);
                    });
                    LoginStore::global(cx).update(cx, |store, cx| store.logout(cx));
                }
                AccountEvent::AccountDeleteFailed => {
                    let locale = this.settings.read(cx).language.clone();
                    let message =
                        mezon_i18n::t(&locale, "accountSetting.toast.deleteAccount.error");
                    Shell::global(cx).update(cx, |shell, cx| {
                        shell.close_modal(cx);
                        shell.error(message, cx);
                    });
                }
                _ => {}
            },
        )
        .detach();

        AccountStore::global(cx).update(cx, |store, cx| store.ensure_account(cx));

        let account_store = AccountStore::global(cx);
        let (profile, account_loaded) = match account_store.read(cx).account.as_ref() {
            Some(account) => (Some(ProfileState::from_account(account)), true),
            None => (None, false),
        };

        let mut this = Self {
            settings,
            clan_list,
            active_tab: ProfileTab::User,
            clan_tab_id: None,
            profile,
            display_name_input: None,
            about_me_input: None,
            _subscriptions: Vec::new(),
            fetch_error: false,
            account_loaded,
            clan_section: None,
            avatar_local_preview: None,
            dm_icon_menu_position: None,
            avatar_image_cache: crate::image_cache::shared_avatar_cache(cx),
            banner_color: None,
            banner_source: String::new(),
            banner_task: None,
            discard_on_next_render: false,
            custom_status_bubble: cx.new(|_| CustomStatusBubble::new()),
            show_delete_confirm: false,
        };
        this.refresh_banner_color(cx);
        this
    }

    pub fn discard_drafts_on_next_render(&mut self, cx: &mut Context<Self>) {
        self.discard_on_next_render = true;
        cx.notify();
    }

    pub fn show_user_profile(&mut self, cx: &mut Context<Self>) {
        self.active_tab = ProfileTab::User;
        self.clan_tab_id = None;
        cx.notify();
    }

    pub fn show_clan_profile(&mut self, clan_id: mezon_store::ClanId, cx: &mut Context<Self>) {
        self.active_tab = ProfileTab::Clan;
        self.clan_tab_id = Some(clan_id);
        let display_name = self
            .profile
            .as_ref()
            .map(|profile| profile.display_name.clone())
            .unwrap_or_default();
        let username = self
            .profile
            .as_ref()
            .map(|profile| profile.username.clone())
            .unwrap_or_default();
        let (avatar_url, status, custom_status) = self.profile.as_ref().map_or_else(
            || (None, SharedString::default(), SharedString::default()),
            |profile| {
                (
                    profile.avatar_url.clone(),
                    profile.status.clone(),
                    profile.custom_status.clone(),
                )
            },
        );
        let section = self.ensure_clan_section(cx);
        section.update(cx, |section, cx| {
            section.set_user_profile(
                display_name,
                username,
                avatar_url,
                status,
                custom_status,
                cx,
            );
            section.fetch(&clan_id.get().to_string(), cx);
        });
        cx.notify();
    }

    fn refresh_banner_color(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.avatar_local_preview.clone() {
            let source = format!("local:{}", path.display());
            if source == self.banner_source {
                return;
            }
            self.banner_source = source;
            self.banner_color = None;
            self.banner_task = spawn_local_banner_color_task(path, cx, |this, color, cx| {
                this.banner_color = Some(color);
                cx.notify();
            });
            return;
        }
        let source = self
            .profile
            .as_ref()
            .and_then(|profile| profile.avatar_url.as_ref())
            .map(|url| crate::util::imgproxy::profile_url(cx, url))
            .unwrap_or_default();
        if source == self.banner_source {
            return;
        }
        self.banner_source = source.clone();
        self.banner_color = None;
        self.banner_task = spawn_banner_color_task(
            self.avatar_image_cache.clone(),
            source,
            cx,
            |this, color, cx| {
                this.banner_color = Some(color);
                cx.notify();
            },
        );
    }

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let display_ph = mezon_i18n::t(&locale, "setting.profile.displayNamePlaceholder");
        let about_ph = mezon_i18n::t(&locale, "setting.profile.aboutPlaceholder");
        let display = cx.new(|cx| InputState::new(window, cx).placeholder(display_ph));
        let about = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder(about_ph)
                .min_height(px(112.))
                .max_visible_lines(6)
                .max_length(128)
        });

        if let Some(state) = &self.profile {
            display.update(cx, |input, cx| {
                input.set_value(&state.display_name, window, cx);
            });
            about.update(cx, |input, cx| {
                input.set_value(&state.about_me, cx);
            });
        }

        self._subscriptions.push(cx.subscribe_in(&display, window, {
            let display = display.clone();
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event
                    && let Some(state) = &mut this.profile
                    && !state.saving
                {
                    let value = display.read(cx).value().to_string();
                    state.display_name = value.into();
                    cx.notify();
                }
            }
        }));

        self._subscriptions.push(cx.subscribe_in(&about, window, {
            let about = about.clone();
            move |this: &mut Self, _, event: &TextAreaEvent, _, cx| {
                if *event == TextAreaEvent::Change
                    && let Some(state) = &mut this.profile
                    && !state.saving
                {
                    let value = about.read(cx).value().to_string();
                    state.about_me = value.into();
                    cx.notify();
                }
            }
        }));

        self.display_name_input = Some(display);
        self.about_me_input = Some(about);
    }

    fn is_dirty(&self) -> bool {
        if let Some(state) = &self.profile {
            state.display_name != state.original_display_name
                || state.about_me != state.original_about_me
                || state.avatar_url != state.original_avatar_url
                || state.logo_url != state.original_logo_url
        } else {
            false
        }
    }

    fn discard_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.profile.as_ref().map(|s| {
            (
                s.original_display_name.clone(),
                s.original_about_me.clone(),
                s.original_avatar_url.clone(),
                s.original_logo_url.clone(),
            )
        });

        if let (Some((display_name, about_me, avatar_url, logo_url)), Some(state)) =
            (original.clone(), &mut self.profile)
        {
            state.display_name = display_name;
            state.about_me = about_me;
            state.avatar_url = avatar_url;
            state.logo_url = logo_url;
        }
        self.avatar_local_preview = None;
        self.refresh_banner_color(cx);

        if let Some((display_name, about_me, _, _)) = original {
            if let Some(input) = &self.display_name_input {
                input.update(cx, |input_state: &mut InputState, input_cx| {
                    input_state.set_value(display_name.clone(), window, input_cx);
                });
            }
            if let Some(input) = &self.about_me_input {
                input.update(cx, |input_state, input_cx| {
                    input_state.set_value(about_me, input_cx);
                });
            }
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.save_user_profile(cx);
    }

    fn save_user_profile(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.profile else {
            return;
        };
        if state.saving {
            return;
        }
        state.saving = true;
        cx.notify();

        let display_name: String = state.display_name.to_string();
        let about_me: String = state.about_me.to_string();
        let avatar_url: Option<String> = state.avatar_url.as_ref().map(|s| s.to_string());
        let logo_url = Some(
            state
                .logo_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        );

        AccountStore::global(cx).update(cx, |store, cx| {
            store.save_account(display_name, avatar_url, about_me, logo_url, cx);
        });
    }

    fn render_user_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let avatar_display = self
            .profile
            .as_ref()
            .and_then(|p| p.avatar_url.as_ref())
            .map(|url| SharedString::from(crate::util::imgproxy::profile_url(cx, url.as_ref())));
        let custom_status = self
            .profile
            .as_ref()
            .map_or_else(SharedString::default, |profile| {
                profile.custom_status.clone()
            });
        let custom_status_bubble = self.custom_status_bubble.clone();
        custom_status_bubble.update(cx, |bubble, cx| {
            bubble.set_content(
                custom_status,
                px(214.),
                theme.tokens.bg_secondary,
                theme.border,
                theme.text_secondary,
                cx,
            );
        });
        let form = self.render_form(theme, cx, avatar_display.clone());
        let preview = self.render_preview(
            theme,
            &locale,
            avatar_display,
            self.avatar_local_preview.clone(),
        );
        v_flex().gap_6().child(
            h_flex()
                .gap_8()
                .items_start()
                .child(div().min_w_0().flex_1().flex_basis(px(0.)).child(form))
                .child(div().min_w_0().flex_1().flex_basis(px(0.)).child(preview)),
        )
    }
}

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.discard_on_next_render {
            self.discard_on_next_render = false;
            self.discard_changes(window, cx);
            if let Some(section) = &self.clan_section {
                section.update(cx, |section, cx| section.discard_changes(window, cx));
            }
        }
        if self.profile.as_ref().is_some_and(|p| !p.loading) && self.display_name_input.is_none() {
            self.init_inputs(window, cx);
        }

        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        if self.fetch_error {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.profile.failedToLoad"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        if self.profile.is_none() || self.profile.as_ref().is_some_and(|p| p.loading) {
            return v_flex()
                .gap_4()
                .child(
                    Label::new(mezon_i18n::t(&locale, "setting.profile.loading"))
                        .text_color(theme.text_muted),
                )
                .into_any_element();
        }

        let clan_list = self.clan_list.read(cx);
        let active_clan_id = self
            .clan_tab_id
            .filter(|id| clan_list.clan_by_id(*id).is_some())
            .or_else(|| clan_list.active_clan().map(|clan| clan.id));
        if active_clan_id.is_none() && self.active_tab == ProfileTab::Clan {
            self.active_tab = ProfileTab::User;
        }
        let is_clan = self.active_tab == ProfileTab::Clan;

        // Render tab toggle
        let user_active = !is_clan;
        let active_border = theme.brand;
        let inactive_border = gpui::transparent_black();

        let tabs = h_flex()
            .gap_4()
            .mb_4()
            .child(
                div()
                    .id("user-profile-tab")
                    .cursor_pointer()
                    .pt_1()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .border_b_2()
                    .when(user_active, |el| {
                        el.border_color(active_border)
                            .text_color(theme.text_primary)
                    })
                    .when(!user_active, |el| {
                        el.border_color(inactive_border)
                            .text_color(theme.text_muted)
                    })
                    .child(mezon_i18n::t(&locale, "setting.profile.userProfile"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.active_tab == ProfileTab::User {
                            return;
                        }
                        if let Some(section) = &this.clan_section {
                            section.update(cx, |section, cx| section.discard_changes(window, cx));
                        }
                        this.active_tab = ProfileTab::User;
                        cx.notify();
                    })),
            )
            .when(active_clan_id.is_some(), |tabs| {
                tabs.child(
                    div()
                        .id("clan-profile-tab")
                        .cursor_pointer()
                        .pt_1()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .border_b_2()
                        .when(is_clan, |el| {
                            el.border_color(active_border)
                                .text_color(theme.text_primary)
                        })
                        .when(!is_clan, |el| {
                            el.border_color(inactive_border)
                                .text_color(theme.text_muted)
                        })
                        .child(mezon_i18n::t(&locale, "setting.profile.clanProfile"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.active_tab == ProfileTab::Clan {
                                return;
                            }
                            this.discard_changes(window, cx);
                            this.active_tab = ProfileTab::Clan;
                            let display_name = this
                                .profile
                                .as_ref()
                                .map(|p| p.display_name.clone())
                                .unwrap_or_default();
                            let username = this
                                .profile
                                .as_ref()
                                .map(|p| p.username.clone())
                                .unwrap_or_default();
                            let (avatar_url, status, custom_status) =
                                this.profile.as_ref().map_or_else(
                                    || (None, SharedString::default(), SharedString::default()),
                                    |profile| {
                                        (
                                            profile.avatar_url.clone(),
                                            profile.status.clone(),
                                            profile.custom_status.clone(),
                                        )
                                    },
                                );
                            let clan_list = this.clan_list.read(cx);
                            let active_clan_id = this
                                .clan_tab_id
                                .filter(|id| clan_list.clan_by_id(*id).is_some())
                                .or_else(|| clan_list.active_clan().map(|c| c.id));
                            let Some(active_clan_id) = active_clan_id else {
                                this.active_tab = ProfileTab::User;
                                cx.notify();
                                return;
                            };
                            this.clan_tab_id = Some(active_clan_id);
                            let section = this.ensure_clan_section(cx);
                            section.update(cx, |s, cx| {
                                s.set_user_profile(
                                    display_name,
                                    username,
                                    avatar_url,
                                    status,
                                    custom_status,
                                    cx,
                                );
                                s.fetch(&active_clan_id.to_string(), cx);
                            });
                            cx.notify();
                        })),
                )
            });

        let body: gpui::AnyElement = if is_clan {
            if let Some(section) = &self.clan_section {
                section.clone().into_any_element()
            } else {
                v_flex()
                    .gap_4()
                    .child(
                        Label::new(mezon_i18n::t(&locale, "setting.profile.loadingClan"))
                            .text_color(theme.text_muted)
                            .text_sm(),
                    )
                    .into_any_element()
            }
        } else {
            self.render_user_section(&theme, cx).into_any_element()
        };

        let clan_section = self.clan_section.clone();
        let (has_unsaved_changes, is_saving) = if is_clan {
            clan_section
                .as_ref()
                .map(|section| {
                    let section = section.read(cx);
                    (section.has_unsaved_changes(), section.is_saving())
                })
                .unwrap_or((false, false))
        } else {
            (
                self.is_dirty(),
                self.profile.as_ref().is_some_and(|profile| profile.saving),
            )
        };
        let profile_entity = cx.entity().clone();

        v_flex()
            .relative()
            .h_full()
            .min_h_0()
            .child(
                v_flex()
                    .id("profile-content-scroll")
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .overflow_y_scroll()
                    .pb(if has_unsaved_changes || is_saving {
                        px(92.)
                    } else {
                        px(8.)
                    })
                    .child(tabs)
                    .child(body)
                    // Delete Account button (only for user profile tab)
                    .when(!is_clan, |el| {
                        el.child(
                            h_flex().child(
                                GpuiButton::new("delete-account-btn")
                                    .label(mezon_i18n::t(&locale, "setting.profile.deleteAccount"))
                                    .danger()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let locale = this.settings.read(cx).language.clone();
                                        Shell::global(cx).update(cx, |shell, cx| {
                                            shell.confirm_delete_account(&locale, window, cx)
                                        });
                                    })),
                            ),
                        )
                    }),
            )
            .when(has_unsaved_changes || is_saving, |el| {
                el.child(
                    h_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(12.))
                        .min_h(px(64.))
                        .p_3()
                        .gap_3()
                        .items_center()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.bg_floating)
                        .shadow_lg()
                        .child(
                            div()
                                .flex_1()
                                .text_base()
                                .text_color(theme.text_primary)
                                .child(mezon_i18n::t(&locale, "setting.profile.unsavedWarning")),
                        )
                        .child(
                            GpuiButton::new("active-profile-reset-btn")
                                .label(mezon_i18n::t(&locale, "profileSetting.reset"))
                                .disabled(is_saving)
                                .text_color(theme.text_muted)
                                .ghost()
                                .on_click({
                                    let profile_entity = profile_entity.clone();
                                    let clan_section = clan_section.clone();
                                    move |_, window, cx| {
                                        if is_clan {
                                            if let Some(section) = &clan_section {
                                                section.update(cx, |section, cx| {
                                                    section.discard_changes(window, cx)
                                                });
                                            }
                                        } else {
                                            profile_entity.update(cx, |page, cx| {
                                                page.discard_changes(window, cx);
                                                cx.notify();
                                            });
                                        }
                                    }
                                }),
                        )
                        .child(
                            GpuiButton::new("active-profile-save-btn")
                                .label(if is_saving {
                                    mezon_i18n::t(&locale, "setting.profile.saving")
                                } else {
                                    mezon_i18n::t(&locale, "setting.profile.saveChanges")
                                })
                                .disabled(is_saving)
                                .primary()
                                .on_click({
                                    let profile_entity = profile_entity.clone();
                                    let clan_section = clan_section.clone();
                                    move |_, _, cx| {
                                        if is_clan {
                                            if let Some(section) = &clan_section {
                                                section.update(cx, |section, cx| {
                                                    section.save_changes(cx)
                                                });
                                            }
                                        } else {
                                            profile_entity.update(cx, |page, cx| page.save(cx));
                                        }
                                    }
                                }),
                        ),
                )
            })
            .into_any_element()
    }
}

impl ProfilePage {
    fn render_form(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
        _avatar_display: Option<SharedString>,
    ) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let about_me: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.about_me.clone());
        let logo_url = self
            .profile
            .as_ref()
            .and_then(|profile| profile.logo_url.clone());
        let dm_icon_menu = self.dm_icon_menu_position.map(|position| {
            let dismiss = cx.entity().downgrade();
            let remove = cx.entity().downgrade();
            let label = mezon_i18n::t(&locale, "clanRoles.roleManagement.removeIcon");
            deferred(
                div()
                    .child(anchored().position(Point::default()).child(
                        div().w(px(100000.)).h(px(100000.)).on_mouse_down(
                            MouseButton::Left,
                            move |_: &MouseDownEvent, _, cx| {
                                let _ = dismiss.update(cx, |this, cx| {
                                    this.dm_icon_menu_position = None;
                                    cx.notify();
                                });
                            },
                        ),
                    ))
                    .child(
                        anchored().position(position).snap_to_window().child(
                            div()
                                .w(px(132.))
                                .p_1()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.tokens.bg_theme_contexify)
                                .shadow_lg()
                                .occlude()
                                .child(
                                    div()
                                        .id("dm-icon-remove-menu-item")
                                        .w_full()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .text_sm()
                                        .text_color(theme.text_primary)
                                        .cursor_pointer()
                                        .hover(|element| element.bg(theme.bg_hover))
                                        .child(label)
                                        .on_click(move |_, _, cx| {
                                            let _ = remove.update(cx, |this, cx| {
                                                if let Some(profile) = &mut this.profile {
                                                    profile.logo_url = None;
                                                }
                                                this.dm_icon_menu_position = None;
                                                cx.notify();
                                            });
                                        }),
                                ),
                        ),
                    ),
            )
        });

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(&locale, "setting.profile.displayName")),
                    )
                    .child(
                        Input::new(
                            self.display_name_input
                                .as_ref()
                                .expect("display_name_input not initialized"),
                        )
                        .w_full(),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(&locale, "profileSetting.avatar")),
                    )
                    .child(
                        h_flex()
                            .gap_5()
                            .items_center()
                            .child(
                                GpuiButton::new("change-avatar-btn")
                                    .label(mezon_i18n::t(&locale, "common.changeAvatar"))
                                    .text_color(theme.text_primary)
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let locale = this.settings.read(cx).language.clone();
                                        let root_entity = cx.entity().clone();
                                        let rx = cx.prompt_for_paths(PathPromptOptions {
                                            files: true,
                                            directories: false,
                                            multiple: false,
                                            prompt: Some(
                                                mezon_i18n::t(
                                                    &locale,
                                                    "setting.profile.chooseAvatar",
                                                )
                                                .into(),
                                            ),
                                        });
                                        cx.spawn(async move |_this, cx| {
                                            let Some(paths) =
                                                crate::util::file_dialog::resolve(rx, cx).await
                                            else {
                                                return;
                                            };
                                            let Some(path) = paths.into_iter().next() else {
                                                return;
                                            };
                                            let is_gif = path
                                                .extension()
                                                .and_then(|extension| extension.to_str())
                                                .is_some_and(|extension| {
                                                    extension.eq_ignore_ascii_case("gif")
                                                });
                                            if !is_gif {
                                                let preview_entity = root_entity.clone();
                                                cx.update(|cx| {
                                                    EditAvatar::open(
                                                        path,
                                                        move |cropped, _, cx| {
                                                            let preview_path = cropped.clone();
                                                            preview_entity.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.avatar_local_preview =
                                                                        Some(preview_path);
                                                                    cx.notify();
                                                                },
                                                            );
                                                            AccountStore::global(cx).update(
                                                                cx,
                                                                |store, cx| {
                                                                    store.upload_user_avatar(
                                                                        &cropped, cx,
                                                                    )
                                                                },
                                                            );
                                                        },
                                                        cx,
                                                    );
                                                });
                                                return;
                                            }
                                            root_entity.update(cx, |this, cx| {
                                                this.avatar_local_preview = Some(path.clone());
                                                cx.notify();
                                                AccountStore::global(cx).update(cx, |store, cx| {
                                                    store.upload_user_avatar(&path, cx)
                                                })
                                            });
                                        })
                                        .detach();
                                    })),
                            )
                            .child(
                                GpuiButton::new("remove-avatar-btn")
                                    .label(mezon_i18n::t(&locale, "common.removeAvatar"))
                                    .text_color(theme.text_muted)
                                    .border_1()
                                    .border_color(theme.border)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.avatar_local_preview = None;
                                        if let Some(state) = &mut this.profile {
                                            state.avatar_url = Some(
                                                AppConfig::global(cx).logo_mezon.clone().into(),
                                            );
                                        }
                                        this.refresh_banner_color(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child(mezon_i18n::t(&locale, "setting.profile.aboutMe")),
                    )
                    .child(
                        TextAreaField::new(
                            self.about_me_input
                                .as_ref()
                                .expect("about_me_input not initialized"),
                        )
                        .h(px(112.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_right()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(format!("{}/128", about_me.chars().count())),
                    ),
            )
            .child(
                div()
                    .relative()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg_primary)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text_muted)
                                    .child(mezon_i18n::t(
                                        &locale,
                                        "profileSetting.directMessageIcon",
                                    )),
                            )
                            .child(
                                div()
                                    .id("direct-message-icon-upload")
                                    .size(px(48.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.bg_secondary)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                            if this
                                                .profile
                                                .as_ref()
                                                .and_then(|profile| profile.logo_url.as_ref())
                                                .is_some()
                                            {
                                                this.dm_icon_menu_position = Some(event.position);
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .when_some(logo_url.clone(), |element, url| {
                                        element.child(
                                            img(crate::util::imgproxy::profile_url(cx, &url))
                                                .size_full()
                                                .rounded_md()
                                                .object_fit(gpui::ObjectFit::Cover),
                                        )
                                    })
                                    .when(
                                        self.profile
                                            .as_ref()
                                            .and_then(|profile| profile.logo_url.as_ref())
                                            .is_none(),
                                        |element| {
                                            element.child(
                                                Icon::new(IconName::Plus)
                                                    .size(px(22.))
                                                    .text_color(theme.text_muted),
                                            )
                                        },
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let root = cx.entity().clone();
                                        let locale = this.settings.read(cx).language.clone();
                                        let rx = cx.prompt_for_paths(PathPromptOptions {
                                            files: true,
                                            directories: false,
                                            multiple: false,
                                            prompt: Some(
                                                mezon_i18n::t(
                                                    &locale,
                                                    "setting.profile.chooseAvatar",
                                                )
                                                .into(),
                                            ),
                                        });
                                        cx.spawn(async move |_this, cx| {
                                            let Some(paths) =
                                                crate::util::file_dialog::resolve(rx, cx).await
                                            else {
                                                return;
                                            };
                                            let Some(path) = paths.into_iter().next() else {
                                                return;
                                            };
                                            root.update(cx, |_, cx| {
                                                if path
                                                    .metadata()
                                                    .map(|metadata| metadata.len())
                                                    .unwrap_or(u64::MAX)
                                                    > 1024 * 1024
                                                {
                                                    let title = mezon_i18n::t(
                                                        &locale,
                                                        "common.filesTooPowerful",
                                                    );
                                                    let content = mezon_i18n::t(
                                                        &locale,
                                                        "common.maxFileSize",
                                                    )
                                                    .replace("{{sizeLimit}}", "1 MB");
                                                    if let Some(handle) =
                                                        crate::app::main_window::handle(cx)
                                                    {
                                                        let _ = cx.update_window(
                                                            handle,
                                                            |_, window, cx| {
                                                                Shell::global(cx).update(
                                                                    cx,
                                                                    |shell, cx| {
                                                                        shell.show_upload_limit(
                                                                            title, content, window,
                                                                            cx,
                                                                        );
                                                                    },
                                                                );
                                                            },
                                                        );
                                                    }
                                                    return;
                                                }
                                                AccountStore::global(cx).update(cx, |store, cx| {
                                                    store.upload_direct_message_icon(&path, cx)
                                                })
                                            });
                                        })
                                        .detach();
                                    })),
                            ),
                    )
                    .when_some(dm_icon_menu, |element, menu| element.child(menu)),
            )
    }

    fn render_preview(
        &self,
        theme: &Theme,
        locale: &str,
        avatar_display: Option<SharedString>,
        avatar_local_preview: Option<std::path::PathBuf>,
    ) -> impl IntoElement {
        let display_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.display_name.clone());
        let username: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.username.clone());
        let status = self
            .profile
            .as_ref()
            .map_or("offline", |p| p.status.as_ref());
        let custom_status = self
            .profile
            .as_ref()
            .map_or_else(SharedString::default, |p| p.custom_status.clone());
        let (status_icon, status_color) = profile_status(status, theme);
        let banner_color = self
            .banner_color
            .map(gpui::Hsla::from)
            .unwrap_or(theme.tokens.bg_secondary.into());

        v_flex()
            .relative()
            .gap_2()
            .child(
                Label::new(mezon_i18n::t(locale, "common.preview"))
                    .text_sm()
                    .text_color(theme.text_muted)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                div()
                    .relative()
                    .h(px(320.))
                    .w_full()
                    .rounded_lg()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg_secondary)
                    // Color banner
                    .child(
                        div()
                            .h(px(132.0))
                            .w_full()
                            .rounded_tl_lg()
                            .rounded_tr_lg()
                            .bg(banner_color),
                    )
                    .child(
                        v_flex()
                            .absolute()
                            .left(px(20.))
                            .right(px(20.))
                            .bottom(px(20.))
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.bg_primary)
                            .child(
                                v_flex()
                                    .w(px(300.))
                                    .max_w_full()
                                    .min_w_0()
                                    .child(
                                        Label::new(display_name.clone())
                                            .w_full()
                                            .truncate()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.text_secondary),
                                    )
                                    .child(
                                        Label::new(username)
                                            .w_full()
                                            .truncate()
                                            .text_sm()
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(20.))
                            .top(px(86.))
                            .size(px(92.))
                            .rounded_full()
                            .bg(theme.bg_secondary)
                            .p(px(6.))
                            .child(if let Some(path) = avatar_local_preview {
                                div()
                                    .size(px(80.))
                                    .rounded_full()
                                    .overflow_hidden()
                                    .child(
                                        img(path)
                                            .size_full()
                                            .rounded_full()
                                            .object_fit(gpui::ObjectFit::Cover),
                                    )
                                    .into_any_element()
                            } else {
                                Avatar::new()
                                    .when_some(avatar_display, |avatar, url| avatar.src(url))
                                    .name(display_name)
                                    .size_px(px(80.))
                                    .image_cache(self.avatar_image_cache.clone())
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .absolute()
                                    .right(px(5.))
                                    .bottom(px(5.))
                                    .p(px(2.))
                                    .rounded_full()
                                    .bg(theme.bg_secondary)
                                    .child(
                                        Icon::new(status_icon)
                                            .size(px(15.))
                                            .text_color(status_color),
                                    ),
                            )
                            .when(!custom_status.is_empty(), |avatar| {
                                avatar.child(
                                    div()
                                        .absolute()
                                        .right(px(-20.))
                                        .top(px(28.))
                                        .size(px(14.))
                                        .rounded_full()
                                        .bg(theme.surfaces.secondary)
                                        .border_1()
                                        .border_color(theme.bg_secondary),
                                )
                            }),
                    ),
            )
            .when(!custom_status.is_empty(), |preview| {
                preview.child(
                    div()
                        .id("user-profile-preview-custom-status")
                        .group("user-profile-preview-custom-status")
                        .absolute()
                        .left(px(120.))
                        .top(px(168.))
                        .max_w(px(214.))
                        .on_hover({
                            let custom_status_bubble = self.custom_status_bubble.clone();
                            move |hovered: &bool, _window, cx| {
                                custom_status_bubble.update(cx, |bubble, cx| {
                                    bubble.set_expanded(*hovered, cx);
                                });
                            }
                        })
                        .child(self.custom_status_bubble.clone()),
                )
            })
    }
}

pub(super) use crate::util::user_status::status_icon_and_color as profile_status;
