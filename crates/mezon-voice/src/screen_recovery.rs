//! Bounded screen recovery, independent of WebRTC and UI runtimes.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GRACE: Duration = Duration::from_millis(400);
const GLOBAL_INTERVAL: Duration = Duration::from_millis(250);
const PUBLISHER_INTERVAL: Duration = Duration::from_millis(1500);
const RETRIES: [Duration; 2] = [Duration::from_millis(1500), Duration::from_secs(3)];

#[derive(Debug, Default)]
struct ViewState {
    main: HashMap<u64, bool>,
    collecting: Option<(HashSet<u64>, Option<u64>, bool)>,
    pip: Option<u64>,
}

/// Shared UI interest. Hidden grid pages do not enter the recovery queue.
#[derive(Debug, Default)]
pub struct ScreenViews(Mutex<ViewState>);

impl ScreenViews {
    pub fn begin(self: &Arc<Self>, enabled: bool, priority: Option<u64>) -> ScreenViewGuard {
        self.0.lock().unwrap().collecting = Some((HashSet::new(), priority, enabled));
        ScreenViewGuard(self.clone())
    }

    pub fn note_rendered(&self, key: u64) {
        if let Some((keys, _, _)) = &mut self.0.lock().unwrap().collecting {
            keys.insert(key);
        }
    }

    pub fn clear_main(&self) {
        self.0.lock().unwrap().main.clear();
    }

    pub fn set_pip(&self, key: Option<u64>) {
        self.0.lock().unwrap().pip = key;
    }

    pub(crate) fn snapshot(&self) -> HashMap<u64, bool> {
        let state = self.0.lock().unwrap();
        let mut views = state.main.clone();
        if let Some(key) = state.pip {
            views.insert(key, true);
        }
        views
    }
}

/// Commits a whole render pass, including an empty pass when leaving the call view.
/// UI rendering is synchronous; this guard must not span an await or another window's render.
pub struct ScreenViewGuard(Arc<ScreenViews>);

impl Drop for ScreenViewGuard {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap();
        if let Some((keys, priority, enabled)) = state.collecting.take() {
            state.main = keys
                .into_iter()
                .filter(|_| enabled)
                .map(|key| (key, priority == Some(key)))
                .collect();
        }
    }
}

#[derive(Default)]
struct ReceiptState {
    frame_at: Option<Instant>,
    closed: bool,
}

/// One receipt per native stream, never identified by a reused MID/track string alone.
#[derive(Default)]
pub struct FrameReceipt(Mutex<ReceiptState>);

impl FrameReceipt {
    pub(crate) fn last_frame(&self) -> Option<Instant> {
        self.0.lock().unwrap().frame_at
    }

    pub(crate) fn publish<T>(&self, now: Instant, publish: impl FnOnce() -> T) -> Option<T> {
        let mut state = self.0.lock().unwrap();
        if state.closed {
            return None;
        }
        let result = publish();
        state.frame_at = Some(now);
        Some(result)
    }

    pub(crate) fn close(&self, cleanup: impl FnOnce()) {
        let mut state = self.0.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        state.frame_at = None;
        cleanup();
    }
}

pub(crate) struct ScreenSource {
    pub key: u64,
    pub publisher: u32,
    pub receipt: Arc<FrameReceipt>,
}

struct Recovery {
    publisher: u32,
    receipt: Arc<FrameReceipt>,
    since: Instant,
    ready: Instant,
    visible: bool,
    priority: bool,
    attempts: usize,
    last_sent: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct ScreenRecovery {
    sources: HashMap<u64, Recovery>,
    last_by_publisher: HashMap<u32, Instant>,
    last_request: Option<Instant>,
}

impl ScreenRecovery {
    pub(crate) fn update(
        &mut self,
        sources: Vec<ScreenSource>,
        views: &HashMap<u64, bool>,
        now: Instant,
    ) {
        let active: HashSet<_> = sources.iter().map(|source| source.key).collect();
        self.sources.retain(|key, _| active.contains(key));
        // Keep cooldowns across rapid stop/start or MID replacement, but not indefinitely.
        self.last_by_publisher
            .retain(|_, sent| now.saturating_duration_since(*sent) < PUBLISHER_INTERVAL);
        for source in sources {
            if source.publisher == 0 {
                continue;
            }
            let same = self.sources.get(&source.key).is_some_and(|old| {
                old.publisher == source.publisher && Arc::ptr_eq(&old.receipt, &source.receipt)
            });
            if !same {
                self.sources.insert(
                    source.key,
                    Recovery {
                        publisher: source.publisher,
                        receipt: source.receipt,
                        since: now,
                        ready: now + GRACE,
                        visible: false,
                        priority: false,
                        attempts: 0,
                        last_sent: None,
                    },
                );
            }
            let state = self.sources.get_mut(&source.key).unwrap();
            let visible = views.contains_key(&source.key);
            let priority = views.get(&source.key).copied().unwrap_or(false);
            if visible && !state.visible {
                state.ready = now + if priority { Duration::ZERO } else { GRACE };
            }
            state.visible = visible;
            state.priority = priority;
        }
    }

    /// Caller gates this on joined + connected + no pending negotiation.
    pub(crate) fn next_request(&self, now: Instant) -> Option<u32> {
        if self
            .last_request
            .is_some_and(|sent| now.saturating_duration_since(sent) < GLOBAL_INTERVAL)
        {
            return None;
        }
        self.sources
            .values()
            .filter(|source| {
                source.visible
                    && source.attempts <= RETRIES.len()
                    && now >= source.ready
                    && !source
                        .receipt
                        .last_frame()
                        .is_some_and(|frame| frame >= source.since)
                    && !self
                        .last_by_publisher
                        .get(&source.publisher)
                        .is_some_and(|sent| {
                            now.saturating_duration_since(*sent) < PUBLISHER_INTERVAL
                        })
                    && source.last_sent.is_none_or(|sent| {
                        now.saturating_duration_since(sent) >= RETRIES[source.attempts - 1]
                    })
            })
            .min_by_key(|source| (!source.priority, source.ready, source.publisher))
            .map(|source| source.publisher)
    }

    pub(crate) fn sent(&mut self, publisher: u32, now: Instant) {
        self.last_request = Some(now);
        self.last_by_publisher.insert(publisher, now);
        // A publisher can appear in multiple views; they share requests and budget.
        for source in self
            .sources
            .values_mut()
            .filter(|source| source.publisher == publisher)
        {
            source.attempts += 1;
            source.last_sent = Some(now);
        }
    }

    pub(crate) fn is_recent_request_error(&self, message: &str, now: Instant) -> bool {
        matches!(message, "must_join_room_first" | "session_not_found")
            && self
                .last_request
                .is_some_and(|sent| now.saturating_duration_since(sent) < Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(key: u64, publisher: u32, receipt: &Arc<FrameReceipt>) -> ScreenSource {
        ScreenSource {
            key,
            publisher,
            receipt: receipt.clone(),
        }
    }

    #[test]
    fn three_requests_with_backoff_then_stop() {
        let start = Instant::now();
        let receipt = Arc::default();
        let mut queue = ScreenRecovery::default();
        queue.update(
            vec![source(5, 29, &receipt)],
            &HashMap::from([(5, false)]),
            start,
        );
        assert_eq!(
            queue.next_request(start + GRACE - Duration::from_millis(1)),
            None
        );
        for millis in [400, 1900, 4900] {
            let now = start + Duration::from_millis(millis);
            assert_eq!(queue.next_request(now), Some(29));
            queue.sent(29, now);
            assert_eq!(queue.next_request(now + Duration::from_millis(1499)), None);
        }
        assert_eq!(queue.next_request(start + Duration::from_secs(120)), None);
    }

    #[test]
    fn real_frame_cancels_retry_and_static_screen_stays_healthy() {
        let start = Instant::now();
        let receipt = Arc::new(FrameReceipt::default());
        let mut queue = ScreenRecovery::default();
        queue.update(
            vec![source(5, 29, &receipt)],
            &HashMap::from([(5, false)]),
            start,
        );
        queue.sent(29, start + GRACE);
        receipt.publish(start + Duration::from_secs(1), || ());
        assert_eq!(queue.next_request(start + Duration::from_secs(2)), None);
        assert_eq!(queue.next_request(start + Duration::from_secs(3600)), None);
    }

    #[test]
    fn offscreen_and_refocus_do_not_reset_budget() {
        let start = Instant::now();
        let receipt = Arc::default();
        let mut queue = ScreenRecovery::default();
        for millis in [0, 1500, 4500] {
            let now = start + Duration::from_millis(millis);
            queue.update(
                vec![source(5, 29, &receipt)],
                &HashMap::from([(5, true)]),
                now,
            );
            assert_eq!(queue.next_request(now), Some(29));
            queue.sent(29, now);
        }
        queue.update(
            vec![source(5, 29, &receipt)],
            &HashMap::new(),
            start + Duration::from_secs(10),
        );
        queue.update(
            vec![source(5, 29, &receipt)],
            &HashMap::from([(5, true)]),
            start + Duration::from_secs(11),
        );
        assert_eq!(queue.next_request(start + Duration::from_secs(30)), None);
    }

    #[test]
    fn hidden_views_do_not_spend_attempts_and_pip_is_deduplicated() {
        let views = Arc::new(ScreenViews::default());
        {
            let _pass = views.begin(true, None);
            views.note_rendered(5);
            views.note_rendered(5);
        }
        views.set_pip(Some(5));
        assert_eq!(views.snapshot(), HashMap::from([(5, true)]));
        {
            let _pass = views.begin(false, None);
            views.note_rendered(5);
        }
        assert_eq!(views.snapshot(), HashMap::from([(5, true)]));
        views.set_pip(None);
        assert!(views.snapshot().is_empty());
        let start = Instant::now();
        let receipt = Arc::default();
        let mut queue = ScreenRecovery::default();
        queue.update(vec![source(5, 29, &receipt)], &views.snapshot(), start);
        assert_eq!(queue.next_request(start + Duration::from_secs(60)), None);
        assert_eq!(queue.sources[&5].attempts, 0);
    }

    #[test]
    fn empty_render_pass_removes_previous_page() {
        let views = Arc::new(ScreenViews::default());
        {
            let _pass = views.begin(true, Some(5));
            views.note_rendered(5);
        }
        assert_eq!(views.snapshot(), HashMap::from([(5, true)]));
        {
            let _pass = views.begin(true, None);
        }
        assert!(views.snapshot().is_empty());
    }

    #[test]
    fn prioritizes_focus_and_limits_many_shares_to_four_per_second() {
        let start = Instant::now();
        let receipt = Arc::default();
        let sources = (1..=30).map(|id| source(id, id as u32, &receipt)).collect();
        let views = (1..=30).map(|id| (id, id == 30)).collect();
        let mut queue = ScreenRecovery::default();
        queue.update(sources, &views, start);
        assert_eq!(queue.next_request(start), Some(30));
        let mut sent = Vec::new();
        for millis in 0..10000 {
            let now = start + Duration::from_millis(millis);
            if let Some(publisher) = queue.next_request(now) {
                queue.sent(publisher, now);
                sent.push(millis);
                assert!(
                    sent.iter()
                        .filter(|time| **time > millis.saturating_sub(1000))
                        .count()
                        <= 4
                );
            }
        }
    }

    #[test]
    fn stop_start_reuses_track_but_requires_a_fresh_frame() {
        let start = Instant::now();
        let receipt = Arc::new(FrameReceipt::default());
        let mut queue = ScreenRecovery::default();
        let views = HashMap::from([(5, false)]);
        queue.update(vec![source(5, 29, &receipt)], &views, start);
        receipt.publish(start + GRACE, || ());
        queue.update(vec![], &views, start + Duration::from_secs(1));
        queue.update(
            vec![source(5, 29, &receipt)],
            &views,
            start + Duration::from_secs(2),
        );
        assert_eq!(queue.next_request(start + Duration::from_secs(3)), Some(29));
        receipt.publish(start + Duration::from_secs(3), || ());
        assert_eq!(queue.next_request(start + Duration::from_secs(4)), None);
    }

    #[test]
    fn replacement_and_publisher_reassignment_do_not_inherit_old_receipts() {
        let start = Instant::now();
        let old = Arc::new(FrameReceipt::default());
        let new = Arc::new(FrameReceipt::default());
        let views = HashMap::from([(5, true)]);
        let mut queue = ScreenRecovery::default();
        queue.update(vec![source(5, 29, &old)], &views, start);
        queue.sent(29, start);
        queue.update(
            vec![source(5, 31, &new)],
            &views,
            start + Duration::from_secs(1),
        );
        old.publish(start + Duration::from_secs(2), || ());
        assert_eq!(queue.next_request(start + Duration::from_secs(2)), Some(31));
    }

    #[test]
    fn rapid_restart_preserves_publisher_cooldown() {
        let start = Instant::now();
        let receipt = Arc::default();
        let views = HashMap::from([(5, true)]);
        let mut queue = ScreenRecovery::default();
        queue.update(vec![source(5, 29, &receipt)], &views, start);
        queue.sent(29, start);
        queue.update(vec![], &views, start + Duration::from_millis(100));
        queue.update(
            vec![source(5, 29, &receipt)],
            &views,
            start + Duration::from_millis(200),
        );
        assert_eq!(
            queue.next_request(start + Duration::from_millis(1499)),
            None
        );
        assert_eq!(
            queue.next_request(start + Duration::from_millis(1500)),
            Some(29)
        );
    }

    #[test]
    fn closed_stream_cannot_publish_or_remove_a_replacement_frame() {
        let receipt = FrameReceipt::default();
        receipt.publish(Instant::now(), || ());
        let mut removals = 0;
        receipt.close(|| removals += 1);
        receipt.close(|| removals += 1);
        assert_eq!(removals, 1);
        assert!(
            receipt
                .publish(Instant::now(), || panic!("stale publish"))
                .is_none()
        );
        assert!(receipt.last_frame().is_none());
    }

    #[test]
    fn recovery_errors_are_scoped_to_recent_requests() {
        let now = Instant::now();
        let mut queue = ScreenRecovery::default();
        assert!(!queue.is_recent_request_error("session_not_found", now));
        queue.sent(29, now);
        assert!(queue.is_recent_request_error("session_not_found", now + Duration::from_secs(1)));
        assert!(!queue.is_recent_request_error("invalid_token", now));
        assert!(!queue.is_recent_request_error("session_not_found", now + Duration::from_secs(6)));
    }
}
