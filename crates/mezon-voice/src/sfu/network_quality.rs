use std::collections::HashMap;

use libwebrtc::stats::{QualityLimitationReason, RtcStats};

use super::mid::MID_CAMERA;

const UPLINK_TARGET_RATIO: f64 = 0.8;
const UPLINK_LIMITED_SAMPLES: u32 = 2;
const UPLINK_RECOVERY_SAMPLES: u32 = 6;
const UPLINK_MAX_RECOVERY_SAMPLES: u32 = 24;
const UPLINK_RELAPSE_SAMPLES: u32 = 12;

pub(super) struct CameraUplinkQuality {
    limited: bool,
    limited_samples: u32,
    clean_samples: u32,
    recovery_samples: u32,
    samples_since_recovery: u32,
    frames_encoded: Option<u32>,
}

impl Default for CameraUplinkQuality {
    fn default() -> Self {
        Self {
            limited: false,
            limited_samples: 0,
            clean_samples: 0,
            recovery_samples: UPLINK_RECOVERY_SAMPLES,
            samples_since_recovery: u32::MAX,
            frames_encoded: None,
        }
    }
}

impl CameraUplinkQuality {
    pub(super) fn update(&mut self, stats: &[RtcStats], max_bitrate: u64) -> bool {
        let camera = stats
            .iter()
            .filter_map(|stat| match stat {
                RtcStats::OutboundRtp(out)
                    if out.stream.kind == "video" && out.outbound.mid == MID_CAMERA =>
                {
                    Some(&out.outbound)
                }
                _ => None,
            })
            .max_by_key(|outbound| outbound.frames_encoded);
        self.update_sample(camera.map(|outbound| {
            let limited = outbound.quality_limitation_reason == QualityLimitationReason::Bandwidth
                && outbound.target_bitrate < max_bitrate as f64 * UPLINK_TARGET_RATIO;
            (outbound.frames_encoded, limited)
        }))
    }

    fn update_sample(&mut self, sample: Option<(u32, bool)>) -> bool {
        let previous_frames = self.frames_encoded;
        self.frames_encoded = sample.map(|(frames, _)| frames);
        let sample_limited = match (sample, previous_frames) {
            (Some((frames, limited)), Some(previous)) if frames > previous => limited,
            _ => {
                self.limited = false;
                self.limited_samples = 0;
                self.clean_samples = 0;
                return false;
            }
        };
        if sample_limited {
            self.limited_samples = self.limited_samples.saturating_add(1);
            self.clean_samples = 0;
        } else {
            self.limited_samples = 0;
            self.clean_samples = self.clean_samples.saturating_add(1);
        }
        if self.limited {
            if self.clean_samples >= self.recovery_samples {
                self.limited = false;
                self.samples_since_recovery = 0;
            }
            return self.limited;
        }
        self.samples_since_recovery = self.samples_since_recovery.saturating_add(1);
        if self.limited_samples >= UPLINK_LIMITED_SAMPLES {
            self.limited = true;
            self.recovery_samples = if self.samples_since_recovery <= UPLINK_RELAPSE_SAMPLES {
                (self.recovery_samples * 2).min(UPLINK_MAX_RECOVERY_SAMPLES)
            } else {
                UPLINK_RECOVERY_SAMPLES
            };
        }
        self.limited
    }
}

const WARNING_LOSS_RATIO: f64 = 0.10;
const SEVERE_LOSS_RATIO: f64 = 0.20;
const RECOVERY_LOSS_RATIO: f64 = 0.05;
const MIN_PACKETS: u64 = 50;
const WARNING_SAMPLES: u32 = 2;
const CLEAR_SAMPLES: u32 = 2;

struct StreamLoss {
    upload: bool,
    packets: u64,
    lost: i64,
}

#[derive(Default)]
struct LossWindow {
    expected: u64,
    lost: u64,
}

impl LossWindow {
    fn ratio(&self) -> Option<f64> {
        (self.expected >= MIN_PACKETS).then(|| self.lost as f64 / self.expected as f64)
    }
}

#[derive(Default)]
pub(super) struct NetworkQuality {
    streams: HashMap<String, StreamLoss>,
    weak: bool,
    bad_samples: u32,
    clean_samples: u32,
}

impl NetworkQuality {
    pub(super) fn update(&mut self, stats: &[RtcStats]) -> bool {
        let streams = loss_streams(stats);
        let mut received = LossWindow::default();
        let mut sent = LossWindow::default();
        for (id, stream) in &streams {
            let Some(previous) = self.streams.get(id) else {
                continue;
            };
            let lost = (stream.lost - previous.lost).max(0) as u64;
            let packets = stream.packets.saturating_sub(previous.packets);
            if stream.upload {
                sent.lost += lost;
                sent.expected += packets;
            } else {
                received.lost += lost;
                received.expected += packets + lost;
            }
        }
        self.streams = streams;
        self.update_loss(received, sent)
    }

    fn update_loss(&mut self, received: LossWindow, sent: LossWindow) -> bool {
        let ratio = received
            .ratio()
            .into_iter()
            .chain(sent.ratio())
            .reduce(f64::max);
        let Some(ratio) = ratio else {
            // Silence or a new stream is not evidence that the network recovered.
            self.bad_samples = 0;
            self.clean_samples = 0;
            return self.weak;
        };
        self.bad_samples = if ratio >= WARNING_LOSS_RATIO {
            self.bad_samples.saturating_add(1)
        } else {
            0
        };
        self.clean_samples = if ratio < RECOVERY_LOSS_RATIO {
            self.clean_samples.saturating_add(1)
        } else {
            0
        };
        if ratio >= SEVERE_LOSS_RATIO || self.bad_samples >= WARNING_SAMPLES {
            self.weak = true;
        } else if self.clean_samples >= CLEAR_SAMPLES {
            self.weak = false;
        }
        self.weak
    }
}

fn loss_streams(stats: &[RtcStats]) -> HashMap<String, StreamLoss> {
    let packets_sent: HashMap<&str, u64> = stats
        .iter()
        .filter_map(|stat| match stat {
            RtcStats::OutboundRtp(out) if out.stream.kind == "audio" => {
                Some((out.rtc.id.as_str(), out.sent.packets_sent))
            }
            _ => None,
        })
        .collect();
    stats
        .iter()
        .filter_map(|stat| match stat {
            RtcStats::InboundRtp(inbound) if inbound.stream.kind == "audio" => Some((
                inbound.rtc.id.clone(),
                StreamLoss {
                    upload: false,
                    packets: inbound.received.packets_received,
                    lost: inbound.received.packets_lost,
                },
            )),
            RtcStats::RemoteInboundRtp(remote) => packets_sent
                .get(remote.remote_inbound.local_id.as_str())
                .map(|&packets| {
                    (
                        remote.rtc.id.clone(),
                        StreamLoss {
                            upload: true,
                            packets,
                            lost: remote.received.packets_lost,
                        },
                    )
                }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(quality: &mut CameraUplinkQuality, frames: &mut u32, limited: &[bool]) -> Vec<bool> {
        limited
            .iter()
            .map(|&limited| {
                *frames += 100;
                quality.update_sample(Some((*frames, limited)))
            })
            .collect()
    }

    #[test]
    fn camera_uplink_turns_on_after_two_limited_samples_and_backs_off_on_relapse() {
        let mut quality = CameraUplinkQuality::default();
        let mut frames = 0;
        assert_eq!(
            feed(&mut quality, &mut frames, &[false, true, true]),
            [false, false, true]
        );
        assert_eq!(
            feed(&mut quality, &mut frames, &[false; 6]),
            [true, true, true, true, true, false]
        );
        assert_eq!(
            feed(&mut quality, &mut frames, &[true, true]),
            [false, true]
        );
        let relapse = feed(&mut quality, &mut frames, &[false; 12]);
        assert!(relapse[..11].iter().all(|&limited| limited));
        assert!(!relapse[11]);
    }

    #[test]
    fn camera_uplink_resets_when_the_camera_stops_encoding() {
        let mut quality = CameraUplinkQuality::default();
        let mut frames = 0;
        feed(&mut quality, &mut frames, &[false, true, true]);
        assert!(!quality.update_sample(Some((frames, true))));
        assert!(!quality.update_sample(None));
    }
}
