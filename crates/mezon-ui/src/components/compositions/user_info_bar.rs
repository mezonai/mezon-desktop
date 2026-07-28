use gpui::{
    App, ClickEvent, Context, DismissEvent, Entity, MouseButton, MouseDownEvent, Point,
    SharedString, Subscription, Window, anchored, deferred, div, prelude::*, px,
};

use crate::components::compositions::footer_profile_popup::FooterProfilePopup;
use crate::components::primitives::{Avatar, Icon, IconName};
use crate::theme::ActiveTheme;
use mezon_store::{AccountStore, AuthState, PresenceStore};

fn on_settings_click() -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        crate::router::navigate(cx, crate::router::Route::SettingsAccount);
    }
}

pub struct UserInfoBar {
    auth_state: Entity<AuthState>,
    account_store: Entity<AccountStore>,
    username: SharedString,
    presence: SharedString,
    status: SharedString,
    user_status: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    profile_popup: Option<Entity<FooterProfilePopup>>,
    _popup_sub: Option<Subscription>,
}

impl UserInfoBar {
    pub fn new(auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        let account_store = AccountStore::global(cx);
        cx.observe(&PresenceStore::global(cx), |this, _, cx| {
            if this.sync_presence(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.observe(&auth_state, |this, _, cx| {
            if this.sync_presence(cx) {
                cx.notify();
            }
        })
        .detach();
        cx.observe(&account_store, |this, _, cx| {
            let changed = this.sync_avatar(cx) | this.sync_status(cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
        account_store.update(cx, |store, cx| store.ensure_account(cx));
        let username = Self::read_username(&auth_state, cx);
        let mut bar = Self {
            auth_state,
            account_store,
            username,
            presence: SharedString::from("Offline"),
            status: SharedString::default(),
            user_status: SharedString::default(),
            avatar_src: SharedString::default(),
            avatar_raw: SharedString::default(),
            profile_popup: None,
            _popup_sub: None,
        };
        bar.sync_presence(cx);
        bar.sync_avatar(cx);
        bar.sync_status(cx);
        bar
    }

    fn sync_status(&mut self, cx: &App) -> bool {
        let prev_status = self.status.clone();
        let prev_custom = self.user_status.clone();
        let (status, custom) = match self.account_store.read(cx).account.as_ref() {
            Some(account) => (account.status.clone(), account.user_status.clone()),
            None => (String::new(), String::new()),
        };
        self.status = SharedString::from(status);
        self.user_status = SharedString::from(custom);
        self.status != prev_status || self.user_status != prev_custom
    }

    fn toggle_profile_popup(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_popup.is_some() {
            self.profile_popup = None;
            self._popup_sub = None;
            cx.notify();
            return;
        }
        let popup = cx.new(FooterProfilePopup::new);
        self._popup_sub = Some(cx.subscribe(&popup, |this, _, _: &DismissEvent, cx| {
            this.profile_popup = None;
            cx.notify();
        }));
        self.profile_popup = Some(popup);
        cx.notify();
    }

    fn sync_avatar(&mut self, cx: &App) -> bool {
        let prev_src = self.avatar_src.clone();
        let prev_raw = self.avatar_raw.clone();
        let raw = self
            .account_store
            .read(cx)
            .account
            .as_ref()
            .and_then(|account| account.avatar_url.clone())
            .unwrap_or_default();
        self.avatar_src = if raw.is_empty() {
            SharedString::default()
        } else {
            SharedString::from(crate::util::imgproxy::avatar_url(cx, &raw))
        };
        self.avatar_raw = SharedString::from(raw);
        self.avatar_src != prev_src || self.avatar_raw != prev_raw
    }

    fn read_username(auth_state: &Entity<AuthState>, cx: &App) -> SharedString {
        match auth_state.read(cx) {
            AuthState::Authenticated(session) => SharedString::from(session.username.clone()),
            _ => SharedString::from("Unknown"),
        }
    }

    pub fn sync_presence(&mut self, cx: &App) -> bool {
        let prev_username = self.username.clone();
        let prev_presence = self.presence.clone();
        let user_id = match self.auth_state.read(cx) {
            AuthState::Authenticated(session) => {
                self.username = SharedString::from(session.username.clone());
                session.user_id.clone()
            }
            _ => {
                self.username = SharedString::from("Unknown");
                self.presence = SharedString::from("Offline");
                return self.username != prev_username || self.presence != prev_presence;
            }
        };
        let online = PresenceStore::global(cx)
            .read(cx)
            .user_online
            .contains(&user_id.parse().unwrap_or_default());
        self.presence = SharedString::from(if online { "Online" } else { "Offline" });
        self.username != prev_username || self.presence != prev_presence
    }
}

impl Render for UserInfoBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let presence_color = match self.presence.as_ref() {
            "Online" => theme.status_online,
            "Idle" => theme.status_idle,
            "Dnd" => theme.status_dnd,
            _ => theme.status_offline,
        };
        let status_dot_color = match self.status.as_ref() {
            "Online" => theme.status_online,
            "Idle" => theme.status_idle,
            "Do Not Disturb" => theme.status_dnd,
            "Invisible" => theme.status_offline,
            _ => presence_color,
        };
        let subtitle = if self.user_status.is_empty() {
            self.presence.clone()
        } else {
            self.user_status.clone()
        };

        let mut settings_btn = div()
            .id(SharedString::from("settings-btn"))
            .cursor_pointer()
            .p_1()
            .rounded_md()
            .hover(|s| s.bg(theme.tokens.bg_item_hover))
            .child(
                Icon::new(IconName::SettingProfile)
                    .size(px(20.0))
                    .text_color(theme.text_secondary),
            );
        settings_btn.interactivity().on_click(on_settings_click());

        let mut avatar = Avatar::new().name(self.username.clone()).size_px(px(32.0));
        if !self.avatar_src.is_empty() {
            avatar = avatar.src(self.avatar_src.clone());
            if !self.avatar_raw.is_empty() && self.avatar_raw != self.avatar_src {
                avatar = avatar.fallback_src(self.avatar_raw.clone());
            }
        } else if !self.avatar_raw.is_empty() {
            avatar = avatar.src(self.avatar_raw.clone());
        }

        // Positioning (absolute / insets) is applied by the cached wrapper in
        // the chat layout so this view can be `.cached()`; keep only the visual
        // box here.
        div()
            .relative()
            .w_full()
            .min_h(px(56.0))
            .when_some(self.profile_popup.clone(), |bar, popup| {
                bar.child(deferred(anchored().position(Point::default()).child(
                    div().w(px(100000.)).h(px(100000.)).occlude().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.profile_popup = None;
                            this._popup_sub = None;
                            cx.notify();
                        }),
                    ),
                )))
                .child(deferred(
                    div()
                        .absolute()
                        .bottom_full()
                        .left(px(8.))
                        .mb_2()
                        .child(popup),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .gap_2()
                    .pl_2()
                    .pr_4()
                    .py_2()
                    .child(
                        div()
                            .id(SharedString::from("footer-profile-open"))
                            .flex()
                            .flex_1()
                            .h(px(40.0))
                            .items_center()
                            .gap_3()
                            .pl_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_item_hover))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.toggle_profile_popup(window, cx);
                            }))
                            .child(
                                div().relative().child(avatar).child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .right_0()
                                        .size_2()
                                        .rounded_full()
                                        .bg(status_dot_color),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .max_w(px(150.))
                                            .truncate()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(self.username.clone()),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(150.))
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(theme.text_muted)
                                            .child(subtitle),
                                    ),
                            ),
                    )
                    .child(settings_btn),
            )
    }
}
