use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent, api_status_from_error};
use mezon_proto::api;

use crate::Freshness;
use crate::account::AccountStore;
use crate::friend::{FriendState, FriendStore};
use crate::ids::UserId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MEMO_CAPTION_MAX_RUNES: usize = 100;
const MEMO_REPLY_MAX_RUNES: usize = 2000;
const MEMO_EXPIRE_MIN_WAKE: Duration = Duration::from_millis(250);
const LIST_MEMOS_FETCH_RETRY_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memo {
    pub creator_id: UserId,
    pub id: i64,
    pub image_url: String,
    pub caption: String,
    pub create_time_second: i64,
    pub expire_at_second: i64,
    pub seen_by_me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoCreatorGroup {
    pub creator_id: UserId,
    pub memos: Vec<Memo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoSlide {
    pub creator_id: UserId,
    pub memo_index: usize,
}

pub fn build_memo_playlist(groups: &[MemoCreatorGroup]) -> Vec<MemoSlide> {
    groups
        .iter()
        .flat_map(|group| {
            (0..group.memos.len()).map(move |memo_index| MemoSlide {
                creator_id: group.creator_id,
                memo_index,
            })
        })
        .collect()
}

pub fn playlist_position(
    playlist: &[MemoSlide],
    creator_id: UserId,
    memo_index: usize,
) -> Option<usize> {
    playlist
        .iter()
        .position(|slide| slide.creator_id == creator_id && slide.memo_index == memo_index)
}

pub fn group_has_unread(group: &MemoCreatorGroup) -> bool {
    group.memos.iter().any(|memo| !memo.seen_by_me)
}

pub fn first_unread_memo_index(group: &MemoCreatorGroup) -> usize {
    group
        .memos
        .iter()
        .position(|memo| !memo.seen_by_me)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum MemoEvent {
    Changed,
    Created,
    CreateFailed,
    CreateCapExceeded,
    DeleteFailed,
    ReplyFailed,
}

fn memo_from_api(m: api::Memo, seen_by_me: bool) -> Memo {
    Memo {
        creator_id: UserId(m.creator_id),
        id: m.id,
        image_url: m.image_url,
        caption: m.caption,
        create_time_second: m.create_time_second,
        expire_at_second: m.expire_at_second,
        seen_by_me,
    }
}

fn group_from_api(g: api::MemoCreatorGroup) -> MemoCreatorGroup {
    MemoCreatorGroup {
        creator_id: UserId(g.creator_id),
        memos: g
            .memos
            .into_iter()
            .filter_map(|item| item.memo.map(|memo| memo_from_api(memo, item.seen_by_me)))
            .collect(),
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn viewer_id(cx: &gpui::App) -> Option<UserId> {
    AccountStore::global(cx)
        .read(cx)
        .account
        .as_ref()
        .map(|account| UserId(account.user_id))
}

fn memo_creator_visible(creator_id: UserId, cx: &gpui::App) -> bool {
    if viewer_id(cx).is_some_and(|viewer| viewer == creator_id) {
        return true;
    }
    FriendStore::global(cx)
        .read(cx)
        .friends()
        .iter()
        .any(|friend| friend.id == creator_id && friend.state == FriendState::Friend)
}

fn sort_groups_for_viewer(groups: &mut [MemoCreatorGroup], viewer_id: UserId) {
    groups.sort_by(|left, right| {
        match (left.creator_id == viewer_id, right.creator_id == viewer_id) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.creator_id.0.cmp(&right.creator_id.0),
        }
    });
}

fn filter_active_group(mut group: MemoCreatorGroup) -> Option<MemoCreatorGroup> {
    let now = now_unix();
    group.memos.retain(|memo| memo.expire_at_second > now);
    if group.memos.is_empty() {
        None
    } else {
        Some(group)
    }
}

fn earliest_expire_at(groups: &[MemoCreatorGroup]) -> Option<i64> {
    groups
        .iter()
        .flat_map(|group| group.memos.iter())
        .map(|memo| memo.expire_at_second)
        .min()
}

fn purge_expired_groups(groups: &mut Vec<MemoCreatorGroup>, now: i64) -> bool {
    let before_groups = groups.len();
    let before_memos: usize = groups.iter().map(|group| group.memos.len()).sum();
    groups.retain_mut(|group| {
        group.memos.retain(|memo| memo.expire_at_second > now);
        !group.memos.is_empty()
    });
    let after_memos: usize = groups.iter().map(|group| group.memos.len()).sum();
    before_groups != groups.len() || before_memos != after_memos
}

fn create_from_path_allowed(creating: bool, connected: bool, caption_runes: usize) -> bool {
    !creating && connected && caption_runes <= MEMO_CAPTION_MAX_RUNES
}

fn list_fetch_backoff_active(fetch_failed_at: Option<Instant>, now: Instant) -> bool {
    fetch_failed_at
        .is_some_and(|at| now.saturating_duration_since(at) < LIST_MEMOS_FETCH_RETRY_BACKOFF)
}

struct GlobalMemoStore(Entity<MemoStore>);
impl Global for GlobalMemoStore {}

pub struct MemoStore {
    groups: Vec<MemoCreatorGroup>,
    loading: bool,
    creating: bool,
    freshness: Freshness,
    reset_generation: u64,
    expire_generation: u64,
    fetch_failed_at: Option<Instant>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
    _expire_task: Option<Task<()>>,
}

impl EventEmitter<MemoEvent> for MemoStore {}

impl MemoStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalMemoStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalMemoStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMemoStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::MemoCreated, &entity, |this, event, cx| {
                this.handle_memo_created(event, cx);
            });
            dispatch.on(RealtimeKind::MemoDeleted, &entity, |this, event, cx| {
                this.handle_memo_deleted(event, cx);
            });
            dispatch.on(RealtimeKind::RemoveFriend, &entity, |this, event, cx| {
                this.handle_remove_friend(event, cx);
            });
            dispatch.on(RealtimeKind::BlockFriend, &entity, |this, event, cx| {
                this.handle_block_friend(event, cx);
            });
            dispatch.on(RealtimeKind::AddFriend, &entity, |this, event, cx| {
                this.handle_add_friend(event, cx);
            });
            dispatch.on(RealtimeKind::UnblockFriend, &entity, |this, event, cx| {
                this.handle_unblock_friend(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| {
                this.fetch_failed_at = None;
                this.freshness.mark_stale();
                this.fetch(cx);
            });
        });
        Self {
            groups: Vec::new(),
            loading: false,
            creating: false,
            freshness: Freshness::new(),
            reset_generation: 0,
            expire_generation: 0,
            fetch_failed_at: None,
            api,
            _conn_watch: conn_watch,
            _expire_task: None,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.expire_generation = self.expire_generation.wrapping_add(1);
        self._expire_task = None;
        self.fetch_failed_at = None;
        self.groups.clear();
        self.loading = false;
        self.creating = false;
        self.freshness.mark_stale();
        cx.emit(MemoEvent::Changed);
        cx.notify();
    }

    pub fn groups(&self) -> &[MemoCreatorGroup] {
        &self.groups
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_creating(&self) -> bool {
        self.creating
    }

    pub fn group_for_creator(&self, creator_id: UserId) -> Option<&MemoCreatorGroup> {
        self.groups
            .iter()
            .find(|group| group.creator_id == creator_id)
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_expire_schedule(cx);
        if self.loading
            || self.freshness.is_fresh(crate::CACHE_TTL)
            || list_fetch_backoff_active(self.fetch_failed_at, Instant::now())
        {
            return;
        }
        self.fetch(cx);
    }

    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if !self.is_connected() {
            return;
        }
        self.loading = true;
        let generation = self.reset_generation;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_memos().await;
            if this
                .update(cx, |this, cx| {
                    if this.reset_generation != generation {
                        return;
                    }
                    this.loading = false;
                    match result {
                        Ok(response) => {
                            this.fetch_failed_at = None;
                            this.apply_list_response(response, cx);
                            this.freshness.mark_fetched();
                            this.sync_expire_schedule(cx);
                            cx.emit(MemoEvent::Changed);
                            cx.notify();
                        }
                        Err(error) => {
                            tracing::warn!("list memos failed: {error:#}");
                            this.fetch_failed_at = Some(Instant::now());
                        }
                    }
                })
                .is_err()
            {
                return;
            }
        })
        .detach();
    }

    pub fn create_from_path(
        &mut self,
        path: impl AsRef<Path>,
        caption: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let caption_runes = caption.chars().count();
        if !create_from_path_allowed(self.creating, self.is_connected(), caption_runes) {
            return false;
        }
        self.creating = true;
        cx.notify();
        let path = path.as_ref().to_path_buf();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                let image_url = api.upload_memo_image(&path).await?;
                api.create_memo(&image_url, &caption).await
            }
            .await;
            if this
                .update(cx, |this, cx| {
                    this.creating = false;
                    match result {
                        Ok(memo) => {
                            this.upsert_memo(memo_from_api(memo, true));
                            if let Some(viewer_id) = viewer_id(cx) {
                                sort_groups_for_viewer(&mut this.groups, viewer_id);
                            }
                            this.freshness.mark_fetched();
                            this.sync_expire_schedule(cx);
                            cx.emit(MemoEvent::Created);
                            cx.emit(MemoEvent::Changed);
                        }
                        Err(error) => {
                            tracing::warn!("create memo failed: {error:#}");
                            if api_status_from_error(&error)
                                .is_some_and(|status| status.is_out_of_range())
                            {
                                cx.emit(MemoEvent::CreateCapExceeded);
                            } else {
                                cx.emit(MemoEvent::CreateFailed);
                            }
                        }
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
        })
        .detach();
        true
    }

    pub fn delete_memo(
        &mut self,
        creator_id: UserId,
        memo_id: i64,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.delete_memo(creator_id.0, memo_id).await;
            this.update(cx, |this, cx| match &result {
                Ok(()) => {
                    this.remove_memo(creator_id, memo_id);
                    this.sync_expire_schedule(cx);
                    cx.emit(MemoEvent::Changed);
                    cx.notify();
                }
                Err(error) => {
                    tracing::warn!("delete memo failed: {error:#}");
                    cx.emit(MemoEvent::DeleteFailed);
                    cx.notify();
                }
            })?;
            result
        })
    }

    pub fn reply_memo(
        &mut self,
        creator_id: UserId,
        memo_id: i64,
        text: String,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<i64>> {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() || trimmed.chars().count() > MEMO_REPLY_MAX_RUNES {
            return cx.spawn(async |_, _| Err(anyhow::anyhow!("invalid memo reply text")));
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.reply_memo(creator_id.0, memo_id, &trimmed).await;
            if result.is_err() {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(MemoEvent::ReplyFailed);
                    cx.notify();
                });
            }
            result
        })
    }

    pub fn mark_seen(&mut self, creator_id: UserId, memo_id: i64, cx: &mut Context<Self>) {
        if self
            .groups
            .iter()
            .find(|group| group.creator_id == creator_id)
            .and_then(|group| group.memos.iter().find(|memo| memo.id == memo_id))
            .is_some_and(|memo| memo.seen_by_me)
        {
            return;
        }
        self.set_seen_local(creator_id, memo_id);
        cx.emit(MemoEvent::Changed);
        cx.notify();
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            if api.mark_memo_seen(creator_id.0, memo_id).await.is_err() {
                let _ = this.update(cx, |this, cx| {
                    this.freshness.mark_stale();
                    cx.emit(MemoEvent::Changed);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn is_connected(&self) -> bool {
        *self.api.status().borrow() == ConnectionStatus::Connected
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
                            this.fetch_failed_at = None;
                            this.freshness.mark_stale();
                            this.fetch(cx);
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

    fn handle_memo_created(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MemoCreated(payload) = event else {
            return;
        };
        let Some(memo) = payload.memo.as_ref() else {
            return;
        };
        let creator_id = UserId(memo.creator_id);
        if !memo_creator_visible(creator_id, cx) {
            return;
        }
        let seen_by_me = viewer_id(cx).is_some_and(|viewer| viewer == creator_id);
        let domain = memo_from_api(memo.clone(), seen_by_me);
        self.upsert_memo(domain);
        if let Some(viewer_id) = viewer_id(cx) {
            sort_groups_for_viewer(&mut self.groups, viewer_id);
        }
        self.freshness.mark_fetched();
        self.sync_expire_schedule(cx);
        cx.emit(MemoEvent::Changed);
        cx.notify();
    }

    fn handle_memo_deleted(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MemoDeleted(payload) = event else {
            return;
        };
        self.remove_memo(UserId(payload.creator_id), payload.memo_id);
        self.sync_expire_schedule(cx);
        cx.emit(MemoEvent::Changed);
        cx.notify();
    }

    fn sync_expire_schedule(&mut self, cx: &mut Context<Self>) {
        let now = now_unix();
        if purge_expired_groups(&mut self.groups, now) {
            cx.emit(MemoEvent::Changed);
            cx.notify();
        }
        self.schedule_expire_purge(cx);
    }

    fn schedule_expire_purge(&mut self, cx: &mut Context<Self>) {
        self.expire_generation = self.expire_generation.wrapping_add(1);
        let generation = self.expire_generation;
        let Some(next_expire) = earliest_expire_at(&self.groups) else {
            self._expire_task = None;
            return;
        };
        let now = now_unix();
        let delay =
            Duration::from_secs((next_expire - now).max(0) as u64).max(MEMO_EXPIRE_MIN_WAKE);
        self._expire_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.expire_generation != generation {
                    return;
                }
                let now = now_unix();
                if purge_expired_groups(&mut this.groups, now) {
                    cx.emit(MemoEvent::Changed);
                    cx.notify();
                }
                this.schedule_expire_purge(cx);
            });
        }));
    }

    fn handle_remove_friend(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::RemoveFriend(payload) = event else {
            return;
        };
        self.drop_creator(UserId(payload.user_id), cx);
    }

    fn handle_block_friend(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::BlockFriend(payload) = event else {
            return;
        };
        self.drop_creator(UserId(payload.user_id), cx);
    }

    fn handle_add_friend(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::AddFriend(payload) = event else {
            return;
        };
        if !memo_creator_visible(UserId(payload.user_id), cx) {
            return;
        }
        self.freshness.mark_stale();
        self.fetch(cx);
    }

    fn handle_unblock_friend(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::UnblockFriend(payload) = event else {
            return;
        };
        if !memo_creator_visible(UserId(payload.user_id), cx) {
            return;
        }
        self.freshness.mark_stale();
        self.fetch(cx);
    }

    fn drop_creator(&mut self, creator_id: UserId, cx: &mut Context<Self>) {
        let before = self.groups.len();
        self.groups.retain(|group| group.creator_id != creator_id);
        if self.groups.len() != before {
            cx.emit(MemoEvent::Changed);
            cx.notify();
        }
    }

    fn upsert_memo(&mut self, memo: Memo) {
        if memo.expire_at_second <= now_unix() {
            return;
        }
        if let Some(group) = self
            .groups
            .iter_mut()
            .find(|g| g.creator_id == memo.creator_id)
        {
            if let Some(existing) = group.memos.iter_mut().find(|m| m.id == memo.id) {
                let seen_by_me = existing.seen_by_me || memo.seen_by_me;
                *existing = memo;
                existing.seen_by_me = seen_by_me;
            } else {
                group.memos.push(memo);
                group.memos.sort_by_key(|memo| std::cmp::Reverse(memo.id));
            }
            return;
        }
        if let Some(group) = filter_active_group(MemoCreatorGroup {
            creator_id: memo.creator_id,
            memos: vec![memo],
        }) {
            self.groups.push(group);
        }
    }

    fn remove_memo(&mut self, creator_id: UserId, memo_id: i64) {
        remove_memo_from_groups(&mut self.groups, creator_id, memo_id);
    }

    fn set_seen_local(&mut self, creator_id: UserId, memo_id: i64) {
        let Some(group) = self
            .groups
            .iter_mut()
            .find(|group| group.creator_id == creator_id)
        else {
            return;
        };
        if let Some(memo) = group.memos.iter_mut().find(|memo| memo.id == memo_id) {
            memo.seen_by_me = true;
        }
    }
}

fn remove_memo_from_groups(
    groups: &mut Vec<MemoCreatorGroup>,
    creator_id: UserId,
    memo_id: i64,
) -> bool {
    let mut removed = false;
    groups.retain_mut(|group| {
        if group.creator_id != creator_id {
            return true;
        }
        let before = group.memos.len();
        group.memos.retain(|memo| memo.id != memo_id);
        if group.memos.len() != before {
            removed = true;
        }
        !group.memos.is_empty()
    });
    removed
}

impl MemoStore {
    pub fn apply_list_response(&mut self, response: api::ListMemosResponse, cx: &gpui::App) {
        self.groups = response
            .groups
            .into_iter()
            .map(group_from_api)
            .filter_map(filter_active_group)
            .collect();
        if let Some(viewer_id) = viewer_id(cx) {
            sort_groups_for_viewer(&mut self.groups, viewer_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memo_creator_allowed(
        creator_id: UserId,
        viewer_id: Option<UserId>,
        friend_ids: &[UserId],
    ) -> bool {
        if viewer_id == Some(creator_id) {
            return true;
        }
        friend_ids.contains(&creator_id)
    }

    #[test]
    fn build_memo_playlist_orders_groups_and_memos() {
        let groups = vec![
            MemoCreatorGroup {
                creator_id: UserId(1),
                memos: vec![
                    Memo {
                        creator_id: UserId(1),
                        id: 10,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: 0,
                        expire_at_second: i64::MAX,
                        seen_by_me: false,
                    },
                    Memo {
                        creator_id: UserId(1),
                        id: 9,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: 0,
                        expire_at_second: i64::MAX,
                        seen_by_me: false,
                    },
                ],
            },
            MemoCreatorGroup {
                creator_id: UserId(2),
                memos: vec![Memo {
                    creator_id: UserId(2),
                    id: 20,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: i64::MAX,
                    seen_by_me: false,
                }],
            },
        ];
        let playlist = build_memo_playlist(&groups);
        assert_eq!(playlist.len(), 3);
        assert_eq!(playlist[0].creator_id, UserId(1));
        assert_eq!(playlist[0].memo_index, 0);
        assert_eq!(playlist[2].creator_id, UserId(2));
        assert_eq!(playlist_position(&playlist, UserId(2), 0), Some(2));
    }

    #[test]
    fn memo_creator_allowed_self_and_friends_only() {
        let me = UserId(10);
        let friend = UserId(20);
        let stranger = UserId(30);
        assert!(memo_creator_allowed(me, Some(me), &[friend]));
        assert!(memo_creator_allowed(friend, Some(me), &[friend]));
        assert!(!memo_creator_allowed(stranger, Some(me), &[friend]));
    }

    #[test]
    fn remove_memo_from_groups_drops_empty_creator() {
        let mut groups = vec![MemoCreatorGroup {
            creator_id: UserId(1),
            memos: vec![Memo {
                creator_id: UserId(1),
                id: 99,
                image_url: String::new(),
                caption: String::new(),
                create_time_second: 0,
                expire_at_second: i64::MAX,
                seen_by_me: false,
            }],
        }];
        assert!(remove_memo_from_groups(&mut groups, UserId(1), 99));
        assert!(groups.is_empty());
    }

    #[test]
    fn sort_groups_for_viewer_puts_self_first() {
        let mut groups = vec![
            MemoCreatorGroup {
                creator_id: UserId(2),
                memos: vec![],
            },
            MemoCreatorGroup {
                creator_id: UserId(5),
                memos: vec![],
            },
        ];
        sort_groups_for_viewer(&mut groups, UserId(5));
        assert_eq!(groups[0].creator_id, UserId(5));
        assert_eq!(groups[1].creator_id, UserId(2));
    }

    #[test]
    fn handle_memo_deleted_removes_from_groups() {
        let mut groups = vec![MemoCreatorGroup {
            creator_id: UserId(7),
            memos: vec![
                Memo {
                    creator_id: UserId(7),
                    id: 10,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: i64::MAX,
                    seen_by_me: false,
                },
                Memo {
                    creator_id: UserId(7),
                    id: 11,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: i64::MAX,
                    seen_by_me: false,
                },
            ],
        }];
        assert!(remove_memo_from_groups(&mut groups, UserId(7), 10));
        assert_eq!(groups[0].memos.len(), 1);
        assert_eq!(groups[0].memos[0].id, 11);
    }

    #[test]
    fn group_from_api_applies_viewer_memo_seen() {
        let group = group_from_api(api::MemoCreatorGroup {
            creator_id: 1,
            memos: vec![
                api::ViewerMemo {
                    memo: Some(api::Memo {
                        creator_id: 1,
                        id: 10,
                        ..Default::default()
                    }),
                    seen_by_me: true,
                },
                api::ViewerMemo {
                    memo: Some(api::Memo {
                        creator_id: 1,
                        id: 11,
                        ..Default::default()
                    }),
                    seen_by_me: false,
                },
            ],
        });
        assert!(group.memos[0].seen_by_me);
        assert!(!group.memos[1].seen_by_me);
        assert_eq!(group.memos[0].id, 10);
        assert_eq!(group.memos[1].id, 11);
    }

    #[test]
    fn first_unread_memo_index_skips_seen() {
        let group = MemoCreatorGroup {
            creator_id: UserId(1),
            memos: vec![
                Memo {
                    creator_id: UserId(1),
                    id: 3,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: i64::MAX,
                    seen_by_me: true,
                },
                Memo {
                    creator_id: UserId(1),
                    id: 2,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: i64::MAX,
                    seen_by_me: false,
                },
            ],
        };
        assert_eq!(first_unread_memo_index(&group), 1);
        assert!(group_has_unread(&group));
    }

    #[test]
    fn first_unread_memo_index_defaults_to_zero_when_all_seen() {
        let group = MemoCreatorGroup {
            creator_id: UserId(1),
            memos: vec![Memo {
                creator_id: UserId(1),
                id: 1,
                image_url: String::new(),
                caption: String::new(),
                create_time_second: 0,
                expire_at_second: i64::MAX,
                seen_by_me: true,
            }],
        };
        assert_eq!(first_unread_memo_index(&group), 0);
        assert!(!group_has_unread(&group));
    }

    #[test]
    fn filter_active_group_drops_expired() {
        let now = now_unix();
        let group = MemoCreatorGroup {
            creator_id: UserId(1),
            memos: vec![
                Memo {
                    creator_id: UserId(1),
                    id: 1,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: now - 100,
                    expire_at_second: now - 1,
                    seen_by_me: false,
                },
                Memo {
                    creator_id: UserId(1),
                    id: 2,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: now,
                    expire_at_second: now + 3600,
                    seen_by_me: false,
                },
            ],
        };
        let filtered = filter_active_group(group).expect("one active memo");
        assert_eq!(filtered.memos.len(), 1);
        assert_eq!(filtered.memos[0].id, 2);
    }

    #[test]
    fn purge_expired_groups_removes_expired_memos_and_empty_groups() {
        let now = now_unix();
        let mut groups = vec![
            MemoCreatorGroup {
                creator_id: UserId(1),
                memos: vec![
                    Memo {
                        creator_id: UserId(1),
                        id: 1,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: now - 100,
                        expire_at_second: now - 1,
                        seen_by_me: false,
                    },
                    Memo {
                        creator_id: UserId(1),
                        id: 2,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: now,
                        expire_at_second: now + 3600,
                        seen_by_me: false,
                    },
                ],
            },
            MemoCreatorGroup {
                creator_id: UserId(2),
                memos: vec![Memo {
                    creator_id: UserId(2),
                    id: 3,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: now - 50,
                    expire_at_second: now - 10,
                    seen_by_me: false,
                }],
            },
        ];
        assert!(purge_expired_groups(&mut groups, now));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].memos.len(), 1);
        assert_eq!(groups[0].memos[0].id, 2);
        assert!(!purge_expired_groups(&mut groups, now));
    }

    #[test]
    fn earliest_expire_at_returns_minimum() {
        let groups = vec![
            MemoCreatorGroup {
                creator_id: UserId(1),
                memos: vec![
                    Memo {
                        creator_id: UserId(1),
                        id: 1,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: 0,
                        expire_at_second: 500,
                        seen_by_me: false,
                    },
                    Memo {
                        creator_id: UserId(1),
                        id: 2,
                        image_url: String::new(),
                        caption: String::new(),
                        create_time_second: 0,
                        expire_at_second: 100,
                        seen_by_me: false,
                    },
                ],
            },
            MemoCreatorGroup {
                creator_id: UserId(2),
                memos: vec![Memo {
                    creator_id: UserId(2),
                    id: 3,
                    image_url: String::new(),
                    caption: String::new(),
                    create_time_second: 0,
                    expire_at_second: 250,
                    seen_by_me: false,
                }],
            },
        ];
        assert_eq!(earliest_expire_at(&groups), Some(100));
        assert_eq!(earliest_expire_at(&[]), None);
    }

    #[test]
    fn create_from_path_allowed_blocks_while_creating_or_offline() {
        assert!(!create_from_path_allowed(true, true, 10));
        assert!(!create_from_path_allowed(false, false, 10));
        assert!(!create_from_path_allowed(
            false,
            true,
            MEMO_CAPTION_MAX_RUNES + 1
        ));
        assert!(create_from_path_allowed(
            false,
            true,
            MEMO_CAPTION_MAX_RUNES
        ));
    }

    #[test]
    fn list_fetch_backoff_blocks_immediate_retry() {
        let now = Instant::now();
        assert!(!list_fetch_backoff_active(None, now));
        assert!(list_fetch_backoff_active(Some(now), now));
        assert!(!list_fetch_backoff_active(
            Some(now - LIST_MEMOS_FETCH_RETRY_BACKOFF),
            now
        ));
    }
}
