use std::time::{Duration, Instant};

pub(super) struct ScreenTier {
    pub fps_cap: f64,
    pub scale_down: f64,
    needs_kbps: u32,
}

pub(super) type ScreenProfile = [ScreenTier; 4];

pub(super) const TEXT_TIERS: ScreenProfile = [
    ScreenTier {
        fps_cap: 20.0,
        scale_down: 1.0,
        needs_kbps: 2_500,
    },
    ScreenTier {
        fps_cap: 12.0,
        scale_down: 1.0,
        needs_kbps: 1_200,
    },
    ScreenTier {
        fps_cap: 8.0,
        scale_down: 1.0,
        needs_kbps: 700,
    },
    ScreenTier {
        fps_cap: 5.0,
        scale_down: 1.5,
        needs_kbps: 0,
    },
];

pub(super) const VIDEO_TIERS: ScreenProfile = [
    ScreenTier {
        fps_cap: 30.0,
        scale_down: 1.0,
        needs_kbps: 2_500,
    },
    ScreenTier {
        fps_cap: 24.0,
        scale_down: 1.25,
        needs_kbps: 1_200,
    },
    ScreenTier {
        fps_cap: 20.0,
        scale_down: 1.5,
        needs_kbps: 700,
    },
    ScreenTier {
        fps_cap: 15.0,
        scale_down: 2.0,
        needs_kbps: 0,
    },
];

// Samples normally arrive every five seconds. Never bridge a long stats gap
// when deciding whether bandwidth has been consistently good or bad.
const MAX_SAMPLE_GAP: Duration = Duration::from_secs(12);
const CONGESTED_QUEUE_MS: f64 = 200.0;
const RECOVERED_QUEUE_MS: f64 = 80.0;
const REQUIRED_SAMPLES: u32 = 2;

#[derive(Clone, Copy)]
pub(super) struct ScreenStats {
    pub at: Instant,
    pub ssrc: u32,
    pub packets: u64,
    pub bytes: u64,
    pub frames: u32,
    pub send_delay_seconds: f64,
    pub pli: u32,
    pub target_kbps: u32,
    pub bwe_kbps: u32,
    pub other_target_kbps: u32,
}

#[derive(Clone, Copy)]
pub(super) struct ScreenSample {
    pub target_kbps: u32,
    pub bwe_kbps: u32,
    pub available_kbps: u32,
    pub sent_kbps: f64,
    pub encoded_fps: f64,
    pub queue_ms: f64,
    pub pli_delta: u32,
}

#[derive(Default)]
pub(super) struct ScreenAdaptation {
    previous: Option<ScreenStats>,
    low_samples: u32,
    recovery: Option<(usize, u32)>,
}

impl ScreenAdaptation {
    pub fn update(
        &mut self,
        current: usize,
        profile: &ScreenProfile,
        stats: ScreenStats,
    ) -> Option<(usize, ScreenSample)> {
        let sample = self.sample(stats);
        let Some(sample) = sample else {
            self.low_samples = 0;
            self.recovery = None;
            return None;
        };
        Some((self.resolve(current, profile, sample), sample))
    }

    fn sample(&mut self, stats: ScreenStats) -> Option<ScreenSample> {
        let previous = self.previous.replace(stats)?;
        let elapsed = stats.at.checked_duration_since(previous.at)?;
        if elapsed.is_zero()
            || elapsed > MAX_SAMPLE_GAP
            || stats.ssrc != previous.ssrc
            || stats.target_kbps == 0
            || !stats.send_delay_seconds.is_finite()
            || !previous.send_delay_seconds.is_finite()
            || stats.send_delay_seconds < previous.send_delay_seconds
        {
            return None;
        }
        let packets = stats.packets.checked_sub(previous.packets)?;
        if packets == 0 {
            return None;
        }
        let bytes = stats.bytes.checked_sub(previous.bytes)?;
        let frames = stats.frames.checked_sub(previous.frames)?;
        let pli_delta = stats.pli.checked_sub(previous.pli)?;
        Some(ScreenSample {
            target_kbps: stats.target_kbps,
            bwe_kbps: stats.bwe_kbps,
            // BWE covers the whole transport. A camera allocation must not be
            // mistaken for spare screen bandwidth during recovery.
            available_kbps: stats.bwe_kbps.saturating_sub(stats.other_target_kbps),
            sent_kbps: bytes as f64 * 8.0 / elapsed.as_secs_f64() / 1000.0,
            encoded_fps: f64::from(frames) / elapsed.as_secs_f64(),
            queue_ms: (stats.send_delay_seconds - previous.send_delay_seconds) * 1000.0
                / packets as f64,
            pli_delta,
        })
    }

    fn resolve(&mut self, current: usize, profile: &ScreenProfile, sample: ScreenSample) -> usize {
        let current = current.min(profile.len() - 1);
        if sample.target_kbps < profile[current].needs_kbps {
            self.recovery = None;
            self.low_samples += 1;
            let desired = profile
                .iter()
                .position(|tier| sample.target_kbps >= tier.needs_kbps)
                .unwrap_or(profile.len() - 1);
            if self.low_samples >= REQUIRED_SAMPLES {
                self.low_samples = 0;
                return desired;
            }
            // An overloaded sender needs prompt relief, but one keyframe burst
            // must not immediately force full-resolution text to the last tier.
            if sample.queue_ms >= CONGESTED_QUEUE_MS {
                return (current + 1).min(desired);
            }
            return current;
        }
        self.low_samples = 0;
        if current == 0 || sample.queue_ms > RECOVERED_QUEUE_MS {
            self.recovery = None;
            return current;
        }

        // The encoder target can remain low at a reduced FPS/resolution. Use
        // spare transport bandwidth too, but only once the send queue clears.
        let budget = sample.target_kbps.max(sample.available_kbps);
        let desired = profile
            .iter()
            .position(|tier| u64::from(budget) * 100 >= u64::from(tier.needs_kbps) * 120)
            .unwrap_or(profile.len() - 1);
        if desired >= current {
            self.recovery = None;
            return current;
        }
        let (candidate, count) = self.recovery.unwrap_or((desired, 0));
        // Recover only as far as both consecutive samples support.
        let candidate = candidate.max(desired);
        if count + 1 >= REQUIRED_SAMPLES {
            self.recovery = None;
            return candidate;
        }
        self.recovery = Some((candidate, count + 1));
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(target: u32, available: u32, queue_ms: f64) -> ScreenSample {
        ScreenSample {
            target_kbps: target,
            bwe_kbps: available,
            available_kbps: available,
            sent_kbps: 0.0,
            encoded_fps: 0.0,
            queue_ms,
            pli_delta: 0,
        }
    }

    #[test]
    fn transient_dip_does_not_reduce_resolution_or_fps() {
        let mut control = ScreenAdaptation::default();
        assert_eq!(control.resolve(0, &TEXT_TIERS, sample(521, 1079, 20.0)), 0);
        assert_eq!(control.resolve(0, &TEXT_TIERS, sample(2800, 3500, 20.0)), 0);
        assert_eq!(control.resolve(0, &TEXT_TIERS, sample(521, 1079, 20.0)), 0);
    }

    #[test]
    fn sustained_low_bandwidth_still_reaches_the_safe_tier() {
        let mut control = ScreenAdaptation::default();
        let low = sample(521, 1079, 20.0);
        assert_eq!(control.resolve(0, &TEXT_TIERS, low), 0);
        assert_eq!(control.resolve(0, &TEXT_TIERS, low), 3);
    }

    #[test]
    fn logged_queue_spike_reduces_only_one_tier_immediately() {
        let mut control = ScreenAdaptation::default();
        let spike = sample(521, 1079, 354.0);
        assert_eq!(control.resolve(0, &TEXT_TIERS, spike), 1);
        assert_eq!(TEXT_TIERS[1].scale_down, 1.0);
        assert_eq!(control.resolve(1, &TEXT_TIERS, spike), 3);
    }

    #[test]
    fn recovery_can_skip_tiers_after_two_clear_samples() {
        let mut control = ScreenAdaptation::default();
        let healthy = sample(650, 3500, 20.0);
        assert_eq!(control.resolve(3, &TEXT_TIERS, healthy), 3);
        assert_eq!(control.resolve(3, &TEXT_TIERS, healthy), 0);
    }

    #[test]
    fn a_busy_queue_breaks_the_recovery_streak() {
        let mut control = ScreenAdaptation::default();
        let healthy = sample(1600, 3500, 20.0);
        assert_eq!(control.resolve(2, &TEXT_TIERS, healthy), 2);
        assert_eq!(
            control.resolve(2, &TEXT_TIERS, sample(1600, 3500, 100.0)),
            2
        );
        assert_eq!(control.resolve(2, &TEXT_TIERS, healthy), 2);
        assert_eq!(control.resolve(2, &TEXT_TIERS, healthy), 0);
    }

    #[test]
    fn recovery_requires_headroom_in_both_samples() {
        let mut control = ScreenAdaptation::default();
        assert_eq!(control.resolve(3, &TEXT_TIERS, sample(650, 3500, 20.0)), 3);
        assert_eq!(control.resolve(3, &TEXT_TIERS, sample(650, 1500, 20.0)), 1);
        for _ in 0..4 {
            assert_eq!(control.resolve(1, &TEXT_TIERS, sample(2600, 2600, 20.0)), 1);
        }
    }

    fn stats(at: Instant) -> ScreenStats {
        ScreenStats {
            at,
            ssrc: 42,
            packets: 1000,
            bytes: 1_000_000,
            frames: 100,
            send_delay_seconds: 10.0,
            pli: 1,
            target_kbps: 2800,
            bwe_kbps: 3500,
            other_target_kbps: 1000,
        }
    }

    fn advance(mut stats: ScreenStats) -> ScreenStats {
        stats.at += Duration::from_secs(5);
        stats.packets += 100;
        stats.bytes += 100_000;
        stats.frames += 30;
        stats.send_delay_seconds += 30.0;
        stats.pli += 2;
        stats
    }

    #[test]
    fn stats_use_interval_counters_and_reserve_camera_bandwidth() {
        let mut control = ScreenAdaptation::default();
        let first = stats(Instant::now());
        assert!(control.update(0, &TEXT_TIERS, first).is_none());
        let (_, measured) = control.update(0, &TEXT_TIERS, advance(first)).unwrap();
        assert_eq!(measured.queue_ms, 300.0);
        assert_eq!(measured.sent_kbps, 160.0);
        assert_eq!(measured.encoded_fps, 6.0);
        assert_eq!(measured.pli_delta, 2);
        assert_eq!(measured.bwe_kbps, 3500);
        assert_eq!(measured.available_kbps, 2500);
    }

    #[test]
    fn camera_bandwidth_cannot_fund_a_screen_upgrade() {
        let mut control = ScreenAdaptation::default();
        let mut reading = stats(Instant::now());
        reading.target_kbps = 800;
        reading.bwe_kbps = 2000;
        reading.other_target_kbps = 1000;
        control.update(2, &VIDEO_TIERS, reading);
        for _ in 0..4 {
            reading = advance(reading);
            reading.send_delay_seconds = 10.0;
            assert_eq!(control.update(2, &VIDEO_TIERS, reading).unwrap().0, 2);
        }
    }

    #[test]
    fn missing_bwe_still_allows_recovery_from_encoder_target() {
        let mut control = ScreenAdaptation::default();
        let healthy = sample(1600, 0, 20.0);
        assert_eq!(control.resolve(2, &TEXT_TIERS, healthy), 2);
        assert_eq!(control.resolve(2, &TEXT_TIERS, healthy), 1);
    }

    #[test]
    fn reported_trace_avoids_the_transient_fps_cut_and_final_resolution_drop() {
        // Publisher log, 2026-09-22 07:45:51--07:46:26 UTC. queue_ms
        // is a lifetime average in that log, so reconstruct its total before
        // feeding the same counters through the interval sampler.
        let trace = [
            (276, 299, 33, 15.7, 1, 2868, 3500),
            (1033, 1149, 106, 13.5, 1, 3274, 3500),
            (2061, 2316, 176, 65.3, 3, 1257, 1608),
            (2894, 3272, 206, 103.0, 8, 2820, 3500),
            (3840, 4364, 233, 87.3, 10, 3321, 3500),
            (4612, 5254, 259, 90.8, 12, 2602, 3500),
            (5932, 6774, 317, 82.1, 15, 2862, 3500),
            (6697, 7636, 380, 113.1, 16, 521, 1079),
        ];
        let mut control = ScreenAdaptation::default();
        let start = Instant::now();
        let mut tier = 0;
        for (i, (packets, kb, frames, queue_ms, pli, target, bwe)) in trace.into_iter().enumerate()
        {
            let reading = ScreenStats {
                at: start + Duration::from_secs(i as u64 * 5),
                ssrc: 3994587156,
                packets,
                bytes: kb * 1000,
                frames,
                send_delay_seconds: queue_ms * packets as f64 / 1000.0,
                pli,
                target_kbps: target,
                bwe_kbps: bwe,
                other_target_kbps: 0,
            };
            if let Some((next, sample)) = control.update(tier, &TEXT_TIERS, reading) {
                tier = next;
                if i == 7 {
                    assert!(sample.queue_ms > 350.0);
                    assert_eq!(tier, 1);
                    assert_eq!(TEXT_TIERS[tier].scale_down, 1.0);
                } else {
                    assert_eq!(tier, 0);
                }
            }
        }
    }

    #[test]
    fn invalid_or_idle_stats_do_not_change_tiers_or_keep_streaks() {
        for case in 0..7 {
            let mut control = ScreenAdaptation::default();
            let mut first = stats(Instant::now());
            first.target_kbps = 500;
            control.update(0, &TEXT_TIERS, first);
            let mut low = advance(first);
            low.send_delay_seconds = 11.0;
            assert_eq!(control.update(0, &TEXT_TIERS, low).unwrap().0, 0);
            let mut invalid = advance(low);
            match case {
                0 => invalid.ssrc += 1,
                1 => invalid.packets = 1,
                2 => invalid.at += Duration::from_secs(30),
                3 => invalid.packets = low.packets,
                4 => invalid.target_kbps = 0,
                5 => invalid.send_delay_seconds = f64::NAN,
                _ => invalid.frames = 0,
            }
            assert!(control.update(0, &TEXT_TIERS, invalid).is_none());
            assert_eq!(control.low_samples, 0);
            assert!(control.recovery.is_none());
        }
    }

    #[test]
    fn profiles_trade_resolution_and_motion_differently() {
        for profile in [&TEXT_TIERS, &VIDEO_TIERS] {
            for pair in profile.windows(2) {
                assert!(pair[0].needs_kbps > pair[1].needs_kbps);
                assert!(pair[0].fps_cap >= pair[1].fps_cap);
                assert!(pair[0].scale_down <= pair[1].scale_down);
            }
        }
        assert_eq!(TEXT_TIERS[2].scale_down, 1.0);
        assert!(VIDEO_TIERS[2].scale_down > 1.0);
        assert!(VIDEO_TIERS.iter().all(|tier| tier.fps_cap >= 15.0));
    }
}
