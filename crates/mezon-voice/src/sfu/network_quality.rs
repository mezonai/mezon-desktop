use std::collections::HashMap;

use libwebrtc::stats::RtcStats;

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
