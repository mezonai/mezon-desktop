use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use ogg::writing::{PacketWriteEndInfo, PacketWriter};

use crate::AudioError;

const TARGET_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 960;
const MAX_PACKET_BYTES: usize = 4000;
const VOICE_BITRATE: i32 = 32_000;
const DEFAULT_LOOKAHEAD: i32 = 312;

pub struct VoiceRecording {
    pub bytes: Vec<u8>,
    pub duration_secs: f64,
}

pub struct VoiceEncoder {
    encoder: OpusEncoder,
    writer: PacketWriter<'static, Cursor<Vec<u8>>>,
    serial: u32,
    source_rate: u32,
    source_channels: usize,
    source_pcm: Vec<f32>,
    source_cursor: f64,
    frame: Vec<f32>,
    packet: Vec<u8>,
    preskip: u64,
    total_samples: u64,
    encoded_samples: u64,
}

impl VoiceEncoder {
    pub fn new(source_rate: u32, source_channels: u16) -> Result<Self, AudioError> {
        let mut encoder = OpusEncoder::new(TARGET_RATE as i32, 1)?;
        encoder.set_bitrate(VOICE_BITRATE)?;
        let preskip = encoder.lookahead().max(0) as u64;

        let serial = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
            .unwrap_or(1)
            | 1;

        let mut recorder = Self {
            encoder,
            writer: PacketWriter::new(Cursor::new(Vec::new())),
            serial,
            source_rate: source_rate.max(1),
            source_channels: (source_channels as usize).max(1),
            source_pcm: Vec::new(),
            source_cursor: 0.0,
            frame: Vec::with_capacity(FRAME_SAMPLES),
            packet: vec![0u8; MAX_PACKET_BYTES],
            preskip,
            total_samples: 0,
            encoded_samples: 0,
        };
        recorder.write_headers()?;
        Ok(recorder)
    }

    fn write_headers(&mut self) -> Result<(), AudioError> {
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1);
        head.push(1);
        head.extend_from_slice(&(self.preskip as u16).to_le_bytes());
        head.extend_from_slice(&self.source_rate.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        self.write_packet(head, PacketWriteEndInfo::EndPage, 0)?;

        let vendor = b"mezon-desktop";
        let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        self.write_packet(tags, PacketWriteEndInfo::EndPage, 0)
    }

    fn write_packet(
        &mut self,
        data: Vec<u8>,
        end: PacketWriteEndInfo,
        granule: u64,
    ) -> Result<(), AudioError> {
        self.writer
            .write_packet(data, self.serial, end, granule)
            .map_err(|e| AudioError::Encode(e.to_string()))
    }

    pub fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks(self.source_channels) {
            let sum: f32 = frame.iter().sum();
            self.source_pcm.push(sum / frame.len() as f32);
        }
        self.resample();
    }

    fn resample(&mut self) {
        if self.source_rate == TARGET_RATE {
            self.frame.append(&mut self.source_pcm);
            return;
        }
        let step = self.source_rate as f64 / TARGET_RATE as f64;
        while self.source_cursor + 1.0 < self.source_pcm.len() as f64 {
            let index = self.source_cursor as usize;
            let fraction = (self.source_cursor - index as f64) as f32;
            let sample =
                self.source_pcm[index] * (1.0 - fraction) + self.source_pcm[index + 1] * fraction;
            self.frame.push(sample);
            self.source_cursor += step;
        }
        let consumed = self.source_cursor as usize;
        if consumed > 0 {
            self.source_pcm.drain(..consumed);
            self.source_cursor -= consumed as f64;
        }
    }

    pub fn encode_ready_frames(&mut self) -> Result<(), AudioError> {
        while self.frame.len() >= FRAME_SAMPLES {
            let rest = self.frame.split_off(FRAME_SAMPLES);
            let frame = std::mem::replace(&mut self.frame, rest);
            self.total_samples += FRAME_SAMPLES as u64;
            self.encode_frame(&frame, false)?;
        }
        Ok(())
    }

    fn encode_frame(&mut self, samples: &[f32], last: bool) -> Result<(), AudioError> {
        let written = self.encoder.encode_float(samples, &mut self.packet)?;
        self.encoded_samples += FRAME_SAMPLES as u64;
        let granule = if last {
            self.preskip + self.total_samples
        } else {
            self.preskip + self.encoded_samples
        };
        let end = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        let packet = self.packet[..written].to_vec();
        self.write_packet(packet, end, granule)
    }

    pub fn finish(mut self) -> Result<VoiceRecording, AudioError> {
        self.encode_ready_frames()?;

        let tail = self.frame.len();
        if tail > 0 {
            self.total_samples += tail as u64;
        }
        if self.total_samples == 0 {
            return Err(AudioError::Encode("no audio was recorded".into()));
        }
        let mut last = std::mem::take(&mut self.frame);
        last.resize(FRAME_SAMPLES, 0.0);
        self.encode_frame(&last, true)?;

        let duration_secs = self.total_samples as f64 / TARGET_RATE as f64;
        let bytes = self.writer.into_inner().into_inner();
        Ok(VoiceRecording {
            bytes,
            duration_secs,
        })
    }
}

struct OpusEncoder {
    inner: *mut unsafe_libopus::OpusEncoder,
}

unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    fn new(sample_rate: i32, channels: i32) -> Result<Self, AudioError> {
        let mut error = 0i32;
        let inner = unsafe {
            unsafe_libopus::opus_encoder_create(
                sample_rate,
                channels,
                unsafe_libopus::OPUS_APPLICATION_VOIP,
                &mut error,
            )
        };
        if inner.is_null() || error != unsafe_libopus::OPUS_OK {
            return Err(AudioError::Encode(format!(
                "opus_encoder_create failed (error {error})"
            )));
        }
        Ok(Self { inner })
    }

    fn set_bitrate(&mut self, bitrate: i32) -> Result<(), AudioError> {
        let status = unsafe {
            unsafe_libopus::opus_encoder_ctl!(
                self.inner,
                unsafe_libopus::OPUS_SET_BITRATE_REQUEST,
                bitrate
            )
        };
        if status != unsafe_libopus::OPUS_OK {
            return Err(AudioError::Encode(format!(
                "opus set bitrate failed (error {status})"
            )));
        }
        Ok(())
    }

    fn lookahead(&mut self) -> i32 {
        let mut lookahead = 0i32;
        let status = unsafe {
            unsafe_libopus::opus_encoder_ctl!(
                self.inner,
                unsafe_libopus::OPUS_GET_LOOKAHEAD_REQUEST,
                &mut lookahead
            )
        };
        if status == unsafe_libopus::OPUS_OK {
            lookahead
        } else {
            DEFAULT_LOOKAHEAD
        }
    }

    fn encode_float(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize, AudioError> {
        let written = unsafe {
            unsafe_libopus::opus_encode_float(
                self.inner,
                pcm.as_ptr(),
                pcm.len() as i32,
                out.as_mut_ptr(),
                out.len() as i32,
            )
        };
        if written < 0 {
            return Err(AudioError::Encode(format!(
                "opus_encode_float failed (error {written})"
            )));
        }
        Ok(written as usize)
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_encoder_destroy(self.inner) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: u16, seconds: f64) -> Vec<f32> {
        let frames = (rate as f64 * seconds) as usize;
        let mut pcm = Vec::with_capacity(frames * channels as usize);
        for frame in 0..frames {
            let phase = frame as f64 / rate as f64 * 440.0 * std::f64::consts::TAU;
            let sample = (phase.sin() * 0.5) as f32;
            for _ in 0..channels {
                pcm.push(sample);
            }
        }
        pcm
    }

    fn record(rate: u32, channels: u16, seconds: f64) -> VoiceRecording {
        let mut encoder = VoiceEncoder::new(rate, channels).expect("encoder");
        for chunk in sine(rate, channels, seconds).chunks(1024) {
            encoder.push(chunk);
            encoder.encode_ready_frames().expect("encode");
        }
        encoder.finish().expect("finish")
    }

    #[test]
    fn encodes_ogg_opus_the_player_can_decode() {
        let recording = record(48_000, 1, 2.0);

        assert_eq!(&recording.bytes[..4], b"OggS");
        assert!((recording.duration_secs - 2.0).abs() < 0.05);

        let decoded = crate::decode_audio(recording.bytes).expect("round-trip decode");
        assert_eq!(decoded.sample_rate, TARGET_RATE);
        assert_eq!(decoded.channels, 1);
        assert!((decoded.duration_secs() - 2.0).abs() < 0.1);
    }

    #[test]
    fn resamples_and_downmixes_the_capture_device_format() {
        let recording = record(44_100, 2, 1.5);

        assert!((recording.duration_secs - 1.5).abs() < 0.05);
        let decoded = crate::decode_audio(recording.bytes).expect("round-trip decode");
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.sample_rate, TARGET_RATE);
        assert!((decoded.duration_secs() - 1.5).abs() < 0.1);
    }

    #[test]
    fn empty_recording_is_an_error() {
        let encoder = VoiceEncoder::new(48_000, 1).expect("encoder");
        assert!(encoder.finish().is_err());
    }
}
