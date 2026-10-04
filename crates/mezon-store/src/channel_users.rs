use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::api;

use crate::ids::{ChannelId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{CACHE_TTL, KeyedCache};
use crate::{ChannelEvent, ChannelList};

const MAX_CACHED_CHANNELS: usize = 64;

/// `mezon-api` caps this at `2 * MAX_USER_CHANNEL` and defaults to `MAX_USER_CHANNEL`
/// (1000) when the request leaves it at 0, so 1000 is the largest limit every caller
/// agrees on — the server caches the response under `(channel_id)` alone, ignoring the
/// limit, so a smaller number here silently truncates the list for everyone that hits
/// the same cache entry afterwards.
const CHANNEL_USER_FETCH_LIMIT: i32 = 1000;

#[derive(Debug, Clone)]
pub enum ChannelUsersEvent {
    Changed { channel_id: ChannelId },
    MembershipChanged { channel_id: ChannelId },
}

/// Identity the channel-user listing carries for each member. The clan roster is the
/// richer source (nickname, clan avatar), but it is capped server-side and drops anyone
/// who left the clan, so these fields are what keeps such a row from rendering blank.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelUserProfile {
    pub username: String,
    pub display_name: String,
    pub avatar: String,
}

#[derive(Debug, Default)]
struct ChannelUsers {
    ids: Vec<UserId>,
    profiles: HashMap<UserId, ChannelUserProfile>,
}

pub struct ChannelUsersStore {
    cache: KeyedCache<ChannelId, ChannelUsers>,
    loading: HashMap<ChannelId, u64>,
    next_fetch: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
    _channel_list_sub: Option<Subscription>,
}

struct GlobalChannelUsersStore(Entity<ChannelUsersStore>);
impl Global for GlobalChannelUsersStore {}

impl EventEmitter<ChannelUsersEvent> for ChannelUsersStore {}

impl ChannelUsersStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelUsersStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let channel_list_sub = ChannelList::try_global(cx).map(|channels| {
            cx.subscribe(&channels, |this, _, event: &ChannelEvent, cx| {
                if let ChannelEvent::PrivacyChanged {
                    channel_id,
                    private,
                    ..
                } = event
                {
                    this.apply_privacy_change(*channel_id, *private, cx);
                }
            })
        });
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            loading: HashMap::new(),
            next_fetch: 0,
            api,
            _conn_watch: conn_watch,
            _channel_list_sub: channel_list_sub,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelUsersStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelUsersStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cache.clear();
        self.loading.clear();
        cx.notify();
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::UserChannelAdded,
                RealtimeKind::UserChannelRemoved,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_realtime(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, _| this.invalidate());
        });
    }

    fn handle_realtime(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let channel_id = match event {
            RealtimeEvent::UserChannelAdded(event) => event
                .channel_desc
                .as_ref()
                .map(|desc| ChannelId(desc.channel_id)),
            RealtimeEvent::UserChannelRemoved(event) => Some(ChannelId(event.channel_id)),
            _ => None,
        };
        if let Some(channel_id) = channel_id.filter(|id| !id.is_zero()) {
            self.apply_membership_change(channel_id, cx);
        }
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

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
    }

    pub fn user_ids(&self, channel_id: ChannelId) -> &[UserId] {
        self.cache
            .get(&channel_id)
            .map(|users| users.ids.as_slice())
            .unwrap_or_default()
    }

    /// Name/avatar the listing shipped for `user_id`, for rows the clan roster cannot
    /// resolve.
    pub fn profile(&self, channel_id: ChannelId, user_id: UserId) -> Option<&ChannelUserProfile> {
        self.cache.get(&channel_id)?.profiles.get(&user_id)
    }

    pub fn is_loaded(&self, channel_id: ChannelId) -> bool {
        self.cache.contains(&channel_id)
    }

    pub fn is_loading(&self, channel_id: ChannelId) -> bool {
        self.loading.contains_key(&channel_id)
    }

    pub fn ensure_loaded(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.get() == 0
            || self.cache.is_fresh(&channel_id, CACHE_TTL)
            || self.loading.contains_key(&channel_id)
        {
            return;
        }
        self.next_fetch += 1;
        let fetch = self.next_fetch;
        self.loading.insert(channel_id, fetch);
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_users_uc(channel_id.get(), CHANNEL_USER_FETCH_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_fetch(channel_id, fetch, result, cx);
            });
        })
        .detach();
    }

    fn finish_fetch(
        &mut self,
        channel_id: ChannelId,
        fetch: u64,
        result: anyhow::Result<api::AllUsersAddChannelResponse>,
        cx: &mut Context<Self>,
    ) {
        if self.loading.get(&channel_id) != Some(&fetch) {
            return;
        }
        self.loading.remove(&channel_id);
        match result {
            Ok(response) => {
                self.cache
                    .insert(channel_id, channel_users_from(response), None);
                cx.emit(ChannelUsersEvent::Changed { channel_id });
                cx.notify();
            }
            Err(error) => {
                tracing::error!("list_channel_users_uc failed for {channel_id}: {error}")
            }
        }
    }

    pub fn apply_privacy_change(
        &mut self,
        channel_id: ChannelId,
        private: bool,
        cx: &mut Context<Self>,
    ) {
        let in_flight = self.loading.remove(&channel_id).is_some();
        if !in_flight && !self.cache.contains(&channel_id) {
            return;
        }
        if private {
            self.cache.mark_stale(&channel_id);
            self.ensure_loaded(channel_id, cx);
            return;
        }
        self.cache.insert(channel_id, ChannelUsers::default(), None);
        cx.emit(ChannelUsersEvent::Changed { channel_id });
        cx.notify();
    }

    fn apply_membership_change(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.loading.remove(&channel_id);
        self.cache.mark_stale(&channel_id);
        cx.emit(ChannelUsersEvent::MembershipChanged { channel_id });
    }

    #[cfg(test)]
    pub(crate) fn seed_users_for_test(&mut self, channel_id: ChannelId, user_ids: &[UserId]) {
        let users = ChannelUsers {
            ids: user_ids.to_vec(),
            profiles: HashMap::new(),
        };
        self.cache.insert(channel_id, users, None);
    }

    pub fn add_users(
        &mut self,
        channel_id: ChannelId,
        user_ids: &[UserId],
        cx: &mut Context<Self>,
    ) {
        let Some(existing) = self.cache.get_mut(&channel_id) else {
            return;
        };
        if apply_add(existing, user_ids) {
            cx.emit(ChannelUsersEvent::Changed { channel_id });
            cx.notify();
        }
    }

    pub fn remove_users(
        &mut self,
        channel_id: ChannelId,
        user_ids: &[UserId],
        cx: &mut Context<Self>,
    ) {
        let Some(existing) = self.cache.get_mut(&channel_id) else {
            return;
        };
        if apply_remove(existing, user_ids) {
            cx.emit(ChannelUsersEvent::Changed { channel_id });
            cx.notify();
        }
    }
}

/// The listing ships identity as parallel arrays; anything the server left short just
/// falls back to the clan roster at render time.
fn channel_users_from(response: api::AllUsersAddChannelResponse) -> ChannelUsers {
    let mut users = ChannelUsers {
        ids: Vec::with_capacity(response.user_ids.len()),
        profiles: HashMap::with_capacity(response.user_ids.len()),
    };
    let mut usernames = response.usernames.into_iter();
    let mut display_names = response.display_names.into_iter();
    let mut avatars = response.avatars.into_iter();
    for raw_id in response.user_ids {
        let user_id = UserId(raw_id);
        users.ids.push(user_id);
        let profile = ChannelUserProfile {
            username: usernames.next().unwrap_or_default(),
            display_name: display_names.next().unwrap_or_default(),
            avatar: avatars.next().unwrap_or_default(),
        };
        if profile != ChannelUserProfile::default() {
            users.profiles.insert(user_id, profile);
        }
    }
    users
}

fn apply_add(existing: &mut ChannelUsers, user_ids: &[UserId]) -> bool {
    let mut changed = false;
    for user_id in user_ids {
        if !existing.ids.contains(user_id) {
            existing.ids.push(*user_id);
            changed = true;
        }
    }
    changed
}

fn apply_remove(existing: &mut ChannelUsers, user_ids: &[UserId]) -> bool {
    let before = existing.ids.len();
    existing.ids.retain(|id| !user_ids.contains(id));
    if existing.ids.len() == before {
        return false;
    }
    for user_id in user_ids {
        existing.profiles.remove(user_id);
    }
    true
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn users(ids: &[i64]) -> ChannelUsers {
        ChannelUsers {
            ids: ids.iter().copied().map(UserId).collect(),
            profiles: HashMap::new(),
        }
    }

    fn init_store(cx: &mut App) -> Entity<ChannelUsersStore> {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        RealtimeDispatch::init(api.clone(), cx);
        ChannelUsersStore::init(api, cx)
    }

    fn user_added(channel_id: i64) -> RealtimeEvent {
        RealtimeEvent::UserChannelAdded(mezon_proto::realtime::UserChannelAdded {
            channel_desc: Some(api::ChannelDescription {
                channel_id,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn user_removed(channel_id: i64) -> RealtimeEvent {
        RealtimeEvent::UserChannelRemoved(mezon_proto::realtime::UserChannelRemoved {
            channel_id,
            ..Default::default()
        })
    }

    #[gpui::test]
    fn a_membership_event_marks_a_cached_member_list_stale(cx: &mut gpui::TestAppContext) {
        let store = cx.update(init_store);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let _sub = cx.update(|cx| {
            let seen = seen.clone();
            cx.subscribe(&store, move |_, event: &ChannelUsersEvent, _| {
                if let ChannelUsersEvent::MembershipChanged { channel_id } = event {
                    seen.borrow_mut().push(*channel_id);
                }
            })
        });
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.seed_users_for_test(ChannelId(1), &[UserId(5)]);
                store.handle_realtime(&user_added(1), cx);
                assert!(!store.is_loading(ChannelId(1)));
                assert_eq!(store.user_ids(ChannelId(1)), &[UserId(5)]);
                store.ensure_loaded(ChannelId(1), cx);
                assert!(store.is_loading(ChannelId(1)));
            });
        });
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                let fetch = store.loading[&ChannelId(1)];
                store.finish_fetch(ChannelId(1), fetch, Ok(listing(&[5, 6])), cx);
                assert_eq!(store.user_ids(ChannelId(1)), &[UserId(5), UserId(6)]);
                store.handle_realtime(&user_removed(1), cx);
                store.ensure_loaded(ChannelId(1), cx);
                assert!(store.is_loading(ChannelId(1)));
            });
        });
        assert_eq!(*seen.borrow(), vec![ChannelId(1), ChannelId(1)]);
    }

    #[gpui::test]
    fn a_membership_event_discards_a_fetch_already_in_flight(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.ensure_loaded(ChannelId(1), cx);
                let stale_fetch = store.loading[&ChannelId(1)];
                store.handle_realtime(&user_added(1), cx);
                assert!(!store.is_loading(ChannelId(1)));
                store.finish_fetch(ChannelId(1), stale_fetch, Ok(listing(&[5])), cx);
                assert!(!store.is_loaded(ChannelId(1)));
                store.ensure_loaded(ChannelId(1), cx);
                let fresh_fetch = store.loading[&ChannelId(1)];
                assert_ne!(stale_fetch, fresh_fetch);
                store.finish_fetch(ChannelId(1), fresh_fetch, Ok(listing(&[5, 9])), cx);
                assert_eq!(store.user_ids(ChannelId(1)), &[UserId(5), UserId(9)]);
            });
        });
    }

    #[gpui::test]
    fn a_membership_event_leaves_uncached_channels_unfetched(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.handle_realtime(&user_added(2), cx);
                store.handle_realtime(&user_removed(3), cx);
                store.handle_realtime(&user_added(0), cx);
                assert!(!store.is_loading(ChannelId(2)));
                assert!(!store.is_loading(ChannelId(3)));
                assert!(!store.is_loaded(ChannelId(2)));
            });
        });
    }

    #[gpui::test]
    fn going_public_empties_the_cached_member_list(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.seed_users_for_test(ChannelId(1), &[UserId(5), UserId(6)]);
                store.apply_privacy_change(ChannelId(1), false, cx);
                assert!(store.user_ids(ChannelId(1)).is_empty());
                assert!(store.is_loaded(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn going_private_refetches_a_cached_member_list(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.seed_users_for_test(ChannelId(1), &[UserId(5), UserId(6)]);
                store.apply_privacy_change(ChannelId(1), true, cx);
                assert!(store.is_loading(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn a_privacy_change_leaves_uncached_channels_alone(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.apply_privacy_change(ChannelId(2), true, cx);
                store.apply_privacy_change(ChannelId(3), false, cx);
                assert!(!store.is_loading(ChannelId(2)));
                assert!(!store.is_loaded(ChannelId(2)));
                assert!(!store.is_loaded(ChannelId(3)));
            });
        });
    }

    #[gpui::test]
    fn going_public_discards_a_fetch_already_in_flight(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.seed_users_for_test(ChannelId(1), &[UserId(5), UserId(6)]);
                store.cache.mark_stale(&ChannelId(1));
                store.ensure_loaded(ChannelId(1), cx);
                let stale_fetch = store.loading[&ChannelId(1)];
                store.apply_privacy_change(ChannelId(1), false, cx);
                store.finish_fetch(ChannelId(1), stale_fetch, Ok(listing(&[5, 6])), cx);
                assert!(store.user_ids(ChannelId(1)).is_empty());
                assert!(!store.is_loading(ChannelId(1)));
            });
        });
    }

    #[gpui::test]
    fn going_private_restarts_a_fetch_already_in_flight(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_store(cx);
            store.update(cx, |store, cx| {
                store.ensure_loaded(ChannelId(1), cx);
                let stale_fetch = store.loading[&ChannelId(1)];
                store.apply_privacy_change(ChannelId(1), true, cx);
                let fresh_fetch = store.loading[&ChannelId(1)];
                assert_ne!(stale_fetch, fresh_fetch);
                store.finish_fetch(ChannelId(1), stale_fetch, Ok(listing(&[5, 6])), cx);
                assert!(store.is_loading(ChannelId(1)));
                assert!(!store.is_loaded(ChannelId(1)));
                store.finish_fetch(ChannelId(1), fresh_fetch, Ok(listing(&[9])), cx);
                assert_eq!(store.user_ids(ChannelId(1)), &[UserId(9)]);
            });
        });
    }

    fn listing(ids: &[i64]) -> api::AllUsersAddChannelResponse {
        api::AllUsersAddChannelResponse {
            user_ids: ids.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn add_users_skips_duplicates_and_reports_change() {
        let mut existing = users(&[1, 2]);
        assert!(apply_add(&mut existing, &[UserId(3)]));
        assert_eq!(existing.ids, vec![UserId(1), UserId(2), UserId(3)]);
        assert!(!apply_add(&mut existing, &[UserId(2), UserId(3)]));
        assert_eq!(existing.ids, vec![UserId(1), UserId(2), UserId(3)]);
    }

    #[test]
    fn remove_users_reports_change_only_when_present() {
        let mut existing = users(&[1, 2, 3]);
        assert!(apply_remove(&mut existing, &[UserId(2)]));
        assert_eq!(existing.ids, vec![UserId(1), UserId(3)]);
        assert!(!apply_remove(&mut existing, &[UserId(99)]));
        assert_eq!(existing.ids, vec![UserId(1), UserId(3)]);
    }

    #[test]
    fn identity_arrays_are_kept_per_user_and_dropped_on_removal() {
        let response = api::AllUsersAddChannelResponse {
            channel_id: 7,
            user_ids: vec![1, 2, 3],
            limit: 1000,
            usernames: vec!["one".into(), "two".into(), "three".into()],
            display_names: vec!["One".into(), "Two".into()],
            avatars: vec!["a1".into()],
            onlines: Vec::new(),
        };
        let mut users = channel_users_from(response);
        assert_eq!(users.ids, vec![UserId(1), UserId(2), UserId(3)]);
        assert_eq!(
            users.profiles.get(&UserId(1)),
            Some(&ChannelUserProfile {
                username: "one".into(),
                display_name: "One".into(),
                avatar: "a1".into(),
            })
        );
        assert_eq!(
            users.profiles.get(&UserId(3)),
            Some(&ChannelUserProfile {
                username: "three".into(),
                display_name: String::new(),
                avatar: String::new(),
            })
        );
        assert!(apply_remove(&mut users, &[UserId(1)]));
        assert!(!users.profiles.contains_key(&UserId(1)));
    }

    #[test]
    fn profile_fields_reuse_response_allocations() {
        let response = api::AllUsersAddChannelResponse {
            user_ids: vec![9],
            usernames: vec!["qa_member".into()],
            display_names: vec!["QA Member".into()],
            avatars: vec!["https://example.test/avatar.png".into()],
            ..Default::default()
        };
        let username = response.usernames[0].as_ptr();
        let display_name = response.display_names[0].as_ptr();
        let avatar = response.avatars[0].as_ptr();
        let users = channel_users_from(response);
        let profile = &users.profiles[&UserId(9)];
        assert_eq!(profile.username.as_ptr(), username);
        assert_eq!(profile.display_name.as_ptr(), display_name);
        assert_eq!(profile.avatar.as_ptr(), avatar);
    }

    #[test]
    fn a_listing_without_identity_columns_keeps_every_id() {
        let response = api::AllUsersAddChannelResponse {
            channel_id: 7,
            user_ids: vec![4, 5],
            limit: 1000,
            usernames: Vec::new(),
            display_names: Vec::new(),
            avatars: Vec::new(),
            onlines: Vec::new(),
        };
        let users = channel_users_from(response);
        assert_eq!(users.ids, vec![UserId(4), UserId(5)]);
        assert!(users.profiles.is_empty());
    }
}
