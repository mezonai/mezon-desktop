use std::time::Duration;

use crate::components::primitives::{
    Avatar, Button as GpuiButton, ButtonVariants, Input, InputEvent, InputState, Label, Sizable,
    Size, h_flex, v_flex,
};
use gpui::{
    Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use mezon_store::{AccountEvent, AccountStore, ClanList, Settings, UserAccount};

use super::clan_profile_section::ClanProfileSection;
use crate::theme::{ActiveTheme, Theme};

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
    original_display_name: SharedString,
    original_about_me: SharedString,
    original_avatar_url: Option<SharedString>,
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
            original_display_name: display_name,
            original_about_me: about_me,
            original_avatar_url: avatar_url,
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
    about_me_input: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
    fetch_error: bool,
    account_loaded: bool,
    clan_section: Option<Entity<ClanProfileSection>>,
    toast_message: Option<SharedString>,
    #[allow(dead_code)]
    show_delete_confirm: bool,
}

impl ProfilePage {
    pub fn new(
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&clan_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&AccountStore::global(cx), |this, store, cx| {
            if !this.account_loaded
                && let Some(account) = store.read(cx).account.as_ref()
            {
                this.account_loaded = true;
                this.profile = Some(ProfileState::from_account(account));
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(
            &AccountStore::global(cx),
            |this, _, event, cx| match event {
                AccountEvent::AccountLoadFailed => {
                    this.fetch_error = true;
                    cx.notify();
                }
                AccountEvent::AccountSaved => {
                    if let Some(state) = &mut this.profile {
                        state.original_display_name = state.display_name.clone();
                        state.original_about_me = state.about_me.clone();
                        state.original_avatar_url = state.avatar_url.clone();
                        state.saving = false;
                    }
                    this.show_toast("Profile saved", cx);
                }
                AccountEvent::AccountSaveFailed(msg) => {
                    if let Some(state) = &mut this.profile {
                        state.saving = false;
                    }
                    this.show_toast(format!("Failed to save: {}", msg), cx);
                }
                AccountEvent::AvatarUploaded(url) => {
                    if let Some(state) = &mut this.profile {
                        state.avatar_url = Some(url.clone().into());
                    }
                    cx.notify();
                }
                AccountEvent::AvatarUploadFailed(msg) => {
                    this.show_toast(format!("Failed to upload avatar: {}", msg), cx);
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

        Self {
            settings,
            clan_list,
            active_tab: ProfileTab::User,
            profile,
            display_name_input: None,
            about_me_input: None,
            _subscriptions: Vec::new(),
            fetch_error: false,
            account_loaded,
            clan_section: None,
            toast_message: None,
            show_delete_confirm: false,
        }
    }

    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_message = Some(message.into());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.toast_message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn init_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let locale = self.settings.read(cx).language.clone();
        let display_ph = mezon_i18n::t(&locale, "setting.profile.displayNamePlaceholder");
        let about_ph = mezon_i18n::t(&locale, "setting.profile.aboutPlaceholder");
        let display = cx.new(|cx| InputState::new(window, cx).placeholder(display_ph));
        let about = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder(about_ph)
        });

        if let Some(state) = &self.profile {
            display.update(cx, |input, cx| {
                input.set_value(&state.display_name, window, cx);
            });
            about.update(cx, |input, cx| {
                input.set_value(&state.about_me, window, cx);
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
            move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event
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
            )
        });

        if let (Some((display_name, about_me, avatar_url)), Some(state)) =
            (original.clone(), &mut self.profile)
        {
            state.display_name = display_name;
            state.about_me = about_me;
            state.avatar_url = avatar_url;
        }

        if let Some((display_name, about_me, _)) = original {
            if let Some(input) = &self.display_name_input {
                input.update(cx, |input_state: &mut InputState, input_cx| {
                    input_state.set_value(display_name.clone(), window, input_cx);
                });
            }
            if let Some(input) = &self.about_me_input {
                input.update(cx, |input_state: &mut InputState, input_cx| {
                    input_state.set_value(about_me, window, input_cx);
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

        AccountStore::global(cx).update(cx, |store, cx| {
            store.save_account(display_name, avatar_url, about_me, cx);
        });
    }

    fn render_user_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();
        let locale = self.settings.read(cx).language.clone();
        let avatar_display = self
            .profile
            .as_ref()
            .and_then(|p| p.avatar_url.as_ref())
            .map(|url| SharedString::from(crate::util::imgproxy::profile_url(cx, url.as_ref())));
        let form = self.render_form(theme, cx, avatar_display.clone());
        let preview = self.render_preview(theme, &locale, avatar_display);
        let is_dirty = self.is_dirty();
        let saving = self.profile.as_ref().map(|p| p.saving).unwrap_or(false);

        v_flex()
            .gap_6()
            .child(h_flex().gap_8().child(form).child(preview))
            .when(is_dirty || saving, |el| {
                el.child(
                    v_flex()
                        .gap_3()
                        .pt_4()
                        .child(h_flex().gap_2().items_center().child(
                            div().text_sm().text_color(theme.status_dnd).child(format!(
                                "⚠ {}",
                                mezon_i18n::t(&locale, "setting.profile.unsavedWarning")
                            )),
                        ))
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    GpuiButton::new("user-save-btn")
                                        .label(if saving {
                                            mezon_i18n::t(&locale, "setting.profile.saving")
                                        } else {
                                            mezon_i18n::t(&locale, "setting.profile.saveChanges")
                                        })
                                        .disabled(saving)
                                        .text_color(theme.text_secondary)
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, _, cx| {
                                                e.update(cx, |this, view_cx| {
                                                    this.save(view_cx);
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    GpuiButton::new("user-discard-btn")
                                        .label(mezon_i18n::t(&locale, "common.discard"))
                                        .disabled(saving)
                                        .text_color(theme.text_primary)
                                        .ghost()
                                        .on_click({
                                            let e = entity.clone();
                                            move |_, window, cx| {
                                                e.update(cx, |this, view_cx| {
                                                    this.discard_changes(window, view_cx);
                                                    view_cx.notify();
                                                });
                                            }
                                        }),
                                ),
                        ),
                )
            })
            .when_some(self.toast_message.clone(), |this, msg| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .bg(theme.bg_floating)
                        .rounded_md()
                        .text_sm()
                        .text_color(theme.text_primary)
                        .child(msg),
                )
            })
    }
}

impl Render for ProfilePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_tab = ProfileTab::User;
                        cx.notify();
                    })),
            )
            .child(
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
                    .on_click(cx.listener(|this, _, _, cx| {
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
                        let active_clan_id = this.clan_list.read(cx).active_clan().map(|c| c.id);
                        this.clan_section.get_or_insert_with(|| {
                            cx.new(|cx| {
                                ClanProfileSection::new(
                                    this.settings.clone(),
                                    this.clan_list.clone(),
                                    cx,
                                )
                            })
                        });
                        if let Some(section) = &this.clan_section {
                            section.update(cx, |s, cx| {
                                s.set_user_profile(display_name, username);
                                if let Some(ref clan_id) = active_clan_id {
                                    s.fetch(&clan_id.to_string(), cx);
                                }
                            });
                        }
                        cx.notify();
                    })),
            );

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

        v_flex()
            .gap_6()
            .child(tabs)
            .child(body)
            // Delete Account button (only for user profile tab)
            .when(!is_clan, |el| {
                el.child(
                    GpuiButton::new("delete-account-btn")
                        .label(mezon_i18n::t(&locale, "setting.profile.deleteAccount"))
                        .text_color(theme.status_dnd)
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            let locale = this.settings.read(cx).language.clone();
                            this.show_toast(
                                mezon_i18n::t(&locale, "setting.profile.deleteComingSoon"),
                                cx,
                            );
                        })),
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
        avatar_display: Option<SharedString>,
    ) -> impl IntoElement {
        let locale = self.settings.read(cx).language.clone();
        let display_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.display_name.clone());
        let about_me: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.about_me.clone());

        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Avatar::new()
                            .when_some(avatar_display.clone(), |av, url| av.src(url))
                            .name(display_name.clone())
                            .with_size(Size::Large),
                    )
                    .child(
                        GpuiButton::new("change-avatar-btn")
                            .label(mezon_i18n::t(&locale, "common.changeAvatar"))
                            .text_color(theme.text_primary)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let locale = this.settings.read(cx).language.clone();
                                let root_entity = cx.entity().clone();
                                let rx = cx.prompt_for_paths(PathPromptOptions {
                                    files: true,
                                    directories: false,
                                    multiple: false,
                                    prompt: Some(
                                        mezon_i18n::t(&locale, "setting.profile.chooseAvatar")
                                            .into(),
                                    ),
                                });
                                cx.spawn(async move |_this, cx| {
                                    let paths = match rx.await {
                                        Ok(Ok(Some(p))) => p,
                                        _ => return,
                                    };
                                    let path = match paths.into_iter().next() {
                                        Some(p) => p,
                                        None => return,
                                    };
                                    root_entity.update(cx, |_, cx| {
                                        AccountStore::global(cx).update(cx, |store, cx| {
                                            store.upload_avatar(&path, cx);
                                        });
                                    });
                                })
                                .detach();
                            })),
                    )
                    .child(
                        GpuiButton::new("remove-avatar-btn")
                            .label(mezon_i18n::t(&locale, "common.removeAvatar"))
                            .text_color(theme.text_muted)
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(state) = &mut this.profile {
                                    state.avatar_url = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
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
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_primary)
                            .child(mezon_i18n::t(&locale, "setting.profile.aboutMe")),
                    )
                    .child(
                        Input::new(
                            self.about_me_input
                                .as_ref()
                                .expect("about_me_input not initialized"),
                        )
                        .w_full()
                        .h(px(100.0)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(format!("{}/128", about_me.len())),
                    ),
            )
    }

    fn render_preview(
        &self,
        theme: &Theme,
        locale: &str,
        avatar_display: Option<SharedString>,
    ) -> impl IntoElement {
        let display_name: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.display_name.clone());
        let about_me: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.about_me.clone());
        let username: SharedString = self
            .profile
            .as_ref()
            .map_or("".into(), |p| p.username.clone());

        v_flex()
            .gap_4()
            .child(
                Label::new(mezon_i18n::t(locale, "common.preview"))
                    .text_xl()
                    .text_color(theme.text_primary)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                v_flex()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(theme.bg_primary)
                    // Color banner
                    .child(div().h(px(105.0)).w_full().bg(theme.brand))
                    .child(
                        v_flex()
                            .gap_4()
                            .px_6()
                            .py_6()
                            .child(
                                h_flex()
                                    .gap_4()
                                    .child(
                                        Avatar::new()
                                            .when_some(avatar_display.clone(), |av, url| {
                                                av.src(url)
                                            })
                                            .name(display_name.clone())
                                            .with_size(Size::Large),
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                Label::new(display_name.clone())
                                                    .text_xl()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme.text_primary),
                                            )
                                            .child(
                                                Label::new(format!("@{}", username))
                                                    .text_sm()
                                                    .text_color(theme.text_muted),
                                            ),
                                    ),
                            )
                            .when(!about_me.is_empty(), |el| {
                                el.child(
                                    Label::new(about_me.clone())
                                        .text_sm()
                                        .text_color(theme.text_muted),
                                )
                            }),
                    ),
            )
    }
}
