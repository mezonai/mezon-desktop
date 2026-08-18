use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use rtc::rtp::codec::opus::OpusPacket;
use rtc::rtp::codec::vp8::Vp8Packet;
use rtc::rtp::codec::vp9::Vp9Packet;
use rtc::rtp::packetizer::Depacketizer;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};

use mezon_codec::{EncodedFrame, VpxCodec};

pub fn spawn_opus_receiver(track: Arc<dyn TrackRemote>) -> flume::Receiver<Vec<u8>> {
    let (tx, rx) = flume::unbounded::<Vec<u8>>();
    tokio::spawn(async move {
        let mut depacketizer = OpusPacket;
        while let Some(event) = track.poll().await {
            if let TrackRemoteEvent::OnRtpPacket(pkt) = event
                && let Ok(payload) = depacketizer.depacketize(&pkt.payload)
                && !payload.is_empty()
                && tx.send(payload.to_vec()).is_err()
            {
                break;
            }
        }
    });
    rx
}

pub fn spawn_video_receiver(
    track: Arc<dyn TrackRemote>,
    codec: VpxCodec,
) -> flume::Receiver<EncodedFrame> {
    let (tx, rx) = flume::unbounded::<EncodedFrame>();
    tokio::spawn(async move {
        let mut reassembler = FrameReassembler::new(codec);
        while let Some(event) = track.poll().await {
            let TrackRemoteEvent::OnRtpPacket(pkt) = event else {
                continue;
            };
            for frame in reassembler.push(&pkt) {
                if tx.send(frame).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

const MAX_BUFFERED_FRAGMENTS: usize = 512;

struct Fragment {
    timestamp: u32,
    starts_frame: bool,
    ends_frame: bool,
    payload: Vec<u8>,
}

struct FrameReassembler {
    codec: VpxCodec,
    fragments: BTreeMap<u64, Fragment>,
    highest_seq: Option<u16>,
    rollovers: u64,
}

impl FrameReassembler {
    fn new(codec: VpxCodec) -> Self {
        Self {
            codec,
            fragments: BTreeMap::new(),
            highest_seq: None,
            rollovers: 0,
        }
    }

    fn push(&mut self, pkt: &rtc::rtp::Packet) -> Vec<EncodedFrame> {
        let Some(fragment) = self.parse(pkt) else {
            return Vec::new();
        };
        let key = self.extended_seq(pkt.header.sequence_number);
        self.fragments.insert(key, fragment);

        while self.fragments.len() > MAX_BUFFERED_FRAGMENTS {
            let Some(&oldest) = self.fragments.keys().next() else {
                break;
            };
            self.fragments.remove(&oldest);
        }

        let mut frames = Vec::new();
        while let Some(range) = self.next_complete_frame() {
            if let Some(frame) = self.take_frame(range) {
                frames.push(frame);
            }
        }
        frames
    }

    fn parse(&self, pkt: &rtc::rtp::Packet) -> Option<Fragment> {
        let (payload, starts_frame, ends_frame) = match self.codec {
            VpxCodec::Vp8 => {
                let mut descriptor = Vp8Packet::default();
                let payload = descriptor.depacketize(&pkt.payload).ok()?;
                (
                    payload,
                    descriptor.s == 1 && descriptor.pid == 0,
                    pkt.header.marker,
                )
            }
            VpxCodec::Vp9 => {
                let mut descriptor = Vp9Packet::default();
                let payload = descriptor.depacketize(&pkt.payload).ok()?;
                (payload, descriptor.b, descriptor.e)
            }
        };
        Some(Fragment {
            timestamp: pkt.header.timestamp,
            starts_frame,
            ends_frame,
            payload: payload.to_vec(),
        })
    }

    fn extended_seq(&mut self, seq: u16) -> u64 {
        let Some(highest) = self.highest_seq else {
            self.highest_seq = Some(seq);
            return seq as u64;
        };
        if seq.wrapping_sub(highest) < u16::MAX / 2 {
            if seq < highest {
                self.rollovers += 1;
            }
            self.highest_seq = Some(seq);
            (self.rollovers << 16) | seq as u64
        } else {
            let cycle = if seq > highest {
                self.rollovers.saturating_sub(1)
            } else {
                self.rollovers
            };
            (cycle << 16) | seq as u64
        }
    }

    fn next_complete_frame(&self) -> Option<RangeInclusive<u64>> {
        let mut run: Option<(u64, u32, u64)> = None;
        for (&key, fragment) in &self.fragments {
            run = match run {
                Some((first, timestamp, expected))
                    if key == expected && fragment.timestamp == timestamp =>
                {
                    Some((first, timestamp, key + 1))
                }
                _ if fragment.starts_frame => Some((key, fragment.timestamp, key + 1)),
                _ => None,
            };
            if let Some((first, _, _)) = run
                && fragment.ends_frame
            {
                return Some(first..=key);
            }
        }
        None
    }

    fn take_frame(&mut self, range: RangeInclusive<u64>) -> Option<EncodedFrame> {
        let last = *range.end();
        let mut data = Vec::new();
        for key in range {
            if let Some(fragment) = self.fragments.remove(&key) {
                data.extend_from_slice(&fragment.payload);
            }
        }
        self.fragments.retain(|&key, _| key > last);
        if data.is_empty() {
            return None;
        }
        let is_keyframe = match self.codec {
            VpxCodec::Vp8 => vp8_is_keyframe(&data),
            VpxCodec::Vp9 => vp9_is_keyframe(&data),
        };
        Some(EncodedFrame {
            data,
            is_keyframe,
            spatial_layer: 0,
            temporal_layer: 0,
        })
    }
}

fn vp8_is_keyframe(data: &[u8]) -> bool {
    !data.is_empty() && (data[0] & 0x01) == 0
}

fn vp9_is_keyframe(data: &[u8]) -> bool {
    let mut reader = BitReader::new(data);
    if reader.read_bits(2) != Some(0b10) {
        return false;
    }
    let (Some(low), Some(high)) = (reader.read_bits(1), reader.read_bits(1)) else {
        return false;
    };
    let profile = (high << 1) | low;
    if profile == 3 && reader.read_bits(1).is_none() {
        return false;
    }
    match reader.read_bits(1) {
        Some(1) => false,
        Some(0) => reader.read_bits(1) == Some(0),
        _ => false,
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self.data.get(self.bit_pos / 8)?;
            let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
            value = (value << 1) | bit as u32;
            self.bit_pos += 1;
        }
        Some(value)
    }
}
