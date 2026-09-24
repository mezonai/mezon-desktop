use crate::ids::UserId;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};

use crate::Freshness;
use crate::clan_members::{User, user_from_api};
use crate::ctrlk_search::CtrlKSearchType;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

#[derive(Debug, Clone)]
pub enum UsersByUserEvent {
    Changed,
    SearchSettled,
}

pub struct UsersByUserStore {
    by_id: HashMap<UserId, User>,
    loading: bool,
    freshness: Freshness,
    api: Arc<AppApi>,
    search_generation: u64,
    search_query: String,
    search_hits: Vec<UserId>,
    _search_task: Task<()>,
    _conn_watch: Task<()>,
}

struct GlobalUsersByUserStore(Entity<UsersByUserStore>);
impl Global for GlobalUsersByUserStore {}

impl EventEmitter<UsersByUserEvent> for UsersByUserStore {}

impl UsersByUserStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalUsersByUserStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalUsersByUserStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalUsersByUserStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_id.clear();
        self.search_hits.clear();
        self.search_query.clear();
        self.search_generation = self.search_generation.wrapping_add(1);
        self.loading = false;
        self.freshness.mark_stale();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            by_id: HashMap::new(),
            loading: false,
            freshness: Freshness::new(),
            api,
            search_generation: 0,
            search_query: String::new(),
            search_hits: Vec::new(),
            _search_task: Task::ready(()),
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::AddClanUser, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| this.refresh(cx));
        });
    }

    fn spawn_connection_watch(api: Arc<AppApi>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut status_rx = api.status();
            let mut was_connected = false;
            loop {
                if status_rx.changed().await.is_err() {
                    break;
                }
                let connected = *status_rx.borrow() == ConnectionStatus::Connected;
                if connected && !was_connected {
                    was_connected = true;
                    if this.update(cx, |this, _| this.invalidate()).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn user(&self, user_id: UserId) -> Option<&User> {
        self.by_id.get(&user_id)
    }

    pub fn users(&self) -> impl Iterator<Item = &User> + '_ {
        self.by_id.values()
    }

    pub fn count(&self) -> usize {
        self.by_id.len()
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.freshness.is_fresh(crate::CACHE_TTL) && !self.loading {
            self.fetch(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    pub fn search_hits(&self) -> (&str, &[UserId]) {
        (&self.search_query, &self.search_hits)
    }

    pub fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        self.search_generation = self.search_generation.wrapping_add(1);
        self._search_task = Task::ready(());
        if query.is_empty() {
            if !self.search_hits.is_empty() || !self.search_query.is_empty() {
                self.search_hits.clear();
                self.search_query.clear();
                cx.emit(UsersByUserEvent::SearchSettled);
            }
            return;
        }
        if query == self.search_query {
            return;
        }
        let generation = self.search_generation;
        let api = self.api.clone();
        tracing::debug!(query, generation, "user search sent");
        self._search_task = cx.spawn(async move |this, cx| {
            let result = api
                .search_ctrl_k(&query, CtrlKSearchType::Users.as_raw())
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_search_result(generation, query, result, cx)
            });
        });
    }

    fn apply_search_result(
        &mut self,
        generation: u64,
        query: String,
        result: anyhow::Result<mezon_proto::api::SearchCtrlKResponse>,
        cx: &mut Context<Self>,
    ) {
        if self.search_generation != generation {
            tracing::debug!(query, generation, "user search result ignored");
            return;
        }
        match result {
            Ok(response) => {
                tracing::debug!(
                    query,
                    generation,
                    hits = response.users.len(),
                    "user search result applied"
                );
                self.merge_users(query, response.users, cx)
            }
            Err(e) => tracing::warn!("SearchCtrlK for users failed: {e}"),
        }
    }

    fn merge_users(
        &mut self,
        query: String,
        users: Vec<mezon_proto::api::User>,
        cx: &mut Context<Self>,
    ) {
        let hits: Vec<UserId> = users
            .iter()
            .filter(|user| user.id != 0)
            .map(|user| UserId(user.id))
            .collect();
        let hits_changed = self.search_hits != hits || self.search_query != query;
        if hits_changed {
            self.search_hits = hits;
            self.search_query = query;
        }
        let mut users_changed = false;
        for user in users.into_iter().filter_map(user_from_api) {
            match self.by_id.get_mut(&user.id) {
                Some(existing) => {
                    if existing.username != user.username
                        || existing.display_name != user.display_name
                        || existing.avatar_url != user.avatar_url
                    {
                        existing.username = user.username;
                        existing.display_name = user.display_name;
                        existing.avatar_url = user.avatar_url;
                        users_changed = true;
                    }
                }
                None => {
                    self.by_id.insert(user.id, user);
                    users_changed = true;
                }
            }
        }
        tracing::debug!(users_changed, hits_changed, "user search merged");
        if users_changed {
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
        if hits_changed {
            cx.emit(UsersByUserEvent::SearchSettled);
        }
    }

    fn invalidate(&mut self) {
        self.freshness.mark_stale();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_user_clans_by_user().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(users) => {
                        this.freshness.mark_fetched();
                        for user in users {
                            if let Some(user) = user_from_api(user) {
                                this.by_id.insert(user.id, user);
                            }
                        }
                        tracing::info!("UsersByUserStore: loaded {} users", this.by_id.len());
                        cx.emit(UsersByUserEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_user_clans_by_user failed: {e}"),
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        if let RealtimeEvent::AddClanUser(e) = event {
            let Some(redis) = e.user.as_ref() else {
                return;
            };
            if redis.user_id == 0 {
                return;
            }
            let user = User {
                id: UserId(redis.user_id),
                username: redis.username.clone(),
                display_name: redis.display_name.clone(),
                avatar_url: redis.avatar.clone(),
                about_me: String::new(),
                create_time_seconds: redis.create_time_second,
                join_time_seconds: 0,
            };
            self.by_id.insert(user.id, user);
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::api;

    #[test]
    fn user_from_api_maps_fields() {
        let user = user_from_api(api::User {
            id: 5,
            username: "carol".into(),
            display_name: "Carol".into(),
            avatar_url: "c.png".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(user.id, UserId(5));
        assert_eq!(user.username, "carol");
        assert_eq!(user.display_name, "Carol");
        assert_eq!(user.avatar_url, "c.png");
    }

    #[test]
    fn user_from_api_skips_zero_id() {
        assert!(user_from_api(api::User::default()).is_none());
    }

    fn init_store(cx: &mut App) -> Entity<UsersByUserStore> {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        cx.new(|cx| UsersByUserStore::new(api, cx))
    }

    fn hit(id: i64, username: &str, display_name: &str) -> api::User {
        api::User {
            id,
            username: username.into(),
            display_name: display_name.into(),
            avatar_url: format!("{username}.png"),
            ..Default::default()
        }
    }

    #[gpui::test]
    fn search_hits_are_cached_as_users_and_remembered_as_the_last_result(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.merge_users(
                    "token".into(),
                    vec![hit(7, "token-bot", "Token Bot"), hit(0, "", "")],
                    cx,
                );
                assert_eq!(store.search_hits(), ("token", &[UserId(7)][..]));
                assert_eq!(
                    store.user(UserId(7)).map(|u| u.username.as_str()),
                    Some("token-bot")
                );

                store.merge_users("oth".into(), vec![hit(9, "other", "Other")], cx);
                assert_eq!(store.search_hits(), ("oth", &[UserId(9)][..]));
                assert_eq!(store.count(), 2);
            });
        });
    }

    #[gpui::test]
    fn a_search_hit_refreshes_names_without_dropping_the_fuller_record(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.by_id.insert(
                    UserId(7),
                    User {
                        id: UserId(7),
                        username: "old-name".into(),
                        display_name: "Old".into(),
                        avatar_url: "old.png".into(),
                        about_me: "bio".into(),
                        create_time_seconds: 42,
                        join_time_seconds: 43,
                    },
                );
                store.merge_users("token".into(), vec![hit(7, "token-bot", "Token Bot")], cx);
                let user = store.user(UserId(7)).unwrap();
                assert_eq!(user.username, "token-bot");
                assert_eq!(user.display_name, "Token Bot");
                assert_eq!(user.avatar_url, "token-bot.png");
                assert_eq!(user.about_me, "bio");
                assert_eq!(user.create_time_seconds, 42);
            });
        });
    }

    #[gpui::test]
    fn a_result_for_a_superseded_query_is_ignored(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.search("bot", cx);
                let stale_generation = store.search_generation;
                store.search("bot-r", cx);
                let response = mezon_proto::api::SearchCtrlKResponse {
                    users: vec![hit(7, "bot-hrm", "HRM")],
                    ..Default::default()
                };
                store.apply_search_result(stale_generation, "bot".into(), Ok(response), cx);
                assert_eq!(store.search_hits(), ("", &[][..]));
                assert_eq!(store.count(), 0);

                let response = mezon_proto::api::SearchCtrlKResponse {
                    users: vec![hit(9, "bot-reward", "Reward")],
                    ..Default::default()
                };
                let current = store.search_generation;
                store.apply_search_result(current, "bot-r".into(), Ok(response), cx);
                assert_eq!(store.search_hits(), ("bot-r", &[UserId(9)][..]));
            });
        });
    }

    #[gpui::test]
    fn an_empty_query_forgets_the_last_hits(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.merge_users("token".into(), vec![hit(7, "token-bot", "Token Bot")], cx);
                store.search("   ", cx);
                assert_eq!(store.search_hits(), ("", &[][..]));
                assert_eq!(store.count(), 1);
            });
        });
    }
}
