use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ids::{ChannelId, UserId};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use mezon_client::RealtimeEvent;

use crate::badge::BadgeService;
use crate::channel::ChannelList;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const STATUS_NOTIFY_DEBOUNCE_MS: u64 = 5000;
const TYPING_TTL: Duration = Duration::from_millis(3000);
const TYPING_SWEEP_MS: u64 = 1000;

#[derive(Debug)]
struct TypingEntry {
    name: String,
    at: Instant,
}

#[derive(Debug, Clone)]
pub enum PresenceEvent {
    TypingChanged { channel_id: ChannelId },
    ChannelPresenceChanged { channel_id: ChannelId },
    StatusChanged,
}

#[derive(Debug)]
pub struct PresenceStore {
    typing_by_channel: HashMap<ChannelId, HashMap<UserId, TypingEntry>>,
    pub channel_online: HashMap<ChannelId, HashSet<UserId>>,
    pub user_online: HashSet<UserId>,
    pub user_status: HashMap<UserId, String>,
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
        self.user_status.clear();
        cx.emit(PresenceEvent::StatusChanged);
        cx.notify();
    }

    pub fn typing_users(&self, channel_id: ChannelId) -> Vec<SharedString> {
        self.typing_users_at(channel_id, Instant::now())
    }

    fn typing_users_at(&self, channel_id: ChannelId, now: Instant) -> Vec<SharedString> {
        self.typing_by_channel
            .get(&channel_id)
            .map(|users| {
                users
                    .values()
                    .filter(|entry| now.duration_since(entry.at) < TYPING_TTL)
                    .map(|entry| SharedString::from(entry.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
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
            user_status: HashMap::new(),
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
                let statuses: Vec<(UserId, String)> = e
                    .joins
                    .iter()
                    .map(|u| (UserId(u.user_id), u.status.clone().unwrap_or_default()))
                    .collect();
                self.apply_status_presence(&joins, &leaves);
                self.apply_seed_user_status(&statuses);
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
            display_name.to_owned()
        } else if !username.is_empty() {
            username.to_owned()
        } else {
            sender_id.to_string()
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

    pub fn seed_presence(
        &mut self,
        online: &[UserId],
        statuses: &[(UserId, String)],
        cx: &mut Context<Self>,
    ) {
        let changed = self.apply_seed_online(online) | self.apply_seed_user_status(statuses);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_store() -> PresenceStore {
        PresenceStore {
            typing_by_channel: HashMap::new(),
            channel_online: HashMap::new(),
            user_online: HashSet::new(),
            user_status: HashMap::new(),
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
                    name: "Old".to_owned(),
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
}
