use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::AuthState;
use crate::channel_permissions::{ChannelPermissionsStore, is_overridden_slug};
use crate::clan::ClanList;
use crate::ids::{ChannelId, ClanId, UserId};

pub const PERMISSION_CLAN_OWNER: &str = "clan-owner";
pub const PERMISSION_ADMINISTRATOR: &str = "administrator";
pub const PERMISSION_MANAGE_CHANNEL: &str = "manage-channel";
pub const PERMISSION_MANAGE_CLAN: &str = "manage-clan";

pub const PERMISSION_SCOPE_CHANNEL: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDefinition {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub level: i32,
    pub scope: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClanSettingsPermissions {
    pub is_clan_owner: bool,
    pub has_manage_clan: bool,
    pub has_manage_channel: bool,
    pub has_administrator: bool,
}

impl ClanSettingsPermissions {
    pub fn none() -> Self {
        Self {
            is_clan_owner: false,
            has_manage_clan: false,
            has_manage_channel: false,
            has_administrator: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PermissionEvent {
    Changed { clan_id: Option<ClanId> },
}

pub struct PermissionStore {
    catalog: HashMap<String, i32>,
    definitions: Vec<PermissionDefinition>,
    catalog_loaded: bool,
    catalog_loading: bool,
    max_level_by_clan: HashMap<ClanId, i32>,
    loading_clans: HashSet<ClanId>,
    reset_generation: u64,
    api: Arc<AppApi>,
    auth_state: Entity<AuthState>,
    _conn_watch: Task<()>,
}

struct GlobalPermissionStore(Entity<PermissionStore>);
impl Global for GlobalPermissionStore {}

impl EventEmitter<PermissionEvent> for PermissionStore {}

impl PermissionStore {
    pub fn init(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, auth_state, cx));
        cx.set_global(GlobalPermissionStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPermissionStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPermissionStore>()
            .map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, auth_state: Entity<AuthState>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            catalog: HashMap::new(),
            definitions: Vec::new(),
            catalog_loaded: false,
            catalog_loading: false,
            max_level_by_clan: HashMap::new(),
            loading_clans: HashSet::new(),
            reset_generation: 0,
            api,
            auth_state,
            _conn_watch: conn_watch,
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
                            this.max_level_by_clan.clear();
                            if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
                                this.load_clan_permissions(clan_id, cx);
                            }
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

    fn current_user_id(&self, cx: &App) -> Option<UserId> {
        match self.auth_state.read(cx) {
            AuthState::Authenticated(session) | AuthState::Connecting(session) => {
                session.user_id.parse::<i64>().ok().map(UserId)
            }
            _ => None,
        }
    }

    fn is_clan_owner(&self, clan_id: ClanId, cx: &App) -> bool {
        let Some(user_id) = self.current_user_id(cx) else {
            return false;
        };
        ClanList::global(cx)
            .read(cx)
            .clan_by_id(clan_id)
            .is_some_and(|clan| clan.creator_id == user_id)
    }

    fn has_permission_level(&self, max_level: Option<i32>, slug: &str) -> bool {
        let Some(max_level) = max_level else {
            return false;
        };
        let Some(required_level) = self.catalog.get(slug) else {
            return false;
        };
        *required_level <= max_level
    }

    pub fn check(
        &self,
        clan_id: ClanId,
        channel_id: Option<ChannelId>,
        slug: &str,
        cx: &App,
    ) -> bool {
        if is_overridden_slug(slug) {
            let Some(channel_id) = channel_id else {
                return false;
            };
            return ChannelPermissionsStore::try_global(cx)
                .is_some_and(|store| store.read(cx).has_permission(slug, clan_id, channel_id));
        }
        if slug == PERMISSION_CLAN_OWNER {
            return self.is_clan_owner(clan_id, cx);
        }
        if self.is_clan_owner(clan_id, cx) {
            return true;
        }
        let max_level = self.max_level_by_clan.get(&clan_id).copied();
        self.has_permission_level(max_level, slug)
    }

    pub fn check_permission(&self, clan_id: ClanId, slug: &str, cx: &App) -> bool {
        self.check(clan_id, None, slug, cx)
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.max_level_by_clan.clear();
        self.loading_clans.clear();
        cx.notify();
    }

    pub fn current_permission_level(&self, clan_id: ClanId, cx: &App) -> Option<i32> {
        if self.is_clan_owner(clan_id, cx) {
            return Some(i32::MAX);
        }
        self.max_level_by_clan.get(&clan_id).copied()
    }

    pub fn has_clan_permissions_loaded(&self, clan_id: ClanId, cx: &App) -> bool {
        self.is_clan_owner(clan_id, cx) || self.max_level_by_clan.contains_key(&clan_id)
    }

    pub fn permission_definitions(&self) -> &[PermissionDefinition] {
        &self.definitions
    }

    pub fn channel_scoped_definitions(&self) -> impl Iterator<Item = &PermissionDefinition> {
        self.definitions
            .iter()
            .filter(|definition| definition.scope == PERMISSION_SCOPE_CHANNEL)
    }

    pub fn ensure_catalog_loaded(&mut self, cx: &mut Context<Self>) {
        self.load_permission_catalog(cx);
    }

    pub fn clan_settings_permissions(&self, clan_id: ClanId, cx: &App) -> ClanSettingsPermissions {
        let is_clan_owner = self.is_clan_owner(clan_id, cx);
        if is_clan_owner {
            return ClanSettingsPermissions {
                is_clan_owner: true,
                has_manage_clan: true,
                has_manage_channel: true,
                has_administrator: true,
            };
        }
        let max_level = self.max_level_by_clan.get(&clan_id).copied();
        ClanSettingsPermissions {
            is_clan_owner: false,
            has_manage_clan: self.has_permission_level(max_level, PERMISSION_MANAGE_CLAN),
            has_manage_channel: self.has_permission_level(max_level, PERMISSION_MANAGE_CHANNEL),
            has_administrator: self.has_permission_level(max_level, PERMISSION_ADMINISTRATOR),
        }
    }

    pub fn load_clan_permissions(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.load_clan_permissions_task(clan_id, cx).detach();
    }

    pub fn load_clan_permissions_task(
        &mut self,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.load_permission_catalog(cx);
        if self.max_level_by_clan.contains_key(&clan_id) {
            return Task::ready(());
        }
        self.fetch_clan_permissions(clan_id, cx)
    }

    pub fn reload_clan_permissions(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.load_permission_catalog(cx);
        self.fetch_clan_permissions(clan_id, cx).detach();
    }

    fn fetch_clan_permissions(&mut self, clan_id: ClanId, cx: &mut Context<Self>) -> Task<()> {
        if !self.loading_clans.insert(clan_id) {
            return Task::ready(());
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.get_clan_user_role(clan_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading_clans.remove(&clan_id);
                match result {
                    Ok(role_list) => {
                        let previous = this
                            .max_level_by_clan
                            .insert(clan_id, role_list.max_level_permission);
                        if previous == Some(role_list.max_level_permission) {
                            return;
                        }
                        cx.emit(PermissionEvent::Changed {
                            clan_id: Some(clan_id),
                        });
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!(
                            "get_clan_user_role failed for clan {}: {err}",
                            clan_id.get()
                        );
                    }
                }
            });
        })
    }

    fn load_permission_catalog(&mut self, cx: &mut Context<Self>) {
        if self.catalog_loaded || self.catalog_loading {
            return;
        }
        self.catalog_loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.get_list_permission().await;
            let _ = this.update(cx, |this, cx| {
                this.catalog_loading = false;
                match result {
                    Ok(list) => {
                        let definitions = definitions_from_permission_list(list);
                        if definitions.is_empty() {
                            tracing::warn!(
                                "get_list_permission returned an empty catalog; keeping it unloaded so the next access retries"
                            );
                            return;
                        }
                        this.catalog = definitions
                            .iter()
                            .map(|definition| (definition.slug.clone(), definition.level))
                            .collect();
                        this.definitions = definitions;
                        this.catalog_loaded = true;
                        cx.emit(PermissionEvent::Changed { clan_id: None });
                        cx.notify();
                    }
                    Err(err) => tracing::error!("get_list_permission failed: {err}"),
                }
            });
        })
        .detach();
    }
}

fn definitions_from_permission_list(list: api::PermissionList) -> Vec<PermissionDefinition> {
    list.permissions
        .into_iter()
        .filter(|permission| !permission.slug.is_empty())
        .map(|permission| PermissionDefinition {
            id: permission.id,
            slug: permission.slug,
            title: permission.title,
            description: permission.description,
            level: permission.level,
            scope: permission.scope,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use mezon_client::{Session, TransportClient};

    use super::*;
    use crate::channel_permissions::{
        ChannelPermissionsStore, PERMISSION_DELETE_MESSAGE, PERMISSION_MANAGE_THREAD,
        PERMISSION_SEND_MESSAGE,
    };
    use crate::clan::Clan;
    use crate::realtime::RealtimeDispatch;

    const TEST_CLAN: ClanId = ClanId(1);
    const TEST_OWNER: UserId = UserId(42);

    fn test_api() -> Arc<AppApi> {
        Arc::new(AppApi::new(
            Arc::new(TransportClient::new(String::new())),
            String::new(),
        ))
    }

    fn owned_clan() -> Clan {
        Clan {
            id: TEST_CLAN,
            creator_id: TEST_OWNER,
            name: "Test".into(),
            avatar_url: None,
            banner_url: None,
            badge_count: 0,
            has_unread: false,
            muted: false,
            welcome_channel_id: None,
            status: 0,
            is_onboarding: false,
            is_community: false,
            prevent_anonymous: false,
            community_banner: String::new(),
            about: String::new(),
            description: String::new(),
            short_url: String::new(),
        }
    }

    fn init_stores(auth: AuthState, cx: &mut App) -> Entity<PermissionStore> {
        let api = test_api();
        RealtimeDispatch::init(api.clone(), cx);
        ClanList::init(api.clone(), cx);
        ChannelPermissionsStore::init(api.clone(), cx);
        let auth_state = cx.new(|_| auth);
        PermissionStore::init(api, auth_state, cx)
    }

    fn permission(id: i64, slug: &str, level: i32, scope: i32) -> api::Permission {
        api::Permission {
            id,
            title: slug.to_uppercase(),
            slug: slug.into(),
            description: String::new(),
            active: 0,
            scope,
            level,
        }
    }

    #[gpui::test]
    fn check_permission_is_denied_after_reset(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_stores(AuthState::NotAuthenticated, cx);
            store.update(cx, |store, cx| {
                store.catalog.insert(PERMISSION_MANAGE_CLAN.into(), 5);
                store.max_level_by_clan.insert(TEST_CLAN, 9);
                store.loading_clans.insert(TEST_CLAN);

                assert!(store.check_permission(TEST_CLAN, PERMISSION_MANAGE_CLAN, cx));
                assert!(store.check(TEST_CLAN, None, PERMISSION_MANAGE_CLAN, cx));

                store.reset(cx);

                assert!(!store.check_permission(TEST_CLAN, PERMISSION_MANAGE_CLAN, cx));
                assert!(!store.check(TEST_CLAN, None, PERMISSION_MANAGE_CLAN, cx));
                assert!(store.loading_clans.is_empty());
                assert!(store.catalog.contains_key(PERMISSION_MANAGE_CLAN));
            });
        });
    }

    /// `check` is level-based, and mezon-api seeds manage-channel below manage-clan
    /// (`migrate/sql/20260408173801_initial_insert.sql`), so holding manage-clan already grants
    /// manage-channel. Anything that ORs the two slugs together is testing nothing.
    #[gpui::test]
    fn manage_clan_level_already_covers_manage_channel(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_stores(AuthState::NotAuthenticated, cx);
            store.update(cx, |store, cx| {
                store.catalog.insert(PERMISSION_MANAGE_CHANNEL.into(), 2);
                store.catalog.insert(PERMISSION_MANAGE_CLAN.into(), 3);

                store.max_level_by_clan.insert(TEST_CLAN, 3);
                assert!(store.check(TEST_CLAN, None, PERMISSION_MANAGE_CLAN, cx));
                assert!(store.check(TEST_CLAN, None, PERMISSION_MANAGE_CHANNEL, cx));

                store.max_level_by_clan.insert(TEST_CLAN, 1);
                assert!(!store.check(TEST_CLAN, None, PERMISSION_MANAGE_CLAN, cx));
                assert!(!store.check(TEST_CLAN, None, PERMISSION_MANAGE_CHANNEL, cx));
            });
        });
    }

    #[gpui::test]
    fn overridden_slug_without_channel_is_denied_for_clan_owner(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let session = Session {
                user_id: TEST_OWNER.get().to_string(),
                ..Default::default()
            };
            let store = init_stores(AuthState::Authenticated(session), cx);
            ClanList::global(cx).update(cx, |clans, cx| clans.update_clans(vec![owned_clan()], cx));

            store.update(cx, |store, cx| {
                assert!(store.check(TEST_CLAN, None, PERMISSION_CLAN_OWNER, cx));
                assert!(store.check(TEST_CLAN, None, PERMISSION_MANAGE_CLAN, cx));

                assert!(!store.check(TEST_CLAN, None, PERMISSION_SEND_MESSAGE, cx));
                assert!(!store.check(TEST_CLAN, None, PERMISSION_DELETE_MESSAGE, cx));
                assert!(!store.check(TEST_CLAN, None, PERMISSION_MANAGE_THREAD, cx));
                assert!(!store.check_permission(TEST_CLAN, PERMISSION_SEND_MESSAGE, cx));
            });
        });
    }

    #[gpui::test]
    fn clan_owner_has_administrator_in_settings_permissions(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let session = Session {
                user_id: TEST_OWNER.get().to_string(),
                ..Default::default()
            };
            let store = init_stores(AuthState::Authenticated(session), cx);
            ClanList::global(cx).update(cx, |clans, cx| clans.update_clans(vec![owned_clan()], cx));

            store.update(cx, |store, cx| {
                assert!(
                    store
                        .clan_settings_permissions(TEST_CLAN, cx)
                        .has_administrator
                );
            });
        });
    }

    #[gpui::test]
    fn administrator_level_grants_settings_administrator(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_stores(AuthState::NotAuthenticated, cx);
            store.update(cx, |store, cx| {
                store.catalog.insert(PERMISSION_ADMINISTRATOR.into(), 8);
                store.catalog.insert(PERMISSION_MANAGE_CLAN.into(), 9);
                store.max_level_by_clan.insert(TEST_CLAN, 8);

                let perms = store.clan_settings_permissions(TEST_CLAN, cx);
                assert!(perms.has_administrator);
                assert!(!perms.has_manage_clan);
                assert!(!perms.is_clan_owner);
            });
        });
    }

    #[gpui::test]
    fn channel_scoped_definitions_keeps_only_channel_scope(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_stores(AuthState::NotAuthenticated, cx);
            store.update(cx, |store, _cx| {
                store.definitions = definitions_from_permission_list(api::PermissionList {
                    max_level_permission: 0,
                    permissions: vec![
                        permission(1, PERMISSION_MANAGE_CLAN, 9, 1),
                        permission(2, PERMISSION_SEND_MESSAGE, 1, PERMISSION_SCOPE_CHANNEL),
                        permission(3, PERMISSION_DELETE_MESSAGE, 2, PERMISSION_SCOPE_CHANNEL),
                        permission(4, PERMISSION_ADMINISTRATOR, 10, 0),
                        permission(5, "", 3, PERMISSION_SCOPE_CHANNEL),
                    ],
                });

                let slugs: Vec<&str> = store
                    .channel_scoped_definitions()
                    .map(|definition| definition.slug.as_str())
                    .collect();
                assert_eq!(
                    slugs,
                    vec![PERMISSION_SEND_MESSAGE, PERMISSION_DELETE_MESSAGE]
                );
                assert_eq!(store.permission_definitions().len(), 4);
            });
        });
    }

    #[test]
    fn definitions_carry_scope_from_dto() {
        let definitions = definitions_from_permission_list(api::PermissionList {
            max_level_permission: 0,
            permissions: vec![permission(
                7,
                PERMISSION_SEND_MESSAGE,
                1,
                PERMISSION_SCOPE_CHANNEL,
            )],
        });
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].scope, PERMISSION_SCOPE_CHANNEL);
        assert_eq!(definitions[0].level, 1);
        assert_eq!(definitions[0].id, 7);
    }
}
