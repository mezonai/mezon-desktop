use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

const MOOV_MAX: u64 = 64 << 20;
const TOP_BOXES_MAX: usize = 1024;
const TRAKS_MAX: usize = 16;
const SAMPLES_MAX: u32 = 4 << 20;
const SEGMENTS_MAX: u32 = 16384;
const SEGMENT_S: u64 = 4;

const TOP_BOXES: [&[u8; 4]; 10] = [
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"pnot", b"junk", b"meta",
];

#[derive(Clone, Copy, Default)]
struct VideoSample {
    dts: u64,
    cto: i32,
    sync: bool,
}

struct VideoTrack {
    timescale: u32,
    edit_start: u64,
    samples: Vec<VideoSample>,
}

#[derive(Clone, Copy, Default)]
struct Trak<'a> {
    tkhd: Option<&'a [u8]>,
    mdhd: Option<&'a [u8]>,
    hdlr: Option<&'a [u8]>,
    elst: Option<&'a [u8]>,
    stsd: Option<&'a [u8]>,
    stts: Option<&'a [u8]>,
    ctts: Option<&'a [u8]>,
    stss: Option<&'a [u8]>,
    stsz: Option<&'a [u8]>,
    stz2: Option<&'a [u8]>,
    stsc: Option<&'a [u8]>,
    stco: Option<&'a [u8]>,
    co64: Option<&'a [u8]>,
}

fn rd16(p: &[u8]) -> u16 {
    u16::from_be_bytes([p[0], p[1]])
}

fn rd32(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

fn rd64(p: &[u8]) -> u64 {
    u64::from_be_bytes([p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]])
}

fn next_box<'a>(rest: &mut &'a [u8]) -> Option<([u8; 4], &'a [u8])> {
    if rest.len() < 8 {
        return None;
    }
    let mut size = u64::from(rd32(rest));
    let mut hdr = 8usize;
    if size == 1 {
        if rest.len() < 16 {
            return None;
        }
        size = rd64(&rest[8..]);
        hdr = 16;
    } else if size == 0 {
        size = rest.len() as u64;
    }
    if size < hdr as u64 || size > rest.len() as u64 {
        return None;
    }
    let kind = [rest[4], rest[5], rest[6], rest[7]];
    let end = size as usize;
    let body = &rest[hdr..end];
    *rest = &rest[end..];
    Some((kind, body))
}

fn child<'a>(mut parent: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    while let Some((t, body)) = next_box(&mut parent) {
        if &t == kind {
            return Some(body);
        }
    }
    None
}

fn trak_boxes(trak: &[u8]) -> Trak<'_> {
    let mut t = Trak {
        tkhd: child(trak, b"tkhd"),
        elst: child(trak, b"edts").and_then(|edts| child(edts, b"elst")),
        ..Default::default()
    };
    let Some(mdia) = child(trak, b"mdia") else {
        return t;
    };
    t.mdhd = child(mdia, b"mdhd");
    t.hdlr = child(mdia, b"hdlr");
    if let Some(stbl) = child(mdia, b"minf").and_then(|minf| child(minf, b"stbl")) {
        t.stsd = child(stbl, b"stsd");
        t.stts = child(stbl, b"stts");
        t.ctts = child(stbl, b"ctts");
        t.stss = child(stbl, b"stss");
        t.stsz = child(stbl, b"stsz");
        t.stz2 = child(stbl, b"stz2");
        t.stsc = child(stbl, b"stsc");
        t.stco = child(stbl, b"stco");
        t.co64 = child(stbl, b"co64");
    }
    t
}

fn timescale_of(b: &[u8]) -> u32 {
    if b.len() < 24 {
        return 0;
    }
    if b[0] == 1 {
        if b.len() >= 32 { rd32(&b[20..]) } else { 0 }
    } else {
        rd32(&b[12..])
    }
}

fn table(b: Option<&[u8]>, entry_size: usize) -> Option<u32> {
    let b = b?;
    if b.len() < 8 {
        return None;
    }
    let count = rd32(&b[4..]);
    ((count as usize) <= (b.len() - 8) / entry_size).then_some(count)
}

struct BitReader<'a> {
    p: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            if self.bit >= self.p.len() * 8 {
                return None;
            }
            v = (v << 1) | u32::from((self.p[self.bit / 8] >> (7 - (self.bit % 8))) & 1);
            self.bit += 1;
        }
        Some(v)
    }
}

fn parse_asc(p: &[u8]) -> bool {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let mut r = BitReader { p, bit: 0 };
    let Some(mut aot) = r.read(5) else {
        return false;
    };
    if aot == 31 {
        let Some(ext) = r.read(6) else {
            return false;
        };
        aot = 32 + ext;
    }
    let _ = aot;
    let Some(sfi) = r.read(4) else {
        return false;
    };
    let rate = if sfi == 15 {
        let Some(rate) = r.read(24) else {
            return false;
        };
        rate
    } else if (sfi as usize) < RATES.len() {
        RATES[sfi as usize]
    } else {
        return false;
    };
    r.read(4).is_some() && rate > 0
}

fn descriptor<'a>(s: &mut &'a [u8]) -> Option<(u8, &'a [u8])> {
    if s.len() < 2 {
        return None;
    }
    let tag = s[0];
    let mut i = 1usize;
    let mut len = 0usize;
    for k in 0.. {
        if i >= s.len() || k == 4 {
            return None;
        }
        let b = s[i];
        i += 1;
        len = (len << 7) | usize::from(b & 0x7f);
        if (b & 0x80) == 0 {
            break;
        }
    }
    if len > s.len() - i {
        return None;
    }
    let body = &s[i..i + len];
    *s = &s[i + len..];
    Some((tag, body))
}

fn esds_asc(esds: &[u8]) -> Option<&[u8]> {
    if esds.len() < 4 {
        return None;
    }
    let mut s = &esds[4..];
    let (tag, mut es) = descriptor(&mut s)?;
    if tag != 0x03 || es.len() < 3 {
        return None;
    }
    let flags = es[2];
    let mut skip = 3 + if (flags & 0x80) != 0 { 2 } else { 0 };
    if (flags & 0x40) != 0 {
        if es.len() <= skip {
            return None;
        }
        skip += 1 + usize::from(es[skip]);
    }
    skip += if (flags & 0x20) != 0 { 2 } else { 0 };
    if skip > es.len() {
        return None;
    }
    es = &es[skip..];
    while let Some((tag, d)) = descriptor(&mut es) {
        if tag != 0x04 {
            continue;
        }
        if d.len() < 13 || d[0] != 0x40 {
            return None;
        }
        let mut sub = &d[13..];
        while let Some((sub_tag, info)) = descriptor(&mut sub) {
            if sub_tag == 0x05 {
                return Some(info);
            }
        }
        return None;
    }
    None
}

fn video_entry(fourcc: &[u8], e: &[u8], mut width: u32, mut height: u32) -> Option<&'static str> {
    let avc = fourcc == b"avc1" || fourcc == b"avc3";
    let hevc = fourcc == b"hvc1" || fourcc == b"hev1";
    if !avc && !hevc {
        return Some("video is not H.264 or HEVC");
    }
    if e.len() < 86 {
        return Some("short video sample entry");
    }
    let cfg_box = if avc { b"avcC" } else { b"hvcC" };
    let min_cfg = if avc { 4 } else { 23 };
    let Some(cfg) = child(&e[86..], cfg_box) else {
        return Some(if avc { "no avcC" } else { "no hvcC" });
    };
    if cfg.len() < min_cfg || cfg[0] != 1 {
        return Some(if avc { "no avcC" } else { "no hvcC" });
    }
    if width == 0 || height == 0 {
        width = u32::from(rd16(&e[32..]));
        height = u32::from(rd16(&e[34..]));
    }
    if width > 0 && height > 0 {
        None
    } else {
        Some("no picture size")
    }
}

fn audio_entry(fourcc: &[u8], e: &[u8]) -> Option<&'static str> {
    if e.len() < 36 {
        return Some("short audio sample entry");
    }
    if fourcc == b"Opus" {
        return match child(&e[36..], b"dOps") {
            Some(dops) if dops.len() >= 11 => None,
            _ => Some("no dOps"),
        };
    }
    if fourcc != b"mp4a" {
        return Some("audio is not AAC or Opus");
    }
    let version = rd16(&e[16..]);
    let boxes = match version {
        1 => 52,
        2 => 72,
        _ => 36,
    };
    if version > 2 || e.len() < boxes {
        return Some("unknown sound sample description");
    }
    let rest = &e[boxes..];
    let Some(esds) =
        child(rest, b"esds").or_else(|| child(rest, b"wave").and_then(|wave| child(wave, b"esds")))
    else {
        return Some("no esds");
    };
    let Some(asc) = esds_asc(esds) else {
        return Some("audio is not AAC");
    };
    if asc.is_empty() || asc.len() > 64 || !parse_asc(asc) {
        return Some("audio is not AAC");
    }
    None
}

fn sample_size(f: &[u8], fixed: u32, field: u32, i: usize) -> u32 {
    if fixed != 0 {
        fixed
    } else if field == 32 {
        rd32(&f[4 * i..])
    } else if field == 16 {
        u32::from(rd16(&f[2 * i..]))
    } else if field == 8 {
        u32::from(f[i])
    } else {
        u32::from((f[i / 2] >> if i.is_multiple_of(2) { 4 } else { 0 }) & 15)
    }
}

fn check_track(
    b: &Trak<'_>,
    video: bool,
    movie_timescale: u32,
    file_size: u64,
) -> Result<VideoTrack, &'static str> {
    let timescale = b.mdhd.map_or(0, timescale_of);
    let (Some(tkhd), Some(stsd)) = (b.tkhd, b.stsd) else {
        return Err("track without timescale, tkhd or stsd");
    };
    if timescale == 0 || tkhd.len() < 84 || (tkhd[0] == 1 && tkhd.len() < 96) || stsd.len() < 16 {
        return Err("track without timescale, tkhd or stsd");
    }
    let m_off = if tkhd[0] == 1 { 52 } else { 40 };
    let width = rd32(&tkhd[m_off + 36..]) >> 16;
    let height = rd32(&tkhd[m_off + 40..]) >> 16;

    let entry_len = rd32(&stsd[8..]) as usize;
    if rd32(&stsd[4..]) == 0 || entry_len < 8 || entry_len > stsd.len() - 8 {
        return Err("bad stsd");
    }
    let e = &stsd[8..8 + entry_len];
    let fourcc = &e[4..8];
    let bad = if video {
        video_entry(fourcc, e, width, height)
    } else {
        audio_entry(fourcc, e)
    };
    if let Some(reason) = bad {
        return Err(reason);
    }

    let (sz, fixed, field, n) = if let Some(stsz) = b.stsz {
        if stsz.len() < 12 {
            return Err("short stsz");
        }
        (stsz, rd32(&stsz[4..]), 32u32, rd32(&stsz[8..]))
    } else if let Some(stz2) = b.stz2.filter(|s| s.len() >= 12) {
        let field = u32::from(stz2[7]);
        let n = rd32(&stz2[8..]);
        if field != 4 && field != 8 && field != 16 {
            return Err("bad stz2 field size");
        }
        (stz2, 0u32, field, n)
    } else {
        return Err("no stsz");
    };
    if n == 0 {
        return if video {
            Err("video track without samples")
        } else {
            Ok(VideoTrack {
                timescale,
                edit_start: 0,
                samples: Vec::new(),
            })
        };
    }
    if n > SAMPLES_MAX {
        return Err("over 4 million samples in a track");
    }
    if fixed == 0 && u64::from(n) * u64::from(field) > ((sz.len() - 12) as u64) * 8 {
        return Err("stsz shorter than its count");
    }
    let f = &sz[12..];
    for i in 0..n as usize {
        if sample_size(f, fixed, field, i) >= 1 << 31 {
            return Err("sample over 2 GiB");
        }
    }

    let mut samples = if video {
        vec![VideoSample::default(); n as usize]
    } else {
        Vec::new()
    };

    let Some(ne) = table(b.stts, 8) else {
        return Err("bad stts");
    };
    let stts = b.stts.unwrap_or_default();
    let mut s = 0u32;
    let mut dts = 0u64;
    for i in 0..ne as usize {
        if s >= n {
            break;
        }
        let count = rd32(&stts[8 + 8 * i..]);
        let delta = u64::from(rd32(&stts[12 + 8 * i..]));
        for _ in 0..count {
            if s >= n {
                break;
            }
            if video {
                samples[s as usize].dts = dts;
            }
            dts = dts.wrapping_add(delta);
            s += 1;
        }
    }
    if s != n {
        return Err("stts covers fewer samples than stsz");
    }
    let end_dts = dts;

    if let Some(ctts) = b.ctts {
        let Some(ne) = table(Some(ctts), 8) else {
            return Err("bad ctts");
        };
        if video {
            let mut s = 0usize;
            for i in 0..ne as usize {
                if s >= n as usize {
                    break;
                }
                let count = rd32(&ctts[8 + 8 * i..]);
                let off = rd32(&ctts[12 + 8 * i..]) as i32;
                for _ in 0..count {
                    if s >= n as usize {
                        break;
                    }
                    samples[s].cto = off;
                    s += 1;
                }
            }
        }
    }

    if let Some(stss) = b.stss {
        let Some(ne) = table(Some(stss), 4) else {
            return Err("bad stss");
        };
        for i in 0..ne as usize {
            let num = rd32(&stss[8 + 4 * i..]);
            if num == 0 || num > n {
                return Err("stss names a sample that does not exist");
            }
            if video {
                samples[(num - 1) as usize].sync = true;
            }
        }
    } else if video {
        for sample in &mut samples {
            sample.sync = true;
        }
    }

    let wide = b.stco.is_none();
    let co_opt = if wide { b.co64 } else { b.stco };
    let (Some(co), Some(nchunks), Some(stsc), Some(nsc)) = (
        co_opt,
        table(co_opt, if wide { 8 } else { 4 }),
        b.stsc,
        table(b.stsc, 12),
    ) else {
        return Err("bad stsc, stco or co64");
    };
    if nsc == 0 {
        return Err("bad stsc, stco or co64");
    }
    let mut s = 0u32;
    for i in 0..nsc as usize {
        if s >= n {
            break;
        }
        let x = &stsc[8 + 12 * i..];
        let first = rd32(x);
        let per = rd32(&x[4..]);
        let next = if i + 1 < nsc as usize {
            rd32(&x[12..])
        } else {
            nchunks + 1
        };
        if first == 0 || (i == 0 && first != 1) || next <= first || next > nchunks + 1 || per == 0 {
            return Err("bad stsc");
        }
        if rd32(&x[8..]) != 1 {
            return Err("more than one sample description");
        }
        for ch in first..next {
            if s >= n {
                break;
            }
            let mut off = if wide {
                rd64(&co[8 + 8 * (ch - 1) as usize..])
            } else {
                u64::from(rd32(&co[8 + 4 * (ch - 1) as usize..]))
            };
            for _ in 0..per {
                if s >= n {
                    break;
                }
                let size = u64::from(sample_size(f, fixed, field, s as usize));
                if off > file_size || size > file_size - off {
                    return Err("sample past the end of the file");
                }
                off += size;
                s += 1;
            }
        }
    }
    if s != n {
        return Err("chunks hold fewer samples than stsz");
    }

    let mut edit_start = 0u64;
    if let Some(elst) = b.elst {
        let v1 = elst.first() == Some(&1);
        let entry_len = if v1 { 20 } else { 12 };
        let Some(ne) = table(Some(elst), entry_len) else {
            return Err("bad elst");
        };
        let mut delay = 0u128;
        for i in 0..ne as usize {
            let x = &elst[8 + i * entry_len..];
            let dur = if v1 { rd64(x) } else { u64::from(rd32(x)) };
            let media = if v1 {
                rd64(&x[8..]) as i64
            } else {
                i64::from(rd32(&x[4..]) as i32)
            };
            if media == -1 {
                delay += u128::from(dur);
                continue;
            }
            if media < 0 || (media as u64) > end_dts {
                return Err("edit outside the track");
            }
            edit_start = media as u64;
            break;
        }
        if movie_timescale > 0 && delay > 0 {
            delay = delay * u128::from(timescale) / u128::from(movie_timescale);
            if delay > u128::from(u32::MAX) {
                return Err("edit delay too long");
            }
        }
    }

    let mut lo = 0i64;
    let mut hi = 0i64;
    if video {
        for sample in &samples {
            lo = lo.min(i64::from(sample.cto));
            hi = hi.max(i64::from(sample.cto));
        }
    }
    if hi - lo > i64::from(i32::MAX) {
        return Err("composition offsets out of range");
    }

    Ok(VideoTrack {
        timescale,
        edit_start,
        samples,
    })
}

fn media_pts(sample: &VideoSample) -> i64 {
    (sample.dts as i64).wrapping_add(i64::from(sample.cto))
}

fn parse_moov(mut moov: &[u8], file_size: u64) -> Option<&'static str> {
    let mut video: Option<(Trak<'_>, bool)> = None;
    let mut audio: Option<(Trak<'_>, bool)> = None;
    let mut movie_timescale = 0u32;
    let mut traks = 0usize;
    while let Some((kind, b)) = next_box(&mut moov) {
        if &kind == b"mvex" {
            return Some("fragmented MP4");
        }
        if &kind == b"mvhd" {
            movie_timescale = timescale_of(b);
        }
        if &kind != b"trak" {
            continue;
        }
        traks += 1;
        if traks > TRAKS_MAX {
            return Some("more than 16 tracks");
        }
        let t = trak_boxes(b);
        let (Some(hdlr), Some(tkhd)) = (t.hdlr, t.tkhd) else {
            continue;
        };
        if hdlr.len() < 12 || tkhd.len() < 4 {
            continue;
        }
        let enabled = (tkhd[3] & 1) != 0;
        if &hdlr[8..12] == b"vide" {
            if video.is_none_or(|(_, prev_enabled)| enabled && !prev_enabled) {
                video = Some((t, enabled));
            }
        } else if &hdlr[8..12] == b"soun"
            && audio.is_none_or(|(_, prev_enabled)| enabled && !prev_enabled)
        {
            audio = Some((t, enabled));
        }
    }
    let Some((video_trak, _)) = video else {
        return Some("no video track");
    };
    let v = match check_track(&video_trak, true, movie_timescale, file_size) {
        Ok(v) => v,
        Err(reason) => return Some(reason),
    };
    if let Some((audio_trak, _)) = audio
        && let Err(reason) = check_track(&audio_trak, false, movie_timescale, file_size)
    {
        return Some(reason);
    }
    if !v.samples.iter().any(|s| s.sync) {
        return Some("no video keyframe");
    }

    let mut first = 0usize;
    while !v.samples[first].sync {
        first += 1;
    }
    let edit_start = v.edit_start as i64;
    let trim = media_pts(&v.samples[first]) < edit_start;
    while trim
        && first < v.samples.len()
        && !(v.samples[first].sync && media_pts(&v.samples[first]) >= edit_start)
    {
        first += 1;
    }
    if first == v.samples.len() {
        return Some("no keyframe from the video edit's start on");
    }
    let step = SEGMENT_S * u64::from(v.timescale);
    let mut n = 1u32;
    let mut start = first;
    for i in (first + 1)..v.samples.len() {
        if v.samples[i].sync && v.samples[i].dts.wrapping_sub(v.samples[start].dts) >= step {
            n += 1;
            start = i;
        }
    }
    if n > SEGMENTS_MAX {
        return Some("over 16384 segments");
    }
    None
}

fn read_at(file: &mut (impl Read + Seek), off: u64, buf: &mut [u8]) -> bool {
    file.seek(SeekFrom::Start(off)).is_ok() && file.read_exact(buf).is_ok()
}

pub(crate) fn hls_rejection(file: &mut (impl Read + Seek), size: u64) -> Option<&'static str> {
    let mut off = 0u64;
    let mut moov_off = 0u64;
    let mut moov_len = 0u64;
    let mut boxes = 0usize;
    while moov_len == 0 {
        if off >= size {
            return Some("no moov box");
        }
        let left = size - off;
        if boxes == TOP_BOXES_MAX || left < 8 {
            return Some("bad top-level boxes");
        }
        let mut h = [0u8; 16];
        if !read_at(file, off, &mut h[..8]) {
            return Some("unreadable");
        }
        let mut box_size = u64::from(rd32(&h));
        let mut hdr = 8u64;
        if box_size == 1 {
            if left < 16 {
                return Some("bad top-level boxes");
            }
            if !read_at(file, off + 8, &mut h[8..16]) {
                return Some("unreadable");
            }
            box_size = rd64(&h[8..]);
            hdr = 16;
        } else if box_size == 0 {
            box_size = left;
        }
        let kind = &h[4..8];
        let known = TOP_BOXES.iter().any(|top| *top == kind);
        if kind == b"moof" || kind == b"styp" || kind == b"sidx" {
            return Some("fragmented MP4");
        }
        if boxes == 0 && !known {
            return Some("not an MP4 or MOV file");
        }
        if box_size < hdr || box_size > left {
            return Some("box past the end of the file");
        }
        if kind == b"moov" {
            moov_off = off + hdr;
            moov_len = box_size - hdr;
            if moov_len == 0 {
                return Some("empty moov");
            }
        }
        off += box_size;
        boxes += 1;
    }
    if moov_len > MOOV_MAX {
        return Some("moov over 64 MiB");
    }
    let mut moov = vec![0u8; moov_len as usize];
    if !read_at(file, moov_off, &mut moov) {
        return Some("unreadable");
    }
    parse_moov(&moov, size)
}

pub(crate) async fn hls_rejection_at(path: PathBuf, size: u64) -> Option<&'static str> {
    crate::transport_runtime::runtime()
        .spawn_blocking(move || match std::fs::File::open(&path) {
            Ok(mut file) => hls_rejection(&mut file, size),
            Err(_) => Some("unreadable"),
        })
        .await
        .unwrap_or(Some("unreadable"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn full_box(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + payload.len());
        body.push(version);
        body.extend_from_slice(&flags.to_be_bytes()[1..4]);
        body.extend_from_slice(payload);
        mp4_box(kind, &body)
    }

    fn mvhd(timescale: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 20];
        payload[8..12].copy_from_slice(&timescale.to_be_bytes());
        full_box(b"mvhd", 0, 0, &payload)
    }

    fn tkhd(enabled: bool, width: u16, height: u16) -> Vec<u8> {
        let mut payload = vec![0u8; 80];
        payload[36..40].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        payload[52..56].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        payload[68..72].copy_from_slice(&0x4000_0000u32.to_be_bytes());
        payload[72..76].copy_from_slice(&(u32::from(width) << 16).to_be_bytes());
        payload[76..80].copy_from_slice(&(u32::from(height) << 16).to_be_bytes());
        full_box(b"tkhd", 0, u32::from(enabled), &payload)
    }

    fn mdhd(timescale: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 20];
        payload[8..12].copy_from_slice(&timescale.to_be_bytes());
        full_box(b"mdhd", 0, 0, &payload)
    }

    fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut payload = vec![0u8; 20];
        payload[4..8].copy_from_slice(handler);
        full_box(b"hdlr", 0, 0, &payload)
    }

    fn stsd(entry: &[u8]) -> Vec<u8> {
        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.extend_from_slice(entry);
        full_box(b"stsd", 0, 0, &payload)
    }

    fn avc1_entry() -> Vec<u8> {
        let mut body = vec![0u8; 78];
        body[24..26].copy_from_slice(&1920u16.to_be_bytes());
        body[26..28].copy_from_slice(&1080u16.to_be_bytes());
        body.extend_from_slice(&mp4_box(b"avcC", &[1, 0x64, 0, 0x1f]));
        mp4_box(b"avc1", &body)
    }

    fn hvc1_entry() -> Vec<u8> {
        let mut body = vec![0u8; 78];
        body[24..26].copy_from_slice(&1920u16.to_be_bytes());
        body[26..28].copy_from_slice(&1080u16.to_be_bytes());
        let mut hvcc = vec![0u8; 23];
        hvcc[0] = 1;
        body.extend_from_slice(&mp4_box(b"hvcC", &hvcc));
        mp4_box(b"hvc1", &body)
    }

    fn esds_box(oti: u8, asc: &[u8]) -> Vec<u8> {
        let mut dec_cfg = vec![oti, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        dec_cfg.push(0x05);
        dec_cfg.push(asc.len() as u8);
        dec_cfg.extend_from_slice(asc);
        let mut es = vec![0, 1, 0, 0x04, dec_cfg.len() as u8];
        es.extend_from_slice(&dec_cfg);
        let mut payload = vec![0x03, es.len() as u8];
        payload.extend_from_slice(&es);
        full_box(b"esds", 0, 0, &payload)
    }

    fn mp4a_entry(oti: u8) -> Vec<u8> {
        let mut body = vec![0u8; 28];
        body[16..18].copy_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&esds_box(oti, &[0x11, 0x90]));
        mp4_box(b"mp4a", &body)
    }

    fn mp4a_v1_wave_entry() -> Vec<u8> {
        let mut body = vec![0u8; 44];
        body[8..10].copy_from_slice(&1u16.to_be_bytes());
        body[16..18].copy_from_slice(&2u16.to_be_bytes());
        let wave = mp4_box(b"wave", &esds_box(0x40, &[0x11, 0x90]));
        body.extend_from_slice(&wave);
        mp4_box(b"mp4a", &body)
    }

    fn opus_entry() -> Vec<u8> {
        let mut body = vec![0u8; 28];
        body.extend_from_slice(&mp4_box(b"dOps", &[0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        mp4_box(b"Opus", &body)
    }

    fn stsz(sizes: &[u32]) -> Vec<u8> {
        let mut payload = 0u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for s in sizes {
            payload.extend_from_slice(&s.to_be_bytes());
        }
        full_box(b"stsz", 0, 0, &payload)
    }

    fn stts(count: u32, delta: u32) -> Vec<u8> {
        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&count.to_be_bytes());
        payload.extend_from_slice(&delta.to_be_bytes());
        full_box(b"stts", 0, 0, &payload)
    }

    fn stss(syncs: &[u32]) -> Vec<u8> {
        let mut payload = (syncs.len() as u32).to_be_bytes().to_vec();
        for s in syncs {
            payload.extend_from_slice(&s.to_be_bytes());
        }
        full_box(b"stss", 0, 0, &payload)
    }

    fn stsc(per_chunk: u32, desc_index: u32) -> Vec<u8> {
        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&per_chunk.to_be_bytes());
        payload.extend_from_slice(&desc_index.to_be_bytes());
        full_box(b"stsc", 0, 0, &payload)
    }

    fn stco(offsets: &[u32]) -> Vec<u8> {
        let mut payload = (offsets.len() as u32).to_be_bytes().to_vec();
        for o in offsets {
            payload.extend_from_slice(&o.to_be_bytes());
        }
        full_box(b"stco", 0, 0, &payload)
    }

    fn elst_v0(duration: u32, media_time: i32) -> Vec<u8> {
        let mut payload = 1u32.to_be_bytes().to_vec();
        payload.extend_from_slice(&duration.to_be_bytes());
        payload.extend_from_slice(&media_time.to_be_bytes());
        payload.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        full_box(b"elst", 0, 0, &payload)
    }

    struct TrakSpec {
        handler: [u8; 4],
        enabled: bool,
        entry: Vec<u8>,
        sizes: Vec<u32>,
        delta: u32,
        syncs: Option<Vec<u32>>,
        desc_index: u32,
        chunk_offset: u32,
        elst: Option<Vec<u8>>,
    }

    impl TrakSpec {
        fn video(entry: Vec<u8>) -> Self {
            Self {
                handler: *b"vide",
                enabled: true,
                entry,
                sizes: vec![16, 16],
                delta: 1000,
                syncs: Some(vec![1]),
                desc_index: 1,
                chunk_offset: 32,
                elst: None,
            }
        }

        fn audio(entry: Vec<u8>, enabled: bool) -> Self {
            Self {
                handler: *b"soun",
                enabled,
                entry,
                sizes: vec![8, 8],
                delta: 1024,
                syncs: None,
                desc_index: 1,
                chunk_offset: 64,
                elst: None,
            }
        }

        fn to_box(&self) -> Vec<u8> {
            let mut stbl_body = Vec::new();
            stbl_body.extend_from_slice(&stsd(&self.entry));
            stbl_body.extend_from_slice(&stts(self.sizes.len() as u32, self.delta));
            if let Some(ref syncs) = self.syncs {
                stbl_body.extend_from_slice(&stss(syncs));
            }
            stbl_body.extend_from_slice(&stsz(&self.sizes));
            stbl_body.extend_from_slice(&stsc(self.sizes.len() as u32, self.desc_index));
            stbl_body.extend_from_slice(&stco(&[self.chunk_offset]));
            let minf = mp4_box(b"minf", &mp4_box(b"stbl", &stbl_body));

            let mut mdia_body = Vec::new();
            mdia_body.extend_from_slice(&mdhd(1000));
            mdia_body.extend_from_slice(&hdlr(&self.handler));
            mdia_body.extend_from_slice(&minf);

            let mut trak_body = Vec::new();
            trak_body.extend_from_slice(&tkhd(self.enabled, 1920, 1080));
            if let Some(ref elst) = self.elst {
                trak_body.extend_from_slice(&mp4_box(b"edts", elst));
            }
            trak_body.extend_from_slice(&mp4_box(b"mdia", &mdia_body));
            mp4_box(b"trak", &trak_body)
        }
    }

    fn moov_from(traks: &[TrakSpec], extra: &[u8]) -> Vec<u8> {
        let mut body = mvhd(1000);
        for t in traks {
            body.extend_from_slice(&t.to_box());
        }
        body.extend_from_slice(extra);
        mp4_box(b"moov", &body)
    }

    fn probe_bytes(bytes: Vec<u8>) -> Option<&'static str> {
        let size = bytes.len() as u64;
        hls_rejection(&mut Cursor::new(bytes), size)
    }

    #[test]
    fn accepts_supported_mp4_and_mov_layouts() {
        let traks = [
            TrakSpec::video(avc1_entry()),
            TrakSpec::audio(mp4a_entry(0x40), true),
        ];
        let mut moov_first = mp4_box(b"ftyp", &[0; 16]);
        let moov = moov_from(&traks, &[]);
        let mdat_off = (moov_first.len() + moov.len() + 8) as u32;
        let traks_moov_first = [
            TrakSpec {
                chunk_offset: mdat_off,
                ..TrakSpec::video(avc1_entry())
            },
            TrakSpec {
                chunk_offset: mdat_off + 32,
                ..TrakSpec::audio(mp4a_entry(0x40), true)
            },
        ];
        moov_first.extend_from_slice(&moov_from(&traks_moov_first, &[]));
        moov_first.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        assert_eq!(probe_bytes(moov_first), None);

        let mut moov_after = mp4_box(b"ftyp", &[0; 16]);
        moov_after.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        moov_after.extend_from_slice(&moov);
        assert_eq!(probe_bytes(moov_after), None);

        let mut mov_no_ftyp = mp4_box(b"wide", &[]);
        mov_no_ftyp.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        mov_no_ftyp.extend_from_slice(&moov);
        assert_eq!(probe_bytes(mov_no_ftyp), None);

        let mut hevc_only = mp4_box(b"ftyp", &[0; 16]);
        hevc_only.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        hevc_only.extend_from_slice(&moov_from(&[TrakSpec::video(hvc1_entry())], &[]));
        assert_eq!(probe_bytes(hevc_only), None);

        let mut opus = mp4_box(b"ftyp", &[0; 16]);
        opus.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        opus.extend_from_slice(&moov_from(
            &[
                TrakSpec::video(avc1_entry()),
                TrakSpec::audio(opus_entry(), true),
            ],
            &[],
        ));
        assert_eq!(probe_bytes(opus), None);

        let mut qt_wave = mp4_box(b"ftyp", &[0; 16]);
        qt_wave.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        qt_wave.extend_from_slice(&moov_from(
            &[
                TrakSpec::video(avc1_entry()),
                TrakSpec::audio(mp4a_v1_wave_entry(), true),
            ],
            &[],
        ));
        assert_eq!(probe_bytes(qt_wave), None);

        let mut two_audio = mp4_box(b"ftyp", &[0; 16]);
        two_audio.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        two_audio.extend_from_slice(&moov_from(
            &[
                TrakSpec::video(avc1_entry()),
                TrakSpec::audio(mp4a_entry(0x6B), false),
                TrakSpec::audio(mp4a_entry(0x40), true),
            ],
            &[],
        ));
        assert_eq!(probe_bytes(two_audio), None);
    }

    #[test]
    fn rejects_unsupported_containers_and_fragmented_files() {
        let ebml = vec![0x1A, 0x45, 0xDF, 0xA3, 0x93, 0x42, 0x82, 0x88, b'm', b'a'];
        assert_eq!(probe_bytes(ebml), Some("not an MP4 or MOV file"));

        let mut top_moof = mp4_box(b"ftyp", &[0; 16]);
        top_moof.extend_from_slice(&mp4_box(b"moof", &[0; 8]));
        assert_eq!(probe_bytes(top_moof), Some("fragmented MP4"));

        let mut mvex_in_moov = mp4_box(b"ftyp", &[0; 16]);
        mvex_in_moov.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        mvex_in_moov.extend_from_slice(&moov_from(
            &[TrakSpec::video(avc1_entry())],
            &mp4_box(b"mvex", &[]),
        ));
        assert_eq!(probe_bytes(mvex_in_moov), Some("fragmented MP4"));
    }

    #[test]
    fn rejects_unsupported_codecs_and_invalid_track_tables() {
        let vp09_entry = mp4_box(b"vp09", &[0u8; 86]);
        let mut vp09 = mp4_box(b"ftyp", &[0; 16]);
        vp09.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        vp09.extend_from_slice(&moov_from(&[TrakSpec::video(vp09_entry)], &[]));
        assert_eq!(probe_bytes(vp09), Some("video is not H.264 or HEVC"));

        let mut mp3_in_mp4a = mp4_box(b"ftyp", &[0; 16]);
        mp3_in_mp4a.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        mp3_in_mp4a.extend_from_slice(&moov_from(
            &[
                TrakSpec::video(avc1_entry()),
                TrakSpec::audio(mp4a_entry(0x6B), true),
            ],
            &[],
        ));
        assert_eq!(probe_bytes(mp3_in_mp4a), Some("audio is not AAC"));

        let ac3_entry = mp4_box(b"ac-3", &[0u8; 36]);
        let mut ac3 = mp4_box(b"ftyp", &[0; 16]);
        ac3.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        ac3.extend_from_slice(&moov_from(
            &[
                TrakSpec::video(avc1_entry()),
                TrakSpec::audio(ac3_entry, true),
            ],
            &[],
        ));
        assert_eq!(probe_bytes(ac3), Some("audio is not AAC or Opus"));

        let mut no_video = mp4_box(b"ftyp", &[0; 16]);
        no_video.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        no_video.extend_from_slice(&moov_from(&[TrakSpec::audio(mp4a_entry(0x40), true)], &[]));
        assert_eq!(probe_bytes(no_video), Some("no video track"));

        let many_traks: Vec<TrakSpec> = (0..17).map(|_| TrakSpec::video(avc1_entry())).collect();
        let mut too_many = mp4_box(b"ftyp", &[0; 16]);
        too_many.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        too_many.extend_from_slice(&moov_from(&many_traks, &[]));
        assert_eq!(probe_bytes(too_many), Some("more than 16 tracks"));

        let mut desc2 = TrakSpec::video(avc1_entry());
        desc2.desc_index = 2;
        let mut bad_desc = mp4_box(b"ftyp", &[0; 16]);
        bad_desc.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        bad_desc.extend_from_slice(&moov_from(&[desc2], &[]));
        assert_eq!(
            probe_bytes(bad_desc),
            Some("more than one sample description")
        );

        let mut no_sync = TrakSpec::video(avc1_entry());
        no_sync.syncs = Some(Vec::new());
        let mut no_keyframe = mp4_box(b"ftyp", &[0; 16]);
        no_keyframe.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        no_keyframe.extend_from_slice(&moov_from(&[no_sync], &[]));
        assert_eq!(probe_bytes(no_keyframe), Some("no video keyframe"));

        let mut past_eof_trak = TrakSpec::video(avc1_entry());
        past_eof_trak.chunk_offset = 10_000;
        let mut past_eof = mp4_box(b"ftyp", &[0; 16]);
        past_eof.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        past_eof.extend_from_slice(&moov_from(&[past_eof_trak], &[]));
        assert_eq!(
            probe_bytes(past_eof),
            Some("sample past the end of the file")
        );

        let big_moov_len = (MOOV_MAX + 1) as u32 + 8;
        let mut big_moov = Vec::new();
        big_moov.extend_from_slice(&big_moov_len.to_be_bytes());
        big_moov.extend_from_slice(b"moov");
        assert_eq!(
            hls_rejection(&mut Cursor::new(big_moov), u64::from(big_moov_len)),
            Some("moov over 64 MiB")
        );

        let mut trimmed_trak = TrakSpec::video(avc1_entry());
        trimmed_trak.sizes = vec![16, 16, 16];
        trimmed_trak.syncs = Some(vec![1]);
        trimmed_trak.elst = Some(elst_v0(1000, 1500));
        let mut trimmed = mp4_box(b"ftyp", &[0; 16]);
        trimmed.extend_from_slice(&mp4_box(b"mdat", &[0; 64]));
        trimmed.extend_from_slice(&moov_from(&[trimmed_trak], &[]));
        assert_eq!(
            probe_bytes(trimmed),
            Some("no keyframe from the video edit's start on")
        );
    }
}
