use std::collections::HashMap;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, SharedString, StatefulInteractiveElement, Styled, Subscription,
    UniformListScrollHandle, Window, div, img, prelude::*, px, relative, rgb, uniform_list,
};
use mezon_store::activity::{ACTIVITY_TYPE_LIVE, ACTIVITY_TYPE_PLAY, ACTIVITY_TYPE_WORK};
use mezon_store::{
    ActivityEvent, ActivityStore, BadgeService, DirectMessageStore, DmAvatarPresence, Friend,
    FriendEvent, FriendState, FriendStore, PresenceEvent, PresenceStore, Settings, UserActivity,
    UserId, current_user_presence,
};

use crate::app::shell::{FriendRemovalKind, Shell};
use crate::app::window_controls::APP_HEADER_HEIGHT;
use crate::chat::message::{ShareContactModal, share_contact_subject};
use crate::components::primitives::{
    Avatar, ContextMenu, Icon, IconName, Input, InputEvent, InputState, ToastKind, context_menu_at,
};
use crate::image_cache::LruImageCache;
use crate::router::{Route, Router, navigate};
use crate::theme::{ActiveTheme, Theme};
use crate::util::imgproxy;

const COLOR_DANGER: u32 = 0xDA373C;
const ROW_HEIGHT: f32 = 64.;
const ACTIVITY_WIDTH: f32 = 416.;
const AVATAR_SIZE: f32 = 32.;
const MAX_USERNAME_LEN: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FriendsTab {
    All,
    Online,
    Pending,
    Block,
}

impl FriendsTab {
    const ALL: [FriendsTab; 4] = [
        FriendsTab::All,
        FriendsTab::Online,
        FriendsTab::Pending,
        FriendsTab::Block,
    ];

    fn elem_id(self) -> &'static str {
        match self {
            FriendsTab::All => "friend-tab-all",
            FriendsTab::Online => "friend-tab-online",
            FriendsTab::Pending => "friend-tab-pending",
            FriendsTab::Block => "friend-tab-block",
        }
    }

    fn title_key(self) -> &'static str {
        match self {
            FriendsTab::All => "friendsPage.tabs.all",
            FriendsTab::Online => "friendsPage.tabs.online",
            FriendsTab::Pending => "friendsPage.tabs.pending",
            FriendsTab::Block => "friendsPage.tabs.block",
        }
    }
}

struct FriendRow {
    id: UserId,
    group_name: SharedString,
    label: SharedString,
    username: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    online: bool,
    presence: DmAvatarPresence,
    user_status: SharedString,
    state: FriendState,
}

/// A row of the right-hand activity sidebar — either a group heading ("ACTIVITY - WORK - N") or a
/// single user's activity (avatar + name + one-line description), mirroring React's flat
/// separator-plus-items list.
enum ActivityRow {
    Header(SharedString),
    Item {
        label: SharedString,
        description: SharedString,
        avatar_src: SharedString,
        avatar_raw: SharedString,
    },
}

/// The activity groups, in render order, each with its `activity_type` and i18n heading key —
/// matches React `ActivitiesType` (1 = Work, 2 = Live, 3 = Play).
const ACTIVITY_SECTIONS: [(i32, &str); 3] = [
    (ACTIVITY_TYPE_WORK, "friendsPage.activity.coding"),
    (ACTIVITY_TYPE_LIVE, "friendsPage.activity.music"),
    (ACTIVITY_TYPE_PLAY, "friendsPage.activity.gaming"),
];

pub struct FriendsPage {
    settings: Entity<Settings>,
    selected_tab: FriendsTab,
    add_friend_open: bool,
    search: Option<Entity<InputState>>,
    add_input: Option<Entity<InputState>>,
    add_error: Option<SharedString>,
    rows: Vec<FriendRow>,
    activity_rows: Vec<ActivityRow>,
    pending_count: usize,
    list_header: SharedString,
    list_scroll: UniformListScrollHandle,
    activity_scroll: UniformListScrollHandle,
    avatar_cache: Entity<LruImageCache>,
    open_menu: Option<(UserId, Point<Pixels>)>,
    cached_locale: SharedString,
    _subs: Vec<Subscription>,
}

impl FriendsPage {
    pub fn new(settings: Entity<Settings>, cx: &mut Context<Self>) -> Self {
        let avatar_cache = cx.new(|cx| {
            LruImageCache::avatar_thumbnail_small(
                "friends-page",
                512,
                16 * 1024 * 1024,
                4 * 1024 * 1024,
                cx,
            )
        });

        let mut subs = Vec::new();
        subs.push(
            cx.subscribe(&FriendStore::global(cx), |this, _, event, cx| match event {
                FriendEvent::Changed => {
                    if on_friends_route(cx) {
                        this.rebuild(cx);
                    }
                }
                FriendEvent::AddSucceeded => {
                    this.toast(ToastKind::Success, "friends.toast.sendAddFriendSuccess", cx);
                }
                FriendEvent::AcceptSucceeded => {
                    this.toast(
                        ToastKind::Success,
                        "friends.toast.acceptAddFriendSuccess",
                        cx,
                    );
                }
                FriendEvent::AddFailed => {
                    this.toast(ToastKind::Error, "friends.toast.sendAddFriendFail", cx);
                }
                FriendEvent::AddingChanged => cx.notify(),
                FriendEvent::BlockSucceeded => {
                    this.toast(
                        ToastKind::Success,
                        "friendsPage.toast.userBlockedSuccess",
                        cx,
                    );
                }
                FriendEvent::BlockFailed => {
                    this.toast(ToastKind::Error, "friendsPage.toast.userBlockedFailed", cx);
                }
                FriendEvent::UnblockSucceeded => {
                    this.toast(
                        ToastKind::Success,
                        "friendsPage.toast.userUnblockedSuccess",
                        cx,
                    );
                }
                FriendEvent::UnblockFailed => {
                    this.toast(
                        ToastKind::Error,
                        "friendsPage.toast.userUnblockedFailed",
                        cx,
                    );
                }
            }),
        );
        subs.push(cx.subscribe(
            &PresenceStore::global(cx),
            |this, _, event, cx| match event {
                PresenceEvent::ChannelPresenceChanged { .. } | PresenceEvent::StatusChanged => {
                    if on_friends_route(cx) {
                        this.apply_presence(cx);
                    }
                }
                PresenceEvent::TypingChanged { .. } => {}
            },
        ));
        subs.push(cx.subscribe(
            &ActivityStore::global(cx),
            |this, _, event, cx| match event {
                ActivityEvent::Changed => {
                    if on_friends_route(cx) {
                        this.rebuild_activity_rows(cx);
                    }
                }
            },
        ));
        subs.push(cx.observe(&Router::global(cx), |this, _, cx| {
            if on_friends_route(cx) {
                FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
                ActivityStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
                this.rebuild(cx);
            } else {
                this.clear_transient_state(cx);
            }
        }));
        subs.push(cx.observe(&settings, |this, settings, cx| {
            let locale = SharedString::from(settings.read(cx).language.clone());
            if locale != this.cached_locale {
                this.cached_locale = locale;
                this.rebuild(cx);
                cx.notify();
            }
        }));

        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ActivityStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

        let initial_locale = settings.read(cx).language.clone();
        let mut this = Self {
            settings,
            selected_tab: FriendsTab::All,
            add_friend_open: false,
            search: None,
            add_input: None,
            add_error: None,
            rows: Vec::new(),
            activity_rows: Vec::new(),
            pending_count: 0,
            list_header: SharedString::default(),
            list_scroll: UniformListScrollHandle::new(),
            activity_scroll: UniformListScrollHandle::new(),
            avatar_cache,
            open_menu: None,
            cached_locale: SharedString::from(initial_locale),
            _subs: subs,
        };
        this.rebuild(cx);
        this
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder = mezon_i18n::t(&locale, "friendsPage.search").to_string();
            let search = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .embedded(true)
            });
            self._subs
                .push(cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.rebuild_friend_rows(cx);
                    }
                }));
            self.search = Some(search);
        }
        if self.add_input.is_none() {
            let locale = self.settings.read(cx).language.clone();
            let placeholder =
                mezon_i18n::t(&locale, "friendsPage.addFriendModal.placeholder").to_string();
            let add_input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .embedded(true)
                    .validate(|value, _| value.chars().count() <= MAX_USERNAME_LEN)
            });
            self._subs.push(cx.subscribe(
                &add_input,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.revalidate_add_input(cx),
                    InputEvent::PressEnter => this.submit_add_friend(cx),
                },
            ));
            self.add_input = Some(add_input);
        }
    }

    fn search_has_text(&self, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|s| !s.read(cx).value().is_empty())
    }

    fn search_text(&self, cx: &App) -> String {
        self.search
            .as_ref()
            .map(|s| s.read(cx).value().to_string())
            .unwrap_or_default()
    }

    fn current_user_id(cx: &App) -> UserId {
        BadgeService::try_global(cx)
            .and_then(|b| b.read(cx).current_user_id(cx))
            .unwrap_or(UserId(0))
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.rebuild_friend_rows(cx);
        self.rebuild_activity_rows(cx);
    }

    fn rebuild_activity_rows(&mut self, cx: &mut Context<Self>) {
        let store = FriendStore::global(cx);
        let locale = self.settings.read(cx).language.clone();
        self.activity_rows =
            build_activity_rows(&locale, store.read(cx), ActivityStore::global(cx), cx);
        cx.notify();
    }

    fn apply_presence(&mut self, cx: &mut Context<Self>) {
        if self.selected_tab == FriendsTab::Online {
            self.rebuild_friend_rows(cx);
            return;
        }
        let store = PresenceStore::global(cx);
        let presence = store.read(cx);
        let mut changed = false;
        let own_presence = current_user_presence(cx);
        for row in &mut self.rows {
            let online = presence.is_online(row.id);
            if row.online != online {
                row.online = online;
                changed = true;
            }
            let badge = presence.member_presence(row.id, own_presence);
            if row.presence != badge {
                row.presence = badge;
                changed = true;
            }
            let status = presence.user_status(row.id).unwrap_or("");
            if row.user_status.as_ref() != status {
                row.user_status = SharedString::from(status.to_string());
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn rebuild_friend_rows(&mut self, cx: &mut Context<Self>) {
        let store = FriendStore::global(cx);
        let presence = PresenceStore::global(cx);
        let me = Self::current_user_id(cx);
        let tab = self.selected_tab;
        let search = self.search_text(cx).to_lowercase();
        let locale = self.settings.read(cx).language.clone();

        let friends = store.read(cx);
        let presence = presence.read(cx);
        let pending = friends.pending_incoming_count();

        let mut filtered: Vec<&Friend> = friends
            .friends()
            .iter()
            .filter(|f| match tab {
                FriendsTab::All => f.state == FriendState::Friend,
                FriendsTab::Online => f.state == FriendState::Friend && presence.is_online(f.id),
                FriendsTab::Pending => {
                    f.state == FriendState::InviteSent || f.state == FriendState::InviteReceived
                }
                FriendsTab::Block => f.state == FriendState::Blocked && f.source_id == me,
            })
            .filter(|f| {
                search.is_empty()
                    || f.username.to_lowercase().contains(&search)
                    || f.display_name.to_lowercase().contains(&search)
            })
            .collect();

        if tab == FriendsTab::Pending {
            filtered.sort_by_key(|f| std::cmp::Reverse(pending_rank(f.state)));
        } else {
            filtered.sort_by_cached_key(|f| f.label().to_lowercase());
        }

        let own_presence = current_user_presence(cx);
        self.rows = filtered
            .into_iter()
            .map(|f| FriendRow {
                id: f.id,
                group_name: SharedString::from(format!("friend-row-{}", f.id)),
                label: SharedString::from(f.label().to_string()),
                username: SharedString::from(f.username.clone()),
                avatar_src: SharedString::from(imgproxy::avatar_url(cx, &f.avatar_url)),
                avatar_raw: SharedString::from(f.avatar_url.clone()),
                online: presence.is_online(f.id),
                presence: presence.member_presence(f.id, own_presence),
                user_status: SharedString::from(
                    presence.user_status(f.id).unwrap_or("").to_string(),
                ),
                state: f.state,
            })
            .collect();

        self.pending_count = pending;
        self.list_header = SharedString::from(format!(
            "{} - {}",
            mezon_i18n::t(&locale, tab.title_key()).to_uppercase(),
            self.rows.len()
        ));
        cx.notify();
    }

    fn clear_transient_state(&mut self, cx: &mut Context<Self>) {
        let had_state =
            self.open_menu.is_some() || self.add_friend_open || self.add_error.is_some();
        self.open_menu = None;
        self.add_friend_open = false;
        self.add_error = None;
        if let Some(search) = self.search.clone() {
            search.update(cx, |input, cx| input.clear(cx));
        }
        if had_state {
            cx.notify();
        }
    }

    fn select_tab(&mut self, tab: FriendsTab, cx: &mut Context<Self>) {
        self.selected_tab = tab;
        self.add_friend_open = false;
        self.rebuild_friend_rows(cx);
    }

    fn submit_add_friend(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.add_input.clone() else {
            return;
        };
        let raw = input.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return;
        }
        let value: String = raw.chars().take(MAX_USERNAME_LEN).collect();
        if !is_valid_username(&value) {
            self.set_add_error("friendsPage.addFriendModal.invalidInput", cx);
            return;
        }

        let store = FriendStore::global(cx);
        if store.read(cx).is_blocked_by_me(&value, cx) {
            self.set_add_error("friendsPage.addFriendModal.blockedUser", cx);
            return;
        }

        let lower = value.to_lowercase();
        let existing = store
            .read(cx)
            .friends()
            .iter()
            .find(|f| f.username.to_lowercase() == lower)
            .map(|f| (f.id, f.state));

        if let Some((id, state)) = existing {
            match state {
                FriendState::InviteReceived => {
                    store.update(cx, |s, cx| s.accept_friend(id, cx));
                    self.clear_add_field(cx);
                }
                FriendState::InviteSent => {
                    self.set_add_error("friendsPage.addFriendModal.waitAccept", cx);
                }
                FriendState::Friend | FriendState::Blocked => {
                    self.set_add_error("friendsPage.addFriendModal.alreadyFriends", cx);
                }
            }
            return;
        }

        store.update(cx, |s, cx| s.add_friend_by_username(value, cx));
        self.clear_add_field(cx);
    }

    fn revalidate_add_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.add_input.clone() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        let key = if value.is_empty() {
            None
        } else if !is_valid_username(&value) {
            Some("friendsPage.addFriendModal.invalidInput")
        } else if FriendStore::global(cx)
            .read(cx)
            .is_blocked_by_me(&value, cx)
        {
            Some("friendsPage.addFriendModal.blockedUser")
        } else {
            None
        };
        let next = key.map(|key| {
            SharedString::from(mezon_i18n::t(&self.settings.read(cx).language, key).to_string())
        });
        if self.add_error != next {
            self.add_error = next;
            cx.notify();
        }
    }

    fn can_submit_add_friend(&self, cx: &App) -> bool {
        let Some(input) = self.add_input.as_ref() else {
            return false;
        };
        !input.read(cx).value().trim().is_empty()
            && self.add_error.is_none()
            && !FriendStore::global(cx).read(cx).is_adding()
    }

    fn friend_label(&self, user_id: UserId) -> SharedString {
        self.rows
            .iter()
            .find(|row| row.id == user_id)
            .map(|row| row.label.clone())
            .unwrap_or_default()
    }

    fn toast(&self, kind: ToastKind, key: &'static str, cx: &mut Context<Self>) {
        let message =
            SharedString::from(mezon_i18n::t(&self.settings.read(cx).language, key).to_string());
        Shell::global(cx).update(cx, |shell, cx| shell.toast(kind, message, cx));
    }

    fn set_add_error(&mut self, key: &'static str, cx: &mut Context<Self>) {
        self.add_error = Some(SharedString::from(
            mezon_i18n::t(&self.settings.read(cx).language, key).to_string(),
        ));
        cx.notify();
    }

    fn clear_add_field(&mut self, cx: &mut Context<Self>) {
        self.add_error = None;
        if let Some(input) = self.add_input.clone() {
            input.update(cx, |input, cx| input.clear(cx));
        }
        cx.notify();
    }
}

fn on_friends_route(cx: &App) -> bool {
    matches!(
        Router::global(cx).read(cx).route(),
        Route::Friends | Route::Direct
    )
}

fn empty_state_key(tab: FriendsTab, has_text: bool) -> &'static str {
    match (tab, has_text) {
        (FriendsTab::All, false) => "friendsPage.statusTapListFriends.all",
        (FriendsTab::Online, false) => "friendsPage.statusTapListFriends.online",
        (FriendsTab::Pending, false) => "friendsPage.statusTapListFriends.pending",
        (FriendsTab::Block, false) => "friendsPage.statusTapListFriends.block",
        (FriendsTab::All, true) => "friendsPage.statusTapSearchFriends.all",
        (FriendsTab::Online, true) => "friendsPage.statusTapSearchFriends.online",
        (FriendsTab::Pending, true) => "friendsPage.statusTapSearchFriends.pending",
        (FriendsTab::Block, true) => "friendsPage.statusTapSearchFriends.block",
    }
}

fn pending_rank(state: FriendState) -> u8 {
    match state {
        FriendState::InviteReceived => 2,
        FriendState::InviteSent => 1,
        _ => 0,
    }
}

/// Build the activity-sidebar rows: for each group (Work/Live/Play) a heading plus one row per
/// known user currently in that activity. Mirrors React `ActivityList` — activities are filtered to
/// the friend set, grouped by `activity_type`, and every group heading renders (with its count)
/// even when empty.
fn build_activity_rows(
    locale: &str,
    friends: &FriendStore,
    activities: Entity<ActivityStore>,
    cx: &App,
) -> Vec<ActivityRow> {
    let activities = activities.read(cx);
    let by_id: HashMap<UserId, &Friend> = friends.friends().iter().map(|f| (f.id, f)).collect();
    let mut rows = Vec::new();
    for (activity_type, key) in ACTIVITY_SECTIONS {
        let group: Vec<(&UserActivity, &Friend)> = activities
            .activities()
            .iter()
            .filter(|a| a.activity_type == activity_type && a.user_id != UserId(0))
            .filter_map(|a| by_id.get(&a.user_id).map(|f| (a, *f)))
            .collect();
        rows.push(ActivityRow::Header(SharedString::from(format!(
            "{} - {}",
            mezon_i18n::t(locale, key).to_uppercase(),
            group.len()
        ))));
        for (a, f) in group {
            let description = if a.activity_description.is_empty() {
                a.activity_name.clone()
            } else {
                a.activity_description.clone()
            };
            rows.push(ActivityRow::Item {
                label: SharedString::from(f.label().to_string()),
                description: SharedString::from(description),
                avatar_src: SharedString::from(imgproxy::avatar_url(cx, &f.avatar_url)),
                avatar_raw: SharedString::from(f.avatar_url.clone()),
            });
        }
    }
    rows
}

fn is_valid_username(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || ('+'..='_').contains(&c))
}

fn open_remove_friend_modal(
    user_id: UserId,
    username: SharedString,
    kind: FriendRemovalKind,
    locale: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    Shell::global(cx).update(cx, |shell, cx| {
        shell.confirm_remove_friend(user_id, &username, kind, &locale, window, cx);
    });
}

pub(crate) fn open_dm_with_user(user: UserId, error_message: SharedString, cx: &mut App) {
    let Some(store) = DirectMessageStore::try_global(cx) else {
        return;
    };
    let task = store.update(cx, |store, cx| {
        store.create_dm_with_user(user, String::new(), String::new(), String::new(), cx)
    });
    cx.spawn(async move |cx| match task.await {
        Ok((channel_id, channel_type)) => {
            cx.update(|cx| {
                navigate(
                    cx,
                    Route::DirectMessage {
                        direct_id: channel_id,
                        message_type: channel_type.to_string(),
                    },
                );
            });
        }
        Err(err) => {
            tracing::warn!("create DM from friends page failed: {err}");
            cx.update(|cx| {
                Shell::global(cx).update(cx, move |shell, cx| shell.error(error_message, cx));
            });
        }
    })
    .detach();
}

impl Render for FriendsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        let theme = cx.theme().clone();
        let locale = self.settings.read(cx).language.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .size_full()
            .bg(theme.surfaces.secondary.ramp())
            .child(self.render_header(&theme, &locale, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.render_main(&theme, &locale, cx))
                    .child(self.render_activity(&theme, cx)),
            )
            .when_some(self.open_menu, |el, (user_id, pos)| {
                el.child(context_menu_at(
                    pos,
                    self.build_friend_menu(user_id, &locale, cx),
                ))
            })
    }
}

impl FriendsPage {
    fn render_header(
        &self,
        theme: &Theme,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pending = self.pending_count;
        let selected = self.selected_tab;
        let add_open = self.add_friend_open;

        let tabs =
            div()
                .flex()
                .flex_row()
                .gap_4()
                .pr_4()
                .children(FriendsTab::ALL.into_iter().map(|tab| {
                    let is_active = selected == tab && !add_open;
                    let title = mezon_i18n::t(locale, tab.title_key());
                    let show_badge = tab == FriendsTab::Pending && pending != 0;
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(tab.elem_id())
                                .px_3()
                                .py(px(6.))
                                .rounded_lg()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_theme_primary)
                                .border_b_1()
                                .border_color(theme.tokens.border_primary)
                                .cursor_pointer()
                                .when(show_badge, |el| el.pr(px(30.)))
                                .when(is_active, |el| {
                                    el.bg(theme.tokens.bg_active_button)
                                        .text_color(theme.tokens.text_secondary)
                                })
                                .when(!is_active, |el| {
                                    el.hover(|s| {
                                        s.bg(theme.tokens.bg_tertiary)
                                            .text_color(theme.tokens.text_secondary)
                                    })
                                })
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.select_tab(tab, cx);
                                }))
                                .child(title),
                        )
                        .when(show_badge, |el| {
                            el.child(
                                crate::sidebar::friend_request_badge(pending, px(11.))
                                    .absolute()
                                    .top(px(2.))
                                    .right(px(12.)),
                            )
                        })
                }));

        let add_button = div()
            .id("friend-add")
            .px_2()
            .py(px(6.))
            .rounded_lg()
            .whitespace_nowrap()
            .font_weight(gpui::FontWeight::MEDIUM)
            .when(add_open, |el| {
                el.bg(theme.tokens.bg_button_add_friend_active)
                    .text_color(theme.tokens.text_button_add_friend_active)
                    .cursor_default()
            })
            .when(!add_open, |el| {
                el.bg(theme.tokens.button_theme_primary)
                    .text_color(gpui::white())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.tokens.bg_button_primary_hover))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.add_friend_open = true;
                        cx.notify();
                    }))
            })
            .child(mezon_i18n::t(locale, "friendsPage.addFriend"));

        div()
            .flex()
            .items_center()
            .justify_start()
            .h(px(APP_HEADER_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .px_6()
            .py_3()
            .border_b_1()
            .border_color(theme.tokens.border_primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.tokens.text_secondary)
                            .child(img("icons/icon-friends.svg").size(px(20.)).flex_none())
                            .child(mezon_i18n::t(locale, "friendsPage.friends")),
                    )
                    .child(
                        div()
                            .size(px(4.))
                            .rounded_full()
                            .bg(theme.tokens.text_theme_primary),
                    )
                    .child(tabs)
                    .child(add_button),
            )
    }

    fn render_main(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let main = div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_col()
            .bg(theme.surfaces.secondary.ramp());

        if self.add_friend_open {
            main.child(self.render_add_friend(theme, locale, cx))
        } else {
            main.child(self.render_search(theme, cx))
                .child(self.render_list(theme, locale, cx))
        }
    }

    fn render_search(&self, theme: &Theme, cx: &Context<Self>) -> impl IntoElement {
        let has_text = self.search_has_text(cx);

        let mut search_field = div()
            .relative()
            .w_full()
            .h(px(44.))
            .mb_6()
            .rounded_lg()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.surfaces.primary)
            .flex()
            .items_center()
            .px_3();
        if let Some(search) = self.search.as_ref() {
            search_field = search_field.child(
                Input::new(search)
                    .w_full()
                    .pr(px(48.))
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_primary),
            );
        }
        search_field = search_field
            .when(has_text, |el| {
                el.child(
                    div()
                        .id("friend-search-clear")
                        .absolute()
                        .top(px(10.))
                        .right(px(48.))
                        .px_2()
                        .text_size(px(25.))
                        .text_color(theme.tokens.text_theme_primary)
                        .cursor_pointer()
                        .hover(|s| s.text_color(rgb(COLOR_DANGER)))
                        .on_click(cx.listener(|this, _, _window, cx| {
                            if let Some(search) = this.search.clone() {
                                search.update(cx, |input, cx| input.clear(cx));
                            }
                            this.rebuild_friend_rows(cx);
                        }))
                        .child("×"),
                )
            })
            .child(
                div().absolute().top(px(12.)).right(px(20.)).child(
                    Icon::new(IconName::Search)
                        .size_4()
                        .text_color(theme.tokens.text_theme_primary),
                ),
            );

        div()
            .flex()
            .flex_col()
            .px_8()
            .pt_6()
            .text_color(theme.tokens.text_theme_primary)
            .child(search_field)
            .child(
                div()
                    .px(px(14.))
                    .mb_4()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(self.list_header.clone()),
            )
    }

    fn render_list(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let container = div().flex().flex_1().min_h_0().px_8().pb_4();

        if self.rows.is_empty() {
            let has_text = self.search_has_text(cx);
            let key = empty_state_key(self.selected_tab, has_text);
            return container.child(
                div()
                    .flex()
                    .w_full()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .h_full()
                    .text_color(theme.tokens.text_theme_primary)
                    .child(
                        div()
                            .w_2_3()
                            .flex()
                            .justify_center()
                            .text_center()
                            .mb(px(120.))
                            .child(mezon_i18n::t(locale, key).to_string()),
                    ),
            );
        }

        let entity = cx.entity();
        let theme = cx.theme().clone();
        let count = self.rows.len();
        let avatar_cache = self.avatar_cache.clone();
        let locale = SharedString::from(locale.to_string());
        let list = uniform_list("friends-list", count, move |range, _window, cx| {
            let rows = &entity.read(cx).rows;
            range
                .map(|ix| match rows.get(ix) {
                    Some(row) => {
                        render_friend_row(&theme, row, &avatar_cache, entity.clone(), &locale)
                    }
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .suppress_hover_while_scrolling()
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        container.child(list)
    }

    fn render_activity(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let avatar_cache = self.avatar_cache.clone();
        let count = self.activity_rows.len();
        let bg = theme.surfaces.active_friend_list.ramp();
        let row_theme = cx.theme().clone();
        let list = uniform_list("friends-activity-list", count, move |range, _window, cx| {
            let rows = &entity.read(cx).activity_rows;
            range
                .map(|ix| match rows.get(ix) {
                    Some(row) => render_activity_row(&row_theme, row, &avatar_cache),
                    None => div().into_any_element(),
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.activity_scroll)
        .flex_1()
        .min_h_0();

        div()
            .flex()
            .flex_col()
            .w(px(ACTIVITY_WIDTH))
            .max_w(relative(0.4))
            .h_full()
            .flex_shrink_0()
            .bg(bg)
            .child(list)
    }

    fn build_friend_menu(&self, user_id: UserId, locale: &str, cx: &Context<Self>) -> ContextMenu {
        let t = |key: &'static str| mezon_i18n::t(locale, key).to_string();
        let panel = cx.entity().downgrade();
        let dismiss = {
            let panel = panel.clone();
            move |_window: &mut Window, cx: &mut App| {
                if let Some(p) = panel.upgrade() {
                    p.update(cx, |this, cx| {
                        this.open_menu = None;
                        cx.notify();
                    });
                }
            }
        };
        let coming_soon = {
            let settings = self.settings.clone();
            move |_window: &mut Window, cx: &mut App| {
                let msg =
                    mezon_i18n::t(&settings.read(cx).language, "common.comingSoon").to_string();
                Shell::global(cx).update(cx, |shell, cx| shell.info(msg, cx));
            }
        };

        let confirm_remove = {
            let username = self.friend_label(user_id);
            let locale = SharedString::from(locale.to_string());
            move |window: &mut Window, cx: &mut App| {
                open_remove_friend_modal(
                    user_id,
                    username.clone(),
                    FriendRemovalKind::RemoveFriend,
                    locale.clone(),
                    window,
                    cx,
                );
            }
        };

        ContextMenu::new()
            .on_dismiss(dismiss)
            .item(
                t("friendsPage.friendMenu.startVideoCall"),
                coming_soon.clone(),
            )
            .item(
                t("friendsPage.friendMenu.startVoiceCall"),
                coming_soon.clone(),
            )
            .item(t("contextMenu.member.shareContact"), {
                let panel = panel.clone();
                let settings = self.settings.clone();
                move |window, cx| {
                    let Some(friend) = FriendStore::global(cx).read(cx).friend(user_id) else {
                        return;
                    };
                    let contact = share_contact_subject(user_id, &friend.display_name, None, cx);
                    let locale = settings.read(cx).language.clone().into();
                    ShareContactModal::open(contact, locale, window, cx);
                    if let Some(p) = panel.upgrade() {
                        p.update(cx, |this, cx| {
                            this.open_menu = None;
                            cx.notify();
                        });
                    }
                }
            })
            .danger_item(t("friendsPage.friendMenu.removeFriend"), confirm_remove)
            .danger_item(t("friendsPage.friendMenu.block"), move |_window, cx| {
                FriendStore::global(cx).update(cx, |s, cx| s.block_friend(user_id, cx));
            })
    }

    fn render_add_friend(
        &self,
        theme: &Theme,
        locale: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_error = self.add_error.is_some();
        let mut input_wrap = div()
            .relative()
            .w_full()
            .mb_2()
            .mt_1()
            .rounded_lg()
            .bg(theme.tokens.bg_input_secondary)
            .flex()
            .items_center()
            .py_3()
            .px_3()
            .when(has_error, |el| {
                el.border_1().border_color(rgb(COLOR_DANGER))
            });
        if let Some(add_input) = self.add_input.as_ref() {
            input_wrap = input_wrap.child(
                Input::new(add_input)
                    .w_full()
                    .pr(px(140.))
                    .text_size(px(16.))
                    .text_color(theme.tokens.text_theme_primary),
            );
        }
        let can_submit = self.can_submit_add_friend(cx);
        input_wrap = input_wrap.child(
            div()
                .id("friend-add-send")
                .absolute()
                .right(px(8.))
                .px_2()
                .py(px(5.))
                .min_w(px(130.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .text_size(px(14.))
                .bg(theme.tokens.button_theme_primary)
                .text_color(gpui::white())
                .map(|el| {
                    if can_submit {
                        el.cursor_pointer()
                            .hover(|s| s.bg(theme.tokens.bg_button_primary_hover))
                            .on_click(
                                cx.listener(|this, _, _window, cx| this.submit_add_friend(cx)),
                            )
                    } else {
                        el.cursor_not_allowed().opacity(0.5)
                    }
                })
                .child(mezon_i18n::t(locale, "friendsPage.addFriendModal.sendRequest").to_string()),
        );

        div()
            .p_8()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.tokens.border_primary)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.tokens.text_secondary)
                            .child(
                                mezon_i18n::t(locale, "friendsPage.addFriendModal.title")
                                    .to_string(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.tokens.text_theme_primary)
                            .child(
                                mezon_i18n::t(locale, "friendsPage.addFriendModal.description")
                                    .to_string(),
                            ),
                    )
                    .child(input_wrap)
                    .when_some(self.add_error.clone(), |el, err| {
                        el.child(
                            div()
                                .text_size(px(14.))
                                .pb_5()
                                .text_color(rgb(COLOR_DANGER))
                                .child(err),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(28.))
                    .pt_8()
                    .child(
                        div().text_color(theme.tokens.text_theme_primary).child(
                            mezon_i18n::t(locale, "friendsPage.addFriendModal.waitingMessage")
                                .to_string(),
                        ),
                    ),
            )
    }
}

fn render_friend_row(
    theme: &Theme,
    row: &FriendRow,
    avatar_cache: &Entity<LruImageCache>,
    entity: Entity<FriendsPage>,
    locale: &str,
) -> gpui::AnyElement {
    let id = row.id;
    let state = row.state;

    let mut avatar = Avatar::new()
        .name(row.label.clone())
        .size_px(px(AVATAR_SIZE))
        .image_cache(avatar_cache.clone());
    if !row.avatar_src.is_empty() {
        avatar = avatar.src(row.avatar_src.clone());
        if !row.avatar_raw.is_empty() && row.avatar_raw != row.avatar_src {
            avatar = avatar.fallback_src(row.avatar_raw.clone());
        }
    } else if !row.avatar_raw.is_empty() {
        avatar = avatar.src(row.avatar_raw.clone());
    }

    let presence_badge: Option<AnyElement> = match row.presence {
        DmAvatarPresence::None => None,
        DmAvatarPresence::Idle => Some(
            div()
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .size(px(10.))
                .child(
                    Icon::new(IconName::DarkModeIcon)
                        .size(px(10.))
                        .with_transformation(gpui::Transformation::rotate(gpui::radians(
                            -std::f32::consts::FRAC_PI_2,
                        )))
                        .text_color(
                            crate::util::user_status::presence_badge_color(DmAvatarPresence::Idle)
                                .unwrap_or(theme.status_idle),
                        ),
                )
                .into_any_element(),
        ),
        DmAvatarPresence::Online | DmAvatarPresence::Dnd => Some(
            div()
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .size(px(10.))
                .rounded_full()
                .border_2()
                .border_color(theme.tokens.bg_secondary)
                .bg(crate::util::user_status::presence_badge_color(row.presence)
                    .unwrap_or(theme.status_online))
                .into_any_element(),
        ),
    };
    let avatar_slot = div()
        .relative()
        .flex_shrink_0()
        .size(px(AVATAR_SIZE))
        .child(avatar)
        .children(presence_badge);

    let group_name = row.group_name.clone();
    let mut name_line = div()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(16.))
        .text_color(theme.tokens.text_theme_primary)
        .child(div().truncate().child(row.label.clone()));
    if !row.username.is_empty() {
        name_line = name_line.child(
            div()
                .id(("friend-username", id.get() as usize))
                .invisible()
                .group_hover(group_name.clone(), |s| s.visible())
                .hover(|s| s.text_color(theme.tokens.text_secondary))
                .child(row.username.clone()),
        );
    }

    let mut name_col = div().flex().flex_col().min_w_0().flex_1().child(name_line);
    if !row.user_status.is_empty() {
        let mut status_color = theme.tokens.text_theme_primary;
        status_color.a *= 0.6;
        name_col = name_col.child(
            div()
                .text_size(px(11.))
                .text_color(status_color)
                .truncate()
                .child(row.user_status.clone()),
        );
    }

    let profile = div()
        .flex()
        .flex_1()
        .min_w_0()
        .pr_2()
        .items_center()
        .gap_3()
        .child(avatar_slot)
        .child(name_col);

    let actions = render_row_actions(theme, id, state, entity.clone(), locale, row.label.clone());

    let clickable_open_dm = !matches!(state, FriendState::InviteSent | FriendState::InviteReceived);

    let inner = div()
        .id(("friend-open", id.get() as usize))
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .justify_between()
        .h_full()
        .px_3()
        .rounded_lg()
        .cursor_pointer()
        .hover(|s| s.bg(theme.tokens.bg_item_hover))
        .child(profile)
        .child(actions)
        .when(clickable_open_dm, |el| {
            let err = SharedString::from(mezon_i18n::t(locale, "shareContact.card.messageError"));
            el.on_click(move |_, _window, cx| {
                open_dm_with_user(id, err.clone(), cx);
            })
        });

    div()
        .group(group_name)
        .flex()
        .items_center()
        .h(px(ROW_HEIGHT))
        .w_full()
        .border_t_1()
        .border_color(theme.tokens.border_primary)
        .text_color(theme.tokens.text_theme_primary)
        .child(inner)
        .into_any_element()
}

fn render_activity_row(
    theme: &Theme,
    row: &ActivityRow,
    avatar_cache: &Entity<LruImageCache>,
) -> gpui::AnyElement {
    match row {
        ActivityRow::Header(title) => div()
            .flex()
            .items_center()
            .w_full()
            .px_4()
            .h(px(48.))
            .truncate()
            .text_size(px(14.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.tokens.text_theme_primary)
            .child(title.clone())
            .into_any_element(),
        ActivityRow::Item {
            label,
            description,
            avatar_src,
            avatar_raw,
        } => {
            let mut avatar = Avatar::new()
                .name(label.clone())
                .size_px(px(AVATAR_SIZE))
                .image_cache(avatar_cache.clone());
            if !avatar_src.is_empty() {
                avatar = avatar.src(avatar_src.clone());
                if !avatar_raw.is_empty() && avatar_raw != avatar_src {
                    avatar = avatar.fallback_src(avatar_raw.clone());
                }
            } else if !avatar_raw.is_empty() {
                avatar = avatar.src(avatar_raw.clone());
            }

            let mut desc_color = theme.tokens.text_theme_primary;
            desc_color.a *= 0.6;

            div()
                .flex()
                .items_center()
                .w_full()
                .gap(px(9.))
                .px_4()
                .h(px(48.))
                .overflow_hidden()
                .child(div().flex_shrink_0().size(px(AVATAR_SIZE)).child(avatar))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.tokens.text_theme_primary)
                                .truncate()
                                .child(label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(desc_color)
                                .truncate()
                                .child(description.clone()),
                        ),
                )
                .into_any_element()
        }
    }
}

fn render_row_actions(
    theme: &Theme,
    id: UserId,
    state: FriendState,
    entity: Entity<FriendsPage>,
    locale: &str,
    label: SharedString,
) -> gpui::AnyElement {
    let locale_owned = SharedString::from(locale.to_string());
    match state {
        FriendState::Friend => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                round_chat_button(("friend-chat", id.get() as usize), theme).on_click({
                    let err =
                        SharedString::from(mezon_i18n::t(locale, "shareContact.card.messageError"));
                    move |_, _window, cx| {
                        open_dm_with_user(id, err.clone(), cx);
                    }
                }),
            )
            .child(
                round_button(
                    ("friend-more", id.get() as usize),
                    IconName::IconEditThreeDot,
                    theme,
                )
                .on_click({
                    let entity = entity.clone();
                    move |event: &ClickEvent, _window, cx| {
                        let pos = event.position();
                        entity.update(cx, |this, cx| {
                            this.open_menu = Some((id, pos));
                            cx.notify();
                        });
                    }
                }),
            )
            .into_any_element(),
        FriendState::InviteSent => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                circle_button(("friend-cancel", id.get() as usize), "✕", theme).on_click(
                    move |_, window, cx| {
                        open_remove_friend_modal(
                            id,
                            label.clone(),
                            FriendRemovalKind::CancelRequest,
                            locale_owned.clone(),
                            window,
                            cx,
                        );
                    },
                ),
            )
            .into_any_element(),
        FriendState::InviteReceived => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                circle_button(("friend-accept", id.get() as usize), "✓", theme).on_click(
                    move |_, _window, cx| {
                        FriendStore::global(cx).update(cx, |s, cx| s.accept_friend(id, cx));
                    },
                ),
            )
            .child(
                circle_button(("friend-reject", id.get() as usize), "✕", theme).on_click(
                    move |_, window, cx| {
                        open_remove_friend_modal(
                            id,
                            label.clone(),
                            FriendRemovalKind::RejectRequest,
                            locale_owned.clone(),
                            window,
                            cx,
                        );
                    },
                ),
            )
            .into_any_element(),
        FriendState::Blocked => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .id(("friend-unblock", id.get() as usize))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .rounded(px(6.))
                    .text_size(px(14.))
                    .bg(theme.tokens.bg_tertiary)
                    .text_color(theme.tokens.text_secondary)
                    .cursor_pointer()
                    .occlude()
                    .hover(|s| s.bg(theme.surfaces.primary))
                    .on_click(move |_, _window, cx| {
                        FriendStore::global(cx).update(cx, |s, cx| s.unblock_friend(id, cx));
                    })
                    .child(mezon_i18n::t(locale, "friendsPage.friendMenu.unblock").to_string()),
            )
            .into_any_element(),
    }
}

fn round_button(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .rounded_full()
        .cursor_pointer()
        .occlude()
        .hover(|s| s.bg(theme.tokens.bg_secondary_button_hover))
        .child(
            Icon::new(icon)
                .size_4()
                .text_color(theme.tokens.text_theme_primary),
        )
}

fn round_chat_button(id: impl Into<gpui::ElementId>, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .rounded_full()
        .cursor_pointer()
        .occlude()
        .hover(|s| s.bg(theme.tokens.bg_secondary_button_hover))
        .child(crate::chat::voice::chat_toggle_icon(
            theme.tokens.text_theme_primary,
            px(16.),
        ))
}

fn circle_button(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_full()
        .text_color(theme.tokens.text_theme_primary)
        .cursor_pointer()
        .occlude()
        .hover(|s| s.bg(theme.tokens.bg_secondary_button_hover))
        .child(glyph)
}
