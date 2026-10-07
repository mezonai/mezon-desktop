use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};

use super::create_filter;
use crate::runtime;

const MODEL_FILE: &str = "mezon_ns_asym.onnx";
const MODEL_META_FILE: &str = "mezon_ns_asym.json";
const MODEL_URL: &str = "https://cdn.komu.vn/ns/mezon_ns_asym.onnx";
const MODEL_WAIT: Duration = Duration::from_secs(15);

struct ModelSlot {
    model: Option<Arc<[u8]>>,
    loading: bool,
    error: Option<String>,
}

struct ModelFile {
    model: Vec<u8>,
    etag: Option<String>,
}

static MODEL: Mutex<ModelSlot> = Mutex::new(ModelSlot {
    model: None,
    loading: false,
    error: None,
});
static MODEL_CHANGED: Condvar = Condvar::new();

pub(super) fn prefetch_model() {
    let mut slot = MODEL.lock();
    if !slot.loading {
        start_model_load(&mut slot);
    }
}

pub(super) fn model_bytes() -> Result<Arc<[u8]>, String> {
    let deadline = Instant::now() + MODEL_WAIT;
    let mut slot = MODEL.lock();
    if slot.model.is_none() && !slot.loading {
        start_model_load(&mut slot);
    }
    loop {
        if let Some(model) = &slot.model {
            return Ok(model.clone());
        }
        if !slot.loading {
            return Err(slot
                .error
                .clone()
                .unwrap_or_else(|| "Mezon-NS model unavailable".into()));
        }
        if Instant::now() >= deadline {
            return Err("Mezon-NS model is still downloading".into());
        }
        MODEL_CHANGED.wait_until(&mut slot, deadline);
    }
}

fn start_model_load(slot: &mut ModelSlot) {
    slot.loading = true;
    let spawned = std::thread::Builder::new()
        .name("mezon-ns-model".into())
        .spawn(|| {
            let loaded = load_model();
            let mut slot = MODEL.lock();
            slot.loading = false;
            match loaded {
                Ok(model) => {
                    slot.model = Some(model.into());
                    slot.error = None;
                }
                Err(error) => {
                    tracing::warn!("Mezon-NS model unavailable: {error}");
                    slot.error = Some(error);
                }
            }
            MODEL_CHANGED.notify_all();
        });
    if let Err(error) = spawned {
        slot.loading = false;
        slot.error = Some(error.to_string());
    }
}

fn load_model() -> Result<Vec<u8>, String> {
    let dir = dirs::cache_dir().map(|base| base.join("mezon").join("ns"));
    let cached = dir
        .as_deref()
        .and_then(read_cached_model)
        .filter(|file| create_filter(&file.model).is_ok());
    if let Some(cached) = &cached {
        // Make a valid cache usable before waiting on the CDN refresh.
        let mut slot = MODEL.lock();
        if slot.model.is_none() {
            slot.model = Some(cached.model.clone().into());
            MODEL_CHANGED.notify_all();
        }
    }
    let etag = cached.as_ref().and_then(|cached| cached.etag.clone());
    let fresh = runtime::runtime()
        .block_on(download_model(etag))
        .and_then(|downloaded| match downloaded {
            Some(file) => create_filter(&file.model).map(|_| Some(file)),
            None => Ok(None),
        });
    match (fresh, cached) {
        (Ok(Some(file)), _) => {
            tracing::info!(bytes = file.model.len(), "Mezon-NS model downloaded");
            if let Some(dir) = &dir
                && let Err(error) = save_model(dir, &file)
            {
                tracing::warn!("Mezon-NS model cache write failed: {error}");
            }
            Ok(file.model)
        }
        (Ok(None), Some(cached)) => {
            tracing::info!("Mezon-NS model is up to date");
            Ok(cached.model)
        }
        (Err(error), Some(cached)) => {
            tracing::warn!("Mezon-NS model refresh failed, using the cached copy: {error}");
            Ok(cached.model)
        }
        (Ok(None), None) => Err("Mezon-NS model cache is missing".into()),
        (Err(error), None) => Err(error),
    }
}

fn read_cached_model(dir: &Path) -> Option<ModelFile> {
    let meta = std::fs::read(dir.join(MODEL_META_FILE)).ok()?;
    let meta: serde_json::Value = serde_json::from_slice(&meta).ok()?;
    let model = std::fs::read(dir.join(MODEL_FILE)).ok()?;
    if meta.get("sha256")?.as_str()? != sha256_hex(&model) {
        return None;
    }
    let etag = meta
        .get("etag")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some(ModelFile { model, etag })
}

async fn download_model(etag: Option<String>) -> Result<Option<ModelFile>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.get(MODEL_URL);
    if let Some(etag) = &etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let model = response.bytes().await.map_err(|error| error.to_string())?;
    Ok(Some(ModelFile {
        model: model.to_vec(),
        etag,
    }))
}

fn save_model(dir: &Path, file: &ModelFile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomically(&dir.join(MODEL_FILE), &file.model)?;
    let meta = serde_json::json!({ "etag": file.etag, "sha256": sha256_hex(&file.model) });
    write_atomically(&dir.join(MODEL_META_FILE), meta.to_string().as_bytes())
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
