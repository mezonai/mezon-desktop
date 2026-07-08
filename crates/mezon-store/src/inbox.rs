use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{
    AppApi, ConnectionStatus, DIRECTION_BEFORE_TIMESTAMP, INBOX_PAGE_LIMIT, InboxCategory,
    InboxNotification, RealtimeEvent, inbox_notification_from_api,
};

use crate::CACHE_TTL;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const REALTIME_BUCKET_CAP: usize = (INBOX_PAGE_LIMIT as usize) * 4;

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

    pub fn items(&self, clan_id: &str, category: InboxCategory) -> &[InboxNotification] {
        self.bucket(clan_id, category)
            .map(|b| b.items.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self, clan_id: &str, category: InboxCategory) -> bool {
        self.bucket(clan_id, category)
            .map(|b| b.loading)
            .unwrap_or(false)
    }

    pub fn has_more(&self, clan_id: &str, category: InboxCategory) -> bool {
        self.bucket(clan_id, category)
            .map(|b| b.has_more)
            .unwrap_or(true)
    }

    fn bucket(&self, clan_id: &str, category: InboxCategory) -> Option<&CategoryBucket> {
        self.buckets.get(&BucketKey {
            clan_id: clan_id.to_string(),
            category,
        })
    }

    fn bucket_mut(&mut self, clan_id: &str, category: InboxCategory) -> &mut CategoryBucket {
        self.buckets
            .entry(BucketKey {
                clan_id: clan_id.to_string(),
                category,
            })
            .or_default()
    }

    fn is_fresh(&self, clan_id: &str, category: InboxCategory) -> bool {
        self.bucket(clan_id, category)
            .and_then(|b| b.fetched_at)
            .is_some_and(|t| t.elapsed() < CACHE_TTL)
    }

    pub fn fetch_if_empty(
        &mut self,
        clan_id: &str,
        category: InboxCategory,
        cx: &mut Context<Self>,
    ) {
        if self.is_fresh(clan_id, category) {
            return;
        }
        let never_fetched = self
            .bucket(clan_id, category)
            .is_none_or(|b| b.fetched_at.is_none());
        if !never_fetched {
            let empty = self
                .bucket(clan_id, category)
                .is_none_or(|b| b.items.is_empty());
            if !empty {
                return;
            }
        }
        self.fetch_page(clan_id, category, None, cx);
    }

    pub fn fetch_more(&mut self, clan_id: &str, category: InboxCategory, cx: &mut Context<Self>) {
        let Some(last_id) = self
            .bucket(clan_id, category)
            .and_then(|b| b.last_id.clone())
        else {
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
        let bucket = self.bucket_mut(clan_id, category);
        if bucket.loading {
            return;
        }
        bucket.loading = true;
        bucket.fetch_generation = bucket.fetch_generation.wrapping_add(1);
        let generation = bucket.fetch_generation;
        let notification_id = cursor.unwrap_or_else(|| "0".to_string());
        cx.notify();

        let api = self.api.clone();
        let clan_id = clan_id.to_string();
        let reset_gen = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api
                .list_notifications(
                    &clan_id,
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
                this.apply_fetch_result(&clan_id, category, generation, result, cx);
            });
        })
        .detach();
    }

    fn apply_fetch_result(
        &mut self,
        clan_id: &str,
        category: InboxCategory,
        generation: u64,
        result: Result<Vec<InboxNotification>, anyhow::Error>,
        cx: &mut Context<Self>,
    ) {
        let bucket = self.bucket_mut(clan_id, category);
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
                    let existing: std::collections::HashSet<String> =
                        bucket.items.iter().map(|n| n.id.clone()).collect();
                    bucket
                        .items
                        .extend(items.into_iter().filter(|n| !existing.contains(&n.id)));
                    bucket
                        .items
                        .sort_by_key(|n| std::cmp::Reverse(n.create_time_seconds));
                }
                bucket.last_id = bucket.items.last().map(|n| n.id.clone());
                bucket.fetched_at = Some(Instant::now());
                cx.emit(InboxEvent::Updated {
                    clan_id: Some(clan_id.to_string()),
                });
                cx.notify();
            }
            Err(e) => {
                tracing::error!("list_notifications failed: {e}");
                cx.emit(InboxEvent::Updated {
                    clan_id: Some(clan_id.to_string()),
                });
                cx.notify();
            }
        }
    }

    pub fn delete(
        &mut self,
        clan_id: &str,
        id: &str,
        category: InboxCategory,
        cx: &mut Context<Self>,
    ) {
        let Some(removed) = self.remove_item(clan_id, category, id) else {
            return;
        };
        cx.emit(InboxEvent::Updated {
            clan_id: Some(clan_id.to_string()),
        });
        cx.notify();

        let api = self.api.clone();
        let id = id.to_string();
        let clan_id = clan_id.to_string();
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
                    this.reinsert_item(&clan_id, category, removed);
                    cx.emit(InboxEvent::Updated {
                        clan_id: Some(clan_id.clone()),
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remove_item(
        &mut self,
        clan_id: &str,
        category: InboxCategory,
        id: &str,
    ) -> Option<InboxNotification> {
        let key = BucketKey {
            clan_id: clan_id.to_string(),
            category,
        };
        let bucket = self.buckets.get_mut(&key)?;
        let pos = bucket.items.iter().position(|n| n.id == id)?;
        Some(bucket.items.remove(pos))
    }

    fn reinsert_item(&mut self, clan_id: &str, category: InboxCategory, item: InboxNotification) {
        let key = BucketKey {
            clan_id: clan_id.to_string(),
            category,
        };
        let Some(bucket) = self.buckets.get_mut(&key) else {
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
        let mut changed_clans: std::collections::HashSet<String> = std::collections::HashSet::new();
        for raw in &batch.notifications {
            let Ok(notification) = inbox_notification_from_api(raw.clone()) else {
                continue;
            };
            if self.should_skip_realtime(&notification) {
                continue;
            }
            let Some(clan_id) = self.realtime_bucket_clan_id(&notification) else {
                continue;
            };
            let key = BucketKey {
                clan_id: clan_id.clone(),
                category: notification.category,
            };
            let Some(bucket) = self.buckets.get_mut(&key) else {
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
            changed_clans.insert(clan_id);
        }
        if !changed_clans.is_empty() {
            for clan_id in changed_clans {
                cx.emit(InboxEvent::Updated {
                    clan_id: Some(clan_id),
                });
            }
            cx.notify();
        }
    }

    fn realtime_bucket_clan_id(&self, notification: &InboxNotification) -> Option<String> {
        notification
            .effective_clan_id()
            .or_else(|| self.active_clan_id.clone())
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
        let Some(active_clan_id) = self.active_clan_id.clone() else {
            return;
        };
        let categories: Vec<InboxCategory> = self
            .buckets
            .keys()
            .filter(|key| key.clan_id == active_clan_id)
            .map(|key| key.category)
            .collect();
        for category in categories {
            self.fetch_page(&active_clan_id, category, None, cx);
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
}
