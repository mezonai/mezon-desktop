use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ids::{ChannelId, UserId};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use mezon_client::RealtimeEvent;

use crate::account::{AccountEvent, AccountStore};
use crate::badge::BadgeService;
use crate::channel::ChannelList;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STATUS_NOTIFY_DEBOUNCE_MS: u64 = 5000;
const TYPING_TTL: Duration = Duration::from_millis(3000);
const TYPING_SWEEP_MS: u64 = 1000;

#[derive(Debug)]
struct TypingEntry {
    name: SharedString,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelTypingState {
    Idle,
    One(SharedString),
    Several,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceEvent {
    TypingChanged { channel_id: ChannelId },
    ChannelPresenceChanged { channel_id: ChannelId },
    StatusChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DmAvatarPresence {
    None = 0,
    Online = 1,
    Idle = 2,
    Dnd = 3,
}

pub const USER_STATUS_ONLINE: &str = "Online";
pub const USER_STATUS_IDLE: &str = "Idle";
pub const USER_STATUS_DND: &str = "Do Not Disturb";
pub const USER_STATUS_INVISIBLE: &str = "Invisible";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserPresence {
    #[default]
    Online,
    Idle,
    Dnd,
    Invisible,
}

impl UserPresence {
    pub fn from_status(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.eq_ignore_ascii_case(USER_STATUS_IDLE) {
            Self::Idle
        } else if raw.eq_ignore_ascii_case("dnd") || raw.eq_ignore_ascii_case(USER_STATUS_DND) {
            Self::Dnd
        } else if raw.eq_ignore_ascii_case(USER_STATUS_INVISIBLE)
            || raw.eq_ignore_ascii_case("offline")
        {
            Self::Invisible
        } else {
            Self::Online
        }
    }

    pub fn as_status(self) -> &'static str {
        match self {
            Self::Online => USER_STATUS_ONLINE,
            Self::Idle => USER_STATUS_IDLE,
            Self::Dnd => USER_STATUS_DND,
            Self::Invisible => USER_STATUS_INVISIBLE,
        }
    }

    pub fn is_visible(self) -> bool {
        !matches!(self, Self::Invisible)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberStatus {
    pub presence: UserPresence,
    pub custom_status: String,
    pub online: bool,
}

impl Default for MemberStatus {
    fn default() -> Self {
        Self::own("", "")
    }
}

impl MemberStatus {
    pub fn own(status: &str, custom_status: &str) -> Self {
        let presence = UserPresence::from_status(status);
        Self {
            presence,
            custom_status: custom_status.to_string(),
            online: presence.is_visible(),
        }
    }
}

pub fn current_user_presence(cx: &App) -> Option<(UserId, UserPresence)> {
    let store = AccountStore::try_global(cx)?;
    let account = store.read(cx).account.as_ref()?;
    Some((
        UserId(account.user_id),
        UserPresence::from_status(&account.status),
    ))
}

pub fn current_user_status(cx: &App) -> Option<(UserId, MemberStatus)> {
    let store = AccountStore::try_global(cx)?;
    let account = store.read(cx).account.as_ref()?;
    Some((
        UserId(account.user_id),
        MemberStatus::own(&account.status, &account.user_status),
    ))
}

#[derive(Debug)]
pub struct PresenceStore {
    typing_by_channel: HashMap<ChannelId, HashMap<UserId, TypingEntry>>,
    pub channel_online: HashMap<ChannelId, HashSet<UserId>>,
    pub user_online: HashSet<UserId>,
    pub presence_status: HashMap<UserId, String>,
    pub user_status: HashMap<UserId, String>,
    presence_known: HashSet<UserId>,
    status_notify_task: Option<Task<()>>,
    typing_sweep_task: Option<Task<()>>,
    _channel_sub: Subscription,
}

struct GlobalPresenceStore(Entity<PresenceStore>);
impl Global for GlobalPresenceStore {}

impl EventEmitter<PresenceEvent> for PresenceStore {}

impl PresenceStore {
    pub fn init(api: Arc<mezon_client::AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalPresenceStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPresenceStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalPresenceStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.typing_by_channel.clear();
        self.typing_sweep_task = None;
        self.channel_online.clear();
        self.user_online.clear();
        self.presence_status.clear();
        self.user_status.clear();
        self.presence_known.clear();
        cx.emit(PresenceEvent::StatusChanged);
        cx.notify();
    }

    pub fn typing_users(&self, channel_id: ChannelId) -> Vec<SharedString> {
        self.typing_users_at(channel_id, Instant::now())
    }

    pub fn channel_typing_state(&self, channel_id: ChannelId) -> ChannelTypingState {
        self.channel_typing_state_at(channel_id, Instant::now())
    }

    fn channel_typing_state_at(&self, channel_id: ChannelId, now: Instant) -> ChannelTypingState {
        let Some(users) = self.typing_by_channel.get(&channel_id) else {
            return ChannelTypingState::Idle;
        };
        let mut first: Option<SharedString> = None;
        let mut count = 0usize;
        for entry in users.values() {
            if now.duration_since(entry.at) >= TYPING_TTL {
                continue;
            }
            count += 1;
            if count == 1 {
                first = Some(entry.name.clone());
            } else {
                return ChannelTypingState::Several;
            }
        }
        match first {
            Some(name) => ChannelTypingState::One(name),
            None => ChannelTypingState::Idle,
        }
    }

    fn typing_users_at(&self, channel_id: ChannelId, now: Instant) -> Vec<SharedString> {
        match self.channel_typing_state_at(channel_id, now) {
            ChannelTypingState::Idle => Vec::new(),
            ChannelTypingState::One(name) => vec![name],
            ChannelTypingState::Several => self
                .typing_by_channel
                .get(&channel_id)
                .map(|users| {
                    users
                        .values()
                        .filter(|entry| now.duration_since(entry.at) < TYPING_TTL)
                        .map(|entry| entry.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub fn is_online(&self, user_id: UserId) -> bool {
        self.user_online.contains(&user_id)
    }

    pub fn user_status(&self, user_id: UserId) -> Option<&str> {
        self.user_status
            .get(&user_id)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn presence_status(&self, user_id: UserId) -> Option<&str> {
        self.presence_status
            .get(&user_id)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn member_online(&self, user_id: UserId, cx: &App) -> bool {
        self.resolve_member_online(user_id, current_user_presence(cx))
    }

    fn resolve_member_online(&self, user_id: UserId, own: Option<(UserId, UserPresence)>) -> bool {
        match own {
            Some((me, presence)) if me == user_id => presence.is_visible(),
            _ => self.is_online(user_id),
        }
    }

    pub fn member_presence(
        &self,
        user_id: UserId,
        own: Option<(UserId, UserPresence)>,
    ) -> DmAvatarPresence {
        if let Some((_, presence)) = own.filter(|(me, _)| *me == user_id) {
            return presence_badge(presence);
        }
        if !self.is_online(user_id) {
            return DmAvatarPresence::None;
        }
        match self.presence_status(user_id) {
            Some(status) => dm_presence_from_status(status),
            None => DmAvatarPresence::Online,
        }
    }

    pub fn dm_avatar_presence(&self, user_id: UserId, api_online: bool) -> DmAvatarPresence {
        let bootstrap = api_online && !self.presence_known.contains(&user_id);
        if !self.is_online(user_id) && !bootstrap {
            return DmAvatarPresence::None;
        }
        match self.presence_status(user_id) {
            Some(status) => dm_presence_from_status(status),
            None => DmAvatarPresence::Online,
        }
    }

    fn mark_presence_known(&mut self, user_id: UserId) {
        self.presence_known.insert(user_id);
    }

    fn new(_api: Arc<mezon_client::AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let channel_sub = cx.subscribe(&ChannelList::global(cx), |this, _channel, event, cx| {
            if let crate::channel::ChannelEvent::ActiveChannelChanged(Some(_)) = event {
                this.typing_by_channel.clear();
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            }
        });

        Self {
            typing_by_channel: HashMap::new(),
            channel_online: HashMap::new(),
            user_online: HashSet::new(),
            presence_status: HashMap::new(),
            user_status: HashMap::new(),
            presence_known: HashSet::new(),
            status_notify_task: None,
            typing_sweep_task: None,
            _channel_sub: channel_sub,
        }
    }

    /// Register realtime handlers with the central dispatcher (cf. `add_message_handler`).
    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            for kind in [
                RealtimeKind::MessageTyping,
                RealtimeKind::ChannelPresence,
                RealtimeKind::StatusPresence,
                RealtimeKind::CustomStatus,
                RealtimeKind::UserStatus,
            ] {
                dispatch.on(kind, &entity, |this, event, cx| {
                    this.handle_event(event, cx)
                });
            }
            dispatch.on_lagged(&entity, |this, cx| {
                tracing::warn!("PresenceStore realtime lagged — clearing state");
                this.typing_by_channel.clear();
                this.channel_online.clear();
                this.user_online.clear();
                this.presence_status.clear();
                this.user_status.clear();
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            });
        });
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::MessageTyping(e) => {
                let sender = UserId(e.sender_id);
                let is_self = BadgeService::try_global(cx)
                    .and_then(|svc| svc.read(cx).current_user_id(cx))
                    .is_some_and(|me| me == sender);
                if is_self {
                    return;
                }
                let cid = if e.topic_id != 0 {
                    ChannelId(e.topic_id)
                } else {
                    ChannelId(e.channel_id)
                };
                let channel_id =
                    self.apply_typing(cid, &e.sender_display_name, &e.sender_username, sender);
                self.schedule_typing_sweep(cx);
                cx.emit(PresenceEvent::TypingChanged { channel_id });
            }
            RealtimeEvent::ChannelPresence(e) => {
                let cid = ChannelId(e.channel_id);
                let joins: Vec<UserId> = e.joins.iter().map(|u| UserId(u.user_id)).collect();
                let leaves: Vec<UserId> = e.leaves.iter().map(|u| UserId(u.user_id)).collect();
                self.apply_channel_presence(cid, &joins, &leaves);
                cx.emit(PresenceEvent::ChannelPresenceChanged { channel_id: cid });
                cx.notify();
            }
            RealtimeEvent::StatusPresence(e) => {
                let joins: Vec<UserId> = e.joins.iter().map(|u| UserId(u.user_id)).collect();
                let leaves: Vec<UserId> = e.leaves.iter().map(|u| UserId(u.user_id)).collect();
                let presence_statuses: Vec<(UserId, String)> = e
                    .joins
                    .iter()
                    .map(|u| (UserId(u.user_id), u.status.clone().unwrap_or_default()))
                    .collect();
                let custom_statuses: Vec<(UserId, String)> = e
                    .joins
                    .iter()
                    .map(|u| (UserId(u.user_id), u.user_status.clone()))
                    .collect();
                self.apply_status_presence(&joins, &leaves);
                self.apply_presence_statuses(&presence_statuses, &leaves);
                self.apply_seed_user_status(&custom_statuses);
                self.schedule_status_notify(cx);
            }
            RealtimeEvent::CustomStatus(e) => {
                tracing::debug!(user_id = e.user_id, " custom-status text updated");
                self.apply_seed_user_status(&[(UserId(e.user_id), e.status.clone())]);
                Self::sync_self_account(e.user_id, None, Some(&e.status), cx);
                self.schedule_status_notify(cx);
            }
            RealtimeEvent::UserStatus(e) => {
                let user_id = UserId(e.user_id);
                let status = e.custom_status.trim();
                tracing::debug!(user_id = e.user_id, status, " user-status updated");
                if UserPresence::from_status(status).is_visible() {
                    self.user_online.insert(user_id);
                } else {
                    self.user_online.remove(&user_id);
                }
                self.presence_status.insert(user_id, status.to_string());
                self.mark_presence_known(user_id);
                Self::sync_self_account(e.user_id, Some(status), None, cx);
                self.schedule_status_notify(cx);
            }
            _ => {}
        }
    }

    fn schedule_status_notify(&mut self, cx: &mut Context<Self>) {
        if self.status_notify_task.is_some() {
            return;
        }
        let delay = Duration::from_millis(STATUS_NOTIFY_DEBOUNCE_MS);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |store, cx| {
                store.status_notify_task = None;
                cx.emit(PresenceEvent::StatusChanged);
                cx.notify();
            });
        });
        self.status_notify_task = Some(task);
    }

    fn schedule_typing_sweep(&mut self, cx: &mut Context<Self>) {
        if self.typing_sweep_task.is_some() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(TYPING_SWEEP_MS))
                    .await;
                let keep_going = this.update(cx, |store, cx| {
                    let expired = store.sweep_expired_typing();
                    for channel_id in expired {
                        cx.emit(PresenceEvent::TypingChanged { channel_id });
                    }
                    !store.typing_by_channel.is_empty()
                });
                match keep_going {
                    Ok(true) => continue,
                    _ => break,
                }
            }
            let _ = this.update(cx, |store, _| {
                store.typing_sweep_task = None;
            });
        });
        self.typing_sweep_task = Some(task);
    }

    fn sweep_expired_typing(&mut self) -> Vec<ChannelId> {
        self.sweep_expired_typing_at(Instant::now())
    }

    fn sweep_expired_typing_at(&mut self, now: Instant) -> Vec<ChannelId> {
        let mut expired = Vec::new();
        self.typing_by_channel.retain(|channel_id, users| {
            let before = users.len();
            users.retain(|_, entry| now.duration_since(entry.at) < TYPING_TTL);
            if users.len() != before {
                expired.push(*channel_id);
            }
            !users.is_empty()
        });
        expired
    }

    pub(crate) fn apply_typing(
        &mut self,
        channel_id: ChannelId,
        display_name: &str,
        username: &str,
        sender_id: UserId,
    ) -> ChannelId {
        let name = if !display_name.is_empty() {
            SharedString::from(display_name.to_owned())
        } else if !username.is_empty() {
            SharedString::from(username.to_owned())
        } else {
            SharedString::from(sender_id.to_string())
        };
        self.typing_by_channel
            .entry(channel_id)
            .or_default()
            .insert(
                sender_id,
                TypingEntry {
                    name,
                    at: Instant::now(),
                },
            );
        channel_id
    }

    pub(crate) fn apply_channel_presence(
        &mut self,
        channel_id: ChannelId,
        joins: &[UserId],
        leaves: &[UserId],
    ) {
        let entry = self.channel_online.entry(channel_id).or_default();
        for uid in joins {
            entry.insert(*uid);
            self.user_online.insert(*uid);
        }
        for uid in leaves {
            entry.remove(uid);
            self.user_online.remove(uid);
        }
        if entry.is_empty() {
            self.channel_online.remove(&channel_id);
        }
    }

    pub(crate) fn apply_status_presence(&mut self, joins: &[UserId], leaves: &[UserId]) {
        for uid in joins {
            self.user_online.insert(*uid);
        }
        for uid in leaves {
            self.user_online.remove(uid);
        }
    }

    fn apply_presence_statuses(&mut self, statuses: &[(UserId, String)], leaves: &[UserId]) {
        for uid in leaves {
            self.presence_status.insert(*uid, "offline".to_string());
            self.mark_presence_known(*uid);
        }
        for (uid, status) in statuses {
            let value = if status.is_empty() { "online" } else { status };
            self.presence_status.insert(*uid, value.to_string());
            self.mark_presence_known(*uid);
            if value.eq_ignore_ascii_case(USER_STATUS_INVISIBLE) {
                self.user_online.remove(uid);
            }
        }
    }

    fn sync_self_account(
        user_id: i64,
        status: Option<&str>,
        custom_status: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let is_self = BadgeService::try_global(cx)
            .and_then(|service| service.read(cx).current_user_id(cx))
            .is_some_and(|current_user_id| current_user_id.0 == user_id);
        if !is_self {
            return;
        }

        AccountStore::global(cx).update(cx, |store, cx| {
            let Some(account) = store.account.as_mut() else {
                return;
            };
            let mut changed = false;
            if let Some(status) = status
                && account.status != status
            {
                account.status = status.to_string();
                changed = true;
            }
            if let Some(custom_status) = custom_status
                && account.user_status != custom_status
            {
                account.user_status = custom_status.to_string();
                changed = true;
            }
            if changed {
                cx.emit(AccountEvent::StatusUpdated);
                cx.notify();
            }
        });
    }

    pub fn seed_presence(
        &mut self,
        online: &[UserId],
        statuses: &[(UserId, String)],
        presences: &[(UserId, String)],
        cx: &mut Context<Self>,
    ) {
        let changed = self.apply_seed_online(online)
            | self.apply_seed_user_status(statuses)
            | self.apply_seed_presence_status(presences);
        if changed {
            cx.emit(PresenceEvent::StatusChanged);
            cx.notify();
        }
    }

    pub(crate) fn apply_seed_online(&mut self, online: &[UserId]) -> bool {
        let mut changed = false;
        for uid in online {
            changed |= self.user_online.insert(*uid);
        }
        changed
    }

    pub(crate) fn apply_seed_presence_status(&mut self, presences: &[(UserId, String)]) -> bool {
        let mut changed = false;
        for (uid, status) in presences {
            if self.presence_status.get(uid).map(String::as_str) != Some(status.as_str()) {
                self.presence_status.insert(*uid, status.clone());
                changed = true;
            }
            self.mark_presence_known(*uid);
            if status.eq_ignore_ascii_case(USER_STATUS_INVISIBLE) {
                changed |= self.user_online.remove(uid);
            }
        }
        changed
    }

    pub(crate) fn apply_seed_user_status(&mut self, statuses: &[(UserId, String)]) -> bool {
        let mut changed = false;
        for (uid, status) in statuses {
            if status.is_empty() {
                changed |= self.user_status.remove(uid).is_some();
            } else if self.user_status.get(uid) != Some(status) {
                self.user_status.insert(*uid, status.clone());
                changed = true;
            }
        }
        changed
    }
}

fn presence_badge(presence: UserPresence) -> DmAvatarPresence {
    match presence {
        UserPresence::Online => DmAvatarPresence::Online,
        UserPresence::Idle => DmAvatarPresence::Idle,
        UserPresence::Dnd => DmAvatarPresence::Dnd,
        UserPresence::Invisible => DmAvatarPresence::None,
    }
}

fn dm_presence_from_status(status: &str) -> DmAvatarPresence {
    presence_badge(UserPresence::from_status(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_store() -> PresenceStore {
        PresenceStore {
            typing_by_channel: HashMap::new(),
            channel_online: HashMap::new(),
            user_online: HashSet::new(),
            presence_status: HashMap::new(),
            user_status: HashMap::new(),
            presence_known: HashSet::new(),
            status_notify_task: None,
            typing_sweep_task: None,
            _channel_sub: gpui::Subscription::new(|| {}),
        }
    }

    #[test]
    fn typing_adds_user_by_display_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "alice_user", UserId(1));
        assert!(
            store
                .typing_users(ChannelId(1))
                .iter()
                .any(|u| u == "Alice")
        );
    }

    #[test]
    fn typing_falls_back_to_username_when_no_display_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "", "alice_user", UserId(1));
        assert!(
            store
                .typing_users(ChannelId(1))
                .iter()
                .any(|u| u == "alice_user")
        );
    }

    #[test]
    fn typing_falls_back_to_sender_id_when_no_name() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "", "", UserId(42));
        assert!(store.typing_users(ChannelId(1)).iter().any(|u| u == "42"));
    }

    #[test]
    fn typing_keys_by_user_id_so_same_user_stays_single() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", UserId(1));
        store.apply_typing(ChannelId(1), "Alice", "", UserId(1));
        assert_eq!(store.typing_users(ChannelId(1)).len(), 1);
    }

    #[test]
    fn typing_keeps_distinct_users() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", UserId(1));
        store.apply_typing(ChannelId(1), "Bob", "", UserId(2));
        assert_eq!(store.typing_users(ChannelId(1)).len(), 2);
    }

    #[test]
    fn typing_sweep_removes_expired_entries() {
        let mut store = empty_store();
        let base = Instant::now();
        store
            .typing_by_channel
            .entry(ChannelId(1))
            .or_default()
            .insert(
                UserId(1),
                TypingEntry {
                    name: "Old".into(),
                    at: base,
                },
            );
        let future = base + TYPING_TTL + Duration::from_secs(1);
        let expired = store.sweep_expired_typing_at(future);
        assert_eq!(expired, vec![ChannelId(1)]);
        assert!(store.typing_users_at(ChannelId(1), future).is_empty());
    }

    #[test]
    fn channel_presence_join_adds_to_channel_and_global() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1), UserId(2)], &[]);
        assert!(store.channel_online[&ChannelId(1)].contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn channel_presence_leave_removes_from_channel_and_global() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1)], &[]);
        store.apply_channel_presence(ChannelId(1), &[], &[UserId(1)]);
        assert!(!store.channel_online.contains_key(&ChannelId(1)));
        assert!(!store.user_online.contains(&UserId(1)));
    }

    #[test]
    fn channel_presence_empty_channel_cleaned_up() {
        let mut store = empty_store();
        store.apply_channel_presence(ChannelId(1), &[UserId(1)], &[]);
        store.apply_channel_presence(ChannelId(1), &[], &[UserId(1)]);
        assert!(!store.channel_online.contains_key(&ChannelId(1)));
    }

    #[test]
    fn status_presence_join_adds_to_user_online() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1), UserId(2)], &[]);
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn status_presence_leave_removes_from_user_online() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1)], &[]);
        store.apply_status_presence(&[], &[UserId(1)]);
        assert!(!store.user_online.contains(&UserId(1)));
    }

    #[test]
    fn seed_online_marks_users_online_without_realtime() {
        let mut store = empty_store();
        let changed = store.apply_seed_online(&[UserId(1), UserId(2)]);
        assert!(changed);
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn seed_online_merges_and_reports_no_change_when_already_present() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1)], &[]);
        let changed = store.apply_seed_online(&[UserId(1), UserId(3)]);
        assert!(changed);
        assert!(store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(3)));
        assert!(!store.apply_seed_online(&[UserId(1), UserId(3)]));
    }

    #[test]
    fn realtime_leave_overrides_seeded_online() {
        let mut store = empty_store();
        store.apply_seed_online(&[UserId(1), UserId(2)]);
        store.apply_status_presence(&[], &[UserId(1)]);
        assert!(!store.user_online.contains(&UserId(1)));
        assert!(store.user_online.contains(&UserId(2)));
    }

    #[test]
    fn seeded_invisible_member_is_dropped_from_the_online_set() {
        let mut store = empty_store();
        store.apply_seed_online(&[UserId(1), UserId(2), UserId(3)]);
        let changed = store.apply_seed_presence_status(&[
            (UserId(1), USER_STATUS_ONLINE.to_string()),
            (UserId(2), USER_STATUS_IDLE.to_string()),
            (UserId(3), USER_STATUS_INVISIBLE.to_string()),
        ]);

        assert!(changed);
        assert!(store.is_online(UserId(1)));
        assert!(store.is_online(UserId(2)));
        assert!(!store.is_online(UserId(3)));
        assert_eq!(store.presence_status(UserId(2)), Some(USER_STATUS_IDLE));
    }

    #[test]
    fn a_realtime_invisible_join_does_not_stay_online() {
        let mut store = empty_store();
        store.apply_status_presence(&[UserId(1), UserId(2)], &[]);
        store.apply_presence_statuses(
            &[
                (UserId(1), USER_STATUS_DND.to_string()),
                (UserId(2), USER_STATUS_INVISIBLE.to_string()),
            ],
            &[],
        );

        assert!(store.is_online(UserId(1)));
        assert!(!store.is_online(UserId(2)));
        assert_eq!(store.presence_status(UserId(1)), Some(USER_STATUS_DND));
    }

    #[test]
    fn a_seeded_status_drives_the_member_badge() {
        let mut store = empty_store();
        store.apply_seed_online(&[UserId(1), UserId(2), UserId(3), UserId(4)]);
        store.apply_seed_presence_status(&[
            (UserId(1), USER_STATUS_ONLINE.to_string()),
            (UserId(2), USER_STATUS_IDLE.to_string()),
            (UserId(3), USER_STATUS_DND.to_string()),
            (UserId(4), USER_STATUS_INVISIBLE.to_string()),
        ]);

        assert_eq!(
            store.member_presence(UserId(1), None),
            DmAvatarPresence::Online
        );
        assert_eq!(
            store.member_presence(UserId(2), None),
            DmAvatarPresence::Idle
        );
        assert_eq!(
            store.member_presence(UserId(3), None),
            DmAvatarPresence::Dnd
        );
        assert_eq!(
            store.member_presence(UserId(4), None),
            DmAvatarPresence::None
        );
    }

    #[test]
    fn an_online_member_without_a_seeded_status_falls_back_to_online() {
        let mut store = empty_store();
        store.apply_seed_online(&[UserId(1)]);

        assert_eq!(
            store.member_presence(UserId(1), None),
            DmAvatarPresence::Online
        );
    }

    #[test]
    fn seed_user_status_stores_and_reads_back() {
        let mut store = empty_store();
        let changed = store.apply_seed_user_status(&[(UserId(1), "Working".to_string())]);
        assert!(changed);
        assert_eq!(store.user_status(UserId(1)), Some("Working"));
    }

    #[test]
    fn seed_user_status_empty_string_reads_as_none() {
        let mut store = empty_store();
        store.apply_seed_user_status(&[(UserId(1), String::new())]);
        assert_eq!(store.user_status(UserId(1)), None);
    }

    #[test]
    fn seed_user_status_empty_clears_previous_and_reports_change() {
        let mut store = empty_store();
        store.apply_seed_user_status(&[(UserId(1), "Busy".to_string())]);
        let changed = store.apply_seed_user_status(&[(UserId(1), String::new())]);
        assert!(changed);
        assert_eq!(store.user_status(UserId(1)), None);
    }

    #[test]
    fn seed_user_status_no_change_when_same() {
        let mut store = empty_store();
        store.apply_seed_user_status(&[(UserId(1), "Away".to_string())]);
        assert!(!store.apply_seed_user_status(&[(UserId(1), "Away".to_string())]));
    }

    #[test]
    fn typing_users_returns_set_for_channel() {
        let mut store = empty_store();
        store.apply_typing(ChannelId(1), "Alice", "", UserId(1));
        let users = store.typing_users(ChannelId(1));
        assert!(users.iter().any(|u| u == "Alice"));
        assert_eq!(users.len(), 1);
    }

    #[test]
    fn typing_users_returns_empty_for_unknown_channel() {
        let store = empty_store();
        assert!(store.typing_users(ChannelId(999)).is_empty());
    }

    #[test]
    fn user_presence_classifies_every_wire_spelling() {
        assert_eq!(UserPresence::from_status("Online"), UserPresence::Online);
        assert_eq!(UserPresence::from_status("idle"), UserPresence::Idle);
        assert_eq!(UserPresence::from_status("dnd"), UserPresence::Dnd);
        assert_eq!(
            UserPresence::from_status("Do Not Disturb"),
            UserPresence::Dnd
        );
        assert_eq!(
            UserPresence::from_status(" invisible "),
            UserPresence::Invisible
        );
        assert_eq!(
            UserPresence::from_status("offline"),
            UserPresence::Invisible
        );
    }

    #[test]
    fn empty_account_status_reads_as_online() {
        assert_eq!(UserPresence::from_status(""), UserPresence::Online);
        assert_eq!(MemberStatus::own("", "").presence, UserPresence::Online);
        assert!(MemberStatus::own("", "").online);
    }

    #[test]
    fn self_reads_online_even_when_the_presence_set_is_empty() {
        let store = empty_store();
        let me = UserId(7);
        assert!(!store.is_online(me));
        assert!(store.resolve_member_online(me, Some((me, UserPresence::Online))));
        assert!(store.resolve_member_online(me, Some((me, UserPresence::Idle))));
        assert!(store.resolve_member_online(me, Some((me, UserPresence::Dnd))));
    }

    #[test]
    fn self_reads_offline_only_when_invisible() {
        let mut store = empty_store();
        let me = UserId(7);
        store.user_online.insert(me);
        assert!(!store.resolve_member_online(me, Some((me, UserPresence::Invisible))));
    }

    #[test]
    fn other_members_still_come_from_the_presence_set() {
        let mut store = empty_store();
        let me = UserId(7);
        let other = UserId(8);
        store.user_online.insert(other);
        assert!(store.resolve_member_online(other, Some((me, UserPresence::Invisible))));
        assert!(!store.resolve_member_online(UserId(9), Some((me, UserPresence::Online))));
    }

    #[test]
    fn without_a_loaded_account_everyone_comes_from_presence() {
        let mut store = empty_store();
        let me = UserId(7);
        assert!(!store.resolve_member_online(me, None));
        store.user_online.insert(me);
        assert!(store.resolve_member_online(me, None));
    }

    #[test]
    fn own_status_matrix_covers_every_state() {
        let cases = [
            ("Online", UserPresence::Online, true),
            ("online", UserPresence::Online, true),
            ("", UserPresence::Online, true),
            ("Idle", UserPresence::Idle, true),
            ("idle", UserPresence::Idle, true),
            ("Do Not Disturb", UserPresence::Dnd, true),
            ("dnd", UserPresence::Dnd, true),
            ("Invisible", UserPresence::Invisible, false),
            ("invisible", UserPresence::Invisible, false),
            ("offline", UserPresence::Invisible, false),
        ];
        for (raw, presence, online) in cases {
            for custom in ["", "Deep work"] {
                let status = MemberStatus::own(raw, custom);
                assert_eq!(status.presence, presence, "presence for {raw:?}");
                assert_eq!(status.online, online, "online for {raw:?}");
                assert_eq!(status.custom_status, custom, "custom for {raw:?}");
            }
        }
    }

    #[test]
    fn own_status_ignores_the_presence_set() {
        let own = MemberStatus::own("Do Not Disturb", "Deep work");
        assert_eq!(own.presence, UserPresence::Dnd);
        assert_eq!(own.custom_status, "Deep work");
        assert!(own.online);
    }

    #[test]
    fn own_invisible_status_reads_as_offline() {
        let own = MemberStatus::own("Invisible", "");
        assert_eq!(own.presence, UserPresence::Invisible);
        assert!(!own.online);
    }

    #[test]
    fn canonical_status_round_trips_through_classification() {
        for presence in [
            UserPresence::Online,
            UserPresence::Idle,
            UserPresence::Dnd,
            UserPresence::Invisible,
        ] {
            assert_eq!(UserPresence::from_status(presence.as_status()), presence);
        }
    }

    #[test]
    fn dm_avatar_presence_prefers_realtime_idle_over_stale_api_offline() {
        let mut store = empty_store();
        let user_id = UserId(42);
        store.user_online.insert(user_id);
        store.presence_status.insert(user_id, "Idle".to_string());
        assert_eq!(
            store.dm_avatar_presence(user_id, false),
            DmAvatarPresence::Idle
        );
    }

    #[test]
    fn dm_avatar_presence_falls_back_to_api_online_before_realtime() {
        let store = empty_store();
        assert_eq!(
            store.dm_avatar_presence(UserId(1), true),
            DmAvatarPresence::Online
        );
    }

    #[test]
    fn dm_avatar_presence_ignores_stale_api_after_presence_known() {
        let mut store = empty_store();
        let user_id = UserId(1);
        store.mark_presence_known(user_id);
        store.user_online.remove(&user_id);
        store.presence_status.clear();
        assert_eq!(
            store.dm_avatar_presence(user_id, true),
            DmAvatarPresence::None
        );
    }

    #[test]
    fn dm_avatar_presence_maps_dnd_status() {
        let mut store = empty_store();
        let user_id = UserId(3);
        store.user_online.insert(user_id);
        store
            .presence_status
            .insert(user_id, "Do Not Disturb".to_string());
        assert_eq!(
            store.dm_avatar_presence(user_id, false),
            DmAvatarPresence::Dnd
        );
    }

    #[test]
    fn dm_avatar_presence_hides_offline_and_invisible_users() {
        let store = empty_store();
        assert_eq!(
            store.dm_avatar_presence(UserId(1), false),
            DmAvatarPresence::None
        );
        let mut store = empty_store();
        let user_id = UserId(7);
        store
            .presence_status
            .insert(user_id, "Invisible".to_string());
        assert_eq!(
            store.dm_avatar_presence(user_id, true),
            DmAvatarPresence::None
        );
    }
}
