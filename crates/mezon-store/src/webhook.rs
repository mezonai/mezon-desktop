use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::api;

use crate::AppConfig;
use crate::KeyedCache;
use crate::clan::upload_image_to_cdn;
use crate::ids::{ChannelId, ClanId, UserId};
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const MAX_CACHED_CLANS: usize = 16;
pub const WEBHOOK_NAME_MAX_LENGTH: usize = 64;
pub const MAX_WEBHOOK_AVATAR_BYTES: u64 = 1024 * 1024;
const WEBHOOK_STATUS_DELETE: i32 = 3;

#[derive(Clone, PartialEq, Eq)]
pub struct ChannelWebhook {
    pub id: String,
    pub webhook_name: String,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub url: String,
    pub avatar: String,
    pub creator_id: UserId,
    pub create_time_seconds: i64,
}

impl std::fmt::Debug for ChannelWebhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelWebhook")
            .field("id", &self.id)
            .field("webhook_name", &self.webhook_name)
            .field("channel_id", &self.channel_id)
            .field("clan_id", &self.clan_id)
            .field("url", &"<redacted>")
            .field("avatar", &self.avatar)
            .field("creator_id", &self.creator_id)
            .field("create_time_seconds", &self.create_time_seconds)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ClanWebhook {
    pub id: String,
    pub webhook_name: String,
    pub clan_id: ClanId,
    pub url: String,
    pub avatar: String,
    pub creator_id: UserId,
    pub create_time_seconds: i64,
}

impl std::fmt::Debug for ClanWebhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClanWebhook")
            .field("id", &self.id)
            .field("webhook_name", &self.webhook_name)
            .field("clan_id", &self.clan_id)
            .field("url", &"<redacted>")
            .field("avatar", &self.avatar)
            .field("creator_id", &self.creator_id)
            .field("create_time_seconds", &self.create_time_seconds)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum WebhookEvent {
    ChannelWebhooksChanged { clan_id: ClanId },
    ClanWebhooksChanged { clan_id: ClanId },
}

#[derive(Debug, Default)]
struct ClanChannelWebhooks {
    webhooks: Vec<ChannelWebhook>,
}

#[derive(Debug, Default)]
struct ClanClanWebhooks {
    webhooks: Vec<ClanWebhook>,
}

pub struct WebhookStore {
    channel_cache: KeyedCache<ClanId, ClanChannelWebhooks>,
    clan_cache: KeyedCache<ClanId, ClanClanWebhooks>,
    channel_loading: HashSet<ClanId>,
    clan_loading: HashSet<ClanId>,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalWebhookStore(Entity<WebhookStore>);
impl Global for GlobalWebhookStore {}

impl EventEmitter<WebhookEvent> for WebhookStore {}

impl WebhookStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalWebhookStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalWebhookStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalWebhookStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.channel_cache.clear();
        self.clan_cache.clear();
        self.channel_loading.clear();
        self.clan_loading.clear();
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);
        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);
        Self {
            channel_cache: KeyedCache::new(Some(MAX_CACHED_CLANS)),
            clan_cache: KeyedCache::new(Some(MAX_CACHED_CLANS)),
            channel_loading: HashSet::new(),
            clan_loading: HashSet::new(),
            api,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::Webhook, &entity, |this, event, cx| {
                this.handle_event(event, cx);
            });
            dispatch.on_lagged(&entity, |this, cx| this.refresh_after_lag(cx));
        });
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::Webhook(proto) = event else {
            return;
        };
        let clan_id = ClanId(proto.clan_id);
        if clan_id.is_zero() || !self.channel_cache.contains(&clan_id) {
            return;
        }
        let Some(entry) = self.channel_cache.get_mut(&clan_id) else {
            return;
        };
        if apply_channel_webhook_event(&mut entry.webhooks, proto.clone(), clan_id) {
            cx.emit(WebhookEvent::ChannelWebhooksChanged { clan_id });
            cx.notify();
        }
    }

    fn refresh_after_lag(&mut self, cx: &mut Context<Self>) {
        self.invalidate();
        let channel_clan_ids: Vec<ClanId> = self.channel_cache.iter().map(|(id, _)| *id).collect();
        for clan_id in channel_clan_ids {
            self.fetch_channel_webhooks(clan_id, cx);
        }
        let clan_webhook_ids: Vec<ClanId> = self.clan_cache.iter().map(|(id, _)| *id).collect();
        for clan_id in clan_webhook_ids {
            self.fetch_clan_webhooks(clan_id, cx);
        }
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
        self.channel_cache.mark_all_stale();
        self.clan_cache.mark_all_stale();
    }

    pub fn ensure_channel_webhooks_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.channel_cache.touch(&clan_id);
        if !self.channel_cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            self.fetch_channel_webhooks(clan_id, cx);
        }
    }

    pub fn ensure_clan_webhooks_loaded(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.clan_cache.touch(&clan_id);
        if !self.clan_cache.is_fresh(&clan_id, crate::CACHE_TTL) {
            self.fetch_clan_webhooks(clan_id, cx);
        }
    }

    pub fn refresh_channel_webhooks(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetch_channel_webhooks(clan_id, cx);
    }

    pub fn refresh_clan_webhooks(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        self.fetch_clan_webhooks(clan_id, cx);
    }

    pub fn channel_webhooks_loading(&self, clan_id: ClanId) -> bool {
        self.channel_loading.contains(&clan_id)
    }

    pub fn clan_webhooks_loading(&self, clan_id: ClanId) -> bool {
        self.clan_loading.contains(&clan_id)
    }

    pub fn channel_webhooks_for_clan(&self, clan_id: ClanId) -> &[ChannelWebhook] {
        self.channel_cache
            .get(&clan_id)
            .map(|entry| entry.webhooks.as_slice())
            .unwrap_or(&[])
    }

    pub fn channel_webhooks_for_channel(
        &self,
        clan_id: ClanId,
        channel_id: ChannelId,
    ) -> Vec<&ChannelWebhook> {
        webhooks_for_channel(self.channel_webhooks_for_clan(clan_id), channel_id)
    }

    pub fn clan_webhooks_for_clan(&self, clan_id: ClanId) -> &[ClanWebhook] {
        self.clan_cache
            .get(&clan_id)
            .map(|entry| entry.webhooks.as_slice())
            .unwrap_or(&[])
    }

    fn fetch_channel_webhooks(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.channel_loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_webhooks_by_channel(0, clan_id.get())
                .await
                .map(|webhooks| {
                    webhooks
                        .into_iter()
                        .filter_map(|proto| channel_webhook_from_proto(proto, clan_id))
                        .collect()
                });
            let _ = this.update(cx, |this, cx| {
                this.channel_loading.remove(&clan_id);
                match result {
                    Ok(webhooks) => {
                        this.channel_cache
                            .insert(clan_id, ClanChannelWebhooks { webhooks }, None);
                        cx.emit(WebhookEvent::ChannelWebhooksChanged { clan_id });
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!("list_webhooks_by_channel failed for {clan_id}: {err}");
                    }
                }
            });
        })
        .detach();
    }

    fn fetch_clan_webhooks(&mut self, clan_id: ClanId, cx: &mut Context<Self>) {
        if !self.clan_loading.insert(clan_id) {
            return;
        }
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_clan_webhooks(clan_id.get()).await.map(|webhooks| {
                webhooks
                    .into_iter()
                    .filter_map(clan_webhook_from_proto)
                    .collect()
            });
            let _ = this.update(cx, |this, cx| {
                this.clan_loading.remove(&clan_id);
                match result {
                    Ok(webhooks) => {
                        this.clan_cache
                            .insert(clan_id, ClanClanWebhooks { webhooks }, None);
                        cx.emit(WebhookEvent::ClanWebhooksChanged { clan_id });
                        cx.notify();
                    }
                    Err(err) => {
                        tracing::error!("list_clan_webhooks failed for {clan_id}: {err}");
                    }
                }
            });
        })
        .detach();
    }

    pub fn create_channel_webhook(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        webhook_name: String,
        avatar: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = api::WebhookCreateRequest {
                webhook_name,
                channel_id: channel_id.get(),
                clan_id: clan_id.get(),
                avatar,
            };
            api.generate_webhook(request)
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_channel_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn update_channel_webhook(
        &mut self,
        webhook: &ChannelWebhook,
        webhook_name: String,
        avatar: String,
        channel_id_update: Option<ChannelId>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let id = webhook.id.parse::<i64>().unwrap_or(0);
        if id == 0 {
            return cx.spawn(async move |_, _| Err("invalid webhook id".into()));
        }
        let api = self.api.clone();
        let clan_id = webhook.clan_id;
        let channel_id = webhook.channel_id;
        cx.spawn(async move |this, cx| {
            let request = api::WebhookUpdateRequestById {
                id,
                webhook_name,
                avatar,
                channel_id: channel_id.get(),
                channel_id_update: channel_id_update
                    .map(|id| id.get())
                    .unwrap_or_else(|| channel_id.get()),
                clan_id: clan_id.get(),
            };
            api.update_webhook(request)
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_channel_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn delete_channel_webhook(
        &mut self,
        webhook: &ChannelWebhook,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let id = webhook.id.parse::<i64>().unwrap_or(0);
        if id == 0 {
            return cx.spawn(async move |_, _| Err("invalid webhook id".into()));
        }
        let api = self.api.clone();
        let clan_id = webhook.clan_id;
        let channel_id = webhook.channel_id;
        cx.spawn(async move |this, cx| {
            let request = api::WebhookDeleteRequestById {
                id,
                clan_id: clan_id.get(),
                channel_id: channel_id.get(),
            };
            api.delete_webhook(request)
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_channel_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn create_clan_webhook(
        &mut self,
        clan_id: ClanId,
        webhook_name: String,
        avatar: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let request = api::GenerateClanWebhookRequest {
                clan_id: clan_id.get(),
                webhook_name,
                avatar,
            };
            api.generate_clan_webhook(request)
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_clan_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn update_clan_webhook(
        &mut self,
        webhook: &ClanWebhook,
        webhook_name: String,
        avatar: String,
        reset_token: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let id = webhook.id.parse::<i64>().unwrap_or(0);
        if id == 0 {
            return cx.spawn(async move |_, _| Err("invalid webhook id".into()));
        }
        let api = self.api.clone();
        let clan_id = webhook.clan_id;
        cx.spawn(async move |this, cx| {
            let request = api::UpdateClanWebhookRequest {
                id,
                clan_id: clan_id.get(),
                webhook_name,
                avatar,
                reset_token,
            };
            api.update_clan_webhook(request)
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_clan_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn upload_webhook_avatar(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<String, String>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let base_img_url = AppConfig::global(cx).base_img_url.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .spawn(async move {
                    upload_image_to_cdn(&api, &base_img_url, &path, MAX_WEBHOOK_AVATAR_BYTES).await
                })
                .await
        })
    }

    pub fn delete_clan_webhook(
        &mut self,
        webhook: &ClanWebhook,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let id = webhook.id.parse::<i64>().unwrap_or(0);
        if id == 0 {
            return cx.spawn(async move |_, _| Err("invalid webhook id".into()));
        }
        let api = self.api.clone();
        let clan_id = webhook.clan_id;
        cx.spawn(async move |this, cx| {
            api.delete_clan_webhook(id, clan_id.get())
                .await
                .map_err(|err| err.to_string())?;
            this.update(cx, |this, cx| {
                this.refresh_clan_webhooks(clan_id, cx);
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }
}

pub fn webhook_name_is_valid(name: &str) -> bool {
    let len = name.trim().chars().count();
    len > 0 && len <= WEBHOOK_NAME_MAX_LENGTH
}

fn webhooks_for_channel(
    webhooks: &[ChannelWebhook],
    channel_id: ChannelId,
) -> Vec<&ChannelWebhook> {
    webhooks
        .iter()
        .filter(|webhook| webhook.channel_id == channel_id)
        .collect()
}

fn apply_channel_webhook_event(
    webhooks: &mut Vec<ChannelWebhook>,
    proto: api::Webhook,
    fallback_clan_id: ClanId,
) -> bool {
    if proto.status == WEBHOOK_STATUS_DELETE {
        let id = proto.id.to_string();
        let before = webhooks.len();
        webhooks.retain(|webhook| webhook.id != id);
        return webhooks.len() != before;
    }
    let Some(mapped) = channel_webhook_from_proto(proto, fallback_clan_id) else {
        return false;
    };
    if let Some(existing) = webhooks.iter_mut().find(|webhook| webhook.id == mapped.id) {
        *existing = mapped;
    } else {
        webhooks.push(mapped);
    }
    true
}

fn channel_webhook_from_proto(proto: api::Webhook, clan_id: ClanId) -> Option<ChannelWebhook> {
    if proto.id == 0 {
        return None;
    }
    Some(ChannelWebhook {
        id: proto.id.to_string(),
        webhook_name: proto.webhook_name,
        channel_id: ChannelId(proto.channel_id),
        clan_id: if proto.clan_id != 0 {
            ClanId(proto.clan_id)
        } else {
            clan_id
        },
        url: proto.url,
        avatar: proto.avatar,
        creator_id: UserId(proto.creator_id),
        create_time_seconds: webhook_create_time_seconds(&proto.create_time),
    })
}

fn clan_webhook_from_proto(proto: api::ClanWebhook) -> Option<ClanWebhook> {
    if proto.id == 0 {
        return None;
    }
    Some(ClanWebhook {
        id: proto.id.to_string(),
        webhook_name: proto.webhook_name,
        clan_id: ClanId(proto.clan_id),
        url: proto.url,
        avatar: proto.avatar,
        creator_id: UserId(proto.creator_id),
        create_time_seconds: webhook_create_time_seconds(&proto.create_time),
    })
}

fn webhook_create_time_seconds(create_time: &str) -> i64 {
    if create_time.is_empty() {
        return 0;
    }
    chrono::DateTime::parse_from_rfc3339(create_time)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_webhook(
        id: i64,
        channel_id: i64,
        clan_id: i64,
        name: &str,
        status: i32,
    ) -> api::Webhook {
        api::Webhook {
            id,
            webhook_name: name.into(),
            channel_id,
            url: "https://secret.example/hooks/token".into(),
            creator_id: 9,
            create_time: "2024-01-15T12:00:00Z".into(),
            avatar: "https://cdn.example/avatar.png".into(),
            status,
            clan_id,
            ..Default::default()
        }
    }

    fn sample(id: &str, channel_id: i64, clan_id: i64) -> ChannelWebhook {
        ChannelWebhook {
            id: id.into(),
            webhook_name: "Captain hook".into(),
            channel_id: ChannelId(channel_id),
            clan_id: ClanId(clan_id),
            url: "https://secret.example/hooks/token".into(),
            avatar: String::new(),
            creator_id: UserId(1),
            create_time_seconds: 1_700_000_000,
        }
    }

    #[test]
    fn proto_mapping_rejects_zero_id() {
        assert!(
            channel_webhook_from_proto(proto_webhook(0, 10, 1, "hook", 1), ClanId(1)).is_none()
        );
    }

    #[test]
    fn proto_mapping_uses_fallback_clan_when_proto_clan_is_zero() {
        let webhook =
            channel_webhook_from_proto(proto_webhook(7, 10, 0, "Captain hook", 1), ClanId(42))
                .expect("mapped");
        assert_eq!(webhook.id, "7");
        assert_eq!(webhook.webhook_name, "Captain hook");
        assert_eq!(webhook.channel_id, ChannelId(10));
        assert_eq!(webhook.clan_id, ClanId(42));
        assert_eq!(webhook.creator_id, UserId(9));
        assert_eq!(webhook.create_time_seconds, 1_705_320_000);
        assert_eq!(webhook.url, "https://secret.example/hooks/token");
    }

    #[test]
    fn channel_webhook_debug_redacts_url() {
        let debug = format!("{:?}", sample("1", 10, 1));
        assert!(debug.contains("url: \"<redacted>\""));
        assert!(!debug.contains("secret.example"));
    }

    #[test]
    fn filter_keeps_only_matching_channel() {
        let webhooks = vec![sample("1", 10, 1), sample("2", 20, 1), sample("3", 10, 1)];
        let filtered = webhooks_for_channel(&webhooks, ChannelId(10));
        assert_eq!(
            filtered
                .iter()
                .map(|webhook| webhook.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "3"]
        );
        assert!(webhooks_for_channel(&webhooks, ChannelId(99)).is_empty());
    }

    #[test]
    fn webhook_name_rejects_empty_blank_and_too_long() {
        assert!(!webhook_name_is_valid(""));
        assert!(!webhook_name_is_valid("   "));
        assert!(!webhook_name_is_valid(
            &"a".repeat(WEBHOOK_NAME_MAX_LENGTH + 1)
        ));
        assert!(webhook_name_is_valid("Captain hook"));
        assert!(webhook_name_is_valid(&"a".repeat(WEBHOOK_NAME_MAX_LENGTH)));
        assert!(webhook_name_is_valid("  trimmed  "));
    }

    #[test]
    fn realtime_create_and_update_upsert_in_place() {
        let mut webhooks = Vec::new();
        assert!(apply_channel_webhook_event(
            &mut webhooks,
            proto_webhook(7, 10, 1, "Captain hook", 1),
            ClanId(1),
        ));
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].webhook_name, "Captain hook");

        assert!(apply_channel_webhook_event(
            &mut webhooks,
            proto_webhook(7, 20, 1, "Spidey bot", 2),
            ClanId(1),
        ));
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].webhook_name, "Spidey bot");
        assert_eq!(webhooks[0].channel_id, ChannelId(20));
    }

    #[test]
    fn realtime_delete_removes_existing_and_ignores_missing() {
        let mut webhooks = vec![sample("7", 10, 1), sample("8", 10, 1)];
        assert!(apply_channel_webhook_event(
            &mut webhooks,
            proto_webhook(7, 10, 1, "Captain hook", WEBHOOK_STATUS_DELETE),
            ClanId(1),
        ));
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].id, "8");
        assert!(!apply_channel_webhook_event(
            &mut webhooks,
            proto_webhook(7, 10, 1, "Captain hook", WEBHOOK_STATUS_DELETE),
            ClanId(1),
        ));
    }

    #[test]
    fn realtime_create_with_zero_id_is_ignored() {
        let mut webhooks = Vec::new();
        assert!(!apply_channel_webhook_event(
            &mut webhooks,
            proto_webhook(0, 10, 1, "Captain hook", 1),
            ClanId(1),
        ));
        assert!(webhooks.is_empty());
    }
}
