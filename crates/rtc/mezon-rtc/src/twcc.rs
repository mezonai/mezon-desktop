use std::sync::{Arc, Mutex};

use rtc::rtcp::transport_feedbacks::transport_layer_cc::{
    PacketStatusChunk, SymbolTypeTcc, TransportLayerCc,
};
use webrtc::media_stream::track_local::{TrackLocal, TrackLocalEvent};

const LOSS_INCREASE_THRESHOLD: f64 = 0.02;
const LOSS_DECREASE_THRESHOLD: f64 = 0.10;
const DELAY_OVERUSE_THRESHOLD: f64 = 0.50;
const DELAY_BACKOFF: f64 = 0.85;
const INCREASE_FRACTION: f64 = 0.08;
const MIN_INCREASE_KBPS: f64 = 16.0;

pub struct SendBitrateController {
    min_kbps: f64,
    max_kbps: f64,
    target_kbps: f64,
}

impl SendBitrateController {
    pub fn new(min_kbps: u32, max_kbps: u32, start_kbps: u32) -> Self {
        let min = min_kbps as f64;
        let max = max_kbps as f64;
        let target = (start_kbps as f64).clamp(min, max);
        Self {
            min_kbps: min,
            max_kbps: max,
            target_kbps: target,
        }
    }

    pub fn on_transport_cc(&mut self, fb: &TransportLayerCc) {
        let (received, total) = Self::count_status(fb);
        if total == 0 {
            return;
        }
        let loss = (total - received) as f64 / total as f64;
        let delay_pressure = Self::large_delta_fraction(fb);

        if loss >= LOSS_DECREASE_THRESHOLD {
            self.target_kbps *= 1.0 - 0.5 * loss.min(1.0);
        } else if delay_pressure >= DELAY_OVERUSE_THRESHOLD {
            self.target_kbps *= DELAY_BACKOFF;
        } else if loss <= LOSS_INCREASE_THRESHOLD {
            let step = (INCREASE_FRACTION * self.target_kbps).max(MIN_INCREASE_KBPS);
            self.target_kbps += step;
        }

        self.target_kbps = self.target_kbps.clamp(self.min_kbps, self.max_kbps);
    }

    pub fn target_kbps(&self) -> u32 {
        self.target_kbps.round() as u32
    }

    fn count_status(fb: &TransportLayerCc) -> (usize, usize) {
        let total = fb.packet_status_count as usize;
        let mut received = 0usize;
        let mut counted = 0usize;
        for chunk in &fb.packet_chunks {
            if counted >= total {
                break;
            }
            match chunk {
                PacketStatusChunk::RunLengthChunk(c) => {
                    let n = (c.run_length as usize).min(total - counted);
                    if is_received(c.packet_status_symbol) {
                        received += n;
                    }
                    counted += n;
                }
                PacketStatusChunk::StatusVectorChunk(c) => {
                    for sym in &c.symbol_list {
                        if counted >= total {
                            break;
                        }
                        if is_received(*sym) {
                            received += 1;
                        }
                        counted += 1;
                    }
                }
            }
        }
        (received, total)
    }

    fn large_delta_fraction(fb: &TransportLayerCc) -> f64 {
        if fb.recv_deltas.is_empty() {
            return 0.0;
        }
        let large = fb
            .recv_deltas
            .iter()
            .filter(|d| d.type_tcc_packet == SymbolTypeTcc::PacketReceivedLargeDelta)
            .count();
        large as f64 / fb.recv_deltas.len() as f64
    }
}

fn is_received(sym: SymbolTypeTcc) -> bool {
    matches!(
        sym,
        SymbolTypeTcc::PacketReceivedSmallDelta
            | SymbolTypeTcc::PacketReceivedLargeDelta
            | SymbolTypeTcc::PacketReceivedWithoutDelta
    )
}

pub fn spawn_twcc_monitor(track: Arc<dyn TrackLocal>, controller: Arc<Mutex<SendBitrateController>>) {
    tokio::spawn(async move {
        while let Some(event) = track.poll().await {
            match event {
                TrackLocalEvent::OnRtcpPacket(packets) => {
                    for pkt in packets {
                        if let Some(tcc) = pkt.as_any().downcast_ref::<TransportLayerCc>()
                            && let Ok(mut ctrl) = controller.lock()
                        {
                            ctrl.on_transport_cc(tcc);
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::rtcp::transport_feedbacks::transport_layer_cc::{
        RecvDelta, RunLengthChunk, StatusChunkTypeTcc,
    };

    fn tcc_all_lost(n: u16) -> TransportLayerCc {
        TransportLayerCc {
            packet_status_count: n,
            packet_chunks: vec![PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                packet_status_symbol: SymbolTypeTcc::PacketNotReceived,
                run_length: n,
            })],
            recv_deltas: vec![],
            ..Default::default()
        }
    }

    fn tcc_all_received(n: u16) -> TransportLayerCc {
        TransportLayerCc {
            packet_status_count: n,
            packet_chunks: vec![PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                packet_status_symbol: SymbolTypeTcc::PacketReceivedSmallDelta,
                run_length: n,
            })],
            recv_deltas: (0..n)
                .map(|_| RecvDelta {
                    type_tcc_packet: SymbolTypeTcc::PacketReceivedSmallDelta,
                    delta: 250,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn tcc_all_received_large_delta(n: u16) -> TransportLayerCc {
        TransportLayerCc {
            packet_status_count: n,
            packet_chunks: vec![PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                packet_status_symbol: SymbolTypeTcc::PacketReceivedLargeDelta,
                run_length: n,
            })],
            recv_deltas: (0..n)
                .map(|_| RecvDelta {
                    type_tcc_packet: SymbolTypeTcc::PacketReceivedLargeDelta,
                    delta: 32_000,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn tcc_partial_loss(n: u16, lost: u16) -> TransportLayerCc {
        let received = n - lost;
        TransportLayerCc {
            packet_status_count: n,
            packet_chunks: vec![
                PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                    type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                    packet_status_symbol: SymbolTypeTcc::PacketReceivedSmallDelta,
                    run_length: received,
                }),
                PacketStatusChunk::RunLengthChunk(RunLengthChunk {
                    type_tcc: StatusChunkTypeTcc::RunLengthChunk,
                    packet_status_symbol: SymbolTypeTcc::PacketNotReceived,
                    run_length: lost,
                }),
            ],
            recv_deltas: (0..received)
                .map(|_| RecvDelta {
                    type_tcc_packet: SymbolTypeTcc::PacketReceivedSmallDelta,
                    delta: 250,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn sustained_loss_drives_target_to_min() {
        let mut c = SendBitrateController::new(100, 2500, 1500);
        let start = c.target_kbps();
        let fb = tcc_all_lost(200);
        c.on_transport_cc(&fb);
        assert!(
            c.target_kbps() < start,
            "one lossy report should reduce target, got {} (start {start})",
            c.target_kbps()
        );
        for _ in 0..50 {
            c.on_transport_cc(&fb);
        }
        assert_eq!(
            c.target_kbps(),
            100,
            "sustained total loss must converge to min_kbps"
        );
    }

    #[test]
    fn clean_delivery_climbs_to_max() {
        let mut c = SendBitrateController::new(100, 2500, 500);
        let start = c.target_kbps();
        let fb = tcc_all_received(200);
        c.on_transport_cc(&fb);
        assert!(
            c.target_kbps() > start,
            "one clean report should raise target, got {} (start {start})",
            c.target_kbps()
        );
        for _ in 0..200 {
            c.on_transport_cc(&fb);
        }
        assert_eq!(
            c.target_kbps(),
            2500,
            "sustained clean delivery must converge to max_kbps"
        );
    }

    #[test]
    fn delay_overuse_without_loss_backs_off() {
        let mut c = SendBitrateController::new(100, 2500, 1500);
        let start = c.target_kbps();
        c.on_transport_cc(&tcc_all_received_large_delta(120));
        let after = c.target_kbps();
        assert!(
            after < start,
            "delay overuse with no loss must reduce target, got {after} (start {start})"
        );
        let mut d = SendBitrateController::new(100, 2500, 1500);
        d.on_transport_cc(&tcc_all_lost(120));
        assert!(
            after > d.target_kbps(),
            "delay back-off ({after}) must be gentler than loss back-off ({})",
            d.target_kbps()
        );
    }

    #[test]
    fn moderate_loss_between_thresholds_holds_steady() {
        let mut c = SendBitrateController::new(100, 2500, 1200);
        let start = c.target_kbps();
        c.on_transport_cc(&tcc_partial_loss(100, 5));
        assert_eq!(
            c.target_kbps(),
            start,
            "loss in the dead-band must hold the estimate steady"
        );
    }

    #[test]
    fn heavier_loss_cuts_harder() {
        let mut light = SendBitrateController::new(100, 4000, 2000);
        let mut heavy = SendBitrateController::new(100, 4000, 2000);
        light.on_transport_cc(&tcc_partial_loss(100, 15));
        heavy.on_transport_cc(&tcc_partial_loss(100, 60));
        assert!(
            heavy.target_kbps() < light.target_kbps(),
            "heavier loss ({}) must cut below lighter loss ({})",
            heavy.target_kbps(),
            light.target_kbps()
        );
    }
}
