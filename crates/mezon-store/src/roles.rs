use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Rgba, SharedString, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::badge::BadgeService;
use crate::clan::ClanList;
use crate::clan_members::{ClanMember, ClanMembersEvent, ClanMembersStore};
use crate::ids::{ChannelId, ClanId, RoleId, UserId};
use crate::permissions::PermissionStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_CLANS: usize = 32;
const FAILED_FETCH_BACKOFF: Duration = Duration::from_secs(30);

pub const DEFAULT_ROLE_COLOR: &str = "#99aab5";
pub const MAX_ROLE_ICON_BYTES: u64 = 256 * 1024;

const ROLE_EVENT_CREATED: i32 = 1;
const ROLE_EVENT_UPDATED: i32 = 2;
const ROLE_EVENT_DELETED: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    pub color: String,
    pub icon: String,
    pub slug: String,
    pub max_level_permission: i32,
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleUser {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolePermission {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClanRoleDetail {
    pub name: String,
    pub color: String,
    pub icon: String,
    pub slug: String,
    pub active: bool,
    pub order_role: i32,
    pub max_level_permission: i32,
    pub permissions: Vec<RolePermission>,
    pub member_count: usize,
    pub member_ids: Vec<UserId>,
    pub channel_ids: Vec<ChannelId>,
    pub role_channel_active: bool,
    pub creator_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoleStyle {
    pub color: Option<Rgba>,
    pub icon: Option<SharedString>,
}

impl From<&ClanRoleDetail> for Role {
    fn from(detail: &ClanRoleDetail) -> Self {
        Self {
            name: detail.name.clone(),
            color: detail.color.clone(),
            icon: detail.icon.clone(),
            slug: detail.slug.clone(),
            max_level_permission: detail.max_level_permission,
            order: detail.order_role,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RolesEvent {
    Changed { clan_id: ClanId },
    Created { clan_id: ClanId, role_id: RoleId },
    RoleSaved { clan_id: ClanId, role_id: RoleId },
    RoleSaveFailed { clan_id: ClanId, role_id: RoleId },
    RoleOrderSaveFailed { clan_id: ClanId },
}

#[derive(Debug, Default, PartialEq)]
struct ClanRoles {
    order: Vec<RoleId>,
    by_id: HashMap<RoleId, ClanRoleDetail>,
}

pub struct RoleDraft {
    pub title: String,
    pub color: String,
    pub icon: String,
    pub add_permission_ids: Vec<i64>,
    pub remove_permission_ids: Vec<i64>,
    pub add_user_ids: Vec<i64>,
    pub remove_user_ids: Vec<i64>,
}

pub struct RolesStore {
    cache: KeyedCache<ClanId, ClanRoles>,
    role_styles: HashMap<ClanId, HashMap<UserId, RoleStyle>>,
    role_users: HashMap<RoleId, Vec<RoleUser>>,
    role_users_loading: HashSet<RoleId>,
    role_users_failed_at: HashMap<RoleId, Instant>,
    loading: HashSet<ClanId>,
    saving: HashSet<ClanId>,
    saving_members: HashSet<(ClanId, RoleId)>,
    saving_order: HashSet<ClanId>,
    reset_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
    _members_watch: Option<gpui::Subscription>,
    _channel_list_watch: Option<gpui::Subscription>,
}

struct GlobalRolesStore(Entity<RolesStore>);
impl Global for GlobalRolesStore {}

impl EventEmitter<RolesEvent> for RolesStore {}

impl RolesStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalRolesStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalRolesStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalRolesStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.cache.clear();
        self.role_styles.clear();
        self.role_users.clear();
        self.role_users_loading.clear();
        self.role_users_failed_at.clear();
        self.loading.clear();
        self.saving.clear();
        self.saving_members.clear();
        self.saving_order.clear();
        cx.notify();
    }

    pub fn is_saving(&self, clan_id: ClanId) -> bool {
        self.saving.contains(&clan_id)
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::RoleEvent, &entity, |this, event, cx| {
                this.handle_role_event(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| {
                this.cache.mark_all_stale();
                this.refresh_active_from_event(cx);
            });
        });
    }

    fn watch_clan_members(cx: &mut Context<Self>) -> Option<gpui::Subscription> {
        let members = ClanMembersStore::try_global(cx)?;
        Some(
            cx.subscribe(&members, |this, _, event: &ClanMembersEvent, cx| {
                let ClanMembersEvent::Changed { clan_id } = event else {
                    return;
                };
                if this.rebuild_role_styles(*clan_id, cx) {
                    cx.emit(RolesEvent::Changed { clan_id: *clan_id });
                    cx.notify();
                }
            }),
        )
    }

    fn watch_channel_list(cx: &mut Context<Self>) -> Option<gpui::Subscription> {
        let channels = crate::ChannelList::try_global(cx)?;
        Some(
            cx.subscribe(&channels, |this, _, event: &crate::ChannelEvent, cx| {
                if let crate::ChannelEvent::PrivacyChanged {
                    clan_id,
                    channel_id,
                    role_ids,
                    ..
                } = event
                {
                    this.set_channel_roles(*clan_id, *channel_id, role_ids, cx);
                }
            }),
        )
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let members_watch = Self::watch_clan_members(cx);
        let channel_list_watch = Self::watch_channel_list(cx);

        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CLANS)),
            role_styles: HashMap::new(),
            role_users: HashMap::new(),
            role_users_loading: HashSet::new(),
            role_users_failed_at: HashMap::new(),
            loading: HashSet::new(),
            saving: HashSet::new(),
            saving_members: HashSet::new(),
            saving_order: HashSet::new(),
            reset_generation: 0,
            api,
            _conn_watch: conn_watch,
            _members_watch: members_watch,
            _channel_list_watch: channel_list_watch,
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

    fn refresh_active(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if ClanList::global(cx).read(cx).active_clan_id == Some(clan_id) {
            self.fetch(clan_id, true, cx).detach();
        }
    }

    fn refresh_active_from_event(&mut self, cx: &mut Context<Self>) {
        if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
            self.fetch(clan_id, true, cx).detach();
        }
    }

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
        self.role_users.clear();
    }

    pub fn role_style(&self, clan_id: ClanId, user_id: UserId) -> Option<&RoleStyle> {
        self.role_styles.get(&clan_id)?.get(&user_id)
    }

    fn compute_role_styles(&self, clan_id: ClanId, cx: &App) -> Option<HashMap<UserId, RoleStyle>> {
        let roles = self.cache.get(&clan_id)?;
        let members_store = ClanMembersStore::try_global(cx)?;
        Some(role_styles_for_clan(
            roles,
            &members_store.read(cx).members(clan_id),
        ))
    }

    fn rebuild_role_styles(&mut self, clan_id: ClanId, cx: &App) -> bool {
        let changed = match self.compute_role_styles(clan_id, cx) {
            Some(styles) if !styles.is_empty() => {
                if self.role_styles.get(&clan_id) == Some(&styles) {
                    false
                } else {
                    self.role_styles.insert(clan_id, styles);
                    true
                }
            }
            _ => self.role_styles.remove(&clan_id).is_some(),
        };
        self.prune_role_styles();
        changed
    }

    fn prune_role_styles(&mut self) {
        let Self {
            cache, role_styles, ..
        } = self;
        role_styles.retain(|clan_id, _| cache.contains(clan_id));
    }

    pub fn roles_for_channel(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
    ) -> Vec<(RoleId, &ClanRoleDetail)> {
        let Some(roles) = self.cache.get(&clan_id) else {
            return Vec::new();
        };
        roles
            .order
            .iter()
            .filter_map(|id| roles.by_id.get(id).map(|role| (*id, role)))
            .filter(|(_, role)| role.channel_ids.contains(&channel_id))
            .collect()
    }

    pub fn add_roles_to_channel(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        role_ids: &[RoleId],
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(roles) = self.cache.get_mut(&clan_id) else {
            return false;
        };
        let mut changed = false;
        for role_id in role_ids {
            let Some(role) = roles.by_id.get_mut(role_id) else {
                continue;
            };
            if !role.channel_ids.contains(&channel_id) {
                role.channel_ids.push(channel_id);
                changed = true;
            }
            if !role.role_channel_active {
                role.role_channel_active = true;
                changed = true;
            }
        }
        if changed {
            cx.emit(RolesEvent::Changed { clan_id });
            cx.notify();
        }
        changed
    }

    #[cfg(test)]
    pub(crate) fn seed_roles_for_test(&mut self, clan_id: ClanId, roles: Vec<api::Role>) {
        self.cache
            .insert(clan_id, roles_map_from_proto(roles), None);
    }

    pub fn set_channel_roles(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        role_ids: &[RoleId],
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(roles) = self.cache.get_mut(&clan_id) else {
            return false;
        };
        let mut changed = false;
        for (role_id, role) in roles.by_id.iter_mut() {
            let granted = role_ids.contains(role_id);
            let linked = role.channel_ids.contains(&channel_id);
            if granted && !linked {
                role.channel_ids.push(channel_id);
                changed = true;
            } else if !granted && linked {
                role.channel_ids.retain(|id| *id != channel_id);
                changed = true;
            }
            if granted && !role.role_channel_active {
                role.role_channel_active = true;
                changed = true;
            }
        }
        if changed {
            cx.emit(RolesEvent::Changed { clan_id });
            cx.notify();
        }
        changed
    }

    pub fn remove_role_from_channel(
        &mut self,
        clan_id: ClanId,
        role_id: RoleId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(role) = self
            .cache
            .get_mut(&clan_id)
            .and_then(|roles| roles.by_id.get_mut(&role_id))
        else {
            return false;
        };
        let before = role.channel_ids.len();
        role.channel_ids.retain(|id| *id != channel_id);
        let changed = role.channel_ids.len() != before;
        if changed {
            cx.emit(RolesEvent::Changed { clan_id });
            cx.notify();
        }
        changed
    }

    pub fn role_users(&self, role_id: RoleId) -> &[RoleUser] {
        self.role_users
            .get(&role_id)
            .map(|users| users.as_slice())
            .unwrap_or(&[])
    }

    pub fn role_member_ids(&self, clan_id: ClanId, role_id: RoleId) -> Vec<UserId> {
        if let Some(users) = self.role_users.get(&role_id) {
            return users.iter().map(|user| user.id).collect();
        }
        self.cache
            .get(&clan_id)
            .and_then(|roles| roles.by_id.get(&role_id))
            .map(|role| role.member_ids.clone())
            .unwrap_or_default()
    }

    pub fn ensure_role_users_loaded(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        if self.role_users.contains_key(&role_id) || self.role_users_loading.contains(&role_id) {
            return;
        }
        if self
            .role_users_failed_at
            .get(&role_id)
            .is_some_and(|at| at.elapsed() < FAILED_FETCH_BACKOFF)
        {
            return;
        }
        self.start_role_users_fetch(role_id, cx);
    }

    fn reload_role_users(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.role_users_loading.remove(&role_id);
        self.role_users_failed_at.remove(&role_id);
        self.start_role_users_fetch(role_id, cx);
    }

    fn start_role_users_fetch(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        if !self.role_users_loading.insert(role_id) {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let mut users = Vec::new();
            let mut cursor = String::new();
            let mut complete = false;
            loop {
                let result = api.list_role_users(role_id.get(), 100, &cursor).await;
                let Ok(page) = result else {
                    break;
                };
                users.extend(page.role_users.into_iter().map(role_user_from_proto));
                if page.cursor.is_empty() {
                    complete = true;
                    break;
                }
                cursor = page.cursor;
            }
            let _ = this.update(cx, |this, cx| {
                this.role_users_loading.remove(&role_id);
                if this.reset_generation != generation {
                    return;
                }
                if !complete {
                    this.role_users_failed_at.insert(role_id, Instant::now());
                    return;
                }
                this.role_users_failed_at.remove(&role_id);
                this.role_users.insert(role_id, users);
                if let Some(clan_id) = this.clan_id_for_role(role_id) {
                    this.sync_role_member_count(role_id, clan_id);
                    cx.emit(RolesEvent::Changed { clan_id });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_role_member_patch(
        &mut self,
        role_id: RoleId,
        clan_id: ClanId,
        add_user_ids: &[i64],
        remove_user_ids: &[i64],
        cx: &App,
    ) {
        let remove_set: HashSet<UserId> = remove_user_ids.iter().copied().map(UserId).collect();
        let users = self.role_users.entry(role_id).or_default();
        users.retain(|user| !remove_set.contains(&user.id));

        if add_user_ids.is_empty() {
            return;
        }

        let Some(members_store) = ClanMembersStore::try_global(cx) else {
            for user_id in add_user_ids {
                let user_id = UserId(*user_id);
                if users.iter().any(|user| user.id == user_id) {
                    continue;
                }
                users.push(RoleUser {
                    id: user_id,
                    username: String::new(),
                    display_name: String::new(),
                    avatar_url: String::new(),
                });
            }
            return;
        };
        let members = members_store.read(cx);
        for user_id in add_user_ids {
            let user_id = UserId(*user_id);
            if users.iter().any(|user| user.id == user_id) {
                continue;
            }
            if let Some(member) = members.member(clan_id, user_id) {
                users.push(role_user_from_clan_member(member));
            } else {
                users.push(RoleUser {
                    id: user_id,
                    username: String::new(),
                    display_name: String::new(),
                    avatar_url: String::new(),
                });
            }
        }
    }

    fn sync_role_member_count(&mut self, role_id: RoleId, clan_id: ClanId) {
        let Some(count) = self.role_users.get(&role_id).map(|users| users.len()) else {
            return;
        };
        if let Some(role) = self
            .cache
            .get_mut(&clan_id)
            .and_then(|roles| roles.by_id.get_mut(&role_id))
        {
            role.member_count = count;
        }
    }

    fn clan_id_for_role(&self, role_id: RoleId) -> Option<ClanId> {
        self.cache
            .iter()
            .find_map(|(clan_id, roles)| roles.by_id.contains_key(&role_id).then_some(*clan_id))
    }

    pub fn mutate_role_members(
        &mut self,
        clan_id: ClanId,
        role_id: RoleId,
        add_user_ids: Vec<i64>,
        remove_user_ids: Vec<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if add_user_ids.is_empty() && remove_user_ids.is_empty() {
            return false;
        }
        let Some(role) = self
            .cache
            .get(&clan_id)
            .and_then(|roles| roles.by_id.get(&role_id).cloned())
        else {
            return false;
        };
        if !self.saving_members.insert((clan_id, role_id)) {
            return false;
        }
        let max_permission_id = self.resolve_max_permission_id(clan_id, cx);
        let api = self.api.clone();
        let added = add_user_ids.clone();
        let removed = remove_user_ids.clone();
        cx.spawn(async move |this, cx| {
            let request = api::UpdateRoleRequest {
                role_id: role_id.get(),
                title: Some(role.name),
                color: Some(role.color),
                role_icon: Some(role.icon),
                clan_id: clan_id.get(),
                max_permission_id,
                add_user_ids,
                remove_user_ids,
                ..Default::default()
            };
            let result = api.update_role(request).await;
            let _ = this.update(cx, |this, cx| {
                this.saving_members.remove(&(clan_id, role_id));
                match result {
                    Ok(()) => {
                        this.apply_role_member_patch(role_id, clan_id, &added, &removed, cx);
                        this.sync_role_member_count(role_id, clan_id);
                        if let Some(members) = ClanMembersStore::try_global(cx) {
                            members.update(cx, |members, cx| {
                                members
                                    .apply_role_membership(clan_id, role_id, &added, &removed, cx);
                            });
                        }
                        this.reload_role_users(role_id, cx);
                        cx.emit(RolesEvent::Changed { clan_id });
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!("mutate_role_members failed for role {role_id}: {err}");
                        cx.emit(RolesEvent::RoleSaveFailed { clan_id, role_id });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        true
    }

    pub fn ensure_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.ensure_loaded_task(clan_id, cx).detach();
    }

    pub fn ensure_loaded_task(&mut self, clan_id: ClanId, cx: &mut Context<Self>) -> Task<()> {
        self.cache.touch(&clan_id);
        if self.cache.is_fresh(&clan_id, crate::EVENT_BACKED_CACHE_TTL) {
            return Task::ready(());
        }
        self.fetch(clan_id, false, cx)
    }

    pub fn reload(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetch(clan_id, true, cx).detach();
    }

    fn fetch(&mut self, clan_id: ClanId, force: bool, cx: &mut Context<Self>) -> Task<()> {
        if !force && !self.loading.insert(clan_id) {
            return Task::ready(());
        }
        if force {
            self.loading.insert(clan_id);
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_roles(clan_id.get(), 1000, "").await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&clan_id);
                match result {
                    Ok(resp) => {
                        let proto_roles = resp.roles.map(|rl| rl.roles).unwrap_or_default();
                        let roles = roles_map_from_proto(proto_roles);
                        tracing::info!(
                            "RolesStore: fetched {} roles for clan {clan_id}",
                            roles.order.len()
                        );
                        let empty = roles.order.is_empty();
                        // A reload usually returns what we already hold. Announcing it
                        // anyway would rebuild the member list for nothing.
                        let unchanged = this.cache.get(&clan_id) == Some(&roles);
                        this.cache.insert(clan_id, roles, None);
                        if empty {
                            this.cache.mark_stale(&clan_id);
                        }
                        if unchanged {
                            return;
                        }
                        this.rebuild_role_styles(clan_id, cx);
                        cx.emit(RolesEvent::Changed { clan_id });
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_roles failed for {clan_id}: {e}"),
                }
            });
        })
    }

    pub fn roles_for(&self, clan_id: ClanId, role_ids: &[RoleId]) -> Vec<Role> {
        let Some(roles) = self.cache.get(&clan_id) else {
            return Vec::new();
        };
        role_ids
            .iter()
            .filter_map(|id| roles.by_id.get(id))
            .map(Role::from)
            .collect()
    }

    pub fn role(&self, clan_id: ClanId, role_id: RoleId) -> Option<Role> {
        self.cache
            .get(&clan_id)?
            .by_id
            .get(&role_id)
            .map(Role::from)
    }

    pub fn clan_role(&self, clan_id: ClanId, role_id: RoleId) -> Option<&ClanRoleDetail> {
        self.cache.get(&clan_id)?.by_id.get(&role_id)
    }

    pub fn roles_in_clan(&self, clan_id: ClanId) -> Vec<(RoleId, Role)> {
        let Some(roles) = self.cache.get(&clan_id) else {
            return Vec::new();
        };
        roles
            .order
            .iter()
            .filter_map(|id| roles.by_id.get(id).map(|role| (*id, Role::from(role))))
            .collect()
    }

    pub fn active_roles_in_clan(&self, clan_id: ClanId) -> Vec<(RoleId, &ClanRoleDetail)> {
        let Some(roles) = self.cache.get(&clan_id) else {
            return Vec::new();
        };
        roles
            .order
            .iter()
            .filter_map(|id| roles.by_id.get(id).map(|role| (*id, role)))
            .collect()
    }

    pub fn update_role_order(
        &mut self,
        clan_id: ClanId,
        ordered_ids: Vec<RoleId>,
        cx: &mut Context<Self>,
    ) {
        let Some(roles) = self.cache.get_mut(&clan_id) else {
            return;
        };
        if roles.order == ordered_ids {
            return;
        }
        let previous_order = std::mem::replace(&mut roles.order, ordered_ids.clone());
        apply_order_ranks(roles);
        self.saving_order.insert(clan_id);
        cx.emit(RolesEvent::Changed { clan_id });
        cx.notify();

        let request = ordered_ids
            .iter()
            .enumerate()
            .map(|(index, role_id)| (index as i32 + 1, role_id.get()))
            .collect::<Vec<_>>();
        let generation = self.reset_generation;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.update_role_order(clan_id.get(), &request).await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.saving_order.remove(&clan_id);
                if let Err(error) = result {
                    tracing::error!("update_role_order failed for {clan_id}: {error}");
                    if let Some(roles) = this.cache.get_mut(&clan_id) {
                        roles.order = reconcile_order(&previous_order, &roles.order);
                        apply_order_ranks(roles);
                    }
                    cx.emit(RolesEvent::RoleOrderSaveFailed { clan_id });
                }
                cx.emit(RolesEvent::Changed { clan_id });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn is_saving_order(&self, clan_id: ClanId) -> bool {
        self.saving_order.contains(&clan_id)
    }

    pub fn everyone_role_id(&self, clan_id: ClanId) -> Option<RoleId> {
        let expected = everyone_slug(clan_id);
        let roles = self.cache.get(&clan_id)?;
        roles.order.iter().copied().find(|id| {
            roles
                .by_id
                .get(id)
                .is_some_and(|role| role.slug == expected)
        })
    }

    pub fn is_everyone_role(&self, clan_id: ClanId, role: &ClanRoleDetail) -> bool {
        role.slug == everyone_slug(clan_id)
    }

    pub fn create_role(
        &mut self,
        clan_id: ClanId,
        draft: RoleDraft,
        max_permission_id: i64,
        cx: &mut Context<Self>,
    ) {
        if !self.saving.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = api::CreateRoleRequest {
                title: draft.title,
                color: draft.color,
                role_icon: draft.icon,
                clan_id: clan_id.get(),
                max_permission_id,
                active_permission_ids: draft.add_permission_ids,
                add_user_ids: draft.add_user_ids,
                ..Default::default()
            };
            let result = api.create_role(request).await;
            let _ = this.update(cx, |this, cx| {
                this.saving.remove(&clan_id);
                match result {
                    Ok(role_proto) => {
                        let role_id = RoleId(role_proto.id);
                        this.upsert_role_from_proto(clan_id, role_proto);
                        this.rebuild_role_styles(clan_id, cx);
                        cx.emit(RolesEvent::Changed { clan_id });
                        cx.emit(RolesEvent::Created { clan_id, role_id });
                        this.refresh_active(clan_id, cx);
                    }
                    Err(err) => tracing::error!("create_role failed for {clan_id}: {err}"),
                }
            });
        })
        .detach();
    }

    pub fn update_role(
        &mut self,
        clan_id: ClanId,
        role_id: RoleId,
        draft: RoleDraft,
        max_permission_id: i64,
        cx: &mut Context<Self>,
    ) {
        if !self.saving.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = api::UpdateRoleRequest {
                role_id: role_id.get(),
                title: Some(draft.title),
                color: Some(draft.color),
                role_icon: Some(draft.icon),
                clan_id: clan_id.get(),
                max_permission_id,
                active_permission_ids: draft.add_permission_ids,
                remove_permission_ids: draft.remove_permission_ids,
                add_user_ids: draft.add_user_ids,
                remove_user_ids: draft.remove_user_ids,
                ..Default::default()
            };
            let result = api.update_role(request).await;
            let _ = this.update(cx, |this, cx| {
                this.saving.remove(&clan_id);
                match result {
                    Ok(()) => {
                        this.refresh_active(clan_id, cx);
                        cx.emit(RolesEvent::RoleSaved { clan_id, role_id });
                    }
                    Err(err) => {
                        tracing::error!("update_role failed for role {role_id}: {err}");
                        cx.emit(RolesEvent::RoleSaveFailed { clan_id, role_id });
                    }
                }
            });
        })
        .detach();
    }

    pub fn delete_role(&mut self, clan_id: ClanId, role_id: RoleId, cx: &mut Context<Self>) {
        if !self.saving.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.delete_role(role_id.get(), clan_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                this.saving.remove(&clan_id);
                match result {
                    Ok(()) => {
                        this.refresh_active(clan_id, cx);
                    }
                    Err(err) => {
                        tracing::error!("delete_role failed for role {role_id}: {err}")
                    }
                }
            });
        })
        .detach();
    }

    fn upsert_role_from_proto(&mut self, clan_id: ClanId, r: api::Role) -> bool {
        if r.id == 0 || r.active != 1 {
            return false;
        }
        let id = RoleId(r.id);
        let previous_members = self
            .cache
            .get(&clan_id)
            .and_then(|roles| roles.by_id.get(&id))
            .map(|role| (role.member_count, role.member_ids.clone()));
        let has_role_users = r.role_user_list.is_some();
        let mut detail = role_detail_from_proto(r);
        if !has_role_users && let Some((count, member_ids)) = previous_members {
            detail.member_count = count;
            detail.member_ids = member_ids;
        }
        let roles = self.cache.get_mut(&clan_id);
        if let Some(roles) = roles {
            if roles.by_id.insert(id, detail).is_none() {
                roles.order.push(id);
            }
        } else {
            let mut clan_roles = ClanRoles::default();
            clan_roles.by_id.insert(id, detail);
            clan_roles.order.push(id);
            self.cache.insert(clan_id, clan_roles, None);
        }
        true
    }

    fn merge_role_from_proto(&mut self, clan_id: ClanId, r: &api::Role) -> bool {
        let Some(role) = self
            .cache
            .get_mut(&clan_id)
            .and_then(|roles| roles.by_id.get_mut(&RoleId(r.id)))
        else {
            return false;
        };
        let permissions: Option<Vec<RolePermission>> = r
            .permission_list
            .as_ref()
            .filter(|list| !list.permissions.is_empty())
            .map(|list| list.permissions.iter().map(permission_from_proto).collect());
        let mut changed = false;
        if role.name != r.title {
            role.name = r.title.clone();
            changed = true;
        }
        if role.color != r.color {
            role.color = r.color.clone();
            changed = true;
        }
        if role.icon != r.role_icon {
            role.icon = r.role_icon.clone();
            changed = true;
        }
        if r.max_level_permission != 0 && role.max_level_permission != r.max_level_permission {
            role.max_level_permission = r.max_level_permission;
            changed = true;
        }
        if let Some(permissions) = permissions
            && role.permissions != permissions
        {
            role.permissions = permissions;
            changed = true;
        }
        changed
    }

    fn remove_role(&mut self, clan_id: ClanId, role_id: RoleId) -> bool {
        let Some(roles) = self.cache.get_mut(&clan_id) else {
            return false;
        };
        if roles.by_id.remove(&role_id).is_none() {
            return false;
        }
        roles.order.retain(|id| *id != role_id);
        self.role_users.remove(&role_id);
        self.role_users_loading.remove(&role_id);
        true
    }

    fn handle_role_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(role_event)) = event
        else {
            return;
        };
        let Some(role) = role_event.role.as_ref() else {
            return;
        };
        if role.id == 0 || role.clan_id == 0 {
            return;
        }
        let self_id = BadgeService::try_global(cx)
            .and_then(|badge| badge.read(cx).current_user_id(cx))
            .map(|id| id.get());
        if self_id.is_some_and(|id| id == role_event.user_id) {
            return;
        }
        let clan_id = ClanId(role.clan_id);
        let role_id = RoleId(role.id);

        self.apply_member_role_ids(clan_id, role_id, role_event, cx);

        let membership_changed_for_self = self_id.is_some_and(|id| {
            role_event.user_add_ids.contains(&id) || role_event.user_remove_ids.contains(&id)
        });
        let role_permissions_changed = !role_event.active_permission_ids.is_empty()
            || !role_event.remove_permission_ids.is_empty();
        let permissions_changed_on_my_role =
            role_permissions_changed && self_holds_role(clan_id, role_id, cx);
        let affects_self = membership_changed_for_self || permissions_changed_on_my_role;
        if affects_self
            && matches!(role_event.status, ROLE_EVENT_CREATED | ROLE_EVENT_UPDATED)
            && let Some(permissions) = PermissionStore::try_global(cx)
        {
            permissions.update(cx, |permissions, cx| {
                permissions.reload_clan_permissions(clan_id, cx);
            });
        }

        if !self.cache.contains(&clan_id) {
            return;
        }

        let mut changed = if role_event.status == ROLE_EVENT_CREATED {
            self.upsert_role_from_proto(clan_id, role.clone())
        } else {
            let mut changed = self.apply_role_user_list(clan_id, role_id, role_event, cx);
            changed |= match role_event.status {
                ROLE_EVENT_UPDATED => self.merge_role_from_proto(clan_id, role),
                ROLE_EVENT_DELETED => self.remove_role(clan_id, role_id),
                _ => false,
            };
            changed
        };

        changed |= self.rebuild_role_styles(clan_id, cx);

        if changed {
            cx.emit(RolesEvent::Changed { clan_id });
            cx.notify();
        }
    }

    fn apply_member_role_ids(
        &mut self,
        clan_id: ClanId,
        role_id: RoleId,
        role_event: &realtime::RoleEvent,
        cx: &mut Context<Self>,
    ) {
        if role_event.user_add_ids.is_empty() && role_event.user_remove_ids.is_empty() {
            return;
        }
        let Some(members) = ClanMembersStore::try_global(cx) else {
            return;
        };
        members.update(cx, |members, cx| {
            members.apply_role_membership(
                clan_id,
                role_id,
                &role_event.user_add_ids,
                &role_event.user_remove_ids,
                cx,
            );
        });
    }

    fn apply_role_user_list(
        &mut self,
        clan_id: ClanId,
        role_id: RoleId,
        role_event: &realtime::RoleEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if role_event.user_add_ids.is_empty() && role_event.user_remove_ids.is_empty() {
            return false;
        }
        if self.role_users.contains_key(&role_id) {
            self.apply_role_member_patch(
                role_id,
                clan_id,
                &role_event.user_add_ids,
                &role_event.user_remove_ids,
                cx,
            );
            self.sync_role_member_count(role_id, clan_id);
            return true;
        }
        false
    }

    pub fn resolve_max_permission_id(&self, clan_id: ClanId, cx: &App) -> i64 {
        let Some(role_ids) = ClanMembersStore::try_global(cx).and_then(|store| {
            store
                .read(cx)
                .self_role_ids(clan_id)
                .map(|ids| ids.to_vec())
        }) else {
            return 0;
        };
        let Some(roles) = self.cache.get(&clan_id) else {
            return role_ids.first().copied().unwrap_or(0);
        };
        role_ids
            .iter()
            .filter_map(|id| {
                roles
                    .by_id
                    .get(&RoleId(*id))
                    .map(|role| (role.max_level_permission, *id))
            })
            .max_by_key(|(level, _)| *level)
            .map(|(_, id)| id)
            .unwrap_or(0)
    }
}

pub fn everyone_slug(clan_id: ClanId) -> String {
    format!("everyone-{}", clan_id.get())
}

fn role_user_from_proto(user: api::role_user_list::RoleUser) -> RoleUser {
    RoleUser {
        id: UserId(user.id),
        username: user.username,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
    }
}

fn role_user_from_clan_member(member: &crate::clan_members::ClanMember) -> RoleUser {
    RoleUser {
        id: member.id(),
        username: member.user.username.clone(),
        display_name: member.name().to_string(),
        avatar_url: member.avatar().to_string(),
    }
}

fn permission_from_proto(p: &api::Permission) -> RolePermission {
    RolePermission {
        id: p.id,
        slug: p.slug.clone(),
        title: p.title.clone(),
        active: p.active == 1,
    }
}

fn self_holds_role(clan_id: ClanId, role_id: RoleId, cx: &App) -> bool {
    ClanMembersStore::try_global(cx)
        .and_then(|members| {
            members
                .read(cx)
                .self_role_ids(clan_id)
                .map(|ids| ids.contains(&role_id.get()))
        })
        .unwrap_or(false)
}

fn reconcile_order(previous: &[RoleId], current: &[RoleId]) -> Vec<RoleId> {
    let live: HashSet<RoleId> = current.iter().copied().collect();
    let mut ordered: Vec<RoleId> = previous
        .iter()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    let restored: HashSet<RoleId> = ordered.iter().copied().collect();
    ordered.extend(current.iter().copied().filter(|id| !restored.contains(id)));
    ordered
}

fn apply_order_ranks(roles: &mut ClanRoles) {
    for (index, role_id) in roles.order.iter().enumerate() {
        if let Some(role) = roles.by_id.get_mut(role_id) {
            role.order_role = index as i32 + 1;
        }
    }
}

fn role_detail_from_proto(r: api::Role) -> ClanRoleDetail {
    let permissions = r
        .permission_list
        .map(|list| {
            list.permissions
                .into_iter()
                .map(|p| RolePermission {
                    id: p.id,
                    slug: p.slug,
                    title: p.title,
                    active: p.active == 1,
                })
                .collect()
        })
        .unwrap_or_default();
    let member_ids: Vec<UserId> = r
        .role_user_list
        .map(|list| list.role_users.iter().map(|user| UserId(user.id)).collect())
        .unwrap_or_default();
    let member_count = member_ids.len();
    ClanRoleDetail {
        name: r.title,
        color: r.color,
        icon: r.role_icon,
        slug: r.slug,
        active: r.active == 1,
        order_role: r.order_role,
        max_level_permission: r.max_level_permission,
        permissions,
        member_count,
        member_ids,
        channel_ids: r.channel_ids.into_iter().map(ChannelId).collect(),
        role_channel_active: r.role_channel_active == 1,
        creator_id: UserId(r.creator_id),
    }
}

pub fn parse_role_color(raw: &str) -> Option<Rgba> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !hex.is_ascii() {
        return None;
    }
    let (r, g, b) = match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            (r * 17, g * 17, b * 17)
        }
        6 | 8 => {
            let value = u32::from_str_radix(&hex[0..6], 16).ok()?;
            (
                ((value >> 16) & 0xff) as u8,
                ((value >> 8) & 0xff) as u8,
                (value & 0xff) as u8,
            )
        }
        _ => return None,
    };
    Some(Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: 1.,
    })
}

fn role_styles_for_clan(roles: &ClanRoles, members: &[&ClanMember]) -> HashMap<UserId, RoleStyle> {
    let mut rank: HashMap<RoleId, usize> = HashMap::with_capacity(roles.order.len());
    for (index, role_id) in roles.order.iter().enumerate() {
        rank.insert(*role_id, index);
    }
    let mut styles: HashMap<UserId, RoleStyle> = HashMap::new();
    let mut ranked: Vec<(usize, &ClanRoleDetail)> = Vec::new();
    for member in members {
        ranked.clear();
        ranked.extend(member.role_ids.iter().filter_map(|role_id| {
            let index = *rank.get(role_id)?;
            let role = roles.by_id.get(role_id)?;
            Some((index, role))
        }));
        if ranked.is_empty() {
            continue;
        }
        ranked.sort_unstable_by_key(|(index, _)| *index);
        let winning_color = ranked
            .first()
            .map(|(_, role)| role.color.trim())
            .filter(|color| !color.is_empty())
            .unwrap_or(DEFAULT_ROLE_COLOR);
        let icon = ranked.iter().find_map(|(_, role)| {
            let icon = role.icon.trim();
            (!icon.is_empty()).then(|| SharedString::from(icon.to_string()))
        });
        styles.insert(
            member.id(),
            RoleStyle {
                color: parse_role_color(winning_color),
                icon,
            },
        );
    }
    styles
}

fn roles_map_from_proto(roles: Vec<api::Role>) -> ClanRoles {
    let mut indexed: Vec<(usize, api::Role)> = roles.into_iter().enumerate().collect();
    indexed.sort_by(|(i1, r1), (i2, r2)| {
        let o1 = r1.order_role;
        let o2 = r2.order_role;
        if o1 > 0 && o2 > 0 {
            return o1.cmp(&o2);
        }
        if o1 > 0 {
            return std::cmp::Ordering::Less;
        }
        if o2 > 0 {
            return std::cmp::Ordering::Greater;
        }
        i1.cmp(i2)
    });

    let mut out = ClanRoles::default();
    for (_, r) in indexed {
        if r.id == 0 || r.active != 1 {
            continue;
        }
        let id = RoleId(r.id);
        if out.by_id.insert(id, role_detail_from_proto(r)).is_none() {
            out.order.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthState;
    use crate::clan_members::User;
    use gpui::TestAppContext;
    use mezon_client::{Session, TransportClient};
    use mezon_proto::api;

    const TEST_CLAN: ClanId = ClanId(1);
    const TEST_SELF: i64 = 42;
    const TEST_ACTOR: i64 = 77;

    fn test_api() -> Arc<AppApi> {
        Arc::new(AppApi::new(
            Arc::new(TransportClient::new(String::new())),
            String::new(),
        ))
    }

    fn init_roles_store(cx: &mut App) -> Entity<RolesStore> {
        let api = test_api();
        RealtimeDispatch::init(api.clone(), cx);
        ClanList::init(api.clone(), cx);
        let auth_state = cx.new(|_| {
            AuthState::Authenticated(Session {
                user_id: TEST_SELF.to_string(),
                ..Default::default()
            })
        });
        BadgeService::init(auth_state, cx);
        ClanMembersStore::init(api.clone(), cx);
        RolesStore::init(api, cx)
    }

    fn member_with_roles(user_id: i64, role_ids: &[i64]) -> ClanMember {
        ClanMember {
            user: User {
                id: UserId(user_id),
                ..Default::default()
            },
            role_ids: role_ids.iter().copied().map(RoleId).collect(),
            ..Default::default()
        }
    }

    fn styled_role(id: i64, color: &str, icon: &str) -> api::Role {
        api::Role {
            id,
            title: format!("role-{id}"),
            color: color.into(),
            role_icon: icon.into(),
            active: 1,
            ..Default::default()
        }
    }

    fn role_event(status: i32, role: api::Role, actor: i64) -> RealtimeEvent {
        RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(
            realtime::RoleEvent {
                role: Some(role),
                status,
                user_id: actor,
                ..Default::default()
            },
        ))
    }

    fn roles_for_in(
        by_clan: &HashMap<ClanId, ClanRoles>,
        clan_id: ClanId,
        role_ids: &[RoleId],
    ) -> Vec<Role> {
        let Some(roles) = by_clan.get(&clan_id) else {
            return Vec::new();
        };
        role_ids
            .iter()
            .filter_map(|id| roles.by_id.get(id))
            .map(Role::from)
            .collect()
    }

    fn make_role(id: i64, title: &str, color: &str) -> api::Role {
        api::Role {
            id,
            title: title.into(),
            color: color.into(),
            active: 1,
            ..Default::default()
        }
    }

    fn make_role_with_order(id: i64, title: &str, color: &str, order_role: i32) -> api::Role {
        api::Role {
            order_role,
            ..make_role(id, title, color)
        }
    }

    #[test]
    fn maps_proto_roles_to_domain() {
        let roles = roles_map_from_proto(vec![
            make_role(1, "Admin", "#ff0000"),
            make_role(2, "Member", "#00ff00"),
        ]);
        assert_eq!(roles.by_id.len(), 2);
        assert_eq!(roles.by_id[&RoleId(1)].name, "Admin");
        assert_eq!(roles.by_id[&RoleId(1)].color, "#ff0000");
        assert_eq!(roles.by_id[&RoleId(2)].name, "Member");
    }

    #[test]
    fn sorts_by_order_role() {
        let mut high = make_role(9, "Zulu", "");
        high.order_role = 3;
        let mut mid = make_role(3, "Alpha", "");
        mid.order_role = 1;
        let mut low = make_role(7, "Mike", "");
        low.order_role = 2;
        let roles = roles_map_from_proto(vec![high, mid, low]);
        assert_eq!(roles.order, vec![RoleId(3), RoleId(7), RoleId(9)]);
    }

    #[test]
    fn sorts_roles_by_order_role_ascending() {
        let roles = roles_map_from_proto(vec![
            make_role_with_order(9, "Zulu", "", 5),
            make_role_with_order(3, "Alpha", "", 1),
            make_role_with_order(7, "Mike", "", 3),
        ]);
        assert_eq!(roles.order, vec![RoleId(3), RoleId(7), RoleId(9)]);
    }

    #[test]
    fn skips_role_with_zero_id() {
        let roles =
            roles_map_from_proto(vec![make_role(0, "Bad", ""), make_role(1, "Good", "blue")]);
        assert!(!roles.by_id.contains_key(&RoleId(0)));
        assert!(roles.by_id.contains_key(&RoleId(1)));
        assert_eq!(roles.order, vec![RoleId(1)]);
    }

    #[test]
    fn roles_for_returns_matching_roles() {
        let mut by_clan: HashMap<ClanId, ClanRoles> = HashMap::new();
        by_clan.insert(
            ClanId(1),
            roles_map_from_proto(vec![
                make_role(10, "Admin", "#f00"),
                make_role(20, "Mod", "#0f0"),
            ]),
        );
        let result = roles_for_in(&by_clan, ClanId(1), &[RoleId(10), RoleId(20), RoleId(99)]);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|r| r.name == "Admin"));
        assert!(result.iter().any(|r| r.name == "Mod"));
    }

    #[test]
    fn roles_for_returns_empty_for_unknown_clan() {
        let by_clan: HashMap<ClanId, ClanRoles> = HashMap::new();
        assert!(roles_for_in(&by_clan, ClanId(99), &[RoleId(1)]).is_empty());
    }

    #[test]
    fn keyed_cache_reconnect_marks_stale_without_dropping_values_then_refetches() {
        let mut cache: KeyedCache<ClanId, ClanRoles> = KeyedCache::new(None);
        cache.insert(
            ClanId(1),
            roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
            None,
        );
        assert!(cache.is_fresh(&ClanId(1), crate::CACHE_TTL));

        cache.mark_all_stale();
        assert!(!cache.is_fresh(&ClanId(1), crate::CACHE_TTL));
        assert!(cache.get(&ClanId(1)).is_some());

        cache.insert(
            ClanId(1),
            roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
            None,
        );
        assert!(cache.is_fresh(&ClanId(1), crate::CACHE_TTL));
    }

    #[test]
    fn role_style_takes_color_from_first_role_and_first_non_empty_icon() {
        let roles = roles_map_from_proto(vec![
            styled_role(10, "", ""),
            styled_role(20, "#112233", "icon-b.png"),
            styled_role(30, "#445566", "icon-c.png"),
        ]);
        let member = member_with_roles(5, &[30, 20, 10]);
        let styles = role_styles_for_clan(&roles, &[&member]);

        let style = styles.get(&UserId(5)).expect("style for member");
        assert_eq!(style.color, parse_role_color(DEFAULT_ROLE_COLOR));
        assert_eq!(style.icon.as_deref(), Some("icon-b.png"));
    }

    #[test]
    fn role_style_keeps_first_role_color_even_when_later_role_has_one() {
        let roles = roles_map_from_proto(vec![
            styled_role(10, "#aabbcc", ""),
            styled_role(20, "#112233", "icon-b.png"),
        ]);
        let member = member_with_roles(5, &[10, 20]);
        let styles = role_styles_for_clan(&roles, &[&member]);

        let style = styles.get(&UserId(5)).expect("style for member");
        assert_eq!(style.color, parse_role_color("#aabbcc"));
        assert_eq!(style.icon.as_deref(), Some("icon-b.png"));
    }

    #[test]
    fn role_style_orders_by_role_order_not_by_member_role_ids_order() {
        let roles = roles_map_from_proto(vec![
            make_role_with_order(10, "Low", "#aabbcc", 2),
            make_role_with_order(20, "High", "#112233", 1),
        ]);
        assert_eq!(roles.order, vec![RoleId(20), RoleId(10)]);
        let member = member_with_roles(5, &[10, 20]);
        let styles = role_styles_for_clan(&roles, &[&member]);

        assert_eq!(
            styles.get(&UserId(5)).and_then(|style| style.color),
            parse_role_color("#112233")
        );
    }

    #[test]
    fn role_style_skips_members_without_known_roles() {
        let roles = roles_map_from_proto(vec![styled_role(10, "#aabbcc", "icon.png")]);
        let no_roles = member_with_roles(5, &[]);
        let unknown_role = member_with_roles(6, &[99]);
        let styles = role_styles_for_clan(&roles, &[&no_roles, &unknown_role]);

        assert!(styles.is_empty());
    }

    #[test]
    fn role_style_has_no_icon_when_every_role_icon_is_empty() {
        let roles = roles_map_from_proto(vec![
            styled_role(10, "#aabbcc", ""),
            styled_role(20, "", ""),
        ]);
        let member = member_with_roles(5, &[10, 20]);
        let styles = role_styles_for_clan(&roles, &[&member]);

        assert!(styles[&UserId(5)].icon.is_none());
    }

    #[test]
    fn maps_channel_ids_and_role_channel_active_from_proto() {
        let roles = roles_map_from_proto(vec![api::Role {
            channel_ids: vec![7, 9],
            role_channel_active: 1,
            ..make_role(10, "Admin", "#f00")
        }]);
        let role = &roles.by_id[&RoleId(10)];
        assert_eq!(role.channel_ids, vec![ChannelId(7), ChannelId(9)]);
        assert!(role.role_channel_active);
    }

    #[gpui::test]
    fn set_channel_roles_replaces_the_roles_linked_to_a_channel(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.seed_roles_for_test(
                    TEST_CLAN,
                    vec![
                        api::Role {
                            channel_ids: vec![7, 9],
                            role_channel_active: 1,
                            ..make_role(10, "A", "#f00")
                        },
                        api::Role {
                            channel_ids: vec![7],
                            role_channel_active: 1,
                            ..make_role(20, "B", "#0f0")
                        },
                        make_role(30, "C", "#00f"),
                    ],
                );
                let linked = |store: &RolesStore, channel_id: i64| {
                    store
                        .roles_for_channel(TEST_CLAN, ChannelId(channel_id))
                        .into_iter()
                        .map(|(role_id, _)| role_id)
                        .collect::<Vec<_>>()
                };

                assert!(store.set_channel_roles(TEST_CLAN, ChannelId(7), &[RoleId(30)], cx));
                assert_eq!(linked(store, 7), vec![RoleId(30)]);
                assert_eq!(linked(store, 9), vec![RoleId(10)]);

                assert!(store.set_channel_roles(TEST_CLAN, ChannelId(7), &[], cx));
                assert!(linked(store, 7).is_empty());
                assert!(!store.set_channel_roles(TEST_CLAN, ChannelId(7), &[], cx));
            });
        });
    }

    #[gpui::test]
    fn update_role_order_writes_one_based_ranks_optimistically(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![
                        make_role_with_order(10, "First", "#f00", 1),
                        make_role_with_order(20, "Second", "#0f0", 2),
                        make_role_with_order(30, "Third", "#00f", 3),
                    ]),
                    None,
                );

                store.update_role_order(TEST_CLAN, vec![RoleId(30), RoleId(10), RoleId(20)], cx);

                let ordered = store
                    .active_roles_in_clan(TEST_CLAN)
                    .into_iter()
                    .map(|(id, role)| (id, role.order_role))
                    .collect::<Vec<_>>();
                assert_eq!(
                    ordered,
                    vec![(RoleId(30), 1), (RoleId(10), 2), (RoleId(20), 3)]
                );
            });
        });
    }

    #[test]
    fn reconcile_order_keeps_roles_created_during_an_in_flight_save() {
        let previous = vec![RoleId(1), RoleId(2), RoleId(3)];
        let current = vec![RoleId(3), RoleId(1), RoleId(2), RoleId(9)];

        assert_eq!(
            reconcile_order(&previous, &current),
            vec![RoleId(1), RoleId(2), RoleId(3), RoleId(9)],
            "rollback restores the old relative order but must not drop the new role"
        );
    }

    #[test]
    fn reconcile_order_drops_roles_deleted_during_an_in_flight_save() {
        let previous = vec![RoleId(1), RoleId(2), RoleId(3)];
        let current = vec![RoleId(1), RoleId(3)];
        assert_eq!(
            reconcile_order(&previous, &current),
            vec![RoleId(1), RoleId(3)]
        );
    }

    #[gpui::test]
    fn update_role_order_is_a_noop_when_order_is_unchanged(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![
                        make_role_with_order(10, "First", "#f00", 1),
                        make_role_with_order(20, "Second", "#0f0", 2),
                    ]),
                    None,
                );

                store.update_role_order(TEST_CLAN, vec![RoleId(10), RoleId(20)], cx);

                let ordered = store
                    .active_roles_in_clan(TEST_CLAN)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                assert_eq!(ordered, vec![RoleId(10), RoleId(20)]);
            });
        });
    }

    #[gpui::test]
    fn roles_for_channel_filters_on_channel_ids_only(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, _| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![
                        api::Role {
                            channel_ids: vec![7],
                            role_channel_active: 1,
                            ..make_role(10, "InChannelActive", "#f00")
                        },
                        api::Role {
                            channel_ids: vec![7],
                            role_channel_active: 0,
                            ..make_role(20, "InChannelInactive", "#0f0")
                        },
                        api::Role {
                            channel_ids: vec![8],
                            role_channel_active: 1,
                            ..make_role(30, "OtherChannel", "#00f")
                        },
                    ]),
                    None,
                );

                let matched: Vec<RoleId> = store
                    .roles_for_channel(TEST_CLAN, ChannelId(7))
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                assert_eq!(matched, vec![RoleId(10), RoleId(20)]);
                assert!(store.roles_for_channel(TEST_CLAN, ChannelId(99)).is_empty());
                assert!(
                    store
                        .roles_for_channel(ClanId(404), ChannelId(7))
                        .is_empty()
                );
            });
        });
    }

    #[gpui::test]
    fn channel_role_mutators_mirror_react_reducers(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
                    None,
                );

                assert!(store.add_roles_to_channel(TEST_CLAN, ChannelId(7), &[RoleId(10)], cx));
                let role = store.clan_role(TEST_CLAN, RoleId(10)).expect("role");
                assert_eq!(role.channel_ids, vec![ChannelId(7)]);
                assert!(role.role_channel_active);

                assert!(!store.add_roles_to_channel(TEST_CLAN, ChannelId(7), &[RoleId(10)], cx));

                assert!(store.remove_role_from_channel(TEST_CLAN, RoleId(10), ChannelId(7), cx));
                let role = store.clan_role(TEST_CLAN, RoleId(10)).expect("role");
                assert!(role.channel_ids.is_empty());
                assert!(role.role_channel_active);

                assert!(!store.remove_role_from_channel(TEST_CLAN, RoleId(10), ChannelId(7), cx));
            });
        });
    }

    #[gpui::test]
    fn role_event_created_adds_role_to_cached_clan(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
                    None,
                );

                let mut created = styled_role(20, "#112233", "icon.png");
                created.clan_id = TEST_CLAN.get();
                store.handle_role_event(&role_event(ROLE_EVENT_CREATED, created, TEST_ACTOR), cx);

                assert_eq!(
                    store.cache.get(&TEST_CLAN).map(|roles| roles.order.clone()),
                    Some(vec![RoleId(10), RoleId(20)])
                );
                let role = store.clan_role(TEST_CLAN, RoleId(20)).expect("new role");
                assert_eq!(role.color, "#112233");
                assert_eq!(role.icon, "icon.png");
            });
        });
    }

    #[gpui::test]
    fn role_event_update_merges_fields_and_keeps_member_count(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                let mut existing = make_role(10, "Admin", "#f00");
                existing.role_user_list = Some(api::RoleUserList {
                    role_users: vec![
                        api::role_user_list::RoleUser::default(),
                        api::role_user_list::RoleUser::default(),
                    ],
                    ..Default::default()
                });
                store
                    .cache
                    .insert(TEST_CLAN, roles_map_from_proto(vec![existing]), None);
                assert_eq!(
                    store
                        .clan_role(TEST_CLAN, RoleId(10))
                        .map(|r| r.member_count),
                    Some(2)
                );

                let mut updated = styled_role(10, "#0000ff", "new-icon.png");
                updated.clan_id = TEST_CLAN.get();
                updated.title = "Renamed".into();
                store.handle_role_event(&role_event(ROLE_EVENT_UPDATED, updated, TEST_ACTOR), cx);

                let role = store.clan_role(TEST_CLAN, RoleId(10)).expect("role");
                assert_eq!(role.name, "Renamed");
                assert_eq!(role.color, "#0000ff");
                assert_eq!(role.icon, "new-icon.png");
                assert_eq!(role.member_count, 2);
            });
        });
    }

    #[gpui::test]
    fn role_event_leaves_member_count_alone_when_role_users_not_fetched(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                let mut existing = make_role(10, "Admin", "#f00");
                existing.role_user_list = Some(api::RoleUserList {
                    role_users: vec![api::role_user_list::RoleUser::default()],
                    ..Default::default()
                });
                store
                    .cache
                    .insert(TEST_CLAN, roles_map_from_proto(vec![existing]), None);

                let mut role = styled_role(10, "#f00", "");
                role.clan_id = TEST_CLAN.get();
                role.title = "Admin".into();
                let event = RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(
                    realtime::RoleEvent {
                        role: Some(role),
                        status: ROLE_EVENT_UPDATED,
                        user_id: TEST_ACTOR,
                        user_add_ids: vec![101, 102],
                        ..Default::default()
                    },
                ));
                store.handle_role_event(&event, cx);

                assert_eq!(
                    store
                        .clan_role(TEST_CLAN, RoleId(10))
                        .map(|r| r.member_count),
                    Some(1)
                );
            });
        });
    }

    #[gpui::test]
    fn role_member_ids_come_from_the_clan_roles_snapshot(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, _cx| {
                let mut role = make_role(10, "Admin", "#f00");
                role.role_user_list = Some(api::RoleUserList {
                    role_users: vec![
                        api::role_user_list::RoleUser {
                            id: 101,
                            ..Default::default()
                        },
                        api::role_user_list::RoleUser {
                            id: 102,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                });
                store
                    .cache
                    .insert(TEST_CLAN, roles_map_from_proto(vec![role]), None);

                assert_eq!(
                    store.role_member_ids(TEST_CLAN, RoleId(10)),
                    vec![UserId(101), UserId(102)]
                );
                assert!(store.role_member_ids(TEST_CLAN, RoleId(99)).is_empty());
            });
        });
    }

    #[gpui::test]
    fn role_member_ids_prefer_the_fetched_role_user_list(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, _cx| {
                let mut role = make_role(10, "Admin", "#f00");
                role.role_user_list = Some(api::RoleUserList {
                    role_users: vec![api::role_user_list::RoleUser {
                        id: 101,
                        ..Default::default()
                    }],
                    ..Default::default()
                });
                store
                    .cache
                    .insert(TEST_CLAN, roles_map_from_proto(vec![role]), None);
                store.role_users.insert(
                    RoleId(10),
                    vec![
                        RoleUser {
                            id: UserId(101),
                            username: String::new(),
                            display_name: String::new(),
                            avatar_url: String::new(),
                        },
                        RoleUser {
                            id: UserId(103),
                            username: String::new(),
                            display_name: String::new(),
                            avatar_url: String::new(),
                        },
                    ],
                );

                assert_eq!(
                    store.role_member_ids(TEST_CLAN, RoleId(10)),
                    vec![UserId(101), UserId(103)]
                );
            });
        });
    }

    #[gpui::test]
    fn role_event_delete_removes_role(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![
                        make_role(10, "Admin", "#f00"),
                        make_role(20, "Mod", "#0f0"),
                    ]),
                    None,
                );
                store.role_users.insert(RoleId(20), Vec::new());

                let mut deleted = make_role(20, "Mod", "#0f0");
                deleted.clan_id = TEST_CLAN.get();
                store.handle_role_event(&role_event(ROLE_EVENT_DELETED, deleted, TEST_ACTOR), cx);

                assert_eq!(
                    store.cache.get(&TEST_CLAN).map(|roles| roles.order.clone()),
                    Some(vec![RoleId(10)])
                );
                assert!(store.clan_role(TEST_CLAN, RoleId(20)).is_none());
                assert!(!store.role_users.contains_key(&RoleId(20)));
            });
        });
    }

    #[gpui::test]
    fn role_event_from_self_is_ignored(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![make_role(10, "Admin", "#f00")]),
                    None,
                );

                let mut created = styled_role(20, "#112233", "icon.png");
                created.clan_id = TEST_CLAN.get();
                store.handle_role_event(&role_event(ROLE_EVENT_CREATED, created, TEST_SELF), cx);

                assert!(store.clan_role(TEST_CLAN, RoleId(20)).is_none());
            });
        });
    }

    #[gpui::test]
    fn role_event_for_unloaded_clan_is_ignored(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            store.update(cx, |store, cx| {
                let mut created = styled_role(20, "#112233", "icon.png");
                created.clan_id = TEST_CLAN.get();
                store.handle_role_event(&role_event(ROLE_EVENT_CREATED, created, TEST_ACTOR), cx);

                assert!(store.cache.get(&TEST_CLAN).is_none());
            });
        });
    }

    #[gpui::test]
    fn role_event_patches_member_role_ids_even_when_roles_are_not_cached(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            let members = ClanMembersStore::global(cx);
            members.update(cx, |members, _| {
                members.seed_members_for_test(TEST_CLAN, vec![member_with_roles(5, &[])]);
            });

            store.update(cx, |store, cx| {
                assert!(store.cache.get(&TEST_CLAN).is_none());

                let mut role = styled_role(10, "#aabbcc", "icon.png");
                role.clan_id = TEST_CLAN.get();
                let event = RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(
                    realtime::RoleEvent {
                        role: Some(role),
                        status: ROLE_EVENT_UPDATED,
                        user_id: TEST_ACTOR,
                        user_add_ids: vec![5],
                        ..Default::default()
                    },
                ));
                store.handle_role_event(&event, cx);
            });

            assert_eq!(
                members
                    .read(cx)
                    .member(TEST_CLAN, UserId(5))
                    .map(|m| m.role_ids.clone()),
                Some(vec![RoleId(10)])
            );
        });
    }

    #[gpui::test]
    fn role_event_updates_member_role_ids_and_role_style_map(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            let members = ClanMembersStore::global(cx);
            members.update(cx, |members, _| {
                members.seed_members_for_test(TEST_CLAN, vec![member_with_roles(5, &[])]);
            });

            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![styled_role(10, "#aabbcc", "icon.png")]),
                    None,
                );
                assert!(store.role_style(TEST_CLAN, UserId(5)).is_none());

                let mut role = styled_role(10, "#aabbcc", "icon.png");
                role.clan_id = TEST_CLAN.get();
                let event = RealtimeEvent::Unhandled(realtime::envelope::Message::RoleEvent(
                    realtime::RoleEvent {
                        role: Some(role),
                        status: ROLE_EVENT_UPDATED,
                        user_id: TEST_ACTOR,
                        user_add_ids: vec![5],
                        ..Default::default()
                    },
                ));
                store.handle_role_event(&event, cx);

                let style = store
                    .role_style(TEST_CLAN, UserId(5))
                    .expect("role style after grant");
                assert_eq!(style.color, parse_role_color("#aabbcc"));
                assert_eq!(style.icon.as_deref(), Some("icon.png"));
            });

            assert_eq!(
                members
                    .read(cx)
                    .member(TEST_CLAN, UserId(5))
                    .map(|member| member.role_ids.clone()),
                Some(vec![RoleId(10)])
            );
        });
    }

    #[gpui::test]
    fn role_styles_are_dropped_when_clan_leaves_the_cache(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = init_roles_store(cx);
            let members = ClanMembersStore::global(cx);
            members.update(cx, |members, _| {
                members.seed_members_for_test(TEST_CLAN, vec![member_with_roles(5, &[10])]);
            });

            store.update(cx, |store, cx| {
                store.cache.insert(
                    TEST_CLAN,
                    roles_map_from_proto(vec![styled_role(10, "#aabbcc", "icon.png")]),
                    None,
                );
                assert!(store.rebuild_role_styles(TEST_CLAN, cx));
                assert!(store.role_style(TEST_CLAN, UserId(5)).is_some());

                store.cache.remove(&TEST_CLAN);
                assert!(store.rebuild_role_styles(TEST_CLAN, cx));
                assert!(store.role_style(TEST_CLAN, UserId(5)).is_none());
                assert!(store.role_styles.is_empty());
            });
        });
    }

    #[test]
    fn parse_role_color_accepts_shorthand_and_ignores_alpha() {
        assert_eq!(parse_role_color("#fff"), parse_role_color("#ffffff"));
        assert_eq!(parse_role_color("#ff000080"), parse_role_color("#ff0000"));
        assert!(parse_role_color("").is_none());
        assert!(parse_role_color("not-a-color").is_none());
        assert!(parse_role_color("日本語").is_none());
        assert!(parse_role_color("#日本").is_none());
    }
}
