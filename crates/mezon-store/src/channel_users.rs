use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::ids::{ChannelId, UserId};
use crate::{CACHE_TTL, KeyedCache};

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
    loading: HashSet<ChannelId>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
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
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            loading: HashSet::new(),
            api,
            _conn_watch: conn_watch,
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
        self.loading.contains(&channel_id)
    }

    pub fn ensure_loaded(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.get() == 0 || self.cache.is_fresh(&channel_id, CACHE_TTL) {
            return;
        }
        if !self.loading.insert(channel_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_users_uc(channel_id.get(), CHANNEL_USER_FETCH_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&channel_id);
                match result {
                    Ok(response) => {
                        this.cache
                            .insert(channel_id, channel_users_from(response), None);
                        cx.emit(ChannelUsersEvent::Changed { channel_id });
                        cx.notify();
                    }
                    Err(error) => {
                        tracing::error!("list_channel_users_uc failed for {channel_id}: {error}")
                    }
                }
            });
        })
        .detach();
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
    use super::*;

    fn users(ids: &[i64]) -> ChannelUsers {
        ChannelUsers {
            ids: ids.iter().copied().map(UserId).collect(),
            profiles: HashMap::new(),
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
