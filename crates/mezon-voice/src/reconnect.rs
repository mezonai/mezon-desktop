use std::collections::VecDeque;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

/// Correlate a self peer update with the mute acknowledgement. A later unmute
/// supersedes the earlier update; repeated mute updates must not postpone it.
fn update_pending_self_mute(
    deadline: &mut Option<Instant>,
    remote_muted: bool,
    local_muted: bool,
    now: Instant,
) {
    if remote_muted && !local_muted {
        deadline.get_or_insert(now + Duration::from_millis(300));
    } else {
        *deadline = None;
    }
}

struct PendingMute {
    muted: bool,
    revision: u64,
    sent_at: Instant,
}

/// State for one WebSocket session. `peer_updated` may precede a local command's
/// `mute_changed` reply by seconds, so silence from the server is not moderation.
#[derive(Default)]
pub(crate) struct MuteSync {
    pending: VecDeque<PendingMute>,
    forced_mute_deadline: Option<Instant>,
}

impl MuteSync {
    pub(crate) fn sent(&mut self, muted: bool, revision: u64, now: Instant) {
        self.cancel_inference();
        self.pending.push_back(PendingMute {
            muted,
            revision,
            sent_at: now,
        });
    }

    pub(crate) fn acknowledge(&mut self, muted: bool) -> Option<u64> {
        // No wire revision exists yet. Match ordered replies to the ordered
        // writes on this socket; a mismatched/unsolicited ACK proves nothing.
        if self
            .pending
            .front()
            .is_some_and(|request| request.muted == muted)
        {
            let revision = self.pending.pop_front().map(|request| request.revision);
            self.cancel_inference();
            revision
        } else {
            None
        }
    }

    pub(crate) fn observe_self(&mut self, remote_muted: bool, local_muted: bool, now: Instant) {
        if self.pending.is_empty() {
            update_pending_self_mute(
                &mut self.forced_mute_deadline,
                remote_muted,
                local_muted,
                now,
            );
        } else {
            self.cancel_inference();
        }
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.forced_mute_deadline
    }

    pub(crate) fn cancel_inference(&mut self) {
        self.forced_mute_deadline = None;
    }

    pub(crate) fn take_forced_mute(&mut self, now: Instant) -> bool {
        if self.pending.is_empty() && self.forced_mute_deadline.is_some_and(|at| now >= at) {
            self.cancel_inference();
            true
        } else {
            false
        }
    }

    pub(crate) fn timed_out_revision(&self, now: Instant) -> Option<u64> {
        self.pending
            .front()
            .filter(|request| {
                now.saturating_duration_since(request.sent_at) >= Duration::from_secs(10)
            })
            .map(|request| request.revision)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfuCloseAction {
    Stop,
    RefreshToken,
    ResetTransport,
    Retry,
}

pub fn sfu_close_action(code: Option<u16>) -> SfuCloseAction {
    match code {
        Some(1000 | 4006 | 4011 | 4012) => SfuCloseAction::Stop,
        Some(4003 | 4004 | 4005) => SfuCloseAction::RefreshToken,
        Some(4013) => SfuCloseAction::ResetTransport,
        _ => SfuCloseAction::Retry,
    }
}

/// Zero-based attempts, equal jitter: 0.5–1 / 1–2 / 2–4 / 4–8 seconds.
pub fn sfu_reconnect_delay(attempt: u32) -> Duration {
    // RandomState uses independently seeded hashes; no shared reconnect cadence.
    reconnect_delay_with_jitter(attempt, RandomState::new().build_hasher().finish())
}

fn reconnect_delay_with_jitter(attempt: u32, jitter: u64) -> Duration {
    let ceiling = 1_000u64 << attempt.min(3);
    let half = ceiling / 2;
    Duration::from_millis(half + jitter % (half + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmute_ack_delayed_like_the_reported_log_does_not_close_the_mic() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        sync.sent(false, 2, start); // 12:53:54.736
        sync.observe_self(true, false, start + Duration::from_millis(2100));
        // The old 300 ms inference had already closed the mic at this point.
        assert!(!sync.take_forced_mute(start + Duration::from_millis(2409)));
        assert!(sync.deadline().is_none());
        assert_eq!(sync.acknowledge(false), Some(2)); // 12:53:57.458
        assert_eq!(sync.pending_count(), 0);
        assert!(!sync.take_forced_mute(start + Duration::from_millis(2722)));
        assert!(
            sync.timed_out_revision(start + Duration::from_secs(20))
                .is_none()
        );
    }

    #[test]
    fn an_old_join_mute_ack_cannot_complete_the_new_unmute_request() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        sync.sent(true, 1, start);
        sync.sent(false, 2, start + Duration::from_millis(50));
        assert_eq!(sync.acknowledge(true), Some(1));
        assert_eq!(sync.pending_count(), 1);
        sync.observe_self(true, false, start + Duration::from_secs(1));
        assert!(!sync.take_forced_mute(start + Duration::from_secs(2)));
        assert_eq!(sync.acknowledge(false), Some(2));
        assert!(sync.deadline().is_none());
    }

    #[test]
    fn mute_unmute_mute_replies_are_correlated_in_order() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        for (revision, muted) in [(1, true), (2, false), (3, true)] {
            sync.sent(muted, revision, start);
        }
        assert_eq!(sync.acknowledge(true), Some(1));
        assert_eq!(sync.acknowledge(true), None); // Does not consume revision 3.
        assert_eq!(sync.pending_count(), 2);
        assert_eq!(sync.acknowledge(false), Some(2));
        assert_eq!(sync.acknowledge(true), Some(3));
        assert_eq!(sync.pending_count(), 0);
    }

    #[test]
    fn lost_ack_requests_resynchronization_instead_of_inferred_moderation() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        sync.sent(false, 2, start);
        sync.observe_self(true, false, start + Duration::from_secs(1));
        assert_eq!(sync.acknowledge(true), None);
        assert!(
            sync.timed_out_revision(start + Duration::from_secs(9))
                .is_none()
        );
        assert_eq!(
            sync.timed_out_revision(start + Duration::from_secs(10)),
            Some(2)
        );
        assert!(!sync.take_forced_mute(start + Duration::from_secs(10)));
        // Recovery begins a new WebSocket with its own empty correlation state.
        let mut next = MuteSync::default();
        assert_eq!(next.acknowledge(false), None);
        assert_eq!(next.pending_count(), 0);
    }

    #[test]
    fn moderation_after_the_local_ack_still_closes_the_mic() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        sync.sent(false, 2, start);
        assert_eq!(sync.acknowledge(false), Some(2));
        sync.observe_self(true, false, start + Duration::from_secs(1));
        assert!(!sync.take_forced_mute(start + Duration::from_millis(1299)));
        assert!(sync.take_forced_mute(start + Duration::from_millis(1300)));
        assert!(!sync.take_forced_mute(start + Duration::from_secs(2)));
    }

    #[test]
    fn new_local_request_cancels_an_older_moderation_inference() {
        let start = Instant::now();
        let mut sync = MuteSync::default();
        sync.observe_self(true, false, start);
        sync.sent(false, 2, start + Duration::from_millis(100));
        assert!(sync.deadline().is_none());
        assert!(!sync.take_forced_mute(start + Duration::from_secs(1)));
    }

    #[test]
    fn a_newer_self_unmute_cancels_the_pending_moderator_inference() {
        let now = Instant::now();
        let mut deadline = None;
        update_pending_self_mute(&mut deadline, true, false, now);
        assert!(deadline.is_some());
        update_pending_self_mute(
            &mut deadline,
            false,
            false,
            now + Duration::from_millis(100),
        );
        assert!(deadline.is_none());
    }

    #[test]
    fn repeated_mute_updates_preserve_the_original_moderation_deadline() {
        let now = Instant::now();
        let mut deadline = None;
        update_pending_self_mute(&mut deadline, true, false, now);
        let original = deadline;
        update_pending_self_mute(&mut deadline, true, false, now + Duration::from_millis(200));
        assert_eq!(deadline, original);
        assert!(deadline.unwrap() <= now + Duration::from_millis(300));
        update_pending_self_mute(&mut deadline, true, true, now);
        assert!(deadline.is_none());
    }

    #[test]
    fn close_contract() {
        for code in [1000, 4006, 4011, 4012] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::Stop);
        }
        for code in [4003, 4004, 4005] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::RefreshToken);
        }
        assert_eq!(sfu_close_action(Some(4013)), SfuCloseAction::ResetTransport);
        for code in [1001, 1006, 1011, 4001, 4008, 4999] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::Retry);
        }
        assert_eq!(sfu_close_action(None), SfuCloseAction::Retry);
    }
    #[test]
    fn backoff_is_exponential_bounded_and_jittered() {
        for attempt in 0..32 {
            let cap = 1_000u64 << attempt.min(3);
            assert_eq!(
                reconnect_delay_with_jitter(attempt, 0).as_millis(),
                u128::from(cap / 2)
            );
            assert_eq!(
                reconnect_delay_with_jitter(attempt, cap / 2).as_millis(),
                u128::from(cap)
            );
            for _ in 0..20 {
                assert!(
                    (u128::from(cap / 2)..=u128::from(cap))
                        .contains(&sfu_reconnect_delay(attempt).as_millis())
                );
            }
        }
    }
}
