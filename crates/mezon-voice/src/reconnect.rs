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
