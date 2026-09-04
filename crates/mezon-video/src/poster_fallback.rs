use std::fs::File;
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};

use openh264::formats::YUVSource;

use crate::VideoProbe;

const POSTER_SECONDS: f64 = 1.0;
const MAX_SAMPLES_SCANNED: u32 = 300;
const MAX_EXTRA_FEEDS: u32 = 8;
const DEFAULT_NAL_LENGTH_SIZE: usize = 4;

struct VideoTrack {
    id: u32,
    width: u32,
    height: u32,
    timescale: u32,
    nal_length_size: usize,
    parameter_sets: Vec<u8>,
}

struct PosterSample {
    index: u32,
    bytes: Vec<u8>,
}

pub(crate) fn probe_without_decoder(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    catch_unwind(AssertUnwindSafe(|| read_container(path, max_poster_edge))).unwrap_or_else(|_| {
        tracing::warn!(target: "mezon_video", path, "container parser panicked, skipping the poster");
        None
    })
}

fn read_container(path: &str, max_poster_edge: u32) -> Option<VideoProbe> {
    let file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut reader = mp4::Mp4Reader::read_header(BufReader::new(file), size).ok()?;
    let track = video_track(&reader)?;
    let poster_jpeg = decode_poster(&mut reader, &track, max_poster_edge);
    if poster_jpeg.is_none() {
        tracing::warn!(
            target: "mezon_video",
            width = track.width,
            height = track.height,
            "could not decode a poster, sending the container's size only"
        );
    }
    Some(VideoProbe {
        width: track.width,
        height: track.height,
        poster_jpeg,
    })
}

fn video_track<R: std::io::Read + std::io::Seek>(reader: &mp4::Mp4Reader<R>) -> Option<VideoTrack> {
    let mut tracks: Vec<_> = reader.tracks().iter().collect();
    tracks.sort_by_key(|(id, _)| **id);
    tracks.into_iter().find_map(|(id, track)| {
        if !matches!(track.track_type(), Ok(mp4::TrackType::Video))
            || track.width() == 0
            || track.height() == 0
        {
            return None;
        }
        let avcc = track
            .trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .map(|avc1| &avc1.avcc);
        Some(VideoTrack {
            id: *id,
            width: u32::from(track.width()),
            height: u32::from(track.height()),
            timescale: track.timescale(),
            nal_length_size: avcc.map_or(DEFAULT_NAL_LENGTH_SIZE, |avcc| {
                usize::from(avcc.length_size_minus_one & 0x3) + 1
            }),
            parameter_sets: avcc
                .map(|avcc| {
                    let mut out = Vec::new();
                    for nal in avcc
                        .sequence_parameter_sets
                        .iter()
                        .chain(avcc.picture_parameter_sets.iter())
                    {
                        out.extend_from_slice(&[0, 0, 0, 1]);
                        out.extend_from_slice(&nal.bytes);
                    }
                    out
                })
                .unwrap_or_default(),
        })
    })
}

fn decode_poster<R: std::io::Read + std::io::Seek>(
    reader: &mut mp4::Mp4Reader<R>,
    track: &VideoTrack,
    max_poster_edge: u32,
) -> Option<Vec<u8>> {
    if track.parameter_sets.is_empty() {
        return None;
    }
    let keyframe = poster_sample(reader, track)?;
    let mut decoder = openh264::decoder::Decoder::new()
        .inspect_err(|error| {
            tracing::warn!(target: "mezon_video", %error, "openh264 decoder init failed");
        })
        .ok()?;

    let mut unit = track.parameter_sets.clone();
    append_annex_b(&mut unit, &keyframe.bytes, track.nal_length_size);
    let first_extra = keyframe.index.saturating_add(1);
    for next in first_extra..=first_extra.saturating_add(MAX_EXTRA_FEEDS) {
        match decoder.decode(&unit) {
            Ok(Some(yuv)) => return encode(&yuv, max_poster_edge),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(target: "mezon_video", %error, "openh264 could not decode this video");
                return None;
            }
        }
        let Some(sample) = reader.read_sample(track.id, next).ok().flatten() else {
            break;
        };
        unit.clear();
        append_annex_b(&mut unit, &sample.bytes, track.nal_length_size);
    }
    None
}

fn encode(yuv: &openh264::decoder::DecodedYUV<'_>, max_poster_edge: u32) -> Option<Vec<u8>> {
    let (width, height) = yuv.dimensions();
    let mut rgb = vec![0u8; width.checked_mul(height)?.checked_mul(3)?];
    yuv.write_rgb8(&mut rgb);
    let rgb =
        image::RgbImage::from_raw(u32::try_from(width).ok()?, u32::try_from(height).ok()?, rgb)?;
    crate::poster::encode_rgb_jpeg(rgb, max_poster_edge)
}

fn poster_sample<R: std::io::Read + std::io::Seek>(
    reader: &mut mp4::Mp4Reader<R>,
    track: &VideoTrack,
) -> Option<PosterSample> {
    let count = reader.sample_count(track.id).ok()?.min(MAX_SAMPLES_SCANNED);
    let mut best: Option<PosterSample> = None;
    for index in 1..=count {
        let Some(sample) = reader.read_sample(track.id, index).ok().flatten() else {
            continue;
        };
        if best.is_some() && sample_seconds(sample.start_time, track.timescale) > POSTER_SECONDS {
            break;
        }
        if sample.is_sync {
            best = Some(PosterSample {
                index,
                bytes: sample.bytes.to_vec(),
            });
        }
    }
    best
}

fn sample_seconds(start_time: u64, timescale: u32) -> f64 {
    if timescale == 0 {
        return 0.0;
    }
    start_time as f64 / f64::from(timescale)
}

fn append_annex_b(out: &mut Vec<u8>, avcc: &[u8], nal_length_size: usize) {
    if nal_length_size == 0 {
        return;
    }
    let mut at = 0usize;
    while at + nal_length_size <= avcc.len() {
        let len = avcc[at..at + nal_length_size]
            .iter()
            .fold(0usize, |len, byte| (len << 8) | usize::from(*byte));
        at += nal_length_size;
        let Some(end) = at.checked_add(len).filter(|end| *end <= avcc.len()) else {
            return;
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[at..end]);
        at = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_annex_b_restamps_every_length_prefixed_unit() {
        let avcc = [0, 0, 0, 2, 0x67, 0xAA, 0, 0, 0, 1, 0x68];
        let mut out = Vec::new();
        append_annex_b(&mut out, &avcc, 4);
        assert_eq!(out, vec![0, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x68]);
    }

    #[test]
    fn append_annex_b_reads_a_two_byte_length_prefix() {
        let avcc = [0, 2, 0x67, 0xAA, 0, 1, 0x68];
        let mut out = Vec::new();
        append_annex_b(&mut out, &avcc, 2);
        assert_eq!(out, vec![0, 0, 0, 1, 0x67, 0xAA, 0, 0, 0, 1, 0x68]);
    }

    #[test]
    fn append_annex_b_reads_a_one_byte_length_prefix() {
        let mut out = Vec::new();
        append_annex_b(&mut out, &[1, 0x67, 2, 0x68, 0xBB], 1);
        assert_eq!(out, vec![0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x68, 0xBB]);
    }

    #[test]
    fn append_annex_b_stops_on_a_length_that_runs_past_the_buffer() {
        let mut out = Vec::new();
        append_annex_b(&mut out, &[0, 0, 0, 9, 0x67], 4);
        assert!(out.is_empty());
    }

    #[test]
    fn append_annex_b_ignores_a_trailing_partial_prefix() {
        let mut out = Vec::new();
        append_annex_b(&mut out, &[0, 0, 0, 1, 0x67, 0, 0], 4);
        assert_eq!(out, vec![0, 0, 0, 1, 0x67]);
    }

    #[test]
    fn append_annex_b_rejects_a_zero_length_prefix() {
        let mut out = Vec::new();
        append_annex_b(&mut out, &[0, 0, 0, 1, 0x67], 0);
        assert!(out.is_empty());
    }

    #[test]
    fn sample_seconds_uses_the_track_timescale_and_survives_a_zero() {
        assert!((sample_seconds(1500, 1000) - 1.5).abs() < f64::EPSILON);
        assert_eq!(sample_seconds(1500, 0), 0.0);
    }
}
