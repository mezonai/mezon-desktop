use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{
    AppApi, ConnectionStatus, DIRECTION_BEFORE_TIMESTAMP, INBOX_PAGE_LIMIT, InboxCategory,
    InboxNotification, RealtimeEvent, inbox_notification_from_api,
    is_pending_inbox_notification_id,
};

use crate::CACHE_TTL;
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
        if !bucket.server_loaded {
            return true;
        }
        if bucket.fetched_at.is_some_and(|t| t.elapsed() < CACHE_TTL) {
            return false;
        }
        bucket.items.is_empty()
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
                this.apply_fetch_result(category, generation, result, cx);
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
    ) {
        let bucket = self.bucket_mut(category);
        if bucket.items.iter().any(|n| n.id == notification.id) {
            return;
        }
        if let Some(message_id) = notification.effective_message_id()
            && bucket.items.iter().any(|existing| {
                existing.effective_message_id().as_deref() == Some(message_id.as_str())
            })
        {
            return;
        }
        bucket.items.insert(0, notification);
        if bucket.items.len() > REALTIME_BUCKET_CAP {
            bucket.items.truncate(REALTIME_BUCKET_CAP);
        }
        self.emit_updated(cx);
        cx.notify();
    }

    fn drop_pending_duplicates(items: &mut Vec<InboxNotification>, incoming: &[InboxNotification]) {
        let incoming_message_ids: std::collections::HashSet<String> = incoming
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

    fn apply_fetch_result(
        &mut self,
        category: InboxCategory,
        generation: u64,
        result: Result<Vec<InboxNotification>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        let bucket = self.bucket_mut(category);
        if bucket.fetch_generation != generation {
            return;
        }
        bucket.loading = false;
        match result {
            Ok(items) => {
                bucket.has_more = items.len() >= INBOX_PAGE_LIMIT as usize;
                if bucket.items.is_empty() {
                    bucket.items = items;
                } else {
                    Self::drop_pending_duplicates(&mut bucket.items, &items);
                    let existing: std::collections::HashSet<String> =
                        bucket.items.iter().map(|n| n.id.clone()).collect();
                    bucket
                        .items
                        .extend(items.into_iter().filter(|n| !existing.contains(&n.id)));
                }
                bucket
                    .items
                    .sort_by_key(|n| std::cmp::Reverse(n.create_time_seconds));
                bucket.last_id = bucket.items.last().map(|n| n.id.clone());
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
        let RealtimeEvent::Notifications(batch) = event else {
            return;
        };
        let mut changed = false;
        for raw in &batch.notifications {
            let Ok(notification) = inbox_notification_from_api(raw.clone()) else {
                continue;
            };
            if self.should_skip_realtime(&notification) {
                continue;
            }
            let Some(bucket) = self
                .buckets
                .get_mut(&Self::bucket_key("", notification.category))
            else {
                continue;
            };
            if bucket.fetched_at.is_none() {
                continue;
            }
            if bucket.items.iter().any(|n| n.id == notification.id) {
                continue;
            }
            bucket.items.insert(0, notification);
            if bucket.items.len() > REALTIME_BUCKET_CAP {
                bucket.items.truncate(REALTIME_BUCKET_CAP);
            }
            changed = true;
        }
        if changed {
            self.emit_updated(cx);
            cx.notify();
        }
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
            self.fetch_page(GLOBAL_INBOX_BUCKET_CLAN_ID, category, None, cx);
        }
    }
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
    fn fetch_initial_after_local_prepend_without_server_page() {
        let local_only = CategoryBucket {
            items: vec![sample_notification("7")],
            ..CategoryBucket::default()
        };
        assert!(InboxStore::should_fetch_initial(Some(&local_only)));
        assert!(InboxStore::should_fetch_initial(None));
    }

    #[test]
    fn skip_initial_fetch_when_server_page_is_fresh() {
        let loaded = CategoryBucket {
            items: vec![sample_notification("7")],
            last_id: Some("1".into()),
            fetched_at: Some(Instant::now()),
            server_loaded: true,
            ..CategoryBucket::default()
        };
        assert!(!InboxStore::should_fetch_initial(Some(&loaded)));
    }
}
