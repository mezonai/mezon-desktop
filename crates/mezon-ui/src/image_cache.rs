use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use futures::future::{AbortHandle, Abortable};
use futures::{AsyncReadExt as _, FutureExt};
use gpui::{
    App, AppContext, Asset, AssetLogger, Context, Entity, Global, ImageCache, ImageCacheError,
    ImageCacheItem, RenderImage, Resource, Window, hash,
};
use indexmap::IndexMap;

#[derive(Default)]
struct PendingAtlasDrops(Vec<Arc<RenderImage>>);
impl Global for PendingAtlasDrops {}

pub(crate) fn queue_atlas_drop(cx: &mut App, image: Arc<RenderImage>) {
    cx.default_global::<PendingAtlasDrops>().0.push(image);
}

#[derive(Default)]
struct PendingAtlasReplaces(Vec<Arc<RenderImage>>);
impl Global for PendingAtlasReplaces {}

#[cfg_attr(target_os = "macos", expect(dead_code))]
pub(crate) fn queue_atlas_replace(cx: &mut App, image: Arc<RenderImage>) {
    let pending = &mut cx.default_global::<PendingAtlasReplaces>().0;
    if let Some(existing) = pending.iter_mut().find(|queued| queued.id == image.id) {
        *existing = image;
    } else {
        pending.push(image);
    }
}

pub fn flush_atlas_replaces(window: &mut Window, cx: &mut App) {
    let pending = std::mem::take(&mut cx.default_global::<PendingAtlasReplaces>().0);
    for image in pending {
        cx.update_render_image(&image, Some(window));
    }
}

const RELIEF_FLUSH_THRESHOLD_BYTES: u64 = 8 * 1024 * 1024;

pub fn flush_atlas_drops(window: &mut Window, cx: &mut App) {
    let pending = std::mem::take(&mut cx.default_global::<PendingAtlasDrops>().0);
    if pending.is_empty() {
        return;
    }
    let mut freed_bytes = 0u64;
    for image in pending {
        freed_bytes = freed_bytes.saturating_add(image_bytes(&image));
        cx.drop_image(image, Some(window));
    }
    if freed_bytes >= RELIEF_FLUSH_THRESHOLD_BYTES {
        release_freed_memory_to_os(cx);
    }
}

const IDLE_TRIM_INTERVAL: Duration = Duration::from_secs(30);
const IDLE_TRIM_TTL: Duration = Duration::from_secs(60);

/// Decode-completion notifies are coalesced per view: a burst of images
/// finishing across many frames (fast scroll, channel open) would otherwise
/// trigger one full re-render of the list per completion frame.
const DECODE_NOTIFY_DEBOUNCE: Duration = Duration::from_millis(50);

#[derive(Default)]
struct PendingDecodeNotifies(std::collections::HashSet<gpui::EntityId>);
impl Global for PendingDecodeNotifies {}

fn schedule_decode_notify(entity: gpui::EntityId, cx: &mut App) {
    if !cx
        .default_global::<PendingDecodeNotifies>()
        .0
        .insert(entity)
    {
        return;
    }
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        executor.timer(DECODE_NOTIFY_DEBOUNCE).await;
        cx.update(|cx| {
            cx.default_global::<PendingDecodeNotifies>()
                .0
                .remove(&entity);
            cx.notify(entity);
        });
    })
    .detach();
}

#[derive(Default)]
struct IdleTrimRegistry(Vec<gpui::WeakEntity<LruImageCache>>);
impl Global for IdleTrimRegistry {}

static IDLE_TRIM_STARTED: AtomicBool = AtomicBool::new(false);

pub fn start_idle_trim(cx: &mut App) {
    if IDLE_TRIM_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            executor.timer(IDLE_TRIM_INTERVAL).await;
            cx.update(|cx| {
                let registry = std::mem::take(&mut cx.default_global::<IdleTrimRegistry>().0);
                let mut live = Vec::with_capacity(registry.len());
                let mut evicted = false;
                for weak in registry {
                    if let Some(cache) = weak.upgrade() {
                        evicted |=
                            cache.update(cx, |cache, cx| cache.evict_idle(IDLE_TRIM_TTL, cx));
                        live.push(weak);
                    }
                }
                cx.default_global::<IdleTrimRegistry>().0.extend(live);
                if evicted {
                    cx.refresh_windows();
                }
            });
        }
    })
    .detach();
}

const SHARED_AVATAR_CACHE_CAPACITY: usize = 512;
const SHARED_AVATAR_CACHE_BYTES: u64 = 24 * 1024 * 1024;
const SHARED_SMALL_AVATAR_CACHE_BYTES: u64 = 12 * 1024 * 1024;

struct SharedAvatarCache(Entity<LruImageCache>);
impl Global for SharedAvatarCache {}

struct SharedOgpCache(Entity<LruImageCache>);
impl Global for SharedOgpCache {}

const OGP_SHARED_CACHE_CAPACITY: usize = 16;
const OGP_SHARED_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const OGP_SHARED_ENTRY_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// App-global, bounded, aspect-preserving cache for OGP link-preview images
/// (decode-capped at [`OGP_THUMB_DECODE_MAX_PX`]), so previews never share/evict
/// the message image cache and large external OG images decode downscaled. Must
/// be initialized once with a `&mut App`; read from render via [`ogp_image_cache`].
pub fn shared_ogp_cache(cx: &mut App) -> Entity<LruImageCache> {
    if let Some(existing) = cx.try_global::<SharedOgpCache>() {
        return existing.0.clone();
    }
    let cache = cx.new(|cx| {
        LruImageCache::ogp_thumbnail(
            "ogp-shared",
            OGP_SHARED_CACHE_CAPACITY,
            OGP_SHARED_CACHE_BYTES,
            OGP_SHARED_ENTRY_MAX_BYTES,
            cx,
        )
    });
    cx.set_global(SharedOgpCache(cache.clone()));
    cache
}

/// The app-global OGP image cache if [`shared_ogp_cache`] has been initialized.
pub fn ogp_image_cache(app: &App) -> Option<Entity<LruImageCache>> {
    app.try_global::<SharedOgpCache>()
        .map(|cache| cache.0.clone())
}

pub fn sweep_ogp_cache(window: &mut Window, cx: &mut App) {
    if let Some(cache) = ogp_image_cache(cx) {
        cache.update(cx, |cache, cx| cache.sweep_once_per_frame(window, cx));
    }
}

pub fn shared_avatar_cache(cx: &mut App) -> Entity<LruImageCache> {
    if let Some(existing) = cx.try_global::<SharedAvatarCache>() {
        return existing.0.clone();
    }
    let cache = cx.new(|cx| {
        LruImageCache::avatar_thumbnail(
            "avatar-shared",
            SHARED_AVATAR_CACHE_CAPACITY,
            SHARED_AVATAR_CACHE_BYTES,
            AVATAR_ENTRY_MAX_BYTES,
            cx,
        )
    });
    cx.set_global(SharedAvatarCache(cache.clone()));
    cache
}

struct SharedSmallAvatarCache(Entity<LruImageCache>);
impl Global for SharedSmallAvatarCache {}

pub fn shared_small_avatar_cache(cx: &mut App) -> Entity<LruImageCache> {
    if let Some(existing) = cx.try_global::<SharedSmallAvatarCache>() {
        return existing.0.clone();
    }
    let cache = cx.new(|cx| {
        LruImageCache::avatar_thumbnail_small(
            "avatar-shared-small",
            SHARED_AVATAR_CACHE_CAPACITY,
            SHARED_SMALL_AVATAR_CACHE_BYTES,
            AVATAR_ENTRY_MAX_BYTES,
            cx,
        )
    });
    cx.set_global(SharedSmallAvatarCache(cache.clone()));
    cache
}

pub fn clear_all_image_caches(cx: &mut App) {
    let registry = std::mem::take(&mut cx.default_global::<IdleTrimRegistry>().0);
    let mut live = Vec::with_capacity(registry.len());
    for weak in registry {
        if let Some(cache) = weak.upgrade() {
            cache.update(cx, |cache, cx| cache.clear_app(cx));
            live.push(weak);
        }
    }
    cx.default_global::<IdleTrimRegistry>().0.extend(live);
}

#[cfg(target_os = "macos")]
mod os_mem {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn malloc_default_zone() -> *mut c_void;
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    pub fn release_freed_pages() {
        unsafe {
            let zone = malloc_default_zone();
            if !zone.is_null() {
                malloc_zone_pressure_relief(zone, 0);
            }
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod os_mem {
    pub fn release_freed_pages() {
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

#[cfg(target_os = "windows")]
mod os_mem {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetProcessHeap() -> *mut c_void;
        fn HeapCompact(heap: *mut c_void, flags: u32) -> usize;
    }

    pub fn release_freed_pages() {
        unsafe {
            let heap = GetProcessHeap();
            if !heap.is_null() {
                HeapCompact(heap, 0);
            }
        }
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", target_env = "gnu")
))]
static MEMORY_RELIEF_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub fn release_freed_memory_to_os(cx: &mut App) {
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", target_env = "gnu")
    ))]
    {
        if MEMORY_RELIEF_IN_FLIGHT.swap(true, Ordering::AcqRel) {
            return;
        }
        cx.background_executor()
            .spawn(async {
                os_mem::release_freed_pages();
                MEMORY_RELIEF_IN_FLIGHT.store(false, Ordering::Release);
            })
            .detach();
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", target_env = "gnu")
    )))]
    let _ = cx;
}

pub(crate) const AVATAR_FETCH_MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const GALLERY_FETCH_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MESSAGE_FETCH_MAX_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const VIEWER_FETCH_MAX_BYTES: usize = 32 * 1024 * 1024;
const IMAGE_PIPELINE_CONCURRENCY: usize = 3;
static IMAGE_PIPELINE_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(IMAGE_PIPELINE_CONCURRENCY)));

async fn acquire_image_pipeline_permit()
-> Result<tokio::sync::OwnedSemaphorePermit, ImageCacheError> {
    IMAGE_PIPELINE_PERMITS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| {
            ImageCacheError::Other(Arc::new(anyhow::anyhow!("image pipeline semaphore closed")))
        })
}

pub(crate) async fn read_body_limited(
    response: &mut gpui::http_client::Response<gpui::http_client::AsyncBody>,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    if let Some(length) = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > limit as u64
    {
        return Err(std::io::Error::other(format!(
            "response body of {length} bytes exceeds the {limit} byte transfer limit"
        )));
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .take(limit as u64 + 1)
        .read_to_end(&mut body)
        .await?;
    if body.len() > limit {
        return Err(std::io::Error::other(format!(
            "response body exceeds the {limit} byte transfer limit"
        )));
    }
    Ok(body)
}

pub const MESSAGE_IMAGE_CACHE_CAPACITY: usize = 48;
pub const MESSAGE_IMAGE_CACHE_BYTES: u64 = 32 * 1024 * 1024;
pub const AVATAR_IMAGE_CACHE_CAPACITY: usize = 256;
pub const AVATAR_IMAGE_CACHE_BYTES: u64 = 8 * 1024 * 1024;

pub const VIEWER_IMAGE_CACHE_CAPACITY: usize = 24;
pub const VIEWER_IMAGE_CACHE_BYTES: u64 = 32 * 1024 * 1024;
pub const VIEWER_IMAGE_ENTRY_MAX_BYTES: u64 = 24 * 1024 * 1024;

/// App-wide fallback cache attached at the root, so any `img`/avatar that does
/// not declare its own cache uses this bounded LRU instead of GPUI's unbounded
/// global asset cache (which never evicts and leaks RAM for every URL seen).
pub const SHARED_IMAGE_CACHE_CAPACITY: usize = 384;
pub const SHARED_IMAGE_CACHE_BYTES: u64 = 24 * 1024 * 1024;
pub const GALLERY_IMAGE_CACHE_CAPACITY: usize = 48;
pub const GALLERY_IMAGE_CACHE_BYTES: u64 = 12 * 1024 * 1024;

pub const PREVIEW_IMAGE_CACHE_CAPACITY: usize = 64;
pub const PREVIEW_IMAGE_CACHE_BYTES: u64 = 32 * 1024 * 1024;
pub const PREVIEW_ENTRY_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Per-image decoded-size caps. A compressed file is tiny on the wire but is
/// stored uncompressed in RAM as `width * height * 4` bytes *per frame*. An
/// animated GIF/WebP therefore explodes: a ~400 KB animated avatar can decode
/// to hundreds of MB once every frame is expanded. When the resizing image
/// proxy is unavailable (dev, or a prod outage) we fall back to the raw,
/// full-resolution file, so we guard against a single pathological image
/// blowing up RAM by refusing to retain anything decoded larger than this and
/// negatively caching it (shown as the initials fallback instead).
pub const AVATAR_ENTRY_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const MESSAGE_ENTRY_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const SHARED_ENTRY_MAX_BYTES: u64 = 12 * 1024 * 1024;

const GRACE_PERIOD: Duration = Duration::from_secs(2);
const STATS_LOG_INTERVAL: u64 = 600;
const MESSAGE_ANIMATION_MAX_PX: u32 = 400;
const MESSAGE_STATIC_MAX_PX: u32 = 1024;
const SHARED_ANIMATION_MAX_PX: u32 = 400;
const SHARED_STATIC_MAX_PX: u32 = 2048;
/// Longest side (px) that an animated GIF/WebP is downscaled to for the image
/// viewer. Larger than the message cap since the viewer shows media bigger,
/// but still bounded so a long animation cannot expand to hundreds of MB.
const VIEWER_ANIMATION_MAX_PX: u32 = 480;
const VIEWER_STATIC_MAX_PX: u32 = 1600;

#[derive(Default)]
struct CacheMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub current_bytes: u64,
    pub items: usize,
}

struct CacheEntry {
    item: ImageCacheItem,
    abort: AbortHandle,
    /// Decoded size in bytes, once the image has finished loading.
    bytes: Option<u64>,
    /// The sweep epoch in which this entry was last requested.
    touched_epoch: u64,
    last_used: Instant,
    /// When a transient load failure was first observed. Deterministic
    /// failures — oversized-image rejections (`bytes == Some(0)`), canvas/
    /// dimension guards (`Asset`), decode/limits errors (`Image`), bad SVGs
    /// (`Usvg`) — are never retried; only network-shaped failures (`Io`,
    /// `BadStatus`, `Other`) are retried once per
    /// [`NEGATIVE_CACHE_RETRY_TTL`] while the image keeps being requested.
    failed_at: Option<Instant>,
}

const NEGATIVE_CACHE_RETRY_TTL: Duration = Duration::from_secs(15);

/// Sum of the decoded byte size across all frames of an image.
fn image_bytes(image: &RenderImage) -> u64 {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .map(|buf| buf.len() as u64)
        .sum()
}

fn entry_is_stale(touched_epoch: u64, epoch: u64, age: Duration, grace: Duration) -> bool {
    touched_epoch != epoch && age > grace
}

fn entry_is_idle(touched_epoch: u64, epoch: u64, age: Duration, ttl: Duration) -> bool {
    touched_epoch != epoch && age > ttl
}

/// An LRU image cache bounded by both an item count and a decoded-byte budget.
///
/// The byte budget is what actually keeps RAM in check: large attachments are
/// evicted as soon as the total decoded size exceeds `max_bytes`, instead of
/// lingering until the (much larger) item count or a channel switch clears them.
static CACHE_INSTANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Which decoder a cache uses to turn a resource into a `RenderImage`.
#[derive(Clone, Copy)]
enum LoaderKind {
    /// Bounded general-purpose loader for the app-wide fallback cache (OGP
    /// embeds, misc `img()` without an explicit cache): static images capped at
    /// [`SHARED_STATIC_MAX_PX`], animations at [`SHARED_ANIMATION_MAX_PX`] with
    /// an in-decode byte budget so a pathological file cannot spike RAM while
    /// decoding.
    Full,
    /// Decodes only the first frame and downscales to avatar size, so even an
    /// animated full-resolution source costs ~100 KB of RAM. Used for avatars.
    AvatarThumbnail,
    AvatarThumbnailSmall,
    IconThumbnail,
    GalleryThumbnail,
    /// Aspect-preserving thumbnail for OGP link previews, capped at
    /// [`OGP_THUMB_DECODE_MAX_PX`].
    OgpThumbnail,
    /// Aspect-preserving thumbnail for Timeline/Events/Event-Detail preview
    /// cards, capped at [`GALLERY_PREVIEW_DECODE_MAX_PX`].
    GalleryPreview,
    Message,
    /// The image-viewer loader: still images keep near-full resolution
    /// ([`VIEWER_STATIC_MAX_PX`]); animated GIF/WebP keep every frame so they
    /// animate, but downscaled to [`VIEWER_ANIMATION_MAX_PX`] and bounded by an
    /// in-decode byte budget.
    Viewer,
}

pub struct LruImageCache {
    label: &'static str,
    instance: u64,
    loader: LoaderKind,
    max_items: usize,
    max_bytes: u64,
    /// Largest decoded size (bytes, summed across frames) a single entry may
    /// have before it is dropped and negatively cached. Protects against a
    /// single huge/animated image consuming hundreds of MB.
    max_entry_bytes: u64,
    total_bytes: u64,
    epoch: u64,
    sweeps: u64,
    sweep_scheduled: bool,
    metrics: CacheMetrics,
    cache: IndexMap<u64, CacheEntry>,
}

impl LruImageCache {
    pub fn new(max_items: usize, max_bytes: u64, cx: &mut Context<Self>) -> Self {
        Self::labeled("image", max_items, max_bytes, u64::MAX, cx)
    }

    pub fn labeled(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::Full,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    /// A cache for avatars: decodes only the first frame and downscales to
    /// avatar size, so animated or oversized sources can never blow up RAM.
    pub fn avatar_thumbnail(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::AvatarThumbnail,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    pub fn avatar_thumbnail_small(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::AvatarThumbnailSmall,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    pub fn icon_thumbnail(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::IconThumbnail,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    pub fn gallery_thumbnail(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::GalleryThumbnail,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    pub fn ogp_thumbnail(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::OgpThumbnail,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    /// A cache for Timeline/Events/Event-Detail preview cards: aspect-preserving,
    /// downscaled to [`GALLERY_PREVIEW_DECODE_MAX_PX`] so landscape banners and
    /// square grid cells both stay sharp under `object-fit: cover` without the
    /// full-resolution decode blowing the cache's byte budget.
    pub fn gallery_preview(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::GalleryPreview,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    pub fn message(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::Message,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    /// A cache for the image viewer: decodes only the first frame at full
    /// resolution. The viewer renders a single static frame, so this avoids
    /// retaining every frame of an animated GIF/WebP (which the viewer never
    /// shows) while keeping full-resolution quality for still images.
    pub fn viewer(
        label: &'static str,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_loader(
            label,
            LoaderKind::Viewer,
            max_items,
            max_bytes,
            max_entry_bytes,
            cx,
        )
    }

    fn with_loader(
        label: &'static str,
        loader: LoaderKind,
        max_items: usize,
        max_bytes: u64,
        max_entry_bytes: u64,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|cache, cx| {
            for (_, mut entry) in std::mem::take(&mut cache.cache) {
                entry.abort.abort();
                if let Some(Ok(image)) = entry.item.get() {
                    queue_atlas_drop(cx, image);
                }
            }
        })
        .detach();

        let instance = CACHE_INSTANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let weak = cx.weak_entity();
        cx.default_global::<IdleTrimRegistry>().0.push(weak);
        Self {
            label,
            instance,
            loader,
            max_items,
            max_bytes,
            max_entry_bytes,
            total_bytes: 0,
            epoch: 0,
            sweeps: 0,
            sweep_scheduled: false,
            metrics: CacheMetrics::default(),
            cache: IndexMap::with_capacity(max_items),
        }
    }

    pub fn stats(&self) -> ImageCacheStats {
        ImageCacheStats {
            hits: self.metrics.hits.load(Ordering::Relaxed),
            misses: self.metrics.misses.load(Ordering::Relaxed),
            evictions: self.metrics.evictions.load(Ordering::Relaxed),
            current_bytes: self.total_bytes,
            items: self.cache.len(),
        }
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        for (_, mut entry) in std::mem::take(&mut self.cache) {
            entry.abort.abort();
            if let Some(Ok(image)) = entry.item.get() {
                cx.drop_image(image, Some(window));
            }
        }
        self.total_bytes = 0;
    }

    pub fn clear_app(&mut self, cx: &mut App) {
        for (_, mut entry) in std::mem::take(&mut self.cache) {
            entry.abort.abort();
            if let Some(Ok(image)) = entry.item.get() {
                queue_atlas_drop(cx, image);
            }
        }
        self.total_bytes = 0;
    }

    /// Drop every image that was not requested during the most recent frame,
    /// then advance the epoch. Call this once per render: anything that has
    /// scrolled out of the viewport stops being requested and is freed on the
    /// next sweep, so only the currently-visible images stay in RAM.
    pub fn sweep(&mut self, window: &mut Window, cx: &mut App) {
        self.sweep_with_grace(GRACE_PERIOD, window, cx);
    }

    pub fn sweep_once_per_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sweep_scheduled {
            return;
        }
        self.sweep_scheduled = true;
        self.sweep(window, cx);
        cx.on_next_frame(window, |cache, _, _| cache.sweep_scheduled = false);
    }

    fn sweep_with_grace(&mut self, grace: Duration, window: &mut Window, cx: &mut App) {
        let epoch = self.epoch;
        let metrics = &self.metrics;
        let total_bytes = &mut self.total_bytes;
        self.cache.retain(|_, entry| {
            if !entry_is_stale(entry.touched_epoch, epoch, entry.last_used.elapsed(), grace) {
                return true;
            }
            entry.abort.abort();
            metrics.evictions.fetch_add(1, Ordering::Relaxed);
            if let Some(bytes) = entry.bytes {
                *total_bytes = total_bytes.saturating_sub(bytes);
            }
            if let Some(Ok(image)) = entry.item.get() {
                cx.drop_image(image, Some(&mut *window));
            }
            false
        });
        self.epoch = self.epoch.wrapping_add(1);
        self.sweeps = self.sweeps.wrapping_add(1);
        if self.sweeps.is_multiple_of(STATS_LOG_INTERVAL) {
            let stats = self.stats();
            tracing::debug!(
                label = self.label,
                instance = self.instance,
                hits = stats.hits,
                misses = stats.misses,
                evictions = stats.evictions,
                current_bytes = stats.current_bytes,
                items = stats.items,
                "image cache stats"
            );
        }
    }

    fn evict_idle(&mut self, ttl: Duration, cx: &mut App) -> bool {
        let previous_len = self.cache.len();
        let metrics = &self.metrics;
        let total_bytes = &mut self.total_bytes;
        self.cache.retain(|_, entry| {
            if !entry_is_idle(
                entry.touched_epoch,
                self.epoch,
                entry.last_used.elapsed(),
                ttl,
            ) {
                return true;
            }
            entry.abort.abort();
            metrics.evictions.fetch_add(1, Ordering::Relaxed);
            if let Some(bytes) = entry.bytes {
                *total_bytes = total_bytes.saturating_sub(bytes);
            }
            if let Some(Ok(image)) = entry.item.get() {
                queue_atlas_drop(cx, image);
            }
            false
        });
        self.cache.len() < previous_len
    }

    pub fn shrink_to(&mut self, max_bytes: u64, window: &mut Window, cx: &mut App) {
        while self.total_bytes > max_bytes {
            let Some(victim) = self.lru_index() else {
                break;
            };
            let Some((_, mut evicted)) = self.cache.swap_remove_index(victim) else {
                break;
            };
            evicted.abort.abort();
            self.metrics.evictions.fetch_add(1, Ordering::Relaxed);
            if let Some(bytes) = evicted.bytes {
                self.total_bytes = self.total_bytes.saturating_sub(bytes);
            }
            if let Some(Ok(image)) = evicted.item.get() {
                cx.drop_image(image, Some(window));
            }
        }
    }

    /// Evict least-recently-used entries until both the item-count and
    /// byte budgets are satisfied. The victim is the entry with the oldest
    /// `last_used` timestamp (map order no longer tracks recency); the final
    /// remaining entry is never evicted, so the image requested this frame
    /// stays resident.
    fn lru_index(&self) -> Option<usize> {
        self.cache
            .values()
            .enumerate()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(index, _)| index)
    }

    fn evict_to_budget(&mut self, window: &mut Window, cx: &mut App) {
        while self.cache.len() > self.max_items
            || (self.total_bytes > self.max_bytes && self.cache.len() > 1)
        {
            let Some(victim) = self.lru_index() else {
                break;
            };
            let Some((_, mut evicted)) = self.cache.swap_remove_index(victim) else {
                break;
            };
            evicted.abort.abort();
            self.metrics.evictions.fetch_add(1, Ordering::Relaxed);
            if let Some(bytes) = evicted.bytes {
                self.total_bytes = self.total_bytes.saturating_sub(bytes);
            }
            if let Some(Ok(image)) = evicted.item.get() {
                cx.drop_image(image, Some(window));
            }
        }
    }

    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let hash = hash(resource);

        if self.cache.contains_key(&hash) {
            let retry_failed = {
                let entry = self.cache.get_mut(&hash).expect("checked contains_key");
                let transient_failure = entry.bytes.is_none()
                    && matches!(
                        entry.item.get(),
                        Some(Err(ImageCacheError::Io(_)
                            | ImageCacheError::BadStatus { .. }
                            | ImageCacheError::Other(_)))
                    );
                if transient_failure {
                    match entry.failed_at {
                        Some(failed_at) => failed_at.elapsed() >= NEGATIVE_CACHE_RETRY_TTL,
                        None => {
                            entry.failed_at = Some(Instant::now());
                            false
                        }
                    }
                } else {
                    false
                }
            };
            if retry_failed {
                self.cache.swap_remove(&hash);
            }
        }

        if self.cache.contains_key(&hash) {
            self.metrics.hits.fetch_add(1, Ordering::Relaxed);

            enum Measured {
                /// Nothing new to account for (already measured, or still loading).
                None,
                /// Newly decoded image of the given size, kept in the cache.
                Kept(u64),
                /// Newly decoded image exceeded the per-entry cap: dropped and
                /// negatively cached. Carries the image to free + the error.
                TooLarge(Arc<RenderImage>, ImageCacheError),
            }

            let (res, measured) = {
                let entry = self.cache.get_mut(&hash).expect("checked contains_key");
                entry.touched_epoch = self.epoch;
                entry.last_used = Instant::now();
                let res = entry.item.get();
                let measured = if entry.bytes.is_none()
                    && let Some(Ok(image)) = res.as_ref()
                {
                    let bytes = image_bytes(image);
                    if bytes > self.max_entry_bytes {
                        let err = ImageCacheError::Other(Arc::new(anyhow::anyhow!(
                            "image decoded to {bytes} bytes, exceeds per-entry cap of {} bytes",
                            self.max_entry_bytes
                        )));
                        entry.item = ImageCacheItem::Loaded(Err(err.clone()));
                        entry.bytes = Some(0);
                        Measured::TooLarge(image.clone(), err)
                    } else {
                        entry.bytes = Some(bytes);
                        Measured::Kept(bytes)
                    }
                } else {
                    Measured::None
                };
                (res, measured)
            };
            match measured {
                Measured::Kept(bytes) => {
                    self.total_bytes = self.total_bytes.saturating_add(bytes);
                    self.evict_to_budget(window, cx);
                    return res;
                }
                Measured::TooLarge(image, err) => {
                    tracing::warn!(
                        "[imgcache:{}#{}] dropping oversized image: {}",
                        self.label,
                        self.instance,
                        err
                    );
                    cx.drop_image(image, Some(window));
                    return Some(Err(err));
                }
                Measured::None => return res,
            }
        }

        self.metrics.misses.fetch_add(1, Ordering::Relaxed);
        let loader = match self.loader {
            LoaderKind::Full => {
                AssetLogger::<SharedImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::AvatarThumbnail => {
                AssetLogger::<AvatarImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::AvatarThumbnailSmall => {
                AssetLogger::<AvatarImageLoaderSmall>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::IconThumbnail => {
                AssetLogger::<IconImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::GalleryThumbnail => {
                AssetLogger::<GalleryImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::OgpThumbnail => {
                AssetLogger::<OgpImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::GalleryPreview => {
                AssetLogger::<GalleryPreviewLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::Message => {
                AssetLogger::<MessageImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::Viewer => {
                AssetLogger::<ViewerImageLoader>::load(resource.clone(), cx).boxed()
            }
        };
        let task = cx.background_executor().spawn(loader).shared();
        let (abort_handle, abort_reg) = AbortHandle::new_pair();

        self.cache.insert(
            hash,
            CacheEntry {
                item: ImageCacheItem::Loading(task.clone()),
                abort: abort_handle,
                bytes: None,
                touched_epoch: self.epoch,
                last_used: Instant::now(),
                failed_at: None,
            },
        );
        self.evict_to_budget(window, cx);

        let entity = window.current_view();
        let notify_task = task.clone();
        window
            .spawn(cx, async move |cx| {
                let _ = Abortable::new(notify_task, abort_reg).await;
                let _ = cx.update(|_, cx| schedule_decode_notify(entity, cx));
            })
            .detach();

        None
    }
}

impl ImageCache for LruImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        LruImageCache::load(self, resource, window, cx)
    }
}

/// Largest dimension (device pixels) an avatar is ever drawn at: the biggest
/// avatar is 80px logical, which is 160px on a 2x display. Decoding to this size
/// keeps a single avatar at ~100 KB regardless of the source file.
const AVATAR_DECODE_MAX_PX: u32 = 160;
const AVATAR_SMALL_DECODE_MAX_PX: u32 = 80;
const ICON_DECODE_MAX_PX: u32 = 64;
const GALLERY_THUMB_DECODE_MAX_PX: u32 = 320;
/// OGP link-preview thumbnails render at ≤200px tall; decode to 512px longest
/// side (aspect-preserving, ~2x for retina) so a large external OG image
/// (typically 1200×630) can never decode oversized in the preview card.
const OGP_THUMB_DECODE_MAX_PX: u32 = 512;
/// Timeline/Events/Event-Detail preview cards render up to ~900px wide
/// (featured banner) at aspect ratio; decode to this longest side so the
/// featured image stays sharp while grid/card thumbnails (much smaller on
/// screen) downscale further for free via `object-fit: cover`.
const GALLERY_PREVIEW_DECODE_MAX_PX: u32 = 768;

/// An [`Asset`] loader for avatars that, unlike GPUI's stock [`ImageAssetLoader`],
/// decodes **only the first frame** and **downscales** to avatar size before
/// building the `RenderImage`.
///
/// GPUI's loader expands every frame of an animated GIF/WebP to
/// `width * height * 4` uncompressed bytes and keeps them all, so a ~400 KB
/// animated avatar can decode to hundreds of MB. Avatars never need animation
/// or full resolution, so we sidestep that entirely: `image::load_from_memory`
/// reads a single frame even for animated formats, and we shrink it to at most
/// [`AVATAR_DECODE_MAX_PX`]. The result is a tiny, static image that cannot blow
/// up RAM even when the resizing image proxy is unavailable and we fall back to
/// the raw source file.
fn load_avatar_scaled(
    source: Resource,
    max_px: u32,
    cx: &mut App,
) -> impl Future<Output = Result<Arc<RenderImage>, ImageCacheError>> + Send + 'static {
    let client = cx.http_client();
    let svg_renderer = cx.svg_renderer();
    let asset_source = cx.asset_source().clone();
    async move {
        let _permit = acquire_image_pipeline_permit().await?;
        let bytes = match source.clone() {
            Resource::Path(uri) => {
                if let Some(decoded) = decode_scaled_dynamic_path(uri.as_ref(), max_px) {
                    return Ok(avatar_render_image(decoded, max_px));
                }
                std::fs::read(uri.as_ref())?
            }
            Resource::Uri(uri) => {
                use anyhow::Context as _;

                let mut response = client
                    .get(uri.as_ref(), ().into(), true)
                    .await
                    .with_context(|| format!("loading avatar from {uri:?}"))?;
                let body = read_body_limited(&mut response, AVATAR_FETCH_MAX_BYTES).await?;
                if !response.status().is_success() {
                    let mut body = String::from_utf8_lossy(&body).into_owned();
                    let first_line = body.lines().next().unwrap_or("").trim_end();
                    body.truncate(first_line.len());
                    return Err(ImageCacheError::BadStatus {
                        uri,
                        status: response.status(),
                        body,
                    });
                }
                body
            }
            Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                Some(data) => data.to_vec(),
                None => {
                    return Err(ImageCacheError::Asset(
                        format!("Embedded resource not found: {path}").into(),
                    ));
                }
            },
        };

        if image::guess_format(&bytes).is_ok() {
            let decoded = match decode_scaled_dynamic(&bytes, max_px) {
                Some(image) => image,
                None => image::load_from_memory(&bytes)?,
            };
            Ok(avatar_render_image(decoded, max_px))
        } else {
            svg_renderer
                .render_single_frame(&bytes, 1.0)
                .map_err(Into::into)
        }
    }
}

pub enum AvatarImageLoader {}

impl Asset for AvatarImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        load_avatar_scaled(source, AVATAR_DECODE_MAX_PX, cx)
    }
}

pub enum AvatarImageLoaderSmall {}

impl Asset for AvatarImageLoaderSmall {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        load_avatar_scaled(source, AVATAR_SMALL_DECODE_MAX_PX, cx)
    }
}

pub enum IconImageLoader {}

impl Asset for IconImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        load_avatar_scaled(source, ICON_DECODE_MAX_PX, cx)
    }
}

pub enum GalleryImageLoader {}

impl Asset for GalleryImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let _permit = acquire_image_pipeline_permit().await?;
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading gallery image from {uri:?}"))?;
                    let body = read_body_limited(&mut response, GALLERY_FETCH_MAX_BYTES).await?;
                    if !response.status().is_success() {
                        let mut body = String::from_utf8_lossy(&body).into_owned();
                        let first_line = body.lines().next().unwrap_or("").trim_end();
                        body.truncate(first_line.len());
                        return Err(ImageCacheError::BadStatus {
                            uri,
                            status: response.status(),
                            body,
                        });
                    }
                    body
                }
                Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                    Some(data) => data.to_vec(),
                    None => {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {path}").into(),
                        ));
                    }
                },
            };

            if image::guess_format(&bytes).is_ok() {
                let decoded = match decode_scaled_dynamic(&bytes, GALLERY_THUMB_DECODE_MAX_PX) {
                    Some(image) => image,
                    None => image::load_from_memory(&bytes)?,
                };
                let side = decoded
                    .width()
                    .min(decoded.height())
                    .clamp(1, GALLERY_THUMB_DECODE_MAX_PX);
                let mut data = decoded
                    .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
                    .into_rgba8();
                for pixel in data.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                Ok(Arc::new(RenderImage::new(vec![image::Frame::new(data)])))
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
    }
}

fn downscale_dimensions(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || longest <= max_px {
        (width.max(1), height.max(1))
    } else {
        let scale = max_px as f32 / longest as f32;
        (
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
        )
    }
}

fn bgra_frame(decoded: image::DynamicImage) -> image::Frame {
    let mut data = decoded.into_rgba8();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    image::Frame::new(data)
}

fn downscaled_static_frame(decoded: image::DynamicImage, max_px: u32) -> image::Frame {
    let (tw, th) = downscale_dimensions(decoded.width(), decoded.height(), max_px);
    let decoded = if tw == decoded.width() && th == decoded.height() {
        decoded
    } else {
        decoded.resize(tw, th, image::imageops::FilterType::Triangle)
    };
    bgra_frame(decoded)
}

enum AnimationDecodeError {
    BudgetExceeded,
    Image(ImageCacheError),
}

fn downscaled_animation_frames<I>(
    frames: I,
    max_px: u32,
    byte_budget: u64,
) -> Result<Vec<image::Frame>, AnimationDecodeError>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    let mut out: Vec<image::Frame> = Vec::new();
    let mut target: Option<(u32, u32)> = None;
    let mut decoded_bytes: u64 = 0;
    for frame in frames {
        let frame = frame.map_err(|err| AnimationDecodeError::Image(err.into()))?;
        let delay = frame.delay();
        let buffer = frame.into_buffer();
        let (tw, th) = *target
            .get_or_insert_with(|| downscale_dimensions(buffer.width(), buffer.height(), max_px));
        decoded_bytes = decoded_bytes.saturating_add(u64::from(tw) * u64::from(th) * 4);
        if decoded_bytes > byte_budget {
            return Err(AnimationDecodeError::BudgetExceeded);
        }
        let mut buffer = if buffer.width() == tw && buffer.height() == th {
            buffer
        } else {
            image::imageops::resize(&buffer, tw, th, image::imageops::FilterType::Triangle)
        };
        for pixel in buffer.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        out.push(image::Frame::from_parts(buffer, 0, 0, delay));
    }
    if out.is_empty() {
        return Err(AnimationDecodeError::Image(ImageCacheError::Other(
            Arc::new(anyhow::anyhow!("animation decoded to zero frames")),
        )));
    }
    Ok(out)
}

fn scaled_to_dynamic(scaled: mezon_video::ScaledImage) -> Option<image::DynamicImage> {
    let mut rgba = scaled.bgra;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(scaled.width, scaled.height, rgba)?;
    Some(image::DynamicImage::ImageRgba8(buffer))
}

fn decode_scaled_dynamic(bytes: &[u8], max_px: u32) -> Option<image::DynamicImage> {
    scaled_to_dynamic(mezon_video::scaled_image_decode(bytes, max_px)?)
}

fn decode_scaled_dynamic_path(path: &std::path::Path, max_px: u32) -> Option<image::DynamicImage> {
    scaled_to_dynamic(mezon_video::scaled_image_decode_path(path, max_px)?)
}

fn avatar_render_image(decoded: image::DynamicImage, max_px: u32) -> Arc<RenderImage> {
    let side = decoded.width().min(decoded.height()).clamp(1, max_px);
    let mut data = decoded
        .resize_to_fill(side, side, image::imageops::FilterType::Triangle)
        .into_rgba8();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(data)]))
}

fn decode_static_image(
    bytes: &[u8],
    format: image::ImageFormat,
    max_px: u32,
) -> Result<image::DynamicImage, ImageCacheError> {
    if let Some(scaled) = decode_scaled_dynamic(bytes, max_px) {
        return Ok(scaled);
    }
    Ok(image::load_from_memory_with_format(bytes, format)?)
}

const MAX_DECODE_PIXELS: u64 = 48_000_000;
const MAX_DECODER_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

fn decoder_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_DECODER_ALLOC_BYTES);
    limits
}

fn reject_oversized_canvas(
    bytes: &[u8],
    format: image::ImageFormat,
) -> Result<(), ImageCacheError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.set_format(format);
    let (width, height) = reader.into_dimensions()?;
    if width > 16_384 || height > 16_384 {
        return Err(ImageCacheError::Asset(
            format!("image dimension too large to decode: {width}x{height}").into(),
        ));
    }
    if width as u64 * height as u64 > MAX_DECODE_PIXELS {
        return Err(ImageCacheError::Asset(
            format!("image dimensions too large to decode: {width}x{height}").into(),
        ));
    }
    Ok(())
}

fn decode_message_image(
    bytes: &[u8],
    animation_max_px: u32,
    static_max_px: u32,
    animation_byte_budget: u64,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    use image::AnimationDecoder as _;
    let format = image::guess_format(bytes)?;
    reject_oversized_canvas(bytes, format)?;
    let frames = match format {
        image::ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))?;
            image::ImageDecoder::set_limits(&mut decoder, decoder_limits())?;
            match downscaled_animation_frames(
                decoder.into_frames(),
                animation_max_px,
                animation_byte_budget,
            ) {
                Ok(frames) => frames,
                Err(AnimationDecodeError::BudgetExceeded) => {
                    let mut decoder =
                        image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))?;
                    image::ImageDecoder::set_limits(&mut decoder, decoder_limits())?;
                    first_frame_fallback(decoder, static_max_px)?
                }
                Err(AnimationDecodeError::Image(err)) => return Err(err),
            }
        }
        image::ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))?;
            image::ImageDecoder::set_limits(&mut decoder, decoder_limits())?;
            if decoder.has_animation() {
                match downscaled_animation_frames(
                    decoder.into_frames(),
                    animation_max_px,
                    animation_byte_budget,
                ) {
                    Ok(frames) => frames,
                    Err(AnimationDecodeError::BudgetExceeded) => {
                        let mut decoder =
                            image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))?;
                        image::ImageDecoder::set_limits(&mut decoder, decoder_limits())?;
                        first_frame_fallback(decoder, static_max_px)?
                    }
                    Err(AnimationDecodeError::Image(err)) => return Err(err),
                }
            } else {
                vec![downscaled_static_frame(
                    decode_static_image(bytes, format, static_max_px)?,
                    static_max_px,
                )]
            }
        }
        _ => vec![downscaled_static_frame(
            decode_static_image(bytes, format, static_max_px)?,
            static_max_px,
        )],
    };
    Ok(Arc::new(RenderImage::new(frames)))
}

fn first_frame_fallback<'a, D>(
    decoder: D,
    static_max_px: u32,
) -> Result<Vec<image::Frame>, ImageCacheError>
where
    D: image::AnimationDecoder<'a>,
{
    let frame = decoder
        .into_frames()
        .next()
        .ok_or_else(|| ImageCacheError::Asset("animation has no frames".into()))??;
    let downscaled = downscaled_static_frame(
        image::DynamicImage::ImageRgba8(frame.into_buffer()),
        static_max_px,
    );
    Ok(vec![downscaled])
}

fn message_path_maybe_animated(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("gif") | Some("webp")
    )
}

/// An [`Asset`] loader for message attachments. Animated GIF/WebP are decoded at
/// every frame so they still animate, but each frame is downscaled to at most
/// [`MESSAGE_ANIMATION_MAX_PX`], so a long high-resolution animation cannot
/// expand to hundreds of MB of decoded BGRA. Static images keep full resolution.
/// Fetch + decode an image downscaled to `max_px` on the longest side,
/// **preserving aspect ratio** (unlike [`avatar_render_image`], which crops to
/// a square). Used for OGP banner thumbnails.
fn load_scaled_aspect(
    source: Resource,
    max_px: u32,
    cx: &mut App,
) -> impl Future<Output = Result<Arc<RenderImage>, ImageCacheError>> + Send + 'static {
    let client = cx.http_client();
    let svg_renderer = cx.svg_renderer();
    let asset_source = cx.asset_source().clone();
    async move {
        let _permit = acquire_image_pipeline_permit().await?;
        use anyhow::Context as _;
        let bytes = match source.clone() {
            Resource::Path(uri) => std::fs::read(uri.as_ref())?,
            Resource::Uri(uri) => {
                let mut response = client
                    .get(uri.as_ref(), ().into(), true)
                    .await
                    .with_context(|| format!("loading ogp image from {uri:?}"))?;
                let body = read_body_limited(&mut response, GALLERY_FETCH_MAX_BYTES).await?;
                if !response.status().is_success() {
                    let mut body = String::from_utf8_lossy(&body).into_owned();
                    let first_line = body.lines().next().unwrap_or("").trim_end();
                    body.truncate(first_line.len());
                    return Err(ImageCacheError::BadStatus {
                        uri,
                        status: response.status(),
                        body,
                    });
                }
                body
            }
            Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                Some(data) => data.to_vec(),
                None => {
                    return Err(ImageCacheError::Asset(
                        format!("Embedded resource not found: {path}").into(),
                    ));
                }
            },
        };
        if image::guess_format(&bytes).is_ok() {
            let decoded = match decode_scaled_dynamic(&bytes, max_px) {
                Some(image) => image,
                None => image::load_from_memory(&bytes)?,
            };
            let mut data = decoded.into_rgba8();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(data)])))
        } else {
            svg_renderer
                .render_single_frame(&bytes, 1.0)
                .map_err(Into::into)
        }
    }
}

pub enum OgpImageLoader {}

impl Asset for OgpImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        load_scaled_aspect(source, OGP_THUMB_DECODE_MAX_PX, cx)
    }
}

pub enum GalleryPreviewLoader {}

impl Asset for GalleryPreviewLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        load_scaled_aspect(source, GALLERY_PREVIEW_DECODE_MAX_PX, cx)
    }
}

pub enum MessageImageLoader {}

impl Asset for MessageImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let _permit = acquire_image_pipeline_permit().await?;
            let bytes = match source.clone() {
                Resource::Path(uri) => {
                    if !message_path_maybe_animated(uri.as_ref())
                        && let Some(decoded) =
                            decode_scaled_dynamic_path(uri.as_ref(), MESSAGE_STATIC_MAX_PX)
                    {
                        return Ok(Arc::new(RenderImage::new(vec![downscaled_static_frame(
                            decoded,
                            MESSAGE_STATIC_MAX_PX,
                        )])));
                    }
                    std::fs::read(uri.as_ref())?
                }
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading image from {uri:?}"))?;
                    let body = read_body_limited(&mut response, MESSAGE_FETCH_MAX_BYTES).await?;
                    if !response.status().is_success() {
                        let mut body = String::from_utf8_lossy(&body).into_owned();
                        let first_line = body.lines().next().unwrap_or("").trim_end();
                        body.truncate(first_line.len());
                        return Err(ImageCacheError::BadStatus {
                            uri,
                            status: response.status(),
                            body,
                        });
                    }
                    body
                }
                Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                    Some(data) => data.to_vec(),
                    None => {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {path}").into(),
                        ));
                    }
                },
            };

            if image::guess_format(&bytes).is_ok() {
                decode_message_image(
                    &bytes,
                    MESSAGE_ANIMATION_MAX_PX,
                    MESSAGE_STATIC_MAX_PX,
                    MESSAGE_ENTRY_MAX_BYTES,
                )
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
    }
}

/// Bounded loader for the app-wide fallback cache (`LoaderKind::Full`).
/// Replaces GPUI's stock `ImageAssetLoader`, which decodes at full resolution
/// and keeps every animation frame: statics are capped at
/// [`SHARED_STATIC_MAX_PX`], animations at [`SHARED_ANIMATION_MAX_PX`] with an
/// in-decode byte budget of [`SHARED_ENTRY_MAX_BYTES`].
pub enum SharedImageLoader {}

impl Asset for SharedImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let _permit = acquire_image_pipeline_permit().await?;
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading image from {uri:?}"))?;
                    let body = read_body_limited(&mut response, MESSAGE_FETCH_MAX_BYTES).await?;
                    if !response.status().is_success() {
                        let mut body = String::from_utf8_lossy(&body).into_owned();
                        let first_line = body.lines().next().unwrap_or("").trim_end();
                        body.truncate(first_line.len());
                        return Err(ImageCacheError::BadStatus {
                            uri,
                            status: response.status(),
                            body,
                        });
                    }
                    body
                }
                Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                    Some(data) => data.to_vec(),
                    None => {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {path}").into(),
                        ));
                    }
                },
            };

            if image::guess_format(&bytes).is_ok() {
                decode_message_image(
                    &bytes,
                    SHARED_ANIMATION_MAX_PX,
                    SHARED_STATIC_MAX_PX,
                    SHARED_ENTRY_MAX_BYTES,
                )
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
    }
}

/// An [`Asset`] loader for the image viewer. Still images keep full resolution
/// (single frame), while animated GIF/WebP keep every frame but downscaled to
/// [`VIEWER_ANIMATION_MAX_PX`], so they animate in the viewer (matching the old
/// Electron/browser behaviour) without a full-resolution animation expanding to
/// `width * height * 4 * frames` bytes.
pub enum ViewerImageLoader {}

impl Asset for ViewerImageLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let _permit = acquire_image_pipeline_permit().await?;
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading image from {uri:?}"))?;
                    let body = read_body_limited(&mut response, VIEWER_FETCH_MAX_BYTES).await?;
                    if !response.status().is_success() {
                        let mut body = String::from_utf8_lossy(&body).into_owned();
                        let first_line = body.lines().next().unwrap_or("").trim_end();
                        body.truncate(first_line.len());
                        return Err(ImageCacheError::BadStatus {
                            uri,
                            status: response.status(),
                            body,
                        });
                    }
                    body
                }
                Resource::Embedded(path) => match asset_source.load(&path).ok().flatten() {
                    Some(data) => data.to_vec(),
                    None => {
                        return Err(ImageCacheError::Asset(
                            format!("Embedded resource not found: {path}").into(),
                        ));
                    }
                },
            };

            if image::guess_format(&bytes).is_ok() {
                decode_message_image(
                    &bytes,
                    VIEWER_ANIMATION_MAX_PX,
                    VIEWER_STATIC_MAX_PX,
                    VIEWER_IMAGE_ENTRY_MAX_BYTES,
                )
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_cache_budgets_bound_the_visible_working_set() {
        assert_eq!(IMAGE_PIPELINE_CONCURRENCY, 3);
        assert_eq!(MESSAGE_FETCH_MAX_BYTES, 32 * 1024 * 1024);
        assert_eq!(VIEWER_FETCH_MAX_BYTES, 32 * 1024 * 1024);
        assert_eq!(MESSAGE_IMAGE_CACHE_BYTES, 32 * 1024 * 1024);
        assert_eq!(VIEWER_IMAGE_CACHE_BYTES, 32 * 1024 * 1024);
        assert_eq!(GALLERY_IMAGE_CACHE_BYTES, 12 * 1024 * 1024);
        assert_eq!(PREVIEW_IMAGE_CACHE_CAPACITY, 64);
        assert_eq!(PREVIEW_IMAGE_CACHE_BYTES, 32 * 1024 * 1024);
        assert_eq!(SHARED_IMAGE_CACHE_BYTES, 24 * 1024 * 1024);
        assert_eq!(OGP_SHARED_CACHE_BYTES, 8 * 1024 * 1024);
        assert_eq!(IDLE_TRIM_INTERVAL, Duration::from_secs(30));
        assert_eq!(IDLE_TRIM_TTL, Duration::from_secs(60));
    }

    #[test]
    fn touched_this_epoch_is_never_stale() {
        assert!(!entry_is_stale(
            7,
            7,
            GRACE_PERIOD + Duration::from_secs(5),
            GRACE_PERIOD
        ));
    }

    #[test]
    fn untouched_within_grace_window_is_kept() {
        assert!(!entry_is_stale(6, 7, GRACE_PERIOD / 2, GRACE_PERIOD));
    }

    #[test]
    fn untouched_past_grace_window_is_evicted() {
        assert!(entry_is_stale(
            6,
            7,
            GRACE_PERIOD + Duration::from_millis(1),
            GRACE_PERIOD
        ));
    }

    #[test]
    fn idle_trim_never_evicts_the_current_visible_epoch() {
        assert!(!entry_is_idle(
            7,
            7,
            IDLE_TRIM_TTL + Duration::from_secs(1),
            IDLE_TRIM_TTL
        ));
        assert!(entry_is_idle(
            6,
            7,
            IDLE_TRIM_TTL + Duration::from_secs(1),
            IDLE_TRIM_TTL
        ));
    }

    #[test]
    fn downscale_keeps_images_within_cap_unchanged() {
        assert_eq!(downscale_dimensions(300, 200, 400), (300, 200));
        assert_eq!(downscale_dimensions(400, 400, 400), (400, 400));
    }

    #[test]
    fn downscale_shrinks_oversized_preserving_aspect() {
        assert_eq!(downscale_dimensions(800, 400, 400), (400, 200));
        assert_eq!(downscale_dimensions(498, 498, 400), (400, 400));
    }

    #[test]
    fn downscale_handles_zero_dimension() {
        assert_eq!(downscale_dimensions(0, 0, 400), (1, 1));
    }

    #[test]
    fn static_frame_downscales_oversized_within_cap() {
        let source = image::DynamicImage::new_rgba8(800, 400);
        let frame = downscaled_static_frame(source, 400);
        let buffer = frame.buffer();
        assert_eq!((buffer.width(), buffer.height()), (400, 200));
    }

    #[test]
    fn static_frame_keeps_small_image_full_size() {
        let source = image::DynamicImage::new_rgba8(320, 240);
        let frame = downscaled_static_frame(source, 1600);
        let buffer = frame.buffer();
        assert_eq!((buffer.width(), buffer.height()), (320, 240));
    }

    #[test]
    fn message_static_cap_bounds_decoded_bytes() {
        let source = image::DynamicImage::new_rgba8(5120, 4096);
        let frame = downscaled_static_frame(source, MESSAGE_STATIC_MAX_PX);
        let buffer = frame.buffer();
        assert_eq!(buffer.width().max(buffer.height()), MESSAGE_STATIC_MAX_PX);
        let image = RenderImage::new(vec![frame]);
        assert!(image_bytes(&image) < MESSAGE_ENTRY_MAX_BYTES);
    }

    #[test]
    fn message_static_cap_covers_two_x_inline_display() {
        const MAX_INLINE_LOGICAL_PX: u32 = 480;
        const _: () = assert!(MESSAGE_STATIC_MAX_PX >= MAX_INLINE_LOGICAL_PX * 2);
    }
}
