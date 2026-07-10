use crate::components::primitives::Sizable;
use gpui::{
    AnyView, App, ClickEvent, Context, Entity, FontWeight, MouseButton, NavigationDirection,
    StyleRefinement, Window, div, img, prelude::*, px,
};
use mezon_store::{AuthState, ChannelList, ClanId, ClanList, ConnectionStore, Settings};
use ui::utils::ROUNDED_BORDER_WINDOW;

use crate::app::title_bar::TitleBar;
use crate::app::window_controls;
use crate::auth::login_view::LoginView;
use crate::chat::layout::ChatLayout;
use crate::clan::settings::{ClanSettingScreen, ClanSettingsPage};
use crate::components::primitives::{Button, Icon, IconName, Size, Spinner};
use crate::image_cache::{
    LruImageCache, SHARED_ENTRY_MAX_BYTES, SHARED_IMAGE_CACHE_BYTES, SHARED_IMAGE_CACHE_CAPACITY,
};
use crate::router::{Route, Router};
use crate::settings::SettingsScreen;
use crate::theme::{ActiveTheme, Theme, resolve_theme};

pub struct RootView {
    title_bar: Entity<TitleBar>,
    auth_state: Entity<AuthState>,
    login_view: Entity<LoginView>,
    chat_layout: Entity<ChatLayout>,
    settings_screen: Entity<SettingsScreen>,
    clan_setting_screen: Entity<ClanSettingScreen>,
    settings: Entity<Settings>,
    applied_theme: String,
    image_cache: Entity<LruImageCache>,
}

impl RootView {
    pub fn new(
        title_bar: Entity<TitleBar>,
        auth_state: Entity<AuthState>,
        settings: Entity<Settings>,
        cx: &mut Context<Self>,
    ) -> Self {
        // App shell: owns the cross-cutting overlay layers (toasts + modal). Init before child
        // views so any of them can surface a toast/modal via `Shell::global`.
        let shell = crate::app::shell::Shell::init(cx);
        cx.observe(&shell, |_, _, cx| cx.notify()).detach();

        cx.observe(&settings, |this, settings, cx| {
            let name = settings.read(cx).theme.clone();
            if name != this.applied_theme {
                crate::theme::set_theme(resolve_theme(&name), cx);
                this.applied_theme = name;
                cx.notify();
            }
        })
        .detach();

        let login_view = cx.new({
            let auth_state = auth_state.clone();
            let settings = settings.clone();
            move |cx| LoginView::new(auth_state, settings, cx)
        });

        cx.observe(&Router::global(cx), |this, _router, cx| {
            this.sync_settings_page(cx);
            this.sync_clan_settings_page(cx);
            cx.notify();
        })
        .detach();

        cx.observe(&auth_state, |_, auth_state, cx| {
            if matches!(*auth_state.read(cx), AuthState::NotAuthenticated) {
                crate::image_cache::clear_shared_avatar_cache(cx);
            }
            cx.notify();
        })
        .detach();

        cx.observe(&ConnectionStore::global(cx), |_, _, cx| cx.notify())
            .detach();

        let clan_list: Entity<ClanList> = ClanList::global(cx);

        let clan_list_for_chat = clan_list.clone();
        let auth_state_for_chat = auth_state.clone();
        let settings_for_chat = settings.clone();
        let chat_layout = cx.new({
            let settings = settings_for_chat;
            move |cx| {
                ChatLayout::new(
                    clan_list_for_chat.clone(),
                    auth_state_for_chat.clone(),
                    settings.clone(),
                    cx,
                )
            }
        });

        let auth_state_for_settings = auth_state.clone();
        let clan_list_for_settings = clan_list.clone();
        let settings_screen = cx.new({
            let settings = settings.clone();
            move |cx| {
                SettingsScreen::new(
                    auth_state_for_settings.clone(),
                    settings.clone(),
                    clan_list_for_settings.clone(),
                    cx,
                )
            }
        });

        let channel_list = ChannelList::global(cx);
        let channel_list_for_clan_settings = channel_list.clone();
        let clan_list_for_clan_settings = clan_list.clone();
        let settings_for_clan_settings = settings.clone();
        let clan_setting_screen = cx.new({
            let settings = settings_for_clan_settings;
            move |cx| {
                ClanSettingScreen::new(
                    ClanId(0),
                    ClanSettingsPage::Overview,
                    settings.clone(),
                    clan_list_for_clan_settings.clone(),
                    channel_list_for_clan_settings.clone(),
                    cx,
                )
            }
        });

        let applied_theme = settings.read(cx).theme.clone();
        crate::image_cache::start_idle_trim(cx);
        let image_cache = cx.new(|cx| {
            LruImageCache::labeled(
                "shared",
                SHARED_IMAGE_CACHE_CAPACITY,
                SHARED_IMAGE_CACHE_BYTES,
                SHARED_ENTRY_MAX_BYTES,
                cx,
            )
        });
        Self {
            title_bar,
            auth_state,
            login_view,
            chat_layout,
            settings_screen,
            clan_setting_screen,
            settings,
            applied_theme,
            image_cache,
        }
    }

    fn sync_settings_page(&mut self, cx: &mut Context<Self>) {
        let page = match Router::global(cx).read(cx).route() {
            Route::SettingsProfile => crate::settings::SettingsPage::Profile,
            Route::SettingsDevices => crate::settings::SettingsPage::Device,
            Route::SettingsAppearance => crate::settings::SettingsPage::Appearance,
            Route::SettingsActivity => crate::settings::SettingsPage::Activity,
            Route::SettingsNotifications => crate::settings::SettingsPage::Notifications,
            Route::SettingsLanguage => crate::settings::SettingsPage::Language,
            Route::SettingsVoice => crate::settings::SettingsPage::Voice,
            Route::SettingsAdvanced => crate::settings::SettingsPage::Advanced,
            Route::SettingsAccount => crate::settings::SettingsPage::Account,
            _ => return,
        };
        self.settings_screen
            .update(cx, |s, cx| s.set_page(page, cx));
    }

    fn sync_clan_settings_page(&mut self, cx: &mut Context<Self>) {
        match Router::global(cx).read(cx).route() {
            Route::ClanSettings { clan_id, page } => {
                self.clan_setting_screen.update(cx, |screen, cx| {
                    screen.set_clan_and_page(clan_id, page, cx);
                });
            }
            _ => {
                self.clan_setting_screen.update(cx, |screen, cx| {
                    screen.release_active_page(cx);
                });
            }
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("RootView");
        crate::image_cache::flush_atlas_drops(window, cx);
        let locale = self.settings.read(cx).language.clone();
        let base_font_family = ::theme::theme_settings(cx).ui_font(cx).family.clone();
        let theme = cx.theme();
        let state = self.auth_state.read(cx).clone();

        let content: gpui::AnyElement = match state {
            AuthState::NotAuthenticated | AuthState::OtpRequested { .. } => {
                cached_fill(self.login_view.clone())
            }
            AuthState::AwaitingCallback => render_awaiting_callback(theme, &locale),
            AuthState::Connecting(_) => {
                let attempt = ConnectionStore::global(cx).read(cx).connecting_attempt();
                render_connecting(theme, &locale, attempt)
            }
            AuthState::Authenticated(_) => {
                let route = Router::global(cx).read(cx).route();
                match route {
                    Route::SettingsAccount
                    | Route::SettingsProfile
                    | Route::SettingsDevices
                    | Route::SettingsAppearance
                    | Route::SettingsActivity
                    | Route::SettingsNotifications
                    | Route::SettingsLanguage
                    | Route::SettingsVoice
                    | Route::SettingsAdvanced => uncached_fill(self.settings_screen.clone()),
                    Route::ClanSettings { .. } => cached_fill(self.clan_setting_screen.clone()),
                    Route::NotFound { .. } => render_not_found(theme, &locale),
                    Route::AddFriend { .. } => render_placeholder(theme, "Add Friend"),
                    Route::Invite { .. } => render_placeholder(theme, "Accept Invite"),
                    _ => uncached_fill(self.chat_layout.clone()),
                }
            }
        };

        let overlay = crate::app::shell::Shell::global(cx)
            .read(cx)
            .render_overlay();

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_primary)
            .font_family(base_font_family)
            .text_color(theme.text_primary)
            .rounded(px(ROUNDED_BORDER_WINDOW))
            .overflow_hidden()
            .child(window_controls::render_app_drag_header())
            .image_cache(self.image_cache.clone())
            .on_action(cx.listener(|_, _: &crate::ToggleInspector, window, cx| {
                window.toggle_inspector(cx);
            }))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                |_, _, cx| crate::router::go_back(cx),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                |_, _, cx| crate::router::go_forward(cx),
            )
            .when(window_controls::HAS_CUSTOM_TITLE_BAR, |this| {
                this.child(render_title_bar(self.title_bar.clone()))
            })
            .child(content)
            .when(cfg!(target_os = "macos"), |this| {
                this.child(window_controls::render_controls(theme, window))
            })
            .when(window_controls::is_edge_resizable(), |this| {
                this.child(window_controls::render_resize_edges(window))
            })
            .child(overlay)
    }
}

fn render_title_bar(title_bar: Entity<TitleBar>) -> AnyView {
    let view = AnyView::from(title_bar);
    #[cfg(not(target_os = "windows"))]
    return view.cached(StyleRefinement::default().w_full().h_8());
    #[cfg(target_os = "windows")]
    view
}

fn cached_fill(view: impl Into<AnyView>) -> gpui::AnyElement {
    view.into()
        .cached(StyleRefinement::default().flex_1().min_h_0())
        .into_any_element()
}

fn uncached_fill(view: impl Into<AnyView>) -> gpui::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .child(view.into())
        .into_any_element()
}

fn render_awaiting_callback(theme: &Theme, locale: &str) -> gpui::AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .flex_col()
        .gap_4()
        .child(
            img(crate::util::assets::MEZON_LOGO)
                .w(px(280.))
                .h(px(50.))
                .object_fit(gpui::ObjectFit::Contain),
        )
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
                .child(mezon_i18n::t(locale, "root.awaitingCallback")),
        )
        .into_any_element()
}

fn render_connecting(theme: &Theme, locale: &str, attempt: u32) -> gpui::AnyElement {
    let label = if attempt > 0 {
        mezon_i18n::t(locale, "root.reconnectingAttempt").replace("{{count}}", &attempt.to_string())
    } else {
        mezon_i18n::t(locale, "root.loading").to_string()
    };
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .flex_col()
        .gap_4()
        .child(
            Spinner::new()
                .with_size(Size::Large)
                .color(theme.brand.into()),
        )
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(label),
        )
        .into_any_element()
}

fn render_placeholder(theme: &Theme, label: &str) -> gpui::AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text_primary)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child("Coming soon"),
        )
        .into_any_element()
}

fn render_not_found(theme: &Theme, locale: &str) -> gpui::AnyElement {
    let back_btn = Button::new("back-to-chat")
        .label(mezon_i18n::t(locale, "root.backToChat"))
        .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            crate::router::go_back(cx);
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
                .child(mezon_i18n::t(locale, "root.pageNotFound")),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text_secondary)
                .child(mezon_i18n::t(locale, "root.pageNotFoundDesc")),
        )
        .child(back_btn)
        .into_any_element()
}
