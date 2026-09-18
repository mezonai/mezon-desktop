use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::badge::BadgeService;
use crate::ids::{ChannelId, ClanId, RoleId, UserId};
use crate::permissions::PermissionStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_ENTITIES: usize = 128;

pub const OVERRIDE_TYPE_NEUTRAL: i32 = 0;
pub const OVERRIDE_TYPE_ALLOW: i32 = 1;
pub const OVERRIDE_TYPE_DENY: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionEntity {
    Role(RoleId),
    User(UserId),
}

impl PermissionEntity {
    fn role_id(&self) -> i64 {
        match self {
            Self::Role(id) => id.get(),
            Self::User(_) => 0,
        }
    }

    fn user_id(&self) -> i64 {
        match self {
            Self::Role(_) => 0,
            Self::User(id) => id.get(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EntityKey {
    channel_id: ChannelId,
    entity: PermissionEntity,
}

#[derive(Debug, Clone)]
pub enum ChannelRolePermissionsEvent {
    Changed {
        channel_id: ChannelId,
        entity: PermissionEntity,
    },
    SaveFailed {
        channel_id: ChannelId,
        entity: PermissionEntity,
    },
}

pub struct ChannelRolePermissionsStore {
    cache: KeyedCache<EntityKey, HashMap<i64, bool>>,
    loading: HashSet<EntityKey>,
    saving: HashSet<EntityKey>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalChannelRolePermissionsStore(Entity<ChannelRolePermissionsStore>);
impl Global for GlobalChannelRolePermissionsStore {}

impl EventEmitter<ChannelRolePermissionsEvent> for ChannelRolePermissionsStore {}

impl ChannelRolePermissionsStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelRolePermissionsStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::PermissionSet, &entity, |this, event, cx| {
                this.handle_permission_set(event, cx);
            });
        });
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_ENTITIES)),
            loading: HashSet::new(),
            saving: HashSet::new(),
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelRolePermissionsStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelRolePermissionsStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cache.clear();
        self.loading.clear();
        self.saving.clear();
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
                    if this
                        .update(cx, |this, cx| {
                            this.cache.clear();
                            cx.notify();
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

    fn handle_permission_set(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::PermissionSetEvent(set)) = event
        else {
            return;
        };
        let me = BadgeService::try_global(cx).and_then(|b| b.read(cx).current_user_id(cx));
        let caller = set.caller.parse::<i64>().ok();
        if let (Some(me), Some(caller)) = (me, caller)
            && me.get() == caller
        {
            return;
        }
        let Some(entity) = permission_entity_of(set) else {
            return;
        };
        let key = EntityKey {
            channel_id: ChannelId(set.channel_id),
            entity,
        };
        let Some(perms) = self.cache.get_mut(&key) else {
            return;
        };
        let mut changed = false;
        for update in &set.permission_updates {
            if update.r#type == OVERRIDE_TYPE_NEUTRAL || update.permission_id == 0 {
                continue;
            }
            let active = update.r#type == OVERRIDE_TYPE_ALLOW;
            if perms.insert(update.permission_id, active) != Some(active) {
                changed = true;
            }
        }
        if !changed {
            return;
        }
        cx.emit(ChannelRolePermissionsEvent::Changed {
            channel_id: key.channel_id,
            entity,
        });
        cx.notify();
    }

    pub fn is_loaded(&self, channel_id: ChannelId, entity: PermissionEntity) -> bool {
        self.cache.contains(&EntityKey { channel_id, entity })
    }

    pub fn is_saving(&self, channel_id: ChannelId, entity: PermissionEntity) -> bool {
        self.saving.contains(&EntityKey { channel_id, entity })
    }

    pub fn permission_active(
        &self,
        channel_id: ChannelId,
        entity: PermissionEntity,
        permission_id: i64,
    ) -> Option<bool> {
        self.cache
            .get(&EntityKey { channel_id, entity })
            .and_then(|overrides| overrides.get(&permission_id).copied())
    }

    pub fn ensure_loaded(
        &mut self,
        channel_id: ChannelId,
        entity: PermissionEntity,
        cx: &mut Context<Self>,
    ) {
        let key = EntityKey { channel_id, entity };
        if self.cache.contains(&key) || !self.loading.insert(key) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .get_permission_by_role_id_channel_id(
                    entity.role_id(),
                    channel_id.get(),
                    entity.user_id(),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&key);
                match result {
                    Ok(response) => {
                        this.cache
                            .insert(key, overrides_from_response(&response), None);
                        cx.emit(ChannelRolePermissionsEvent::Changed { channel_id, entity });
                        cx.notify();
                    }
                    Err(error) => tracing::error!(
                        "get_permission_by_role_id_channel_id failed for {channel_id}: {error}"
                    ),
                }
            });
        })
        .detach();
    }

    pub fn save(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        entity: PermissionEntity,
        pending: &HashMap<i64, i32>,
        cx: &mut Context<Self>,
    ) {
        let key = EntityKey { channel_id, entity };
        if !self.cache.contains(&key) {
            cx.emit(ChannelRolePermissionsEvent::SaveFailed { channel_id, entity });
            cx.notify();
            return;
        }
        if !self.saving.insert(key) {
            return;
        }
        let definitions = PermissionStore::try_global(cx)
            .map(|store| {
                store
                    .read(cx)
                    .channel_scoped_definitions()
                    .map(|definition| (definition.id, definition.slug.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let persisted = self.cache.get(&key);
        let updates = build_permission_snapshot(&definitions, pending, persisted);
        let max_permission_id = resolve_max_permission_id(clan_id, cx);
        let applied = applied_overrides(&updates);

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .set_role_channel_permission(
                    entity.role_id(),
                    channel_id.get(),
                    entity.user_id(),
                    max_permission_id,
                    updates,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving.remove(&key);
                match result {
                    Ok(()) => {
                        this.cache.insert(key, applied, None);
                        cx.emit(ChannelRolePermissionsEvent::Changed { channel_id, entity });
                        cx.notify();
                    }
                    Err(error) => {
                        tracing::error!(
                            "set_role_channel_permission failed for {channel_id}: {error}"
                        );
                        cx.emit(ChannelRolePermissionsEvent::SaveFailed { channel_id, entity });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

fn resolve_max_permission_id(clan_id: ClanId, cx: &App) -> i64 {
    crate::roles::RolesStore::try_global(cx)
        .map(|roles| roles.read(cx).resolve_max_permission_id(clan_id, cx))
        .unwrap_or(0)
}

fn overrides_from_response(
    response: &api::PermissionRoleChannelListEventResponse,
) -> HashMap<i64, bool> {
    response
        .permission_role_channel
        .iter()
        .map(|entry| (entry.permission_id, entry.active))
        .collect()
}

fn build_permission_snapshot(
    definitions: &[(i64, String)],
    pending: &HashMap<i64, i32>,
    persisted: Option<&HashMap<i64, bool>>,
) -> Vec<api::PermissionUpdate> {
    definitions
        .iter()
        .map(|(id, slug)| {
            let r#type = match pending.get(id) {
                Some(pending_type) => *pending_type,
                None => match persisted.and_then(|overrides| overrides.get(id)) {
                    Some(true) => OVERRIDE_TYPE_ALLOW,
                    Some(false) => OVERRIDE_TYPE_DENY,
                    None => OVERRIDE_TYPE_NEUTRAL,
                },
            };
            api::PermissionUpdate {
                permission_id: *id,
                slug: slug.clone(),
                r#type,
            }
        })
        .collect()
}

fn applied_overrides(updates: &[api::PermissionUpdate]) -> HashMap<i64, bool> {
    updates
        .iter()
        .filter(|update| update.r#type != OVERRIDE_TYPE_NEUTRAL)
        .map(|update| (update.permission_id, update.r#type == OVERRIDE_TYPE_ALLOW))
        .collect()
}

fn permission_entity_of(set: &realtime::PermissionSetEvent) -> Option<PermissionEntity> {
    if set.role_id != 0 {
        return Some(PermissionEntity::Role(RoleId(set.role_id)));
    }
    if set.user_id != 0 {
        return Some(PermissionEntity::User(UserId(set.user_id)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CALLER: i64 = 91;
    const TEST_CHANNEL: ChannelId = ChannelId(7);
    const TEST_ROLE: RoleId = RoleId(3);

    fn init_role_permissions_store(cx: &mut App) -> Entity<ChannelRolePermissionsStore> {
        let api = Arc::new(AppApi::new(
            Arc::new(mezon_client::TransportClient::new(String::new())),
            String::new(),
        ));
        crate::realtime::RealtimeDispatch::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            crate::AuthState::Authenticated(mezon_client::Session {
                user_id: TEST_CALLER.to_string(),
                ..Default::default()
            })
        });
        BadgeService::init(auth_state, cx);
        cx.new(|cx| ChannelRolePermissionsStore::new(api, cx))
    }

    fn permission_set(caller: i64) -> RealtimeEvent {
        RealtimeEvent::Unhandled(realtime::envelope::Message::PermissionSetEvent(
            realtime::PermissionSetEvent {
                caller: caller.to_string(),
                role_id: TEST_ROLE.get(),
                user_id: 0,
                channel_id: TEST_CHANNEL.get(),
                permission_updates: vec![
                    api::PermissionUpdate {
                        permission_id: 1,
                        slug: "send-message".into(),
                        r#type: OVERRIDE_TYPE_ALLOW,
                    },
                    api::PermissionUpdate {
                        permission_id: 2,
                        slug: "delete-message".into(),
                        r#type: OVERRIDE_TYPE_DENY,
                    },
                    api::PermissionUpdate {
                        permission_id: 3,
                        slug: "manage-thread".into(),
                        r#type: OVERRIDE_TYPE_NEUTRAL,
                    },
                ],
            },
        ))
    }

    fn seed_role(store: &mut ChannelRolePermissionsStore) {
        store.cache.insert(
            EntityKey {
                channel_id: TEST_CHANNEL,
                entity: PermissionEntity::Role(TEST_ROLE),
            },
            HashMap::from([(1, false), (2, true), (3, true)]),
            None,
        );
    }

    fn cached(store: &ChannelRolePermissionsStore, id: i64) -> Option<bool> {
        store
            .cache
            .get(&EntityKey {
                channel_id: TEST_CHANNEL,
                entity: PermissionEntity::Role(TEST_ROLE),
            })
            .and_then(|perms| perms.get(&id).copied())
    }

    #[gpui::test]
    fn a_permission_set_from_someone_else_applies_allow_and_deny(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_role_permissions_store(cx);
            store.update(cx, |store, cx| {
                seed_role(store);
                store.handle_permission_set(&permission_set(TEST_CALLER + 1), cx);
                assert_eq!(cached(store, 1), Some(true), "allow flips on");
                assert_eq!(cached(store, 2), Some(false), "deny flips off");
                assert_eq!(
                    cached(store, 3),
                    Some(true),
                    "neutral is skipped, exactly as React filters type 0"
                );
            });
        });
    }

    #[gpui::test]
    fn my_own_permission_set_echo_is_ignored(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_role_permissions_store(cx);
            store.update(cx, |store, cx| {
                seed_role(store);
                store.handle_permission_set(&permission_set(TEST_CALLER), cx);
                assert_eq!(cached(store, 1), Some(false));
                assert_eq!(cached(store, 2), Some(true));
            });
        });
    }

    #[gpui::test]
    fn a_permission_set_for_an_uncached_entity_is_dropped(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let store = init_role_permissions_store(cx);
            store.update(cx, |store, cx| {
                store.handle_permission_set(&permission_set(TEST_CALLER + 1), cx);
                assert!(cached(store, 1).is_none());
            });
        });
    }

    #[test]
    fn permission_entity_prefers_role_then_user() {
        let mut event = realtime::PermissionSetEvent {
            role_id: 3,
            user_id: 9,
            ..Default::default()
        };
        assert_eq!(
            permission_entity_of(&event),
            Some(PermissionEntity::Role(RoleId(3)))
        );
        event.role_id = 0;
        assert_eq!(
            permission_entity_of(&event),
            Some(PermissionEntity::User(UserId(9)))
        );
        event.user_id = 0;
        assert_eq!(permission_entity_of(&event), None);
    }

    fn definitions() -> Vec<(i64, String)> {
        vec![
            (1, "send-message".into()),
            (2, "delete-message".into()),
            (3, "manage-thread".into()),
        ]
    }

    #[test]
    fn snapshot_without_persisted_overrides_would_wipe_them_all() {
        let updates = build_permission_snapshot(&definitions(), &HashMap::new(), None);
        assert!(
            updates.iter().all(|u| u.r#type == OVERRIDE_TYPE_NEUTRAL),
            "an unloaded entity snapshots to all-neutral, which is why save() must refuse it"
        );
    }

    #[test]
    fn snapshot_sends_every_channel_scoped_permission_not_just_the_diff() {
        let pending = HashMap::from([(1, OVERRIDE_TYPE_DENY)]);
        let persisted = HashMap::from([(2, true)]);
        let updates = build_permission_snapshot(&definitions(), &pending, Some(&persisted));

        assert_eq!(updates.len(), 3);
        let by_id = updates
            .iter()
            .map(|u| (u.permission_id, u.r#type))
            .collect::<HashMap<_, _>>();
        assert_eq!(by_id[&1], OVERRIDE_TYPE_DENY);
        assert_eq!(by_id[&2], OVERRIDE_TYPE_ALLOW);
        assert_eq!(by_id[&3], OVERRIDE_TYPE_NEUTRAL);
        assert_eq!(updates[0].slug, "send-message");
    }

    #[test]
    fn snapshot_falls_back_to_neutral_without_persisted_overrides() {
        let updates = build_permission_snapshot(&definitions(), &HashMap::new(), None);
        assert!(updates.iter().all(|u| u.r#type == OVERRIDE_TYPE_NEUTRAL));
    }

    #[test]
    fn applied_overrides_drop_neutral_and_map_allow_deny() {
        let updates = vec![
            api::PermissionUpdate {
                permission_id: 1,
                slug: "send-message".into(),
                r#type: OVERRIDE_TYPE_ALLOW,
            },
            api::PermissionUpdate {
                permission_id: 2,
                slug: "delete-message".into(),
                r#type: OVERRIDE_TYPE_DENY,
            },
            api::PermissionUpdate {
                permission_id: 3,
                slug: "manage-thread".into(),
                r#type: OVERRIDE_TYPE_NEUTRAL,
            },
        ];
        let applied = applied_overrides(&updates);

        assert_eq!(applied.get(&1), Some(&true));
        assert_eq!(applied.get(&2), Some(&false));
        assert_eq!(applied.get(&3), None);
    }

    #[test]
    fn entity_maps_to_exactly_one_id_field() {
        let role = PermissionEntity::Role(RoleId(7));
        assert_eq!((role.role_id(), role.user_id()), (7, 0));
        let user = PermissionEntity::User(UserId(9));
        assert_eq!((user.role_id(), user.user_id()), (0, 9));
    }
}
