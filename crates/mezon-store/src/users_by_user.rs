use crate::clan::ClanList;
use crate::ids::UserId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};

use crate::clan_members::{ClanMembersStore, User, user_from_api};
use crate::ctrlk_search::CtrlKSearchType;
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{Freshness, KeyedCache};

#[derive(Debug, Clone)]
pub enum UsersByUserEvent {
    Changed,
    SearchSettled,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProfileUpdate {
    display_name: String,
    avatar: String,
    about_me: String,
}

impl From<&mezon_proto::realtime::UserProfileUpdatedEvent> for ProfileUpdate {
    fn from(event: &mezon_proto::realtime::UserProfileUpdatedEvent) -> Self {
        Self {
            display_name: event.display_name.clone(),
            avatar: event.avatar.clone(),
            about_me: event.about_me.clone(),
        }
    }
}

fn channel_profile(
    users: mezon_proto::api::AllUsersAddChannelResponse,
    user_id: UserId,
    cached: Option<User>,
    update: &ProfileUpdate,
) -> anyhow::Result<User> {
    let index = users
        .user_ids
        .iter()
        .position(|id| *id == user_id.0)
        .ok_or_else(|| anyhow::anyhow!("profile snapshot did not contain the updated user"))?;
    let mut user = cached.unwrap_or(User {
        id: user_id,
        ..Default::default()
    });
    user.username = users
        .usernames
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("profile snapshot omitted username"))?;
    user.display_name = users
        .display_names
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("profile snapshot omitted display name"))?;
    user.avatar_url = users
        .avatars
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("profile snapshot omitted avatar"))?;
    if !update.about_me.is_empty() {
        user.about_me.clone_from(&update.about_me);
    }
    Ok(user)
}

#[derive(Clone, Copy)]
enum ProfileSource {
    Clan(i64),
    Channel(i64),
}

pub struct UsersByUserStore {
    by_id: HashMap<UserId, User>,
    loading: bool,
    freshness: Freshness,
    api: Arc<AppApi>,
    search_generation: u64,
    search_query: String,
    search_hits: Vec<UserId>,
    profile_updates: HashMap<UserId, ProfileUpdate>,
    profile_pending: HashMap<UserId, (ProfileUpdate, ProfileSource)>,
    confirmed_profiles: HashSet<UserId>,
    profile_task: Option<Task<()>>,
    reset_generation: u64,
    missing_profile_ids: HashSet<UserId>,
    profile_lookup_results: KeyedCache<UserId, Result<(), ()>>,
    missing_profile_task: Option<Task<()>>,
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
        self.reset_profiles();
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
            profile_updates: HashMap::new(),
            profile_pending: HashMap::new(),
            confirmed_profiles: HashSet::new(),
            profile_task: None,
            reset_generation: 0,
            missing_profile_ids: HashSet::new(),
            profile_lookup_results: KeyedCache::new(Some(
                mezon_client::INBOX_PAGE_LIMIT as usize * 4,
            )),
            missing_profile_task: None,
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
            dispatch.on(
                RealtimeKind::UserProfileUpdated,
                &entity,
                |this, event, cx| this.handle_event(event, cx),
            );
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

    pub fn ensure_user_profiles(&mut self, user_ids: Vec<UserId>, cx: &mut Context<Self>) {
        let cached = ClanMembersStore::try_global(cx);
        let mut changed = false;
        for id in user_ids {
            if !self.by_id.contains_key(&id)
                && let Some(user) = cached
                    .as_ref()
                    .and_then(|store| store.read(cx).find_user(id))
                    .cloned()
            {
                self.by_id.insert(id, user);
                changed = true;
            }
            if id.get() > 0
                && !self.profile_lookup_results.is_fresh(&id, crate::CACHE_TTL)
                && self.user(id).is_none()
            {
                self.missing_profile_ids.insert(id);
            }
        }
        if changed {
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
        if self.missing_profile_ids.is_empty() || self.missing_profile_task.is_some() {
            return;
        }
        let Some(clans) = ClanList::try_global(cx).map(|store| {
            store
                .read(cx)
                .clans
                .iter()
                .map(|clan| clan.id)
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        if clans.is_empty() {
            return;
        }
        let generation = self.reset_generation;
        let targets = self.missing_profile_ids.clone();
        self.missing_profile_task = Some(cx.spawn(async move |this, cx| {
            let mut failed = false;
            for clan in clans {
                let pending = this
                    .update(cx, |this, _| {
                        if generation != this.reset_generation {
                            return false;
                        }
                        let users = &this.by_id;
                        this.missing_profile_ids
                            .retain(|id| !users.contains_key(id));
                        targets
                            .iter()
                            .any(|id| this.missing_profile_ids.contains(id))
                    })
                    .unwrap_or(false);
                if !pending {
                    break;
                }
                let roster = cx.update(|cx| {
                    ClanMembersStore::global(cx)
                        .update(cx, |store, cx| store.ensure_loaded_task(clan, cx))
                });
                let loaded = roster.await;
                failed |= !loaded;
                let _ = this.update(cx, |this, cx| {
                    if generation != this.reset_generation {
                        return;
                    }
                    let members = ClanMembersStore::global(cx);
                    let members = members.read(cx);
                    let mut changed = false;
                    for id in &targets {
                        if this.missing_profile_ids.contains(id)
                            && let Some(user) = members.find_user(*id)
                        {
                            if !this.confirmed_profiles.contains(id) && !this.by_id.contains_key(id)
                            {
                                this.by_id.insert(*id, user.clone());
                                changed = true;
                            }
                            this.missing_profile_ids.remove(id);
                            this.profile_lookup_results.insert(*id, Ok(()), None);
                        }
                    }
                    if changed {
                        cx.emit(UsersByUserEvent::Changed);
                        cx.notify();
                    }
                });
            }
            let _ = this.update(cx, |this, cx| {
                if generation == this.reset_generation {
                    for id in targets {
                        if this.missing_profile_ids.remove(&id) {
                            this.profile_lookup_results.insert(
                                id,
                                if failed { Err(()) } else { Ok(()) },
                                None,
                            );
                        }
                    }
                    this.missing_profile_task = None;
                    this.ensure_user_profiles(Vec::new(), cx);
                }
            });
        }));
    }

    pub fn retry_missing_profiles(&mut self) {
        let failures: Vec<_> = self
            .profile_lookup_results
            .iter()
            .filter_map(|(id, result)| result.is_err().then_some(*id))
            .collect();
        for id in failures {
            self.profile_lookup_results.mark_stale(&id);
        }
    }

    pub fn users(&self) -> impl Iterator<Item = &User> + '_ {
        self.by_id.values()
    }

    pub fn count(&self) -> usize {
        self.by_id.len()
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.freshness.is_fresh(crate::CACHE_TTL) && !self.loading {
            self.profile_lookup_results.clear();
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
                Some(existing) if !self.confirmed_profiles.contains(&user.id) => {
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
                Some(_) => {}
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
        self.reset_profiles();
        self.loading = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self._search_task = Task::ready(());
        self.freshness.mark_stale();
    }

    fn reset_profiles(&mut self) {
        self.missing_profile_task = None;
        self.missing_profile_ids.clear();
        self.profile_lookup_results.clear();
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.profile_task = None;
        self.profile_pending.clear();
        self.profile_updates.clear();
        self.confirmed_profiles.clear();
    }

    pub(crate) fn confirmed_profile(&self, user_id: UserId) -> Option<&User> {
        self.confirmed_profiles
            .contains(&user_id)
            .then(|| self.by_id.get(&user_id))
            .flatten()
    }

    pub(crate) fn apply_user_profile(&mut self, user: User, cx: &mut Context<Self>) {
        let changed = match self.by_id.get_mut(&user.id) {
            Some(existing) => existing.update_profile(&user),
            None => {
                self.by_id.insert(user.id, user.clone());
                true
            }
        };
        let first_confirmation = self.confirmed_profiles.insert(user.id);
        if changed || first_confirmation {
            crate::user_profile::apply_user_profile(&user, cx);
        }
        if changed {
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
    }

    fn queue_profile(
        &mut self,
        event: &mezon_proto::realtime::UserProfileUpdatedEvent,
        cx: &mut Context<Self>,
    ) {
        let user_id = UserId(event.user_id);
        if user_id.0 == 0
            || crate::account::AccountStore::try_global(cx).is_some_and(|store| {
                store
                    .read(cx)
                    .account
                    .as_ref()
                    .is_some_and(|account| account.user_id == user_id.0)
            })
        {
            return;
        }
        let update = ProfileUpdate::from(event);
        if self.profile_updates.get(&user_id) == Some(&update)
            && self.profile_pending.contains_key(&user_id)
        {
            return;
        }
        let source = (event.clan_id != 0)
            .then_some(ProfileSource::Clan(event.clan_id))
            .or_else(|| {
                crate::clan_members::ClanMembersStore::try_global(cx).and_then(|store| {
                    store
                        .read(cx)
                        .profile_clan(user_id)
                        .map(|id| ProfileSource::Clan(id.0))
                })
            })
            .or_else(|| (event.channel_id != 0).then_some(ProfileSource::Channel(event.channel_id)))
            .or_else(|| {
                crate::direct::DirectMessageStore::try_global(cx).and_then(|store| {
                    store
                        .read(cx)
                        .dm_with_peer(user_id)
                        .map(|dm| ProfileSource::Channel(dm.id.0))
                })
            })
            .or_else(|| {
                crate::group_members::GroupMembersStore::try_global(cx).and_then(|store| {
                    store
                        .read(cx)
                        .profile_channel(user_id)
                        .map(|id| ProfileSource::Channel(id.0))
                })
            });
        let Some(source) = source else {
            self.confirmed_profiles.remove(&user_id);
            tracing::warn!("profile update has no accessible snapshot source");
            return;
        };
        self.profile_updates.insert(user_id, update.clone());
        self.profile_pending.insert(user_id, (update, source));
        if self.profile_task.is_some() {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        self.profile_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some((user_id, update, source))) = this.update(cx, |this, _| {
                    if this.reset_generation != generation {
                        return None;
                    }
                    let Some(&user_id) = this.profile_pending.keys().next() else {
                        this.profile_task = None;
                        return None;
                    };
                    let (update, source) = this.profile_pending.remove(&user_id)?;
                    Some((user_id, update, source))
                }) else {
                    break;
                };
                let cached = this
                    .update(cx, |this, _| this.by_id.get(&user_id).cloned())
                    .ok()
                    .flatten();
                let result = match source {
                    ProfileSource::Clan(clan_id) => api
                        .list_clan_users(clan_id)
                        .await
                        .map(|users| {
                            users
                                .into_iter()
                                .filter_map(|member| member.user)
                                .find(|user| user.id == user_id.0)
                                .and_then(user_from_api)
                        })
                        .and_then(|user| {
                            user.ok_or_else(|| {
                                anyhow::anyhow!("profile snapshot did not contain the updated user")
                            })
                        }),
                    ProfileSource::Channel(channel_id) => api
                        .list_channel_users_uc(
                            channel_id,
                            crate::group_members::GROUP_MEMBER_FETCH_LIMIT,
                        )
                        .await
                        .and_then(|users| channel_profile(users, user_id, cached, &update)),
                };
                if this
                    .update(cx, |this, cx| {
                        this.apply_profile_result(user_id, &update, generation, result, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn apply_profile_result(
        &mut self,
        user_id: UserId,
        update: &ProfileUpdate,
        generation: u64,
        result: anyhow::Result<User>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.reset_generation
            || self.profile_updates.get(&user_id) != Some(update)
            || self.profile_pending.contains_key(&user_id)
        {
            return;
        }
        match result {
            Ok(mut user) if user.id == user_id => {
                self.profile_updates.remove(&user_id);
                if !update.display_name.is_empty() {
                    user.display_name.clone_from(&update.display_name);
                }
                self.apply_user_profile(user, cx);
            }
            Ok(_) => {
                self.profile_updates.remove(&user_id);
                self.confirmed_profiles.remove(&user_id);
                tracing::warn!("profile snapshot identity did not match");
            }
            Err(error) => {
                self.profile_updates.remove(&user_id);
                self.confirmed_profiles.remove(&user_id);
                tracing::warn!("profile snapshot failed: {error}");
            }
        }
    }

    fn apply_users_snapshot(&mut self, users: Vec<mezon_proto::api::User>) {
        self.freshness.mark_fetched();
        for user in users {
            if let Some(user) = user_from_api(user)
                && !self.confirmed_profiles.contains(&user.id)
            {
                self.by_id.insert(user.id, user);
            }
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_user_clans_by_user().await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.reset_generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(users) => {
                        this.apply_users_snapshot(users);
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
        if let RealtimeEvent::UserProfileUpdated(event) = event {
            self.queue_profile(event, cx);
            return;
        }
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
            if self.confirmed_profiles.contains(&user.id) {
                return;
            }
            self.by_id.insert(user.id, user);
            cx.emit(UsersByUserEvent::Changed);
            cx.notify();
        }
    }
}

pub(crate) mod message_avatar {
    use std::sync::Arc;

    use gpui::{App, Global};
    use mezon_client::{AppApi, transport::ApiMessage};

    use crate::{ChannelId, ClanId, MessageId};

    pub(crate) const AVATAR_LOOKUP_CONCURRENCY: usize = 4;
    pub(crate) const AVATAR_NOTIFY_COALESCE: std::time::Duration =
        std::time::Duration::from_millis(100);
    const AVATAR_MESSAGE_DIRECTION_AROUND: i32 = 2;
    const AVATAR_MESSAGE_LOOKUP_LIMIT: u32 = 2;

    struct AvatarRequestPermits(Arc<tokio::sync::Semaphore>);
    impl Global for AvatarRequestPermits {}

    pub(crate) fn avatar_request_permits(cx: &mut App) -> Arc<tokio::sync::Semaphore> {
        if !cx.has_global::<AvatarRequestPermits>() {
            cx.set_global(AvatarRequestPermits(Arc::new(tokio::sync::Semaphore::new(
                AVATAR_LOOKUP_CONCURRENCY,
            ))));
        }
        cx.global::<AvatarRequestPermits>().0.clone()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub(crate) struct MessageAvatarKey {
        pub clan_id: ClanId,
        pub parent_id: ChannelId,
        pub channel_id: ChannelId,
        pub message_id: MessageId,
    }

    pub(crate) async fn fetch_avatar_message(
        api: &AppApi,
        key: MessageAvatarKey,
    ) -> anyhow::Result<Option<ApiMessage>> {
        let page = if key.channel_id == key.parent_id {
            api.list_channel_messages(
                key.clan_id.get(),
                key.channel_id.get(),
                key.message_id.get(),
                AVATAR_MESSAGE_DIRECTION_AROUND,
                AVATAR_MESSAGE_LOOKUP_LIMIT,
            )
            .await?
        } else {
            api.list_topic_messages(
                key.clan_id.get(),
                key.parent_id.get(),
                key.channel_id.get(),
                key.message_id.get(),
                AVATAR_MESSAGE_DIRECTION_AROUND,
                AVATAR_MESSAGE_LOOKUP_LIMIT,
            )
            .await?
        };
        Ok(page
            .messages
            .into_iter()
            .find(|message| message.message_id == key.message_id.get()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[gpui::test]
        fn lookup_consumers_share_one_concurrency_budget(cx: &mut gpui::TestAppContext) {
            cx.update(|cx| {
                let first = avatar_request_permits(cx);
                let second = avatar_request_permits(cx);
                assert!(Arc::ptr_eq(&first, &second));
                let permits: Vec<_> = (0..AVATAR_LOOKUP_CONCURRENCY)
                    .map(|_| first.clone().try_acquire_owned().unwrap())
                    .collect();
                assert!(second.clone().try_acquire_owned().is_err());
                drop(permits);
                assert!(second.try_acquire_owned().is_ok());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[gpui::test]
    fn authoritative_snapshot_can_clear_avatar_and_display_name(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, _| {
                store.by_id.insert(
                    UserId(7),
                    User {
                        id: UserId(7),
                        username: "user".into(),
                        display_name: "Old".into(),
                        avatar_url: "old.png".into(),
                        ..Default::default()
                    },
                );
                store.apply_users_snapshot(vec![api::User {
                    id: 7,
                    username: "user".into(),
                    ..Default::default()
                }]);
                let user = store.user(UserId(7)).unwrap();
                assert!(user.avatar_url.is_empty());
                assert!(user.display_name.is_empty());
                assert_eq!(user.username, "user");
            });
        });
    }

    #[gpui::test]
    fn cached_user_without_avatar_is_resolved_without_roster_requests(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            let members = ClanMembersStore::init(store.read(cx).api.clone(), cx);
            members.update(cx, |members, _| {
                members.seed_members_for_test(
                    crate::ClanId(1),
                    vec![crate::ClanMember {
                        user: User {
                            id: UserId(7),
                            username: "no-avatar".into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }],
                );
            });
            store.update(cx, |store, cx| {
                store.ensure_user_profiles(vec![UserId(7), UserId(7)], cx);
                assert_eq!(store.user(UserId(7)).unwrap().username, "no-avatar");
                assert!(store.user(UserId(7)).unwrap().avatar_url.is_empty());
                assert!(store.missing_profile_ids.is_empty());
                assert!(store.missing_profile_task.is_none());
                store
                    .profile_lookup_results
                    .insert(UserId(8), Err(()), None);
                store.profile_lookup_results.insert(UserId(9), Ok(()), None);
                store.retry_missing_profiles();
                assert!(
                    !store
                        .profile_lookup_results
                        .is_fresh(&UserId(8), crate::CACHE_TTL)
                );
                assert!(
                    store
                        .profile_lookup_results
                        .is_fresh(&UserId(9), crate::CACHE_TTL)
                );
            });
        });
    }

    #[gpui::test]
    fn reset_cancels_missing_sender_lookup_and_clears_retry_state(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.missing_profile_ids.insert(UserId(7));
                store
                    .profile_lookup_results
                    .insert(UserId(8), Err(()), None);
                store.missing_profile_task =
                    Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
                let generation = store.reset_generation;
                store.reset(cx);
                assert_ne!(store.reset_generation, generation);
                assert!(store.missing_profile_ids.is_empty());
                assert!(store.profile_lookup_results.is_empty());
                assert!(store.missing_profile_task.is_none());
            });
        });
    }

    #[gpui::test]
    fn duplicate_clan_events_coalesce_and_stale_results_cannot_replace_profile(
        cx: &mut gpui::TestAppContext,
    ) {
        let (store, api) = cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.profile_task =
                    Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
            });
            (store.clone(), store.read(cx).api.clone())
        });
        cx.run_until_parked();
        let mut event = mezon_proto::realtime::UserProfileUpdatedEvent {
            user_id: 7,
            clan_id: 1,
            display_name: "New".into(),
            ..Default::default()
        };
        for clan_id in [1, 2, 3] {
            event.clan_id = clan_id;
            api.publish_event(RealtimeEvent::UserProfileUpdated(event.clone()));
        }
        cx.run_until_parked();
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                assert_eq!(store.profile_pending.len(), 1);
                assert_eq!(store.profile_updates.len(), 1);
                assert!(store.user(UserId(7)).is_none());
                let old_update = store.profile_updates[&UserId(7)].clone();
                let generation = store.reset_generation;
                event.display_name = "Newest".into();
                store.queue_profile(&event, cx);
                let new_update = store.profile_updates[&UserId(7)].clone();
                store.apply_profile_result(
                    UserId(7),
                    &old_update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        display_name: "Old".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(store.user(UserId(7)).is_none());
                store.profile_pending.remove(&UserId(7));
                store.apply_profile_result(
                    UserId(7),
                    &new_update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        username: "user".into(),
                        display_name: "Old snapshot".into(),
                        about_me: "Bio".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                let user = store.user(UserId(7)).unwrap();
                assert_eq!(user.display_name, "Newest");
                assert_eq!(user.about_me, "Bio");
                store.merge_users("old".into(), vec![hit(7, "old", "Old")], cx);
                assert_eq!(store.user(UserId(7)).unwrap().display_name, "Newest");
                store.reset(cx);
                store.apply_profile_result(
                    UserId(7),
                    &new_update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        display_name: "Late".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(store.user(UserId(7)).is_none());
                assert!(store.profile_pending.is_empty());
                assert!(store.profile_task.is_none());
                assert!(store.confirmed_profiles.is_empty());
            })
        });
    }

    #[gpui::test]
    fn empty_profile_event_uses_snapshot_and_failure_keeps_known_profile(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                let event = mezon_proto::realtime::UserProfileUpdatedEvent {
                    user_id: 7,
                    clan_id: 1,
                    ..Default::default()
                };
                store.by_id.insert(
                    UserId(7),
                    User {
                        id: UserId(7),
                        username: "user".into(),
                        display_name: "Old".into(),
                        avatar_url: "old.png".into(),
                        about_me: "Bio".into(),
                        ..Default::default()
                    },
                );
                store.profile_task =
                    Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
                let known = store.user(UserId(7)).unwrap().clone();
                store.apply_user_profile(known, cx);
                store.queue_profile(&event, cx);
                assert_eq!(store.user(UserId(7)).unwrap().about_me, "Bio");
                let update = store.profile_updates[&UserId(7)].clone();
                let generation = store.reset_generation;
                store.profile_pending.remove(&UserId(7));
                store.apply_profile_result(
                    UserId(7),
                    &update,
                    generation,
                    Err(anyhow::anyhow!("offline")),
                    cx,
                );
                assert_eq!(store.user(UserId(7)).unwrap().display_name, "Old");
                assert!(!store.profile_updates.contains_key(&UserId(7)));
                assert!(store.confirmed_profile(UserId(7)).is_none());
                store.queue_profile(&event, cx);
                store.profile_pending.remove(&UserId(7));
                store.apply_profile_result(
                    UserId(7),
                    &update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        username: "user".into(),
                        about_me: "Bio".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                let user = store.confirmed_profile(UserId(7)).unwrap();
                assert!(user.display_name.is_empty());
                assert!(user.avatar_url.is_empty());
                assert_eq!(user.about_me, "Bio");
                store.profile_pending.clear();
                store.queue_profile(&event, cx);
                assert!(store.profile_pending.contains_key(&UserId(7)));
                store.invalidate();
                assert!(store.confirmed_profile(UserId(7)).is_none());
                assert!(store.profile_task.is_none());
            });
        });
    }

    #[gpui::test]
    fn identical_sparse_event_during_request_requires_a_new_snapshot(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.profile_task =
                    Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
                let event = mezon_proto::realtime::UserProfileUpdatedEvent {
                    user_id: 7,
                    clan_id: 1,
                    ..Default::default()
                };
                store.queue_profile(&event, cx);
                let update = store.profile_updates[&UserId(7)].clone();
                let generation = store.reset_generation;
                store.profile_pending.remove(&UserId(7));
                store.queue_profile(&event, cx);
                store.apply_profile_result(
                    UserId(7),
                    &update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        display_name: "Stale".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(store.user(UserId(7)).is_none());
                store.profile_pending.remove(&UserId(7));
                store.apply_profile_result(
                    UserId(7),
                    &update,
                    generation,
                    Ok(User {
                        id: UserId(7),
                        username: "user".into(),
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(store.user(UserId(7)).unwrap().display_name.is_empty());
                assert!(store.profile_updates.is_empty());
            });
        });
    }

    #[test]
    fn channel_snapshot_distinguishes_empty_fields_from_missing_columns() {
        let event = mezon_proto::realtime::UserProfileUpdatedEvent {
            user_id: 7,
            ..Default::default()
        };
        let update = ProfileUpdate::from(&event);
        let known = User {
            id: UserId(7),
            username: "old".into(),
            display_name: "Old".into(),
            avatar_url: "old.png".into(),
            about_me: "Bio".into(),
            ..Default::default()
        };
        let mut response = mezon_proto::api::AllUsersAddChannelResponse {
            user_ids: vec![7],
            usernames: vec!["user".into()],
            display_names: vec![String::new()],
            avatars: vec![String::new()],
            ..Default::default()
        };
        let user =
            channel_profile(response.clone(), UserId(7), Some(known.clone()), &update).unwrap();
        assert!(user.display_name.is_empty());
        assert!(user.avatar_url.is_empty());
        assert_eq!(user.about_me, "Bio");
        assert_eq!(user.username, "user");
        assert!(channel_profile(response.clone(), UserId(8), None, &update).is_err());
        response.display_names.clear();
        assert!(channel_profile(response, UserId(7), Some(known), &update).is_err());
    }

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
