use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use matroska_demuxer::{
    ContentCompAlgo, ContentEncodingValue, Frame, MatroskaFile, TrackEntry, TrackType,
};

const VIDEO_TIMESCALE: u32 = 90_000;
const MOVIE_TIMESCALE: u32 = 1_000;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
const FALLBACK_FRAME_NANOS: u64 = 33_333_333;
const LANGUAGE_UND: u16 = 0x55c4;
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemuxError {
    #[error("matroska source has no video track")]
    NoVideo,
    #[error("matroska video is VP8, which mp4 cannot carry")]
    Vp8,
    #[error("matroska video codec {0} cannot be remuxed")]
    UnsupportedVideo(String),
    #[error("matroska source has no samples")]
    Empty,
    #[error("matroska demux failed: {0}")]
    Demux(#[from] matroska_demuxer::DemuxError),
    #[error("remux io failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCodec {
    Avc,
    Hevc,
    Vp9,
    Av1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioCodec {
    Aac,
    Opus,
}

struct TrackPlan {
    number: u64,
    strip_prefix: Vec<u8>,
    kind: TrackKind,
}

enum TrackKind {
    Video {
        codec: VideoCodec,
        width: u32,
        height: u32,
        private: Vec<u8>,
    },
    Audio {
        codec: AudioCodec,
        channels: u16,
        sample_rate: u32,
        private: Vec<u8>,
    },
}

struct Sample {
    offset: u64,
    size: u32,
    pts_ns: u64,
    keyframe: bool,
    audio_ticks: Option<u32>,
}

pub(crate) fn remux_to_mp4(
    src: &Path,
    dst: &Path,
    max_seconds: Option<f64>,
) -> Result<(), RemuxError> {
    let mut demuxer = MatroskaFile::open(BufReader::new(File::open(src)?))?;
    let timestamp_scale = demuxer.info().timestamp_scale().get();
    let video = plan_video(demuxer.tracks())?;
    let audio = plan_audio(demuxer.tracks());
    let limit_ns = max_seconds
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| (seconds * 1e9) as u64);

    let mut out = BufWriter::new(File::create(dst)?);
    write_ftyp(&mut out)?;
    let mdat_start = out.stream_position()?;
    out.write_all(&1u32.to_be_bytes())?;
    out.write_all(b"mdat")?;
    out.write_all(&0u64.to_be_bytes())?;

    let mut video_samples = Vec::new();
    let mut audio_samples = Vec::new();
    let mut vp9_header = None;
    let mut frame = Frame::default();
    while demuxer.next_frame(&mut frame)? {
        let pts_ns = frame.timestamp.saturating_mul(timestamp_scale);
        let (plan, samples) = if frame.track == video.number {
            if limit_ns.is_some_and(|limit| pts_ns > limit && !video_samples.is_empty()) {
                break;
            }
            (&video, &mut video_samples)
        } else if let Some(audio) = audio.as_ref().filter(|audio| audio.number == frame.track) {
            (audio, &mut audio_samples)
        } else {
            continue;
        };
        let size = plan.strip_prefix.len() + frame.data.len();
        let Ok(size) = u32::try_from(size) else {
            continue;
        };
        let offset = out.stream_position()?;
        out.write_all(&plan.strip_prefix)?;
        out.write_all(&frame.data)?;
        let sample = match &plan.kind {
            TrackKind::Video { codec, private, .. } => {
                if *codec == VideoCodec::Vp9 && vp9_header.is_none() {
                    vp9_header = Vp9Header::parse(&frame.data).filter(|header| header.keyframe);
                }
                Sample {
                    offset,
                    size,
                    pts_ns,
                    keyframe: frame
                        .is_keyframe
                        .unwrap_or_else(|| is_keyframe(*codec, private, &frame.data)),
                    audio_ticks: None,
                }
            }
            TrackKind::Audio { codec, .. } => Sample {
                offset,
                size,
                pts_ns,
                keyframe: true,
                audio_ticks: audio_frame_ticks(*codec, &frame.data),
            },
        };
        samples.push(sample);
    }
    if video_samples.is_empty() {
        return Err(RemuxError::Empty);
    }

    let mdat_end = out.stream_position()?;
    out.seek(SeekFrom::Start(mdat_start + 8))?;
    out.write_all(&(mdat_end - mdat_start).to_be_bytes())?;
    out.seek(SeekFrom::Start(mdat_end))?;

    let start_ns = video_samples
        .iter()
        .chain(audio_samples.iter())
        .map(|sample| sample.pts_ns)
        .min()
        .unwrap_or(0);
    let mut tracks = vec![build_video_track(
        &video,
        &video_samples,
        start_ns,
        vp9_header,
    )];
    if let Some(audio) = audio.as_ref().filter(|_| !audio_samples.is_empty()) {
        tracks.push(build_audio_track(audio, &audio_samples, start_ns));
    }
    out.write_all(&moov(&tracks))?;
    out.flush()?;
    Ok(())
}

fn codec_id(track: &TrackEntry) -> &str {
    track.codec_id().trim_end_matches('\0')
}

fn strip_prefix(track: &TrackEntry) -> Option<Vec<u8>> {
    let mut prefix = Vec::new();
    for encoding in track.content_encodings().unwrap_or_default() {
        match encoding.encoding() {
            ContentEncodingValue::Compression(compression)
                if compression.algo() == ContentCompAlgo::Stripping =>
            {
                prefix.extend_from_slice(compression.settings().unwrap_or_default());
            }
            _ => return None,
        }
    }
    Some(prefix)
}

fn plan_video(tracks: &[TrackEntry]) -> Result<TrackPlan, RemuxError> {
    let track = tracks
        .iter()
        .find(|track| track.track_type() == TrackType::Video && track.flag_enabled())
        .ok_or(RemuxError::NoVideo)?;
    let id = codec_id(track);
    let codec = match id {
        "V_MPEG4/ISO/AVC" => VideoCodec::Avc,
        "V_MPEGH/ISO/HEVC" => VideoCodec::Hevc,
        "V_VP9" => VideoCodec::Vp9,
        "V_AV1" => VideoCodec::Av1,
        "V_VP8" => return Err(RemuxError::Vp8),
        other => return Err(RemuxError::UnsupportedVideo(other.to_string())),
    };
    let private = track.codec_private().unwrap_or_default().to_vec();
    if codec != VideoCodec::Vp9 && private.is_empty() {
        return Err(RemuxError::UnsupportedVideo(id.to_string()));
    }
    let strip = strip_prefix(track).ok_or_else(|| RemuxError::UnsupportedVideo(id.to_string()))?;
    let (width, height) = track.video().map_or((0, 0), |video| {
        (
            u32::try_from(video.pixel_width().get()).unwrap_or(0),
            u32::try_from(video.pixel_height().get()).unwrap_or(0),
        )
    });
    Ok(TrackPlan {
        number: track.track_number().get(),
        strip_prefix: strip,
        kind: TrackKind::Video {
            codec,
            width,
            height,
            private,
        },
    })
}

fn plan_audio(tracks: &[TrackEntry]) -> Option<TrackPlan> {
    tracks
        .iter()
        .filter(|track| track.track_type() == TrackType::Audio && track.flag_enabled())
        .find_map(|track| {
            let id = codec_id(track);
            let audio = track.audio()?;
            let sample_rate = audio.sampling_frequency().round() as u32;
            let channels = u16::try_from(audio.channels().get()).unwrap_or(2);
            let (codec, private) = match id {
                "A_OPUS" => (AudioCodec::Opus, track.codec_private()?.to_vec()),
                "A_AAC" => (AudioCodec::Aac, track.codec_private()?.to_vec()),
                legacy if legacy.starts_with("A_AAC/") => (
                    AudioCodec::Aac,
                    legacy_aac_config(legacy, sample_rate, channels)?,
                ),
                other => {
                    tracing::info!(
                        target: "mezon_video",
                        codec = other,
                        "matroska audio codec cannot be remuxed; playing video without it"
                    );
                    return None;
                }
            };
            Some(TrackPlan {
                number: track.track_number().get(),
                strip_prefix: strip_prefix(track)?,
                kind: TrackKind::Audio {
                    codec,
                    channels,
                    sample_rate: if codec == AudioCodec::Opus {
                        48_000
                    } else {
                        sample_rate
                    },
                    private,
                },
            })
        })
}

fn legacy_aac_config(codec_id: &str, sample_rate: u32, channels: u16) -> Option<Vec<u8>> {
    let object_type: u16 = if codec_id.ends_with("/MAIN") {
        1
    } else if codec_id.ends_with("/SSR") {
        3
    } else if codec_id.ends_with("/LTP") {
        4
    } else {
        2
    };
    let index = AAC_SAMPLE_RATES
        .iter()
        .position(|rate| *rate == sample_rate)? as u16;
    let config = (object_type << 11) | (index << 7) | ((channels & 0xf) << 3);
    Some(config.to_be_bytes().to_vec())
}

fn audio_frame_ticks(codec: AudioCodec, data: &[u8]) -> Option<u32> {
    match codec {
        AudioCodec::Aac => Some(1024),
        AudioCodec::Opus => opus_packet_samples(data),
    }
}

fn opus_packet_samples(packet: &[u8]) -> Option<u32> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let frame_samples: u32 = match config {
        0..=11 => [480, 960, 1920, 2880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames = match toc & 0x3 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(*packet.get(1)? & 0x3f),
    };
    (frames > 0).then_some(frame_samples * frames)
}

fn is_keyframe(codec: VideoCodec, private: &[u8], data: &[u8]) -> bool {
    match codec {
        VideoCodec::Avc => {
            let length_size = private.get(4).map_or(4, |byte| usize::from(byte & 0x3) + 1);
            nal_types(data, length_size).any(|header| header & 0x1f == 5)
        }
        VideoCodec::Hevc => {
            let length_size = private
                .get(21)
                .map_or(4, |byte| usize::from(byte & 0x3) + 1);
            nal_types(data, length_size).any(|header| (16..=21).contains(&((header >> 1) & 0x3f)))
        }
        VideoCodec::Vp9 => Vp9Header::parse(data).is_some_and(|header| header.keyframe),
        VideoCodec::Av1 => false,
    }
}

fn nal_types(data: &[u8], length_size: usize) -> impl Iterator<Item = u8> + '_ {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let length_bytes = data.get(at..at.checked_add(length_size)?)?;
        let length = length_bytes
            .iter()
            .fold(0usize, |acc, byte| (acc << 8) | usize::from(*byte));
        let start = at.checked_add(length_size)?;
        let end = start.checked_add(length)?;
        let header = *data.get(start..end)?.first()?;
        at = end;
        Some(header)
    })
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn read(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.data.get(self.bit / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Vp9Header {
    keyframe: bool,
    profile: u8,
    bit_depth: u8,
    chroma_subsampling: u8,
    full_range: bool,
}

impl Vp9Header {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut bits = BitReader { data, bit: 0 };
        if bits.read(2)? != 2 {
            return None;
        }
        let low = bits.read(1)?;
        let high = bits.read(1)?;
        let profile = (high << 1 | low) as u8;
        if profile == 3 {
            bits.read(1)?;
        }
        let mut header = Self {
            keyframe: false,
            profile,
            bit_depth: 8,
            chroma_subsampling: 1,
            full_range: false,
        };
        if bits.read(1)? == 1 {
            return Some(header);
        }
        header.keyframe = bits.read(1)? == 0;
        if !header.keyframe {
            return Some(header);
        }
        bits.read(2)?;
        if bits.read(24)? != 0x49_83_42 {
            return Some(header);
        }
        if profile >= 2 {
            header.bit_depth = if bits.read(1)? == 1 { 12 } else { 10 };
        }
        let color_space = bits.read(3)?;
        if color_space == 7 {
            header.full_range = true;
            header.chroma_subsampling = 3;
            return Some(header);
        }
        header.full_range = bits.read(1)? == 1;
        if profile == 1 || profile == 3 {
            let x = bits.read(1)?;
            let y = bits.read(1)?;
            header.chroma_subsampling = match (x, y) {
                (1, 0) => 2,
                (0, 0) => 3,
                _ => 1,
            };
        }
        Some(header)
    }
}

fn vp9_level(width: u32, height: u32) -> u8 {
    let pixels = u64::from(width) * u64::from(height);
    [
        (36_864, 10),
        (73_728, 11),
        (122_880, 20),
        (245_760, 21),
        (552_960, 30),
        (983_040, 31),
        (2_228_224, 40),
        (8_912_896, 50),
        (35_651_584, 60),
    ]
    .iter()
    .find(|(limit, _)| pixels <= *limit)
    .map_or(62, |(_, level)| *level)
}

fn ns_to_ticks(ns: u64, timescale: u32) -> u64 {
    (u128::from(ns) * u128::from(timescale) / NANOS_PER_SECOND) as u64
}

struct BuiltTrack {
    handler: [u8; 4],
    timescale: u32,
    media_duration: u64,
    presentation_duration: u64,
    start_offset_ns: u64,
    media_time: u64,
    width: u32,
    height: u32,
    sample_entry: Vec<u8>,
    deltas: Vec<u32>,
    composition_offsets: Option<Vec<u32>>,
    sync_samples: Option<Vec<u32>>,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
}

fn build_video_track(
    plan: &TrackPlan,
    samples: &[Sample],
    start_ns: u64,
    vp9_header: Option<Vp9Header>,
) -> BuiltTrack {
    let TrackKind::Video {
        codec,
        width,
        height,
        private,
    } = &plan.kind
    else {
        unreachable!("video plan carries a video kind");
    };
    let pts: Vec<u64> = samples
        .iter()
        .map(|sample| ns_to_ticks(sample.pts_ns - start_ns, VIDEO_TIMESCALE))
        .collect();
    let mut decode = pts.clone();
    decode.sort_unstable();
    let delay = pts
        .iter()
        .zip(&decode)
        .map(|(pts, dts)| dts.saturating_sub(*pts))
        .max()
        .unwrap_or(0);
    let first_decode = decode[0];
    let decode: Vec<u64> = decode.iter().map(|dts| dts - first_decode).collect();
    let mut deltas: Vec<u32> = decode
        .windows(2)
        .map(|pair| u32::try_from(pair[1] - pair[0]).unwrap_or(u32::MAX))
        .collect();
    let last = deltas
        .last()
        .copied()
        .filter(|delta| *delta > 0)
        .unwrap_or(ns_to_ticks(FALLBACK_FRAME_NANOS, VIDEO_TIMESCALE) as u32);
    deltas.push(last);
    let composition: Vec<u32> = pts
        .iter()
        .zip(&decode)
        .map(|(pts, dts)| u32::try_from(pts - first_decode + delay - dts).unwrap_or(0))
        .collect();
    let has_composition = composition.iter().any(|offset| *offset != 0);
    let all_sync = samples.iter().all(|sample| sample.keyframe);
    let mut sync: Vec<u32> = samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.keyframe)
        .map(|(index, _)| index as u32 + 1)
        .collect();
    if sync.is_empty() {
        sync.push(1);
    }
    let media_duration = deltas.iter().map(|delta| u64::from(*delta)).sum();
    let first_pts = pts.iter().copied().min().unwrap_or(0);
    let last_pts = pts.iter().copied().max().unwrap_or(0);
    BuiltTrack {
        handler: *b"vide",
        timescale: VIDEO_TIMESCALE,
        media_duration,
        presentation_duration: last_pts - first_pts + u64::from(last),
        start_offset_ns: first_pts * NANOS_PER_SECOND as u64 / u64::from(VIDEO_TIMESCALE),
        media_time: delay,
        width: *width,
        height: *height,
        sample_entry: video_sample_entry(*codec, *width, *height, private, vp9_header),
        deltas,
        composition_offsets: has_composition.then_some(composition),
        sync_samples: (!all_sync).then_some(sync),
        sizes: samples.iter().map(|sample| sample.size).collect(),
        offsets: samples.iter().map(|sample| sample.offset).collect(),
    }
}

fn build_audio_track(plan: &TrackPlan, samples: &[Sample], start_ns: u64) -> BuiltTrack {
    let TrackKind::Audio {
        codec,
        channels,
        sample_rate,
        private,
    } = &plan.kind
    else {
        unreachable!("audio plan carries an audio kind");
    };
    let fallback = samples
        .windows(2)
        .map(|pair| ns_to_ticks(pair[1].pts_ns.saturating_sub(pair[0].pts_ns), *sample_rate))
        .find(|ticks| *ticks > 0)
        .unwrap_or(1024) as u32;
    let deltas: Vec<u32> = samples
        .iter()
        .map(|sample| sample.audio_ticks.unwrap_or(fallback))
        .collect();
    let media_duration = deltas.iter().map(|delta| u64::from(*delta)).sum();
    BuiltTrack {
        handler: *b"soun",
        timescale: *sample_rate,
        media_duration,
        presentation_duration: media_duration,
        start_offset_ns: samples[0].pts_ns.saturating_sub(start_ns),
        media_time: 0,
        width: 0,
        height: 0,
        sample_entry: audio_sample_entry(*codec, *channels, *sample_rate, private),
        deltas,
        composition_offsets: None,
        sync_samples: None,
        sizes: samples.iter().map(|sample| sample.size).collect(),
        offsets: samples.iter().map(|sample| sample.offset).collect(),
    }
}

fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn full_box(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 4);
    body.extend_from_slice(&((u32::from(version) << 24) | (flags & 0x00ff_ffff)).to_be_bytes());
    body.extend_from_slice(payload);
    mp4_box(kind, &body)
}

fn container(kind: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    mp4_box(kind, &children.concat())
}

fn write_ftyp(out: &mut impl Write) -> std::io::Result<()> {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"isom");
    payload.extend_from_slice(&0x200u32.to_be_bytes());
    for brand in [b"isom", b"iso2", b"mp41"] {
        payload.extend_from_slice(brand);
    }
    out.write_all(&mp4_box(b"ftyp", &payload))
}

fn matrix() -> Vec<u8> {
    UNITY_MATRIX
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

fn movie_ticks(ns: u64) -> u64 {
    ns_to_ticks(ns, MOVIE_TIMESCALE)
}

fn track_movie_duration(track: &BuiltTrack) -> u64 {
    movie_ticks(track.start_offset_ns) + presentation_movie_ticks(track)
}

fn presentation_movie_ticks(track: &BuiltTrack) -> u64 {
    track.presentation_duration * u64::from(MOVIE_TIMESCALE) / u64::from(track.timescale)
}

fn moov(tracks: &[BuiltTrack]) -> Vec<u8> {
    let duration = tracks.iter().map(track_movie_duration).max().unwrap_or(0);
    let mut mvhd = Vec::new();
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
    mvhd.extend_from_slice(&(duration as u32).to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0; 10]);
    mvhd.extend_from_slice(&matrix());
    mvhd.extend_from_slice(&[0; 24]);
    mvhd.extend_from_slice(&(tracks.len() as u32 + 1).to_be_bytes());
    let mut children = vec![full_box(b"mvhd", 0, 0, &mvhd)];
    children.extend(
        tracks
            .iter()
            .enumerate()
            .map(|(index, track)| trak(track, index as u32 + 1)),
    );
    container(b"moov", &children)
}

fn trak(track: &BuiltTrack, track_id: u32) -> Vec<u8> {
    let is_audio = &track.handler == b"soun";
    let mut tkhd = Vec::new();
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&track_id.to_be_bytes());
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&(track_movie_duration(track) as u32).to_be_bytes());
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    tkhd.extend_from_slice(&(if is_audio { 0x0100u16 } else { 0 }).to_be_bytes());
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    tkhd.extend_from_slice(&matrix());
    tkhd.extend_from_slice(&(track.width << 16).to_be_bytes());
    tkhd.extend_from_slice(&(track.height << 16).to_be_bytes());

    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&0u32.to_be_bytes());
    mdhd.extend_from_slice(&track.timescale.to_be_bytes());
    mdhd.extend_from_slice(&(track.media_duration as u32).to_be_bytes());
    mdhd.extend_from_slice(&LANGUAGE_UND.to_be_bytes());
    mdhd.extend_from_slice(&0u16.to_be_bytes());

    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(&0u32.to_be_bytes());
    hdlr.extend_from_slice(&track.handler);
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.extend_from_slice(if is_audio {
        b"SoundHandler\0".as_slice()
    } else {
        b"VideoHandler\0".as_slice()
    });

    let media_header = if is_audio {
        full_box(b"smhd", 0, 0, &[0; 4])
    } else {
        full_box(b"vmhd", 0, 1, &[0; 8])
    };
    let dref = full_box(
        b"dref",
        0,
        0,
        &[1u32.to_be_bytes().as_slice(), &full_box(b"url ", 0, 1, &[])].concat(),
    );
    let minf = container(
        b"minf",
        &[media_header, container(b"dinf", &[dref]), stbl(track)],
    );
    let mdia = container(
        b"mdia",
        &[
            full_box(b"mdhd", 0, 0, &mdhd),
            full_box(b"hdlr", 0, 0, &hdlr),
            minf,
        ],
    );
    container(
        b"trak",
        &[full_box(b"tkhd", 0, 3, &tkhd), edts(track), mdia],
    )
}

fn edts(track: &BuiltTrack) -> Vec<u8> {
    let mut entries: Vec<(u32, i32)> = Vec::new();
    let offset = movie_ticks(track.start_offset_ns);
    if offset > 0 {
        entries.push((offset as u32, -1));
    }
    entries.push((
        presentation_movie_ticks(track) as u32,
        track.media_time as i32,
    ));
    let mut elst = (entries.len() as u32).to_be_bytes().to_vec();
    for (duration, media_time) in entries {
        elst.extend_from_slice(&duration.to_be_bytes());
        elst.extend_from_slice(&media_time.to_be_bytes());
        elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    }
    container(b"edts", &[full_box(b"elst", 0, 0, &elst)])
}

fn run_length(values: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for value in values {
        match runs.last_mut() {
            Some((count, last)) if last == value => *count += 1,
            _ => runs.push((1, *value)),
        }
    }
    runs
}

fn table(runs: &[(u32, u32)]) -> Vec<u8> {
    let mut payload = (runs.len() as u32).to_be_bytes().to_vec();
    for (count, value) in runs {
        payload.extend_from_slice(&count.to_be_bytes());
        payload.extend_from_slice(&value.to_be_bytes());
    }
    payload
}

fn stbl(track: &BuiltTrack) -> Vec<u8> {
    let stsd = full_box(
        b"stsd",
        0,
        0,
        &[1u32.to_be_bytes().as_slice(), &track.sample_entry].concat(),
    );
    let mut children = vec![
        stsd,
        full_box(b"stts", 0, 0, &table(&run_length(&track.deltas))),
    ];
    if let Some(offsets) = &track.composition_offsets {
        children.push(full_box(b"ctts", 0, 0, &table(&run_length(offsets))));
    }
    if let Some(sync) = &track.sync_samples {
        let mut payload = (sync.len() as u32).to_be_bytes().to_vec();
        payload.extend(sync.iter().flat_map(|index| index.to_be_bytes()));
        children.push(full_box(b"stss", 0, 0, &payload));
    }
    let stsc: Vec<u8> = [1u32, 1, 1, 1]
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
    children.push(full_box(b"stsc", 0, 0, &stsc));
    let mut stsz = 0u32.to_be_bytes().to_vec();
    stsz.extend_from_slice(&(track.sizes.len() as u32).to_be_bytes());
    stsz.extend(track.sizes.iter().flat_map(|size| size.to_be_bytes()));
    children.push(full_box(b"stsz", 0, 0, &stsz));
    let mut co64 = (track.offsets.len() as u32).to_be_bytes().to_vec();
    co64.extend(track.offsets.iter().flat_map(|offset| offset.to_be_bytes()));
    children.push(full_box(b"co64", 0, 0, &co64));
    container(b"stbl", &children)
}

fn video_sample_entry(
    codec: VideoCodec,
    width: u32,
    height: u32,
    private: &[u8],
    vp9_header: Option<Vp9Header>,
) -> Vec<u8> {
    let (kind, config) = match codec {
        VideoCodec::Avc => (*b"avc1", mp4_box(b"avcC", private)),
        VideoCodec::Hevc => (*b"hvc1", mp4_box(b"hvcC", private)),
        VideoCodec::Av1 => (*b"av01", mp4_box(b"av1C", private)),
        VideoCodec::Vp9 => (*b"vp09", vpcc(width, height, vp9_header)),
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(&[0; 6]);
    payload.extend_from_slice(&1u16.to_be_bytes());
    payload.extend_from_slice(&[0; 16]);
    payload.extend_from_slice(&(width.min(u32::from(u16::MAX)) as u16).to_be_bytes());
    payload.extend_from_slice(&(height.min(u32::from(u16::MAX)) as u16).to_be_bytes());
    payload.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    payload.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload.extend_from_slice(&1u16.to_be_bytes());
    payload.extend_from_slice(&[0; 32]);
    payload.extend_from_slice(&0x0018u16.to_be_bytes());
    payload.extend_from_slice(&(-1i16).to_be_bytes());
    payload.extend_from_slice(&config);
    mp4_box(&kind, &payload)
}

fn vpcc(width: u32, height: u32, header: Option<Vp9Header>) -> Vec<u8> {
    let header = header.unwrap_or(Vp9Header {
        keyframe: true,
        profile: 0,
        bit_depth: 8,
        chroma_subsampling: 1,
        full_range: false,
    });
    let payload = [
        header.profile,
        vp9_level(width, height),
        (header.bit_depth << 4) | (header.chroma_subsampling << 1) | u8::from(header.full_range),
        2,
        2,
        2,
        0,
        0,
    ];
    full_box(b"vpcC", 1, 0, &payload)
}

fn audio_sample_entry(
    codec: AudioCodec,
    channels: u16,
    sample_rate: u32,
    private: &[u8],
) -> Vec<u8> {
    let (kind, config) = match codec {
        AudioCodec::Aac => (*b"mp4a", esds(private)),
        AudioCodec::Opus => (*b"Opus", dops(private, channels)),
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(&[0; 6]);
    payload.extend_from_slice(&1u16.to_be_bytes());
    payload.extend_from_slice(&[0; 8]);
    payload.extend_from_slice(&channels.to_be_bytes());
    payload.extend_from_slice(&16u16.to_be_bytes());
    payload.extend_from_slice(&[0; 4]);
    payload.extend_from_slice(&(sample_rate.min(u32::from(u16::MAX)) << 16).to_be_bytes());
    payload.extend_from_slice(&config);
    mp4_box(&kind, &payload)
}

fn descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = payload.len() as u32;
    out.extend_from_slice(&[
        0x80 | ((len >> 21) & 0x7f) as u8,
        0x80 | ((len >> 14) & 0x7f) as u8,
        0x80 | ((len >> 7) & 0x7f) as u8,
        (len & 0x7f) as u8,
    ]);
    out.extend_from_slice(payload);
    out
}

fn esds(audio_specific_config: &[u8]) -> Vec<u8> {
    let mut decoder = vec![0x40, 0x15, 0, 0, 0];
    decoder.extend_from_slice(&0u32.to_be_bytes());
    decoder.extend_from_slice(&0u32.to_be_bytes());
    decoder.extend_from_slice(&descriptor(0x05, audio_specific_config));
    let mut es = vec![0, 0, 0];
    es.extend_from_slice(&descriptor(0x04, &decoder));
    es.extend_from_slice(&descriptor(0x06, &[0x02]));
    full_box(b"esds", 0, 0, &descriptor(0x03, &es))
}

fn dops(opus_head: &[u8], channels: u16) -> Vec<u8> {
    let head = opus_head
        .strip_prefix(b"OpusHead")
        .filter(|head| head.len() >= 11);
    let mut payload = vec![0u8];
    match head {
        Some(head) => {
            payload.push(head[1]);
            payload.extend_from_slice(&u16::from_le_bytes([head[2], head[3]]).to_be_bytes());
            payload.extend_from_slice(
                &u32::from_le_bytes([head[4], head[5], head[6], head[7]]).to_be_bytes(),
            );
            payload.extend_from_slice(&i16::from_le_bytes([head[8], head[9]]).to_be_bytes());
            payload.push(head[10]);
            if head[10] != 0 {
                let mapping_len = 2 + usize::from(head[1]);
                payload.extend_from_slice(head.get(11..11 + mapping_len).unwrap_or_default());
            }
        }
        None => {
            payload.push(channels.min(2) as u8);
            payload.extend_from_slice(&312u16.to_be_bytes());
            payload.extend_from_slice(&48_000u32.to_be_bytes());
            payload.extend_from_slice(&0i16.to_be_bytes());
            payload.push(0);
        }
    }
    mp4_box(b"dOps", &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BitWriter {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bit: 0,
            }
        }

        fn put(&mut self, value: u32, count: usize) -> &mut Self {
            for shift in (0..count).rev() {
                if self.bit.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let bit = ((value >> shift) & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.bit % 8);
                self.bit += 1;
            }
            self
        }
    }

    fn sample(pts_ms: u64, keyframe: bool) -> Sample {
        Sample {
            offset: 0,
            size: 1,
            pts_ns: pts_ms * 1_000_000,
            keyframe,
            audio_ticks: None,
        }
    }

    fn avc_plan() -> TrackPlan {
        TrackPlan {
            number: 1,
            strip_prefix: Vec::new(),
            kind: TrackKind::Video {
                codec: VideoCodec::Avc,
                width: 640,
                height: 360,
                private: vec![1, 0x64, 0, 0x1f, 0xff],
            },
        }
    }

    #[test]
    fn opus_packet_durations_follow_the_toc() {
        assert_eq!(opus_packet_samples(&[31 << 3]), Some(960));
        assert_eq!(opus_packet_samples(&[(1 << 3) | 1]), Some(1920));
        assert_eq!(opus_packet_samples(&[(16 << 3) | 3, 3]), Some(360));
        assert_eq!(opus_packet_samples(&[(31 << 3) | 3, 0]), None);
        assert_eq!(opus_packet_samples(&[]), None);
    }

    #[test]
    fn vp9_keyframe_header_yields_profile_depth_and_range() {
        let mut bits = BitWriter::new();
        bits.put(2, 2)
            .put(0, 1)
            .put(1, 1)
            .put(0, 1)
            .put(0, 1)
            .put(1, 1)
            .put(0, 1)
            .put(0x49_83_42, 24)
            .put(1, 1)
            .put(2, 3)
            .put(1, 1);
        let header = Vp9Header::parse(&bits.bytes).unwrap();
        assert_eq!(
            header,
            Vp9Header {
                keyframe: true,
                profile: 2,
                bit_depth: 12,
                chroma_subsampling: 1,
                full_range: true,
            }
        );
        let mut inter = BitWriter::new();
        inter.put(2, 2).put(0, 2).put(0, 1).put(1, 1);
        assert!(!Vp9Header::parse(&inter.bytes).unwrap().keyframe);
        assert!(Vp9Header::parse(&[0x00]).is_none());
    }

    #[test]
    fn vp9_level_tracks_the_picture_size() {
        assert_eq!(vp9_level(640, 360), 21);
        assert_eq!(vp9_level(1280, 720), 31);
        assert_eq!(vp9_level(1920, 1080), 40);
        assert_eq!(vp9_level(7680, 4320), 60);
    }

    #[test]
    fn legacy_aac_codec_ids_get_an_audio_specific_config() {
        assert_eq!(
            legacy_aac_config("A_AAC/MPEG4/LC", 48_000, 2),
            Some(vec![0x11, 0x90])
        );
        assert_eq!(
            legacy_aac_config("A_AAC/MPEG2/MAIN", 44_100, 1),
            Some(vec![0x0a, 0x08])
        );
        assert_eq!(legacy_aac_config("A_AAC/MPEG4/LC", 47_000, 2), None);
    }

    #[test]
    fn opus_head_becomes_a_big_endian_dops_box() {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
        let dops = dops(&head, 2);
        assert_eq!(&dops[4..8], b"dOps");
        assert_eq!(&dops[8..], &[0, 2, 0x01, 0x38, 0, 0, 0xbb, 0x80, 0, 0, 0]);
    }

    #[test]
    fn avc_keyframes_are_found_from_idr_nal_units() {
        let private = [1, 0x64, 0, 0x1f, 0xff];
        let idr = [0, 0, 0, 2, 0x09, 0xf0, 0, 0, 0, 1, 0x65];
        let non_idr = [0, 0, 0, 1, 0x41];
        assert!(is_keyframe(VideoCodec::Avc, &private, &idr));
        assert!(!is_keyframe(VideoCodec::Avc, &private, &non_idr));
        assert!(!is_keyframe(VideoCodec::Avc, &private, &[0, 0, 0, 9, 0x65]));
    }

    #[test]
    fn reordered_frames_get_non_negative_offsets_and_keep_the_last_frame() {
        let samples = [
            sample(0, true),
            sample(100, false),
            sample(33, false),
            sample(67, false),
        ];
        let track = build_video_track(&avc_plan(), &samples, 0, None);
        let offsets = track.composition_offsets.unwrap();
        let decode: Vec<u64> = std::iter::once(0)
            .chain(track.deltas.iter().scan(0u64, |at, delta| {
                *at += u64::from(*delta);
                Some(*at)
            }))
            .take(samples.len())
            .collect();
        let presented: Vec<u64> = decode
            .iter()
            .zip(&offsets)
            .map(|(dts, offset)| dts + u64::from(*offset) - track.media_time)
            .collect();
        assert_eq!(presented, vec![0, 9000, 2970, 6030]);
        assert_eq!(track.sync_samples, Some(vec![1]));
        assert_eq!(track.presentation_duration, 9000 + 2970);
        assert_eq!(track.start_offset_ns, 0);
    }

    #[test]
    fn a_late_track_start_becomes_an_empty_edit() {
        let samples = [sample(500, true), sample(533, true)];
        let track = build_video_track(&avc_plan(), &samples, 0, None);
        let edts = edts(&track);
        let elst = &edts[16..];
        assert_eq!(u32::from_be_bytes(elst[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(elst[8..12].try_into().unwrap()), 500);
        assert_eq!(i32::from_be_bytes(elst[12..16].try_into().unwrap()), -1);
        assert!(track.sync_samples.is_none());
    }
}
