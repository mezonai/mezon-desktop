use std::io::Cursor;

use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CODEC_TYPE_NULL, CODEC_TYPE_OPUS, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::AudioError;

const OPUS_SAMPLE_RATE: u32 = 48_000;
const OPUS_MAX_FRAME: usize = 5760;
const MAX_DECODED_FRAMES: usize = OPUS_SAMPLE_RATE as usize * 60 * 15;

pub struct DecodedPcm {
    pub samples: std::sync::Arc<[f32]>,
    pub channels: usize,
    pub sample_rate: u32,
}

impl DecodedPcm {
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.sample_rate as f64
        }
    }
}

pub fn decode_audio(bytes: Vec<u8>) -> Result<DecodedPcm, AudioError> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut samples: Vec<f32> = Vec::new();
    let mut channels = 1usize;
    let mut sample_rate = OPUS_SAMPLE_RATE;

    decode_packets(
        mss,
        |_| {},
        |pcm, spec| {
            samples.extend_from_slice(pcm);
            channels = spec.channels;
            sample_rate = spec.sample_rate;
        },
    )?;

    if samples.is_empty() {
        return Err(AudioError::Decode("no samples produced".into()));
    }

    Ok(DecodedPcm {
        samples: samples.into(),
        channels,
        sample_rate,
    })
}

#[derive(Clone, Copy)]
pub(crate) struct PcmSpec {
    pub channels: usize,
    pub sample_rate: u32,
}

pub(crate) fn decode_packets<T, F>(
    mss: MediaSourceStream,
    mut on_track: T,
    mut emit: F,
) -> Result<(), AudioError>
where
    T: FnMut(Option<f64>),
    F: FnMut(&[f32], PcmSpec),
{
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Demux(e.to_string()))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(AudioError::NoAudioTrack)?;

    let track_id = track.id;
    let codec = track.codec_params.codec;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(1)
        .clamp(1, 2);
    on_track(track_duration_secs(&track.codec_params));

    if codec == CODEC_TYPE_OPUS {
        decode_opus(format.as_mut(), track_id, channels, &mut emit)
    } else {
        decode_symphonia(format.as_mut(), track_id, &mut emit)
    }
}

fn track_duration_secs(params: &symphonia::core::codecs::CodecParameters) -> Option<f64> {
    let frames = params.n_frames?;
    let time_base = params.time_base?;
    let time = time_base.calc_time(frames);
    let seconds = time.seconds as f64 + time.frac;
    (seconds > 0.0).then_some(seconds)
}

fn decode_opus<F>(
    format: &mut dyn FormatReader,
    track_id: u32,
    channels: usize,
    emit: &mut F,
) -> Result<(), AudioError>
where
    F: FnMut(&[f32], PcmSpec),
{
    let mut decoder = OpusDecoder::new(OPUS_SAMPLE_RATE as i32, channels as i32)?;
    let mut scratch = vec![0.0f32; OPUS_MAX_FRAME * channels];
    let spec = PcmSpec {
        channels,
        sample_rate: OPUS_SAMPLE_RATE,
    };
    let mut emitted_frames = 0usize;
    let mut produced = false;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) if produced => {
                tracing::warn!("audio stream ended early: {e}");
                break;
            }
            Err(e) => return Err(AudioError::Demux(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode_float(&packet.data, &mut scratch, OPUS_MAX_FRAME as i32);
        if decoded > 0 {
            let frames = decoded as usize;
            emit(&scratch[..frames * channels], spec);
            produced = true;
            emitted_frames += frames;
            if emitted_frames >= MAX_DECODED_FRAMES {
                break;
            }
        }
    }

    if !produced {
        return Err(AudioError::Decode("opus produced no samples".into()));
    }
    Ok(())
}

fn decode_symphonia<F>(
    format: &mut dyn FormatReader,
    track_id: u32,
    emit: &mut F,
) -> Result<(), AudioError>
where
    F: FnMut(&[f32], PcmSpec),
{
    let params = format
        .tracks()
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.codec_params.clone())
        .ok_or(AudioError::NoAudioTrack)?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;

    let mut buffer: Option<SampleBuffer<f32>> = None;
    let mut emitted_frames = 0usize;
    let mut produced = false;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) if produced => {
                tracing::warn!("audio stream ended early: {e}");
                break;
            }
            Err(e) => return Err(AudioError::Demux(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) if produced => {
                tracing::warn!("audio stream ended early: {e}");
                break;
            }
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        let frames = emit_interleaved(decoded, &mut buffer, emit);
        if frames > 0 {
            produced = true;
            emitted_frames += frames;
            if emitted_frames >= MAX_DECODED_FRAMES {
                break;
            }
        }
    }

    if !produced {
        return Err(AudioError::Decode("no samples produced".into()));
    }
    Ok(())
}

fn emit_interleaved<F>(
    decoded: AudioBufferRef<'_>,
    buffer: &mut Option<SampleBuffer<f32>>,
    emit: &mut F,
) -> usize
where
    F: FnMut(&[f32], PcmSpec),
{
    let spec = *decoded.spec();
    let capacity = decoded.capacity() as u64;
    let channels = spec.channels.count().max(1);
    let buf = buffer.get_or_insert_with(|| SampleBuffer::new(capacity, spec));
    buf.copy_interleaved_ref(decoded);
    let samples = buf.samples();
    emit(
        samples,
        PcmSpec {
            channels,
            sample_rate: spec.rate,
        },
    );
    samples.len() / channels
}

struct OpusDecoder {
    inner: *mut unsafe_libopus::OpusDecoder,
}

impl OpusDecoder {
    fn new(sample_rate: i32, channels: i32) -> Result<Self, AudioError> {
        let mut error = 0i32;
        let inner =
            unsafe { unsafe_libopus::opus_decoder_create(sample_rate, channels, &mut error) };
        if inner.is_null() || error != 0 {
            return Err(AudioError::Decode(format!(
                "opus_decoder_create failed (error {error})"
            )));
        }
        Ok(Self { inner })
    }

    fn decode_float(&mut self, packet: &[u8], pcm: &mut [f32], max_frame: i32) -> i32 {
        unsafe {
            unsafe_libopus::opus_decode_float(
                self.inner,
                packet.as_ptr(),
                packet.len() as i32,
                pcm.as_mut_ptr(),
                max_frame,
                0,
            )
        }
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.inner) };
    }
}
