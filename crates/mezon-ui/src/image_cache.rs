use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::future::{AbortHandle, Abortable};
use futures::{AsyncReadExt as _, FutureExt};
use gpui::{
    App, AppContext, Asset, AssetLogger, Context, Entity, Global, ImageAssetLoader, ImageCache,
    ImageCacheError, ImageCacheItem, RenderImage, Resource, WeakEntity, Window, hash,
};
use indexmap::IndexMap;

#[derive(Default)]
struct PendingAtlasDrops(Vec<Arc<RenderImage>>);
impl Global for PendingAtlasDrops {}

fn queue_atlas_drop(cx: &mut App, image: Arc<RenderImage>) {
    cx.default_global::<PendingAtlasDrops>().0.push(image);
}

pub fn flush_atlas_drops(window: &mut Window, cx: &mut App) {
    let pending = std::mem::take(&mut cx.default_global::<PendingAtlasDrops>().0);
    for image in pending {
        cx.drop_image(image, Some(window));
    }
}

const SHARED_AVATAR_CACHE_CAPACITY: usize = 512;
const SHARED_AVATAR_CACHE_BYTES: u64 = 40 * 1024 * 1024;

struct SharedAvatarCache(Entity<LruImageCache>);
impl Global for SharedAvatarCache {}

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

#[derive(Default)]
struct CacheRegistry(Vec<WeakEntity<LruImageCache>>);
impl Global for CacheRegistry {}

pub fn log_cache_budgets(cx: &mut App) {
    let entries = std::mem::take(&mut cx.default_global::<CacheRegistry>().0);
    let mut alive = Vec::with_capacity(entries.len());
    let mut total_mb = 0u64;
    for weak in entries {
        let Some(entity) = weak.upgrade() else {
            continue;
        };
        let cache = entity.read(cx);
        let stats = cache.stats();
        total_mb += stats.current_bytes / (1024 * 1024);
        tracing::info!(
            label = cache.label,
            instance = cache.instance,
            used_mb = stats.current_bytes / (1024 * 1024),
            budget_mb = cache.max_bytes / (1024 * 1024),
            items = stats.items,
            evictions = stats.evictions,
            "img cache budget"
        );
        alive.push(weak);
    }
    tracing::info!(
        total_used_mb = total_mb,
        live_caches = alive.len(),
        "img cache budgets total"
    );
    cx.default_global::<CacheRegistry>().0 = alive;
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

#[cfg(target_os = "macos")]
static MEMORY_RELIEF_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub fn release_freed_memory_to_os(cx: &mut App) {
    #[cfg(target_os = "macos")]
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
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

pub const MESSAGE_IMAGE_CACHE_CAPACITY: usize = 48;
pub const MESSAGE_IMAGE_CACHE_BYTES: u64 = 48 * 1024 * 1024;
pub const AVATAR_IMAGE_CACHE_CAPACITY: usize = 256;
pub const AVATAR_IMAGE_CACHE_BYTES: u64 = 16 * 1024 * 1024;

pub const VIEWER_IMAGE_CACHE_CAPACITY: usize = 24;
pub const VIEWER_IMAGE_CACHE_BYTES: u64 = 96 * 1024 * 1024;
pub const VIEWER_IMAGE_ENTRY_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// App-wide fallback cache attached at the root, so any `img`/avatar that does
/// not declare its own cache uses this bounded LRU instead of GPUI's unbounded
/// global asset cache (which never evicts and leaks RAM for every URL seen).
pub const SHARED_IMAGE_CACHE_CAPACITY: usize = 384;
pub const SHARED_IMAGE_CACHE_BYTES: u64 = 64 * 1024 * 1024;
pub const GALLERY_IMAGE_CACHE_CAPACITY: usize = 48;
pub const GALLERY_IMAGE_CACHE_BYTES: u64 = 48 * 1024 * 1024;

/// Per-image decoded-size caps. A compressed file is tiny on the wire but is
/// stored uncompressed in RAM as `width * height * 4` bytes *per frame*. An
/// animated GIF/WebP therefore explodes: a ~400 KB animated avatar can decode
/// to hundreds of MB once every frame is expanded. When the resizing image
/// proxy is unavailable (dev, or a prod outage) we fall back to the raw,
/// full-resolution file, so we guard against a single pathological image
/// blowing up RAM by refusing to retain anything decoded larger than this and
/// negatively caching it (shown as the initials fallback instead).
pub const AVATAR_ENTRY_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const MESSAGE_ENTRY_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const SHARED_ENTRY_MAX_BYTES: u64 = 16 * 1024 * 1024;

const GRACE_PERIOD: Duration = Duration::from_secs(2);
const STATS_LOG_INTERVAL: u64 = 600;
const MESSAGE_ANIMATION_MAX_PX: u32 = 400;
const MESSAGE_STATIC_MAX_PX: u32 = 1024;
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
}

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

/// An LRU image cache bounded by both an item count and a decoded-byte budget.
///
/// The byte budget is what actually keeps RAM in check: large attachments are
/// evicted as soon as the total decoded size exceeds `max_bytes`, instead of
/// lingering until the (much larger) item count or a channel switch clears them.
static CACHE_INSTANCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Which decoder a cache uses to turn a resource into a `RenderImage`.
#[derive(Clone, Copy)]
enum LoaderKind {
    /// GPUI's stock loader: decodes the image at full resolution and keeps every
    /// frame of animated GIF/WebP. Used for message attachments that must render
    /// full-size and animated.
    Full,
    /// Decodes only the first frame and downscales to avatar size, so even an
    /// animated full-resolution source costs ~100 KB of RAM. Used for avatars.
    AvatarThumbnail,
    GalleryThumbnail,
    Message,
    /// Decodes only the first frame at full resolution. The image viewer paints
    /// a single static frame (frame 0), so keeping every frame of an animated
    /// GIF/WebP is wasted RAM: a large animation decodes to `w * h * 4 * frames`
    /// bytes yet only the first frame is ever shown.
    ViewerFirstFrame,
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
            LoaderKind::ViewerFirstFrame,
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

        let weak = cx.weak_entity();
        cx.default_global::<CacheRegistry>().0.push(weak);

        let instance = CACHE_INSTANCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let epoch = self.epoch;
        let stale: Vec<u64> = self
            .cache
            .iter()
            .filter(|(_, entry)| {
                entry_is_stale(
                    entry.touched_epoch,
                    epoch,
                    entry.last_used.elapsed(),
                    GRACE_PERIOD,
                )
            })
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(mut entry) = self.cache.shift_remove(&key) {
                entry.abort.abort();
                self.metrics.evictions.fetch_add(1, Ordering::Relaxed);
                if let Some(bytes) = entry.bytes {
                    self.total_bytes = self.total_bytes.saturating_sub(bytes);
                }
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, Some(window));
                }
            }
        }
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

    pub fn shrink_to(&mut self, max_bytes: u64, window: &mut Window, cx: &mut App) {
        while self.total_bytes > max_bytes {
            let Some((_, mut evicted)) = self.cache.shift_remove_index(0) else {
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
    /// byte budgets are satisfied. The most-recently-used entry (back of the
    /// map) is never evicted, so the image requested this frame stays resident.
    fn evict_to_budget(&mut self, window: &mut Window, cx: &mut App) {
        while self.cache.len() > self.max_items
            || (self.total_bytes > self.max_bytes && self.cache.len() > 1)
        {
            let Some((_, mut evicted)) = self.cache.shift_remove_index(0) else {
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

        if let Some(entry) = self.cache.shift_remove(&hash) {
            self.cache.insert(hash, entry);
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
                let entry = self.cache.get_mut(&hash).expect("just re-inserted");
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
            LoaderKind::Full => AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx).boxed(),
            LoaderKind::AvatarThumbnail => {
                AssetLogger::<AvatarImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::GalleryThumbnail => {
                AssetLogger::<GalleryImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::Message => {
                AssetLogger::<MessageImageLoader>::load(resource.clone(), cx).boxed()
            }
            LoaderKind::ViewerFirstFrame => {
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
            },
        );
        self.evict_to_budget(window, cx);

        let entity = window.current_view();
        let notify_task = task.clone();
        window
            .spawn(cx, async move |cx| {
                let _ = Abortable::new(notify_task, abort_reg).await;
                cx.on_next_frame(move |_, cx| {
                    cx.notify(entity);
                });
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
const GALLERY_THUMB_DECODE_MAX_PX: u32 = 320;

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
pub enum AvatarImageLoader {}

impl Asset for AvatarImageLoader {
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
            let bytes = match source.clone() {
                Resource::Path(uri) => {
                    if let Some(decoded) =
                        decode_scaled_dynamic_path(uri.as_ref(), AVATAR_DECODE_MAX_PX)
                    {
                        return Ok(avatar_render_image(decoded, AVATAR_DECODE_MAX_PX));
                    }
                    std::fs::read(uri.as_ref())?
                }
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading avatar from {uri:?}"))?;
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
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
                // `load_from_memory` decodes a single frame even for animated
                // GIF/WebP, so this never expands the whole animation.
                let decoded = match decode_scaled_dynamic(&bytes, AVATAR_DECODE_MAX_PX) {
                    Some(image) => image,
                    None => image::load_from_memory(&bytes)?,
                };
                Ok(avatar_render_image(decoded, AVATAR_DECODE_MAX_PX))
            } else {
                svg_renderer
                    .render_single_frame(&bytes, 1.0)
                    .map_err(Into::into)
            }
        }
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
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading gallery image from {uri:?}"))?;
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
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

fn downscaled_animation_frames<I>(
    frames: I,
    max_px: u32,
) -> Result<Vec<image::Frame>, ImageCacheError>
where
    I: Iterator<Item = image::ImageResult<image::Frame>>,
{
    let mut out: Vec<image::Frame> = Vec::new();
    let mut target: Option<(u32, u32)> = None;
    for frame in frames {
        let frame = frame?;
        let delay = frame.delay();
        let buffer = frame.into_buffer();
        let (tw, th) = *target
            .get_or_insert_with(|| downscale_dimensions(buffer.width(), buffer.height(), max_px));
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
        return Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
            "animation decoded to zero frames"
        ))));
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

fn decode_message_image(
    bytes: &[u8],
    animation_max_px: u32,
    static_max_px: u32,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    use image::AnimationDecoder as _;
    let format = image::guess_format(bytes)?;
    let frames = match format {
        image::ImageFormat::Gif => {
            let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))?;
            downscaled_animation_frames(decoder.into_frames(), animation_max_px)?
        }
        image::ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))?;
            if decoder.has_animation() {
                downscaled_animation_frames(decoder.into_frames(), animation_max_px)?
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
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
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
                decode_message_image(&bytes, MESSAGE_ANIMATION_MAX_PX, MESSAGE_STATIC_MAX_PX)
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
            let bytes = match source.clone() {
                Resource::Path(uri) => std::fs::read(uri.as_ref())?,
                Resource::Uri(uri) => {
                    use anyhow::Context as _;

                    let mut response = client
                        .get(uri.as_ref(), ().into(), true)
                        .await
                        .with_context(|| format!("loading image from {uri:?}"))?;
                    let mut body = Vec::new();
                    response.body_mut().read_to_end(&mut body).await?;
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
                decode_message_image(&bytes, VIEWER_ANIMATION_MAX_PX, VIEWER_STATIC_MAX_PX)
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
        assert!(MESSAGE_STATIC_MAX_PX >= MAX_INLINE_LOGICAL_PX * 2);
    }
}
