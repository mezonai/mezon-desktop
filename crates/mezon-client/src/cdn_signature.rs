use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use http_client::http;
use parking_lot::Mutex;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

const SIGNATURE_REFRESH_AFTER: Duration = Duration::from_secs(5 * 60 * 60);
const REFUSED_RETRY_AFTER: Duration = Duration::from_secs(60);
const FORBIDDEN_REFETCH_AFTER: Duration = Duration::from_secs(30);
const SIGNATURE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const CHANNEL_SEGMENT_LEN: usize = 16;
const IMGPROXY_SOURCE_MARKER: &str = "/plain/";
const IMGPROXY_SOURCE_ESCAPES: &AsciiSet = &CONTROLS.add(b'%').add(b'?').add(b'#').add(b'@');

pub type SignatureFetcher =
    Arc<dyn Fn(i64) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>;

enum Entry {
    Signed { signature: String, at: Instant },
    Refused { at: Instant },
}

pub struct SignedUrl {
    pub url: String,
    pub channel_id: i64,
    pub signature: String,
}

pub struct CdnSigner {
    origins: Vec<String>,
    proxies: Vec<String>,
    fetch: SignatureFetcher,
    refresh_after: Duration,
    refetch_forbidden_after: Duration,
    entries: Mutex<HashMap<i64, Entry>>,
    flights: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    generation: AtomicU64,
}

static SIGNER: OnceLock<Arc<CdnSigner>> = OnceLock::new();

pub fn install(signer: CdnSigner) {
    let _ = SIGNER.set(Arc::new(signer));
}

pub fn installed() -> Option<Arc<CdnSigner>> {
    SIGNER.get().cloned()
}

pub async fn sign(url: &str) -> Option<SignedUrl> {
    installed()?.sign(url).await
}

pub fn clear() {
    if let Some(signer) = installed() {
        signer.clear();
    }
}

pub fn forget(signed: &SignedUrl) -> bool {
    installed().is_some_and(|signer| signer.invalidate(signed.channel_id, &signed.signature))
}

pub async fn send<B, F, Fut>(url: &str, send: F) -> anyhow::Result<http::Response<B>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<http::Response<B>>>,
{
    match installed() {
        Some(signer) => signer.send(url, send).await,
        None => send(url.to_string()).await,
    }
}

impl CdnSigner {
    pub fn new(origins: Vec<String>, proxies: Vec<String>, fetch: SignatureFetcher) -> Self {
        Self::with_timing(
            origins,
            proxies,
            fetch,
            SIGNATURE_REFRESH_AFTER,
            FORBIDDEN_REFETCH_AFTER,
        )
    }

    pub(crate) fn with_timing(
        origins: Vec<String>,
        proxies: Vec<String>,
        fetch: SignatureFetcher,
        refresh_after: Duration,
        refetch_forbidden_after: Duration,
    ) -> Self {
        Self {
            origins: url_bases(origins),
            proxies: url_bases(proxies),
            fetch,
            refresh_after,
            refetch_forbidden_after,
            entries: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
        }
    }

    pub fn clear(&self) {
        let mut entries = self.entries.lock();
        self.generation.fetch_add(1, Ordering::AcqRel);
        entries.clear();
    }

    pub fn channel_of(&self, url: &str) -> Option<i64> {
        let rest = self
            .origins
            .iter()
            .find_map(|origin| url.strip_prefix(origin.as_str()))?;
        if rest.contains(['?', '#']) {
            return None;
        }
        let mut segments = rest.strip_prefix('/')?.split('/');
        let channel = channel_from_segment(segments.next()?)?;
        segments
            .next()
            .is_some_and(|file| !file.is_empty())
            .then_some(channel)
    }

    pub fn wants(&self, url: &str) -> bool {
        self.channel_of(url).is_some()
            || self
                .rendition_source(url)
                .is_some_and(|(_, source, _)| self.channel_of(source).is_some())
    }

    pub async fn sign(&self, url: &str) -> Option<SignedUrl> {
        if let Some(channel_id) = self.channel_of(url) {
            let signature = self.signature(channel_id).await?;
            return Some(SignedUrl {
                url: format!("{url}?{signature}"),
                channel_id,
                signature,
            });
        }
        let (head, source, extension) = self.rendition_source(url)?;
        let channel_id = self.channel_of(source)?;
        let signature = self.signature(channel_id).await?;
        let signed_source = format!("{source}?{signature}");
        let escaped = utf8_percent_encode(&signed_source, IMGPROXY_SOURCE_ESCAPES);
        Some(SignedUrl {
            url: format!("{head}{IMGPROXY_SOURCE_MARKER}{escaped}{extension}"),
            channel_id,
            signature,
        })
    }

    fn rendition_source<'a>(&self, url: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
        self.proxies
            .iter()
            .filter_map(|proxy| url.strip_prefix(proxy.as_str()))
            .any(|rest| rest.starts_with('/'))
            .then(|| imgproxy_source(url))
            .flatten()
    }

    pub async fn send<B, F, Fut>(&self, url: &str, send: F) -> anyhow::Result<http::Response<B>>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = anyhow::Result<http::Response<B>>>,
    {
        let Some(signed) = self.sign(url).await else {
            return send(url.to_string()).await;
        };
        let response = send(signed.url.clone()).await?;
        if response.status() != http::StatusCode::FORBIDDEN
            || !self.invalidate(signed.channel_id, &signed.signature)
        {
            return Ok(response);
        }
        match self.sign(url).await {
            Some(resigned) if resigned.signature != signed.signature => send(resigned.url).await,
            _ => Ok(response),
        }
    }

    pub fn invalidate(&self, channel_id: i64, signature: &str) -> bool {
        let mut entries = self.entries.lock();
        match entries.get(&channel_id) {
            Some(Entry::Signed {
                signature: cached,
                at,
            }) if cached == signature => {
                if at.elapsed() < self.refetch_forbidden_after {
                    return false;
                }
                entries.remove(&channel_id);
                true
            }
            _ => true,
        }
    }

    fn cached(&self, channel_id: i64) -> Option<Option<String>> {
        match self.entries.lock().get(&channel_id)? {
            Entry::Signed { signature, at } if at.elapsed() < self.refresh_after => {
                Some(Some(signature.clone()))
            }
            Entry::Refused { at } if at.elapsed() < REFUSED_RETRY_AFTER => Some(None),
            _ => None,
        }
    }

    async fn signature(&self, channel_id: i64) -> Option<String> {
        if let Some(cached) = self.cached(channel_id) {
            return cached;
        }
        let flight = self
            .flights
            .lock()
            .entry(channel_id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = flight.lock().await;
        if let Some(cached) = self.cached(channel_id) {
            return cached;
        }
        let generation = self.generation.load(Ordering::Acquire);
        let fetch = (self.fetch)(channel_id);
        let fetched = crate::transport_runtime::handle()
            .spawn(async move { tokio::time::timeout(SIGNATURE_FETCH_TIMEOUT, fetch).await })
            .await;
        let signature = match fetched {
            Ok(Ok(Ok(signature))) if !signature.is_empty() => Some(signature),
            Ok(Ok(Ok(_))) => {
                tracing::debug!(channel_id, "CDN signature came back empty");
                None
            }
            Ok(Ok(Err(error))) => {
                tracing::debug!(channel_id, "CDN signature refused: {error:#}");
                None
            }
            Ok(Err(_)) => {
                tracing::debug!(channel_id, "CDN signature request timed out");
                None
            }
            Err(error) => {
                tracing::debug!(channel_id, "CDN signature task failed: {error}");
                None
            }
        };
        let at = Instant::now();
        let entry = match &signature {
            Some(signature) => Entry::Signed {
                signature: signature.clone(),
                at,
            },
            None => Entry::Refused { at },
        };
        let mut entries = self.entries.lock();
        if self.generation.load(Ordering::Acquire) != generation {
            return None;
        }
        entries.insert(channel_id, entry);
        signature
    }
}

fn url_bases(urls: Vec<String>) -> Vec<String> {
    urls.into_iter()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .collect()
}

fn channel_from_segment(segment: &str) -> Option<i64> {
    if segment.len() != CHANNEL_SEGMENT_LEN
        || !segment
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let channel_id = u64::from_str_radix(segment, 16).ok()?;
    (channel_id != 0).then_some(channel_id as i64)
}

fn imgproxy_source(url: &str) -> Option<(&str, &str, &str)> {
    let (head, rest) = url.split_once(IMGPROXY_SOURCE_MARKER)?;
    let (source, extension) = match rest.rfind('@') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    Some((head, source, extension))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CDN: &str = "https://cdn.komu.vn";
    const PROXY: &str = "https://imgproxy.komu.vn";
    const GENERAL: i64 = 2087764882924507136;
    const FILE: &str = "https://cdn.komu.vn/1cf93b197d401000/2107323379391401984_cdn_demo.png";
    const SIGNATURE: &str = "1791260095-k0FkZPKIrCePNnae3gp5JlgpgWqxw4l04sG%2BHAODX7k%3D";

    fn counting_fetcher(
        answer: impl Fn(i64) -> anyhow::Result<String> + Send + Sync + 'static,
    ) -> (SignatureFetcher, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let answer = Arc::new(answer);
        let fetch: SignatureFetcher = Arc::new(move |channel_id| {
            counter.fetch_add(1, Ordering::SeqCst);
            let answer = answer.clone();
            Box::pin(async move { answer(channel_id) })
        });
        (fetch, calls)
    }

    fn signer_for(fetch: SignatureFetcher) -> CdnSigner {
        CdnSigner::new(vec![format!("{CDN}/")], vec![format!("{PROXY}/")], fetch)
    }

    fn expiring_signer_for(fetch: SignatureFetcher) -> CdnSigner {
        CdnSigner::with_timing(
            vec![CDN.to_string()],
            vec![PROXY.to_string()],
            fetch,
            SIGNATURE_REFRESH_AFTER,
            Duration::ZERO,
        )
    }

    #[test]
    fn only_channel_files_on_our_cdn_are_signable() {
        let (fetch, _) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = signer_for(fetch);
        assert_eq!(signer.channel_of(FILE), Some(GENERAL));
        assert_eq!(
            signer.channel_of("https://cdn.komu.vn/0000000000000000/1_avatar.png"),
            None
        );
        assert_eq!(
            signer.channel_of("https://cdn.komu.vn/1767478432163199777/2100112663546695682.txt"),
            None
        );
        assert_eq!(
            signer.channel_of("https://cdn.komu.vn/1CF93B197D401000/2107_x.png"),
            None
        );
        assert_eq!(
            signer.channel_of("https://cdn.komu.vn/emojis/2107140000000000000.webp"),
            None
        );
        assert_eq!(signer.channel_of(&format!("{FILE}?probe=1f")), None);
        assert_eq!(
            signer.channel_of("https://cdn.komu.vn/1cf93b197d401000/"),
            None
        );
        assert_eq!(
            signer.channel_of("https://media.tenor.com/1cf93b197d401000/x.gif"),
            None
        );
    }

    #[tokio::test]
    async fn a_channel_file_gets_the_signature_as_its_query() {
        let (fetch, calls) = counting_fetcher(|channel| {
            assert_eq!(channel, GENERAL);
            Ok(SIGNATURE.to_string())
        });
        let signer = signer_for(fetch);
        let signed = signer.sign(FILE).await.expect("a channel file is signable");
        assert_eq!(signed.url, format!("{FILE}?{SIGNATURE}"));
        assert_eq!(signed.channel_id, GENERAL);
        signer.sign(FILE).await.expect("still signable");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the signature is reused");
    }

    #[tokio::test]
    async fn concurrent_loads_in_one_channel_share_one_request() {
        let (fetch, calls) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = signer_for(fetch);
        let other = "https://cdn.komu.vn/1cf93b197d401000/2107323379391401985_other.png";
        let (a, b) = futures::join!(signer.sign(FILE), signer.sign(other));
        assert!(a.is_some() && b.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_old_signature_is_fetched_again() {
        let (fetch, calls) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = CdnSigner::with_timing(
            vec![CDN.to_string()],
            Vec::new(),
            fetch,
            Duration::ZERO,
            FORBIDDEN_REFETCH_AFTER,
        );
        signer.sign(FILE).await.expect("signed");
        signer.sign(FILE).await.expect("signed again");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_refused_or_empty_signature_leaves_the_url_unsigned_for_a_while() {
        let (fetch, calls) =
            counting_fetcher(|_| Err(anyhow::anyhow!("API error: code=7 (permission denied)")));
        let signer = signer_for(fetch);
        assert!(signer.sign(FILE).await.is_none());
        assert!(signer.sign(FILE).await.is_none());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a refusal is not retried at once"
        );

        let (fetch, _) = counting_fetcher(|_| Ok(String::new()));
        assert!(signer_for(fetch).sign(FILE).await.is_none());
    }

    #[tokio::test]
    async fn invalidating_a_channel_fetches_a_new_signature() {
        let (fetch, calls) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = expiring_signer_for(fetch);
        let signed = signer.sign(FILE).await.expect("signed");
        signer.invalidate(GENERAL, "1791260000-older");
        signer.sign(FILE).await.expect("still cached");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "another signature is not dropped"
        );
        signer.invalidate(GENERAL, &signed.signature);
        signer.sign(FILE).await.expect("signed after invalidate");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    fn rotating_fetcher() -> (SignatureFetcher, Arc<AtomicUsize>) {
        let issued = Arc::new(AtomicUsize::new(0));
        let next = issued.clone();
        counting_fetcher(move |_| Ok(format!("sig-{}", next.fetch_add(1, Ordering::SeqCst) + 1)))
    }

    fn cdn_answer(
        forbidden: &'static str,
        sent: Arc<parking_lot::Mutex<Vec<String>>>,
    ) -> impl Fn(String) -> futures::future::Ready<anyhow::Result<http::Response<()>>> {
        move |url| {
            let status = if url.ends_with(forbidden) {
                http::StatusCode::FORBIDDEN
            } else {
                http::StatusCode::OK
            };
            sent.lock().push(url);
            futures::future::ready(Ok(http::Response::builder()
                .status(status)
                .body(())
                .unwrap()))
        }
    }

    #[tokio::test]
    async fn a_forbidden_answer_is_sent_again_with_a_fresh_signature() {
        let (fetch, calls) = rotating_fetcher();
        let signer = expiring_signer_for(fetch);
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let response = signer
            .send(FILE, cdn_answer("?sig-1", sent.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(
            *sent.lock(),
            vec![format!("{FILE}?sig-1"), format!("{FILE}?sig-2")]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_forbidden_answer_without_a_new_signature_is_not_sent_again() {
        let (fetch, _) = counting_fetcher(|_| Ok("sig-1".to_string()));
        let signer = expiring_signer_for(fetch);
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let response = signer
            .send(FILE, cdn_answer("?sig-1", sent.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
        assert_eq!(sent.lock().len(), 1);
    }

    #[tokio::test]
    async fn loads_forbidden_together_fetch_one_new_signature() {
        let (fetch, calls) = rotating_fetcher();
        let signer = expiring_signer_for(fetch);
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let other = "https://cdn.komu.vn/1cf93b197d401000/2107323379391401985_other.png";
        let (a, b) = futures::join!(
            signer.send(FILE, cdn_answer("?sig-1", sent.clone())),
            signer.send(other, cdn_answer("?sig-1", sent.clone())),
        );
        assert_eq!(a.unwrap().status(), http::StatusCode::OK);
        assert_eq!(b.unwrap().status(), http::StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "{:?}", sent.lock());
    }

    #[tokio::test]
    async fn clearing_drops_every_signature() {
        let (fetch, calls) = rotating_fetcher();
        let signer = signer_for(fetch);
        let other = "https://cdn.komu.vn/1cfa10eff303ffff/2107323379391401985_other.png";
        signer.sign(FILE).await.expect("signed");
        signer.sign(other).await.expect("signed");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        signer.clear();
        let again = signer.sign(FILE).await.expect("signed after clear");
        assert_eq!(again.signature, "sig-3");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_signature_that_lands_after_a_clear_is_dropped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Notify::new());
        let fetch: SignatureFetcher = {
            let calls = calls.clone();
            let gate = gate.clone();
            Arc::new(move |_| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                let gate = gate.clone();
                Box::pin(async move {
                    if call == 0 {
                        gate.notified().await;
                    }
                    Ok(format!("sig-{}", call + 1))
                })
            })
        };
        let signer = Arc::new(signer_for(fetch));
        let in_flight = tokio::spawn({
            let signer = signer.clone();
            async move { signer.sign(FILE).await.map(|signed| signed.signature) }
        });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        signer.clear();
        gate.notify_one();
        assert_eq!(in_flight.await.unwrap(), None);
        let fresh = signer.sign(FILE).await.expect("signed for the new session");
        assert_eq!(fresh.signature, "sig-2");
    }

    #[tokio::test]
    async fn a_cdn_refusing_fresh_signatures_costs_one_signature_request() {
        let (fetch, calls) = rotating_fetcher();
        let signer = signer_for(fetch);
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        for index in 0..30 {
            let file = format!("{CDN}/1cf93b197d401000/21073233793914{index:05}_x.png");
            let response = signer
                .send(&file, cdn_answer("", sent.clone()))
                .await
                .unwrap();
            assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(sent.lock().len(), 30);
        assert!(
            !signer.invalidate(GENERAL, "sig-1"),
            "a fresh signature is kept"
        );
    }

    #[tokio::test]
    async fn a_rendition_from_another_host_never_gets_the_signature() {
        let (fetch, calls) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = signer_for(fetch);
        for host in [
            "https://evil.example/k",
            "https://imgproxy.komu.vn.evil.example/k",
            "https://evil.example/https://imgproxy.komu.vn/k",
        ] {
            let rendition = format!("{host}/rs:fit:80:80:1/mb:2097152/plain/{FILE}@webp");
            assert!(!signer.wants(&rendition), "{rendition}");
            assert!(signer.sign(&rendition).await.is_none(), "{rendition}");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_file_outside_any_channel_is_sent_unsigned() {
        let (fetch, calls) = rotating_fetcher();
        let signer = signer_for(fetch);
        let sent = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let avatar = "https://cdn.komu.vn/1767478432163199777/2100112663546695682.png";
        signer
            .send(avatar, cdn_answer("?sig-1", sent.clone()))
            .await
            .unwrap();
        assert_eq!(*sent.lock(), vec![avatar.to_string()]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_imgproxy_rendition_signs_its_source_escaped() {
        let (fetch, _) = counting_fetcher(|_| Ok(SIGNATURE.to_string()));
        let signer = signer_for(fetch);
        let rendition =
            format!("https://imgproxy.komu.vn/k/rs:fill:800:600:1/mb:2097152/plain/{FILE}@webp");
        assert!(signer.wants(&rendition));
        let signed = signer
            .sign(&rendition)
            .await
            .expect("rendition is signable");
        let (head, rest) = signed.url.split_once("/plain/").unwrap();
        assert_eq!(
            head,
            "https://imgproxy.komu.vn/k/rs:fill:800:600:1/mb:2097152"
        );
        let escaped = rest.strip_suffix("@webp").expect("extension kept");
        assert!(!escaped.contains('?'), "{escaped}");
        let source = percent_encoding::percent_decode_str(escaped)
            .decode_utf8()
            .unwrap();
        assert_eq!(source, format!("{FILE}?{SIGNATURE}"));

        let foreign = "https://imgproxy.komu.vn/k/rs:fill:80:80:1/mb:2097152/plain/https://media.tenor.com/a.gif@webp";
        assert!(!signer.wants(foreign));
        assert!(signer.sign(foreign).await.is_none());
    }
}
