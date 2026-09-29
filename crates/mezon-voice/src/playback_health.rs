use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REBINDS: u32 = 3;

pub(crate) struct PlaybackRecovery {
    pub(crate) attempt: u32,
    frames: Arc<AtomicU64>,
    observed: u64,
}

impl PlaybackRecovery {
    pub(crate) fn still_stalled(&self) -> bool {
        self.frames.load(Ordering::Relaxed) == self.observed
    }

    pub(crate) fn rebind(&self) -> bool {
        self.attempt <= MAX_REBINDS
    }
}

/// Require fresh inbound RTP on the same playback reader before escalating.
/// Silence, muted publishers and a replaced receiver must not cause reconnects.
#[derive(Default)]
pub(crate) struct AudioRecoveryProgress {
    previous: Option<(Arc<AtomicU64>, String, u64)>,
}

impl AudioRecoveryProgress {
    pub(crate) fn observe(
        &mut self,
        recovery: &PlaybackRecovery,
        stream_id: &str,
        packets: u64,
    ) -> bool {
        let receiving = self
            .previous
            .as_ref()
            .is_some_and(|(frames, id, previous)| {
                Arc::ptr_eq(frames, &recovery.frames) && id == stream_id && packets > *previous
            });
        self.previous = Some((recovery.frames.clone(), stream_id.to_owned(), packets));
        !recovery.rebind() && recovery.still_stalled() && receiving
    }
}

pub(super) struct PlaybackHealth {
    frames: Arc<AtomicU64>,
    observed: u64,
    last_progress: Instant,
    attempts: u32,
}

impl PlaybackHealth {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            frames: Arc::new(AtomicU64::new(0)),
            observed: 0,
            last_progress: now,
            attempts: 0,
        }
    }

    pub(super) fn frame_counter(&self) -> Arc<AtomicU64> {
        self.frames.clone()
    }

    pub(super) fn poll(&mut self, now: Instant) -> Option<PlaybackRecovery> {
        let frames = self.frames.load(Ordering::Relaxed);
        if frames != self.observed {
            self.observed = frames;
            self.last_progress = now;
            self.attempts = 0;
            return None;
        }
        if now.saturating_duration_since(self.last_progress) < FRAME_TIMEOUT {
            return None;
        }
        self.last_progress = now;
        self.attempts = self.attempts.saturating_add(1);
        Some(PlaybackRecovery {
            attempt: self.attempts,
            frames: self.frames.clone(),
            observed: self.observed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebinds_are_bounded_but_recovery_probes_continue() {
        let start = Instant::now();
        let mut health = PlaybackHealth::new(start);
        assert!(health.poll(start + FRAME_TIMEOUT / 2).is_none());
        for attempt in 1..=6 {
            let recovery = health.poll(start + FRAME_TIMEOUT * attempt).unwrap();
            assert_eq!(recovery.attempt, attempt);
            assert_eq!(recovery.rebind(), attempt <= MAX_REBINDS);
        }
    }

    #[test]
    fn resumed_playback_invalidates_queued_recovery_and_resets_attempts() {
        let start = Instant::now();
        let mut health = PlaybackHealth::new(start);
        let recovery = health.poll(start + FRAME_TIMEOUT).unwrap();
        health.frame_counter().fetch_add(1, Ordering::Relaxed);
        assert!(!recovery.still_stalled());
        assert!(health.poll(start + FRAME_TIMEOUT * 2).is_none());
        assert_eq!(health.poll(start + FRAME_TIMEOUT * 3).unwrap().attempt, 1);
    }

    #[test]
    fn stalled_playback_with_fresh_rtp_escalates_after_rebinds() {
        let start = Instant::now();
        let mut health = PlaybackHealth::new(start);
        let mut progress = AudioRecoveryProgress::default();
        for attempt in 1..=4 {
            let recovery = health.poll(start + FRAME_TIMEOUT * attempt).unwrap();
            assert_eq!(
                progress.observe(&recovery, "receiver-1", u64::from(attempt) * 100),
                attempt == 4,
            );
        }
    }

    #[test]
    fn muted_or_silent_publishers_do_not_restart_the_session() {
        let start = Instant::now();
        let mut health = PlaybackHealth::new(start);
        let mut progress = AudioRecoveryProgress::default();
        for attempt in 1..=10 {
            let recovery = health.poll(start + FRAME_TIMEOUT * attempt).unwrap();
            assert!(!progress.observe(&recovery, "receiver-1", 100));
        }
    }

    #[test]
    fn replaced_receivers_and_counter_resets_need_a_new_rtp_baseline() {
        let start = Instant::now();
        let mut health = PlaybackHealth::new(start);
        let mut progress = AudioRecoveryProgress::default();
        let mut recovery = health.poll(start + FRAME_TIMEOUT).unwrap();
        recovery.attempt = MAX_REBINDS + 1;
        assert!(!progress.observe(&recovery, "receiver-1", 100));
        assert!(!progress.observe(&recovery, "receiver-2", 200));
        assert!(!progress.observe(&recovery, "receiver-2", 0));

        let mut replacement = PlaybackHealth::new(start);
        let mut next = replacement.poll(start + FRAME_TIMEOUT).unwrap();
        next.attempt = MAX_REBINDS + 1;
        assert!(!progress.observe(&next, "receiver-2", 300));
        assert!(progress.observe(&next, "receiver-2", 400));
        replacement.frame_counter().fetch_add(1, Ordering::Relaxed);
        assert!(!progress.observe(&next, "receiver-2", 500));
    }
}
