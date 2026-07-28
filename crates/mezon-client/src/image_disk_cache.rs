use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use foyer::{
    BlockEngineConfig, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder,
    PsyncIoEngineConfig,
};
use futures::AsyncReadExt as _;
use futures::future::BoxFuture;
use http_client::{AsyncBody, HttpClient, Method, Request, Response, Url, http};
use reqwest_client::ReqwestClient;

use crate::transport_runtime::handle;

const MEMORY_BUDGET_BYTES: usize = 4 * 1024 * 1024;
const DISK_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSFER_BYTES: u64 = 32 * 1024 * 1024;
const STATS_LOG_INTERVAL: u64 = 256;

type ImageHybridCache = HybridCache<u64, Vec<u8>>;

pub struct DiskImageCacheClient {
    inner: ReqwestClient,
    dir: Option<PathBuf>,
    media_origins: Arc<[String]>,
    upload_origin: String,
    read_origin: String,
    cache: Arc<tokio::sync::OnceCell<Option<ImageHybridCache>>>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub struct DiskImageCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub disk_write_bytes: u64,
    pub disk_read_bytes: u64,
}

impl DiskImageCacheClient {
    pub fn new(
        inner: ReqwestClient,
        media_origins: Vec<String>,
        upload_origin: String,
        read_origin: String,
    ) -> Self {
        let dir = dirs::cache_dir().map(|base| base.join("mezon").join("images"));
        Self {
            inner,
            dir,
            media_origins: media_origins.into(),
            upload_origin,
            read_origin,
            cache: Arc::new(tokio::sync::OnceCell::new()),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn stats(&self) -> DiskImageCacheStats {
        let (disk_write_bytes, disk_read_bytes) = self
            .cache
            .get()
            .and_then(|cache| cache.as_ref())
            .map(|cache| {
                let stats = cache.statistics();
                (
                    stats.disk_write_bytes() as u64,
                    stats.disk_read_bytes() as u64,
                )
            })
            .unwrap_or((0, 0));
        DiskImageCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            disk_write_bytes,
            disk_read_bytes,
        }
    }
}

fn image_path(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

fn has_image_extension(path: &str) -> bool {
    const EXTS: [&[u8]; 7] = [
        b".png", b".jpg", b".jpeg", b".webp", b".gif", b".bmp", b".avif",
    ];
    let bytes = path.as_bytes();
    EXTS.iter().any(|ext| {
        bytes.len() >= ext.len() && bytes[bytes.len() - ext.len()..].eq_ignore_ascii_case(ext)
    })
}

fn url_has_origin(url: &str, origin: &str) -> bool {
    let origin = origin.trim_end_matches('/');
    !origin.is_empty()
        && url
            .strip_prefix(origin)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn rewrite_url_origin(url: &str, source_origin: &str, target_origin: &str) -> Option<String> {
    let source_origin = source_origin.trim_end_matches('/');
    let target_origin = target_origin.trim_end_matches('/');
    if source_origin.is_empty() || target_origin.is_empty() {
        return None;
    }
    let suffix = url.strip_prefix(source_origin)?;
    if !suffix.is_empty() && !suffix.starts_with('/') {
        return None;
    }
    Some(format!("{target_origin}{suffix}"))
}

fn is_cacheable_image_url(url: &str, media_origins: &[String]) -> bool {
    let path = image_path(url);
    if (path.contains("/rs:fill:") || path.contains("/rs:fit:")) && path.ends_with("@webp") {
        return true;
    }
    media_origins
        .iter()
        .any(|origin| url_has_origin(url, origin))
        && has_image_extension(path)
}

fn stable_key(url: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in image_path(url).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn is_cache_hit(fetched_ok: bool, fetcher_ran: bool) -> bool {
    fetched_ok && !fetcher_ran
}

fn ok_response(bytes: Vec<u8>) -> Response<AsyncBody> {
    Response::new(AsyncBody::from(bytes))
}

async fn build_cache(dir: PathBuf) -> Option<ImageHybridCache> {
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        tracing::error!("image disk cache: create dir failed: {err}");
        return None;
    }
    let device = match FsDeviceBuilder::new(&dir)
        .with_capacity(DISK_BUDGET_BYTES)
        .build()
    {
        Ok(device) => device,
        Err(err) => {
            tracing::error!("image disk cache: device build failed: {err}");
            return None;
        }
    };
    match HybridCacheBuilder::new()
        .with_name("mezon-image-disk")
        .memory(MEMORY_BUDGET_BYTES)
        .with_weighter(|_key: &u64, value: &Vec<u8>| value.len())
        .storage()
        .with_io_engine_config(PsyncIoEngineConfig::new())
        .with_engine_config(BlockEngineConfig::new(device))
        .build()
        .await
    {
        Ok(cache) => {
            tracing::info!("image disk cache ready at {}", dir.display());
            Some(cache)
        }
        Err(err) => {
            tracing::error!("image disk cache: build failed: {err}");
            None
        }
    }
}

enum FetchResult {
    Bytes(Vec<u8>),
    Passthrough(Response<AsyncBody>),
}

async fn read_limited(response: &mut Response<AsyncBody>) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    response
        .body_mut()
        .take(MAX_TRANSFER_BYTES + 1)
        .read_to_end(&mut body)
        .await?;
    anyhow::ensure!(
        body.len() as u64 <= MAX_TRANSFER_BYTES,
        "image body exceeds the {MAX_TRANSFER_BYTES} byte transfer limit"
    );
    Ok(body)
}

async fn buffered_response(
    network: BoxFuture<'static, anyhow::Result<Response<AsyncBody>>>,
) -> anyhow::Result<Response<AsyncBody>> {
    let mut response = network.await?;
    let body = read_limited(&mut response).await?;
    let (parts, _) = response.into_parts();
    Ok(Response::from_parts(parts, AsyncBody::from(body)))
}

impl HttpClient for DiskImageCacheClient {
    fn user_agent(&self) -> Option<&http::HeaderValue> {
        self.inner.user_agent()
    }

    fn proxy(&self) -> Option<&Url> {
        self.inner.proxy()
    }

    fn send(
        &self,
        mut req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        if *req.method() == Method::GET {
            let url = req.uri().to_string();
            let read_url = rewrite_url_origin(&url, &self.upload_origin, &self.read_origin)
                .or_else(|| {
                    rewrite_url_origin(&url, "http://cdn.mezon.ai", "https://cdn.mezon.ai")
                });
            if let Some(read_url) = read_url
                && let Ok(read_uri) = read_url.parse()
            {
                *req.uri_mut() = read_uri;
            }
        }
        let uri = req.uri().to_string();
        if *req.method() != Method::GET || !is_cacheable_image_url(&uri, &self.media_origins) {
            return self.inner.send(req);
        }
        let key = stable_key(&uri);
        let network = self.inner.send(req);
        let dir = self.dir.clone();
        let cache_cell = self.cache.clone();
        let hits = self.hits.clone();
        let misses = self.misses.clone();
        Box::pin(async move {
            let Some(dir) = dir else {
                return network.await;
            };
            let result = handle()
                .spawn(async move {
                    let cache = match cache_cell.get().cloned() {
                        Some(Some(cache)) => cache,
                        Some(None) => {
                            return buffered_response(network)
                                .await
                                .map(FetchResult::Passthrough);
                        }
                        None => {
                            let cache_cell = cache_cell.clone();
                            tokio::spawn(async move {
                                let _ = cache_cell.get_or_init(|| build_cache(dir)).await;
                            });
                            return buffered_response(network)
                                .await
                                .map(FetchResult::Passthrough);
                        }
                    };
                    let passthrough: Arc<Mutex<Option<Response<AsyncBody>>>> =
                        Arc::new(Mutex::new(None));
                    let slot = passthrough.clone();
                    let fetcher_ran = Arc::new(AtomicBool::new(false));
                    let ran_flag = fetcher_ran.clone();
                    let fetched = cache
                        .get_or_fetch(&key, move || async move {
                            ran_flag.store(true, Ordering::Relaxed);
                            let mut response = network.await?;
                            if !response.status().is_success() {
                                let body = read_limited(&mut response).await?;
                                let (parts, _) = response.into_parts();
                                if let Ok(mut held) = slot.lock() {
                                    *held =
                                        Some(Response::from_parts(parts, AsyncBody::from(body)));
                                }
                                anyhow::bail!("image fetch returned non-success status");
                            }
                            let body = read_limited(&mut response).await?;
                            if body.is_empty() || body.len() > MAX_ENTRY_BYTES {
                                if let Ok(mut held) = slot.lock() {
                                    *held = Some(ok_response(body));
                                }
                                anyhow::bail!("image body not admissible to disk cache");
                            }
                            Ok::<Vec<u8>, anyhow::Error>(body)
                        })
                        .await;
                    let downloaded = passthrough.lock().ok().and_then(|mut held| held.take());
                    let hit = is_cache_hit(fetched.is_ok(), fetcher_ran.load(Ordering::Relaxed));
                    let counted = if hit {
                        hits.fetch_add(1, Ordering::Relaxed);
                        hits.load(Ordering::Relaxed) + misses.load(Ordering::Relaxed)
                    } else {
                        misses.fetch_add(1, Ordering::Relaxed);
                        hits.load(Ordering::Relaxed) + misses.load(Ordering::Relaxed)
                    };
                    if counted.is_multiple_of(STATS_LOG_INTERVAL) {
                        tracing::debug!(
                            hits = hits.load(Ordering::Relaxed),
                            misses = misses.load(Ordering::Relaxed),
                            "image disk cache"
                        );
                    }
                    match fetched {
                        Ok(entry) => Ok(FetchResult::Bytes(entry.value().clone())),
                        Err(err) => match downloaded {
                            Some(response) => Ok(FetchResult::Passthrough(response)),
                            None => Err(anyhow::anyhow!("image disk cache fetch failed: {err}")),
                        },
                    }
                })
                .await
                .map_err(|err| anyhow::anyhow!("image cache task failed: {err}"))?;
            match result? {
                FetchResult::Bytes(bytes) => Ok(ok_response(bytes)),
                FetchResult::Passthrough(response) => Ok(response),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVATAR: &str = "https://dev-imgproxy.nccsoft.vn/k/rs:fill:100:100:1/mb:2097152/plain/https://cdn.komu.vn/a.png@webp";
    const PROFILE: &str = "https://dev-imgproxy.nccsoft.vn/k/rs:fit:300:300:1/mb:2097152/plain/https://cdn.komu.vn/a.png@webp";
    const MESSAGE_RENDITION: &str = "https://imgproxy.komu.vn/k/rs:fill:800:600:1/mb:2097152/plain/https://cdn.komu.vn/x.jpg@webp";
    const RAW_CDN: &str = "https://cdn.komu.vn/1700000000_0photo.png";
    const RAW_LEGACY_CDN: &str = "https://cdn.mezon.ai/stickers/hellomezon.gif";
    const RAW_LEGACY_HTTP_CDN: &str = "http://cdn.mezon.ai/landing-page-mezon/legacy.gif";
    const RAW_UPLOAD: &str = "https://cdn-api.mezon.ai/1700000000_0photo.png";
    const RAW_PROFILE: &str = "https://profile.mezon.ai/1700000000_0avatar.jpg";
    const VIDEO: &str = "https://cdn.komu.vn/1700000000_0clip.mp4";
    const PDF: &str = "https://cdn.komu.vn/1700000000_0doc.pdf";
    const API: &str = "https://api.mezon.ai/v2/account";
    const TENOR: &str = "https://media.tenor.com/abc.gif";

    fn media_origins() -> Vec<String> {
        vec![
            "https://cdn.komu.vn".into(),
            "https://profile.mezon.ai".into(),
            "https://cdn.mezon.ai".into(),
            "http://cdn.mezon.ai".into(),
        ]
    }

    #[test]
    fn disk_cache_keeps_only_a_small_memory_front() {
        assert_eq!(MEMORY_BUDGET_BYTES, 4 * 1024 * 1024);
        assert_eq!(DISK_BUDGET_BYTES, 256 * 1024 * 1024);
        assert_eq!(MAX_TRANSFER_BYTES, 32 * 1024 * 1024);
    }

    #[test]
    fn predicate_matches_renditions_avatars_profile_and_raw_cdn_images() {
        let origins = media_origins();
        assert!(is_cacheable_image_url(AVATAR, &origins));
        assert!(is_cacheable_image_url(PROFILE, &origins));
        assert!(is_cacheable_image_url(MESSAGE_RENDITION, &origins));
        assert!(is_cacheable_image_url(RAW_CDN, &origins));
        assert!(is_cacheable_image_url(RAW_LEGACY_CDN, &origins));
        assert!(is_cacheable_image_url(RAW_LEGACY_HTTP_CDN, &origins));
        assert!(is_cacheable_image_url(RAW_PROFILE, &origins));
        assert!(!is_cacheable_image_url(RAW_UPLOAD, &origins));
    }

    #[test]
    fn predicate_excludes_video_pdf_api_and_third_party() {
        let origins = media_origins();
        assert!(!is_cacheable_image_url(VIDEO, &origins));
        assert!(!is_cacheable_image_url(PDF, &origins));
        assert!(!is_cacheable_image_url(API, &origins));
        assert!(!is_cacheable_image_url(TENOR, &origins));
    }

    #[test]
    fn predicate_excludes_lookalike_hosts() {
        let origins = media_origins();
        assert!(!is_cacheable_image_url(
            "https://cdn.komu.vn.attacker.com/x.png",
            &origins
        ));
        assert!(!is_cacheable_image_url(
            "https://profile.mezon.ai.attacker.com/x.jpg",
            &origins
        ));
        assert!(!is_cacheable_image_url(
            "https://cdn-api.mezon.ai.attacker.com/x.png",
            &origins
        ));
    }

    #[test]
    fn upload_get_url_is_rewritten_to_the_read_cdn() {
        assert_eq!(
            rewrite_url_origin(
                "https://cdn-api.mezon.ai/a/photo.png?version=1",
                "https://cdn-api.mezon.ai",
                "https://cdn.komu.vn",
            ),
            Some("https://cdn.komu.vn/a/photo.png?version=1".into())
        );
        assert_eq!(
            rewrite_url_origin(
                "https://cdn-api.mezon.ai.attacker.com/a.png",
                "https://cdn-api.mezon.ai",
                "https://cdn.komu.vn",
            ),
            None
        );
    }

    #[test]
    fn legacy_http_cdn_is_upgraded_to_https() {
        assert_eq!(
            rewrite_url_origin(
                "http://cdn.mezon.ai/landing-page-mezon/legacy.gif",
                "http://cdn.mezon.ai",
                "https://cdn.mezon.ai",
            ),
            Some("https://cdn.mezon.ai/landing-page-mezon/legacy.gif".into())
        );
        assert_eq!(
            rewrite_url_origin(
                "http://cdn.mezon.ai.attacker.com/legacy.gif",
                "http://cdn.mezon.ai",
                "https://cdn.mezon.ai",
            ),
            None
        );
    }

    #[test]
    fn stable_key_ignores_query_and_fragment() {
        assert_eq!(stable_key(RAW_CDN), stable_key(RAW_CDN));
        let presigned = format!("{RAW_CDN}?X-Amz-Expires=3600&X-Amz-Signature=deadbeef");
        assert_eq!(stable_key(RAW_CDN), stable_key(&presigned));
        let fragment = format!("{RAW_CDN}#anchor");
        assert_eq!(stable_key(RAW_CDN), stable_key(&fragment));
    }

    #[test]
    fn stable_key_distinguishes_distinct_sources_and_renditions() {
        assert_ne!(stable_key(RAW_CDN), stable_key(RAW_PROFILE));
        let bigger = MESSAGE_RENDITION.replace("800:600", "1600:1200");
        assert_ne!(stable_key(MESSAGE_RENDITION), stable_key(&bigger));
    }

    #[test]
    fn fresh_download_is_classified_as_miss() {
        assert!(!is_cache_hit(true, true));
        assert!(!is_cache_hit(false, true));
        assert!(!is_cache_hit(false, false));
    }

    #[test]
    fn served_without_running_fetcher_is_classified_as_hit() {
        assert!(is_cache_hit(true, false));
    }

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    async fn test_cache(dir: &std::path::Path) -> ImageHybridCache {
        tokio::fs::create_dir_all(dir).await.unwrap();
        let device = FsDeviceBuilder::new(dir)
            .with_capacity(4 * 1024 * 1024)
            .build()
            .unwrap();
        HybridCacheBuilder::new()
            .memory(1024 * 1024)
            .with_weighter(|_key: &u64, value: &Vec<u8>| value.len())
            .storage()
            .with_io_engine_config(PsyncIoEngineConfig::new())
            .with_engine_config(BlockEngineConfig::new(device).with_block_size(16 * 1024))
            .build()
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fresh_fetch_counts_as_miss_then_identical_request_as_hit() {
        let dir = std::env::temp_dir().join(format!(
            "mezon-img-cache-test-{}-{}",
            std::process::id(),
            TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = test_cache(&dir).await;
        let key = stable_key(RAW_CDN);

        let first_ran = Arc::new(AtomicBool::new(false));
        let flag = first_ran.clone();
        let first = cache
            .get_or_fetch(&key, move || async move {
                flag.store(true, Ordering::Relaxed);
                Ok::<Vec<u8>, anyhow::Error>(vec![1, 2, 3, 4])
            })
            .await
            .unwrap();
        assert_eq!(first.value(), &vec![1, 2, 3, 4]);
        assert!(first_ran.load(Ordering::Relaxed));
        assert!(!is_cache_hit(true, first_ran.load(Ordering::Relaxed)));

        let second_ran = Arc::new(AtomicBool::new(false));
        let flag = second_ran.clone();
        let second = cache
            .get_or_fetch(&key, move || async move {
                flag.store(true, Ordering::Relaxed);
                Ok::<Vec<u8>, anyhow::Error>(vec![9, 9, 9, 9])
            })
            .await
            .unwrap();
        assert_eq!(second.value(), &vec![1, 2, 3, 4]);
        assert!(!second_ran.load(Ordering::Relaxed));
        assert!(is_cache_hit(true, second_ran.load(Ordering::Relaxed)));

        cache.close().await.ok();
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
