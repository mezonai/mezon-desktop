use crate::ids::{ClanId, RoleId, UserId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::badge::BadgeService;
use crate::clan::{ClanEvent, ClanList};
use crate::presence::PresenceStore;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const ONLINE_FETCH_LIMIT: i32 = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    pub about_me: String,
    pub create_time_seconds: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClanMember {
    pub user: User,
    pub clan_nick: String,
    pub clan_avatar: String,
    pub role_ids: Vec<RoleId>,
    pub online: bool,
}

impl ClanMember {
    pub fn id(&self) -> UserId {
        self.user.id
    }

    pub fn name(&self) -> &str {
        if !self.clan_nick.is_empty() {
            &self.clan_nick
        } else if !self.user.display_name.is_empty() {
            &self.user.display_name
        } else {
            &self.user.username
        }
    }

    pub fn avatar(&self) -> &str {
        if !self.clan_avatar.is_empty() {
            &self.clan_avatar
        } else {
            &self.user.avatar_url
        }
    }
}

#[derive(Debug, Clone)]
pub enum ClanMembersEvent {
    Changed { clan_id: ClanId },
}

#[derive(Default)]
struct ClanBucket {
    by_id: HashMap<UserId, ClanMember>,
    ids: Vec<UserId>,
}

impl ClanBucket {
    fn upsert(&mut self, member: ClanMember) {
        let id = member.user.id;
        if self.by_id.insert(id, member).is_none() {
            self.ids.push(id);
        }
    }

    fn remove(&mut self, user_id: UserId) {
        if self.by_id.remove(&user_id).is_some() {
            self.ids.retain(|id| *id != user_id);
        }
    }
}

pub struct ClanMembersStore {
    cache: KeyedCache<ClanId, ClanBucket>,
    loading: HashSet<ClanId>,
    api: Arc<AppApi>,
    _clan_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalClanMembersStore(Entity<ClanMembersStore>);
impl Global for GlobalClanMembersStore {}

impl EventEmitter<ClanMembersEvent> for ClanMembersStore {}

impl ClanMembersStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalClanMembersStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalClanMembersStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalClanMembersStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cache.clear();
        self.loading.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let clan_sub = cx.subscribe(&ClanList::global(cx), |this, _clan, event, cx| {
            if let ClanEvent::ActiveClanChanged(Some(clan_id)) = event {
                this.ensure_loaded(*clan_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(None),
            loading: HashSet::new(),
            api,
            _clan_sub: clan_sub,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::AddClanUser,
                RealtimeKind::UserClanRemoved,
                RealtimeKind::ClanProfileUpdated,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| this.refresh_active(cx));
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
                    let marked = this.update(cx, |this, cx| {
                        this.cache.mark_all_stale();
                        this.refresh_active(cx);
                    });
                    if marked.is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn member(&self, clan_id: ClanId, user_id: UserId) -> Option<&ClanMember> {
        self.cache.get(&clan_id)?.by_id.get(&user_id)
    }

    pub fn members(&self, clan_id: ClanId) -> Vec<&ClanMember> {
        match self.cache.get(&clan_id) {
            Some(bucket) => bucket
                .ids
                .iter()
                .filter_map(|id| bucket.by_id.get(id))
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn count(&self, clan_id: ClanId) -> usize {
        self.cache.get(&clan_id).map(|b| b.ids.len()).unwrap_or(0)
    }

    pub fn debug_memory(&self) -> (usize, usize) {
        let mut clans = 0;
        let mut members = 0;
        for (_, bucket) in self.cache.iter() {
            clans += 1;
            members += bucket.ids.len();
        }
        (clans, members)
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        if let Some(clan_id) = ClanList::global(cx).read(cx).active_clan_id {
            self.fetch(clan_id, cx);
        }
    }

    pub fn ensure_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            self.fetch(clan_id, cx);
        }
    }

    pub fn refresh(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetch(clan_id, cx);
    }

    fn fetch(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let (users_result, online_result, status_result) = tokio::join!(
                api.list_clan_users(clan_id.get()),
                api.list_user_online(clan_id.get(), ONLINE_FETCH_LIMIT, 0),
                api.list_clan_users_status(clan_id.get()),
            );
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&clan_id);
                match users_result {
                    Ok(users) => {
                        let self_id = BadgeService::global(cx)
                            .read(cx)
                            .current_user_id(cx)
                            .map(|uid| uid.0);
                        let online_users = online_result.unwrap_or_default();
                        let online_ids = online_ids_from_users(&online_users, self_id);
                        let mut bucket = ClanBucket::default();
                        for cu in users {
                            if let Some(mut member) = clan_member_from_proto(cu) {
                                member.online = online_ids.contains(&member.user.id.0);
                                bucket.upsert(member);
                            }
                        }
                        tracing::info!(
                            "ClanMembersStore: fetched {} members for clan {clan_id}",
                            bucket.ids.len()
                        );
                        this.cache.insert(clan_id, bucket, None);
                        let online_uids: Vec<UserId> =
                            online_ids.iter().map(|id| UserId(*id)).collect();
                        let statuses = status_result
                            .map(user_statuses_from_list)
                            .unwrap_or_default();
                        PresenceStore::global(cx).update(cx, |presence, cx| {
                            presence.seed_presence(&online_uids, &statuses, cx);
                        });
                        cx.emit(ClanMembersEvent::Changed { clan_id });
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_clan_users failed for {clan_id}: {e}"),
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let changed_clan = match event {
            RealtimeEvent::AddClanUser(e) => {
                let clan_id = ClanId(e.clan_id);
                clan_member_from_redis(e.user.as_ref())
                    .filter(|member| apply_add_member(&mut self.cache, clan_id, member.clone()))
                    .map(|_| clan_id)
            }
            RealtimeEvent::UserClanRemoved(e) => {
                let clan_id = ClanId(e.clan_id);
                let ids: Vec<UserId> = e.user_ids.iter().map(|id| UserId(*id)).collect();
                apply_remove_members(&mut self.cache, clan_id, &ids).then_some(clan_id)
            }
            RealtimeEvent::ClanProfileUpdated(e) => {
                let clan_id = ClanId(e.clan_id);
                apply_profile_update(
                    &mut self.cache,
                    clan_id,
                    UserId(e.user_id),
                    &e.clan_nick,
                    &e.clan_avatar,
                )
                .then_some(clan_id)
            }
            _ => None,
        };
        if let Some(clan_id) = changed_clan {
            cx.emit(ClanMembersEvent::Changed { clan_id });
            cx.notify();
        }
    }
}

/// Add a member to an **already-loaded** clan bucket. Ignored for a clan we have not fetched yet,
/// so a partial bucket never blocks the full `list_clan_users` fetch on clan switch.
fn apply_add_member(
    by_clan: &mut KeyedCache<ClanId, ClanBucket>,
    clan_id: ClanId,
    member: ClanMember,
) -> bool {
    match by_clan.get_mut(&clan_id) {
        Some(bucket) => {
            bucket.upsert(member);
            true
        }
        None => false,
    }
}

fn apply_remove_members(
    by_clan: &mut KeyedCache<ClanId, ClanBucket>,
    clan_id: ClanId,
    user_ids: &[UserId],
) -> bool {
    let Some(bucket) = by_clan.get_mut(&clan_id) else {
        return false;
    };
    for id in user_ids {
        bucket.remove(*id);
    }
    true
}

fn apply_profile_update(
    by_clan: &mut KeyedCache<ClanId, ClanBucket>,
    clan_id: ClanId,
    user_id: UserId,
    clan_nick: &str,
    clan_avatar: &str,
) -> bool {
    let Some(bucket) = by_clan.get_mut(&clan_id) else {
        return false;
    };
    let Some(member) = bucket.by_id.get_mut(&user_id) else {
        return false;
    };
    member.clan_nick = clan_nick.to_string();
    member.clan_avatar = clan_avatar.to_string();
    true
}

pub(crate) fn user_from_api(user: api::User) -> Option<User> {
    if user.id == 0 {
        return None;
    }
    Some(User {
        id: UserId(user.id),
        username: user.username,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
        about_me: user.about_me,
        create_time_seconds: user.create_time_seconds,
    })
}

fn clan_member_from_proto(cu: api::clan_user_list::ClanUser) -> Option<ClanMember> {
    let user = user_from_api(cu.user?)?;
    Some(ClanMember {
        user,
        clan_nick: cu.clan_nick,
        clan_avatar: cu.clan_avatar,
        role_ids: cu.role_id.iter().map(|id| RoleId(*id)).collect(),
        online: false,
    })
}

fn clan_member_from_redis(user: Option<&realtime::UserProfileRedis>) -> Option<ClanMember> {
    let user = user?;
    if user.user_id == 0 {
        return None;
    }
    Some(ClanMember {
        user: User {
            id: UserId(user.user_id),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar.clone(),
            about_me: String::new(),
            create_time_seconds: user.create_time_second,
        },
        clan_nick: String::new(),
        clan_avatar: String::new(),
        role_ids: Vec::new(),
        online: false,
    })
}

fn user_statuses_from_list(list: api::ClanUserStatusList) -> Vec<(UserId, String)> {
    list.clan_user_statuses
        .into_iter()
        .filter(|e| e.user_id != 0)
        .map(|e| (UserId(e.user_id), e.user_status))
        .collect()
}

fn online_ids_from_users(users: &[api::User], self_id: Option<i64>) -> HashSet<i64> {
    let mut ids: HashSet<i64> = users.iter().map(|u| u.id).filter(|id| *id != 0).collect();
    if let Some(sid) = self_id.filter(|id| *id != 0) {
        ids.insert(sid);
    }
    ids
}

/// Partition members into (online, offline) id lists, each ordered online-first then by
/// case-insensitive display name — mirrors React `selectChannelMembersSortedByStatus`.
pub fn split_members_by_status(
    members: &[&ClanMember],
    online: &HashSet<UserId>,
) -> (Vec<UserId>, Vec<UserId>) {
    let mut sorted: Vec<&&ClanMember> = members.iter().collect();
    sorted.sort_by_cached_key(|m| (!online.contains(&m.id()), m.name().to_lowercase()));

    let mut online_ids = Vec::new();
    let mut offline_ids = Vec::new();
    for member in sorted {
        if online.contains(&member.id()) {
            online_ids.push(member.id());
        } else {
            offline_ids.push(member.id());
        }
    }
    (online_ids, offline_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_clan_user(
        id: i64,
        username: &str,
        display: &str,
        nick: &str,
    ) -> api::clan_user_list::ClanUser {
        api::clan_user_list::ClanUser {
            user: Some(api::User {
                id,
                username: username.into(),
                display_name: display.into(),
                avatar_url: "avatar.png".into(),
                ..Default::default()
            }),
            role_id: vec![10, 20],
            clan_nick: nick.into(),
            clan_avatar: String::new(),
            clan_id: 1,
        }
    }

    #[test]
    fn maps_proto_clan_user_to_domain() {
        let member = clan_member_from_proto(proto_clan_user(42, "alice", "Alice", "")).unwrap();
        assert_eq!(member.id(), UserId(42));
        assert_eq!(member.user.username, "alice");
        assert_eq!(member.role_ids, vec![RoleId(10), RoleId(20)]);
    }

    #[test]
    fn name_prefers_clan_nick_then_display_then_username() {
        let with_nick = clan_member_from_proto(proto_clan_user(1, "u", "Disp", "Nick")).unwrap();
        assert_eq!(with_nick.name(), "Nick");
        let with_display = clan_member_from_proto(proto_clan_user(1, "u", "Disp", "")).unwrap();
        assert_eq!(with_display.name(), "Disp");
        let with_username = clan_member_from_proto(proto_clan_user(1, "uname", "", "")).unwrap();
        assert_eq!(with_username.name(), "uname");
    }

    #[test]
    fn avatar_prefers_clan_avatar_over_user_avatar() {
        let mut cu = proto_clan_user(1, "u", "U", "");
        cu.clan_avatar = "clan.png".into();
        let member = clan_member_from_proto(cu).unwrap();
        assert_eq!(member.avatar(), "clan.png");
    }

    #[test]
    fn skips_clan_user_without_user_or_zero_id() {
        let mut cu = proto_clan_user(0, "u", "U", "");
        assert!(clan_member_from_proto(cu.clone()).is_none());
        cu.user = None;
        assert!(clan_member_from_proto(cu).is_none());
    }

    #[test]
    fn maps_add_clan_user_redis_profile() {
        let redis = realtime::UserProfileRedis {
            user_id: 7,
            username: "bob".into(),
            display_name: "Bob".into(),
            avatar: "bob.png".into(),
            ..Default::default()
        };
        let member = clan_member_from_redis(Some(&redis)).unwrap();
        assert_eq!(member.id(), UserId(7));
        assert_eq!(member.name(), "Bob");
        assert_eq!(member.avatar(), "bob.png");
    }

    #[test]
    fn bucket_upsert_dedupes_and_tracks_order() {
        let mut bucket = ClanBucket::default();
        bucket.upsert(clan_member_from_proto(proto_clan_user(1, "a", "A", "")).unwrap());
        bucket.upsert(clan_member_from_proto(proto_clan_user(2, "b", "B", "")).unwrap());
        bucket.upsert(clan_member_from_proto(proto_clan_user(1, "a", "A2", "")).unwrap());
        assert_eq!(bucket.ids, vec![UserId(1), UserId(2)]);
        assert_eq!(bucket.by_id[&UserId(1)].user.display_name, "A2");
    }

    #[test]
    fn bucket_remove_drops_member_and_order() {
        let mut bucket = ClanBucket::default();
        bucket.upsert(clan_member_from_proto(proto_clan_user(1, "a", "A", "")).unwrap());
        bucket.upsert(clan_member_from_proto(proto_clan_user(2, "b", "B", "")).unwrap());
        bucket.remove(UserId(1));
        assert_eq!(bucket.ids, vec![UserId(2)]);
        assert!(!bucket.by_id.contains_key(&UserId(1)));
    }

    fn api_user(id: i64, online: bool) -> api::User {
        api::User {
            id,
            online,
            ..Default::default()
        }
    }

    #[test]
    fn online_ids_collects_returned_users_and_adds_self() {
        let users = vec![api_user(1, true), api_user(2, true)];
        let ids = online_ids_from_users(&users, Some(9));
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&9));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn online_ids_skips_zero_ids_and_zero_self() {
        let users = vec![api_user(0, true), api_user(5, true)];
        let ids = online_ids_from_users(&users, Some(0));
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&5));
    }

    #[test]
    fn online_ids_without_self_returns_only_online_users() {
        let ids = online_ids_from_users(&[api_user(3, true)], None);
        assert_eq!(ids, [3].into_iter().collect());
    }

    #[test]
    fn user_statuses_maps_entries_and_skips_zero_ids() {
        let list = api::ClanUserStatusList {
            clan_user_statuses: vec![
                api::ClanUserStatusEntry {
                    user_id: 1,
                    user_status: "Working".into(),
                },
                api::ClanUserStatusEntry {
                    user_id: 0,
                    user_status: "ghost".into(),
                },
            ],
        };
        let statuses = user_statuses_from_list(list);
        assert_eq!(statuses, vec![(UserId(1), "Working".to_string())]);
    }

    #[test]
    fn split_orders_online_first_then_by_name() {
        let zoe = clan_member_from_proto(proto_clan_user(1, "zoe", "Zoe", "")).unwrap();
        let amy = clan_member_from_proto(proto_clan_user(2, "amy", "Amy", "")).unwrap();
        let bob = clan_member_from_proto(proto_clan_user(3, "bob", "Bob", "")).unwrap();
        let members = vec![&zoe, &amy, &bob];
        let online: HashSet<UserId> = [UserId(1)].into_iter().collect();
        let (online_ids, offline_ids) = split_members_by_status(&members, &online);
        assert_eq!(online_ids, vec![UserId(1)]);
        assert_eq!(offline_ids, vec![UserId(2), UserId(3)]);
    }

    fn member(id: i64, name: &str) -> ClanMember {
        clan_member_from_proto(proto_clan_user(id, name, name, "")).unwrap()
    }

    fn bucket_with(members: Vec<ClanMember>) -> KeyedCache<ClanId, ClanBucket> {
        let mut bucket = ClanBucket::default();
        for m in members {
            bucket.upsert(m);
        }
        let mut cache = KeyedCache::new(None);
        cache.insert(ClanId(1), bucket, None);
        cache
    }

    #[test]
    fn add_member_applies_to_loaded_clan() {
        let mut by_clan = bucket_with(vec![]);
        assert!(apply_add_member(&mut by_clan, ClanId(1), member(1, "a")));
        assert!(
            by_clan
                .get(&ClanId(1))
                .unwrap()
                .by_id
                .contains_key(&UserId(1))
        );
    }

    #[test]
    fn add_member_ignored_for_unfetched_clan() {
        let mut by_clan: KeyedCache<ClanId, ClanBucket> = KeyedCache::new(None);
        assert!(!apply_add_member(&mut by_clan, ClanId(1), member(1, "a")));
        assert!(by_clan.get(&ClanId(1)).is_none());
    }

    #[test]
    fn remove_members_drops_from_loaded_bucket() {
        let mut by_clan = bucket_with(vec![member(1, "a"), member(2, "b")]);
        assert!(apply_remove_members(&mut by_clan, ClanId(1), &[UserId(1)]));
        assert_eq!(by_clan.get(&ClanId(1)).unwrap().ids, vec![UserId(2)]);
    }

    #[test]
    fn remove_members_ignored_for_unfetched_clan() {
        let mut by_clan: KeyedCache<ClanId, ClanBucket> = KeyedCache::new(None);
        assert!(!apply_remove_members(&mut by_clan, ClanId(1), &[UserId(1)]));
    }

    #[test]
    fn profile_update_changes_nick_and_avatar() {
        let mut by_clan = bucket_with(vec![member(1, "a")]);
        assert!(apply_profile_update(
            &mut by_clan,
            ClanId(1),
            UserId(1),
            "NewNick",
            "new.png"
        ));
        let bucket = by_clan.get(&ClanId(1)).unwrap();
        let updated = &bucket.by_id[&UserId(1)];
        assert_eq!(updated.name(), "NewNick");
        assert_eq!(updated.avatar(), "new.png");
    }

    #[test]
    fn profile_update_ignored_for_unknown_member() {
        let mut by_clan = bucket_with(vec![]);
        assert!(!apply_profile_update(
            &mut by_clan,
            ClanId(1),
            UserId(99),
            "x",
            "y"
        ));
    }

    #[test]
    fn reconnect_mark_all_stale_keeps_members_then_refetch_restores_freshness() {
        let mut by_clan = bucket_with(vec![member(1, "a"), member(2, "b")]);
        assert!(by_clan.is_fresh(&ClanId(1), crate::CACHE_TTL));

        by_clan.mark_all_stale();
        assert!(!by_clan.is_fresh(&ClanId(1), crate::CACHE_TTL));
        assert_eq!(by_clan.get(&ClanId(1)).unwrap().ids.len(), 2);

        let mut refreshed = ClanBucket::default();
        refreshed.upsert(member(1, "a"));
        by_clan.insert(ClanId(1), refreshed, None);
        assert!(by_clan.is_fresh(&ClanId(1), crate::CACHE_TTL));
    }
}
