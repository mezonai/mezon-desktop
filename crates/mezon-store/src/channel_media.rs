use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use mezon_client::{AppApi, UploadFile};
use mezon_proto::api;
use prost::Message;

use crate::AppConfig;
use crate::KeyedCache;
use crate::ids::{ChannelId, ClanId, UserId};

pub const CHANNEL_MEDIA_CACHE_TTL: Duration = Duration::from_secs(60 * 20);
pub const CHANNEL_MEDIA_PAGE_SIZE: i32 = 50;
const MAX_CACHED_BUCKETS: usize = 64;
const MAX_DETAIL_CACHE: usize = 32;
const PENDING_EVENT_ID: i64 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    channel_id: ChannelId,
    year: i32,
}

#[derive(Debug, Clone)]
pub struct ChannelTimelineAttachment {
    pub id: i64,
    pub file_name: String,
    pub file_url: String,
    pub file_type: String,
    pub file_size: i64,
    pub thumbnail: String,
    pub width: i32,
    pub height: i32,
    pub duration: i32,
    pub message_id: i64,
}

impl ChannelTimelineAttachment {
    fn from_proto(a: api::ChannelTimelineAttachment) -> Self {
        Self {
            id: a.id,
            file_name: a.file_name,
            file_url: a.file_url,
            file_type: a.file_type,
            file_size: a.file_size,
            thumbnail: a.thumbnail,
            width: a.width,
            height: a.height,
            duration: a.duration,
            message_id: a.message_id,
        }
    }

    pub fn to_proto(&self) -> api::ChannelTimelineAttachment {
        api::ChannelTimelineAttachment {
            id: self.id,
            file_name: self.file_name.clone(),
            file_url: self.file_url.clone(),
            file_type: self.file_type.clone(),
            file_size: self.file_size,
            width: self.width,
            height: self.height,
            thumbnail: self.thumbnail.clone(),
            duration: self.duration,
            message_id: self.message_id,
        }
    }

    pub fn display_url(&self) -> &str {
        if !self.thumbnail.is_empty() {
            &self.thumbnail
        } else {
            &self.file_url
        }
    }

    pub fn is_uploaded(&self) -> bool {
        self.file_url.starts_with("https")
    }
}

#[derive(Debug, Clone)]
pub struct ChannelTimeline {
    pub id: i64,
    pub clan_id: ClanId,
    pub channel_id: ChannelId,
    pub start_time_seconds: u32,
    pub title: String,
    pub description: String,
    pub end_time_seconds: u32,
    pub location: String,
    pub status: i32,
    pub event_type: i32,
    pub creator_id: UserId,
    pub create_time_seconds: u32,
    pub update_time_seconds: u32,
    pub preview_imgs: Vec<ChannelTimelineAttachment>,
    pub attachments: Vec<ChannelTimelineAttachment>,
}

impl ChannelTimeline {
    pub fn from_proto(event: api::ChannelTimeline) -> Self {
        Self {
            id: event.id,
            clan_id: ClanId(event.clan_id),
            channel_id: ChannelId(event.channel_id),
            start_time_seconds: event.start_time_seconds,
            title: event.title,
            description: event.description,
            end_time_seconds: event.end_time_seconds,
            location: event.location,
            status: event.status,
            event_type: event.r#type,
            creator_id: UserId(event.creator_id),
            create_time_seconds: event.create_time_seconds,
            update_time_seconds: event.update_time_seconds,
            preview_imgs: parse_timeline_attachment_bytes(&event.preview_imgs),
            attachments: parse_timeline_attachment_bytes(&event.attachments),
        }
    }

    pub fn is_special(&self) -> bool {
        self.event_type == 1
    }

    pub fn preview_urls(&self) -> Vec<&str> {
        self.preview_imgs
            .iter()
            .map(|a| a.display_url())
            .filter(|url| !url.is_empty())
            .collect()
    }

    pub fn media_attachments(&self) -> Vec<ChannelTimelineAttachment> {
        merge_attachment_lists(&self.attachments, &self.preview_imgs)
            .into_iter()
            .filter(|attachment| !attachment.display_url().is_empty())
            .collect()
    }
}

fn merge_attachment_lists(
    primary: &[ChannelTimelineAttachment],
    secondary: &[ChannelTimelineAttachment],
) -> Vec<ChannelTimelineAttachment> {
    if primary.is_empty() {
        return secondary.to_vec();
    }
    let mut merged = primary.to_vec();
    for attachment in secondary {
        if !merged
            .iter()
            .any(|existing| attachments_same(existing, attachment))
        {
            merged.push(attachment.clone());
        }
    }
    merged
}

fn attachments_same(a: &ChannelTimelineAttachment, b: &ChannelTimelineAttachment) -> bool {
    if a.id != 0 && b.id != 0 {
        return a.id == b.id;
    }
    if !a.file_url.is_empty() && !b.file_url.is_empty() {
        return a.file_url == b.file_url;
    }
    !a.thumbnail.is_empty() && a.thumbnail == b.thumbnail
}

fn merged_event_attachments(
    existing: &ChannelTimeline,
    incoming: &ChannelTimeline,
) -> Vec<ChannelTimelineAttachment> {
    let primary = if !incoming.attachments.is_empty() {
        incoming.attachments.as_slice()
    } else if !existing.attachments.is_empty() {
        existing.attachments.as_slice()
    } else {
        &[]
    };
    let mut merged = primary.to_vec();
    for attachment in existing
        .attachments
        .iter()
        .chain(&incoming.preview_imgs)
        .chain(&existing.preview_imgs)
    {
        if !merged
            .iter()
            .any(|existing_attachment| attachments_same(existing_attachment, attachment))
        {
            merged.push(attachment.clone());
        }
    }
    merged
}

fn parse_timeline_attachment_bytes(bytes: &[u8]) -> Vec<ChannelTimelineAttachment> {
    if bytes.is_empty() {
        return Vec::new();
    }
    match api::ListChannelTimelineAttachment::decode(bytes) {
        Ok(list) => list
            .attachments
            .into_iter()
            .map(ChannelTimelineAttachment::from_proto)
            .collect(),
        Err(e) => {
            tracing::warn!(
                "failed to decode channel timeline attachments ({} bytes): {e}",
                bytes.len()
            );
            Vec::new()
        }
    }
}

fn sort_events_newest_first(events: &mut [ChannelTimeline]) {
    events.sort_by_key(|e| std::cmp::Reverse(e.start_time_seconds));
}

pub fn first_event_year(events: &[ChannelTimeline]) -> Option<String> {
    use chrono::{Datelike, TimeZone};
    events
        .iter()
        .filter(|e| e.start_time_seconds > 0)
        .min_by_key(|e| e.start_time_seconds)
        .map(|e| {
            chrono::Utc
                .timestamp_opt(e.start_time_seconds as i64, 0)
                .single()
                .map(|dt| dt.year().to_string())
                .unwrap_or_default()
        })
        .filter(|y| !y.is_empty())
}

pub fn timeline_card_position(index: usize) -> TimelineCardSide {
    if index.is_multiple_of(2) {
        TimelineCardSide::Left
    } else {
        TimelineCardSide::Right
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineCardSide {
    Left,
    Right,
}

#[derive(Default, Clone)]
struct ChannelMediaBucket {
    events: Vec<ChannelTimeline>,
    is_loading: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMediaEvent {
    Changed(ChannelId),
}

#[derive(Debug, Clone)]
struct DetailCacheEntry {
    event: ChannelTimeline,
    fetched_at: Instant,
}

impl DetailCacheEntry {
    fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() < CHANNEL_MEDIA_CACHE_TTL
    }
}

#[derive(Debug, Clone)]
pub struct CreateTimelineInput {
    pub title: String,
    pub description: String,
    pub start_time_seconds: u32,
    pub end_time_seconds: u32,
    pub attachments: Vec<ChannelTimelineAttachment>,
}

#[derive(Debug, Clone)]
pub struct UpdateTimelineInput {
    pub id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub start_time_seconds: u32,
    pub attachments: Option<Vec<ChannelTimelineAttachment>>,
}

pub struct ChannelMediaStore {
    by_key: KeyedCache<CacheKey, ChannelMediaBucket>,
    active_key: Option<CacheKey>,
    detail_cache: HashMap<i64, DetailCacheEntry>,
    detail_loading: HashMap<i64, ChannelId>,
    detail_errors: HashMap<i64, String>,
    api: Arc<AppApi>,
}

struct GlobalChannelMediaStore(Entity<ChannelMediaStore>);
impl Global for GlobalChannelMediaStore {}

impl EventEmitter<ChannelMediaEvent> for ChannelMediaStore {}

impl ChannelMediaStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            by_key: KeyedCache::new(Some(MAX_CACHED_BUCKETS)),
            active_key: None,
            detail_cache: HashMap::new(),
            detail_loading: HashMap::new(),
            detail_errors: HashMap::new(),
            api,
        });
        cx.set_global(GlobalChannelMediaStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChannelMediaStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChannelMediaStore>()
            .map(|g| g.0.clone())
    }

    pub fn events(&self, channel_id: ChannelId, year: i32) -> &[ChannelTimeline] {
        self.by_key
            .get(&CacheKey { channel_id, year })
            .map(|b| b.events.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_loading(&self, channel_id: ChannelId, year: i32) -> bool {
        self.by_key
            .get(&CacheKey { channel_id, year })
            .is_some_and(|b| b.is_loading)
    }

    pub fn error(&self, channel_id: ChannelId, year: i32) -> Option<&str> {
        self.by_key
            .get(&CacheKey { channel_id, year })
            .and_then(|b| b.error.as_deref())
    }

    pub fn detail_error(&self, event_id: i64) -> Option<&str> {
        self.detail_errors.get(&event_id).map(String::as_str)
    }

    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        year: i32,
        cx: &mut Context<Self>,
    ) {
        let key = CacheKey { channel_id, year };
        self.active_key = Some(key.clone());
        let needs_fetch = match self.by_key.get(&key) {
            Some(bucket) => {
                !bucket.is_loading
                    && (!self.by_key.is_fresh(&key, CHANNEL_MEDIA_CACHE_TTL)
                        || bucket.error.is_some())
            }
            None => true,
        };
        if needs_fetch {
            self.fetch(clan_id, channel_id, year, false, cx);
        } else {
            self.by_key.touch(&key);
        }
    }

    pub fn refresh_if_stale(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        year: i32,
        cx: &mut Context<Self>,
    ) {
        let key = CacheKey { channel_id, year };
        self.active_key = Some(key.clone());
        let stale = self
            .by_key
            .get(&key)
            .is_some_and(|_| !self.by_key.is_fresh(&key, CHANNEL_MEDIA_CACHE_TTL));
        if stale {
            self.fetch(clan_id, channel_id, year, false, cx);
        } else if self.by_key.contains(&key) {
            self.by_key.touch(&key);
        }
    }

    pub fn refresh_on_focus(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        years: &[i32],
        cx: &mut Context<Self>,
    ) {
        for year in years {
            self.refresh_if_stale(clan_id, channel_id, *year, cx);
        }
    }

    pub fn fetch(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        year: i32,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let key = CacheKey { channel_id, year };
        self.active_key = Some(key.clone());
        if !force {
            if let Some(bucket) = self.by_key.get(&key)
                && bucket.is_loading
            {
                return;
            }
            if self.by_key.is_fresh(&key, CHANNEL_MEDIA_CACHE_TTL) && self.by_key.contains(&key) {
                self.by_key.touch(&key);
                return;
            }
        }

        if self.by_key.contains(&key) {
            self.by_key.mark_stale(&key);
        }
        if let Some(bucket) = self.by_key.get_mut(&key) {
            bucket.is_loading = true;
            bucket.error = None;
        } else {
            let bucket = ChannelMediaBucket {
                is_loading: true,
                ..Default::default()
            };
            self.by_key
                .insert(key.clone(), bucket, self.active_key.as_ref());
        }
        cx.emit(ChannelMediaEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_timeline(clan_id.0, channel_id.0, year, CHANNEL_MEDIA_PAGE_SIZE)
                .await;

            this.update(cx, |store, cx| {
                if let Some(bucket) = store.by_key.get_mut(&key) {
                    bucket.is_loading = false;
                    match result {
                        Ok(response) => {
                            let mut events: Vec<ChannelTimeline> = response
                                .events
                                .into_iter()
                                .map(ChannelTimeline::from_proto)
                                .collect();
                            sort_events_newest_first(&mut events);
                            bucket.events = events;
                            bucket.error = None;
                        }
                        Err(e) => {
                            bucket.error = Some(e.to_string());
                        }
                    }
                }
                store.by_key.mark_fetched(&key);
                cx.emit(ChannelMediaEvent::Changed(channel_id));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        let keys: Vec<CacheKey> = self
            .by_key
            .iter()
            .filter_map(|(key, _)| (key.channel_id == channel_id).then_some(key.clone()))
            .collect();
        if keys.is_empty()
            && !self
                .detail_cache
                .values()
                .any(|entry| entry.event.channel_id == channel_id)
        {
            return;
        }
        for key in keys {
            self.by_key.remove(&key);
        }
        self.detail_cache
            .retain(|_, entry| entry.event.channel_id != channel_id);
        self.detail_loading
            .retain(|_, loading_channel| *loading_channel != channel_id);
        self.detail_errors
            .retain(|id, _| self.detail_cache.contains_key(id));
        if self
            .active_key
            .as_ref()
            .is_some_and(|k| k.channel_id == channel_id)
        {
            self.active_key = None;
        }
        cx.emit(ChannelMediaEvent::Changed(channel_id));
        cx.notify();
    }

    pub fn event_detail(&self, event_id: i64) -> Option<&ChannelTimeline> {
        self.detail_cache.get(&event_id).map(|entry| &entry.event)
    }

    pub fn has_detail(&self, event_id: i64) -> bool {
        self.detail_cache.contains_key(&event_id)
    }

    pub fn is_detail_loading(&self, event_id: i64) -> bool {
        self.detail_loading.contains_key(&event_id)
    }

    pub fn patch_event_detail(&mut self, event: ChannelTimeline, cx: &mut Context<Self>) {
        let channel_id = event.channel_id;
        let merged = self
            .detail_cache
            .get(&event.id)
            .map(|existing| merge_timeline_event(&existing.event, &event))
            .unwrap_or(event);
        self.detail_cache.insert(
            merged.id,
            DetailCacheEntry {
                event: merged,
                fetched_at: Instant::now(),
            },
        );
        self.trim_detail_cache();
        cx.emit(ChannelMediaEvent::Changed(channel_id));
        cx.notify();
    }

    pub fn fetch_detail(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        event_id: i64,
        start_time_seconds: u32,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if !force {
            if let Some(entry) = self.detail_cache.get(&event_id)
                && entry.is_fresh()
            {
                return;
            }
            if self.detail_loading.contains_key(&event_id) {
                return;
            }
        }

        self.detail_loading.insert(event_id, channel_id);
        cx.emit(ChannelMediaEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .detail_channel_timeline(clan_id.0, channel_id.0, event_id, start_time_seconds)
                .await;

            this.update(cx, |store, cx| {
                store.detail_loading.remove(&event_id);
                match result {
                    Ok(response) => {
                        store.detail_errors.remove(&event_id);
                        if let Some(event) = response.event {
                            let mapped = ChannelTimeline::from_proto(event);
                            store.patch_event_detail(mapped, cx);
                        }
                    }
                    Err(e) => {
                        store.detail_errors.insert(event_id, e.to_string());
                        cx.emit(ChannelMediaEvent::Changed(channel_id));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn create_timeline(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        input: CreateTimelineInput,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<ChannelTimeline, String>> {
        let year = event_year(input.start_time_seconds);
        let pending = pending_timeline_event(clan_id, channel_id, &input);
        self.insert_event_sorted(channel_id, year, pending);
        cx.emit(ChannelMediaEvent::Changed(channel_id));
        cx.notify();

        let req = api::CreateChannelTimelineRequest {
            clan_id: clan_id.0,
            channel_id: channel_id.0,
            start_time_seconds: input.start_time_seconds,
            title: input.title,
            description: input.description,
            end_time_seconds: input.end_time_seconds,
            location: String::new(),
            r#type: 0,
            attachments: input.attachments.iter().map(|a| a.to_proto()).collect(),
        };
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.create_channel_timeline(req).await;
            this.update(cx, |store, cx| {
                store.remove_event_by_id(channel_id, year, PENDING_EVENT_ID);
                match result {
                    Ok(response) => {
                        if let Some(event) = response.event {
                            let mapped = ChannelTimeline::from_proto(event);
                            store.insert_event_sorted(channel_id, year, mapped.clone());
                            store.patch_event_detail(mapped.clone(), cx);
                            Ok(mapped)
                        } else {
                            cx.emit(ChannelMediaEvent::Changed(channel_id));
                            cx.notify();
                            Err("CreateChannelTimeline returned empty event".into())
                        }
                    }
                    Err(e) => {
                        cx.emit(ChannelMediaEvent::Changed(channel_id));
                        cx.notify();
                        Err(e.to_string())
                    }
                }
            })
            .unwrap_or(Err("store dropped".into()))
        })
    }

    pub fn update_timeline(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        input: UpdateTimelineInput,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<ChannelTimeline, String>> {
        let existing = self.detail_cache.get(&input.id).map(|e| e.event.clone());
        let title = input
            .title
            .or_else(|| existing.as_ref().map(|e| e.title.clone()))
            .unwrap_or_default();
        let description = input
            .description
            .or_else(|| existing.as_ref().map(|e| e.description.clone()))
            .unwrap_or_default();
        let req = api::UpdateChannelTimelineRequest {
            clan_id: clan_id.0,
            channel_id: channel_id.0,
            id: input.id,
            start_time_seconds: input.start_time_seconds,
            title,
            description,
            location: String::new(),
            r#type: existing.as_ref().map(|e| e.event_type).unwrap_or(0),
            attachments: input
                .attachments
                .unwrap_or_default()
                .iter()
                .map(|a| a.to_proto())
                .collect(),
        };
        let api = self.api.clone();
        let year = event_year(input.start_time_seconds);
        cx.spawn(async move |this, cx| {
            let result = api.update_channel_timeline(req).await;
            this.update(cx, |store, cx| match result {
                Ok(response) => {
                    if let Some(event) = response.event {
                        let mapped = ChannelTimeline::from_proto(event);
                        store.update_event_in_lists(channel_id, year, &mapped);
                        store.patch_event_detail(mapped.clone(), cx);
                        Ok(mapped)
                    } else {
                        Err("UpdateChannelTimeline returned empty event".into())
                    }
                }
                Err(e) => Err(e.to_string()),
            })
            .unwrap_or(Err("store dropped".into()))
        })
    }

    pub fn upload_attachment(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<ChannelTimelineAttachment, String>> {
        let path = path.to_path_buf();
        let api = self.api.clone();
        let base_img = AppConfig::global(cx).base_img_url.clone();
        cx.spawn(async move |_this, _cx| upload_timeline_file(&api, &base_img, &path).await)
    }

    fn insert_event_sorted(&mut self, channel_id: ChannelId, year: i32, event: ChannelTimeline) {
        let key = CacheKey { channel_id, year };
        self.active_key = Some(key.clone());
        let mut bucket = self.by_key.get(&key).cloned().unwrap_or_default();
        let insert_at = bucket
            .events
            .iter()
            .position(|e| e.start_time_seconds < event.start_time_seconds)
            .unwrap_or(bucket.events.len());
        if bucket.events.iter().any(|e| e.id == event.id) {
            if let Some(existing) = bucket.events.iter_mut().find(|e| e.id == event.id) {
                *existing = event;
            }
        } else {
            bucket.events.insert(insert_at, event);
        }
        self.by_key.insert(key, bucket, self.active_key.as_ref());
    }

    fn remove_event_by_id(&mut self, channel_id: ChannelId, year: i32, event_id: i64) {
        let key = CacheKey { channel_id, year };
        let Some(bucket) = self.by_key.get_mut(&key) else {
            return;
        };
        bucket.events.retain(|e| e.id != event_id);
    }

    fn update_event_in_lists(&mut self, channel_id: ChannelId, year: i32, event: &ChannelTimeline) {
        let key = CacheKey { channel_id, year };
        if let Some(bucket) = self.by_key.get_mut(&key)
            && let Some(existing) = bucket.events.iter_mut().find(|e| e.id == event.id)
        {
            *existing = merge_timeline_event(existing, event);
            sort_events_newest_first(&mut bucket.events);
        }
    }

    fn trim_detail_cache(&mut self) {
        while self.detail_cache.len() > MAX_DETAIL_CACHE {
            let oldest = self
                .detail_cache
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at)
                .map(|(id, _)| *id);
            if let Some(id) = oldest {
                self.detail_cache.remove(&id);
                self.detail_errors.remove(&id);
            } else {
                break;
            }
        }
    }
}

fn merge_timeline_event(existing: &ChannelTimeline, incoming: &ChannelTimeline) -> ChannelTimeline {
    ChannelTimeline {
        id: incoming.id,
        clan_id: incoming.clan_id,
        channel_id: incoming.channel_id,
        start_time_seconds: incoming.start_time_seconds,
        title: if incoming.title.is_empty() {
            existing.title.clone()
        } else {
            incoming.title.clone()
        },
        description: if incoming.description.is_empty() && !existing.description.is_empty() {
            existing.description.clone()
        } else {
            incoming.description.clone()
        },
        end_time_seconds: incoming.end_time_seconds,
        location: incoming.location.clone(),
        status: incoming.status,
        event_type: incoming.event_type,
        creator_id: incoming.creator_id,
        create_time_seconds: incoming.create_time_seconds,
        update_time_seconds: incoming.update_time_seconds,
        preview_imgs: if incoming.preview_imgs.is_empty() {
            existing.preview_imgs.clone()
        } else {
            incoming.preview_imgs.clone()
        },
        attachments: merged_event_attachments(existing, incoming),
    }
}

fn event_year(start_time_seconds: u32) -> i32 {
    use chrono::{Datelike, Local, TimeZone};
    Local
        .timestamp_opt(start_time_seconds as i64, 0)
        .single()
        .map(|dt| dt.year())
        .unwrap_or_else(|| Local::now().year())
}

fn pending_timeline_event(
    clan_id: ClanId,
    channel_id: ChannelId,
    input: &CreateTimelineInput,
) -> ChannelTimeline {
    ChannelTimeline {
        id: PENDING_EVENT_ID,
        clan_id,
        channel_id,
        start_time_seconds: input.start_time_seconds,
        title: input.title.clone(),
        description: input.description.clone(),
        end_time_seconds: input.end_time_seconds,
        location: String::new(),
        status: 0,
        event_type: 0,
        creator_id: UserId(0),
        create_time_seconds: 0,
        update_time_seconds: 0,
        preview_imgs: input.attachments.clone(),
        attachments: input.attachments.clone(),
    }
}

fn mime_from_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg".into(),
        "png" => "image/png".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "mp4" => "video/mp4".into(),
        "webm" => "video/webm".into(),
        "mov" => "video/quicktime".into(),
        _ => "application/octet-stream".into(),
    }
}

fn image_dimensions_from_path(path: &Path) -> (i32, i32) {
    image::image_dimensions(path)
        .map(|(w, h)| (i32::try_from(w).unwrap_or(0), i32::try_from(h).unwrap_or(0)))
        .unwrap_or((0, 0))
}

async fn upload_timeline_file(
    api: &AppApi,
    _base_img_url: &str,
    path: &Path,
) -> Result<ChannelTimelineAttachment, String> {
    let filetype = mime_from_extension(path);
    if !filetype.starts_with("image/") && !filetype.starts_with("video/") {
        return Err("Unsupported file type".into());
    }
    let path_buf = path.to_path_buf();
    let file_size = mezon_client::transport_runtime::file_len(path_buf.clone())
        .await
        .map_err(|e| e.to_string())?;
    if file_size == 0 {
        return Err("File is empty".into());
    }
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string();
    let (width, height) = if filetype.starts_with("image/") {
        let path_for_dims = path_buf.clone();
        mezon_client::transport_runtime::handle()
            .spawn_blocking(move || image_dimensions_from_path(&path_for_dims))
            .await
            .map_err(|e| e.to_string())?
    } else {
        (0, 0)
    };
    let presigned = api
        .presign_file(UploadFile {
            path: path_buf,
            filename,
            filetype: filetype.clone(),
            width,
            height,
            duration: 0,
            thumbnail: None,
        })
        .await
        .map_err(|e| e.to_string())?;
    let att = presigned.attachment.clone();
    api.execute_upload(presigned)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ChannelTimelineAttachment {
        id: 0,
        file_name: att.filename,
        file_url: att.url,
        file_type: filetype,
        file_size: i64::from(att.size),
        thumbnail: att.thumbnail,
        width: att.width,
        height: att.height,
        duration: att.duration,
        message_id: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_attachment(id: i64, url: &str, thumb: &str) -> api::ChannelTimelineAttachment {
        api::ChannelTimelineAttachment {
            id,
            file_name: format!("file{id}.jpg"),
            file_url: url.to_string(),
            file_type: "image/jpeg".to_string(),
            file_size: 1024,
            width: 100,
            height: 100,
            thumbnail: thumb.to_string(),
            duration: 0,
            message_id: 42,
        }
    }

    fn encode_attachments(atts: Vec<api::ChannelTimelineAttachment>) -> Vec<u8> {
        api::ListChannelTimelineAttachment { attachments: atts }.encode_to_vec()
    }

    fn timeline_event(id: i64, start_time_seconds: u32) -> ChannelTimeline {
        ChannelTimeline {
            id,
            clan_id: ClanId(1),
            channel_id: ChannelId(1),
            start_time_seconds,
            title: String::new(),
            description: String::new(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            event_type: 0,
            creator_id: UserId(1),
            create_time_seconds: 0,
            update_time_seconds: 0,
            preview_imgs: Vec::new(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn from_proto_maps_fields_and_decodes_preview_imgs() {
        let preview_bytes = encode_attachments(vec![sample_attachment(
            1,
            "https://cdn.example/a.jpg",
            "https://cdn.example/a-thumb.jpg",
        )]);
        let event = api::ChannelTimeline {
            id: 99,
            clan_id: 10,
            channel_id: 20,
            start_time_seconds: 1_704_067_200,
            title: "Launch".to_string(),
            description: "We shipped".to_string(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            creator_id: 5,
            create_time_seconds: 1,
            update_time_seconds: 2,
            r#type: 1,
            attachments: Vec::new(),
            preview_imgs: preview_bytes,
        };

        let mapped = ChannelTimeline::from_proto(event);
        assert_eq!(mapped.id, 99);
        assert_eq!(mapped.clan_id, ClanId(10));
        assert_eq!(mapped.channel_id, ChannelId(20));
        assert!(mapped.is_special());
        assert_eq!(mapped.preview_imgs.len(), 1);
        assert_eq!(
            mapped.preview_imgs[0].display_url(),
            "https://cdn.example/a-thumb.jpg"
        );
    }

    #[test]
    fn parse_timeline_attachment_bytes_empty_is_ok() {
        assert!(parse_timeline_attachment_bytes(&[]).is_empty());
    }

    #[test]
    fn timeline_card_position_alternates() {
        assert_eq!(timeline_card_position(0), TimelineCardSide::Left);
        assert_eq!(timeline_card_position(1), TimelineCardSide::Right);
        assert_eq!(timeline_card_position(2), TimelineCardSide::Left);
    }

    #[test]
    fn first_event_year_picks_oldest() {
        let events = vec![
            ChannelTimeline {
                id: 1,
                clan_id: ClanId(1),
                channel_id: ChannelId(1),
                start_time_seconds: 1_704_067_200,
                title: String::new(),
                description: String::new(),
                end_time_seconds: 0,
                location: String::new(),
                status: 0,
                event_type: 0,
                creator_id: UserId(1),
                create_time_seconds: 0,
                update_time_seconds: 0,
                preview_imgs: Vec::new(),
                attachments: Vec::new(),
            },
            ChannelTimeline {
                id: 2,
                clan_id: ClanId(1),
                channel_id: ChannelId(1),
                start_time_seconds: 1_577_836_800,
                title: String::new(),
                description: String::new(),
                end_time_seconds: 0,
                location: String::new(),
                status: 0,
                event_type: 0,
                creator_id: UserId(1),
                create_time_seconds: 0,
                update_time_seconds: 0,
                preview_imgs: Vec::new(),
                attachments: Vec::new(),
            },
        ];
        assert_eq!(first_event_year(&events).as_deref(), Some("2020"));
    }

    #[test]
    fn sort_events_newest_first_orders_recent_to_oldest() {
        let mut events = vec![
            timeline_event(1, 1_577_836_800),
            timeline_event(2, 1_704_067_200),
            timeline_event(3, 1_640_995_200),
        ];
        sort_events_newest_first(&mut events);
        let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn sort_events_newest_first_keeps_server_order_for_same_start_time() {
        let mut events = vec![
            timeline_event(1, 1_704_067_200),
            timeline_event(2, 1_704_067_200),
        ];
        sort_events_newest_first(&mut events);
        let ids: Vec<i64> = events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn merge_timeline_event_preserves_existing_when_incoming_empty() {
        let existing = ChannelTimeline {
            id: 1,
            clan_id: ClanId(1),
            channel_id: ChannelId(1),
            start_time_seconds: 100,
            title: "Old title".into(),
            description: "Old desc".into(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            event_type: 0,
            creator_id: UserId(1),
            create_time_seconds: 0,
            update_time_seconds: 0,
            preview_imgs: vec![ChannelTimelineAttachment {
                id: 1,
                file_name: "a.jpg".into(),
                file_url: "https://cdn/a.jpg".into(),
                file_type: "image/jpeg".into(),
                file_size: 1,
                thumbnail: String::new(),
                width: 1,
                height: 1,
                duration: 0,
                message_id: 0,
            }],
            attachments: Vec::new(),
        };
        let incoming = ChannelTimeline {
            title: String::new(),
            description: String::new(),
            preview_imgs: Vec::new(),
            ..existing.clone()
        };
        let merged = merge_timeline_event(&existing, &incoming);
        assert_eq!(merged.title, "Old title");
        assert_eq!(merged.description, "Old desc");
        assert_eq!(merged.preview_imgs.len(), 1);
        assert_eq!(merged.attachments.len(), 1);
    }

    #[test]
    fn media_attachments_merges_preview_imgs_when_both_present() {
        let preview = ChannelTimelineAttachment {
            id: 1,
            file_name: "preview.jpg".into(),
            file_url: "https://cdn/preview.jpg".into(),
            file_type: "image/jpeg".into(),
            file_size: 1,
            thumbnail: String::new(),
            width: 1,
            height: 1,
            duration: 0,
            message_id: 0,
        };
        let attachment = ChannelTimelineAttachment {
            id: 2,
            file_name: "full.jpg".into(),
            file_url: "https://cdn/full.jpg".into(),
            file_type: "image/jpeg".into(),
            file_size: 1,
            thumbnail: String::new(),
            width: 1,
            height: 1,
            duration: 0,
            message_id: 0,
        };
        let event = ChannelTimeline {
            id: 1,
            clan_id: ClanId(1),
            channel_id: ChannelId(1),
            start_time_seconds: 100,
            title: String::new(),
            description: String::new(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            event_type: 0,
            creator_id: UserId(1),
            create_time_seconds: 0,
            update_time_seconds: 0,
            preview_imgs: vec![preview.clone()],
            attachments: vec![attachment.clone()],
        };
        assert_eq!(event.media_attachments().len(), 2);
        assert_eq!(event.media_attachments()[0].file_url, attachment.file_url);
        assert_eq!(event.media_attachments()[1].file_url, preview.file_url);
    }

    #[test]
    fn merge_timeline_event_unions_preview_only_attachments() {
        let existing = ChannelTimeline {
            id: 1,
            clan_id: ClanId(1),
            channel_id: ChannelId(1),
            start_time_seconds: 100,
            title: "Old title".into(),
            description: String::new(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            event_type: 0,
            creator_id: UserId(1),
            create_time_seconds: 0,
            update_time_seconds: 0,
            preview_imgs: vec![
                sample_domain_attachment(1, "https://cdn/a.jpg"),
                sample_domain_attachment(2, "https://cdn/b.jpg"),
            ],
            attachments: Vec::new(),
        };
        let incoming = ChannelTimeline {
            attachments: vec![sample_domain_attachment(3, "https://cdn/c.jpg")],
            preview_imgs: Vec::new(),
            ..existing.clone()
        };
        let merged = merge_timeline_event(&existing, &incoming);
        assert_eq!(merged.attachments.len(), 3);
    }

    fn sample_domain_attachment(id: i64, url: &str) -> ChannelTimelineAttachment {
        ChannelTimelineAttachment {
            id,
            file_name: format!("file{id}.jpg"),
            file_url: url.to_string(),
            file_type: "image/jpeg".into(),
            file_size: 1,
            thumbnail: String::new(),
            width: 1,
            height: 1,
            duration: 0,
            message_id: 0,
        }
    }

    #[test]
    fn media_attachments_falls_back_to_preview_imgs() {
        let preview = ChannelTimelineAttachment {
            id: 1,
            file_name: "preview.jpg".into(),
            file_url: "https://cdn/preview.jpg".into(),
            file_type: "image/jpeg".into(),
            file_size: 1,
            thumbnail: String::new(),
            width: 1,
            height: 1,
            duration: 0,
            message_id: 0,
        };
        let event = ChannelTimeline {
            id: 1,
            clan_id: ClanId(1),
            channel_id: ChannelId(1),
            start_time_seconds: 100,
            title: String::new(),
            description: String::new(),
            end_time_seconds: 0,
            location: String::new(),
            status: 0,
            event_type: 0,
            creator_id: UserId(1),
            create_time_seconds: 0,
            update_time_seconds: 0,
            preview_imgs: vec![preview.clone()],
            attachments: Vec::new(),
        };
        assert_eq!(event.media_attachments().len(), 1);
        assert_eq!(event.media_attachments()[0].file_url, preview.file_url);
    }

    #[test]
    fn pending_timeline_event_uses_zero_id() {
        let input = CreateTimelineInput {
            title: "New".into(),
            description: "Story".into(),
            start_time_seconds: 1_704_067_200,
            end_time_seconds: 0,
            attachments: Vec::new(),
        };
        let pending = pending_timeline_event(ClanId(1), ChannelId(2), &input);
        assert_eq!(pending.id, PENDING_EVENT_ID);
        assert_eq!(pending.title, "New");
        assert_eq!(pending.channel_id, ChannelId(2));
    }
}
