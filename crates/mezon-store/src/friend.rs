use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::transport::ApiFriend;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::realtime;

use crate::Freshness;
use crate::ids::UserId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

/// Relationship state, mirroring the proto `Friend.State` enum (and React `EStateFriend`):
/// - [`Friend`](FriendState::Friend) (0): already friends.
/// - [`InviteSent`](FriendState::InviteSent) (1): the current user sent a request (outgoing, "OTHER_PENDING").
/// - [`InviteReceived`](FriendState::InviteReceived) (2): the current user received a request (incoming, "MY_PENDING").
/// - [`Blocked`](FriendState::Blocked) (3): the current user blocked this user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendState {
    Friend,
    InviteSent,
    InviteReceived,
    Blocked,
}

impl FriendState {
    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => Self::InviteSent,
            2 => Self::InviteReceived,
            3 => Self::Blocked,
            _ => Self::Friend,
        }
    }
}

/// A friend relationship in domain terms. `id` is the other user; `source_id` is
/// whichever side initiated the relationship (used to identify who did the blocking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    pub state: FriendState,
    pub source_id: UserId,
}

impl Friend {
    /// Display name if present, else username — matches React `display_name || username`.
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

#[derive(Debug, Clone)]
pub enum FriendEvent {
    Changed,
    /// A friend request could not be sent (server rejected the username or the RPC failed).
    AddFailed,
}

fn friend_from_api(f: ApiFriend) -> Friend {
    Friend {
        id: UserId(f.account.user_id),
        username: f.account.username,
        display_name: f.account.display_name.unwrap_or_default(),
        avatar_url: f.account.avatar_url.unwrap_or_default(),
        state: FriendState::from_i32(f.state),
        source_id: UserId(f.source_id),
    }
}

fn friend_from_add(e: &realtime::AddFriend) -> Friend {
    Friend {
        id: UserId(e.user_id),
        username: e.username.clone(),
        display_name: e.display_name.clone(),
        avatar_url: e.avatar.clone(),
        state: FriendState::InviteReceived,
        source_id: UserId(e.user_id),
    }
}

/// Apply an incoming `AddFriend` event to the list, mirroring React `upsertFriendRequest`:
/// an existing outgoing request that the peer accepted flips to `Friend`; otherwise it is a
/// new incoming request (`InviteReceived`). Returns `true` if the list changed.
fn apply_add_friend(list: &mut Vec<Friend>, e: &realtime::AddFriend) -> bool {
    let id = UserId(e.user_id);
    if let Some(existing) = list.iter_mut().find(|f| f.id == id) {
        if existing.state == FriendState::InviteSent {
            existing.state = FriendState::Friend;
            return true;
        }
        return false;
    }
    list.push(friend_from_add(e));
    true
}

/// Remove a friend by id. Returns `true` if a friend was removed.
fn apply_remove_friend(list: &mut Vec<Friend>, id: UserId) -> bool {
    let before = list.len();
    list.retain(|f| f.id != id);
    list.len() != before
}

struct GlobalFriendStore(Entity<FriendStore>);
impl Global for GlobalFriendStore {}

pub struct FriendStore {
    friends: Vec<Friend>,
    loading: bool,
    pending_refetch: bool,
    adding: bool,
    freshness: Freshness,
    reset_generation: u64,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

impl EventEmitter<FriendEvent> for FriendStore {}

impl FriendStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalFriendStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalFriendStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFriendStore>().map(|g| g.0.clone())
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            friends: Vec::new(),
            loading: false,
            pending_refetch: false,
            adding: false,
            freshness: Freshness::new(),
            reset_generation: 0,
            api,
            _conn_watch: conn_watch,
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_generation = self.reset_generation.wrapping_add(1);
        self.friends.clear();
        self.loading = false;
        self.pending_refetch = false;
        self.adding = false;
        self.freshness.mark_stale();
        cx.emit(FriendEvent::Changed);
        cx.notify();
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [RealtimeKind::AddFriend, RealtimeKind::RemoveFriend] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| this.refresh(cx));
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
                    if this
                        .update(cx, |this, cx| {
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

    pub fn friends(&self) -> &[Friend] {
        &self.friends
    }

    pub fn is_adding(&self) -> bool {
        self.adding
    }

    /// Count of incoming friend requests awaiting the current user's response
    /// (React `quantityPendingRequest` = friends with state `MY_PENDING`).
    pub fn pending_incoming_count(&self) -> usize {
        self.friends
            .iter()
            .filter(|f| f.state == FriendState::InviteReceived)
            .count()
    }

    /// Look up an existing friend relationship by user id.
    pub fn friend(&self, id: UserId) -> Option<&Friend> {
        self.friends.iter().find(|f| f.id == id)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx);
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.loading && !self.freshness.is_fresh(crate::CACHE_TTL) {
            self.fetch(cx);
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.pending_refetch = true;
            return;
        }
        self.pending_refetch = false;
        self.loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_friends().await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(list) => {
                        this.friends = list.into_iter().map(friend_from_api).collect();
                        this.freshness.mark_fetched();
                        cx.emit(FriendEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_friends failed: {e}"),
                }
                if this.pending_refetch {
                    this.pending_refetch = false;
                    this.fetch(cx);
                }
            });
        })
        .detach();
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let changed = match event {
            RealtimeEvent::AddFriend(e) => apply_add_friend(&mut self.friends, e),
            RealtimeEvent::RemoveFriend(e) => {
                apply_remove_friend(&mut self.friends, UserId(e.user_id))
            }
            _ => false,
        };
        if changed {
            cx.emit(FriendEvent::Changed);
            cx.notify();
        }
    }

    /// Send a friend request by username (React add-friend modal). Optimistically inserts
    /// an outgoing request on success so the Pending tab reflects it immediately.
    pub fn add_friend_by_username(&mut self, username: String, cx: &mut Context<Self>) {
        if self.adding || username.is_empty() {
            return;
        }
        self.adding = true;
        cx.notify();
        let me = self.current_user_id(cx);
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.add_friends(Vec::new(), vec![username.clone()]).await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.adding = false;
                match result {
                    Ok(ids) => match ids.first() {
                        Some(&id) if id != 0 => {
                            let uid = UserId(id);
                            if !this.friends.iter().any(|f| f.id == uid) {
                                this.friends.push(Friend {
                                    id: uid,
                                    username,
                                    display_name: String::new(),
                                    avatar_url: String::new(),
                                    state: FriendState::InviteSent,
                                    source_id: me,
                                });
                            }
                            cx.emit(FriendEvent::Changed);
                        }
                        _ => {
                            tracing::warn!("add_friend: server returned no id");
                            cx.emit(FriendEvent::AddFailed);
                        }
                    },
                    Err(e) => {
                        tracing::error!("add_friend failed: {e}");
                        cx.emit(FriendEvent::AddFailed);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Accept an incoming friend request (React accept). Optimistically flips to `Friend`.
    pub fn accept_friend(&mut self, friend_id: UserId, cx: &mut Context<Self>) {
        if !self.set_state(friend_id, FriendState::Friend, cx) {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.add_friends(vec![friend_id.0], Vec::new()).await;
            if let Err(e) = result {
                tracing::error!("accept_friend failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation == generation {
                        this.refresh(cx);
                    }
                });
            }
        })
        .detach();
    }

    /// Remove / cancel / reject a friend relationship (all map to DeleteFriends).
    /// Optimistically removes the row.
    pub fn delete_friend(&mut self, friend_id: UserId, cx: &mut Context<Self>) {
        if !apply_remove_friend(&mut self.friends, friend_id) {
            return;
        }
        cx.emit(FriendEvent::Changed);
        cx.notify();
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.delete_friends(vec![friend_id.0], Vec::new()).await;
            if let Err(e) = result {
                tracing::error!("delete_friend failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation == generation {
                        this.refresh(cx);
                    }
                });
            }
        })
        .detach();
    }

    /// Block a user. Optimistically marks the relationship blocked (initiated by the current user).
    pub fn block_friend(&mut self, friend_id: UserId, cx: &mut Context<Self>) {
        let me = self.current_user_id(cx);
        let Some(friend) = self.friends.iter_mut().find(|f| f.id == friend_id) else {
            return;
        };
        if friend.state == FriendState::Blocked {
            return;
        }
        friend.state = FriendState::Blocked;
        friend.source_id = me;
        cx.emit(FriendEvent::Changed);
        cx.notify();
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.block_friends(vec![friend_id.0]).await;
            if let Err(e) = result {
                tracing::error!("block_friend failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation == generation {
                        this.refresh(cx);
                    }
                });
            }
        })
        .detach();
    }

    /// Unblock a user. Optimistically clears the block (React flips the state to `Friend`).
    pub fn unblock_friend(&mut self, friend_id: UserId, cx: &mut Context<Self>) {
        if !self.set_state(friend_id, FriendState::Friend, cx) {
            return;
        }
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.unblock_friends(vec![friend_id.0]).await;
            if let Err(e) = result {
                tracing::error!("unblock_friend failed: {e}");
            }
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation == generation {
                    this.freshness.mark_stale();
                    this.fetch(cx);
                }
            });
        })
        .detach();
    }

    fn set_state(&mut self, friend_id: UserId, state: FriendState, cx: &mut Context<Self>) -> bool {
        if let Some(friend) = self.friends.iter_mut().find(|f| f.id == friend_id)
            && friend.state != state
        {
            friend.state = state;
            cx.emit(FriendEvent::Changed);
            cx.notify();
            return true;
        }
        false
    }

    fn current_user_id(&self, cx: &App) -> UserId {
        crate::badge::BadgeService::try_global(cx)
            .and_then(|b| b.read(cx).current_user_id(cx))
            .unwrap_or(UserId(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mezon_client::transport::ApiAccount;

    fn account(id: i64, username: &str, display: &str) -> ApiAccount {
        ApiAccount {
            user_id: id,
            username: username.to_string(),
            email: None,
            display_name: (!display.is_empty()).then(|| display.to_string()),
            avatar_url: None,
            about_me: None,
            phone_number: None,
            password_setted: false,
            logo: None,
        }
    }

    #[test]
    fn state_from_i32_maps_proto_values() {
        assert_eq!(FriendState::from_i32(0), FriendState::Friend);
        assert_eq!(FriendState::from_i32(1), FriendState::InviteSent);
        assert_eq!(FriendState::from_i32(2), FriendState::InviteReceived);
        assert_eq!(FriendState::from_i32(3), FriendState::Blocked);
        assert_eq!(FriendState::from_i32(99), FriendState::Friend);
    }

    #[test]
    fn friend_from_api_maps_fields_and_label() {
        let f = friend_from_api(ApiFriend {
            account: account(7, "alice", ""),
            state: 2,
            source_id: 7,
        });
        assert_eq!(f.id, UserId(7));
        assert_eq!(f.username, "alice");
        assert_eq!(f.state, FriendState::InviteReceived);
        assert_eq!(f.label(), "alice");

        let f2 = friend_from_api(ApiFriend {
            account: account(8, "bob", "Bobby"),
            state: 0,
            source_id: 9,
        });
        assert_eq!(f2.label(), "Bobby");
        assert_eq!(f2.source_id, UserId(9));
    }

    #[test]
    fn add_friend_event_inserts_incoming_request() {
        let mut list = Vec::new();
        let e = realtime::AddFriend {
            user_id: 42,
            username: "carol".into(),
            display_name: "Carol".into(),
            avatar: String::new(),
        };
        assert!(apply_add_friend(&mut list, &e));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, UserId(42));
        assert_eq!(list[0].state, FriendState::InviteReceived);
        // Duplicate incoming request is a no-op.
        assert!(!apply_add_friend(&mut list, &e));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn add_friend_event_promotes_outgoing_request() {
        let mut list = vec![Friend {
            id: UserId(42),
            username: "carol".into(),
            display_name: String::new(),
            avatar_url: String::new(),
            state: FriendState::InviteSent,
            source_id: UserId(1),
        }];
        let e = realtime::AddFriend {
            user_id: 42,
            username: "carol".into(),
            display_name: String::new(),
            avatar: String::new(),
        };
        assert!(apply_add_friend(&mut list, &e));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, FriendState::Friend);
    }

    #[test]
    fn remove_friend_event_drops_row() {
        let mut list = vec![Friend {
            id: UserId(42),
            username: "carol".into(),
            display_name: String::new(),
            avatar_url: String::new(),
            state: FriendState::Friend,
            source_id: UserId(1),
        }];
        assert!(apply_remove_friend(&mut list, UserId(42)));
        assert!(list.is_empty());
        assert!(!apply_remove_friend(&mut list, UserId(42)));
    }
}
