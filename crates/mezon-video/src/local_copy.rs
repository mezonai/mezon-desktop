use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::PlayerError;

const CACHE_DIR_NAME: &str = "videos";
const MAX_VERDICTS: usize = 512;
const STALE_PARTIAL_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_COPY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CACHE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RECV_BODY_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_BYTES: u64 = 16 * 1024;
const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

static NEXT_PARTIAL: AtomicU64 = AtomicU64::new(0);
static VERDICTS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

struct Probe {
    head: Vec<u8>,
    total: Option<u64>,
}

fn remembered_verdict(url: &str) -> Option<bool> {
    let verdicts = VERDICTS.get()?.lock().unwrap_or_else(|e| e.into_inner());
    verdicts.get(url_path(url)).copied()
}

fn remember_verdict(url: &str, copy: bool) {
    let mut verdicts = VERDICTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if verdicts.len() >= MAX_VERDICTS {
        verdicts.clear();
    }
    verdicts.insert(url_path(url).to_owned(), copy);
}

fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|base| base.join("mezon").join(CACHE_DIR_NAME))
}

fn url_path(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

pub fn is_remote_video(url: &str) -> bool {
    let path = url_path(url);
    (path.starts_with("http://") || path.starts_with("https://"))
        && !path.to_ascii_lowercase().ends_with(".m3u8")
}

pub fn prepare(url: &str) -> Result<Option<String>, PlayerError> {
    let Some(dir) = cache_dir() else {
        return Ok(None);
    };
    let target = dir.join(file_name(url));
    if !target.is_file() {
        if remembered_verdict(url) == Some(false) {
            return Ok(None);
        }
        let Some(probe) = probe_head(url) else {
            return Ok(None);
        };
        let copy = cannot_seek_while_streaming(&probe.head)
            && probe.total.is_none_or(|total| total <= MAX_COPY_BYTES);
        if !copy {
            remember_verdict(url, false);
            return Ok(None);
        }
        std::fs::create_dir_all(&dir).map_err(|error| {
            tracing::warn!(target: "mezon_video", %error, "video cache dir unavailable");
            PlayerError::Open
        })?;
        prune(&dir);
        if let Err(error) = download(url, &target) {
            remember_verdict(url, false);
            return Err(error);
        }
    }
    let uri = gstreamer::glib::filename_to_uri(&target, None).map_err(|error| {
        tracing::warn!(target: "mezon_video", %error, "video cache path is not a uri");
        PlayerError::Open
    })?;
    Ok(Some(uri.to_string()))
}

fn file_name(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url_path(url).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn http_agent(recv_body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(recv_body))
        .build()
        .into()
}

fn probe_head(url: &str) -> Option<Probe> {
    let mut response = http_agent(PROBE_TIMEOUT)
        .get(url)
        .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
        .call()
        .map_err(|error| tracing::warn!(target: "mezon_video", ?error, "video probe failed"))
        .ok()?;
    let total = total_size(&response);
    let mut head = Vec::with_capacity(PROBE_BYTES as usize);
    response
        .body_mut()
        .as_reader()
        .take(PROBE_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    Some(Probe { head, total })
}

fn total_size(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    let headers = response.headers();
    if let Some(range) = headers.get("content-range") {
        return range
            .to_str()
            .ok()
            .and_then(|value| value.rsplit('/').next())
            .and_then(|total| total.trim().parse().ok());
    }
    if response.status() != ureq::http::StatusCode::OK {
        return None;
    }
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|length| length.trim().parse().ok())
}

fn cannot_seek_while_streaming(head: &[u8]) -> bool {
    head.starts_with(&EBML_MAGIC) || is_fragmented_mp4(head)
}

fn read_box(data: &[u8]) -> Option<([u8; 4], usize)> {
    let size = u32::from_be_bytes(data.get(0..4)?.try_into().ok()?) as usize;
    let kind: [u8; 4] = data.get(4..8)?.try_into().ok()?;
    (size >= 8).then_some((kind, size))
}

fn is_fragmented_mp4(head: &[u8]) -> bool {
    let mut offset = 0;
    while let Some((kind, size)) = head.get(offset..).and_then(read_box) {
        match &kind {
            b"moof" => return true,
            b"moov" => {
                return head
                    .get(offset + 8..offset + size)
                    .is_some_and(moov_is_fragmented);
            }
            b"mdat" => return false,
            _ => {}
        }
        offset += size;
    }
    false
}

fn moov_is_fragmented(moov: &[u8]) -> bool {
    let mut offset = 0;
    while let Some((kind, size)) = moov.get(offset..).and_then(read_box) {
        match &kind {
            b"mvex" => return true,
            b"mvhd" if mvhd_duration(moov.get(offset + 8..offset + size)) == Some(0) => {
                return true;
            }
            _ => {}
        }
        offset += size;
    }
    false
}

fn mvhd_duration(body: Option<&[u8]>) -> Option<u64> {
    let body = body?;
    match *body.first()? {
        0 => Some(u64::from(u32::from_be_bytes(
            body.get(16..20)?.try_into().ok()?,
        ))),
        1 => Some(u64::from_be_bytes(body.get(24..32)?.try_into().ok()?)),
        _ => None,
    }
}

fn download(url: &str, target: &Path) -> Result<(), PlayerError> {
    let mut response = http_agent(RECV_BODY_TIMEOUT)
        .get(url)
        .call()
        .map_err(|error| {
            tracing::warn!(target: "mezon_video", ?error, "video download failed");
            PlayerError::Open
        })?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_COPY_BYTES)
        .reader();
    let partial: PathBuf = target.with_extension(format!(
        "{}-{}.part",
        std::process::id(),
        NEXT_PARTIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let copied = std::fs::File::create(&partial)
        .and_then(|mut file| std::io::copy(&mut reader, &mut file))
        .and_then(|_| std::fs::rename(&partial, target));
    if let Err(error) = copied {
        tracing::warn!(target: "mezon_video", %error, "video download interrupted");
        let _ = std::fs::remove_file(&partial);
        return Err(PlayerError::Open);
    }
    Ok(())
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        let is_partial = entry.path().extension().is_some_and(|ext| ext == "part");
        let expired = if is_partial {
            age > STALE_PARTIAL_AFTER
        } else {
            age > MAX_CACHE_AGE
        };
        if expired {
            let _ = std::fs::remove_file(entry.path());
        } else if !is_partial {
            kept.push((modified, meta.len(), entry.path()));
        }
    }
    kept.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
    let mut total = 0u64;
    for (_, len, path) in kept {
        total += len;
        if total > MAX_CACHE_BYTES {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn mvhd(duration: u32) -> Vec<u8> {
        let mut body = vec![0u8; 100];
        body[12..16].copy_from_slice(&1000u32.to_be_bytes());
        body[16..20].copy_from_slice(&duration.to_be_bytes());
        mp4_box(b"mvhd", &body)
    }

    fn mp4_with_moov(moov_children: &[Vec<u8>]) -> Vec<u8> {
        let mut file = mp4_box(b"ftyp", b"isom0000");
        file.extend(mp4_box(b"moov", &moov_children.concat()));
        file
    }

    #[test]
    fn webm_needs_a_local_copy() {
        assert!(cannot_seek_while_streaming(&[
            0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42
        ]));
    }

    #[test]
    fn fragmented_mp4_with_mvex_needs_a_local_copy() {
        let head = mp4_with_moov(&[mvhd(5000), mp4_box(b"mvex", &[])]);
        assert!(cannot_seek_while_streaming(&head));
    }

    #[test]
    fn mp4_without_duration_needs_a_local_copy() {
        assert!(cannot_seek_while_streaming(&mp4_with_moov(&[mvhd(0)])));
    }

    #[test]
    fn mp4_starting_with_a_fragment_needs_a_local_copy() {
        let mut head = mp4_box(b"ftyp", b"isom0000");
        head.extend(mp4_box(b"moof", &[]));
        assert!(cannot_seek_while_streaming(&head));
    }

    #[test]
    fn classic_mp4_streams() {
        assert!(!cannot_seek_while_streaming(&mp4_with_moov(&[mvhd(
            518_641
        )])));
    }

    #[test]
    fn mp4_with_moov_after_mdat_streams() {
        let mut head = mp4_box(b"ftyp", b"isom0000");
        head.extend(mp4_box(b"mdat", &[0; 64]));
        assert!(!cannot_seek_while_streaming(&head));
    }

    #[test]
    fn truncated_or_empty_head_streams() {
        assert!(!cannot_seek_while_streaming(&[]));
        assert!(!cannot_seek_while_streaming(&[0, 0, 0, 40, b'm', b'o']));
    }

    #[test]
    fn m3u8_and_local_paths_are_not_remote_videos() {
        assert!(is_remote_video("https://cdn/x/a.mp4?sig=1"));
        assert!(!is_remote_video("https://cdn/x/master.m3u8?sig=1"));
        assert!(!is_remote_video("file:///tmp/a.mp4"));
    }
}
