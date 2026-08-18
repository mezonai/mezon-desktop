use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use image::ImageEncoder as _;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::badge::BadgeService;
use crate::ids::ClanId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const RECENT_EMOJI_CAP: usize = 20;
const RECENT_FETCH_ATTEMPTS: u32 = 3;
const RECENT_FETCH_RETRY_DELAY: Duration = Duration::from_secs(1);
const LAG_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);
const EMOJI_ACTION_CREATED: i32 = 1;
const EMOJI_ACTION_UPDATE: i32 = 2;
const EMOJI_ACTION_DELETE: i32 = 3;

pub const MAX_EMOJI_BYTES: u64 = 256 * 1024;
pub const MAX_STICKER_BYTES: u64 = 512 * 1024;
pub const EMOJI_UPLOAD_MAX_PX: u32 = 128;
pub const STICKER_UPLOAD_MAX_PX: u32 = 320;
pub const EMOTICON_SHORTNAME_MIN: usize = 3;
pub const EMOTICON_SHORTNAME_MAX: usize = 64;
pub const EMOJI_SHORTNAME_MAX: usize = EMOTICON_SHORTNAME_MAX - 2;
pub const EMOTICON_ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];
const EMOTICON_SOURCE_MAX_PX: u32 = 4096;
const EMOTICON_DECODE_MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
const EMOTICON_SIZE_LIMIT_ERROR: &str = "size_limit";
const EMOTICON_IMAGE_TOO_LARGE_ERROR: &str = "image_too_large";
const EMOTICON_INVALID_NAME_ERROR: &str = "invalid_name";
const EMOTICON_UNSUPPORTED_TYPE_ERROR: &str = "unsupported_type";
const EMOTICON_EMPTY_ERROR: &str = "empty";
const EMOTICON_INVALID_IMAGE_ERROR: &str = "invalid_image";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmoticonErrorKind {
    SizeLimit,
    ImageTooLarge,
    InvalidName,
    UnsupportedType,
    Empty,
    InvalidImage,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmoticonError {
    kind: EmoticonErrorKind,
    detail: Option<String>,
}

impl EmoticonError {
    pub fn kind(&self) -> EmoticonErrorKind {
        self.kind
    }

    pub(crate) fn other(detail: impl Into<String>) -> Self {
        Self {
            kind: EmoticonErrorKind::Other,
            detail: Some(detail.into()),
        }
    }
}

impl From<EmoticonErrorKind> for EmoticonError {
    fn from(kind: EmoticonErrorKind) -> Self {
        Self { kind, detail: None }
    }
}

impl std::fmt::Display for EmoticonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(detail) = &self.detail {
            f.write_str(detail)
        } else if let Some(code) = self.kind.code() {
            f.write_str(code)
        } else {
            f.write_str("emoticon_error")
        }
    }
}

impl EmoticonErrorKind {
    pub const fn code(self) -> Option<&'static str> {
        match self {
            Self::SizeLimit => Some(EMOTICON_SIZE_LIMIT_ERROR),
            Self::ImageTooLarge => Some(EMOTICON_IMAGE_TOO_LARGE_ERROR),
            Self::InvalidName => Some(EMOTICON_INVALID_NAME_ERROR),
            Self::UnsupportedType => Some(EMOTICON_UNSUPPORTED_TYPE_ERROR),
            Self::Empty => Some(EMOTICON_EMPTY_ERROR),
            Self::InvalidImage => Some(EMOTICON_INVALID_IMAGE_ERROR),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Emoji {
    pub id: String,
    pub shortname: String,
    pub src: String,
    pub category: String,
    pub clan_id: String,
    pub clan_logo: String,
    pub creator_id: String,
    pub is_for_sale: bool,
}

#[derive(Debug, Clone)]
pub enum EmojiEvent {
    Changed,
}

pub struct EmojiStore {
    by_id: HashMap<String, Emoji>,
    order: Vec<String>,
    recent_ids: Vec<String>,
    freshness: Freshness,
    recent_freshness: Freshness,
    lag_refresh: Freshness,
    loading: bool,
    loading_recent: bool,
    reset_generation: u32,
    api: Arc<AppApi>,
    _conn_watch: Task<()>,
}

struct GlobalEmojiStore(Entity<EmojiStore>);
impl Global for GlobalEmojiStore {}

impl EventEmitter<EmojiEvent> for EmojiStore {}

impl EmojiStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(api, cx));
        cx.set_global(GlobalEmojiStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalEmojiStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalEmojiStore>().map(|g| g.0.clone())
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.by_id.clear();
        self.order.clear();
        self.recent_ids.clear();
        self.freshness.mark_stale();
        self.recent_freshness.mark_stale();
        self.lag_refresh.mark_stale();
        self.loading = false;
        self.loading_recent = false;
        self.reset_generation = self.reset_generation.wrapping_add(1);
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        Self {
            by_id: HashMap::new(),
            order: Vec::new(),
            recent_ids: Vec::new(),
            freshness: Freshness::new(),
            recent_freshness: Freshness::new(),
            lag_refresh: Freshness::new(),
            loading: false,
            loading_recent: false,
            reset_generation: 0,
            api,
            _conn_watch: conn_watch,
        }
    }

    fn register_realtime(cx: &mut Context<Self>) {
        let entity = cx.entity();
        RealtimeDispatch::global(cx).update(cx, |dispatch, _| {
            dispatch.on(RealtimeKind::ClanEmoji, &entity, |this, event, cx| {
                this.handle_event(event, cx)
            });
            dispatch.on(RealtimeKind::MessageReaction, &entity, |this, event, cx| {
                this.handle_reaction_echo(event, cx)
            });
            dispatch.on_lagged(&entity, |this, cx| this.refresh_after_lag(cx));
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
                    let updated = this.update(cx, |this, cx| {
                        this.ensure_loaded(cx);
                        this.ensure_recent_loaded(cx);
                    });
                    if updated.is_err() {
                        break;
                    }
                } else if !connected {
                    was_connected = false;
                }
            }
        })
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.ensure_loaded_task(cx).detach();
    }

    pub fn ensure_loaded_task(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.freshness.is_fresh(crate::CACHE_TTL) {
            return Task::ready(());
        }
        self.fetch(cx)
    }

    fn ensure_recent_loaded(&mut self, cx: &mut Context<Self>) {
        if self.recent_freshness.is_fresh(crate::CACHE_TTL) {
            return;
        }
        self.fetch_recent(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.freshness.mark_stale();
        self.fetch(cx).detach();
        self.recent_freshness.mark_stale();
        self.fetch_recent(cx);
    }

    fn refresh_after_lag(&mut self, cx: &mut Context<Self>) {
        if self.lag_refresh.is_fresh(LAG_REFRESH_MIN_INTERVAL) {
            return;
        }
        self.lag_refresh.mark_fetched();
        self.refresh(cx);
    }

    fn fetch(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.loading {
            return Task::ready(());
        }
        self.loading = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let result = api.list_emojis_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                if this.reset_generation != generation {
                    return;
                }
                this.loading = false;
                match result.map(|emojis| {
                    emojis
                        .into_iter()
                        .filter_map(emoji_from_proto)
                        .collect::<Vec<_>>()
                }) {
                    Ok(emojis) => this.apply_catalog(emojis, cx),
                    Err(e) => tracing::error!("list_emojis_by_user_id failed: {e}"),
                }
            });
        })
    }

    fn apply_catalog(&mut self, emojis: Vec<Emoji>, cx: &mut Context<Self>) {
        let (by_id, order) = index_emojis(emojis);
        self.freshness.mark_fetched();
        if by_id == self.by_id && order == self.order {
            return;
        }
        self.by_id = by_id;
        self.order = order;
        tracing::info!("EmojiStore: fetched {} emojis", self.order.len());
        cx.emit(EmojiEvent::Changed);
        cx.notify();
    }

    fn apply_recents_if_current(
        &mut self,
        generation: u32,
        recents: Vec<api::EmojiRecent>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.reset_generation != generation {
            return false;
        }
        let mut seen = HashSet::new();
        let recent_ids: Vec<String> = recents
            .into_iter()
            .map(|r| r.emoji_id.to_string())
            .filter(|id| seen.insert(id.clone()))
            .take(RECENT_EMOJI_CAP)
            .collect();
        self.recent_freshness.mark_fetched();
        if recent_ids == self.recent_ids {
            return true;
        }
        self.recent_ids = recent_ids;
        cx.emit(EmojiEvent::Changed);
        cx.notify();
        true
    }

    fn fetch_recent(&mut self, cx: &mut Context<Self>) {
        if self.loading_recent {
            return;
        }
        self.loading_recent = true;
        let api = self.api.clone();
        let generation = self.reset_generation;
        cx.spawn(async move |this, cx| {
            let mut delay = RECENT_FETCH_RETRY_DELAY;
            for attempt in 1..=RECENT_FETCH_ATTEMPTS {
                match api.emoji_recent_list().await {
                    Ok(recents) => {
                        let _ = this.update(cx, |this, cx| {
                            if this.apply_recents_if_current(generation, recents, cx) {
                                this.loading_recent = false;
                            }
                        });
                        return;
                    }
                    Err(e) => {
                        tracing::error!(
                            "emoji_recent_list failed (attempt {attempt}/{RECENT_FETCH_ATTEMPTS}): {e}"
                        );
                        if attempt == RECENT_FETCH_ATTEMPTS
                            || api.connection_status() != ConnectionStatus::Connected
                        {
                            break;
                        }
                        cx.background_executor().timer(delay).await;
                        delay *= 2;
                    }
                }
            }
            let _ = this.update(cx, |this, _| {
                if this.reset_generation == generation {
                    this.loading_recent = false;
                }
            });
        })
        .detach();
    }

    fn handle_reaction_echo(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::MessageReaction(r) = event else {
            return;
        };
        if r.action {
            return;
        }
        let Some(current_uid) = BadgeService::global(cx).read(cx).current_user_id(cx) else {
            return;
        };
        if current_uid.get().to_string() != r.sender_id.to_string() {
            return;
        }
        push_recent(&mut self.recent_ids, r.emoji_id.to_string());
        cx.emit(EmojiEvent::Changed);
        cx.notify();
    }

    pub fn recent(&self, limit: usize) -> Vec<&Emoji> {
        self.recent_ids
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .take(limit)
            .collect()
    }

    fn insert(&mut self, emoji: Emoji) {
        let id = emoji.id.clone();
        if self.by_id.insert(id.clone(), emoji).is_none() {
            self.order.push(id);
        }
    }

    fn remove(&mut self, id: &str) -> bool {
        if self.by_id.remove(id).is_some() {
            self.order.retain(|e| e != id);
            true
        } else {
            false
        }
    }

    pub fn all(&self) -> Vec<&Emoji> {
        ordered_emojis(&self.by_id, &self.order).collect()
    }

    pub fn for_clan(&self, clan_id: &str) -> Vec<&Emoji> {
        ordered_emojis(&self.by_id, &self.order)
            .filter(|emoji| emoji.clan_id == clan_id)
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&Emoji> {
        self.by_id.get(id)
    }

    pub fn suggest(&self, query: &str, clan_id: &str, limit: usize) -> Vec<&Emoji> {
        suggest_in(&self.by_id, &self.order, query, clan_id, limit)
    }

    pub fn create_emoji(
        &self,
        clan_id: ClanId,
        path: &Path,
        shortname: &str,
        is_for_sale: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let raw_name = strip_emoji_colons(shortname);
        if !is_valid_emoticon_shortname(&raw_name) || raw_name.chars().count() > EMOJI_SHORTNAME_MAX
        {
            return cx.spawn(async move |_, _| Err(EmoticonErrorKind::InvalidName.into()));
        }
        let shortname = normalize_emoji_shortname(&raw_name);
        if let Err(err) = validate_emoji_create_shortname(&shortname) {
            return cx.spawn(async move |_, _| Err(err.into()));
        }
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            let (id, url) =
                upload_emoticon_file(&api, &path, "emojis", MAX_EMOJI_BYTES, is_for_sale).await?;
            tracing::debug!(clan, %url, %shortname, id, is_for_sale, "CreateClanEmoji");
            api.create_clan_emoji(clan, &url, &shortname, "Custom", id, is_for_sale)
                .await
                .map_err(|e| EmoticonError::other(e.to_string()))?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| EmoticonError::other("store dropped"))?;
            Ok(())
        })
    }

    pub fn update_emoji(
        &self,
        emoji_id: &str,
        clan_id: ClanId,
        shortname: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), EmoticonError>> {
        let api = self.api.clone();
        let id: i64 = match emoji_id.parse() {
            Ok(id) => id,
            Err(_) => {
                return cx.spawn(async move |_, _| Err(EmoticonError::other("invalid emoji id")));
            }
        };
        let raw_name = strip_emoji_colons(shortname);
        if !is_valid_emoticon_shortname(&raw_name) || raw_name.chars().count() > EMOJI_SHORTNAME_MAX
        {
            return cx.spawn(async move |_, _| Err(EmoticonErrorKind::InvalidName.into()));
        }
        let shortname = normalize_emoji_shortname(&raw_name);
        if let Err(err) = validate_emoji_create_shortname(&shortname) {
            return cx.spawn(async move |_, _| Err(err.into()));
        }
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            api.update_clan_emoji_by_id(id, &shortname, clan)
                .await
                .map_err(|e| EmoticonError::other(e.to_string()))?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| EmoticonError::other("store dropped"))?;
            Ok(())
        })
    }

    pub fn delete_emoji(
        &self,
        emoji_id: &str,
        clan_id: ClanId,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let id: i64 = match emoji_id.parse() {
            Ok(id) => id,
            Err(_) => return cx.spawn(async move |_, _| Err("invalid emoji id".into())),
        };
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            api.delete_clan_emoji_by_id(id, clan)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| {
                this.remove(&id.to_string());
                cx.emit(EmojiEvent::Changed);
                cx.notify();
            })
            .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn by_category(&self, active_clan_id: Option<&str>) -> Vec<(String, Vec<&Emoji>)> {
        let mut groups = by_category_in(&self.by_id, &self.order, active_clan_id);
        let recent = self.recent(RECENT_EMOJI_CAP);
        if !recent.is_empty() {
            groups.insert(0, ("Recent".to_string(), recent));
        }
        groups
    }

    fn handle_event(&mut self, event: &RealtimeEvent, cx: &mut Context<Self>) {
        let RealtimeEvent::ClanEmoji(e) = event else {
            return;
        };
        let changed = match e.action {
            EMOJI_ACTION_CREATED => {
                if let Some(emoji) = emoji_from_event(e) {
                    self.insert(emoji);
                    true
                } else {
                    false
                }
            }
            EMOJI_ACTION_UPDATE => {
                let id = e.id.to_string();
                match self.by_id.get_mut(&id) {
                    Some(emoji) => {
                        emoji.shortname = e.short_name.clone();
                        if !e.source.is_empty() {
                            emoji.src = e.source.clone();
                        }
                        let category = if !e.category.is_empty() {
                            e.category.clone()
                        } else {
                            e.clan_name.clone()
                        };
                        if !category.is_empty() {
                            emoji.category = category;
                        }
                        true
                    }
                    None => false,
                }
            }
            EMOJI_ACTION_DELETE => self.remove(&e.id.to_string()),
            _ => false,
        };
        if changed {
            cx.emit(EmojiEvent::Changed);
            cx.notify();
        }
    }
}

fn index_emojis(emojis: Vec<Emoji>) -> (HashMap<String, Emoji>, Vec<String>) {
    let mut by_id = HashMap::with_capacity(emojis.len());
    let mut order = Vec::with_capacity(emojis.len());
    for emoji in emojis {
        let id = emoji.id.clone();
        if by_id.insert(id.clone(), emoji).is_none() {
            order.push(id);
        }
    }
    (by_id, order)
}

fn push_recent(recent_ids: &mut Vec<String>, id: String) {
    recent_ids.retain(|existing| existing != &id);
    recent_ids.insert(0, id);
    recent_ids.truncate(RECENT_EMOJI_CAP);
}

fn ordered_emojis<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
) -> impl Iterator<Item = &'a Emoji> {
    order.iter().filter_map(move |id| by_id.get(id))
}

fn suggest_in<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
    query: &str,
    clan_id: &str,
    limit: usize,
) -> Vec<&'a Emoji> {
    let needle = query.to_lowercase();
    ordered_emojis(by_id, order)
        .filter(|emoji| {
            emoji.clan_id == clan_id && emoji.shortname.to_lowercase().starts_with(&needle)
        })
        .take(limit)
        .collect()
}

fn by_category_in<'a>(
    by_id: &'a HashMap<String, Emoji>,
    order: &'a [String],
    active_clan_id: Option<&str>,
) -> Vec<(String, Vec<&'a Emoji>)> {
    let mut ordered: Vec<&Emoji> = ordered_emojis(by_id, order).collect();
    if let Some(active) = active_clan_id {
        ordered.sort_by_key(|emoji| u8::from(emoji.clan_id != active));
    }
    let mut groups: Vec<(String, Vec<&Emoji>)> = Vec::new();
    for emoji in ordered {
        match groups.iter_mut().find(|(name, _)| name == &emoji.category) {
            Some((_, list)) => list.push(emoji),
            None => groups.push((emoji.category.clone(), vec![emoji])),
        }
    }
    groups
}

fn emoji_from_proto(e: api::ClanEmoji) -> Option<Emoji> {
    if e.id == 0 || e.shortname.is_empty() {
        return None;
    }
    let category = if !e.category.is_empty() {
        e.category
    } else {
        e.clan_name
    };
    Some(Emoji {
        id: e.id.to_string(),
        shortname: e.shortname,
        src: e.src,
        category,
        clan_id: e.clan_id.to_string(),
        clan_logo: e.logo,
        creator_id: e.creator_id.to_string(),
        is_for_sale: e.is_for_sale,
    })
}

fn emoji_from_event(e: &realtime::EventEmoji) -> Option<Emoji> {
    if e.id == 0 || e.short_name.is_empty() {
        return None;
    }
    let category = if !e.category.is_empty() {
        e.category.clone()
    } else {
        e.clan_name.clone()
    };
    Some(Emoji {
        id: e.id.to_string(),
        shortname: e.short_name.clone(),
        src: e.source.clone(),
        category,
        clan_id: e.clan_id.to_string(),
        clan_logo: e.logo.clone(),
        creator_id: e.user_id.to_string(),
        is_for_sale: e.is_for_sale,
    })
}

pub fn is_valid_emoticon_shortname(name: &str) -> bool {
    let len = name.chars().count();
    (EMOTICON_SHORTNAME_MIN..=EMOTICON_SHORTNAME_MAX).contains(&len)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn normalize_emoji_shortname(name: &str) -> String {
    let trimmed = name.trim().trim_matches(':');
    format!(":{trimmed}:")
}

pub fn validate_emoji_create_shortname(shortname: &str) -> Result<(), EmoticonErrorKind> {
    let runes: Vec<char> = shortname.chars().collect();
    let len = runes.len();
    if !(EMOTICON_SHORTNAME_MIN..=EMOTICON_SHORTNAME_MAX).contains(&len) {
        return Err(EmoticonErrorKind::InvalidName);
    }
    if len >= 2 && runes[0] == ':' && runes[1] == ':' {
        return Err(EmoticonErrorKind::InvalidName);
    }
    if len >= 2 && runes[len - 2] == ':' && runes[len - 1] == ':' {
        return Err(EmoticonErrorKind::InvalidName);
    }
    if shortname.contains([' ', '\n', '\t', '\r']) {
        return Err(EmoticonErrorKind::InvalidName);
    }
    Ok(())
}

pub fn strip_emoji_colons(name: &str) -> String {
    name.trim().trim_matches(':').to_string()
}

fn emoticon_extension(path: &Path) -> Result<String, EmoticonErrorKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !EMOTICON_ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(EmoticonErrorKind::UnsupportedType);
    }
    Ok(ext)
}

fn read_emoticon_file(path: &Path, max_bytes: u64) -> Result<(Vec<u8>, String), EmoticonErrorKind> {
    let ext = emoticon_extension(path)?;
    let len = std::fs::metadata(path)
        .map_err(|_| EmoticonErrorKind::InvalidImage)?
        .len();
    if len == 0 {
        return Err(EmoticonErrorKind::Empty);
    }
    if len > max_bytes {
        return Err(EmoticonErrorKind::SizeLimit);
    }
    let data = std::fs::read(path).map_err(|_| EmoticonErrorKind::InvalidImage)?;
    Ok((data, ext))
}

pub fn validate_emoticon_file(path: &Path, max_bytes: u64) -> Result<(), EmoticonErrorKind> {
    let (data, _) = read_emoticon_file(path, max_bytes)?;
    decode_emoticon_image(&data)?;
    Ok(())
}

pub fn generate_snowflake_id() -> i64 {
    static SEQUENCE: AtomicU16 = AtomicU16::new(0);
    const SHARD_ID: u64 = 1;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = u64::from(SEQUENCE.fetch_add(1, Ordering::Relaxed) % 4096);
    let shard = SHARD_ID % 1024;
    let id = (ts << 22) | (shard << 12) | seq;
    i64::try_from(id).unwrap_or(i64::MAX)
}

#[derive(Clone)]
struct PreparedEmoticon {
    bytes: Vec<u8>,
    filetype: &'static str,
    max_px: u32,
}

fn prepare_emoticon_from_path(
    path: &Path,
    max_bytes: u64,
    max_px: u32,
) -> Result<PreparedEmoticon, EmoticonError> {
    let (data, ext) = read_emoticon_file(path, max_bytes)?;
    let decoded = decode_emoticon_image(&data)?;
    let (bytes, filetype) = prepare_emoticon_upload_bytes(&data, decoded, &ext, max_px)?;
    if bytes.len() as u64 > max_bytes {
        return Err(EmoticonErrorKind::SizeLimit.into());
    }
    Ok(PreparedEmoticon {
        bytes,
        filetype,
        max_px,
    })
}

fn blend_channel(base: u8, overlay: u8, alpha: f32) -> u8 {
    let blended = f32::from(base) * (1.0 - alpha) + f32::from(overlay) * alpha;
    blended.round().clamp(0.0, 255.0) as u8
}

fn box_blur_rgba(img: &mut image::RgbaImage, radius: u32) {
    if radius == 0 {
        return;
    }
    let (width, height) = img.dimensions();
    let src = img.clone();
    let radius = i32::try_from(radius).unwrap_or(i32::MAX);
    for y in 0..height {
        for x in 0..width {
            let mut channels = [0u32; 4];
            let mut count = 0u32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let nx = i32::try_from(x).unwrap_or(i32::MAX) + dx;
                    let ny = i32::try_from(y).unwrap_or(i32::MAX) + dy;
                    if nx >= 0
                        && ny >= 0
                        && nx < i32::try_from(width).unwrap_or(i32::MAX)
                        && ny < i32::try_from(height).unwrap_or(i32::MAX)
                    {
                        let pixel = src.get_pixel(nx as u32, ny as u32);
                        for (idx, value) in channels.iter_mut().zip(pixel.0) {
                            *idx += u32::from(value);
                        }
                        count += 1;
                    }
                }
            }
            if count > 0 {
                img.put_pixel(
                    x,
                    y,
                    image::Rgba([
                        (channels[0].checked_div(count).unwrap_or(0)) as u8,
                        (channels[1].checked_div(count).unwrap_or(0)) as u8,
                        (channels[2].checked_div(count).unwrap_or(0)) as u8,
                        (channels[3].checked_div(count).unwrap_or(0)) as u8,
                    ]),
                );
            }
        }
    }
}

fn emoticon_image_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(EMOTICON_SOURCE_MAX_PX);
    limits.max_image_height = Some(EMOTICON_SOURCE_MAX_PX);
    limits.max_alloc = Some(EMOTICON_DECODE_MAX_ALLOC_BYTES);
    limits
}

fn decode_emoticon_image(data: &[u8]) -> Result<image::DynamicImage, EmoticonErrorKind> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| EmoticonErrorKind::InvalidImage)?;
    reader.limits(emoticon_image_limits());
    reader.decode().map_err(emoticon_decode_error)
}

fn emoticon_decode_error(err: image::ImageError) -> EmoticonErrorKind {
    match err {
        image::ImageError::Limits(_) => EmoticonErrorKind::ImageTooLarge,
        _ => EmoticonErrorKind::InvalidImage,
    }
}

fn create_blurred_watermarked_webp(data: &[u8], max_px: u32) -> Result<Vec<u8>, EmoticonError> {
    let img = decode_emoticon_image(data)?;
    let mut rgba = downscale_emoticon_image(img, max_px).to_rgba8();
    box_blur_rgba(&mut rgba, 2);

    let (width, height) = rgba.dimensions();
    let center_x = width as f32 / 2.0;
    let center_y = height as f32 / 2.0;
    let font_size = (width as f32 / 2.0).max(8.0);
    let cos = 0.707_106_77;
    let sin = 0.707_106_77;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - center_x;
            let dy = y as f32 - center_y;
            let rotated_x = dx * cos + dy * sin;
            let rotated_y = -dx * sin + dy * cos;
            if rotated_y.abs() <= font_size * 0.35 && rotated_x.abs() <= font_size * 1.1 {
                let pixel = rgba.get_pixel_mut(x, y);
                for channel in 0..3 {
                    pixel.0[channel] = blend_channel(pixel.0[channel], 128, 0.35);
                }
            }
        }
    }

    let (width, height) = rgba.dimensions();
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(
            rgba.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| EmoticonError::other(e.to_string()))?;
    Ok(out)
}

async fn upload_prepared_emoticon(
    api: &AppApi,
    folder: &str,
    prepared: PreparedEmoticon,
) -> Result<(i64, String), EmoticonError> {
    let id = generate_snowflake_id();
    api.upload_emoticon(folder, id, "webp", prepared.filetype, prepared.bytes)
        .await
        .map_err(|e| EmoticonError::other(e.to_string()))
}

async fn upload_emoticon_sale_preview(
    api: &AppApi,
    folder: &str,
    prepared: &PreparedEmoticon,
) -> Result<(), EmoticonError> {
    let prepared = prepared.clone();
    let blurred = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || create_blurred_watermarked_webp(&prepared.bytes, prepared.max_px))
        .await
        .map_err(|e| EmoticonError::other(e.to_string()))??;
    let preview_id = generate_snowflake_id();
    api.upload_emoticon(folder, preview_id, "webp", "image/webp", blurred)
        .await
        .map_err(|e| EmoticonError::other(e.to_string()))?;
    Ok(())
}

fn max_upload_px_for_folder(folder: &str) -> u32 {
    if folder.trim_matches('/') == "emojis" {
        EMOJI_UPLOAD_MAX_PX
    } else {
        STICKER_UPLOAD_MAX_PX
    }
}

fn prepare_emoticon_upload_bytes(
    data: &[u8],
    decoded: image::DynamicImage,
    ext: &str,
    max_px: u32,
) -> Result<(Vec<u8>, &'static str), EmoticonErrorKind> {
    if ext == "gif" {
        return Ok((data.to_vec(), "image/gif"));
    }

    let thumb = downscale_emoticon_image(decoded, max_px);
    let rgba = thumb.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(
            rgba.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|_| EmoticonErrorKind::InvalidImage)?;
    Ok((out, "image/webp"))
}

fn downscale_emoticon_image(image: image::DynamicImage, max_px: u32) -> image::DynamicImage {
    if image.width() <= max_px && image.height() <= max_px {
        image
    } else {
        image.thumbnail(max_px, max_px)
    }
}

pub async fn upload_emoticon_file(
    api: &AppApi,
    path: &Path,
    folder: &str,
    max_bytes: u64,
    upload_sale_preview: bool,
) -> Result<(i64, String), EmoticonError> {
    let path_buf = path.to_path_buf();
    let max = max_bytes;
    let max_px = max_upload_px_for_folder(folder);
    let prepared = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || prepare_emoticon_from_path(&path_buf, max, max_px))
        .await
        .map_err(|e| EmoticonError::other(e.to_string()))??;

    let (id, url) = upload_prepared_emoticon(api, folder, prepared.clone()).await?;
    if upload_sale_preview {
        upload_emoticon_sale_preview(api, folder, &prepared).await?;
    }
    Ok((id, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_emoji_store(cx: &mut gpui::TestAppContext) -> (Arc<AppApi>, Entity<EmojiStore>) {
        cx.update(|cx| {
            let api = Arc::new(mezon_client::AppApi::new(
                Arc::new(mezon_client::TransportClient::new(String::new())),
                String::new(),
            ));
            RealtimeDispatch::init(api.clone(), cx);
            let store = cx.new(|cx| EmojiStore::new(api.clone(), cx));
            (api, store)
        })
    }

    fn connect(api: &Arc<AppApi>, cx: &mut gpui::TestAppContext) {
        api.set_status(ConnectionStatus::Disconnected);
        cx.run_until_parked();
        api.set_status(ConnectionStatus::Connected);
        cx.run_until_parked();
    }

    fn recent_fetch_in_flight(store: &Entity<EmojiStore>, cx: &gpui::TestAppContext) -> bool {
        store.read_with(cx, |store, _| store.loading_recent)
    }

    fn count_changed_events(
        store: &Entity<EmojiStore>,
        cx: &mut gpui::TestAppContext,
    ) -> std::rc::Rc<std::cell::Cell<usize>> {
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let sink = seen.clone();
        cx.update(|cx| {
            cx.subscribe(store, move |_, _: &EmojiEvent, _| {
                sink.set(sink.get() + 1);
            })
            .detach();
        });
        seen
    }

    fn settle_recent_fetch_with_success(store: &Entity<EmojiStore>, cx: &mut gpui::TestAppContext) {
        store.update(cx, |store, cx| {
            let generation = store.reset_generation;
            store.loading_recent = false;
            store.apply_recents_if_current(generation, Vec::new(), cx);
        });
    }

    fn recent(emoji_id: i64) -> api::EmojiRecent {
        api::EmojiRecent {
            emoji_id,
            ..Default::default()
        }
    }

    fn proto(
        id: i64,
        shortname: &str,
        category: &str,
        clan_name: &str,
        clan_id: i64,
    ) -> api::ClanEmoji {
        api::ClanEmoji {
            id,
            src: format!("https://cdn/{id}.png"),
            shortname: shortname.into(),
            category: category.into(),
            clan_name: clan_name.into(),
            clan_id,
            ..Default::default()
        }
    }

    fn store_with(emojis: Vec<Emoji>) -> (HashMap<String, Emoji>, Vec<String>) {
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for e in emojis {
            let id = e.id.clone();
            if by_id.insert(id.clone(), e).is_none() {
                order.push(id);
            }
        }
        (by_id, order)
    }

    fn emoji(id: &str, shortname: &str, category: &str, clan_id: &str) -> Emoji {
        Emoji {
            id: id.into(),
            shortname: shortname.into(),
            src: String::new(),
            category: category.into(),
            clan_id: clan_id.into(),
            clan_logo: String::new(),
            creator_id: String::new(),
            is_for_sale: false,
        }
    }

    fn ids(emojis: Vec<&Emoji>) -> Vec<String> {
        emojis.into_iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn maps_proto_and_falls_back_to_clan_name_for_category() {
        let with_category = emoji_from_proto(proto(1, "smile", "Faces", "MyClan", 7)).unwrap();
        assert_eq!(with_category.id, "1");
        assert_eq!(with_category.category, "Faces");
        assert_eq!(with_category.clan_id, "7");
        let no_category = emoji_from_proto(proto(2, "wave", "", "MyClan", 7)).unwrap();
        assert_eq!(no_category.category, "MyClan");
    }

    #[test]
    fn skips_invalid_proto() {
        assert!(emoji_from_proto(proto(0, "smile", "", "", 1)).is_none());
        assert!(emoji_from_proto(proto(3, "", "", "", 1)).is_none());
    }

    #[test]
    fn event_create_maps_clan_and_category() {
        let e = realtime::EventEmoji {
            id: 9,
            short_name: "tada".into(),
            source: "https://cdn/9.png".into(),
            category: String::new(),
            clan_name: "Party".into(),
            clan_id: 42,
            action: EMOJI_ACTION_CREATED,
            ..Default::default()
        };
        let mapped = emoji_from_event(&e).unwrap();
        assert_eq!(mapped.id, "9");
        assert_eq!(mapped.shortname, "tada");
        assert_eq!(mapped.category, "Party");
        assert_eq!(mapped.clan_id, "42");
    }

    #[test]
    fn suggest_is_clan_scoped_and_prefix_filtered() {
        let (by_id, order) = store_with(vec![
            emoji("1", "smile", "Faces", "A"),
            emoji("2", "smirk", "Faces", "A"),
            emoji("3", "smile", "Faces", "B"),
            emoji("4", "wave", "Hands", "A"),
        ]);
        assert_eq!(
            ids(suggest_in(&by_id, &order, "sm", "A", 50)),
            vec!["1", "2"]
        );
        assert_eq!(ids(suggest_in(&by_id, &order, "smile", "B", 50)), vec!["3"]);
        assert!(suggest_in(&by_id, &order, "smile", "C", 50).is_empty());
        assert_eq!(ids(suggest_in(&by_id, &order, "wave", "A", 50)), vec!["4"]);
    }

    #[test]
    fn suggest_respects_limit() {
        let (by_id, order) = store_with(vec![
            emoji("1", "smile", "Faces", "A"),
            emoji("2", "smirk", "Faces", "A"),
            emoji("3", "smug", "Faces", "A"),
        ]);
        assert_eq!(suggest_in(&by_id, &order, "sm", "A", 2).len(), 2);
    }

    #[test]
    fn push_recent_moves_existing_to_front_and_dedupes() {
        let mut recent = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        push_recent(&mut recent, "2".to_string());
        assert_eq!(recent, vec!["2", "1", "3"]);
    }

    #[test]
    fn push_recent_caps_at_limit() {
        let mut recent: Vec<String> = (0..RECENT_EMOJI_CAP).map(|i| i.to_string()).collect();
        push_recent(&mut recent, "new".to_string());
        assert_eq!(recent.len(), RECENT_EMOJI_CAP);
        assert_eq!(recent[0], "new");
        assert!(!recent.contains(&(RECENT_EMOJI_CAP - 1).to_string()));
    }

    #[gpui::test]
    fn construction_does_not_fetch_recents(cx: &mut gpui::TestAppContext) {
        let (_api, store) = init_emoji_store(cx);
        cx.run_until_parked();
        assert!(
            !recent_fetch_in_flight(&store, cx),
            "recents must wait for the socket, not fire while disconnected"
        );
    }

    #[gpui::test]
    fn connecting_fetches_recents_and_a_fresh_reconnect_does_not(cx: &mut gpui::TestAppContext) {
        let (api, store) = init_emoji_store(cx);

        connect(&api, cx);
        assert!(
            recent_fetch_in_flight(&store, cx),
            "the first connect must fetch recents"
        );

        settle_recent_fetch_with_success(&store, cx);
        connect(&api, cx);
        assert!(
            !recent_fetch_in_flight(&store, cx),
            "a reconnect within the TTL must not refetch"
        );
    }

    #[gpui::test]
    fn logout_forces_a_refetch_on_the_next_connect(cx: &mut gpui::TestAppContext) {
        let (api, store) = init_emoji_store(cx);
        connect(&api, cx);
        settle_recent_fetch_with_success(&store, cx);

        store.update(cx, |store, cx| store.reset(cx));
        connect(&api, cx);

        assert!(
            recent_fetch_in_flight(&store, cx),
            "logout must invalidate recents so the next connect refetches"
        );
    }

    #[gpui::test]
    fn a_response_that_outlives_a_logout_is_dropped(cx: &mut gpui::TestAppContext) {
        let (api, store) = init_emoji_store(cx);
        connect(&api, cx);

        store.update(cx, |store, cx| {
            let in_flight = store.reset_generation;
            store.reset(cx);

            assert!(
                !store.apply_recents_if_current(in_flight, vec![recent(1), recent(2)], cx),
                "a response issued for the previous account must be dropped"
            );
            assert!(store.recent_ids.is_empty());
            assert!(!store.recent_freshness.is_fresh(crate::CACHE_TTL));
        });
    }

    #[gpui::test]
    fn an_unchanged_recent_list_does_not_wake_the_pickers(cx: &mut gpui::TestAppContext) {
        let (_api, store) = init_emoji_store(cx);
        let changed = count_changed_events(&store, cx);

        store.update(cx, |store, cx| {
            let generation = store.reset_generation;
            store.apply_recents_if_current(generation, vec![recent(1), recent(2)], cx);
            store.apply_recents_if_current(generation, vec![recent(1), recent(2)], cx);
        });
        cx.run_until_parked();

        assert_eq!(
            changed.get(),
            1,
            "an identical response must refresh the TTL without rebuilding every picker"
        );
        assert!(
            store.read_with(cx, |store, _| store
                .recent_freshness
                .is_fresh(crate::CACHE_TTL)),
            "the TTL must still be refreshed on the second response"
        );
    }

    #[gpui::test]
    fn an_unchanged_catalog_does_not_wake_the_pickers(cx: &mut gpui::TestAppContext) {
        let (_api, store) = init_emoji_store(cx);
        let changed = count_changed_events(&store, cx);
        let catalog = vec![emoji("1", "smile", "Faces", "A")];

        store.update(cx, |store, cx| {
            store.apply_catalog(catalog.clone(), cx);
            store.apply_catalog(catalog.clone(), cx);
        });
        cx.run_until_parked();

        assert_eq!(changed.get(), 1, "an identical catalog must not re-emit");
    }

    #[gpui::test]
    fn a_lag_storm_refreshes_at_most_once_per_interval(cx: &mut gpui::TestAppContext) {
        let (_api, store) = init_emoji_store(cx);

        store.update(cx, |store, cx| {
            store.refresh_after_lag(cx);
            assert!(store.loading, "the first lag must refresh the catalog");

            store.loading = false;
            store.loading_recent = false;
            store.refresh_after_lag(cx);
            assert!(
                !store.loading,
                "a second lag inside the interval must not refetch the catalog"
            );

            store.lag_refresh.mark_stale();
            store.refresh_after_lag(cx);
            assert!(store.loading, "a lag past the interval must refresh again");
        });
    }

    #[test]
    fn by_category_orders_active_clan_first() {
        let (by_id, order) = store_with(vec![
            emoji("1", "b_smile", "ClanB", "B"),
            emoji("2", "a_smile", "ClanA", "A"),
            emoji("3", "b_wave", "ClanB", "B"),
        ]);
        let groups = by_category_in(&by_id, &order, Some("A"));
        assert_eq!(groups[0].0, "ClanA");
        assert_eq!(ids(groups[0].1.clone()), vec!["2"]);
        assert_eq!(groups[1].0, "ClanB");
        assert_eq!(ids(groups[1].1.clone()), vec!["1", "3"]);
    }

    #[test]
    fn prepare_emoticon_upload_bytes_encodes_png_as_webp() {
        let mut img = image::RgbaImage::new(64, 64);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8, y as u8, 128, 255]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_emoticon_image(&png).unwrap();
        let (out, mime) =
            prepare_emoticon_upload_bytes(&png, decoded, "png", EMOJI_UPLOAD_MAX_PX).unwrap();
        assert_eq!(mime, "image/webp");
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::WebP);
    }

    #[test]
    fn validate_emoji_create_shortname_matches_server_rules() {
        assert!(validate_emoji_create_shortname(":wave:").is_ok());
        assert!(validate_emoji_create_shortname("::bad::").is_err());
        assert!(validate_emoji_create_shortname(":x:").is_ok());
        assert!(validate_emoji_create_shortname(":a").is_err());
        assert!(validate_emoji_create_shortname(":has space:").is_err());
    }

    #[test]
    fn snowflake_id_is_nineteen_digits() {
        let id = generate_snowflake_id();
        assert_eq!(id.to_string().len(), 19, "id={id}");
    }

    #[test]
    fn create_blurred_watermarked_webp_encodes_webp() {
        let mut img = image::RgbaImage::new(32, 32);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8, y as u8, 200, 255]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = create_blurred_watermarked_webp(&png, EMOJI_UPLOAD_MAX_PX).unwrap();
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::WebP);
    }

    #[test]
    fn prepare_emoticon_upload_bytes_keeps_gif() {
        let data = b"GIF89a";
        let decoded = decode_emoticon_image(data).unwrap_err();
        assert_eq!(decoded, EmoticonErrorKind::InvalidImage);
        let gif = image::DynamicImage::new_rgba8(1, 1);
        let (out, mime) =
            prepare_emoticon_upload_bytes(data, gif, "gif", EMOJI_UPLOAD_MAX_PX).unwrap();
        assert_eq!(mime, "image/gif");
        assert_eq!(out, data);
    }

    #[test]
    fn prepare_emoticon_upload_resizes_source_larger_than_emoji_dimensions() {
        let img = image::RgbaImage::from_pixel(256, 192, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let decoded = decode_emoticon_image(&png).unwrap();
        let (out, _) =
            prepare_emoticon_upload_bytes(&png, decoded, "png", EMOJI_UPLOAD_MAX_PX).unwrap();
        let resized = image::load_from_memory(&out).unwrap();

        assert_eq!((resized.width(), resized.height()), (128, 96));
    }

    #[test]
    fn validate_emoticon_accepts_source_larger_than_output_dimensions() {
        let img = image::RgbaImage::from_pixel(256, 192, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "mezon-emoticon-validation-{}.png",
            generate_snowflake_id()
        ));
        std::fs::write(&path, &png).unwrap();

        let result = validate_emoticon_file(&path, MAX_EMOJI_BYTES);
        let _ = std::fs::remove_file(path);

        assert!(result.is_ok());
    }

    #[test]
    fn downscale_does_not_upscale_small_sources() {
        let image = image::DynamicImage::new_rgba8(32, 24);
        let output = downscale_emoticon_image(image, EMOJI_UPLOAD_MAX_PX);
        assert_eq!((output.width(), output.height()), (32, 24));
    }

    #[test]
    fn validation_classifies_oversized_source_dimensions() {
        let img = image::RgbaImage::from_pixel(
            EMOTICON_SOURCE_MAX_PX + 1,
            1,
            image::Rgba([10, 20, 30, 255]),
        );
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "mezon-emoticon-dimension-limit-{}.png",
            generate_snowflake_id()
        ));
        std::fs::write(&path, &png).unwrap();

        let result = validate_emoticon_file(&path, MAX_EMOJI_BYTES);
        let _ = std::fs::remove_file(path);

        assert_eq!(result, Err(EmoticonErrorKind::ImageTooLarge));
    }

    #[test]
    fn sale_preview_is_bounded_before_blur() {
        let img = image::RgbaImage::from_pixel(256, 192, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let out = create_blurred_watermarked_webp(&png, EMOJI_UPLOAD_MAX_PX).unwrap();
        let preview = image::load_from_memory(&out).unwrap();

        assert_eq!((preview.width(), preview.height()), (128, 96));
    }

    #[test]
    fn emoji_raw_name_limit_accounts_for_colons() {
        assert_eq!(EMOJI_SHORTNAME_MAX, EMOTICON_SHORTNAME_MAX - 2);
        assert!(validate_emoji_create_shortname(&format!(":{}:", "a".repeat(62))).is_ok());
        assert!(validate_emoji_create_shortname(&format!(":{}:", "a".repeat(63))).is_err());
    }
}
