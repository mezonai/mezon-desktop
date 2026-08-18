use crate::app::shell::Shell;
use crate::app::title_bar::TitleBar;
use crate::app::window_controls;
use crate::auth::login_view::LoginView;
use crate::chat::call_window::CallOverlay;
use crate::chat::channel_settings::ChannelSettingScreen;
use crate::chat::layout::ChatLayout;
use crate::clan::settings::{ClanSettingScreen, ClanSettingsPage};
use crate::components::primitives::{Button, Icon, IconName};
use crate::image_cache::{
    LruImageCache, SHARED_ENTRY_MAX_BYTES, SHARED_IMAGE_CACHE_BYTES, SHARED_IMAGE_CACHE_CAPACITY,
};
use crate::router::{Route, Router};
use crate::settings::SettingsScreen;
use crate::theme::{ActiveTheme, Theme, resolve_theme};
use gpui::{
    Animation, AnimationExt as _, AnyView, App, ClickEvent, Context, Entity, FontWeight,
    MouseButton, NavigationDirection, StyleRefinement, Task, Window, div, img, prelude::*, px,
};
use mezon_store::{AuthState, ChannelList, ClanId, ClanList, ConnectionStore, Settings};
use std::time::{Duration, Instant};

pub struct RootView {
    title_bar: Entity<TitleBar>,
    auth_state: Entity<AuthState>,
    login_view: Entity<LoginView>,
    chat_layout: Entity<ChatLayout>,
    settings_screen: Entity<SettingsScreen>,
    clan_setting_screen: Entity<ClanSettingScreen>,
    channel_setting_screen: Entity<ChannelSettingScreen>,
    shell: Entity<Shell>,
    applied_theme: String,
    cached_locale: String,
    image_cache: Entity<LruImageCache>,
    connecting_since: Option<Instant>,
    network_online: bool,
    call_overlay: Entity<CallOverlay>,
    _splash_delay: Option<Task<()>>,
    _recording_toasts: Option<gpui::Subscription>,
}

fn surface_recording_toast(
    root: &mut RootView,
    _voice: gpui::Entity<mezon_store::VoiceStore>,
    event: &mezon_store::VoiceStoreEvent,
    cx: &mut Context<RootView>,
) {
    let locale = root.cached_locale.clone();
    let toast = match event {
        mezon_store::VoiceStoreEvent::RecordingVideoUnavailable => {
            crate::app::shell::Shell::global(cx).update(cx, |shell, cx| {
                shell.toast(
                    crate::components::primitives::ToastKind::Info,
                    mezon_i18n::t(&locale, "channelVoice.recordingVideoUnavailable").to_string(),
                    cx,
                )
            });
            return;
        }
        mezon_store::VoiceStoreEvent::RecordingFinished(toast) => toast.clone(),
    };
    let (kind, message) = match toast {
        mezon_store::RecordingToast::Saved(path) => (
            crate::components::primitives::ToastKind::Success,
            format!(
                "{} {}",
                mezon_i18n::t(&locale, "channelVoice.recordingSaved"),
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
        ),
        mezon_store::RecordingToast::Failed(error) => (
            crate::components::primitives::ToastKind::Error,
            format!(
                "{}: {error}",
                mezon_i18n::t(&locale, "channelVoice.recordingFailed")
            ),
        ),
    };
    crate::app::shell::Shell::global(cx).update(cx, |shell, cx| shell.toast(kind, message, cx));
}

fn spawn_splash_delay(cx: &mut Context<RootView>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(SPLASH_QUIET_MS))
            .await;
        let _ = this.update(cx, |_, cx| cx.notify());
    })
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
        let shell = Shell::init(cx);

        let recording_toasts = mezon_store::VoiceStore::try_global(cx)
            .map(|voice| cx.subscribe(&voice, surface_recording_toast));

        cx.observe(&settings, |this, settings, cx| {
            let (language, name) = {
                let settings = settings.read(cx);
                (settings.language.clone(), settings.theme.clone())
            };
            if language != this.cached_locale {
                this.cached_locale = language;
                cx.notify();
            }
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
            this.sync_channel_settings_tab(cx);
            cx.notify();
        })
        .detach();

        cx.observe(&auth_state, |this, auth_state, cx| {
            if matches!(*auth_state.read(cx), AuthState::Connecting(_)) {
                if this.connecting_since.is_none() {
                    this.connecting_since = Some(Instant::now());
                    this._splash_delay = Some(spawn_splash_delay(cx));
                }
            } else {
                this.connecting_since = None;
                this._splash_delay = None;
            }
            if matches!(*auth_state.read(cx), AuthState::NotAuthenticated) {
                crate::image_viewer::close_image_viewer(cx);
                crate::chat::media_channel::close_media_image_modal(cx);
                crate::image_cache::clear_all_image_caches(cx);
                mezon_canvas::reset_canvas_image_caches(cx);
                Router::global(cx).update(cx, |router, cx| {
                    router.reset();
                    cx.notify();
                });
            }
            cx.notify();
        })
        .detach();

        cx.observe(&ConnectionStore::global(cx), |this, store, cx| {
            let online = store.read(cx).is_online();
            if this.network_online != online {
                this.network_online = online;
                let locale = this.cached_locale.clone();
                Shell::global(cx).update(cx, |shell, cx| {
                    if online {
                        shell.dismiss(NETWORK_OFFLINE_TOAST_KEY, cx);
                    } else {
                        let message =
                            mezon_i18n::t(&locale, "common.errorBoundary.stillOffline").to_string();
                        shell.sticky(
                            NETWORK_OFFLINE_TOAST_KEY,
                            crate::components::primitives::ToastKind::Error,
                            message,
                            cx,
                        );
                    }
                });
            }
            cx.notify();
        })
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

        let channel_setting_screen = cx.new({
            let settings = settings.clone();
            move |cx| ChannelSettingScreen::new(settings.clone(), cx)
        });

        let applied_theme = settings.read(cx).theme.clone();
        let cached_locale = settings.read(cx).language.clone();
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
        let network_online = ConnectionStore::global(cx).read(cx).is_online();
        let connecting_at_start = matches!(*auth_state.read(cx), AuthState::Connecting(_));
        let (connecting_since, splash_delay) = if connecting_at_start {
            (Some(Instant::now()), Some(spawn_splash_delay(cx)))
        } else {
            (None, None)
        };
        let call_overlay = cx.new(CallOverlay::new);
        Self {
            call_overlay,
            title_bar,
            auth_state,
            login_view,
            chat_layout,
            settings_screen,
            clan_setting_screen,
            channel_setting_screen,
            shell,
            applied_theme,
            cached_locale,
            image_cache,
            connecting_since,
            network_online,
            _splash_delay: splash_delay,
            _recording_toasts: recording_toasts,
        }
    }

    fn sync_settings_page(&mut self, cx: &mut Context<Self>) {
        let page = match Router::global(cx).read(cx).route() {
            Route::SettingsProfile => {
                self.settings_screen
                    .update(cx, |screen, cx| screen.set_profile_target(None, cx));
                return;
            }
            Route::SettingsClanProfile { clan_id } => {
                self.settings_screen.update(cx, |screen, cx| {
                    screen.set_profile_target(Some(clan_id), cx)
                });
                return;
            }
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

    fn sync_channel_settings_tab(&mut self, cx: &mut Context<Self>) {
        match Router::global(cx).read(cx).route() {
            Route::ChannelSettings {
                clan_id,
                channel_id,
                tab,
            } => {
                self.channel_setting_screen.update(cx, |screen, cx| {
                    screen.set_target(clan_id, channel_id, tab, cx);
                });
            }
            _ => {
                self.channel_setting_screen.update(cx, |screen, cx| {
                    screen.release_active_tab(cx);
                });
            }
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace_render!("RootView");
        crate::image_cache::flush_atlas_drops(window, cx);
        crate::image_cache::flush_atlas_replaces(window, cx);
        let locale = self.cached_locale.as_str();
        let base_font_family = ::theme::theme_settings(cx).ui_font(cx).family.clone();
        let theme = cx.theme();

        let content: gpui::AnyElement = match self.auth_state.read(cx) {
            AuthState::NotAuthenticated | AuthState::OtpRequested { .. } => {
                cached_fill(self.login_view.clone())
            }
            AuthState::AwaitingCallback => render_awaiting_callback(theme, locale),
            AuthState::Connecting(_) => {
                let (attempt, online) = {
                    let store = ConnectionStore::global(cx).read(cx);
                    (store.connecting_attempt(), store.is_online())
                };
                let waited = self
                    .connecting_since
                    .map(|since| since.elapsed())
                    .unwrap_or_default();
                if should_show_splash(attempt, online, waited) {
                    render_connecting(locale, attempt, online, window.is_window_active())
                } else {
                    render_quiet_startup()
                }
            }
            AuthState::Authenticated(_) => {
                let route = Router::global(cx).read(cx).route();
                match route {
                    Route::SettingsAccount
                    | Route::SettingsProfile
                    | Route::SettingsClanProfile { .. }
                    | Route::SettingsDevices
                    | Route::SettingsAppearance
                    | Route::SettingsActivity
                    | Route::SettingsNotifications
                    | Route::SettingsLanguage
                    | Route::SettingsVoice
                    | Route::SettingsAdvanced => uncached_fill(self.settings_screen.clone()),
                    Route::ClanSettings { .. } => cached_fill(self.clan_setting_screen.clone()),
                    Route::ChannelSettings { .. } => {
                        uncached_fill(self.channel_setting_screen.clone())
                    }
                    Route::NotFound { .. } => render_not_found(theme, locale),
                    Route::AddFriend { .. } => render_placeholder(theme, "Add Friend"),
                    Route::Invite { .. } => render_placeholder(theme, "Accept Invite"),
                    _ => cached_fill(self.chat_layout.clone()),
                }
            }
        };

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .font_family(base_font_family)
            .text_color(theme.text_primary)
            .overflow_hidden()
            .bg(theme.surfaces.secondary.ramp())
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
            .when(window_controls::is_edge_resizable(), |this| {
                this.child(window_controls::render_resize_edges(window))
            })
            .child(self.shell.clone())
            .child(self.call_overlay.clone())
    }
}

fn render_title_bar(title_bar: Entity<TitleBar>) -> AnyView {
    AnyView::from(title_bar).cached(StyleRefinement::default().w_full().h_8())
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

const NETWORK_OFFLINE_TOAST_KEY: &str = "network-offline";
const SPLASH_BG: u32 = 0x1e1f22;
const SPLASH_ACCENT: u32 = 0x5865f2;
const SPLASH_ACCENT_END: u32 = 0x7289da;
const SPLASH_STATUS_TEXT: u32 = 0xd1d5db;
const SPLASH_LOGO_WIDTH: f32 = 280.;
const SPLASH_LOGO_HEIGHT: f32 = 50.;
const SPLASH_LOGO_VIEWPORT_FRACTION: f32 = 0.72;
const SPLASH_DOT_BASE_SIZE: f32 = 6.;
const SPLASH_DOT_SCALE_MIN: f32 = 0.8;
const SPLASH_DOT_SCALE_RANGE: f32 = 0.4;
const SPLASH_DOT_CELL_SIZE: f32 =
    SPLASH_DOT_BASE_SIZE * (SPLASH_DOT_SCALE_MIN + SPLASH_DOT_SCALE_RANGE);
const SPLASH_DOT_PITCH: f32 = 12.;
const SPLASH_DOT_CYCLE_MS: u64 = 1400;
const SPLASH_DOT_STAGGER_MS: u64 = 200;
const SPLASH_PROGRESS_MS: u64 = 30_000;
const SPLASH_PROGRESS_HEIGHT: f32 = 3.;
const SPLASH_QUIET_MS: u64 = 600;
const SPLASH_FADE_MS: u64 = 250;

fn splash_progress_fraction(delta: f32) -> f32 {
    if delta <= 0.8 {
        delta / 0.8 * 0.85
    } else {
        0.85 + (delta - 0.8) / 0.2 * 0.1
    }
}

fn splash_dot_intensity(delta: f32, offset: f32) -> f32 {
    let phase = (delta + offset).rem_euclid(1.0);
    if phase <= 0.4 {
        phase / 0.4
    } else if phase <= 0.8 {
        1.0 - (phase - 0.4) / 0.4
    } else {
        0.0
    }
}

/// `animate: false` renders the dots as plain static circles. The animation repeats forever, and a
/// connect that never succeeds leaves this screen up indefinitely — driving a frame request per
/// tick on the root view for as long as the machine stays offline. Nobody is watching a background
/// window, so the pulse is dropped there.
fn render_splash_dots(animate: bool) -> gpui::AnyElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SPLASH_DOT_PITCH - SPLASH_DOT_CELL_SIZE))
        .mt(px(4.))
        .h(px(SPLASH_DOT_CELL_SIZE));
    for index in 0..3u64 {
        let offset = (index * SPLASH_DOT_STAGGER_MS) as f32 / SPLASH_DOT_CYCLE_MS as f32;
        let dot = div()
            .rounded_full()
            .bg(gpui::rgb(SPLASH_ACCENT))
            .size(px(SPLASH_DOT_BASE_SIZE));
        let dot = if animate {
            dot.with_animation(
                gpui::ElementId::Integer(index),
                Animation::new(Duration::from_millis(SPLASH_DOT_CYCLE_MS)).repeat(),
                move |el, delta| {
                    let intensity = splash_dot_intensity(delta, offset);
                    let scale = SPLASH_DOT_SCALE_MIN + SPLASH_DOT_SCALE_RANGE * intensity;
                    el.opacity(0.2 + 0.8 * intensity)
                        .size(px(SPLASH_DOT_BASE_SIZE * scale))
                },
            )
            .into_any_element()
        } else {
            dot.opacity(0.6).into_any_element()
        };
        row = row.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(SPLASH_DOT_CELL_SIZE))
                .child(dot),
        );
    }
    row.into_any_element()
}

fn render_splash_progress_bar() -> gpui::AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .h(px(SPLASH_PROGRESS_HEIGHT))
        .rounded_r(px(2.))
        .bg(gpui::linear_gradient(
            90.,
            gpui::linear_color_stop(gpui::rgb(SPLASH_ACCENT), 0.),
            gpui::linear_color_stop(gpui::rgb(SPLASH_ACCENT_END), 1.),
        ))
        .with_animation(
            "splash-progress",
            Animation::new(Duration::from_millis(SPLASH_PROGRESS_MS)),
            |el, delta| el.w(gpui::relative(splash_progress_fraction(delta))),
        )
        .into_any_element()
}

fn should_show_splash(attempt: u32, online: bool, waited: Duration) -> bool {
    attempt > 0 || !online || waited >= Duration::from_millis(SPLASH_QUIET_MS)
}

fn render_quiet_startup() -> gpui::AnyElement {
    div()
        .flex_1()
        .size_full()
        .bg(gpui::rgb(SPLASH_BG))
        .into_any_element()
}

fn render_connecting(locale: &str, attempt: u32, online: bool, animate: bool) -> gpui::AnyElement {
    let label = if !online {
        mezon_i18n::t(locale, "common.errorBoundary.stillOffline").to_string()
    } else if attempt > 0 {
        mezon_i18n::t(locale, "root.reconnectingAttempt").replace("{{count}}", &attempt.to_string())
    } else {
        mezon_i18n::t(locale, "root.loading").to_string()
    };
    let fade = |id: &'static str| {
        (
            id,
            Animation::new(Duration::from_millis(SPLASH_FADE_MS)).with_easing(gpui::ease_in_out),
        )
    };
    let (bar_id, bar_anim) = fade("splash-bar-fade");
    let (body_id, body_anim) = fade("splash-body-fade");
    div()
        .relative()
        .flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .flex_col()
        .bg(gpui::rgb(SPLASH_BG))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .child(render_splash_progress_bar())
                .with_animation(bar_id, bar_anim, |el, delta| el.opacity(delta)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    img(crate::util::assets::MEZON_LOGO)
                        .w(px(SPLASH_LOGO_WIDTH))
                        .h(px(SPLASH_LOGO_HEIGHT))
                        .max_w(gpui::relative(SPLASH_LOGO_VIEWPORT_FRACTION))
                        .object_fit(gpui::ObjectFit::Contain)
                        .mb(px(24.)),
                )
                .child(render_splash_dots(animate))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(gpui::rgb(SPLASH_STATUS_TEXT))
                        .mt(px(10.))
                        .mb(px(12.))
                        .child(label),
                )
                .with_animation(body_id, body_anim, |el, delta| el.opacity(delta)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_startup_stays_silent_while_the_first_connect_is_quick() {
        assert!(!should_show_splash(0, true, Duration::ZERO));
        assert!(!should_show_splash(
            0,
            true,
            Duration::from_millis(SPLASH_QUIET_MS - 1)
        ));
    }

    #[test]
    fn splash_appears_once_the_quiet_window_elapses() {
        assert!(should_show_splash(
            0,
            true,
            Duration::from_millis(SPLASH_QUIET_MS)
        ));
    }

    #[test]
    fn splash_shows_immediately_while_reconnecting_or_offline() {
        assert!(should_show_splash(1, true, Duration::ZERO));
        assert!(should_show_splash(0, false, Duration::ZERO));
        assert!(should_show_splash(3, false, Duration::ZERO));
    }

    #[test]
    fn progress_bar_never_reaches_full_width() {
        assert_eq!(splash_progress_fraction(0.0), 0.0);
        assert!((splash_progress_fraction(0.8) - 0.85).abs() < f32::EPSILON);
        assert!((splash_progress_fraction(1.0) - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn dots_are_staggered_so_they_do_not_pulse_together() {
        let offset = 200. / 1400.;
        let at_peak = splash_dot_intensity(0.4, 0.0);
        assert!((at_peak - 1.0).abs() < f32::EPSILON);
        assert!(splash_dot_intensity(0.4, offset) < at_peak);
        assert_eq!(splash_dot_intensity(0.9, 0.0), 0.0);
    }
}
