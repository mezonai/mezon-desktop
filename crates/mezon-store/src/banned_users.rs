use std::collections::HashSet;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus};

use mezon_client::RealtimeEvent;

use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};
use crate::{CACHE_TTL, KeyedCache};

const MAX_CACHED_CHANNELS: usize = 32;

pub const BAN_FOR_15_MINUTES_SEC: i32 = 15 * 60;
pub const BAN_FOR_1_HOUR_SEC: i32 = 60 * 60;
pub const BAN_FOR_3_HOURS_SEC: i32 = 3 * 60 * 60;
pub const BAN_FOR_8_HOURS_SEC: i32 = 8 * 60 * 60;
pub const BAN_FOR_24_HOURS_SEC: i32 = 24 * 60 * 60;
pub const BAN_FOREVER: i32 = 0;

const BAN_ACTION_BANNED: i32 = 1;

#[derive(Debug, Clone)]
pub struct BannedEntry {
    pub channel_id: i64,
    pub banned_id: i64,
    pub banner_id: i64,
    pub ban_time: i32,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum BannedUsersEvent {
    Changed { channel_id: ChannelId },
    BanFailed,
    UnbanFailed,
}

pub struct BannedUsersStore {
    cache: KeyedCache<ChannelId, HashSet<UserId>>,
    loading: HashSet<ChannelId>,
    mutation_epoch: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalBannedUsersStore(Entity<BannedUsersStore>);
impl Global for GlobalBannedUsersStore {}

impl EventEmitter<BannedUsersEvent> for BannedUsersStore {}

impl BannedUsersStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalBannedUsersStore(entity.clone()));
        entity
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            cache: KeyedCache::new(Some(MAX_CACHED_CHANNELS)),
            loading: HashSet::new(),
            mutation_epoch: 0,
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBannedUsersStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalBannedUsersStore>()
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
                    if this
                        .update(cx, |this, _| this.cache.mark_all_stale())
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

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::BanUser, &entity, |this, event, cx| {
                let RealtimeEvent::BanUser(event) = event else {
                    return;
                };
                let channel_id = ChannelId(event.channel_id);
                if channel_id.get() == 0 {
                    return;
                }
                let banned = event.action == BAN_ACTION_BANNED;
                for user_id in &event.user_ids {
                    this.apply_local(channel_id, UserId(*user_id), banned, cx);
                }
            });
            dispatch.on_lagged(&entity, |this, _| this.cache.mark_all_stale());
        });
    }

    pub fn is_banned(&self, channel_id: ChannelId, user_id: UserId) -> bool {
        self.cache
            .get(&channel_id)
            .is_some_and(|banned| banned.contains(&user_id))
    }

    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if clan_id.is_zero() || channel_id.get() == 0 {
            return;
        }
        self.cache.touch(&channel_id);
        if self.cache.is_fresh(&channel_id, CACHE_TTL) {
            return;
        }
        self.fetch(clan_id, channel_id, cx);
    }

    fn fetch(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        if !self.loading.insert(channel_id) {
            return;
        }
        let api = self.api.clone();
        let epoch = self.mutation_epoch;
        cx.spawn(async move |this, cx| {
            let result = api.list_banned_users(clan_id.get(), channel_id.get()).await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&channel_id);
                match result {
                    Ok(list) => {
                        if this.mutation_epoch != epoch {
                            this.cache.mark_stale(&channel_id);
                            return;
                        }
                        let banned = banned_ids_for_channel(&list.banned_users, channel_id);
                        this.cache.insert(channel_id, banned, None);
                        cx.emit(BannedUsersEvent::Changed { channel_id });
                        cx.notify();
                    }
                    Err(error) => {
                        tracing::error!("list_banned_users failed for {channel_id}: {error}");
                    }
                }
            });
        })
        .detach();
    }

    pub fn fetch_raw(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Vec<BannedEntry>>> {
        let api = self.api.clone();
        cx.background_spawn(async move {
            let list = api
                .list_banned_users(clan_id.get(), channel_id.get())
                .await?;
            Ok(list
                .banned_users
                .into_iter()
                .map(|user| BannedEntry {
                    channel_id: user.channel_id,
                    banned_id: user.banned_id,
                    banner_id: user.banner_id,
                    ban_time: user.ban_time,
                    reason: user.reason,
                })
                .collect())
        })
    }

    pub fn ban(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        user_id: UserId,
        ban_time: i32,
        cx: &mut Context<Self>,
    ) {
        if clan_id.is_zero() || channel_id.get() == 0 {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .ban_clan_users(
                    clan_id.get(),
                    channel_id.get(),
                    vec![user_id.get().to_string()],
                    ban_time,
                )
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.apply_local(channel_id, user_id, true, cx),
                Err(error) => {
                    tracing::error!("ban_clan_users failed for {user_id} in {channel_id}: {error}");
                    cx.emit(BannedUsersEvent::BanFailed);
                }
            });
        })
        .detach();
    }

    pub fn unban(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        user_id: UserId,
        cx: &mut Context<Self>,
    ) {
        if clan_id.is_zero() || channel_id.get() == 0 {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .unban_clan_users(
                    clan_id.get(),
                    channel_id.get(),
                    vec![user_id.get().to_string()],
                )
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.apply_local(channel_id, user_id, false, cx),
                Err(error) => {
                    tracing::error!(
                        "unban_clan_users failed for {user_id} in {channel_id}: {error}"
                    );
                    cx.emit(BannedUsersEvent::UnbanFailed);
                }
            });
        })
        .detach();
    }

    fn apply_local(
        &mut self,
        channel_id: ChannelId,
        user_id: UserId,
        banned: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.cache.contains(&channel_id) {
            self.cache.insert(channel_id, HashSet::new(), None);
            self.cache.mark_stale(&channel_id);
        }
        let Some(bucket) = self.cache.get_mut(&channel_id) else {
            return;
        };
        if apply_ban_state(bucket, user_id, banned) {
            self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
            cx.emit(BannedUsersEvent::Changed { channel_id });
            cx.notify();
        }
    }
}

fn banned_ids_for_channel(
    banned_users: &[mezon_proto::api::BannedUser],
    channel_id: ChannelId,
) -> HashSet<UserId> {
    banned_users
        .iter()
        .filter(|user| user.channel_id == channel_id.get() && user.banned_id != 0)
        .map(|user| UserId(user.banned_id))
        .collect()
}

fn apply_ban_state(bucket: &mut HashSet<UserId>, user_id: UserId, banned: bool) -> bool {
    if banned {
        bucket.insert(user_id)
    } else {
        bucket.remove(&user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banned_user(channel_id: i64, banned_id: i64) -> mezon_proto::api::BannedUser {
        mezon_proto::api::BannedUser {
            channel_id,
            banned_id,
            ..Default::default()
        }
    }

    #[test]
    fn banned_ids_keep_only_the_requested_channel_and_drop_empty_ids() {
        let list = vec![
            banned_user(10, 1),
            banned_user(11, 2),
            banned_user(10, 0),
            banned_user(10, 3),
        ];
        let ids = banned_ids_for_channel(&list, ChannelId(10));
        assert_eq!(ids, HashSet::from([UserId(1), UserId(3)]));
    }

    #[test]
    fn ban_state_reports_change_only_on_a_real_transition() {
        let mut bucket = HashSet::new();
        assert!(apply_ban_state(&mut bucket, UserId(1), true));
        assert!(!apply_ban_state(&mut bucket, UserId(1), true));
        assert!(bucket.contains(&UserId(1)));
        assert!(apply_ban_state(&mut bucket, UserId(1), false));
        assert!(!apply_ban_state(&mut bucket, UserId(1), false));
        assert!(bucket.is_empty());
    }
}
