use std::sync::Arc;

use gpui::{div, prelude::*, Context, Entity, FontWeight, Window};
use mezon_client::{AppApi, MezonClient};
use mezon_store::AuthState;

use crate::account_test_view::AccountTestView;
use crate::login_view::LoginView;
use crate::theme::Theme;
use crate::title_bar::TitleBar;

/// RootView is the top-level GPUI view for the main window.
///
/// Owns the TitleBar and switches content area based on [`AuthState`]:
///   - `NotAuthenticated` / `OtpRequested` → `LoginView`
///   - `Authenticated`                     → (Stage 2: `MainLayout`)
pub struct RootView {
    title_bar: Entity<TitleBar>,
    auth_state: Entity<AuthState>,
    login_view: Entity<LoginView>,
    account_test_view: Entity<AccountTestView>,
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
        let account_test_view = cx.new({
            let api = api.clone();
            move |_cx| AccountTestView::new(api)
        });

        Self {
            title_bar,
            auth_state,
            login_view,
            account_test_view,
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
            AuthState::Authenticated(_) => self.account_test_view.clone().into_any_element(),
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
                .child("Connecting — complete sign-in in your browser..."),
        )
        .into_any_element()
}
