use crate::{AudioFrame, CodecError};

const SAMPLE_RATE: i32 = 48_000;
const MAX_PACKET: usize = 4000;
const MAX_FRAME: usize = 5760;

pub struct OpusEncoder {
    inner: *mut unsafe_libopus::OpusEncoder,
    channels: usize,
    scratch: Vec<u8>,
}

unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    pub fn new(channels: u8, bitrate_bps: i32) -> Result<Self, CodecError> {
        let ch = channels.clamp(1, 2) as i32;
        let mut error = 0i32;
        let inner = unsafe {
            unsafe_libopus::opus_encoder_create(
                SAMPLE_RATE,
                ch,
                unsafe_libopus::OPUS_APPLICATION_VOIP,
                &mut error,
            )
        };
        if inner.is_null() || error != unsafe_libopus::OPUS_OK {
            return Err(CodecError::Init(format!(
                "opus_encoder_create failed (error {error})"
            )));
        }
        let mut enc = Self {
            inner,
            channels: ch as usize,
            scratch: vec![0u8; MAX_PACKET],
        };
        enc.set_bitrate(bitrate_bps)?;
        Ok(enc)
    }

    pub fn set_bitrate(&mut self, bps: i32) -> Result<(), CodecError> {
        let status = unsafe {
            unsafe_libopus::opus_encoder_ctl!(
                self.inner,
                unsafe_libopus::OPUS_SET_BITRATE_REQUEST,
                bps
            )
        };
        if status != unsafe_libopus::OPUS_OK {
            return Err(CodecError::Init(format!(
                "opus set bitrate failed (error {status})"
            )));
        }
        Ok(())
    }

    pub fn encode(&mut self, frame: AudioFrame) -> Result<Vec<u8>, CodecError> {
        let ch = frame.channels.clamp(1, 2) as usize;
        if ch != self.channels {
            return Err(CodecError::InvalidFrame(format!(
                "channel mismatch: encoder={}, frame={ch}",
                self.channels
            )));
        }
        if frame.samples.is_empty() || !frame.samples.len().is_multiple_of(ch) {
            return Err(CodecError::InvalidFrame(
                "sample count must be a non-zero multiple of channels".into(),
            ));
        }
        let per_channel = frame.samples.len() / ch;
        let written = unsafe {
            unsafe_libopus::opus_encode_float(
                self.inner,
                frame.samples.as_ptr(),
                per_channel as i32,
                self.scratch.as_mut_ptr(),
                self.scratch.len() as i32,
            )
        };
        if written < 0 {
            return Err(CodecError::Encode(format!(
                "opus_encode_float failed (error {written})"
            )));
        }
        Ok(self.scratch[..written as usize].to_vec())
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_encoder_destroy(self.inner) };
    }
}

pub struct OpusDecoder {
    inner: *mut unsafe_libopus::OpusDecoder,
    channels: usize,
    scratch: Vec<f32>,
}

unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new(channels: u8) -> Result<Self, CodecError> {
        let ch = channels.clamp(1, 2) as i32;
        let mut error = 0i32;
        let inner = unsafe { unsafe_libopus::opus_decoder_create(SAMPLE_RATE, ch, &mut error) };
        if inner.is_null() || error != unsafe_libopus::OPUS_OK {
            return Err(CodecError::Init(format!(
                "opus_decoder_create failed (error {error})"
            )));
        }
        Ok(Self {
            inner,
            channels: ch as usize,
            scratch: vec![0.0f32; MAX_FRAME * ch as usize],
        })
    }

    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>, CodecError> {
        let decoded = unsafe {
            unsafe_libopus::opus_decode_float(
                self.inner,
                packet.as_ptr(),
                packet.len() as i32,
                self.scratch.as_mut_ptr(),
                MAX_FRAME as i32,
                0,
            )
        };
        if decoded < 0 {
            return Err(CodecError::Decode(format!(
                "opus_decode_float failed (error {decoded})"
            )));
        }
        let total = decoded as usize * self.channels;
        Ok(self.scratch[..total].to_vec())
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.inner) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioFrame;

    fn sine_20ms(freq: f32) -> Vec<f32> {
        (0..960)
            .map(|n| (n as f32 / 48000.0 * freq * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    #[test]
    fn opus_encode_decode_roundtrip_is_audible() {
        let mut enc = OpusEncoder::new(1, 32_000).unwrap();
        let mut dec = OpusDecoder::new(1).unwrap();
        let input = sine_20ms(440.0);

        let mut last = vec![0.0f32; 960];
        for _ in 0..10 {
            let pkt = enc
                .encode(AudioFrame {
                    samples: &input,
                    channels: 1,
                })
                .unwrap();
            assert!(!pkt.is_empty());
            last = dec.decode(&pkt).unwrap();
        }
        assert_eq!(last.len(), 960);
        let rms = (last.iter().map(|s| s * s).sum::<f32>() / 960.0).sqrt();
        assert!(rms > 0.1, "decoded audio should carry energy, rms={rms}");
    }

    #[test]
    fn channel_mismatch_is_rejected() {
        let mut enc = OpusEncoder::new(1, 24_000).unwrap();
        let stereo = vec![0.0f32; 1920];
        let err = enc.encode(AudioFrame {
            samples: &stereo,
            channels: 2,
        });
        assert!(matches!(err, Err(CodecError::InvalidFrame(_))));
    }
}
