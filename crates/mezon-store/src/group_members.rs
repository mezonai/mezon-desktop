use crate::ids::{ChannelId, UserId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent, api_status_from_error};
use mezon_proto::{api, realtime};

use crate::KeyedCache;
use crate::clan_members::User;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_GROUPS: usize = 64;

const GROUP_MEMBER_FETCH_LIMIT: i32 = 500;

pub const MAX_GROUP_MEMBERS: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupMember {
    pub user: User,
    pub online: bool,
}

impl GroupMember {
    pub fn id(&self) -> UserId {
        self.user.id
    }

    pub fn name(&self) -> &str {
        if !self.user.display_name.is_empty() {
            &self.user.display_name
        } else {
            &self.user.username
        }
    }

    pub fn avatar(&self) -> &str {
        &self.user.avatar_url
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddGroupMembersError {
    GroupFull,
    Api(u32),
    Other(String),
}

#[derive(Debug, Clone)]
pub enum GroupMembersEvent {
    Changed { channel_id: ChannelId },
}

#[derive(Default)]
struct GroupBucket {
    members: Vec<GroupMember>,
    by_id: HashMap<UserId, usize>,
}

impl GroupBucket {
    fn from_members(members: Vec<GroupMember>) -> Self {
        let mut bucket = Self {
            members,
            by_id: HashMap::new(),
        };
        bucket.reindex();
        bucket
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.by_id.reserve(self.members.len());
        for (i, m) in self.members.iter().enumerate() {
            self.by_id.insert(m.user.id, i);
        }
    }

    fn as_slice(&self) -> &[GroupMember] {
        &self.members
    }

    fn get(&self, user_id: UserId) -> Option<&GroupMember> {
        let idx = *self.by_id.get(&user_id)?;
        self.members.get(idx)
    }

    /// Returns whether the roster actually moved, so a redelivered event does not
    /// claim a change.
    fn upsert(&mut self, member: GroupMember) -> bool {
        if let Some(&idx) = self.by_id.get(&member.user.id) {
            if self.members[idx] == member {
                return false;
            }
            self.members[idx] = member;
        } else {
            self.by_id.insert(member.user.id, self.members.len());
            self.members.push(member);
        }
        true
    }

    fn remove_ids(&mut self, user_ids: &[UserId]) -> bool {
        let before = self.members.len();
        self.members.retain(|m| !user_ids.contains(&m.user.id));
        let removed = self.members.len() != before;
        if removed {
            self.reindex();
        }
        removed
    }
}

pub struct GroupMembersStore {
    cache: KeyedCache<ChannelId, GroupBucket>,
    loading: HashSet<ChannelId>,
    mutations: HashMap<ChannelId, u64>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalGroupMembersStore(Entity<GroupMembersStore>);
impl Global for GlobalGroupMembersStore {}

impl EventEmitter<GroupMembersEvent> for GroupMembersStore {}

impl GroupMembersStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalGroupMembersStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalGroupMembersStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalGroupMembersStore>()
            .map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cache.clear();
        self.loading.clear();
        self.mutations.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_GROUPS)),
            loading: HashSet::new(),
            mutations: HashMap::new(),
            api,
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
            dispatch.on_lagged(&entity, |this, _cx| this.invalidate());
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

    fn invalidate(&mut self) {
        self.cache.mark_all_stale();
    }

    pub fn is_loaded(&self, channel_id: ChannelId) -> bool {
        self.cache.contains(&channel_id)
    }

    pub fn members(&self, channel_id: ChannelId) -> &[GroupMember] {
        self.cache
            .get(&channel_id)
            .map(GroupBucket::as_slice)
            .unwrap_or(&[])
    }

    pub fn member(&self, channel_id: ChannelId, user_id: UserId) -> Option<&GroupMember> {
        self.cache.get(&channel_id)?.get(user_id)
    }

    pub fn ensure_loaded(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.cache.touch(&channel_id);
        if !self.cache.is_fresh(&channel_id, crate::CACHE_TTL) {
            self.fetch(channel_id, cx);
        }
    }

    pub fn refresh(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        self.fetch(channel_id, cx);
    }

    pub fn add_members(
        &mut self,
        channel_id: ChannelId,
        user_ids: Vec<UserId>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), AddGroupMembersError>> {
        if user_ids.is_empty() {
            return Task::ready(Ok(()));
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let ids: Vec<String> = user_ids.iter().map(|id| id.get().to_string()).collect();
            if let Err(err) = api.add_channel_users(channel_id.get(), ids).await {
                tracing::warn!("add_channel_users failed for group {channel_id}: {err}");
                return Err(map_add_members_error(err));
            }
            let _ = this.update(cx, |this, cx| this.note_mutation(channel_id, cx));
            Ok(())
        })
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
            let _ = this.update(cx, |this, cx| this.note_mutation(channel_id, cx));
            Ok(())
        })
    }

    fn fetch(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if !self.loading.insert(channel_id) {
            return;
        }
        let api = self.api.clone();
        let started_at = self.mutation_count(channel_id);
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_users_uc(channel_id.get(), GROUP_MEMBER_FETCH_LIMIT)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&channel_id);
                match result {
                    Ok(resp) => {
                        if this.mutation_count(channel_id) != started_at {
                            this.fetch(channel_id, cx);
                            return;
                        }
                        let members = group_members_from_proto(&resp);
                        this.cache
                            .insert(channel_id, GroupBucket::from_members(members), None);
                        cx.emit(GroupMembersEvent::Changed { channel_id });
                        cx.notify();
                    }
                    Err(e) => {
                        tracing::error!("list_channel_users_uc failed for {channel_id}: {e}")
                    }
                }
            });
        })
        .detach();
    }

    fn mutation_count(&self, channel_id: ChannelId) -> u64 {
        self.mutations.get(&channel_id).copied().unwrap_or(0)
    }

    fn note_mutation(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        let next = self.mutation_count(channel_id).wrapping_add(1);
        self.mutations.insert(channel_id, next);
        self.cache.mark_stale(&channel_id);
        self.fetch(channel_id, cx);
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let changed_channel = match event {
            RealtimeEvent::UserChannelAdded(e) => {
                let Some(channel_id) = e.channel_desc.as_ref().map(|d| ChannelId(d.channel_id))
                else {
                    return;
                };
                apply_add_members(&mut self.cache, channel_id, &e.users).then_some(channel_id)
            }
            RealtimeEvent::UserChannelRemoved(e) => {
                let channel_id = ChannelId(e.channel_id);
                let ids: Vec<UserId> = e.user_ids.iter().map(|id| UserId(*id)).collect();
                apply_remove_members(&mut self.cache, channel_id, &ids).then_some(channel_id)
            }
            _ => None,
        };
        if let Some(channel_id) = changed_channel {
            cx.emit(GroupMembersEvent::Changed { channel_id });
            cx.notify();
        }
    }
}

fn map_add_members_error(err: anyhow::Error) -> AddGroupMembersError {
    match api_status_from_error(&err) {
        Some(status) if status.is_invalid_argument() => AddGroupMembersError::GroupFull,
        Some(status) => AddGroupMembersError::Api(status.code),
        None => AddGroupMembersError::Other(err.to_string()),
    }
}

fn group_members_from_proto(resp: &api::AllUsersAddChannelResponse) -> Vec<GroupMember> {
    resp.user_ids
        .iter()
        .enumerate()
        .filter_map(|(i, &uid)| {
            if uid == 0 {
                return None;
            }
            Some(GroupMember {
                user: User {
                    id: UserId(uid),
                    username: resp.usernames.get(i).cloned().unwrap_or_default(),
                    display_name: resp.display_names.get(i).cloned().unwrap_or_default(),
                    avatar_url: resp.avatars.get(i).cloned().unwrap_or_default(),
                    about_me: String::new(),
                    create_time_seconds: 0,
                    join_time_seconds: 0,
                },
                online: resp.onlines.get(i).copied().unwrap_or(false),
            })
        })
        .collect()
}

fn group_member_from_redis(user: &realtime::UserProfileRedis) -> Option<GroupMember> {
    if user.user_id == 0 {
        return None;
    }
    Some(GroupMember {
        user: User {
            id: UserId(user.user_id),
            username: user.username.clone(),
            display_name: user.display_name.clone(),
            avatar_url: user.avatar.clone(),
            about_me: String::new(),
            create_time_seconds: user.create_time_second,
            join_time_seconds: 0,
        },
        online: user.online,
    })
}

fn apply_add_members(
    by_channel: &mut KeyedCache<ChannelId, GroupBucket>,
    channel_id: ChannelId,
    users: &[realtime::UserProfileRedis],
) -> bool {
    let Some(bucket) = by_channel.get_mut(&channel_id) else {
        return false;
    };
    let mut changed = false;
    for user in users {
        let Some(member) = group_member_from_redis(user) else {
            continue;
        };
        changed |= bucket.upsert(member);
    }
    changed
}

fn apply_remove_members(
    by_channel: &mut KeyedCache<ChannelId, GroupBucket>,
    channel_id: ChannelId,
    user_ids: &[UserId],
) -> bool {
    let Some(bucket) = by_channel.get_mut(&channel_id) else {
        return false;
    };
    bucket.remove_ids(user_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_response(ids: &[i64]) -> api::AllUsersAddChannelResponse {
        api::AllUsersAddChannelResponse {
            channel_id: 1,
            user_ids: ids.to_vec(),
            limit: 500,
            usernames: ids.iter().map(|id| format!("user{id}")).collect(),
            display_names: ids.iter().map(|id| format!("User {id}")).collect(),
            avatars: ids.iter().map(|id| format!("{id}.png")).collect(),
            onlines: ids.iter().map(|id| id % 2 == 0).collect(),
        }
    }

    #[test]
    fn maps_parallel_arrays_to_members() {
        let members = group_members_from_proto(&proto_response(&[10, 11]));
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id(), UserId(10));
        assert_eq!(members[0].name(), "User 10");
        assert_eq!(members[0].avatar(), "10.png");
        assert!(members[0].online);
        assert!(!members[1].online);
    }

    #[test]
    fn maps_robustly_when_arrays_shorter_than_user_ids() {
        let mut resp = proto_response(&[10, 11, 12]);
        resp.usernames = vec!["only-one".into()];
        resp.display_names = vec![];
        resp.avatars = vec![];
        resp.onlines = vec![];
        let members = group_members_from_proto(&resp);
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].user.username, "only-one");
        assert_eq!(members[1].user.username, "");
        assert_eq!(members[0].name(), "only-one");
        assert_eq!(members[1].name(), "");
        assert!(!members[0].online);
    }

    #[test]
    fn add_error_maps_invalid_argument_to_a_full_group() {
        let err = mezon_client::ApiStatusError { code: 3 }.into();
        assert_eq!(map_add_members_error(err), AddGroupMembersError::GroupFull);
    }

    #[test]
    fn add_error_keeps_other_api_codes() {
        let err = mezon_client::ApiStatusError { code: 13 }.into();
        assert_eq!(map_add_members_error(err), AddGroupMembersError::Api(13));
    }

    #[test]
    fn add_error_falls_back_to_the_message() {
        let err = anyhow::anyhow!("socket closed");
        assert_eq!(
            map_add_members_error(err),
            AddGroupMembersError::Other("socket closed".to_string())
        );
    }

    #[test]
    fn skips_zero_user_ids() {
        let members = group_members_from_proto(&proto_response(&[0, 5]));
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id(), UserId(5));
    }

    fn assert_index_consistent(bucket: &GroupBucket) {
        assert_eq!(bucket.by_id.len(), bucket.members.len());
        for (i, m) in bucket.members.iter().enumerate() {
            assert_eq!(bucket.by_id.get(&m.user.id), Some(&i));
        }
    }

    fn cache_with(
        channel_id: ChannelId,
        bucket: GroupBucket,
    ) -> KeyedCache<ChannelId, GroupBucket> {
        let mut cache = KeyedCache::new(None);
        cache.insert(channel_id, bucket, None);
        cache
    }

    #[test]
    fn add_members_applies_to_loaded_group() {
        let mut by_channel = cache_with(ChannelId(1), GroupBucket::default());
        let users = vec![realtime::UserProfileRedis {
            user_id: 7,
            username: "bob".into(),
            ..Default::default()
        }];
        assert!(apply_add_members(&mut by_channel, ChannelId(1), &users));
        let bucket = by_channel.get(&ChannelId(1)).unwrap();
        assert_eq!(bucket.members.len(), 1);
        assert_eq!(bucket.members[0].id(), UserId(7));
        assert_eq!(bucket.get(UserId(7)).map(GroupMember::id), Some(UserId(7)));
        assert_index_consistent(bucket);
    }

    #[test]
    fn add_members_ignored_for_unloaded_group() {
        let mut by_channel: KeyedCache<ChannelId, GroupBucket> = KeyedCache::new(None);
        let users = vec![realtime::UserProfileRedis {
            user_id: 7,
            ..Default::default()
        }];
        assert!(!apply_add_members(&mut by_channel, ChannelId(1), &users));
        assert!(by_channel.get(&ChannelId(1)).is_none());
    }

    #[test]
    fn add_members_dedupes_existing_user() {
        let mut by_channel = cache_with(ChannelId(1), GroupBucket::default());
        let users = vec![realtime::UserProfileRedis {
            user_id: 7,
            username: "bob".into(),
            ..Default::default()
        }];
        apply_add_members(&mut by_channel, ChannelId(1), &users);
        apply_add_members(&mut by_channel, ChannelId(1), &users);
        assert_eq!(by_channel.get(&ChannelId(1)).unwrap().members.len(), 1);
        assert_index_consistent(by_channel.get(&ChannelId(1)).unwrap());
    }

    #[test]
    fn remove_members_drops_users() {
        let mut by_channel = cache_with(
            ChannelId(1),
            GroupBucket::from_members(group_members_from_proto(&proto_response(&[1, 2, 3]))),
        );
        assert!(apply_remove_members(
            &mut by_channel,
            ChannelId(1),
            &[UserId(2)]
        ));
        let bucket = by_channel.get(&ChannelId(1)).unwrap();
        let ids: Vec<UserId> = bucket.members.iter().map(GroupMember::id).collect();
        assert_eq!(ids, vec![UserId(1), UserId(3)]);
        assert!(bucket.get(UserId(2)).is_none());
        assert_index_consistent(bucket);
    }

    #[test]
    fn removing_a_user_who_already_left_reports_no_change() {
        let mut by_channel = cache_with(
            ChannelId(1),
            GroupBucket::from_members(group_members_from_proto(&proto_response(&[10, 11]))),
        );

        assert!(apply_remove_members(
            &mut by_channel,
            ChannelId(1),
            &[UserId(10)]
        ));
        assert!(
            !apply_remove_members(&mut by_channel, ChannelId(1), &[UserId(10)]),
            "a no-op removal must not claim a change, or every stale event rebuilds the list"
        );
        assert!(!apply_remove_members(
            &mut by_channel,
            ChannelId(1),
            &[UserId(99)]
        ));
    }

    #[test]
    fn re_adding_a_user_who_is_already_in_the_bucket_reports_no_change() {
        let mut by_channel = cache_with(ChannelId(1), GroupBucket::default());
        let users = vec![realtime::UserProfileRedis {
            user_id: 7,
            username: "bob".into(),
            ..Default::default()
        }];

        assert!(apply_add_members(&mut by_channel, ChannelId(1), &users));
        assert!(
            !apply_add_members(&mut by_channel, ChannelId(1), &users),
            "a redelivered add must not claim a change, or every stale event rebuilds the list"
        );
        assert!(
            !apply_add_members(&mut by_channel, ChannelId(1), &[]),
            "an add with no users is not a change"
        );
        assert!(
            apply_add_members(
                &mut by_channel,
                ChannelId(1),
                &[realtime::UserProfileRedis {
                    user_id: 7,
                    username: "bobby".into(),
                    ..Default::default()
                }]
            ),
            "a profile that actually moved is a change"
        );
    }

    #[test]
    fn reconnect_mark_all_stale_keeps_members_then_refetch_restores_freshness() {
        let mut by_channel = cache_with(
            ChannelId(1),
            GroupBucket::from_members(group_members_from_proto(&proto_response(&[1, 2]))),
        );
        assert!(by_channel.is_fresh(&ChannelId(1), crate::CACHE_TTL));

        by_channel.mark_all_stale();
        assert!(!by_channel.is_fresh(&ChannelId(1), crate::CACHE_TTL));
        assert_eq!(by_channel.get(&ChannelId(1)).unwrap().members.len(), 2);

        by_channel.insert(
            ChannelId(1),
            GroupBucket::from_members(group_members_from_proto(&proto_response(&[1]))),
            None,
        );
        assert!(by_channel.is_fresh(&ChannelId(1), crate::CACHE_TTL));
    }
}
