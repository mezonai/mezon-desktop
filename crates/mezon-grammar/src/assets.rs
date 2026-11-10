use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_CDN_BASE_URL: &str = "https://cdn.komu.vn/mezonai/grammar";

pub const ACCENT_VOCAB_FILE: &str = "accent_vocab.json";
pub const COLLOCATIONS_FILE: &str = "collocations.json";
pub const CONFIG_FILE: &str = "config.json";
pub const VOCAB_FILE: &str = "vocab.json";
pub const ACCENT_MODEL_FILE: &str = "accent_model.onnx";
pub const MODEL_FILE: &str = "model.onnx";

pub const ALL_ASSETS: [&str; 6] = [
    ACCENT_VOCAB_FILE,
    COLLOCATIONS_FILE,
    CONFIG_FILE,
    VOCAB_FILE,
    ACCENT_MODEL_FILE,
    MODEL_FILE,
];

static PREFETCH_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static DOWNLOAD_ENABLED: AtomicBool = AtomicBool::new(true);

#[cfg(not(test))]
static DOWNLOAD_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enables or disables downloading grammar assets from the CDN.
pub fn set_download_enabled(enabled: bool) {
    DOWNLOAD_ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        PREFETCH_STARTED.store(false, Ordering::SeqCst);
    }
}

/// Returns true if downloading from the CDN is enabled by configuration.
pub fn is_download_enabled() -> bool {
    if std::env::var_os("MEZON_GRAMMAR_DOWNLOAD").is_some() {
        return true;
    }
    DOWNLOAD_ENABLED.load(Ordering::SeqCst)
}

/// Returns the directory used for caching grammar models and vocabularies on disk.
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MEZON_GRAMMAR_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    dirs::cache_dir()
        .map(|base| base.join("mezon").join("grammar"))
        .unwrap_or_else(|| std::env::temp_dir().join("mezon").join("grammar"))
}

/// Returns the base CDN URL for downloading grammar assets.
pub fn cdn_base_url() -> String {
    std::env::var("MEZON_GRAMMAR_CDN_URL")
        .unwrap_or_else(|_| DEFAULT_CDN_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Check if a local copy exists (e.g., during tests or development).
fn find_local_fallback(filename: &str) -> Option<PathBuf> {
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = PathBuf::from(manifest).join("assets").join(filename);
        if p.is_file() {
            return Some(p);
        }
    }
    let local = PathBuf::from("crates/mezon-grammar/assets").join(filename);
    if local.is_file() {
        return Some(local);
    }
    None
}

/// Run an operation inside a dedicated OS thread with its own single-threaded tokio runtime.
/// This prevents "Cannot start a runtime from within a runtime" errors if called from an async thread.
fn run_in_dedicated_thread<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("mezon-grammar-loader".to_string())
        .spawn(f)
        .map_err(|e| format!("Failed to spawn download thread: {e}"))?
        .join()
        .map_err(|_| "Download thread panicked".to_string())?
}

/// Downloads an asset from the CDN.
fn download_from_cdn(filename: &str) -> Result<Vec<u8>, String> {
    let url = format!("{}/{}", cdn_base_url(), filename);
    tracing::info!("Downloading grammar asset from CDN: {url}");

    run_in_dedicated_thread(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

        rt.block_on(async move {
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

            let response = client
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| format!("Failed to download {url}: {e}"))?;

            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read response bytes from {url}: {e}"))?;

            Ok(bytes.to_vec())
        })
    })
}

fn save_asset_to_disk(dir: &Path, filename: &str, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(filename);
    let tmp = dir.join(format!("{filename}.part.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &target)
}

/// Loads asset bytes from the local disk cache, or downloads it from the CDN if not yet cached.
pub fn load_asset_bytes(filename: &str) -> Option<Vec<u8>> {
    let dir = cache_dir();
    let cached_path = dir.join(filename);

    if cached_path.is_file() {
        if let Ok(bytes) = std::fs::read(&cached_path) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }

    if let Some(local) = find_local_fallback(filename) {
        if let Ok(bytes) = std::fs::read(&local) {
            if !bytes.is_empty() {
                let _ = save_asset_to_disk(&dir, filename, &bytes);
                return Some(bytes);
            }
        }
    }

    if !is_download_enabled() {
        tracing::debug!("Grammar asset download skipped: config is disabled for {filename}");
        return None;
    }

    match download_from_cdn(filename) {
        Ok(bytes) => {
            if let Err(err) = save_asset_to_disk(&dir, filename, &bytes) {
                tracing::warn!("Failed to cache grammar asset {filename}: {err}");
            }
            Some(bytes)
        }
        Err(err) => {
            tracing::warn!("Failed to fetch grammar asset {filename}: {err}");
            None
        }
    }
}

/// Loads asset text from disk cache, or downloads it from the CDN.
pub fn load_asset_str(filename: &str) -> Option<String> {
    let bytes = load_asset_bytes(filename)?;
    String::from_utf8(bytes).ok()
}

/// Starts prefetching all grammar assets in a background thread if enabled and not already started.
pub fn prefetch_all_assets() {
    if !is_download_enabled() {
        return;
    }
    if PREFETCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("mezon-grammar-prefetch".to_string())
        .spawn(|| {
            for filename in ALL_ASSETS {
                if !is_download_enabled() {
                    break;
                }
                let _ = load_asset_bytes(filename);
            }
        })
        .ok();
}

/// Ensures all assets are present in the disk cache, downloading missing ones synchronously.
pub fn ensure_all_assets_cached() -> bool {
    ALL_ASSETS
        .iter()
        .all(|filename| load_asset_bytes(filename).is_some())
}
