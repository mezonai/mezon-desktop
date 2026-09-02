use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// How long to wait before asking `IsBanned` again after a failed attempt. Without it the chat
/// render loop would re-arm the request every frame for as long as the call keeps failing.
const SELF_BAN_RETRY_BACKOFF: Duration = Duration::from_secs(30);

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
    /// The signed-in user's own ban per channel: `Some(instant)` when it lifts by itself,
    /// `None` when the server gave no expiry. Absent means "asked, not banned".
    self_ban: HashMap<ChannelId, Option<Instant>>,
    self_ban_asked: HashSet<ChannelId>,
    self_ban_failed_at: HashMap<ChannelId, Instant>,
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
            self_ban: HashMap::new(),
            self_ban_asked: HashSet::new(),
            self_ban_failed_at: HashMap::new(),
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
        self.self_ban.clear();
        self.self_ban_asked.clear();
        self.self_ban_failed_at.clear();
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
                        .update(cx, |this, _| {
                            this.cache.mark_all_stale();
                            this.forget_self_bans();
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
                // The cached `IsBanned` answer may have been about this very user, so ask
                // again — but keep the old answer until the new one lands. Clearing it here
                // would flash the composer back for one round trip every time a moderator bans
                // somebody else in a channel this user is banned from.
                this.self_ban_asked.remove(&channel_id);
                this.self_ban_failed_at.remove(&channel_id);
                for user_id in &event.user_ids {
                    this.apply_local(channel_id, UserId(*user_id), banned, cx);
                }
            });
            dispatch.on_lagged(&entity, |this, _| {
                this.cache.mark_all_stale();
                this.forget_self_bans();
            });
        });
    }

    /// Ask the server once per channel whether *this* user is banned there. The answer drives
    /// the composer's banned notice, so it has to come from `IsBanned` rather than the moderator
    /// list, which only says who is banned and never for how long.
    pub fn ensure_self_ban(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if channel_id.is_zero() {
            return;
        }
        if self
            .self_ban_failed_at
            .get(&channel_id)
            .is_some_and(|at| at.elapsed() < SELF_BAN_RETRY_BACKOFF)
        {
            return;
        }
        // A deadline that has passed is no longer an answer: drop it so the ask below runs and
        // the server confirms the ban really is over.
        if self
            .self_ban
            .get(&channel_id)
            .is_some_and(|expires| expires.is_some_and(|at| at <= Instant::now()))
        {
            self.self_ban.remove(&channel_id);
            self.self_ban_asked.remove(&channel_id);
        }
        if !self.self_ban_asked.insert(channel_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let Ok(response) = api.is_banned(channel_id.get()).await else {
                // Stay unanswered rather than guess at a banned notice, but behind a backoff:
                // the caller is a render, so an unguarded retry would fire once per frame.
                let _ = this.update(cx, |this, _| {
                    this.self_ban_asked.remove(&channel_id);
                    this.self_ban_failed_at.insert(channel_id, Instant::now());
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if response.is_banned {
                    let expires = (response.expired_ban_time > 0).then(|| {
                        Instant::now()
                            + std::time::Duration::from_secs(response.expired_ban_time as u64)
                    });
                    this.self_ban.insert(channel_id, expires);
                } else {
                    this.self_ban.remove(&channel_id);
                }
                this.self_ban_failed_at.remove(&channel_id);
                cx.notify();
            });
        })
        .detach();
    }

    /// `Some(None)` = banned with no expiry, `Some(Some(secs))` = seconds left, `None` = not
    /// banned (or not answered yet).
    pub fn self_ban_remaining(&self, channel_id: ChannelId) -> Option<Option<i64>> {
        let expires = self.self_ban.get(&channel_id)?;
        let Some(expires) = expires else {
            return Some(None);
        };
        // A timed ban lapses in the server's cache without any event, so an elapsed deadline
        // means the ban is over — reporting zero here would leave the composer replaced by an
        // "Expired" notice for the rest of the session.
        let left = expires.saturating_duration_since(Instant::now()).as_secs();
        (left > 0).then_some(Some(left as i64))
    }

    /// Forget every `IsBanned` answer, so the next render asks again. Used when the socket has
    /// been away long enough that a ban could have been handed out or lifted unseen.
    fn forget_self_bans(&mut self) {
        self.self_ban.clear();
        self.self_ban_asked.clear();
        self.self_ban_failed_at.clear();
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

    /// The map is what `self_ban_remaining` reads; building the whole store would need a live
    /// `AppApi`, and the expiry rule is the part worth pinning down.
    fn remaining_for(entry: Option<Instant>) -> Option<Option<i64>> {
        let mut map: HashMap<ChannelId, Option<Instant>> = HashMap::new();
        map.insert(ChannelId(1), entry);
        let expires = map.get(&ChannelId(1))?;
        let Some(expires) = expires else {
            return Some(None);
        };
        let left = expires.saturating_duration_since(Instant::now()).as_secs();
        (left > 0).then_some(Some(left as i64))
    }

    #[test]
    fn a_lapsed_ban_reads_as_no_ban_rather_than_zero_seconds() {
        // No expiry: banned until someone lifts it.
        assert_eq!(remaining_for(None), Some(None));
        // Still running.
        let later = Instant::now() + Duration::from_secs(600);
        assert!(matches!(remaining_for(Some(later)), Some(Some(secs)) if secs > 0));
        // Already lapsed — must not report a banned-with-zero-left state, which would leave the
        // composer replaced by an "Expired" notice for the rest of the session.
        let past = Instant::now() - Duration::from_secs(1);
        assert_eq!(remaining_for(Some(past)), None);
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
