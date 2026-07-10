use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, SharedString};
use mezon_client::AppApi;
use mezon_client::transport::ApiChannelAttachment;

use crate::account::AccountStore;
use crate::badge::BadgeService;
use crate::clan_members::ClanMembersStore;
use crate::config::AppConfig;
use crate::ids::{ChannelId, ClanId, MessageId, UserId};
use crate::messages::MessagesStore;
use crate::user_profile::{ProfileContext, resolve_user_profile};

/// Gallery server cache TTL (React `GALLERY_CACHED_TIME` = 1 hour).
pub const GALLERY_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Default page size (React gallery slice `limit = 50`).
pub const GALLERY_PAGE_SIZE: i32 = 50;

/// Client-side media tab filter (React `MediaFilterType`). Switching tabs never
/// refetches — it filters the already-loaded list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaFilter {
    #[default]
    All,
    Image,
    Video,
}

impl MediaFilter {
    fn matches(self, att: &ChannelAttachment) -> bool {
        match self {
            MediaFilter::All => att.is_image || att.is_video,
            MediaFilter::Image => att.is_image,
            MediaFilter::Video => att.is_video,
        }
    }
}

/// A single media attachment in a channel — image or video.
#[derive(Debug, Clone)]
pub struct ChannelAttachment {
    pub id: i64,
    pub channel_id: ChannelId,
    pub clan_id: ClanId,
    pub message_id: MessageId,
    pub uploader_id: UserId,
    /// Original CDN url (used for copy-link, download, open-in-browser).
    pub url: String,
    pub filename: String,
    pub filetype: String,
    pub width: u32,
    pub height: u32,
    pub create_time_seconds: u32,
    pub is_image: bool,
    pub is_video: bool,
    /// Proxied square thumbnail for the gallery grid.
    pub thumb_src: SharedString,
    /// Proxied full-size url for the image viewer.
    pub viewer_src: SharedString,
    /// Calendar-day label ("January 01, 2021") for date grouping headers.
    pub day_label: SharedString,
    /// Preformatted bottom-bar timestamp ("HH:MM:SS D/M/YYYY").
    pub viewer_timestamp: SharedString,
    /// Days since the unix epoch (local), the date-group key.
    pub day_index: i64,
    /// Resolved uploader display name (filled by [`enrich_uploader`]).
    pub uploader_name: SharedString,
    /// Resolved uploader avatar url (filled by [`enrich_uploader`]).
    pub uploader_avatar: SharedString,
    /// Unproxied avatar url for [`Avatar`] fallback when imgproxy fails.
    pub uploader_avatar_raw: SharedString,
}

impl ChannelAttachment {
    pub fn from_api(
        api: ApiChannelAttachment,
        channel_id: ChannelId,
        clan_id: ClanId,
        cfg: &AppConfig,
    ) -> Self {
        let is_video = is_video_type(&api.filetype, &api.url);
        let is_image = !is_video && is_image_type(&api.filetype, &api.url);
        let (thumb_src, viewer_src) = if is_video {
            let url = api.url.clone();
            (url.clone().into(), url.into())
        } else {
            (
                cfg.gallery_thumb_proxy(&api.url).into(),
                cfg.viewer_proxy(&api.url, api.width.max(0) as u32, api.height.max(0) as u32)
                    .into(),
            )
        };
        let day_label = format_day(api.create_time_seconds as i64).into();
        let day_index = day_index(api.create_time_seconds as i64);
        let viewer_timestamp = format_viewer_timestamp(api.create_time_seconds).into();
        Self {
            id: api.id,
            channel_id,
            clan_id,
            message_id: MessageId(api.message_id),
            uploader_id: UserId(api.uploader),
            url: api.url,
            filename: api.filename,
            filetype: api.filetype,
            width: api.width.max(0) as u32,
            height: api.height.max(0) as u32,
            create_time_seconds: api.create_time_seconds,
            is_image,
            is_video,
            thumb_src,
            viewer_src,
            day_label,
            day_index,
            viewer_timestamp,
            uploader_name: SharedString::default(),
            uploader_avatar: SharedString::default(),
            uploader_avatar_raw: SharedString::default(),
        }
    }

    pub fn is_media(&self) -> bool {
        self.is_image || self.is_video
    }
}

pub async fn fetch_channel_attachments(
    api: Arc<AppApi>,
    cfg: AppConfig,
    clan_id: ClanId,
    channel_id: ChannelId,
    before: u32,
    after: u32,
    limit: i32,
) -> anyhow::Result<Vec<ChannelAttachment>> {
    let list = api
        .list_channel_attachments(clan_id.0, channel_id.0, "", 0, limit, before, after)
        .await?;
    Ok(list
        .into_iter()
        .map(|a| ChannelAttachment::from_api(a, channel_id, clan_id, &cfg))
        .filter(ChannelAttachment::is_media)
        .collect())
}

/// Resolved uploader display info.
#[derive(Debug, Clone, Default)]
pub struct UploaderInfo {
    pub name: String,
    pub avatar: String,
    pub avatar_raw: String,
}

fn uploader_urls(raw: &str, cfg: Option<&AppConfig>) -> (String, String) {
    if raw.is_empty() {
        return (String::new(), String::new());
    }
    let avatar = match cfg {
        Some(cfg) => cfg.avatar_proxy(raw),
        None => raw.to_string(),
    };
    (avatar, raw.to_string())
}

pub fn resolve_attachment_uploader(
    clan_id: ClanId,
    channel_id: ChannelId,
    uid: UserId,
    message_id: MessageId,
    cfg: Option<&AppConfig>,
    cx: &App,
) -> UploaderInfo {
    let context = if clan_id.0 == 0 {
        ProfileContext::Direct(channel_id)
    } else {
        ProfileContext::Clan(clan_id)
    };
    if let Some(profile) = resolve_user_profile(uid, context, cx)
        && !profile.display_name.is_empty() {
            let (avatar, avatar_raw) = uploader_urls(&profile.avatar_url, cfg);
            return UploaderInfo {
                name: profile.display_name,
                avatar,
                avatar_raw,
            };
        }
    if let Some(info) = uploader_from_message(channel_id, message_id, cfg, cx) {
        return info;
    }
    if clan_id.0 != 0
        && let Some(member) =
            ClanMembersStore::try_global(cx).and_then(|store| store.read(cx).member(clan_id, uid))
    {
        let name = member.name().to_string();
        if !name.is_empty() {
            let (avatar, avatar_raw) = uploader_urls(member.avatar(), cfg);
            return UploaderInfo {
                name,
                avatar,
                avatar_raw,
            };
        }
    }
    let is_self = BadgeService::global(cx)
        .read(cx)
        .current_user_id(cx)
        .is_some_and(|id| id == uid);
    if is_self {
        let store = AccountStore::global(cx).read(cx);
        if let Some(acct) = &store.account {
            let name = if !acct.display_name.is_empty() {
                acct.display_name.clone()
            } else {
                acct.username.clone()
            };
            if !name.is_empty() {
                let raw = acct
                    .avatar_url
                    .as_deref()
                    .filter(|url| !url.is_empty())
                    .unwrap_or_default()
                    .to_string();
                let (avatar, avatar_raw) = uploader_urls(&raw, cfg);
                return UploaderInfo {
                    name,
                    avatar,
                    avatar_raw,
                };
            }
        }
    }
    UploaderInfo::default()
}

fn uploader_from_message(
    channel_id: ChannelId,
    message_id: MessageId,
    cfg: Option<&AppConfig>,
    cx: &App,
) -> Option<UploaderInfo> {
    if message_id.is_zero() {
        return None;
    }
    let store = MessagesStore::try_global(cx)?;
    let msg = store.read(cx).message_in_channel(channel_id, message_id)?;
    let name = msg.sender_name.to_string();
    if name.is_empty() {
        return None;
    }
    let raw = msg.avatar_url.to_string();
    let avatar = if !msg.avatar_proxied.is_empty() {
        msg.avatar_proxied.to_string()
    } else {
        uploader_urls(&raw, cfg).0
    };
    Some(UploaderInfo {
        name,
        avatar,
        avatar_raw: raw,
    })
}

pub fn enrich_uploader<F>(attachments: &mut [ChannelAttachment], resolve: F)
where
    F: Fn(&ChannelAttachment) -> Option<UploaderInfo>,
{
    for att in attachments.iter_mut() {
        match resolve(att) {
            Some(info) if !info.name.is_empty() => {
                att.uploader_name = info.name.into();
                att.uploader_avatar = info.avatar.into();
                att.uploader_avatar_raw = info.avatar_raw.into();
            }
            _ => {
                att.uploader_name = "Anonymous".into();
                att.uploader_avatar = SharedString::default();
                att.uploader_avatar_raw = SharedString::default();
            }
        }
    }
}

fn is_video_type(filetype: &str, url: &str) -> bool {
    if filetype.starts_with("video/") {
        return true;
    }
    let lower = filetype.to_ascii_lowercase();
    if lower.contains("mp4") || lower.contains("mov") || lower.contains("webm") {
        return true;
    }
    matches!(
        extension(url).as_deref(),
        Some("mp4" | "mov" | "webm" | "mkv" | "avi")
    )
}

fn is_image_type(filetype: &str, url: &str) -> bool {
    if filetype.starts_with("image/") || filetype == "sticker" {
        return true;
    }
    matches!(
        extension(url).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif")
    )
}

fn extension(url: &str) -> Option<String> {
    url.split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('.').next())
        .map(|ext| ext.to_ascii_lowercase())
}

fn format_day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%B %d, %Y").to_string())
        .unwrap_or_default()
}

fn format_viewer_timestamp(ts: u32) -> String {
    use chrono::{Datelike, Timelike};
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.with_timezone(&chrono::Local))
        .map(|local| {
            format!(
                "{:02}:{:02}:{:02} {}/{}/{}",
                local.hour(),
                local.minute(),
                local.second(),
                local.day(),
                local.month(),
                local.year()
            )
        })
        .unwrap_or_default()
}

fn day_index(ts: i64) -> i64 {
    ts.div_euclid(86_400)
}

/// Pagination/scroll direction for [`GalleryStore::fetch_page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadDirection {
    /// Older attachments (scroll down past the oldest loaded item).
    Before,
    /// Newer attachments (scroll up past the newest loaded item).
    After,
}

#[derive(Default)]
struct GalleryChannel {
    attachments: Vec<ChannelAttachment>,
    has_more_before: bool,
    has_more_after: bool,
    is_loading: bool,
    fetched_at: Option<Instant>,
    date_range: Option<(u32, u32)>,
}

impl GalleryChannel {
    fn is_fresh(&self) -> bool {
        self.fetched_at
            .is_some_and(|t| t.elapsed() < GALLERY_CACHE_TTL)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum GalleryEvent {
    Changed(ChannelId),
}

pub struct GalleryStore {
    by_channel: HashMap<ChannelId, GalleryChannel>,
    api: Arc<AppApi>,
}

struct GlobalGalleryStore(Entity<GalleryStore>);
impl Global for GlobalGalleryStore {}

impl EventEmitter<GalleryEvent> for GalleryStore {}

impl GalleryStore {
    pub fn init(api: Arc<AppApi>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            by_channel: HashMap::new(),
            api,
        });
        cx.set_global(GlobalGalleryStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalGalleryStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalGalleryStore>().map(|g| g.0.clone())
    }

    pub fn api(&self) -> Arc<AppApi> {
        self.api.clone()
    }

    pub fn attachments(&self, channel_id: ChannelId) -> &[ChannelAttachment] {
        self.by_channel
            .get(&channel_id)
            .map(|c| c.attachments.as_slice())
            .unwrap_or(&[])
    }

    /// Attachments filtered by the given media tab.
    pub fn filtered(&self, channel_id: ChannelId, filter: MediaFilter) -> Vec<ChannelAttachment> {
        self.attachments(channel_id)
            .iter()
            .filter(|a| filter.matches(a))
            .cloned()
            .collect()
    }

    pub fn is_loading(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.is_loading)
    }

    pub fn has_more_before(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.has_more_before)
    }

    pub fn has_more_after(&self, channel_id: ChannelId) -> bool {
        self.by_channel
            .get(&channel_id)
            .is_some_and(|c| c.has_more_after)
    }

    pub fn date_range(&self, channel_id: ChannelId) -> Option<(u32, u32)> {
        self.by_channel.get(&channel_id).and_then(|c| c.date_range)
    }

    pub fn is_empty(&self, channel_id: ChannelId) -> bool {
        self.attachments(channel_id).is_empty()
    }

    /// Fetch the first page if the channel is not already loaded and fresh
    /// (React gallery button: fetch on first open).
    pub fn ensure_loaded(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let needs_fetch = match self.by_channel.get(&channel_id) {
            Some(c) => !c.is_loading && (c.attachments.is_empty() || !c.is_fresh()),
            None => true,
        };
        if needs_fetch {
            self.fetch(clan_id, channel_id, None, LoadDirection::Before, true, cx);
        }
    }

    pub fn refresh(&mut self, clan_id: ClanId, channel_id: ChannelId, cx: &mut Context<Self>) {
        let range = self.date_range(channel_id);
        self.fetch(clan_id, channel_id, range, LoadDirection::Before, true, cx);
    }

    /// Load the next page in `direction` for infinite scroll.
    pub fn fetch_page(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        direction: LoadDirection,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.by_channel.get(&channel_id) else {
            return;
        };
        if channel.is_loading {
            return;
        }
        match direction {
            LoadDirection::Before if !channel.has_more_before => return,
            LoadDirection::After if !channel.has_more_after => return,
            _ => {}
        }
        let range = channel.date_range;
        self.fetch(clan_id, channel_id, range, direction, false, cx);
    }

    /// Apply a from/to date filter (unix seconds) and refetch from scratch.
    pub fn apply_date_filter(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        after: Option<u32>,
        before: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let range = match (after, before) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or(0), b.unwrap_or(0))),
        };
        self.fetch(clan_id, channel_id, range, LoadDirection::Before, true, cx);
    }

    pub fn clear_date_filter(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.fetch(clan_id, channel_id, None, LoadDirection::Before, true, cx);
    }

    pub fn clear_attachments(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.reset_channel_attachments(channel_id) {
            cx.emit(GalleryEvent::Changed(channel_id));
            cx.notify();
        }
    }

    pub fn reset_channel_attachments(&mut self, channel_id: ChannelId) -> bool {
        if let Some(channel) = self.by_channel.get_mut(&channel_id) {
            channel.attachments.clear();
            channel.date_range = None;
            channel.fetched_at = None;
            channel.is_loading = false;
            channel.has_more_before = true;
            channel.has_more_after = true;
            true
        } else {
            false
        }
    }

    pub fn clear_channel(&mut self, channel_id: ChannelId, cx: &mut Context<Self>) {
        if self.by_channel.remove(&channel_id).is_some() {
            cx.emit(GalleryEvent::Changed(channel_id));
            cx.notify();
        }
    }

    fn fetch(
        &mut self,
        clan_id: ClanId,
        channel_id: ChannelId,
        date_range: Option<(u32, u32)>,
        direction: LoadDirection,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        let (mut before, mut after) = date_range.map(|(a, b)| (b, a)).unwrap_or((0, 0));
        if !reset {
            let channel = self.by_channel.get(&channel_id);
            match direction {
                LoadDirection::Before => {
                    if let Some(oldest) = channel.and_then(|c| c.attachments.last()) {
                        before = oldest.create_time_seconds;
                    }
                }
                LoadDirection::After => {
                    if let Some(newest) = channel.and_then(|c| c.attachments.first()) {
                        after = newest.create_time_seconds;
                    }
                }
            }
        }

        let entry = self.by_channel.entry(channel_id).or_default();
        entry.is_loading = true;
        if reset {
            entry.date_range = date_range;
        }
        cx.emit(GalleryEvent::Changed(channel_id));
        cx.notify();

        let api = self.api.clone();
        cx.spawn(async move |this, cx| {
            let result = api
                .list_channel_attachments(
                    clan_id.0,
                    channel_id.0,
                    "",
                    0,
                    GALLERY_PAGE_SIZE,
                    before,
                    after,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                let cfg = AppConfig::try_global(cx);
                let entry = this.by_channel.entry(channel_id).or_default();
                entry.is_loading = false;
                match result {
                    Ok(list) => {
                        let mapped: Vec<ChannelAttachment> = match &cfg {
                            Some(cfg) => list
                                .into_iter()
                                .map(|a| ChannelAttachment::from_api(a, channel_id, clan_id, cfg))
                                .filter(ChannelAttachment::is_media)
                                .collect(),
                            None => Vec::new(),
                        };
                        let fetched = mapped.len();
                        let added =
                            merge_attachments(&mut entry.attachments, mapped, reset, direction);
                        let full_page = fetched as i32 >= GALLERY_PAGE_SIZE;
                        let progressed = added > 0;
                        if reset {
                            entry.has_more_before = full_page;
                            entry.has_more_after = false;
                        } else {
                            match direction {
                                LoadDirection::Before => {
                                    entry.has_more_before = full_page && progressed;
                                }
                                LoadDirection::After => {
                                    entry.has_more_after = full_page && progressed;
                                }
                            }
                        }
                        entry.fetched_at = Some(Instant::now());
                    }
                    Err(e) => {
                        tracing::error!("list_channel_attachments failed: {e}");
                    }
                }
                cx.emit(GalleryEvent::Changed(channel_id));
                cx.notify();
            });
        })
        .detach();
    }
}

fn attachment_desc_cmp(a: &ChannelAttachment, b: &ChannelAttachment) -> std::cmp::Ordering {
    a.create_time_seconds
        .cmp(&b.create_time_seconds)
        .reverse()
        .then_with(|| a.id.cmp(&b.id).reverse())
}

fn sort_desc_in_place(items: &mut [ChannelAttachment]) {
    items.sort_by(attachment_desc_cmp);
}

fn merge_two_desc_sorted(
    left: Vec<ChannelAttachment>,
    right: Vec<ChannelAttachment>,
) -> Vec<ChannelAttachment> {
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(l), Some(r)) => {
                if attachment_desc_cmp(l, r) == std::cmp::Ordering::Greater {
                    merged.push(right.next().unwrap());
                } else {
                    merged.push(left.next().unwrap());
                }
            }
            (Some(_), None) => {
                merged.extend(left);
                break;
            }
            (None, _) => {
                merged.extend(right);
                break;
            }
        }
    }
    merged
}

fn merge_attachments(
    existing: &mut Vec<ChannelAttachment>,
    incoming: Vec<ChannelAttachment>,
    reset: bool,
    direction: LoadDirection,
) -> usize {
    if reset {
        existing.clear();
    }
    let mut seen: std::collections::HashSet<i64> = existing.iter().map(|a| a.id).collect();
    let mut new_items = Vec::new();
    let mut added = 0;
    for att in incoming {
        if seen.insert(att.id) {
            new_items.push(att);
            added += 1;
        }
    }
    if reset {
        sort_desc_in_place(&mut new_items);
        *existing = new_items;
        return added;
    }
    if added == 0 {
        return 0;
    }
    sort_desc_in_place(&mut new_items);
    let taken = std::mem::take(existing);
    *existing = match direction {
        LoadDirection::Before => merge_two_desc_sorted(taken, new_items),
        LoadDirection::After => merge_two_desc_sorted(new_items, taken),
    };
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(id: i64, ts: u32, filetype: &str) -> ChannelAttachment {
        ChannelAttachment {
            id,
            channel_id: ChannelId(1),
            clan_id: ClanId(1),
            message_id: MessageId(id),
            uploader_id: UserId(7),
            url: format!("https://cdn.mezon.ai/{id}.png"),
            filename: format!("{id}.png"),
            filetype: filetype.to_string(),
            width: 100,
            height: 100,
            create_time_seconds: ts,
            is_image: filetype.starts_with("image/"),
            is_video: filetype.starts_with("video/"),
            thumb_src: SharedString::default(),
            viewer_src: SharedString::default(),
            day_label: SharedString::default(),
            day_index: 0,
            viewer_timestamp: SharedString::default(),
            uploader_name: SharedString::default(),
            uploader_avatar: SharedString::default(),
            uploader_avatar_raw: SharedString::default(),
        }
    }

    #[test]
    fn video_thumb_skips_imgproxy() {
        use mezon_client::transport::ApiChannelAttachment;

        let cfg = AppConfig::dev_defaults();
        let url = "https://cdn.mezon.ai/2016174228608389120/2072236531032002560.mov";
        let api = ApiChannelAttachment {
            url: url.into(),
            filetype: "video/quicktime".into(),
            ..ApiChannelAttachment::default()
        };
        let att = ChannelAttachment::from_api(api, ChannelId(1), ClanId(1), &cfg);
        assert!(att.is_video);
        assert_eq!(att.thumb_src.as_ref(), url);
        assert!(!att.thumb_src.contains("imgproxy"));
        assert_eq!(att.viewer_src.as_ref(), url);
    }

    #[test]
    fn image_thumb_uses_imgproxy() {
        use mezon_client::transport::ApiChannelAttachment;

        let cfg = AppConfig::dev_defaults();
        let api = ApiChannelAttachment {
            url: "https://cdn.mezon.ai/photo.png".into(),
            filetype: "image/png".into(),
            width: 800,
            height: 600,
            ..ApiChannelAttachment::default()
        };
        let att = ChannelAttachment::from_api(api, ChannelId(1), ClanId(1), &cfg);
        assert!(att.is_image);
        assert!(att.thumb_src.contains("dev-imgproxy"));
        assert!(att.viewer_src.contains("dev-imgproxy"));
    }

    #[test]
    fn merge_reset_replaces_and_sorts_desc() {
        let mut existing = vec![att(1, 100, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(2, 300, "image/png"), att(3, 200, "image/png")],
            true,
            LoadDirection::Before,
        );
        assert_eq!(added, 2);
        assert_eq!(
            existing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn merge_appends_older_dedup() {
        let mut existing = vec![att(2, 300, "image/png"), att(3, 200, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(3, 200, "image/png"), att(4, 100, "image/png")],
            false,
            LoadDirection::Before,
        );
        assert_eq!(added, 1);
        assert_eq!(
            existing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn merge_prepends_newer_dedup() {
        let mut existing = vec![att(2, 300, "image/png"), att(3, 200, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(2, 300, "image/png"), att(5, 400, "image/png")],
            false,
            LoadDirection::After,
        );
        assert_eq!(added, 1);
        assert_eq!(
            existing.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![5, 2, 3]
        );
    }

    #[test]
    fn merge_duplicate_page_returns_zero_before() {
        let mut existing = vec![att(2, 300, "image/png"), att(3, 200, "image/png")];
        let added = merge_attachments(
            &mut existing,
            vec![att(2, 300, "image/png"), att(3, 200, "image/png")],
            false,
            LoadDirection::Before,
        );
        assert_eq!(added, 0);
    }

    #[test]
    fn media_filter_matches() {
        let img = att(1, 1, "image/png");
        let vid = att(2, 2, "video/mp4");
        assert!(MediaFilter::All.matches(&img));
        assert!(MediaFilter::All.matches(&vid));
        assert!(MediaFilter::Image.matches(&img));
        assert!(!MediaFilter::Image.matches(&vid));
        assert!(MediaFilter::Video.matches(&vid));
        assert!(!MediaFilter::Video.matches(&img));
    }

    #[test]
    fn detects_video_and_image_types() {
        assert!(is_video_type("video/mp4", ""));
        assert!(is_video_type("", "https://cdn.mezon.ai/clip.mov"));
        assert!(is_image_type("image/png", ""));
        assert!(is_image_type("", "https://cdn.mezon.ai/pic.JPG"));
        assert!(!is_image_type(
            "application/pdf",
            "https://cdn.mezon.ai/a.pdf"
        ));
    }

    #[test]
    fn enrich_falls_back_to_anonymous() {
        let mut atts = vec![att(1, 1, "image/png")];
        enrich_uploader(&mut atts, |_| None);
        assert_eq!(atts[0].uploader_name, SharedString::from("Anonymous"));
    }
}
