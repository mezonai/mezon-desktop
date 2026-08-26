use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::badge::BadgeService;
use crate::clan_members::ClanMembersStore;
use crate::ids::{ChannelId, ClanId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_CHANNEL_PERMISSIONS: usize = 64;
const FAILED_FETCH_BACKOFF: Duration = Duration::from_secs(30);
const REFETCH_CONCURRENCY: usize = 4;
const REFETCH_STEP_DELAY: Duration = Duration::from_millis(120);

pub const PERMISSION_MANAGE_THREAD: &str = "manage-thread";
pub const PERMISSION_SEND_MESSAGE: &str = "send-message";
pub const PERMISSION_DELETE_MESSAGE: &str = "delete-message";

pub const OVERRIDDEN_SLUGS: [&str; 3] = [
    PERMISSION_MANAGE_THREAD,
    PERMISSION_SEND_MESSAGE,
    PERMISSION_DELETE_MESSAGE,
];

pub fn is_overridden_slug(slug: &str) -> bool {
    OVERRIDDEN_SLUGS.contains(&slug)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChannelPermissionKey {
    clan_id: ClanId,
    channel_id: ChannelId,
}

#[derive(Debug, Clone)]
pub enum ChannelPermissionsEvent {
    Changed {
        clan_id: ClanId,
        channel_id: ChannelId,
    },
}

pub struct ChannelPermissionsStore {
    cache: KeyedCache<ChannelPermissionKey, HashMap<String, bool>>,
    loading: HashSet<ChannelPermissionKey>,
    failed_at: HashMap<ChannelPermissionKey, Instant>,
    reset_generation: u64,
    patch_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
    _refetch: Task<()>,
}

struct GlobalChannelPermissionsStore(Entity<ChannelPermissionsStore>);
impl Global for GlobalChannelPermissionsStore {}

impl EventEmitter<ChannelPermissionsEvent> for ChannelPermissionsStore {}

impl ChannelPermissionsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelPermissionsStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelPermissionsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelPermissionsStore>()
            .map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::RoleEvent, &entity, |this, event, cx| {
                this.handle_role_event(event, cx);
            });
            dispatch.on(
                RealtimeKind::PermissionChanged,
                &entity,
                |this, event, cx| {
                    this.handle_permission_changed(event, cx);
                },
            );
        });
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNEL_PERMISSIONS)),
            loading: HashSet::new(),
            failed_at: HashMap::new(),
            reset_generation: 0,
            patch_generation: 0,
            api,
            _conn_watch: conn_watch,
            _refetch: Task::ready(()),
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
                    if this
                        .update(cx, |this, cx| {
                            this.invalidate();
                            this.refetch_cached(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    fn handle_role_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(role_event)) = event
        else {
            return;
        };
        if !self.role_event_affects_me(role_event, cx) {
            return;
        }
        self.invalidate();
        self.refetch_cached(cx);
        cx.notify();
    }

    fn handle_permission_changed(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::PermissionChangedEvent(changed)) =
            event
        else {
            return;
        };
        let Some(me) = BadgeService::try_global(cx).and_then(|b| b.read(cx).current_user_id(cx))
        else {
            return;
        };
        if changed.user_id != me.get() {
            return;
        }
        let channel_id = ChannelId(changed.channel_id);
        let keys = self
            .cache
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| key.channel_id == channel_id)
            .collect::<Vec<_>>();
        let mut touched = Vec::new();
        for key in keys {
            let Some(perms) = self.cache.get_mut(&key) else {
                continue;
            };
            let mut changed_any = false;
            for (updates, active) in [
                (&changed.add_permissions, Some(true)),
                (&changed.remove_permissions, Some(false)),
                (&changed.default_permissions, None),
            ] {
                for update in updates {
                    if update.slug.is_empty() {
                        continue;
                    }
                    let value = active.unwrap_or(update.slug == PERMISSION_SEND_MESSAGE);
                    if perms.insert(update.slug.clone(), value) != Some(value) {
                        changed_any = true;
                    }
                }
            }
            if changed_any {
                touched.push(key);
            }
        }
        if touched.is_empty() {
            return;
        }
        self.patch_generation = self.patch_generation.wrapping_add(1);
        for key in touched {
            cx.emit(ChannelPermissionsEvent::Changed {
                clan_id: key.clan_id,
                channel_id: key.channel_id,
            });
        }
        cx.notify();
    }

    fn role_event_affects_me(&self, role_event: &realtime::RoleEvent, cx: &App) -> bool {
        let Some(me) = BadgeService::try_global(cx).and_then(|b| b.read(cx).current_user_id(cx))
        else {
            return false;
        };
        if role_event.user_add_ids.contains(&me.get())
            || role_event.user_remove_ids.contains(&me.get())
        {
            return true;
        }
        if role_event.active_permission_ids.is_empty()
            && role_event.remove_permission_ids.is_empty()
        {
            return false;
        }
        let Some(role) = role_event.role.as_ref() else {
            return false;
        };
        ClanMembersStore::try_global(cx)
            .and_then(|members| {
                members
                    .read(cx)
                    .self_role_ids(ClanId(role.clan_id))
                    .map(|ids| ids.contains(&role.id))
            })
            .unwrap_or(false)
    }

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
        self.failed_at.clear();
    }

    fn refetch_cached(&mut self, cx: &mut Context<Self>) {
        let keys = self.cache.iter().map(|(key, _)| *key).collect::<Vec<_>>();
        let generation = self.reset_generation;
        self._refetch = cx.spawn(async move |this, cx| {
            for chunk in keys.chunks(REFETCH_CONCURRENCY) {
                let started = this.update(cx, |this, cx| {
                    if this.reset_generation != generation {
                        return false;
                    }
                    for key in chunk {
                        this.fetch(key.clan_id, key.channel_id, cx);
                    }
                    true
                });
                if !matches!(started, Ok(true)) {
                    return;
                }
                cx.background_executor().timer(REFETCH_STEP_DELAY).await;
            }
        });
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.cache.clear();
        self.loading.clear();
        self.failed_at.clear();
        cx.notify();
    }

    pub fn has_permission(&self, slug: &str, clan_id: ClanId, channel_id: ChannelId) -> bool {
        self.permission_value(slug, clan_id, channel_id)
            .unwrap_or(false)
    }

    pub fn permission_value(
        &self,
        slug: &str,
        clan_id: ClanId,
        channel_id: ChannelId,
    ) -> Option<bool> {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        if self.cache.is_invalidated(&key) {
            return None;
        }
        self.cache
            .get(&key)
            .and_then(|perms| perms.get(slug).copied())
    }

    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        self.cache.touch(&key);
        if self.cache.is_fresh(&key, crate::CACHE_TTL) {
            return;
        }
        if self
            .failed_at
            .get(&key)
            .is_some_and(|at| at.elapsed() < FAILED_FETCH_BACKOFF)
        {
            return;
        }
        self.fetch(clan_id, channel_id, cx);
    }

    fn fetch(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let key = ChannelPermissionKey {
            clan_id,
            channel_id,
        };
        if self.loading.contains(&key) {
            return;
        }
        self.loading.insert(key);
        let api = self.api.clone();
        let generation = self.reset_generation;
        let patched_at_start = self.patch_generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .list_user_permission_in_channel(clan_id.get(), channel_id.get())
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading.remove(&key);
                match result {
                    Ok(resp) => {
                        this.failed_at.remove(&key);
                        if this.patch_generation != patched_at_start {
                            return;
                        }
                        let perms = permissions_from_response(&resp);
                        this.cache.insert(key, perms, None);
                        cx.emit(ChannelPermissionsEvent::Changed {
                            clan_id,
                            channel_id,
                        });
                        cx.notify();
                    }
                    Err(e) => {
                        this.failed_at.insert(key, Instant::now());
                        tracing::error!(
                            "list_user_permission_in_channel failed for {channel_id}: {e}"
                        );
                    }
                }
            });
        })
        .detach();
    }
}

fn permissions_from_response(
    resp: &api::UserPermissionInChannelListResponse,
) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    if let Some(list) = &resp.permissions {
        for perm in &list.permissions {
            if !perm.slug.is_empty() {
                map.insert(perm.slug.clone(), perm.active != 0);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CLAN: ClanId = ClanId(1);
    const TEST_CHANNEL: ChannelId = ChannelId(7);

    fn test_store() -> ChannelPermissionsStore {
        ChannelPermissionsStore {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNEL_PERMISSIONS)),
            loading: HashSet::new(),
            failed_at: HashMap::new(),
            reset_generation: 0,
            patch_generation: 0,
            api: Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            )),
            _conn_watch: Task::ready(()),
            _refetch: Task::ready(()),
        }
    }

    const TEST_ME: i64 = 55;

    fn init_permissions_store(cx: &mut App) -> Entity<ChannelPermissionsStore> {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: TEST_ME.to_string(),
                ..Default::default()
            })
        });
        BadgeService::init(auth_state, cx);
        crate::clan::ClanList::init(api.clone(), cx);
        crate::channel::ChannelList::init(api.clone(), cx);
        crate::clan_members::ClanMembersStore::init(api.clone(), cx);
        cx.new(|cx| ChannelPermissionsStore::new(api, cx))
    }

    fn permission_update(slug: &str) -> api::PermissionUpdate {
        api::PermissionUpdate {
            permission_id: 0,
            slug: slug.into(),
            r#type: 0,
        }
    }

    fn permission_changed(user_id: i64) -> RealtimeEvent {
        RealtimeEvent::Unhandled(realtime::envelope::Message::PermissionChangedEvent(
            realtime::PermissionChangedEvent {
                user_id,
                channel_id: TEST_CHANNEL.get(),
                add_permissions: vec![permission_update(PERMISSION_MANAGE_THREAD)],
                remove_permissions: vec![permission_update(PERMISSION_DELETE_MESSAGE)],
                default_permissions: vec![
                    permission_update(PERMISSION_SEND_MESSAGE),
                    permission_update("kick-member"),
                ],
            },
        ))
    }

    #[gpui::test]
    fn a_permission_change_for_me_rewrites_the_cached_channel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_permissions_store(cx);
            store.update(cx, |store, cx| {
                seed_granted(store);
                store.handle_permission_changed(&permission_changed(TEST_ME), cx);
                assert_eq!(
                    store.permission_value(PERMISSION_MANAGE_THREAD, TEST_CLAN, TEST_CHANNEL),
                    Some(true),
                    "add_permissions grants"
                );
                assert_eq!(
                    store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
                    Some(false),
                    "remove_permissions revokes"
                );
                assert_eq!(
                    store.permission_value(PERMISSION_SEND_MESSAGE, TEST_CLAN, TEST_CHANNEL),
                    Some(true),
                    "send-message is the one default that stays on, as in React"
                );
                assert_eq!(
                    store.permission_value("kick-member", TEST_CLAN, TEST_CHANNEL),
                    Some(false),
                    "every other default is denied"
                );
            });
        });
    }

    #[gpui::test]
    fn a_fetch_that_started_before_the_change_cannot_overwrite_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_permissions_store(cx);
            store.update(cx, |store, cx| {
                seed_granted(store);
                let key = ChannelPermissionKey {
                    clan_id: TEST_CLAN,
                    channel_id: TEST_CHANNEL,
                };

                let patched_at_start = store.patch_generation;
                store.handle_permission_changed(&permission_changed(TEST_ME), cx);
                assert_eq!(
                    store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
                    Some(false),
                    "precondition: the event revoked delete-message"
                );

                assert_ne!(
                    store.patch_generation, patched_at_start,
                    "the patch must be visible to a fetch that is already in flight"
                );

                if store.patch_generation == patched_at_start {
                    store.cache.insert(
                        key,
                        HashMap::from([(PERMISSION_DELETE_MESSAGE.to_string(), true)]),
                        None,
                    );
                }

                assert_eq!(
                    store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
                    Some(false),
                    "a response that predates the revocation must not hand the permission back"
                );
                assert_eq!(
                    store.permission_value(PERMISSION_MANAGE_THREAD, TEST_CLAN, TEST_CHANNEL),
                    Some(true),
                    "discarding the response must not blank the whole channel — a stale entry \
                     reads as None, which denies every permission at once"
                );
            });
        });
    }

    #[gpui::test]
    fn a_permission_change_for_someone_else_is_ignored(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_permissions_store(cx);
            store.update(cx, |store, cx| {
                seed_granted(store);
                store.handle_permission_changed(&permission_changed(TEST_ME + 1), cx);
                assert_eq!(
                    store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
                    Some(true)
                );
                assert_eq!(
                    store.permission_value(PERMISSION_MANAGE_THREAD, TEST_CLAN, TEST_CHANNEL),
                    None
                );
            });
        });
    }

    fn seed_granted(store: &mut ChannelPermissionsStore) {
        store.cache.insert(
            ChannelPermissionKey {
                clan_id: TEST_CLAN,
                channel_id: TEST_CHANNEL,
            },
            HashMap::from([(PERMISSION_DELETE_MESSAGE.to_string(), true)]),
            None,
        );
    }

    #[test]
    fn stale_entry_reads_as_absent() {
        let mut store = test_store();
        seed_granted(&mut store);
        assert_eq!(
            store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
            Some(true)
        );
        assert!(store.has_permission(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL));

        store.invalidate();

        assert!(
            store
                .cache
                .get(&ChannelPermissionKey {
                    clan_id: TEST_CLAN,
                    channel_id: TEST_CHANNEL,
                })
                .is_some()
        );
        assert_eq!(
            store.permission_value(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL),
            None
        );
        assert!(!store.has_permission(PERMISSION_DELETE_MESSAGE, TEST_CLAN, TEST_CHANNEL));
    }

    #[test]
    fn overridden_slugs_cover_the_three_channel_scoped_permissions() {
        assert!(is_overridden_slug(PERMISSION_MANAGE_THREAD));
        assert!(is_overridden_slug(PERMISSION_SEND_MESSAGE));
        assert!(is_overridden_slug(PERMISSION_DELETE_MESSAGE));
        assert!(!is_overridden_slug("manage-clan"));
        assert!(!is_overridden_slug("clan-owner"));
    }

    #[test]
    fn permissions_from_response_maps_active_flag() {
        let resp = api::UserPermissionInChannelListResponse {
            permissions: Some(api::PermissionList {
                max_level_permission: 0,
                permissions: vec![
                    api::Permission {
                        slug: "manage-thread".into(),
                        active: 1,
                        ..Default::default()
                    },
                    api::Permission {
                        slug: "send-message".into(),
                        active: 0,
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };
        let perms = permissions_from_response(&resp);
        assert_eq!(perms.get("manage-thread"), Some(&true));
        assert_eq!(perms.get("send-message"), Some(&false));
    }
}
