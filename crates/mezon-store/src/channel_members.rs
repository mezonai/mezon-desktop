use crate::ids::{ChannelId, ClanId, RoleId, UserId};
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::channel::{ChannelEvent, ChannelList};
use crate::clan::ClanList;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_CHANNELS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelMember {
    pub user_id: UserId,
    pub role_ids: Vec<RoleId>,
}

#[derive(Debug, Clone)]
pub enum ChannelMembersEvent {
    Changed { channel_id: ChannelId },
}

#[derive(Default)]
struct ChannelBucket {
    members: Vec<ChannelMember>,
    by_id: HashMap<UserId, usize>,
}

impl ChannelBucket {
    fn upsert(&mut self, member: ChannelMember) {
        if let Some(&idx) = self.by_id.get(&member.user_id) {
            self.members[idx] = member;
        } else {
            self.by_id.insert(member.user_id, self.members.len());
            self.members.push(member);
        }
    }

    fn remove(&mut self, user_id: UserId) {
        if let Some(idx) = self.by_id.get(&user_id).copied() {
            self.members.remove(idx);
            self.reindex();
        }
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.by_id.reserve(self.members.len());
        for (i, m) in self.members.iter().enumerate() {
            self.by_id.insert(m.user_id, i);
        }
    }
}

pub struct ChannelMembersStore {
    cache: KeyedCache<ChannelId, ChannelBucket>,
    loading: HashMap<ChannelId, bool>,
    api: Arc<AppApi>,
    _channel_sub: Subscription,
    _conn_watch: Task<()>,
}

struct GlobalChannelMembersStore(Entity<ChannelMembersStore>);
impl Global for GlobalChannelMembersStore {}

impl EventEmitter<ChannelMembersEvent> for ChannelMembersStore {}

impl ChannelMembersStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalChannelMembersStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelMembersStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelMembersStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cache.clear();
        self.loading.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let ChannelEvent::ActiveChannelChanged(Some(channel_id)) = event {
                this.ensure_loaded(*channel_id, cx);
            }
        });

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            loading: HashMap::new(),
            api,
            _channel_sub: channel_sub,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::UserChannelAdded,
                RealtimeKind::UserChannelRemoved,
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
                    if this.update(cx, |this, _| this.invalidate()).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn member_ids(&self, channel_id: ChannelId) -> Vec<UserId> {
        match self.cache.get(&channel_id) {
            Some(bucket) => bucket.members.iter().map(|m| m.user_id).collect(),
            None => Vec::new(),
        }
    }

    pub fn member_ids_preview(&self, channel_id: ChannelId, max: usize) -> (Vec<UserId>, usize) {
        match self.cache.get(&channel_id) {
            Some(bucket) => {
                let preview = bucket.members.iter().take(max).map(|m| m.user_id).collect();
                (preview, bucket.members.len())
            }
            None => (Vec::new(), 0),
        }
    }

    pub fn has_channel(&self, channel_id: ChannelId) -> bool {
        self.cache.contains(&channel_id)
    }

    /// Seed a channel's roster from a `ListChannelUsers` response fetched outside
    /// this store, so a caller that had to look the members up itself does not
    /// leave the cache cold for the next reader.
    pub fn apply_members_loaded(
        &mut self,
        channel_id: ChannelId,
        users: &[api::channel_user_list::ChannelUser],
        cx: &mut Context<Self>,
    ) {
        let mut bucket = ChannelBucket::default();
        for cu in users {
            bucket.upsert(channel_member_from_proto(cu));
        }
        self.cache.insert(channel_id, bucket, None);
        cx.emit(ChannelMembersEvent::Changed { channel_id });
        cx.notify();
    }

    pub fn apply_members_added(
        &mut self,
        channel_id: ChannelId,
        user_ids: &[UserId],
        cx: &mut Context<Self>,
    ) {
        let Some(bucket) = self.cache.get_mut(&channel_id) else {
            return;
        };
        if !add_members_to_bucket(bucket, user_ids) {
            return;
        }
        cx.emit(ChannelMembersEvent::Changed { channel_id });
        cx.notify();
    }

    pub fn remove_member(
        &mut self,
        channel_id: ChannelId,
        user_id: UserId,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            api.remove_channel_users(channel_id.get(), vec![user_id.get().to_string()])
                .await?;
            this.update(cx, |this, cx| {
                if let Some(bucket) = this.cache.get_mut(&channel_id) {
                    bucket.remove(user_id);
                }
                cx.emit(ChannelMembersEvent::Changed { channel_id });
                cx.notify();
                if let Some(store) = crate::channel_users::ChannelUsersStore::try_global(cx) {
                    store.update(cx, |store, cx| {
                        store.remove_users(channel_id, &[user_id], cx);
                    });
                }
            })?;
            Ok(())
        })
    }

    pub fn is_loading(&self, channel_id: ChannelId) -> bool {
        self.loading.get(&channel_id).copied().unwrap_or(false)
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        if let Some(channel_id) = ChannelList::global(cx).read(cx).active_channel_id {
            self.fetch(channel_id, cx);
        }
    }

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
    }

    pub fn ensure_loaded(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.cache.touch(&channel_id);
        if !self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
            self.fetch(channel_id, cx);
        }
    }

    fn clan_and_type_for_channel(channel_id: ChannelId, cx: &App) -> Option<(ClanId, i32)> {
        let found = ChannelList::global(cx)
            .read(cx)
            .find_channel_in_active_clan(channel_id)
            .map(|c| (c.clan_id, c.channel_type.as_raw() as i32));
        match found {
            Some((clan_id, channel_type)) if !clan_id.is_zero() => Some((clan_id, channel_type)),
            Some((_, channel_type)) => ClanList::global(cx)
                .read(cx)
                .active_clan_id
                .map(|clan_id| (clan_id, channel_type)),
            None => ClanList::global(cx)
                .read(cx)
                .active_clan_id
                .map(|clan_id| (clan_id, 0)),
        }
    }

    fn fetch(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.loading.get(&channel_id).copied().unwrap_or(false) {
            return;
        }
        let Some((clan_id, channel_type)) = Self::clan_and_type_for_channel(channel_id, cx) else {
            return;
        };
        self.loading.insert(channel_id, true);
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_users(clan_id.get(), channel_id.get(), channel_type)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&channel_id);
                match result {
                    Ok(users) => this.apply_members_loaded(channel_id, &users, cx),
                    Err(e) => {
                        tracing::error!("list_channel_users failed for {channel_id}: {e}");
                        cx.emit(ChannelMembersEvent::Changed { channel_id });
                    }
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::UserChannelAdded(e) => {
                let Some(channel_id) = e.channel_desc.as_ref().map(|d| ChannelId(d.channel_id))
                else {
                    return;
                };
                let Some(bucket) = self.cache.get_mut(&channel_id) else {
                    return;
                };
                for user in &e.users {
                    bucket.upsert(channel_member_from_redis(user));
                }
                cx.emit(ChannelMembersEvent::Changed { channel_id });
                cx.notify();
            }
            RealtimeEvent::UserChannelRemoved(e) => {
                let channel_id = ChannelId(e.channel_id);
                let Some(bucket) = self.cache.get_mut(&channel_id) else {
                    return;
                };
                for uid in &e.user_ids {
                    bucket.remove(UserId(*uid));
                }
                cx.emit(ChannelMembersEvent::Changed { channel_id });
                cx.notify();
            }
            _ => {}
        }
    }
}

fn add_members_to_bucket(bucket: &mut ChannelBucket, user_ids: &[UserId]) -> bool {
    let mut changed = false;
    for user_id in user_ids {
        if bucket.members.iter().any(|m| m.user_id == *user_id) {
            continue;
        }
        bucket.upsert(ChannelMember {
            user_id: *user_id,
            role_ids: Vec::new(),
        });
        changed = true;
    }
    changed
}

fn channel_member_from_proto(cu: &api::channel_user_list::ChannelUser) -> ChannelMember {
    ChannelMember {
        user_id: UserId(cu.user_id),
        role_ids: cu.role_id.iter().map(|id| RoleId(*id)).collect(),
    }
}

fn channel_member_from_redis(user: &realtime::UserProfileRedis) -> ChannelMember {
    ChannelMember {
        user_id: UserId(user.user_id),
        role_ids: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_channel_user(user_id: i64) -> api::channel_user_list::ChannelUser {
        api::channel_user_list::ChannelUser {
            user_id,
            role_id: vec![5],
            ..Default::default()
        }
    }

    #[test]
    fn maps_proto_channel_user_to_domain() {
        let member = channel_member_from_proto(&proto_channel_user(9));
        assert_eq!(member.user_id, UserId(9));
        assert_eq!(member.role_ids, vec![RoleId(5)]);
    }

    #[test]
    fn adding_members_locally_skips_users_already_in_the_bucket() {
        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));

        assert!(add_members_to_bucket(&mut bucket, &[UserId(1), UserId(2)]));
        assert_eq!(
            bucket.members.iter().map(|m| m.user_id).collect::<Vec<_>>(),
            vec![UserId(1), UserId(2)]
        );
        assert_eq!(bucket.members[0].role_ids, vec![RoleId(5)]);
    }

    #[test]
    fn adding_only_known_members_reports_no_change() {
        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));

        assert!(!add_members_to_bucket(&mut bucket, &[UserId(1)]));
        assert!(!add_members_to_bucket(&mut bucket, &[]));
    }

    #[test]
    fn bucket_upsert_dedupes_by_user() {
        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));
        bucket.upsert(channel_member_from_proto(&proto_channel_user(2)));
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));
        assert_eq!(bucket.members.len(), 2);
    }

    #[test]
    fn bucket_remove_drops_member() {
        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));
        bucket.upsert(channel_member_from_proto(&proto_channel_user(2)));
        bucket.remove(UserId(1));
        assert_eq!(bucket.members.len(), 1);
        assert_eq!(bucket.members[0].user_id, UserId(2));
    }

    fn assert_index_consistent(bucket: &ChannelBucket) {
        assert_eq!(bucket.by_id.len(), bucket.members.len());
        for (i, m) in bucket.members.iter().enumerate() {
            assert_eq!(bucket.by_id.get(&m.user_id), Some(&i));
        }
    }

    #[test]
    fn bucket_index_stays_consistent_across_upsert_and_remove() {
        let mut bucket = ChannelBucket::default();
        for id in [1, 2, 3, 4] {
            bucket.upsert(channel_member_from_proto(&proto_channel_user(id)));
        }
        assert_index_consistent(&bucket);
        bucket.upsert(channel_member_from_proto(&proto_channel_user(2)));
        assert_index_consistent(&bucket);
        bucket.remove(UserId(1));
        assert_index_consistent(&bucket);
        bucket.remove(UserId(3));
        assert_index_consistent(&bucket);
        let ids: Vec<UserId> = bucket.members.iter().map(|m| m.user_id).collect();
        assert_eq!(ids, vec![UserId(2), UserId(4)]);
    }

    #[test]
    fn redis_profile_maps_user_id_only() {
        let redis = realtime::UserProfileRedis {
            user_id: 33,
            ..Default::default()
        };
        let member = channel_member_from_redis(&redis);
        assert_eq!(member.user_id, UserId(33));
        assert!(member.role_ids.is_empty());
    }

    #[test]
    fn mark_all_stale_keeps_members_visible_then_refetch_restores_freshness() {
        let mut cache: KeyedCache<ChannelId, ChannelBucket> = KeyedCache::new(None);
        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));
        cache.insert(ChannelId(7), bucket, None);
        assert!(cache.is_fresh(&ChannelId(7), crate::CACHE_TTL));

        cache.mark_all_stale();
        assert!(!cache.is_fresh(&ChannelId(7), crate::CACHE_TTL));
        assert_eq!(cache.get(&ChannelId(7)).unwrap().members.len(), 1);

        let mut bucket = ChannelBucket::default();
        bucket.upsert(channel_member_from_proto(&proto_channel_user(1)));
        cache.insert(ChannelId(7), bucket, None);
        assert!(cache.is_fresh(&ChannelId(7), crate::CACHE_TTL));
    }
}
