use std::sync::Arc;

use gpui::{App, ClickEvent, Context, Entity, FontWeight, Window, div, prelude::*};
use mezon_client::{AppApi, MezonClient};
use mezon_store::{AuthState, Settings};

use crate::chat_layout::ChatLayout;
use crate::components::primitives::{Button, Icon, IconName};
use crate::login_view::LoginView;
use crate::router::{Route, Router};
use crate::settings::SettingsScreen;
use crate::theme::{Theme, resolve_theme};
use crate::title_bar::TitleBar;

pub struct RootView {
    title_bar: Entity<TitleBar>,
    settings: Entity<Settings>,
    auth_state: Entity<AuthState>,
    login_view: Entity<LoginView>,
    router: Router,
    chat_layout: Entity<ChatLayout>,
    settings_screen: Entity<SettingsScreen>,
    navigate: crate::components::NavigateFn,
}

impl RootView {
    pub fn new(
        title_bar: Entity<TitleBar>,
        auth_state: Entity<AuthState>,
        client: Arc<MezonClient>,
        api: Arc<AppApi>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx.observe(&settings, |_, _, cx| cx.notify());

        let login_view = cx.new({
            let auth_state = auth_state.clone();
            let settings = settings.clone();
            move |cx| LoginView::new(client, auth_state, settings, cx)
        });

        let router = Router::new();
        let root_entity = cx.entity().clone();

        let navigate: crate::components::NavigateFn = {
            let root_id = root_entity.entity_id();
            Arc::new(move |op: crate::components::NavOp, cx: &mut App| {
                root_entity.update(cx, |this, _cx| {
                    match op {
                        crate::components::NavOp::Push(path) => this.router.navigate(path),
                        crate::components::NavOp::Replace(path) => this.router.replace(path),
                        crate::components::NavOp::Back => this.router.go_back(),
                    }
                });
                cx.notify(root_id);
            })
        };

        let router_for_chat = router.clone();
        let auth_state_for_chat = auth_state.clone();
        let api_for_chat = api.clone();
        let navigate_for_chat = navigate.clone();
        let settings_for_chat = settings.clone();
        let chat_layout = cx.new({
            let settings = settings_for_chat;
            move |cx| {
                ChatLayout::new(
                    router_for_chat,
                    auth_state_for_chat.clone(),
                    api_for_chat.clone(),
                    navigate_for_chat.clone(),
                    settings.clone(),
                    cx,
                )
            }
        });

        let auth_state_for_settings = auth_state.clone();
        let api_for_settings = api.clone();
        let navigate_for_settings = navigate.clone();
        let settings_screen = cx.new({
            let settings = settings.clone();
            move |cx| {
                SettingsScreen::new(
                    auth_state_for_settings.clone(),
                    api_for_settings.clone(),
                    navigate_for_settings.clone(),
                    settings.clone(),
                    cx,
                )
            }
        });

        Self {
            title_bar,
            settings,
            auth_state,
            login_view,
            router,
            chat_layout,
            settings_screen,
            navigate,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = resolve_theme(&self.settings.read(cx).theme);
        let state = self.auth_state.read(cx).clone();

        let content: gpui::AnyElement = match state {
            AuthState::NotAuthenticated | AuthState::OtpRequested { .. } => {
                self.login_view.clone().into_any_element()
            }
            AuthState::AwaitingCallback => render_awaiting_callback(&theme),
            AuthState::Authenticated(_) => {
                let route = self.router.route();
                match route {
                    Route::SettingsAccount
                    | Route::SettingsProfile
                    | Route::SettingsDevices
                    | Route::SettingsAppearance
                    | Route::SettingsActivity
                    | Route::SettingsNotifications
                    | Route::SettingsLanguage
                    | Route::SettingsVoice
                    | Route::SettingsAdvanced => {
                        let page = match route {
                            Route::SettingsProfile => crate::settings::SettingsPage::Profile,
                            Route::SettingsDevices => crate::settings::SettingsPage::Device,
                            Route::SettingsAppearance => crate::settings::SettingsPage::Appearance,
                            Route::SettingsActivity => crate::settings::SettingsPage::Activity,
                            Route::SettingsNotifications => {
                                crate::settings::SettingsPage::Notifications
                            }
                            Route::SettingsLanguage => crate::settings::SettingsPage::Language,
                            Route::SettingsVoice => crate::settings::SettingsPage::Voice,
                            Route::SettingsAdvanced => crate::settings::SettingsPage::Advanced,
                            _ => crate::settings::SettingsPage::Account,
                        };
                        self.settings_screen.update(cx, |s, _| {
                            s.set_page(page);
                        });
                        self.settings_screen.clone().into_any_element()
                    }
                    Route::NotFound { .. } => render_not_found(&theme, &self.navigate),
                    _ => self.chat_layout.clone().into_any_element(),
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_primary)
            .text_color(theme.text_primary)
            .child(self.title_bar.clone())
            .child(content)
    }
}

fn render_awaiting_callback(theme: &Theme) -> gpui::AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .flex_col()
        .gap_4()
        .child(div().size_16().bg(theme.brand).rounded_lg())
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child("Mezon"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child("Connecting - complete sign-in in your browser..."),
        )
        .into_any_element()
}

fn render_not_found(theme: &Theme, navigate: &crate::components::NavigateFn) -> gpui::AnyElement {
    let navigate = navigate.clone();
    let mut back_btn = Button::new("back-to-chat").label("Back to Chat");
    back_btn
        .interactivity()
        .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            navigate(crate::components::NavOp::Back, cx);
        });

    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .flex_col()
        .gap_4()
        .child(
            Icon::new(IconName::TriangleAlert)
                .size_8()
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child("Page Not Found"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child("This path is not registered in the local Mezon router."),
        )
        .child(back_btn)
        .into_any_element()
}
