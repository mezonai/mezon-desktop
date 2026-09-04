use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{
    AppApi, ConnectionStatus, DIRECTION_BEFORE_TIMESTAMP, INBOX_PAGE_LIMIT, InboxCategory,
    InboxNotification, RealtimeEvent, inbox_notification_from_api,
    inbox_notification_from_channel_mention, is_pending_inbox_notification_id,
};
use mezon_proto::api::ChannelMessage;

use crate::CACHE_TTL;
use crate::badge::BadgeService;
use crate::clan_members::ClanMembersStore;
use crate::ids::ClanId;
use crate::message::MessageCode;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const REALTIME_BUCKET_CAP: usize = (INBOX_PAGE_LIMIT as usize) * 4;

pub const GLOBAL_INBOX_BUCKET_CLAN_ID: &str = "0";

#[derive(Debug, Clone)]
pub enum InboxEvent {
    Updated { clan_id: Option<String> },
}

#[derive(Debug, Clone)]
struct CategoryBucket {
    items: Vec<InboxNotification>,
    last_id: Option<String>,
    loading: bool,
    refresh_pending: bool,
    has_more: bool,
    fetch_generation: u64,
    fetched_at: Option<Instant>,
    server_loaded: bool,
}

impl Default for CategoryBucket {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            last_id: None,
            loading: false,
            refresh_pending: false,
            has_more: true,
            fetch_generation: 0,
            fetched_at: None,
            server_loaded: false,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BucketKey {
    clan_id: String,
    category: InboxCategory,
}

pub struct InboxStore {
    buckets: HashMap<BucketKey, CategoryBucket>,
    active_clan_id: Option<String>,
    active_channel_id: Option<String>,
    topic_by_message: HashMap<String, String>,
    api: Arc<AppApi>,
    reset_generation: u64,
    _conn_watch: Task<()>,
}

struct GlobalInboxStore(Entity<InboxStore>);
impl Global for GlobalInboxStore {}

impl EventEmitter<InboxEvent> for InboxStore {}

impl InboxStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalInboxStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalInboxStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalInboxStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.buckets.clear();
        self.topic_by_message.clear();
        self.active_clan_id = None;
        self.active_channel_id = None;
        self.reset_generation = self.reset_generation.wrapping_add(1);
        cx.emit(InboxEvent::Updated { clan_id: None });
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            buckets: HashMap::new(),
            active_clan_id: None,
            active_channel_id: None,
            topic_by_message: HashMap::new(),
            api,
            reset_generation: 0,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::Notifications, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on(RealtimeKind::ChannelMessage, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
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
                    if this.update(cx, |this, cx| this.refresh_active(cx)).is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn set_active_context(
        &mut self,
        clan_id: Option<String>,
        channel_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let changed = self.active_clan_id != clan_id || self.active_channel_id != channel_id;
        self.active_clan_id = clan_id;
        self.active_channel_id = channel_id;
        if changed {
            cx.notify();
        }
    }

    fn bucket_key(_clan_id: &str, category: InboxCategory) -> BucketKey {
        BucketKey {
            clan_id: GLOBAL_INBOX_BUCKET_CLAN_ID.to_string(),
            category,
        }
    }

    fn emit_updated(&self, cx: &mut Context<Self>) {
        cx.emit(InboxEvent::Updated { clan_id: None });
    }

    pub fn items(&self, _clan_id: &str, category: InboxCategory) -> &[InboxNotification] {
        self.bucket(category)
            .map(|b| b.items.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self, _clan_id: &str, category: InboxCategory) -> bool {
        self.bucket(category).map(|b| b.loading).unwrap_or(false)
    }

    pub fn has_more(&self, _clan_id: &str, category: InboxCategory) -> bool {
        self.bucket(category).map(|b| b.has_more).unwrap_or(true)
    }

    fn bucket(&self, category: InboxCategory) -> Option<&CategoryBucket> {
        self.buckets.get(&Self::bucket_key("", category))
    }

    fn bucket_mut(&mut self, category: InboxCategory) -> &mut CategoryBucket {
        self.buckets
            .entry(Self::bucket_key("", category))
            .or_default()
    }

    fn should_fetch_initial(bucket: Option<&CategoryBucket>) -> bool {
        let Some(bucket) = bucket else {
            return true;
        };
        if bucket.loading {
            return false;
        }
        if !bucket.server_loaded
            && (!bucket.items.is_empty()
                || bucket
                    .items
                    .iter()
                    .any(|item| is_pending_inbox_notification_id(&item.id)))
        {
            return false;
        }
        if !bucket.server_loaded {
            return true;
        }
        if bucket.items.is_empty() {
            return true;
        }
        if bucket.fetched_at.is_some_and(|t| t.elapsed() < CACHE_TTL) {
            return false;
        }
        false
    }

    pub fn fetch_if_empty(
        &mut self,
        clan_id: &str,
        category: InboxCategory,
        cx: &mut Context<Self>,
    ) {
        if !Self::should_fetch_initial(self.bucket(category)) {
            return;
        }
        self.fetch_page(clan_id, category, None, cx);
    }

    fn invalidate(bucket: &mut CategoryBucket) {
        bucket.fetched_at = None;
        bucket.server_loaded = false;
    }

    pub fn refresh_category(&mut self, category: InboxCategory, cx: &mut Context<Self>) {
        Self::invalidate(self.bucket_mut(category));
        self.fetch_page(GLOBAL_INBOX_BUCKET_CLAN_ID, category, None, cx);
    }

    pub fn schedule_refresh_category(&mut self, category: InboxCategory, cx: &mut Context<Self>) {
        if self.bucket(category).is_some_and(|bucket| bucket.loading) {
            self.bucket_mut(category).refresh_pending = true;
            return;
        }
        self.refresh_category(category, cx);
    }

    pub fn fetch_more(&mut self, clan_id: &str, category: InboxCategory, cx: &mut Context<Self>) {
        let Some(last_id) = self.bucket(category).and_then(|b| b.last_id.clone()) else {
            return;
        };
        if !self.has_more(clan_id, category) {
            return;
        }
        self.fetch_page(clan_id, category, Some(last_id), cx);
    }

    fn fetch_page(
        &mut self,
        clan_id: &str,
        category: InboxCategory,
        cursor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let bucket = self.bucket_mut(category);
        if bucket.loading {
            return;
        }
        bucket.loading = true;
        bucket.fetch_generation = bucket.fetch_generation.wrapping_add(1);
        let generation = bucket.fetch_generation;
        let is_first_page = cursor.is_none();
        let notification_id = cursor.unwrap_or_else(|| "0".to_string());
        cx.notify();
        let api = self.api.clone();
        let api_clan_id = clan_id.to_string();
        let reset_gen = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .list_notifications(
                    &api_clan_id,
                    INBOX_PAGE_LIMIT,
                    &notification_id,
                    category as i32,
                    DIRECTION_BEFORE_TIMESTAMP,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != reset_gen {
                    return;
                }
                this.apply_fetch_result(category, generation, result, is_first_page, cx);
                if this
                    .bucket(category)
                    .is_some_and(|bucket| bucket.refresh_pending)
                {
                    this.bucket_mut(category).refresh_pending = false;
                    this.refresh_category(category, cx);
                }
            });
        })
        .detach();
    }

    pub fn prepend_local(
        &mut self,
        _clan_id: &str,
        category: InboxCategory,
        notification: InboxNotification,
        cx: &mut Context<Self>,
    ) -> bool {
        self.remember_topic_id(&notification);
        let bucket = self.bucket_mut(category);
        let incoming_message_id = notification.effective_message_id();
        if let Some(pos) = bucket.items.iter().position(|existing| {
            existing.id == notification.id
                || incoming_message_id.as_deref().is_some_and(|message_id| {
                    existing.effective_message_id().as_deref() == Some(message_id)
                })
        }) {
            let mut existing = bucket.items.remove(pos);
            if existing.topic_id.is_none() {
                existing.topic_id = notification.topic_id.clone();
            }
            if let (Some(preview), Some(incoming_preview)) =
                (existing.message.as_mut(), notification.message.as_ref())
                && preview.topic_id.is_none()
            {
                preview.topic_id = incoming_preview.topic_id.clone();
            }
            bucket.items.insert(0, existing);
            self.emit_updated(cx);
            cx.notify();
            return false;
        }
        bucket.items.insert(0, notification);
        if bucket.items.len() > REALTIME_BUCKET_CAP {
            bucket.items.truncate(REALTIME_BUCKET_CAP);
        }
        self.emit_updated(cx);
        cx.notify();
        true
    }

    pub fn note_mention(&mut self, notification: InboxNotification, cx: &mut Context<Self>) {
        if notification.category != InboxCategory::Mentions {
            return;
        }
        self.prepend_local(
            GLOBAL_INBOX_BUCKET_CLAN_ID,
            InboxCategory::Mentions,
            notification,
            cx,
        );
    }

    fn remember_topic_id(&mut self, notification: &InboxNotification) {
        let Some(message_id) = notification.effective_message_id() else {
            return;
        };
        let Some(topic_id) = notification.effective_topic_id() else {
            return;
        };
        self.topic_by_message.insert(message_id, topic_id);
    }

    pub fn remembered_topic_id(&self, message_id: &str) -> Option<&str> {
        self.topic_by_message.get(message_id).map(String::as_str)
    }

    fn apply_remembered_topic_ids(
        items: &mut [InboxNotification],
        remembered: &HashMap<String, String>,
    ) {
        for item in items {
            if item.effective_topic_id().is_some() {
                continue;
            }
            let Some(message_id) = item.effective_message_id() else {
                continue;
            };
            let Some(topic_id) = remembered.get(&message_id) else {
                continue;
            };
            item.topic_id = Some(topic_id.clone());
            if let Some(preview) = item.message.as_mut() {
                preview.topic_id = Some(topic_id.clone());
            }
        }
    }

    fn drop_pending_duplicates(items: &mut Vec<InboxNotification>, incoming: &[InboxNotification]) {
        let incoming_message_ids: HashSet<String> = incoming
            .iter()
            .filter_map(|n| n.effective_message_id())
            .collect();
        if incoming_message_ids.is_empty() {
            return;
        }
        items.retain(|existing| {
            if !is_pending_inbox_notification_id(&existing.id) {
                return true;
            }
            existing
                .effective_message_id()
                .is_none_or(|message_id| !incoming_message_ids.contains(&message_id))
        });
    }

    fn merge_server_page(
        local: Vec<InboxNotification>,
        fetched: Vec<InboxNotification>,
    ) -> Vec<InboxNotification> {
        let fetched_ids: HashSet<String> = fetched.iter().map(|n| n.id.clone()).collect();
        let fetched_message_ids: HashSet<String> = fetched
            .iter()
            .filter_map(|n| n.effective_message_id())
            .collect();
        let mut merged: Vec<InboxNotification> = local
            .into_iter()
            .filter(|item| {
                !fetched_ids.contains(&item.id)
                    && item
                        .effective_message_id()
                        .is_none_or(|message_id| !fetched_message_ids.contains(&message_id))
            })
            .collect();
        merged.extend(fetched);
        Self::sort_items(&mut merged);
        merged
    }

    fn page_cursor(items: &[InboxNotification]) -> Option<String> {
        items
            .iter()
            .rev()
            .find(|item| !is_pending_inbox_notification_id(&item.id))
            .map(|item| item.id.clone())
    }

    fn sort_items(items: &mut [InboxNotification]) {
        items.sort_by(|a, b| {
            match (
                is_pending_inbox_notification_id(&a.id),
                is_pending_inbox_notification_id(&b.id),
            ) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.create_time_seconds.cmp(&a.create_time_seconds),
            }
        });
    }

    fn apply_fetch_result(
        &mut self,
        category: InboxCategory,
        generation: u64,
        result: Result<Vec<InboxNotification>, anyhow::Error>,
        is_first_page: bool,
        cx: &mut Context<Self>,
    ) {
        let remembered = self.topic_by_message.clone();
        let bucket = self.bucket_mut(category);
        if bucket.fetch_generation != generation {
            return;
        }
        bucket.loading = false;
        match result {
            Ok(items) => {
                bucket.has_more = items.len() >= INBOX_PAGE_LIMIT as usize;
                if is_first_page {
                    let local = std::mem::take(&mut bucket.items);
                    bucket.items = Self::merge_server_page(local, items);
                } else {
                    Self::drop_pending_duplicates(&mut bucket.items, &items);
                    let existing_ids: HashSet<String> =
                        bucket.items.iter().map(|n| n.id.clone()).collect();
                    let existing_message_ids: HashSet<String> = bucket
                        .items
                        .iter()
                        .filter_map(|n| n.effective_message_id())
                        .collect();
                    bucket.items.extend(items.into_iter().filter(|n| {
                        !existing_ids.contains(&n.id)
                            && n.effective_message_id().is_none_or(|message_id| {
                                !existing_message_ids.contains(&message_id)
                            })
                    }));
                    Self::sort_items(&mut bucket.items);
                }
                Self::apply_remembered_topic_ids(&mut bucket.items, &remembered);
                bucket.last_id = Self::page_cursor(&bucket.items);
                bucket.fetched_at = Some(Instant::now());
                bucket.server_loaded = true;
                self.emit_updated(cx);
                cx.notify();
            }
            Err(e) => {
                tracing::error!("list_notifications failed: {e}");
                self.emit_updated(cx);
                cx.notify();
            }
        }
    }

    pub fn delete(
        &mut self,
        _clan_id: &str,
        id: &str,
        category: InboxCategory,
        cx: &mut Context<Self>,
    ) {
        let Some(removed) = self.remove_item(category, id) else {
            return;
        };
        self.emit_updated(cx);
        cx.notify();

        if is_pending_inbox_notification_id(id) {
            return;
        }

        let api = self.api.clone();
        let id = id.to_string();
        let reset_gen = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .delete_notifications(&[id.as_str()], category as i32)
                .await;
            if let Err(e) = result {
                tracing::error!("delete_notifications failed: {e}");
                let _ = this.update(cx, |this, cx| {
                    if this.reset_generation != reset_gen {
                        return;
                    }
                    this.reinsert_item(category, removed);
                    this.emit_updated(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remove_item(&mut self, category: InboxCategory, id: &str) -> Option<InboxNotification> {
        let bucket = self.buckets.get_mut(&Self::bucket_key("", category))?;
        let pos = bucket.items.iter().position(|n| n.id == id)?;
        Some(bucket.items.remove(pos))
    }

    fn reinsert_item(&mut self, category: InboxCategory, item: InboxNotification) {
        let Some(bucket) = self.buckets.get_mut(&Self::bucket_key("", category)) else {
            return;
        };
        if bucket.items.iter().any(|n| n.id == item.id) {
            return;
        }
        bucket.items.push(item);
        bucket
            .items
            .sort_by_key(|n| std::cmp::Reverse(n.create_time_seconds));
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        match event {
            RealtimeEvent::Notifications(batch) => {
                for raw in &batch.notifications {
                    let Ok(notification) = inbox_notification_from_api(raw.clone()) else {
                        continue;
                    };
                    if !self.should_prepend_realtime(&notification) {
                        continue;
                    }
                    let category = notification.category;
                    self.prepend_local(GLOBAL_INBOX_BUCKET_CLAN_ID, category, notification, cx);
                }
            }
            RealtimeEvent::ChannelMessage(message) => {
                self.handle_channel_mention(message, cx);
            }
            _ => {}
        }
    }

    fn handle_channel_mention(&mut self, message: &ChannelMessage, cx: &mut Context<Self>) {
        if !channel_message_is_mention_for_inbox(message, cx) {
            return;
        }
        self.note_mention(inbox_notification_from_channel_mention(message), cx);
    }

    fn should_prepend_realtime(&self, notification: &InboxNotification) -> bool {
        notification.category == InboxCategory::Mentions || !self.should_skip_realtime(notification)
    }

    fn should_skip_realtime(&self, notification: &InboxNotification) -> bool {
        if self.active_clan_id.as_deref() != Some(notification.clan_id.as_str()) {
            return false;
        }
        if let Some(active_channel) = self.active_channel_id.as_deref() {
            active_channel == notification.channel_id
        } else {
            false
        }
    }

    fn mark_all_stale(&mut self) {
        for bucket in self.buckets.values_mut() {
            bucket.fetched_at = None;
        }
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        self.mark_all_stale();
        let categories: Vec<InboxCategory> = self
            .buckets
            .keys()
            .filter(|key| key.clan_id == GLOBAL_INBOX_BUCKET_CLAN_ID)
            .map(|key| key.category)
            .collect();
        for category in categories {
            if Self::should_fetch_initial(self.bucket(category)) {
                self.fetch_page(GLOBAL_INBOX_BUCKET_CLAN_ID, category, None, cx);
            }
        }
    }
}

pub(crate) fn skip_inbox_mention_code(code: i32) -> bool {
    matches!(
        MessageCode::from_raw(code),
        MessageCode::ChatUpdate
            | MessageCode::ChatRemove
            | MessageCode::UpdateEphemeralMsg
            | MessageCode::DeleteEphemeralMsg
            | MessageCode::CreatePin
            | MessageCode::Typing
            | MessageCode::Indicator
            | MessageCode::Welcome
    )
}

fn channel_message_is_mention_for_inbox(message: &ChannelMessage, cx: &App) -> bool {
    if skip_inbox_mention_code(message.code) {
        return false;
    }
    let Some(user_id) =
        BadgeService::try_global(cx).and_then(|svc| svc.read(cx).current_user_id(cx))
    else {
        return false;
    };
    if user_id.get() == message.sender_id {
        return false;
    }
    let clan_id = ClanId(message.clan_id);
    let role_ids: Vec<i64> = ClanMembersStore::try_global(cx)
        .map(|members| {
            let members = members.read(cx);
            members
                .self_role_ids(clan_id)
                .map(<[i64]>::to_vec)
                .or_else(|| {
                    members
                        .member(clan_id, user_id)
                        .map(|member| member.role_ids.iter().map(|role| role.get()).collect())
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    mezon_client::transport::is_mention_or_reply(
        &message.content,
        &message.references,
        &message.mentions,
        user_id.get(),
        &role_ids,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notification(channel_id: &str) -> InboxNotification {
        InboxNotification {
            id: "1".into(),
            category: InboxCategory::Mentions,
            subject: String::new(),
            sender_id: String::new(),
            clan_id: "1".into(),
            channel_id: channel_id.into(),
            topic_id: None,
            channel_type: 1,
            avatar_url: String::new(),
            create_time_seconds: 0,
            code: 0,
            message: None,
        }
    }

    #[test]
    fn should_skip_when_viewing_same_channel() {
        let store = InboxStore {
            buckets: HashMap::new(),
            active_clan_id: Some("1".into()),
            active_channel_id: Some("7".into()),
            topic_by_message: HashMap::new(),
            api: Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            )),
            reset_generation: 0,
            _conn_watch: Task::ready(()),
        };
        assert!(store.should_skip_realtime(&sample_notification("7")));
        assert!(!store.should_skip_realtime(&sample_notification("8")));
    }

    #[test]
    fn mentions_prepend_even_when_viewing_same_channel() {
        let store = InboxStore {
            buckets: HashMap::new(),
            active_clan_id: Some("1".into()),
            active_channel_id: Some("7".into()),
            topic_by_message: HashMap::new(),
            api: Arc::new(AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            )),
            reset_generation: 0,
            _conn_watch: Task::ready(()),
        };
        let mention = sample_notification("7");
        assert!(store.should_skip_realtime(&mention));
        assert!(store.should_prepend_realtime(&mention));
    }

    #[test]
    fn schedule_refresh_queues_when_already_loading() {
        let mut bucket = CategoryBucket {
            loading: true,
            ..CategoryBucket::default()
        };
        assert!(!bucket.refresh_pending);
        bucket.refresh_pending = true;
        assert!(bucket.refresh_pending);
    }

    #[test]
    fn skip_initial_fetch_when_realtime_items_are_pending() {
        let local_only = CategoryBucket {
            items: vec![InboxNotification {
                id: "pending-42".into(),
                ..sample_notification("7")
            }],
            ..CategoryBucket::default()
        };
        assert!(!InboxStore::should_fetch_initial(Some(&local_only)));
        assert!(InboxStore::should_fetch_initial(None));
    }

    #[test]
    fn skip_initial_fetch_when_socket_items_exist_before_server_page() {
        let local_only = CategoryBucket {
            items: vec![InboxNotification {
                id: "99".into(),
                ..sample_notification("7")
            }],
            ..CategoryBucket::default()
        };
        assert!(!InboxStore::should_fetch_initial(Some(&local_only)));
    }

    #[test]
    fn fetch_initial_when_bucket_is_empty_and_not_server_loaded() {
        let empty = CategoryBucket::default();
        assert!(InboxStore::should_fetch_initial(Some(&empty)));
    }

    #[test]
    fn pending_inbox_items_sort_ahead_of_older_server_items() {
        let mut items = vec![
            InboxNotification {
                id: "10".into(),
                create_time_seconds: 50,
                ..sample_notification("7")
            },
            InboxNotification {
                id: "pending-42".into(),
                create_time_seconds: 1,
                ..sample_notification("7")
            },
        ];
        InboxStore::sort_items(&mut items);
        assert_eq!(items[0].id, "pending-42");
        assert_eq!(items[1].id, "10");
    }

    #[test]
    fn invalidating_a_category_re_arms_the_initial_fetch() {
        let mut loaded = CategoryBucket {
            items: vec![sample_notification("7")],
            last_id: Some("1".into()),
            fetched_at: Some(Instant::now()),
            server_loaded: true,
            ..CategoryBucket::default()
        };
        assert!(!InboxStore::should_fetch_initial(Some(&loaded)));
        InboxStore::invalidate(&mut loaded);
        assert!(!InboxStore::should_fetch_initial(Some(&loaded)));
        loaded.items.clear();
        assert!(InboxStore::should_fetch_initial(Some(&loaded)));
    }

    #[test]
    fn page_cursor_never_pages_from_an_optimistic_item() {
        let items = vec![
            InboxNotification {
                id: "pending-42".into(),
                ..sample_notification("7")
            },
            InboxNotification {
                id: "20".into(),
                ..sample_notification("7")
            },
            InboxNotification {
                id: "10".into(),
                ..sample_notification("7")
            },
        ];
        assert_eq!(InboxStore::page_cursor(&items).as_deref(), Some("10"));
        assert_eq!(InboxStore::page_cursor(&items[..1]), None);
    }

    #[test]
    fn refetch_when_server_loaded_but_bucket_empty() {
        let empty_loaded = CategoryBucket {
            fetched_at: Some(Instant::now()),
            server_loaded: true,
            ..CategoryBucket::default()
        };
        assert!(InboxStore::should_fetch_initial(Some(&empty_loaded)));
    }

    #[test]
    fn skip_refetch_when_server_page_fresh_and_nonempty() {
        let loaded = CategoryBucket {
            items: vec![sample_notification("7")],
            last_id: Some("1".into()),
            fetched_at: Some(Instant::now()),
            server_loaded: true,
            ..CategoryBucket::default()
        };
        assert!(!InboxStore::should_fetch_initial(Some(&loaded)));
    }

    fn notification_with_message(id: &str, message_id: &str, seconds: u32) -> InboxNotification {
        InboxNotification {
            id: id.into(),
            create_time_seconds: seconds,
            message: Some(mezon_client::InboxMessagePreview {
                message_id: message_id.into(),
                channel_id: "7".into(),
                clan_id: "1".into(),
                sender_id: String::new(),
                content: String::new(),
                raw_content: String::new(),
                avatar: String::new(),
                display_name: String::new(),
                username: String::new(),
                create_time_seconds: seconds,
                attachment_link: String::new(),
                attachment_type: String::new(),
                attachment_filename: String::new(),
                attachment_size: 0,
                attachment_thumbnail: String::new(),
                has_more_attachment: false,
                mention_spans: Vec::new(),
                topic_id: None,
            }),
            ..sample_notification("7")
        }
    }

    #[test]
    fn first_page_keeps_local_mention_when_server_page_is_stale() {
        let pending = notification_with_message("pending-99", "99", 200);
        let older = notification_with_message("10", "10", 50);
        let merged = InboxStore::merge_server_page(vec![pending], vec![older]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "pending-99");
        assert_eq!(merged[1].id, "10");
    }

    #[test]
    fn first_page_replaces_pending_when_server_has_same_message() {
        let pending = notification_with_message("pending-99", "99", 200);
        let saved = notification_with_message("500", "99", 200);
        let older = notification_with_message("10", "10", 50);
        let merged = InboxStore::merge_server_page(vec![pending], vec![saved, older]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "500");
        assert_eq!(merged[1].id, "10");
    }

    #[test]
    fn first_page_keeps_local_mention_when_server_returns_empty() {
        let pending = notification_with_message("pending-99", "99", 200);
        let merged = InboxStore::merge_server_page(vec![pending], Vec::new());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "pending-99");
    }

    #[test]
    fn skip_inbox_mention_ignores_control_codes() {
        assert!(skip_inbox_mention_code(3));
        assert!(skip_inbox_mention_code(1));
        assert!(!skip_inbox_mention_code(0));
        assert!(!skip_inbox_mention_code(9));
    }
}
