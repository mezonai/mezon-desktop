use crate::components::primitives::{Icon, IconName, h_flex, v_flex};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString, Window, div,
    prelude::*, px,
};
use mezon_store::{AuthState, ClanList, LoginStore, Settings};

use super::account_page::AccountPage;
use super::activity_page::ActivityPage;
use super::advanced_page::AdvancedPage;
use super::appearance_page::AppearancePage;
use super::device_page::DevicePage;
use super::language_page::LanguagePage;
use super::notifications_page::NotificationsPage;
use super::profile_page::ProfilePage;
use super::voice_page::VoicePage;
use crate::theme::{ActiveTheme, Theme};

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
    Advanced,
}

impl SettingsPage {
    fn i18n_key(self) -> &'static str {
        match self {
            Self::Account => "accountSetting.myAccount",
            Self::Profile => "setting.accountSettings.profiles",
            Self::Device => "setting.accountSettings.device",
            Self::Appearance => "setting.appSettings.appearance",
            Self::Activity => "setting.appSettings.activity",
            Self::Notifications => "setting.appSettings.notifications",
            Self::Language => "setting.language.title",
            Self::Voice => "setting.appSettings.voice",
            Self::Advanced => "setting.appSettings.advanced",
        }
    }
}

pub struct SettingsScreen {
    auth_state: Entity<AuthState>,
    settings: Entity<Settings>,
    clan_list: Entity<ClanList>,
    current_page: SettingsPage,
    account_page: Option<Entity<AccountPage>>,
    profile_page: Option<Entity<ProfilePage>>,
    device_page: Option<Entity<DevicePage>>,
    appearance_page: Option<Entity<AppearancePage>>,
    activity_page: Option<Entity<ActivityPage>>,
    notifications_page: Option<Entity<NotificationsPage>>,
    language_page: Option<Entity<LanguagePage>>,
    voice_page: Option<Entity<VoicePage>>,
    advanced_page: Option<Entity<AdvancedPage>>,
    prev_page: SettingsPage,
    scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    focus_on_show: bool,
}

impl SettingsScreen {
    pub fn new(
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        clan_list: Entity<ClanList>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let account_page = {
            let s = settings.clone();
            Some(cx.new(|cx| AccountPage::new(s, cx)))
        };
        Self {
            auth_state,
            settings,
            clan_list,
            current_page: SettingsPage::Account,
            account_page,
            profile_page: None,
            device_page: None,
            appearance_page: None,
            activity_page: None,
            notifications_page: None,
            language_page: None,
            voice_page: None,
            advanced_page: None,
            prev_page: SettingsPage::Account,
            scroll: ScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            focus_on_show: false,
        }
    }

    pub fn set_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.current_page = page;
        self.ensure_page(page, cx);
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.focus_on_show = true;
        cx.notify();
    }

    fn page_title(&self, page: SettingsPage, locale: &str) -> SharedString {
        mezon_i18n::t(locale, page.i18n_key()).into()
    }

    fn ensure_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        match page {
            SettingsPage::Account => {
                if self.account_page.is_none() {
                    let s = self.settings.clone();
                    self.account_page = Some(cx.new(|cx| AccountPage::new(s, cx)));
                }
            }
            SettingsPage::Profile => {
                if self.profile_page.is_none() {
                    let s = self.settings.clone();
                    let cl = self.clan_list.clone();
                    self.profile_page = Some(cx.new(|cx| ProfilePage::new(s, cl, cx)));
                }
            }
            SettingsPage::Device => {
                if self.device_page.is_none() {
                    let s = self.settings.clone();
                    let a = self.auth_state.clone();
                    self.device_page = Some(cx.new(|cx| DevicePage::new(s, a, cx)));
                }
            }
            SettingsPage::Appearance => {
                if self.appearance_page.is_none() {
                    let s = self.settings.clone();
                    self.appearance_page = Some(cx.new(|cx| AppearancePage::new(s, cx)));
                }
            }
            SettingsPage::Activity => {
                if self.activity_page.is_none() {
                    let s = self.settings.clone();
                    self.activity_page = Some(cx.new(|cx| ActivityPage::new(s, cx)));
                }
            }
            SettingsPage::Notifications => {
                if self.notifications_page.is_none() {
                    let s = self.settings.clone();
                    self.notifications_page = Some(cx.new(|cx| NotificationsPage::new(s, cx)));
                }
            }
            SettingsPage::Language => {
                if self.language_page.is_none() {
                    let s = self.settings.clone();
                    self.language_page = Some(cx.new(|cx| LanguagePage::new(s, cx)));
                }
            }
            SettingsPage::Voice => {
                if self.voice_page.is_none() {
                    let s = self.settings.clone();
                    self.voice_page = Some(cx.new(|cx| VoicePage::new(s, cx)));
                }
            }
            SettingsPage::Advanced => {
                if self.advanced_page.is_none() {
                    let s = self.settings.clone();
                    self.advanced_page = Some(cx.new(|cx| AdvancedPage::new(s, cx)));
                }
            }
        }
    }

    fn current_page_view(&self) -> Option<gpui::AnyElement> {
        match self.current_page {
            SettingsPage::Account => self
                .account_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Profile => self
                .profile_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Device => self
                .device_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Appearance => self
                .appearance_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Activity => self
                .activity_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Notifications => self
                .notifications_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Language => self
                .language_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Voice => self
                .voice_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
            SettingsPage::Advanced => self
                .advanced_page
                .as_ref()
                .map(|p| p.clone().into_any_element()),
        }
    }
}

impl Focusable for SettingsScreen {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_on_show {
            self.focus_on_show = false;
            window.focus(&self.focus_handle, cx);
        }

        const SETTINGS_CONTENT_WIDTH: f32 = 808.0;
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();
        let page = self.current_page;

        let just_switched_to_device =
            page == SettingsPage::Device && self.prev_page != SettingsPage::Device;
        if just_switched_to_device && let Some(d) = &self.device_page {
            d.update(cx, |d, view_cx| d.refresh(view_cx));
        }
        self.prev_page = page;

        let content = self
            .current_page_view()
            .unwrap_or_else(|| div().flex_1().into_any_element());

        let is_account = page == SettingsPage::Account;
        let is_profile = page == SettingsPage::Profile;
        let is_device = page == SettingsPage::Device;
        let is_appearance = page == SettingsPage::Appearance;
        let is_activity = page == SettingsPage::Activity;
        let is_notifications = page == SettingsPage::Notifications;
        let is_language = page == SettingsPage::Language;
        let is_voice = page == SettingsPage::Voice;
        let is_advanced = page == SettingsPage::Advanced;

        fn nav_item(
            id: &str,
            label: &'static str,
            is_active: bool,
            theme: &Theme,
            path: &str,
        ) -> impl IntoElement {
            let id = id.to_string();
            let path = path.to_string();
            div()
                .id(id)
                .flex()
                .items_center()
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .mb(px(4.0))
                .rounded(px(4.0))
                .text_base()
                .font_weight(gpui::FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover))
                .when(is_active, |el| {
                    el.bg(theme.bg_hover).text_color(theme.text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme.tokens.text_theme_primary)
                })
                .child(label)
                .on_click(move |_, _, cx| {
                    crate::router::replace(cx, crate::router::Route::from_path(&path));
                })
        }

        fn section_title(text: String, theme: &Theme) -> gpui::Div {
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_secondary)
                .child(text.to_uppercase())
        }

        let settings_title = mezon_i18n::t(&locale, "common.settings").to_uppercase();
        let mut nav = v_flex().w(px(220.0)).child(
            section_title(
                mezon_i18n::t(&locale, "setting.accountSettings.title").to_string(),
                &theme,
            )
            .mt(px(4.0)),
        );
        nav = nav
            .child(nav_item(
                "account-page",
                mezon_i18n::t(&locale, "setting.accountSettings.account"),
                is_account,
                &theme,
                "/settings/account",
            ))
            .child(nav_item(
                "device-page",
                mezon_i18n::t(&locale, "setting.accountSettings.devices"),
                is_device,
                &theme,
                "/settings/devices",
            ))
            .child(nav_item(
                "profile-page",
                mezon_i18n::t(&locale, "setting.accountSettings.profiles"),
                is_profile,
                &theme,
                "/settings/profile",
            ))
            .child(div().mt(px(4.0)).border_b_1().border_color(theme.border))
            .child(
                section_title(
                    mezon_i18n::t(&locale, "setting.appSettings.title").to_string(),
                    &theme,
                )
                .mt(px(4.0)),
            )
            .child(nav_item(
                "appearance-page",
                mezon_i18n::t(&locale, "setting.appSettings.appearance"),
                is_appearance,
                &theme,
                "/settings/appearance",
            ))
            .child(nav_item(
                "activity-page",
                mezon_i18n::t(&locale, "setting.appSettings.activity"),
                is_activity,
                &theme,
                "/settings/activity",
            ))
            .child(nav_item(
                "notifications-page",
                mezon_i18n::t(&locale, "setting.appSettings.notifications"),
                is_notifications,
                &theme,
                "/settings/notifications",
            ))
            .child(nav_item(
                "language-page",
                mezon_i18n::t(&locale, "setting.language.title"),
                is_language,
                &theme,
                "/settings/language",
            ))
            .child(nav_item(
                "voice-page",
                mezon_i18n::t(&locale, "setting.appSettings.voice"),
                is_voice,
                &theme,
                "/settings/voice",
            ))
            .child(nav_item(
                "advanced-page",
                mezon_i18n::t(&locale, "setting.appSettings.advanced"),
                is_advanced,
                &theme,
                "/settings/advanced",
            ))
            .child(div().mt(px(4.0)).border_b_1().border_color(theme.border))
            .child(
                div()
                    .id("logout-btn")
                    .mt(px(4.0))
                    .w_full()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(gpui::rgb(0xef4444))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(mezon_i18n::t(&locale, "setting.logOut"))
                    .on_click(move |_, _, cx| {
                        LoginStore::global(cx).update(cx, |store, cx| store.logout(cx));
                    }),
            )
            .child(
                div()
                    .id("quit-app-btn")
                    .mt(px(4.0))
                    .w_full()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_base()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(gpui::rgb(0xef4444))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .child(mezon_i18n::t(&locale, "setting.quit"))
                    .on_click(move |_, _, cx| {
                        cx.quit();
                    }),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            );

        h_flex()
            .id("settings-screen")
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(|_, _: &::menu::Cancel, _window, cx| {
                crate::router::go_back(cx);
            }))
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .bg(theme.tokens.theme_setting_primary)
            .child(
                v_flex()
                    .id("settings-nav")
                    .flex_shrink_0()
                    .w(gpui::relative(0.25))
                    .min_w(px(220.0))
                    .h_full()
                    .min_h_0()
                    .bg(theme.tokens.theme_setting_nav)
                    .child(
                        div()
                            .flex_shrink_0()
                            .w_full()
                            .pt(px(80.0))
                            .pr(px(20.0))
                            .pl(px(20.0))
                            .pb(px(6.0))
                            .child(
                                div().flex().flex_row().justify_end().w_full().child(
                                    div()
                                        .w(px(220.0))
                                        .pl(px(10.0))
                                        .text_base()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(theme.text_primary)
                                        .child(settings_title),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-nav-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.nav_scroll)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .w_full()
                                    .pb(px(60.0))
                                    .pr(px(20.0))
                                    .pl(px(20.0))
                                    .child(nav),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .justify_start()
                    .bg(theme.tokens.theme_setting_primary)
                    .child(
                        h_flex()
                            .h_full()
                            .min_h_0()
                            .flex_shrink_0()
                            .items_start()
                            .child(
                                v_flex()
                                    .h_full()
                                    .min_h_0()
                                    .w(px(SETTINGS_CONTENT_WIDTH))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .w_full()
                                            .pl(px(40.0))
                                            .pr(px(28.0))
                                            .pt(px(60.0))
                                            .child(
                                                div()
                                                    .max_w(px(740.0))
                                                    .text_xl()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .mb_5()
                                                    .text_color(theme.text_primary)
                                                    .child(self.page_title(page, &locale)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("settings-scroll")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.scroll)
                                            .pb(px(28.0))
                                            .pl(px(40.0))
                                            .pr(px(28.0))
                                            .text_sm()
                                            .child(div().max_w(px(740.0)).child(content)),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-close-btn")
                                    .flex_shrink_0()
                                    .pt(px(94.0))
                                    .pl(px(20.0))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .p(px(10.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_secondary)
                                            .child(
                                                Icon::new(IconName::Close)
                                                    .size(px(18.0))
                                                    .text_color(theme.text_secondary),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.text_secondary)
                                            .child("ESC"),
                                    )
                                    .on_click(move |_, _, cx| {
                                        crate::router::go_back(cx);
                                    }),
                            ),
                    ),
            )
    }
}
