use std::sync::Arc;

use gpui::{App, ClickEvent, Context, Entity, FontWeight, Window, div, prelude::*};
use mezon_client::{AppApi, MezonClient};
use mezon_store::AuthState;

use crate::chat_layout::ChatLayout;
use crate::components::primitives::{Button, Icon, IconName};
use crate::login_view::LoginView;
use crate::router::{Route, Router};
use crate::settings_screen::SettingsScreen;
use crate::theme::Theme;
use crate::title_bar::TitleBar;

pub struct RootView {
    title_bar: Entity<TitleBar>,
    auth_state: Entity<AuthState>,
    login_view: Entity<LoginView>,
    router: Router,
    chat_layout: Entity<ChatLayout>,
    settings_screen: SettingsScreen,
    navigate: crate::components::NavigateFn,
}

impl RootView {
    pub fn new(
        title_bar: Entity<TitleBar>,
        auth_state: Entity<AuthState>,
        client: Arc<MezonClient>,
        api: Arc<AppApi>,
        cx: &mut Context<Self>,
    ) -> Self {
        let login_view = cx.new({
            let auth_state = auth_state.clone();
            move |cx| LoginView::new(client, auth_state, cx)
        });

        let router = Router::new();
        let root_entity = cx.entity().clone();

        let navigate: crate::components::NavigateFn = {
            let root_id = root_entity.entity_id();
            Arc::new(move |path: &str, cx: &mut App| {
                root_entity.update(cx, |this, _cx| {
                    this.router.navigate(path);
                });
                cx.notify(root_id);
            })
        };

        let router_for_chat = router.clone();
        let chat_layout = cx.new(|cx| {
            ChatLayout::new(
                router_for_chat,
                auth_state.clone(),
                api.clone(),
                navigate.clone(),
                cx,
            )
        });

        let settings_screen = SettingsScreen::new(navigate.clone());

        Self {
            title_bar,
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
        let theme = Theme::dark();
        let state = self.auth_state.read(cx).clone();

        let content: gpui::AnyElement = match state {
            AuthState::NotAuthenticated | AuthState::OtpRequested { .. } => {
                self.login_view.clone().into_any_element()
            }
            AuthState::AwaitingCallback => render_awaiting_callback(&theme),
            AuthState::Authenticated(_) => {
                let route = self.router.route();
                match route {
                    Route::Settings => self.settings_screen.render(&theme).into_any_element(),
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

fn render_not_found(
    theme: &Theme,
    navigate: &crate::components::NavigateFn,
) -> gpui::AnyElement {
    let navigate = navigate.clone();
    let mut back_btn = Button::new("back-to-chat").label("Back to Chat");
    back_btn
        .interactivity()
        .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            navigate("/chat", cx);
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
