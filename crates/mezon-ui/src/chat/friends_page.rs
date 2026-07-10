use gpui::{
    App, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement, Pixels,
    Point, SharedString, StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle,
    Window, div, img, prelude::*, px, relative, rgb, uniform_list,
};
use mezon_store::activity::{ACTIVITY_TYPE_LIVE, ACTIVITY_TYPE_PLAY, ACTIVITY_TYPE_WORK};
use mezon_store::{
    ActivityEvent, ActivityStore, BadgeService, DirectMessageStore, Friend, FriendEvent,
    FriendState, FriendStore, PresenceEvent, PresenceStore, Settings, UserActivity, UserId,
};

use crate::app::shell::Shell;
use crate::app::window_controls::APP_HEADER_HEIGHT;
use crate::components::primitives::{
    Avatar, ContextMenu, Icon, IconName, Input, InputEvent, InputState, context_menu_at,
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

    fn key(self) -> &'static str {
        match self {
            FriendsTab::All => "all",
            FriendsTab::Online => "online",
            FriendsTab::Pending => "pending",
            FriendsTab::Block => "block",
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
    label: SharedString,
    username: SharedString,
    avatar_src: SharedString,
    avatar_raw: SharedString,
    online: bool,
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
    list_scroll: UniformListScrollHandle,
    activity_scroll: UniformListScrollHandle,
    avatar_cache: Entity<LruImageCache>,
    open_menu: Option<(UserId, Point<Pixels>)>,
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
                FriendEvent::AddFailed => {
                    this.set_add_error("friendsPage.requestFailedPopup.title", cx);
                }
            }),
        );
        subs.push(cx.subscribe(
            &PresenceStore::global(cx),
            |this, _, event, cx| match event {
                PresenceEvent::ChannelPresenceChanged { .. } | PresenceEvent::StatusChanged => {
                    if on_friends_route(cx) {
                        this.rebuild(cx);
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
                        this.rebuild(cx);
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
        subs.push(cx.observe(&settings, |_, _, cx| cx.notify()));

        FriendStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));
        ActivityStore::global(cx).update(cx, |store, cx| store.ensure_loaded(cx));

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
            list_scroll: UniformListScrollHandle::new(),
            activity_scroll: UniformListScrollHandle::new(),
            avatar_cache,
            open_menu: None,
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
                        this.rebuild(cx);
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
            });
            self._subs.push(cx.subscribe(
                &add_input,
                |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        if this.add_error.take().is_some() {
                            cx.notify();
                        }
                    }
                    InputEvent::PressEnter => this.submit_add_friend(cx),
                },
            ));
            self.add_input = Some(add_input);
        }
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

        self.rows = filtered
            .into_iter()
            .map(|f| FriendRow {
                id: f.id,
                label: SharedString::from(f.label().to_string()),
                username: SharedString::from(f.username.clone()),
                avatar_src: SharedString::from(imgproxy::avatar_url(cx, &f.avatar_url)),
                avatar_raw: SharedString::from(f.avatar_url.clone()),
                online: presence.is_online(f.id),
                user_status: SharedString::from(
                    presence.user_status(f.id).unwrap_or("").to_string(),
                ),
                state: f.state,
            })
            .collect();

        self.activity_rows = build_activity_rows(&locale, friends, ActivityStore::global(cx), cx);

        self.pending_count = pending;
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
        self.rebuild(cx);
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
            self.add_error = Some(SharedString::from(
                mezon_i18n::t(
                    &self.settings.read(cx).language,
                    "friendsPage.addFriendModal.invalidInput",
                )
                .to_string(),
            ));
            cx.notify();
            return;
        }

        let lower = value.to_lowercase();
        let store = FriendStore::global(cx);
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
                FriendState::Blocked => {
                    self.set_add_error("friendsPage.addFriendModal.blockedUser", cx);
                }
                FriendState::InviteSent => {
                    self.set_add_error("friendsPage.addFriendModal.waitAccept", cx);
                }
                FriendState::Friend => {
                    self.set_add_error("friendsPage.addFriendModal.alreadyFriends", cx);
                }
            }
            return;
        }

        store.update(cx, |s, cx| s.add_friend_by_username(value, cx));
        self.clear_add_field(cx);
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
    let mut rows = Vec::new();
    for (activity_type, key) in ACTIVITY_SECTIONS {
        let group: Vec<&UserActivity> = activities
            .activities()
            .iter()
            .filter(|a| {
                a.activity_type == activity_type
                    && a.user_id != UserId(0)
                    && friends.friend(a.user_id).is_some()
            })
            .collect();
        rows.push(ActivityRow::Header(SharedString::from(format!(
            "{} - {}",
            mezon_i18n::t(locale, key).to_uppercase(),
            group.len()
        ))));
        for a in group {
            let Some(f) = friends.friend(a.user_id) else {
                continue;
            };
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

fn open_dm_with_user(user: UserId, error_message: SharedString, cx: &mut App) {
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
            .bg(theme.tokens.bg_secondary)
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
                    let title = mezon_i18n::t(locale, tab.title_key()).to_string();
                    let show_badge = tab == FriendsTab::Pending && pending != 0;
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(SharedString::from(format!("friend-tab-{}", tab.key())))
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
                                div()
                                    .absolute()
                                    .top(px(2.))
                                    .right(px(12.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(20.))
                                    .rounded_full()
                                    .bg(rgb(COLOR_DANGER))
                                    .text_size(px(10.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.tokens.text_theme_primary)
                                    .child(pending.to_string()),
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
            .child(mezon_i18n::t(locale, "friendsPage.addFriend").to_string());

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
                            .child(mezon_i18n::t(locale, "friendsPage.friends").to_string()),
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
            .bg(theme.tokens.bg_secondary);

        if self.add_friend_open {
            main.child(self.render_add_friend(theme, locale, cx))
        } else {
            main.child(self.render_search(theme, locale, cx))
                .child(self.render_list(theme, locale, cx))
        }
    }

    fn render_search(&self, theme: &Theme, locale: &str, cx: &Context<Self>) -> impl IntoElement {
        let count = self.rows.len();
        let tab_title = mezon_i18n::t(locale, self.selected_tab.title_key()).to_uppercase();
        let has_text = !self.search_text(cx).is_empty();

        let mut search_field = div()
            .relative()
            .w_full()
            .h(px(44.))
            .mb_6()
            .rounded_lg()
            .border_1()
            .border_color(theme.tokens.border_primary)
            .bg(theme.tokens.bg_primary)
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
                            this.rebuild(cx);
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
                    .child(format!("{tab_title} - {count}")),
            )
    }

    fn render_list(&self, theme: &Theme, locale: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let container = div().flex().flex_1().min_h_0().px_8().pb_4();

        if self.rows.is_empty() {
            let has_text = !self.search_text(cx).is_empty();
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
        let theme = theme.clone();
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
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        container.child(list)
    }

    fn render_activity(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let avatar_cache = self.avatar_cache.clone();
        let count = self.activity_rows.len();
        let bg = theme.tokens.bg_active_friend_list;
        let row_theme = theme.clone();
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

        ContextMenu::new()
            .on_dismiss(dismiss)
            .item(
                t("friendsPage.friendMenu.startVideoCall"),
                coming_soon.clone(),
            )
            .item(t("friendsPage.friendMenu.startVoiceCall"), coming_soon)
            .danger_item(
                t("friendsPage.friendMenu.removeFriend"),
                move |_window, cx| {
                    FriendStore::global(cx).update(cx, |s, cx| s.delete_friend(user_id, cx));
                },
            )
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
                .cursor_pointer()
                .hover(|s| s.bg(theme.tokens.bg_button_primary_hover))
                .on_click(cx.listener(|this, _, _window, cx| this.submit_add_friend(cx)))
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

    let dot_fill = if row.online {
        theme.status_online
    } else {
        theme.text_muted
    };
    let avatar_slot = div()
        .relative()
        .flex_shrink_0()
        .size(px(AVATAR_SIZE))
        .child(avatar)
        .child(
            div()
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .size(px(10.))
                .rounded_full()
                .border_2()
                .border_color(theme.tokens.bg_secondary)
                .bg(dot_fill),
        );

    let group_name = SharedString::from(format!("friend-row-{}", id));
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
                .id(SharedString::from(format!("friend-username-{}", id)))
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

    let actions = render_row_actions(theme, id, state, entity.clone(), locale);

    let clickable_open_dm = !matches!(state, FriendState::InviteSent | FriendState::InviteReceived);

    let inner = div()
        .id(SharedString::from(format!("friend-open-{}", id)))
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
            .px_4()
            .h(px(48.))
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
                .gap(px(9.))
                .px_4()
                .h(px(48.))
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
) -> gpui::AnyElement {
    match state {
        FriendState::Friend => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                round_button(
                    SharedString::from(format!("friend-chat-{}", id)),
                    IconName::Chat,
                    theme,
                )
                .on_click({
                    let err =
                        SharedString::from(mezon_i18n::t(locale, "shareContact.card.messageError"));
                    move |_, _window, cx| {
                        open_dm_with_user(id, err.clone(), cx);
                    }
                }),
            )
            .child(
                round_button(
                    SharedString::from(format!("friend-more-{}", id)),
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
                circle_button(
                    SharedString::from(format!("friend-cancel-{}", id)),
                    "✕",
                    theme,
                )
                .on_click(move |_, _window, cx| {
                    FriendStore::global(cx).update(cx, |s, cx| s.delete_friend(id, cx));
                }),
            )
            .into_any_element(),
        FriendState::InviteReceived => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                circle_button(
                    SharedString::from(format!("friend-accept-{}", id)),
                    "✓",
                    theme,
                )
                .on_click(move |_, _window, cx| {
                    FriendStore::global(cx).update(cx, |s, cx| s.accept_friend(id, cx));
                }),
            )
            .child(
                circle_button(
                    SharedString::from(format!("friend-reject-{}", id)),
                    "✕",
                    theme,
                )
                .on_click(move |_, _window, cx| {
                    FriendStore::global(cx).update(cx, |s, cx| s.delete_friend(id, cx));
                }),
            )
            .into_any_element(),
        FriendState::Blocked => div()
            .flex()
            .items_center()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .id(SharedString::from(format!("friend-unblock-{}", id)))
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
                    .hover(|s| s.bg(theme.tokens.bg_primary))
                    .on_click(move |_, _window, cx| {
                        FriendStore::global(cx).update(cx, |s, cx| s.unblock_friend(id, cx));
                    })
                    .child(mezon_i18n::t(locale, "friendsPage.friendMenu.unblock").to_string()),
            )
            .into_any_element(),
    }
}

fn round_button(id: SharedString, icon: IconName, theme: &Theme) -> gpui::Stateful<gpui::Div> {
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

fn circle_button(
    id: SharedString,
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
