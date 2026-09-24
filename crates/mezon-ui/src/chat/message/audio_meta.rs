use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::AsyncReadExt;
use futures::future::{Either, FutureExt};
use gpui::{
    App, AppContext, Context, Entity, Global, SharedString,
    http_client::{AsyncBody, HttpClient, HttpRequestExt, RedirectPolicy},
};
use mezon_store::MessageAttachment;

const AUDIO_META_PREFIX: usize = 64 * 1024;
const AUDIO_META_PREFIX_MAX: usize = 256 * 1024;
const AUDIO_META_FRAME_SLACK: usize = 8 * 1024;
const AUDIO_META_INFLIGHT: usize = 3;
const AUDIO_META_ATTEMPTS: u8 = 2;
const AUDIO_META_CACHE_CAP: usize = 256;
const AUDIO_META_TIMEOUT: Duration = Duration::from_secs(8);

struct GlobalAudioMetaCache(Entity<AudioMetaCache>);

impl Global for GlobalAudioMetaCache {}

#[derive(Clone, Copy)]
struct AudioMeta {
    duration: f64,
    size: u64,
}

pub struct AudioMetaCache {
    known: HashMap<String, AudioMeta>,
    pending: HashSet<String>,
    failures: HashMap<String, u8>,
}

impl AudioMetaCache {
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalAudioMetaCache>() {
            return global.0.clone();
        }
        let entity = cx.new(|_| Self {
            known: HashMap::new(),
            pending: HashSet::new(),
            failures: HashMap::new(),
        });
        cx.set_global(GlobalAudioMetaCache(entity.clone()));
        entity
    }

    fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAudioMetaCache>()
            .map(|global| global.0.clone())
    }

    pub fn ensure_attachments(attachments: &[MessageAttachment], cx: &mut App) {
        let urls = attachments
            .iter()
            .filter(|att| attachment_needs_audio_probe(att))
            .map(|att| att.url.clone())
            .collect();
        Self::ensure_urls(urls, cx);
    }

    pub fn ensure_urls(urls: Vec<String>, cx: &mut App) {
        if urls.is_empty() {
            return;
        }
        let missing = match Self::try_global(cx) {
            Some(cache) => urls
                .into_iter()
                .filter(|url| !cache.read(cx).settled(url))
                .map(SharedString::from)
                .collect::<Vec<_>>(),
            None => urls.into_iter().map(SharedString::from).collect(),
        };
        if missing.is_empty() {
            return;
        }
        Self::global(cx).update(cx, |cache, cx| {
            for url in missing {
                cache.ensure(url, cx);
            }
        });
    }

    fn settled(&self, url: &str) -> bool {
        self.known.contains_key(url)
            || self.pending.contains(url)
            || self
                .failures
                .get(url)
                .is_some_and(|count| *count >= AUDIO_META_ATTEMPTS)
    }

    fn remember(&mut self, key: String, meta: AudioMeta) {
        self.failures.remove(&key);
        self.known.insert(key.clone(), meta);
        if self.known.len() <= AUDIO_META_CACHE_CAP {
            return;
        }
        if let Some(evict) = self
            .known
            .keys()
            .find(|candidate| *candidate != &key)
            .cloned()
        {
            self.known.remove(&evict);
        }
    }

    fn note_failure(&mut self, key: String, permanent: bool) {
        let count = if permanent {
            AUDIO_META_ATTEMPTS
        } else {
            self.failures
                .get(&key)
                .copied()
                .unwrap_or(0)
                .saturating_add(1)
        };
        self.failures.insert(key.clone(), count);
        if self.failures.len() <= AUDIO_META_CACHE_CAP {
            return;
        }
        if let Some(evict) = self
            .failures
            .keys()
            .find(|candidate| *candidate != &key)
            .cloned()
        {
            self.failures.remove(&evict);
        }
    }

    fn ensure(&mut self, url: SharedString, cx: &mut Context<Self>) {
        let key = url.to_string();
        if key.is_empty() || self.settled(&key) || self.pending.len() >= AUDIO_META_INFLIGHT {
            return;
        }
        self.pending.insert(key.clone());
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let fetch = cx
                .background_executor()
                .spawn(async move { probe_remote(&client, &url).await });
            let timeout = cx.background_executor().timer(AUDIO_META_TIMEOUT);
            let probed = match futures::future::select(fetch.fuse(), timeout.fuse()).await {
                Either::Left((result, _)) => result,
                Either::Right((_, _)) => Err(ProbeError::Transient("audio metadata timed out")),
            };
            let _ = this.update(cx, |cache, cx| {
                cache.pending.remove(&key);
                match probed {
                    Ok(meta) => {
                        cache.remember(key, meta);
                        cx.notify();
                    }
                    Err(ProbeError::Permanent(err)) => {
                        tracing::warn!("audio metadata probe failed: {err}");
                        cache.note_failure(key, true);
                    }
                    Err(ProbeError::Transient(err)) => {
                        tracing::warn!("audio metadata probe failed: {err}");
                        let retry = cache
                            .failures
                            .get(&key)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(1)
                            < AUDIO_META_ATTEMPTS;
                        cache.note_failure(key, false);
                        if retry {
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }
}

pub(crate) fn display_audio_duration(att: &MessageAttachment, cx: &App) -> f64 {
    if att.duration > 0 {
        return att.duration.max(0) as f64;
    }
    lookup(cx, &att.url)
        .map(|meta| meta.duration)
        .filter(|duration| *duration > 0.0)
        .unwrap_or(0.0)
}

pub(crate) fn display_attachment_bytes(att: &MessageAttachment, cx: &App) -> u64 {
    if att.size > 0 {
        return att.size;
    }
    if att.is_audio() {
        return lookup(cx, &att.url)
            .map(|meta| meta.size)
            .filter(|size| *size > 0)
            .unwrap_or(0);
    }
    att.size
}

pub(crate) fn attachment_needs_audio_probe(att: &MessageAttachment) -> bool {
    att.is_audio()
        && !att.url.is_empty()
        && !att.uploading
        && !att.presign_pending
        && !att.upload_failed
        && (att.duration <= 0 || att.size == 0)
}

pub(crate) fn urls_needing_probe(attachments: &[MessageAttachment], cx: &App) -> Vec<String> {
    let tracked =
        |url: &str| AudioMetaCache::try_global(cx).is_some_and(|cache| cache.read(cx).settled(url));
    attachments
        .iter()
        .filter(|att| attachment_needs_audio_probe(att))
        .map(|att| att.url.clone())
        .filter(|url| !tracked(url))
        .collect()
}

pub(crate) fn defer_audio_probe(urls: Vec<String>, cx: &mut App) {
    if urls.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        AudioMetaCache::ensure_urls(urls, cx);
    });
}

fn lookup(cx: &App, url: &str) -> Option<AudioMeta> {
    AudioMetaCache::try_global(cx)?
        .read(cx)
        .known
        .get(url)
        .copied()
}

enum ProbeError {
    Permanent(&'static str),
    Transient(&'static str),
}

struct RemotePrefix {
    bytes: Vec<u8>,
    total: Option<u64>,
    ended: bool,
}

async fn probe_remote(client: &Arc<dyn HttpClient>, url: &str) -> Result<AudioMeta, ProbeError> {
    let head_len = head_content_length(client, url).await;
    let prefix = read_prefix(client, url).await?;
    let size = prefix.total.or(head_len);
    let Some(size) = size else {
        if !prefix.ended {
            return Err(ProbeError::Transient("audio metadata incomplete"));
        }
        let size = prefix.bytes.len() as u64;
        let duration =
            mezon_audio::audio_duration_secs_with_len(&prefix.bytes, size).unwrap_or(0.0);
        return Ok(AudioMeta { duration, size });
    };
    let duration = mezon_audio::audio_duration_secs_with_len(&prefix.bytes, size).unwrap_or(0.0);
    if duration <= 0.0 && (prefix.bytes.len() as u64) < size {
        return Err(ProbeError::Transient("audio duration unavailable"));
    }
    Ok(AudioMeta { duration, size })
}

async fn head_content_length(client: &Arc<dyn HttpClient>, url: &str) -> Option<u64> {
    let request = gpui::http_client::http::Request::builder()
        .method(gpui::http_client::Method::HEAD)
        .uri(url)
        .follow_redirects(RedirectPolicy::FollowAll)
        .body(AsyncBody::empty())
        .ok()?;
    let response = client.send(request).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    header_len(response.headers())
}

async fn read_prefix(client: &Arc<dyn HttpClient>, url: &str) -> Result<RemotePrefix, ProbeError> {
    let end = AUDIO_META_PREFIX_MAX.saturating_sub(1);
    let request = gpui::http_client::http::Request::builder()
        .method(gpui::http_client::Method::GET)
        .uri(url)
        .header("Range", format!("bytes=0-{end}"))
        .follow_redirects(RedirectPolicy::FollowAll)
        .body(AsyncBody::empty())
        .map_err(|_| ProbeError::Permanent("audio metadata request failed"))?;
    let mut response = client
        .send(request)
        .await
        .map_err(|_| ProbeError::Transient("audio metadata request failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProbeError::Permanent("audio metadata status"));
    }
    let total = if status == gpui::http_client::StatusCode::OK {
        header_len(response.headers())
    } else {
        range_total(response.headers())
    };
    let (bytes, ended) = read_audio_prefix(response.body_mut())
        .await
        .map_err(|_| ProbeError::Transient("audio metadata read failed"))?;
    Ok(RemotePrefix {
        bytes,
        total,
        ended,
    })
}

fn prefix_goal(bytes: &[u8]) -> usize {
    let tag = mezon_audio::id3_tag_len(bytes);
    if tag > bytes.len() {
        return tag
            .saturating_add(AUDIO_META_FRAME_SLACK)
            .min(AUDIO_META_PREFIX_MAX);
    }
    AUDIO_META_PREFIX
}

async fn read_audio_prefix(body: &mut AsyncBody) -> std::io::Result<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let goal = prefix_goal(&out);
        if out.len() >= goal || out.len() >= AUDIO_META_PREFIX_MAX {
            let peek = body.read(&mut buf[..1]).await?;
            return Ok((out, peek == 0));
        }
        let want = (goal - out.len()).min(buf.len());
        let read = body.read(&mut buf[..want]).await?;
        if read == 0 {
            return Ok((out, true));
        }
        out.extend_from_slice(&buf[..read]);
    }
}

fn header_len(headers: &gpui::http_client::http::HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|length| *length > 0)
}

fn range_total(headers: &gpui::http_client::http::HeaderMap) -> Option<u64> {
    let value = headers.get("content-range")?.to_str().ok()?;
    let total = value.rsplit('/').next()?;
    total.parse::<u64>().ok().filter(|length| *length > 0)
}

#[cfg(test)]
mod tests {
    use super::attachment_needs_audio_probe;
    use mezon_store::MessageAttachment;

    fn audio(url: &str, duration: i32, size: u64) -> MessageAttachment {
        MessageAttachment {
            url: url.into(),
            filetype: "audio/mpeg".into(),
            duration,
            size,
            ..Default::default()
        }
    }

    #[test]
    fn a_sound_without_duration_or_size_needs_a_probe() {
        assert!(attachment_needs_audio_probe(&audio(
            "https://cdn/leave.mp3",
            0,
            0
        )));
        assert!(attachment_needs_audio_probe(&audio(
            "https://cdn/clip.mp3",
            0,
            4096
        )));
        assert!(!attachment_needs_audio_probe(&audio(
            "https://cdn/clip.mp3",
            3,
            4096
        )));
        assert!(!attachment_needs_audio_probe(&audio("", 0, 0)));
    }
}
