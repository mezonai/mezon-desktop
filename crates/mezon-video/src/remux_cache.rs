use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::PlayerError;
use crate::matroska_remux::{self, RemuxError};

const CACHE_DIR_NAME: &str = "mezon-video-remux";
const CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const STALE_PARTIAL_AFTER: Duration = Duration::from_secs(60 * 60);
const PROBE_SECONDS: f64 = 2.0;

static NEXT_PARTIAL: AtomicU64 = AtomicU64::new(0);

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTRegisterSupplementalVideoDecoderIfAvailable(codec_type: u32);
}

pub(crate) enum PreparedSource {
    Remuxed(PathBuf),
    Vp8(Vec<u8>),
}

pub(crate) fn is_matroska_source(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (ext.eq_ignore_ascii_case("webm") || ext.eq_ignore_ascii_case("mkv"))
        })
}

pub(crate) fn register_vp9_decoder() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        VTRegisterSupplementalVideoDecoderIfAvailable(u32::from_be_bytes(*b"vp09"));
    });
}

pub(crate) fn prepare(url: &str) -> Result<PreparedSource, PlayerError> {
    register_vp9_decoder();
    let dir = cache_dir()?;
    let local = crate::webm_player::local_file_path(url).map(PathBuf::from);
    let key = cache_key(url, local.as_deref());
    let remuxed = dir.join(format!("{key:016x}.mp4"));
    if remuxed.is_file() {
        touch(&remuxed);
        return Ok(PreparedSource::Remuxed(remuxed));
    }
    let partial = next_partial();
    let downloaded = match &local {
        Some(_) => None,
        None => {
            let target = dir.join(format!("{key:016x}.{partial}.src"));
            if let Err(error) = crate::webm_player::download_to_file(url, &target) {
                let _ = fs::remove_file(&target);
                return Err(error);
            }
            Some(target)
        }
    };
    let source = local
        .as_deref()
        .or(downloaded.as_deref())
        .ok_or(PlayerError::InvalidUrl)?;
    let staged = dir.join(format!("{key:016x}.{partial}.part"));
    let result = match matroska_remux::remux_to_mp4(source, &staged, None) {
        Ok(()) => fs::rename(&staged, &remuxed)
            .map(|()| PreparedSource::Remuxed(remuxed.clone()))
            .map_err(|error| {
                tracing::warn!(target: "mezon_video", ?error, "remuxed video could not be stored");
                PlayerError::Open
            }),
        Err(RemuxError::Vp8) => {
            crate::webm_player::read_source_capped(source).map(PreparedSource::Vp8)
        }
        Err(error) => {
            tracing::warn!(target: "mezon_video", %error, "matroska remux failed");
            Err(PlayerError::Open)
        }
    };
    let _ = fs::remove_file(&staged);
    if let Some(downloaded) = downloaded {
        let _ = fs::remove_file(downloaded);
    }
    if result.is_ok() {
        prune(&dir, &remuxed);
    }
    result
}

pub(crate) enum ProbeSource {
    Remuxed(PathBuf),
    Vp8,
}

pub(crate) fn probe_source(path: &str) -> Option<ProbeSource> {
    register_vp9_decoder();
    let dir = cache_dir().ok()?;
    let staged = dir.join(format!("probe.{}.mp4", next_partial()));
    match matroska_remux::remux_to_mp4(Path::new(path), &staged, Some(PROBE_SECONDS)) {
        Ok(()) => Some(ProbeSource::Remuxed(staged)),
        Err(RemuxError::Vp8) => {
            let _ = fs::remove_file(&staged);
            Some(ProbeSource::Vp8)
        }
        Err(error) => {
            tracing::debug!(target: "mezon_video", %error, "matroska probe remux failed");
            let _ = fs::remove_file(&staged);
            None
        }
    }
}

fn cache_dir() -> Result<PathBuf, PlayerError> {
    let dir = std::env::temp_dir().join(CACHE_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|error| {
        tracing::warn!(target: "mezon_video", ?error, "video remux cache dir unavailable");
        PlayerError::Open
    })?;
    Ok(dir)
}

fn next_partial() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        NEXT_PARTIAL.fetch_add(1, Ordering::Relaxed)
    )
}

fn cache_key(url: &str, local: Option<&Path>) -> u64 {
    let mut hash = Fnv1a::default();
    match local {
        Some(path) => {
            hash.write(path.as_os_str().as_encoded_bytes());
            if let Ok(meta) = fs::metadata(path) {
                hash.write(&meta.len().to_le_bytes());
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_nanos());
                hash.write(&modified.to_le_bytes());
            }
        }
        None => hash.write(url.split(['?', '#']).next().unwrap_or(url).as_bytes()),
    }
    hash.0
}

struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv1a {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

fn touch(path: &Path) {
    if let Ok(file) = fs::File::options().append(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn prune(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut finished = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(now);
        let is_finished = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".mp4") && !name.starts_with("probe."));
        if is_finished {
            finished.push((modified, meta.len(), path));
        } else if now.duration_since(modified).unwrap_or_default() > STALE_PARTIAL_AFTER {
            let _ = fs::remove_file(&path);
        }
    }
    finished.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    let mut total = 0u64;
    for (_, len, path) in finished {
        total = total.saturating_add(len);
        if total > CACHE_BUDGET_BYTES && path != keep {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matroska_sources_are_detected_by_extension() {
        assert!(is_matroska_source("https://cdn.example/a/clip.webm"));
        assert!(is_matroska_source("https://cdn.example/a/clip.MKV?sig=1"));
        assert!(!is_matroska_source("https://cdn.example/a/clip.mp4"));
        assert!(!is_matroska_source("https://cdn.example/a/.mkv"));
    }

    #[test]
    fn cache_key_ignores_the_signature_query() {
        assert_eq!(
            cache_key("https://cdn.example/a/clip.mkv?1700000000-aa", None),
            cache_key("https://cdn.example/a/clip.mkv?1800000000-bb", None)
        );
        assert_ne!(
            cache_key("https://cdn.example/a/clip.mkv", None),
            cache_key("https://cdn.example/b/clip.mkv", None)
        );
    }
}
