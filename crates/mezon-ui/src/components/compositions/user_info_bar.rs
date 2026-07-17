use gpui::{App, ClickEvent, Context, Entity, SharedString, Window, div, prelude::*, px};

use crate::components::primitives::{Avatar, Icon, IconName, Sizable, Size};
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
    avatar_src: SharedString,
    avatar_raw: SharedString,
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
            if this.sync_avatar(cx) {
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
            avatar_src: SharedString::default(),
            avatar_raw: SharedString::default(),
        };
        bar.sync_presence(cx);
        bar.sync_avatar(cx);
        bar
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

        let mut avatar = Avatar::new()
            .name(self.username.clone())
            .with_size(Size::Small);
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
            .w_full()
            .min_h(px(56.0))
            .overflow_hidden()
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.bg_surface)
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
                            .flex()
                            .flex_1()
                            .h(px(40.0))
                            .items_center()
                            .gap_3()
                            .pl_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_item_hover))
                            .child(
                                div().relative().child(avatar).child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .right_0()
                                        .size_2()
                                        .rounded_full()
                                        .bg(presence_color),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .child(self.username.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.text_muted)
                                            .child(self.presence.clone()),
                                    ),
                            ),
                    )
                    .child(settings_btn),
            )
    }
}
