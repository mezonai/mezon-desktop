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

pub fn sniff_sound_mime(bytes: &[u8]) -> Option<&'static str> {
    if is_wav(bytes) {
        return Some("audio/wav");
    }
    let audio = skip_id3(bytes);
    if is_mpeg_audio(audio) || has_mp3_stream(audio) {
        return Some("audio/mpeg");
    }
    None
}

fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WAVE"
}

fn id3_payload_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 10 || !bytes.starts_with(b"ID3") {
        return None;
    }
    let size = u32::from(bytes[6] & 0x7F) << 21
        | u32::from(bytes[7] & 0x7F) << 14
        | u32::from(bytes[8] & 0x7F) << 7
        | u32::from(bytes[9] & 0x7F);
    Some(size as usize)
}

fn skip_id3(bytes: &[u8]) -> &[u8] {
    let mut rest = bytes;
    loop {
        let tag = id3_tag_len(rest);
        if tag == 0 {
            return rest;
        }
        rest = rest.get(tag..).unwrap_or(&[]);
    }
}

pub fn id3_tag_len(bytes: &[u8]) -> usize {
    id3_payload_len(bytes)
        .map(|size| {
            let footer = if bytes[3] == 4 && bytes[5] & 0x10 != 0 {
                10
            } else {
                0
            };
            10usize.saturating_add(size).saturating_add(footer)
        })
        .unwrap_or(0)
}

fn is_mpeg_audio(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0
}

const MPEG_SYNC_SEARCH_LIMIT: usize = 64 * 1024;

fn has_mp3_stream(audio: &[u8]) -> bool {
    let end = audio.len().min(MPEG_SYNC_SEARCH_LIMIT);
    (0..end).any(|offset| {
        let header = &audio[offset..];
        mp3_frame_len(header).is_some_and(|len| match audio.get(offset.saturating_add(len)..) {
            Some([]) => true,
            Some(next) => same_mpeg_stream(header, next) && mp3_frame_len(next).is_some(),
            None => false,
        })
    })
}

fn same_mpeg_stream(header: &[u8], next: &[u8]) -> bool {
    next.len() >= 4
        && header[1] & 0xFE == next[1] & 0xFE
        && header[2] & 0x0C == next[2] & 0x0C
        && (header[3] >> 6 == 0b11) == (next[3] >> 6 == 0b11)
}

fn mp3_frame_len(header: &[u8]) -> Option<usize> {
    let frame = first_mp3_frame(header)?;
    if (header[1] >> 1) & 0x03 != 0b01 {
        return None;
    }
    let padding = usize::from((header[2] >> 1) & 0x01);
    Some(
        frame.samples_per_frame as usize / 8 * frame.bitrate as usize / frame.sample_rate as usize
            + padding,
    )
}

const FULL_DECODE_MAX_BYTES: usize = 8 * 1024 * 1024;

pub fn audio_duration_secs(bytes: &[u8]) -> Option<f64> {
    audio_duration_secs_with_len(bytes, bytes.len() as u64)
}

pub fn audio_duration_secs_with_len(bytes: &[u8], total_len: u64) -> Option<f64> {
    let complete = !bytes.is_empty() && bytes.len() as u64 == total_len;
    if complete && bytes.len() <= FULL_DECODE_MAX_BYTES {
        if let Some(duration) = symphonia_header_duration(bytes) {
            return Some(duration);
        }
        if let Some(duration) = decode_audio(bytes.to_vec())
            .ok()
            .map(|pcm| pcm.duration_secs())
            .filter(|duration| *duration > 0.0)
        {
            return Some(duration);
        }
    }
    wav_duration(bytes, total_len).or_else(|| mp3_duration(bytes, total_len))
}

fn wav_duration(bytes: &[u8], total_len: u64) -> Option<f64> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut index = 12usize;
    let mut byte_rate = None;
    while index + 8 <= bytes.len() {
        let id = &bytes[index..index + 4];
        let size = u32::from_le_bytes(bytes[index + 4..index + 8].try_into().ok()?) as usize;
        let body = index + 8;
        if id == b"fmt " && size >= 16 && body + 12 <= bytes.len() {
            let rate = u32::from_le_bytes(bytes[body + 8..body + 12].try_into().ok()?);
            if rate > 0 {
                byte_rate = Some(rate);
            }
        } else if id == b"data" {
            let rate = byte_rate?;
            let mut data_len = size;
            if bytes.len() as u64 == total_len {
                data_len = size.min(bytes.len().saturating_sub(body));
            }
            return (data_len > 0).then_some(data_len as f64 / f64::from(rate));
        }
        let padded = size + (size % 2);
        index = body.saturating_add(padded);
    }
    None
}

fn mp3_duration(bytes: &[u8], total_len: u64) -> Option<f64> {
    let id3 = id3_tag_len(bytes);
    let audio = bytes.get(id3..)?;
    let frame = first_mp3_frame(audio)?;
    if let Some(frames) = xing_frames(audio, &frame) {
        let samples = frames as f64 * f64::from(frame.samples_per_frame);
        return (frame.sample_rate > 0).then_some(samples / f64::from(frame.sample_rate));
    }
    if frame.bitrate == 0 {
        return None;
    }
    let payload = total_len.saturating_sub(id3 as u64);
    (payload > 0).then_some(payload as f64 * 8.0 / f64::from(frame.bitrate))
}

struct Mp3Frame {
    bitrate: u32,
    sample_rate: u32,
    samples_per_frame: u32,
    mpeg1: bool,
    channels: u8,
}

fn first_mp3_frame(audio: &[u8]) -> Option<Mp3Frame> {
    if !is_mpeg_audio(audio) || audio.len() < 4 {
        return None;
    }
    let version_bits = (audio[1] >> 3) & 0x03;
    let layer_bits = (audio[1] >> 1) & 0x03;
    let version = match version_bits {
        0b11 => 0,
        0b10 => 1,
        0b00 => 2,
        _ => return None,
    };
    let layer = match layer_bits {
        0b11 => 1,
        0b10 => 2,
        0b01 => 3,
        _ => return None,
    };
    let bitrate_index = (audio[2] >> 4) & 0x0F;
    let sample_index = (audio[2] >> 2) & 0x03;
    if bitrate_index == 0 || bitrate_index == 0x0F || sample_index == 0x03 {
        return None;
    }
    let kbps = mp3_bitrate_kbps(version, layer, bitrate_index as usize)?;
    let sample_rate = MP3_SAMPLE_RATE[version][sample_index as usize];
    if sample_rate == 0 {
        return None;
    }
    let samples_per_frame = if layer == 1 {
        384
    } else if layer == 3 && version != 0 {
        576
    } else {
        1152
    };
    Some(Mp3Frame {
        bitrate: kbps * 1000,
        sample_rate,
        samples_per_frame,
        mpeg1: version == 0,
        channels: if audio[3] >> 6 == 0b11 { 1 } else { 2 },
    })
}

fn xing_frames(audio: &[u8], frame: &Mp3Frame) -> Option<u32> {
    let side = if frame.mpeg1 {
        if frame.channels == 1 { 17 } else { 32 }
    } else if frame.channels == 1 {
        9
    } else {
        17
    };
    let start = 4 + side;
    let tag = audio.get(start..start + 4)?;
    if tag != b"Xing" && tag != b"Info" {
        return None;
    }
    let flags = u32::from_be_bytes(audio.get(start + 4..start + 8)?.try_into().ok()?);
    if flags & 1 == 0 {
        return None;
    }
    let frames = u32::from_be_bytes(audio.get(start + 8..start + 12)?.try_into().ok()?);
    (frames > 0).then_some(frames)
}

const MP3_SAMPLE_RATE: [[u32; 3]; 3] = [
    [44_100, 48_000, 32_000],
    [22_050, 24_000, 16_000],
    [11_025, 12_000, 8_000],
];

fn mp3_bitrate_kbps(version: usize, layer: u8, index: usize) -> Option<u32> {
    let table: &[u32; 16] = match (version == 0, layer) {
        (true, 1) => &[
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
        ],
        (true, 2) => &[
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
        ],
        (true, 3) => &[
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
        ],
        (false, 1) => &[
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
        ],
        (false, 2 | 3) => &[
            0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
        ],
        _ => return None,
    };
    let kbps = *table.get(index)?;
    (kbps > 0).then_some(kbps)
}

fn symphonia_header_duration(bytes: &[u8]) -> Option<f64> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed
        .format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)?;
    track_duration_secs(&track.codec_params)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_sound_mime_detects_wav() {
        let wav = crate::stream::tests::wav_sine(0.01);
        assert_eq!(sniff_sound_mime(&wav), Some("audio/wav"));
    }

    #[test]
    fn sniff_sound_mime_detects_mp3_sync() {
        assert_eq!(
            sniff_sound_mime(&[0xFF, 0xFB, 0x90, 0x00]),
            Some("audio/mpeg")
        );
    }

    #[test]
    fn sniff_sound_mime_detects_mp3_after_id3() {
        let mut bytes = vec![0u8; 20];
        bytes[0..3].copy_from_slice(b"ID3");
        bytes[10] = 0xFF;
        bytes[11] = 0xFB;
        assert_eq!(sniff_sound_mime(&bytes), Some("audio/mpeg"));
    }

    #[test]
    fn sniff_sound_mime_rejects_non_audio() {
        assert_eq!(sniff_sound_mime(b"GIF89a"), None);
        assert_eq!(sniff_sound_mime(b"PNG\r\n\x1a\n"), None);
    }

    fn mp3_frames(count: usize) -> Vec<u8> {
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        frame.repeat(count)
    }

    fn id3_tag(flags: u8, payload: usize) -> Vec<u8> {
        let mut tag = vec![0u8; 10 + payload];
        tag[..6].copy_from_slice(&[b'I', b'D', b'3', 4, 0, flags]);
        tag[9] = payload as u8;
        if flags & 0x10 != 0 {
            tag.extend_from_slice(&[b'3', b'D', b'I', 4, 0, flags, 0, 0, 0, payload as u8]);
        }
        tag
    }

    #[test]
    fn sniff_sound_mime_finds_mp3_frames_after_padding_or_junk() {
        let padded = [id3_tag(0, 20), vec![0u8; 600], mp3_frames(3)].concat();
        assert_eq!(sniff_sound_mime(&padded), Some("audio/mpeg"));

        let leading_junk = [vec![0u8; 417], mp3_frames(3)].concat();
        assert_eq!(sniff_sound_mime(&leading_junk), Some("audio/mpeg"));

        let last_frame_only = [id3_tag(0, 20), vec![0u8; 600], mp3_frames(1)].concat();
        assert_eq!(sniff_sound_mime(&last_frame_only), Some("audio/mpeg"));
    }

    #[test]
    fn sniff_sound_mime_skips_every_id3_tag_and_its_footer() {
        let two_tags = [id3_tag(0, 20), id3_tag(0, 20), mp3_frames(1)].concat();
        assert_eq!(sniff_sound_mime(&two_tags), Some("audio/mpeg"));

        let footer = id3_tag(0x10, 20);
        assert_eq!(id3_tag_len(&footer), footer.len());
        let with_footer = [footer, mp3_frames(1)].concat();
        assert_eq!(sniff_sound_mime(&with_footer), Some("audio/mpeg"));

        let mut v23_flag = id3_tag(0, 20);
        v23_flag[3] = 3;
        v23_flag[5] = 0x10;
        assert_eq!(id3_tag_len(&v23_flag), v23_flag.len());
    }

    #[test]
    fn sniff_sound_mime_needs_two_frames_in_a_row_past_the_start() {
        let mut lone_header = vec![0u8; 2_000];
        lone_header[300..304].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        assert_eq!(sniff_sound_mime(&lone_header), None);

        let mut jpeg = vec![0u8; 2_000];
        jpeg[..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
        assert_eq!(sniff_sound_mime(&jpeg), None);

        let mut mixed = [vec![0u8; 300], mp3_frames(2), vec![0u8; 100]].concat();
        mixed[300 + 417..300 + 421].copy_from_slice(&[0xFF, 0xFB, 0x94, 0x00]);
        assert_eq!(sniff_sound_mime(&mixed), None);

        let mut channels = [vec![0u8; 300], mp3_frames(2), vec![0u8; 100]].concat();
        channels[300 + 417 + 3] = 0xC0;
        assert_eq!(sniff_sound_mime(&channels), None);
    }

    #[test]
    fn sniff_sound_mime_does_not_resync_on_layer_two_frames() {
        let mut frame = vec![0u8; 417];
        frame[..4].copy_from_slice(&[0xFF, 0xFD, 0x90, 0x00]);
        let layer_two = [vec![0u8; 100], frame.repeat(3)].concat();
        assert_eq!(sniff_sound_mime(&layer_two), None);
    }

    #[test]
    fn wav_duration_reads_the_data_chunk() {
        let wav = crate::stream::tests::wav_sine(1.0);
        let duration = audio_duration_secs(&wav).expect("wav duration");
        assert!((duration - 1.0).abs() < 0.02, "{duration}");
    }

    #[test]
    fn wav_duration_clamps_a_data_chunk_to_the_file() {
        let wav = crate::stream::tests::wav_sine(1.0);
        let prefix = &wav[..64.min(wav.len())];
        let from_header = wav_duration(prefix, wav.len() as u64).expect("header duration");
        assert!((from_header - 1.0).abs() < 0.02, "{from_header}");

        let mut claimed = wav.clone();
        claimed[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        let clamped = wav_duration(&claimed, claimed.len() as u64).expect("clamped duration");
        assert!(clamped < 2.0, "{clamped}");
    }

    #[test]
    fn id3_tag_len_uses_the_synchsafe_size() {
        let mut bytes = vec![0u8; 20];
        bytes[0..3].copy_from_slice(b"ID3");
        bytes[9] = 10;
        assert_eq!(id3_tag_len(&bytes), 20);
        assert!(skip_id3(&bytes).is_empty());
        assert_eq!(id3_tag_len(b"nope"), 0);
    }

    #[test]
    fn mp3_duration_uses_bitrate_and_file_length() {
        let mut bytes = vec![0u8; 128_000];
        bytes[0] = 0xFF;
        bytes[1] = 0xFB;
        bytes[2] = 0x90;
        bytes[3] = 0x00;
        let duration = audio_duration_secs(&bytes).expect("mp3 duration");
        assert!((duration - 8.0).abs() < 0.05, "{duration}");
    }
}
