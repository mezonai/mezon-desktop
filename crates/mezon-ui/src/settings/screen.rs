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
use mezon_store::AuthState;

use super::account_page::AccountPage;
use super::device_page::DevicePage;
use super::profile_page::ProfilePage;
use crate::components::NavigateFn;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Account,
    Profile,
    Device,
}

pub struct SettingsScreen {
    navigate: NavigateFn,
    auth_state: Entity<AuthState>,
    api: Arc<AppApi>,
    current_page: SettingsPage,
    account_page: Option<Entity<AccountPage>>,
    profile_page: Option<Entity<ProfilePage>>,
    device_page: Option<Entity<DevicePage>>,
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
            current_page: SettingsPage::Account,
            account_page: None,
            profile_page: None,
            device_page: None,
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

        // Lazy init sub-page entities and refresh device on revisit
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
        }
        self.prev_page = page;

        let is_account = page == SettingsPage::Account;
        let is_profile = page == SettingsPage::Profile;
        let is_device = page == SettingsPage::Device;

        let content: gpui::AnyElement = match page {
            SettingsPage::Account => self.account_page.clone().unwrap().into_any_element(),
            SettingsPage::Profile => self.profile_page.clone().unwrap().into_any_element(),
            SettingsPage::Device => self.device_page.clone().unwrap().into_any_element(),
        };

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
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .px_2()
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.text_muted)
                                            .px_2()
                                            .py_1()
                                            .child("Account Setting"),
                                    )
                                    .child(
                                        div()
                                            .id("account-page")
                                            .flex()
                                            .items_center()
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .cursor_pointer()
                                            .when(is_account, |el| {
                                                el.bg(theme.bg_primary)
                                                    .text_color(theme.text_primary)
                                            })
                                            .when(!is_account, |el| {
                                                el.text_color(theme.text_primary)
                                            })
                                            .child("Account")
                                            .on_click({
                                                let nav = navigate.clone();
                                                move |_, _, cx| {
                                                    nav("/settings/account", cx);
                                                }
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("device-page")
                                            .flex()
                                            .items_center()
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .cursor_pointer()
                                            .when(is_device, |el| {
                                                el.bg(theme.bg_primary)
                                                    .text_color(theme.text_primary)
                                            })
                                            .when(!is_device, |el| {
                                                el.text_color(theme.text_primary)
                                            })
                                            .child("Devices")
                                            .on_click({
                                                let nav = navigate.clone();
                                                move |_, _, cx| {
                                                    nav("/settings/devices", cx);
                                                }
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("profile-page")
                                            .flex()
                                            .items_center()
                                            .w_full()
                                            .px_2()
                                            .py_1()
                                            .cursor_pointer()
                                            .when(is_profile, |el| {
                                                el.bg(theme.bg_primary)
                                                    .text_color(theme.text_primary)
                                            })
                                            .when(!is_profile, |el| {
                                                el.text_color(theme.text_primary)
                                            })
                                            .child("Profile")
                                            .on_click({
                                                let nav = navigate.clone();
                                                move |_, _, cx| {
                                                    nav("/settings/profile", cx);
                                                }
                                            }),
                                    ),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().h(px(1.0)).w_full().bg(theme.border))
                    .child(
                        div().px_3().py_2().child(
                            GpuiButton::new("logout-btn")
                                .label("Log Out")
                                .text_color(theme.text_primary)
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
