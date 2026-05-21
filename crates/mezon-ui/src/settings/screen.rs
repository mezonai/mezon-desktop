use std::sync::Arc;

use gpui::{Context, Entity, Window, div, prelude::*, px};
use gpui_component::{
    Icon, IconName,
    button::{Button as GpuiButton, ButtonVariants},
    h_flex,
    label::Label,
    scroll::ScrollableElement,
    v_flex,
};
use mezon_client::AppApi;
use mezon_store::{AuthState, Settings};

use super::account_page::AccountPage;
use super::activity_page::ActivityPage;
use super::appearance_page::AppearancePage;
use super::device_page::DevicePage;
use super::language_page::LanguagePage;
use super::notifications_page::NotificationsPage;
use super::profile_page::ProfilePage;
use super::voice_page::VoicePage;
use crate::components::NavigateFn;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Account,
    Profile,
    Device,
    Appearance,
    Activity,
    Notifications,
    Language,
    Voice,
}

pub struct SettingsScreen {
    navigate: NavigateFn,
    auth_state: Entity<AuthState>,
    api: Arc<AppApi>,
    settings: Entity<Settings>,
    current_page: SettingsPage,
    account_page: Option<Entity<AccountPage>>,
    profile_page: Option<Entity<ProfilePage>>,
    device_page: Option<Entity<DevicePage>>,
    appearance_page: Option<Entity<AppearancePage>>,
    activity_page: Option<Entity<ActivityPage>>,
    notifications_page: Option<Entity<NotificationsPage>>,
    language_page: Option<Entity<LanguagePage>>,
    voice_page: Option<Entity<VoicePage>>,
    prev_page: SettingsPage,
}

impl SettingsScreen {
    pub fn new(
        auth_state: Entity<AuthState>,
        api: Arc<AppApi>,
        navigate: NavigateFn,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            navigate,
            auth_state,
            api,
            settings: _cx.new(|_| Settings::load_sync()),
            current_page: SettingsPage::Account,
            account_page: None,
            profile_page: None,
            device_page: None,
            appearance_page: None,
            activity_page: None,
            notifications_page: None,
            language_page: None,
            voice_page: None,
            prev_page: SettingsPage::Account,
        }
    }

    pub fn set_page(&mut self, page: SettingsPage) {
        self.current_page = page;
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let navigate = self.navigate.clone();
        let api = self.api.clone();
        let auth_state = self.auth_state.clone();
        let page = self.current_page;

        // Lazy init sub-page entities, refresh device on revisit
        match page {
            SettingsPage::Account => {
                self.account_page
                    .get_or_insert_with(|| cx.new(|cx| AccountPage::new(api.clone(), navigate.clone(), cx)));
            }
            SettingsPage::Profile => {
                self.profile_page
                    .get_or_insert_with(|| cx.new(|cx| ProfilePage::new(api.clone(), cx)));
            }
            SettingsPage::Device => {
                let just_switched = self.prev_page != SettingsPage::Device;
                if self.device_page.is_none() {
                    self.device_page = Some(cx.new(|cx| DevicePage::new(api.clone(), cx)));
                } else if just_switched && let Some(device_entity) = &self.device_page {
                    device_entity.update(cx, |d, view_cx| d.refresh(view_cx));
                }
            }
            SettingsPage::Appearance => {
                self.appearance_page
                    .get_or_insert_with(|| cx.new(|_| AppearancePage));
            }
            SettingsPage::Activity => {
                self.activity_page.get_or_insert_with(|| {
                    let settings = self.settings.clone();
                    cx.new(|cx| ActivityPage::new(settings, cx))
                });
            }
            SettingsPage::Notifications => {
                self.notifications_page.get_or_insert_with(|| {
                    let settings = self.settings.clone();
                    cx.new(|cx| NotificationsPage::new(settings, cx))
                });
            }
            SettingsPage::Language => {
                self.language_page
                    .get_or_insert_with(|| cx.new(|_| LanguagePage));
            }
            SettingsPage::Voice => {
                self.voice_page
                    .get_or_insert_with(|| cx.new(|_| VoicePage));
            }
        }
        self.prev_page = page;

        let is_account = page == SettingsPage::Account;
        let is_profile = page == SettingsPage::Profile;
        let is_device = page == SettingsPage::Device;
        let is_appearance = page == SettingsPage::Appearance;
        let is_activity = page == SettingsPage::Activity;
        let is_notifications = page == SettingsPage::Notifications;
        let is_language = page == SettingsPage::Language;
        let is_voice = page == SettingsPage::Voice;

        let content: gpui::AnyElement = match page {
            SettingsPage::Account => self.account_page.clone().unwrap().into_any_element(),
            SettingsPage::Profile => self.profile_page.clone().unwrap().into_any_element(),
            SettingsPage::Device => self.device_page.clone().unwrap().into_any_element(),
            SettingsPage::Appearance => self.appearance_page.clone().unwrap().into_any_element(),
            SettingsPage::Activity => self.activity_page.clone().unwrap().into_any_element(),
            SettingsPage::Notifications => self.notifications_page.clone().unwrap().into_any_element(),
            SettingsPage::Language => self.language_page.clone().unwrap().into_any_element(),
            SettingsPage::Voice => self.voice_page.clone().unwrap().into_any_element(),
        };

        fn nav_item(
            id: &str,
            label: &str,
            is_active: bool,
            theme: &Theme,
            navigate: NavigateFn,
            path: &str,
        ) -> impl IntoElement {
            let id = id.to_string();
            let nav = navigate.clone();
            let path = path.to_string();
            div()
                .id(id)
                .flex()
                .items_center()
                .w_full()
                .px_2()
                .py_1()
                .cursor_pointer()
                .when(is_active, |el| {
                    el.bg(theme.bg_primary).text_color(theme.text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme.text_primary)
                })
                .child(label.to_string())
                .on_click(move |_, _, cx| {
                    nav(&path, cx);
                })
        }

        h_flex()
            .flex_1()
            .size_full()
            .bg(theme.bg_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(250.0))
                    .h_full()
                    .bg(theme.bg_secondary)
                    .border_r_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_3()
                            .child(Label::new("Settings").text_color(theme.text_primary)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_2()
                            .child(
                                v_flex()
                                    .gap_1()
                                    // ACCOUNT SETTINGS section
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_muted)
                                            .px_2()
                                            .py_1()
                                            .child("ACCOUNT SETTINGS"),
                                    )
                                    .child(nav_item("account-page", "Account", is_account, &theme, navigate.clone(), "/settings/account"))
                                    .child(nav_item("device-page", "Devices", is_device, &theme, navigate.clone(), "/settings/devices"))
                                    .child(nav_item("profile-page", "Profiles", is_profile, &theme, navigate.clone(), "/settings/profile"))
                                    // APP SETTINGS section
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_muted)
                                            .px_2()
                                            .py_1()
                                            .mt_4()
                                            .child("APP SETTINGS"),
                                    )
                                    .child(nav_item("appearance-page", "Appearance", is_appearance, &theme, navigate.clone(), "/settings/appearance"))
                                    .child(nav_item("activity-page", "Activity", is_activity, &theme, navigate.clone(), "/settings/activity"))
                                    .child(nav_item("notifications-page", "Notifications", is_notifications, &theme, navigate.clone(), "/settings/notifications"))
                                    .child(nav_item("language-page", "Language", is_language, &theme, navigate.clone(), "/settings/language"))
                                    .child(nav_item("voice-page", "Voice", is_voice, &theme, navigate.clone(), "/settings/voice")),
                            ),
                    )
                    .child(div().h(px(1.0)).w_full().bg(theme.border))
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .child(
                                v_flex()
                                    .child(
                                        GpuiButton::new("logout-btn")
                                            .label("Log Out")
                                            .text_color(theme.status_dnd)
                                            .ghost()
                                            .w_full()
                                            .on_click({
                                                let api = api.clone();
                                                let auth_state = auth_state.clone();
                                                move |_, _, cx| {
                                                    let auth = auth_state.read(cx);
                                                    if let AuthState::Authenticated(session) = auth {
                                                        let api = api.clone();
                                                        let token = session.token.clone();
                                                        let refresh_token = session.refresh_token.clone();
                                                        let auth_state = auth_state.clone();
                                                        cx.spawn(async move |cx| {
                                                            let _ = api
                                                                .session_logout(&token, &refresh_token)
                                                                .await;
                                                            cx.update(|cx| {
                                                                auth_state.update(cx, |state, _| {
                                                                    *state = AuthState::NotAuthenticated;
                                                                });
                                                            });
                                                        })
                                                        .detach();
                                                    }
                                                }
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.text_muted)
                                            .px_2()
                                            .child(env!("CARGO_PKG_VERSION")),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_6()
                    .bg(theme.bg_secondary)
                    .child(content),
            )
            .child(
                div()
                    .id("settings-close-btn")
                    .absolute()
                    .top_4()
                    .right_4()
                    .cursor_pointer()
                    .child(
                        Icon::new(IconName::Close)
                            .size_6()
                            .text_color(theme.text_secondary),
                    )
                    .on_click({
                        let nav = navigate.clone();
                        move |_, _, cx| {
                            nav("/chat", cx);
                        }
                    }),
            )
    }
}
