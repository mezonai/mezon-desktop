use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use image::ImageEncoder as _;
use mezon_client::{AppApi, ConnectionStatus, RealtimeEvent};
use mezon_proto::{api, realtime};

use crate::Freshness;
use crate::badge::BadgeService;
use crate::ids::ClanId;
use crate::realtime::{RealtimeDispatch, RealtimeKind};

const RECENT_EMOJI_CAP: usize = 20;
const EMOJI_ACTION_CREATED: i32 = 1;
const EMOJI_ACTION_UPDATE: i32 = 2;
const EMOJI_ACTION_DELETE: i32 = 3;

pub const MAX_EMOJI_BYTES: u64 = 256 * 1024;
pub const MAX_STICKER_BYTES: u64 = 512 * 1024;
pub const EMOJI_UPLOAD_MAX_PX: u32 = 128;
pub const STICKER_UPLOAD_MAX_PX: u32 = 320;
pub const EMOTICON_SHORTNAME_MIN: usize = 3;
pub const EMOTICON_SHORTNAME_MAX: usize = 64;
pub const EMOTICON_ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];

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
    loading: bool,
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
        self.loading = false;
        cx.notify();
    }

    fn new(api: Arc<AppApi>, cx: &mut Context<Self>) -> Self {
        Self::register_realtime(cx);

        let conn_watch = Self::spawn_connection_watch(api.clone(), cx);

        let mut store = Self {
            by_id: HashMap::new(),
            order: Vec::new(),
            recent_ids: Vec::new(),
            freshness: Freshness::new(),
            loading: false,
            api,
            _conn_watch: conn_watch,
        };
        store.fetch_recent(cx);
        store
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
                    if this.update(cx, |this, cx| this.ensure_loaded(cx)).is_err() {
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

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.fetch(cx).detach();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.loading {
            return Task::ready(());
        }
        self.loading = true;
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.list_emojis_by_user_id().await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result.map(|emojis| {
                    emojis
                        .into_iter()
                        .filter_map(emoji_from_proto)
                        .collect::<Vec<_>>()
                }) {
                    Ok(emojis) => {
                        this.by_id.clear();
                        this.order.clear();
                        for emoji in emojis {
                            this.insert(emoji);
                        }
                        this.freshness.mark_fetched();
                        tracing::info!("EmojiStore: fetched {} emojis", this.order.len());
                        cx.emit(EmojiEvent::Changed);
                        cx.notify();
                    }
                    Err(e) => tracing::error!("list_emojis_by_user_id failed: {e}"),
                }
            });
        })
    }

    fn fetch_recent(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api.emoji_recent_list().await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(recents) => {
                    let mut seen = HashSet::new();
                    this.recent_ids = recents
                        .into_iter()
                        .map(|r| r.emoji_id.to_string())
                        .filter(|id| seen.insert(id.clone()))
                        .take(RECENT_EMOJI_CAP)
                        .collect();
                    cx.emit(EmojiEvent::Changed);
                    cx.notify();
                }
                Err(e) => tracing::error!("emoji_recent_list failed: {e}"),
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
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let path = path.to_path_buf();
        let raw_name = strip_emoji_colons(shortname);
        if !is_valid_emoticon_shortname(&raw_name) {
            return cx.spawn(async move |_, _| Err("invalid_name".into()));
        }
        let shortname = normalize_emoji_shortname(&raw_name);
        if let Err(err) = validate_emoji_create_shortname(&shortname) {
            return cx.spawn(async move |_, _| Err(err));
        }
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            let (id, url) =
                upload_emoticon_file(&api, &path, "emojis", MAX_EMOJI_BYTES, is_for_sale).await?;
            tracing::debug!(clan, %url, %shortname, id, is_for_sale, "CreateClanEmoji");
            api.create_clan_emoji(clan, &url, &shortname, "Custom", id, is_for_sale)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| "store dropped".to_string())?;
            Ok(())
        })
    }

    pub fn update_emoji(
        &self,
        emoji_id: &str,
        clan_id: ClanId,
        shortname: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let api = self.api.clone();
        let id: i64 = match emoji_id.parse() {
            Ok(id) => id,
            Err(_) => return cx.spawn(async move |_, _| Err("invalid emoji id".into())),
        };
        let raw_name = strip_emoji_colons(shortname);
        if !is_valid_emoticon_shortname(&raw_name) {
            return cx.spawn(async move |_, _| Err("invalid_name".into()));
        }
        let shortname = normalize_emoji_shortname(&raw_name);
        if let Err(err) = validate_emoji_create_shortname(&shortname) {
            return cx.spawn(async move |_, _| Err(err));
        }
        let clan = clan_id.get();
        cx.spawn(async move |this, cx| {
            api.update_clan_emoji_by_id(id, &shortname, clan)
                .await
                .map_err(|e| e.to_string())?;
            this.update(cx, |this, cx| this.refresh(cx))
                .map_err(|_| "store dropped".to_string())?;
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

pub fn validate_emoji_create_shortname(shortname: &str) -> Result<(), String> {
    let runes: Vec<char> = shortname.chars().collect();
    let len = runes.len();
    if !(3..=64).contains(&len) {
        return Err("invalid_name".into());
    }
    if len >= 2 && runes[0] == ':' && runes[1] == ':' {
        return Err("invalid_name".into());
    }
    if len >= 2 && runes[len - 2] == ':' && runes[len - 1] == ':' {
        return Err("invalid_name".into());
    }
    if shortname.contains([' ', '\n', '\t', '\r']) {
        return Err("invalid_name".into());
    }
    Ok(())
}

pub fn strip_emoji_colons(name: &str) -> String {
    name.trim().trim_matches(':').to_string()
}

pub fn validate_emoticon_file(path: &Path, max_bytes: u64, max_px: u32) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !EMOTICON_ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err("unsupported_type".into());
    }
    let len = std::fs::metadata(path)
        .map_err(|_| "invalid_image".to_string())?
        .len();
    if len == 0 {
        return Err("empty".into());
    }
    if len > max_bytes {
        return Err("size_limit".into());
    }
    let data = std::fs::read(path).map_err(|_| "invalid_image".to_string())?;
    decode_emoticon_image(&data, max_px)?;
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
}

fn prepare_emoticon_from_path(
    path: &Path,
    max_bytes: u64,
    max_px: u32,
) -> Result<PreparedEmoticon, String> {
    validate_emoticon_file(path, max_bytes, max_px)?;
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let (bytes, filetype) = prepare_emoticon_upload_bytes(&data, &ext, max_px)?;
    if bytes.len() as u64 > max_bytes {
        return Err("size_limit".into());
    }
    Ok(PreparedEmoticon { bytes, filetype })
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

fn emoticon_image_limits(max_px: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_px);
    limits.max_image_height = Some(max_px);
    limits.max_alloc = Some(max_px as u64 * max_px as u64 * 4);
    limits
}

fn decode_emoticon_image(data: &[u8], max_px: u32) -> Result<image::DynamicImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(emoticon_image_limits(max_px));
    reader.decode().map_err(emoticon_decode_error)
}

fn emoticon_decode_error(err: image::ImageError) -> String {
    match err {
        image::ImageError::Limits(_) => "image_too_large".into(),
        image::ImageError::Decoding(_) | image::ImageError::Parameter(_) => "invalid_image".into(),
        _ => "invalid_image".into(),
    }
}

fn create_blurred_watermarked_webp(data: &[u8], filetype: &str) -> Result<Vec<u8>, String> {
    let img = decode_emoticon_image(data, STICKER_UPLOAD_MAX_PX)?;
    let mut rgba = img.to_rgba8();
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
        .map_err(|e| e.to_string())?;
    let _ = filetype;
    Ok(out)
}

async fn upload_prepared_emoticon(
    api: &AppApi,
    folder: &str,
    prepared: PreparedEmoticon,
) -> Result<(i64, String), String> {
    let id = generate_snowflake_id();
    api.upload_emoticon(folder, id, "webp", prepared.filetype, prepared.bytes)
        .await
        .map_err(|e| e.to_string())
}

async fn upload_emoticon_sale_preview(
    api: &AppApi,
    folder: &str,
    prepared: &PreparedEmoticon,
) -> Result<(), String> {
    let prepared = prepared.clone();
    let blurred = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || create_blurred_watermarked_webp(&prepared.bytes, prepared.filetype))
        .await
        .map_err(|e| e.to_string())??;
    let preview_id = generate_snowflake_id();
    api.upload_emoticon(folder, preview_id, "webp", "image/webp", blurred)
        .await
        .map_err(|e| e.to_string())?;
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
    ext: &str,
    max_px: u32,
) -> Result<(Vec<u8>, &'static str), String> {
    if ext == "gif" {
        return Ok((data.to_vec(), "image/gif"));
    }

    let img = decode_emoticon_image(data, max_px)?;
    let thumb = img.thumbnail(max_px, max_px);
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
        .map_err(|e| e.to_string())?;
    Ok((out, "image/webp"))
}

pub async fn upload_emoticon_file(
    api: &AppApi,
    path: &Path,
    folder: &str,
    max_bytes: u64,
    upload_sale_preview: bool,
) -> Result<(i64, String), String> {
    let path_buf = path.to_path_buf();
    let max = max_bytes;
    let max_px = max_upload_px_for_folder(folder);
    let prepared = mezon_client::transport_runtime::handle()
        .spawn_blocking(move || prepare_emoticon_from_path(&path_buf, max, max_px))
        .await
        .map_err(|e| e.to_string())??;

    let (id, url) = upload_prepared_emoticon(api, folder, prepared.clone()).await?;
    if upload_sale_preview {
        upload_emoticon_sale_preview(api, folder, &prepared).await?;
    }
    Ok((id, url))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (out, mime) = prepare_emoticon_upload_bytes(&png, "png", EMOJI_UPLOAD_MAX_PX).unwrap();
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
        let out = create_blurred_watermarked_webp(&png, "image/png").unwrap();
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::WebP);
    }

    #[test]
    fn prepare_emoticon_upload_bytes_keeps_gif() {
        let data = b"GIF89a";
        let (out, mime) = prepare_emoticon_upload_bytes(data, "gif", EMOJI_UPLOAD_MAX_PX).unwrap();
        assert_eq!(mime, "image/gif");
        assert_eq!(out, data);
    }
}
