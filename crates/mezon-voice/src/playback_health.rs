use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REBINDS: u32 = 3;

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

    pub(super) fn poll(&mut self, now: Instant) -> Option<u32> {
        let frames = self.frames.load(Ordering::Relaxed);
        if frames != self.observed {
            self.observed = frames;
            self.last_progress = now;
            self.attempts = 0;
            return None;
        }
        if self.attempts >= MAX_REBINDS
            || now.saturating_duration_since(self.last_progress) < FRAME_TIMEOUT
        {
            return None;
        }
        self.last_progress = now;
        self.attempts += 1;
        Some(self.attempts)
    }
}
