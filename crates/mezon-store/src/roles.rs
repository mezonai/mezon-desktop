use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};
use mezon_proto::api;

use crate::KeyedCache;
use crate::clan::ClanList;
use crate::clan_members::ClanMembersStore;
use crate::ids::{ClanId, RoleId, UserId};
use crate::permissions::PERMISSION_ADMINISTRATOR;
use crate::realtime::RealtimeDispatch;

const MAX_CACHED_CLANS: usize = 32;
pub const DEFAULT_ROLE_COLOR: &str = "#99aab5";
pub const MAX_ROLE_ICON_BYTES: u64 = 256 * 1024;

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
}

#[derive(Debug, Default)]
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
    role_users: HashMap<RoleId, Vec<RoleUser>>,
    role_users_loading: HashSet<RoleId>,
    loading: HashSet<ClanId>,
    saving: HashSet<ClanId>,
    reset_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
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
        self.role_users.clear();
        self.role_users_loading.clear();
        self.loading.clear();
        self.saving.clear();
        cx.notify();
    }

    pub fn is_saving(&self, clan_id: ClanId) -> bool {
        self.saving.contains(&clan_id)
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on_lagged(&entity, |this, cx| {
                this.cache.mark_all_stale();
                this.refresh_active_from_event(cx);
            });
        });
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CLANS)),
            role_users: HashMap::new(),
            role_users_loading: HashSet::new(),
            loading: HashSet::new(),
            saving: HashSet::new(),
            reset_generation: 0,
            api,
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

    pub fn role_users(&self, role_id: RoleId) -> &[RoleUser] {
        self.role_users
            .get(&role_id)
            .map(|users| users.as_slice())
            .unwrap_or(&[])
    }

    pub fn ensure_role_users_loaded(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        if self.role_users.contains_key(&role_id) || self.role_users_loading.contains(&role_id) {
            return;
        }
        self.start_role_users_fetch(role_id, cx);
    }

    fn reload_role_users(&mut self, role_id: RoleId, cx: &mut Context<Self>) {
        self.role_users_loading.remove(&role_id);
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
                if this.reset_generation != generation || !complete {
                    return;
                }
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
        if !self.saving.insert(clan_id) {
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
                this.saving.remove(&clan_id);
                match result {
                    Ok(()) => {
                        this.apply_role_member_patch(role_id, clan_id, &added, &removed, cx);
                        this.sync_role_member_count(role_id, clan_id);
                        this.reload_role_users(role_id, cx);
                        this.refresh_active(clan_id, cx);
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!("mutate_role_members failed for role {role_id}: {err}");
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
        if self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
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
                        this.cache.insert(clan_id, roles, None);
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

    pub fn role_has_administrator(&self, role: &ClanRoleDetail) -> bool {
        role.permissions
            .iter()
            .any(|p| p.active && p.slug == PERMISSION_ADMINISTRATOR)
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

    fn upsert_role_from_proto(&mut self, clan_id: ClanId, r: api::Role) {
        if r.id == 0 || r.active != 1 {
            return;
        }
        let id = RoleId(r.id);
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
        let member_count = r
            .role_user_list
            .map(|list| list.role_users.len())
            .unwrap_or(0);
        let detail = ClanRoleDetail {
            name: r.title,
            color: r.color,
            icon: r.role_icon,
            slug: r.slug,
            active: r.active == 1,
            order_role: r.order_role,
            max_level_permission: r.max_level_permission,
            permissions,
            member_count,
        };
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
        let member_count = r
            .role_user_list
            .map(|list| list.role_users.len())
            .unwrap_or(0);
        if out
            .by_id
            .insert(
                id,
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
                },
            )
            .is_none()
        {
            out.order.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_proto::api;

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
}
