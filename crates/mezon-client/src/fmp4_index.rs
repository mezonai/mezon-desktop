use std::collections::HashMap;

const MAX_BUFFERED_BOX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mp4Finalization {
    pub head_offset: u64,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Header,
    Buffer,
    Skip,
}

struct MvhdInfo {
    version: u8,
    timescale: u32,
    duration_offset: usize,
}

struct FragmentEntry {
    time: u64,
    moof_offset: u64,
    traf_index: u8,
}

struct TrackState {
    id: u32,
    timescale: u32,
    end: u128,
    fragments: Vec<FragmentEntry>,
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

fn write_duration(head: &mut [u8], offset: usize, version: u8, value: u128) -> bool {
    match version {
        0 => {
            let Ok(v) = u32::try_from(value) else {
                return false;
            };
            head[offset..offset + 4].copy_from_slice(&v.to_be_bytes());
            true
        }
        1 => {
            let Ok(v) = u64::try_from(value) else {
                return false;
            };
            head[offset..offset + 8].copy_from_slice(&v.to_be_bytes());
            true
        }
        _ => false,
    }
}

fn walk_child_boxes(
    buf: &[u8],
    start: usize,
    end: usize,
    mut visit: impl FnMut(&[u8; 4], usize, usize) -> bool,
) -> bool {
    let mut off = start;
    while off < end {
        if off + 8 > end {
            return false;
        }
        let size32 = read_u32(buf, off) as usize;
        let kind: [u8; 4] = [buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]];
        let (hdr, size) = if size32 == 1 {
            if off + 16 > end {
                return false;
            }
            let size64 = read_u64(buf, off + 8);
            if size64 < 16 || size64 > (end - off) as u64 {
                return false;
            }
            (16, size64 as usize)
        } else if size32 == 0 {
            (8, end - off)
        } else if size32 < 8 || off + size32 > end {
            return false;
        } else {
            (8, size32)
        };
        if !visit(&kind, off + hdr, off + size) {
            return false;
        }
        off += size;
    }
    off == end
}

fn scale_rounded(value: u128, movie_timescale: u32, media_timescale: u32) -> Option<u128> {
    let num = value.checked_mul(u128::from(movie_timescale))?;
    let den = u128::from(media_timescale);
    let twice_num = num.checked_mul(2)?;
    let numer = twice_num.checked_add(den)?;
    let denom = den.checked_mul(2)?;
    numer.checked_div(denom)
}

#[derive(Default)]
pub(crate) struct Fmp4Index {
    disabled: bool,
    saw_ftyp: bool,
    state: State,
    file_offset: u64,
    box_offset: u64,
    box_type: [u8; 4],
    box_header_size: usize,
    header_buf: [u8; 16],
    header_have: usize,
    extended_header: bool,
    payload_buf: Vec<u8>,
    remaining: u64,
    moov_offset: u64,
    moov_bytes: Option<Vec<u8>>,
    mvhd: Option<MvhdInfo>,
    tracks: Vec<TrackState>,
    track_by_id: HashMap<u32, usize>,
    trex_defaults: HashMap<u32, u32>,
}

impl Fmp4Index {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if self.disabled || bytes.is_empty() {
            return;
        }
        let mut pos = 0;
        while pos < bytes.len() && !self.disabled {
            match self.state {
                State::Header => {
                    if self.header_have == 0 {
                        self.box_offset = self.file_offset;
                    }
                    let header_need = if self.extended_header { 16 } else { 8 };
                    let take = (header_need - self.header_have).min(bytes.len() - pos);
                    self.header_buf[self.header_have..self.header_have + take]
                        .copy_from_slice(&bytes[pos..pos + take]);
                    self.header_have += take;
                    pos += take;
                    self.file_offset += take as u64;
                    if self.header_have < header_need {
                        return;
                    }
                    let box_size: u64;
                    if !self.extended_header {
                        let size32 = u64::from(read_u32(&self.header_buf, 0));
                        let kind: [u8; 4] = [
                            self.header_buf[4],
                            self.header_buf[5],
                            self.header_buf[6],
                            self.header_buf[7],
                        ];
                        if !self.saw_ftyp && &kind != b"ftyp" {
                            self.disable();
                            return;
                        }
                        self.box_type = kind;
                        if size32 == 1 {
                            self.extended_header = true;
                            continue;
                        }
                        if size32 < 8 {
                            self.disable();
                            return;
                        }
                        self.box_header_size = 8;
                        box_size = size32;
                    } else {
                        let size64 = read_u64(&self.header_buf, 8);
                        if size64 < 16 {
                            self.disable();
                            return;
                        }
                        self.box_header_size = 16;
                        box_size = size64;
                    }
                    self.saw_ftyp = true;
                    let body_bytes = box_size - self.box_header_size as u64;
                    self.header_have = 0;
                    self.extended_header = false;
                    if &self.box_type == b"moov" || &self.box_type == b"moof" {
                        if box_size > MAX_BUFFERED_BOX_BYTES as u64 {
                            self.disable();
                            return;
                        }
                        let mut buf = Vec::with_capacity(box_size as usize);
                        buf.extend_from_slice(&self.header_buf[..self.box_header_size]);
                        self.payload_buf = buf;
                        self.remaining = body_bytes;
                        if body_bytes == 0 {
                            self.complete_buffered_box();
                        } else {
                            self.state = State::Buffer;
                        }
                    } else if body_bytes > 0 {
                        self.remaining = body_bytes;
                        self.state = State::Skip;
                    }
                }
                State::Buffer => {
                    let avail = (bytes.len() - pos) as u64;
                    let take = self.remaining.min(avail) as usize;
                    self.payload_buf.extend_from_slice(&bytes[pos..pos + take]);
                    self.remaining -= take as u64;
                    pos += take;
                    self.file_offset += take as u64;
                    if self.remaining == 0 {
                        self.state = State::Header;
                        self.complete_buffered_box();
                    }
                }
                State::Skip => {
                    let avail = (bytes.len() - pos) as u64;
                    let take = self.remaining.min(avail) as usize;
                    self.remaining -= take as u64;
                    pos += take;
                    self.file_offset += take as u64;
                    if self.remaining == 0 {
                        self.state = State::Header;
                    }
                }
            }
        }
    }

    pub(crate) fn finish(&self) -> Option<Mp4Finalization> {
        if self.disabled || !self.saw_ftyp {
            return None;
        }
        let moov_bytes = self.moov_bytes.as_ref()?;
        let mvhd = self.mvhd.as_ref()?;
        if self.state != State::Header || self.header_have != 0 || self.tracks.is_empty() {
            return None;
        }
        if self.tracks.iter().any(|track| track.fragments.is_empty()) {
            return None;
        }

        let mut head = moov_bytes.clone();
        let mut movie_duration: u128 = 0;
        for track in &self.tracks {
            let scaled = scale_rounded(track.end, mvhd.timescale, track.timescale)?;
            if scaled > movie_duration {
                movie_duration = scaled;
            }
        }

        if !write_duration(
            &mut head,
            mvhd.duration_offset,
            mvhd.version,
            movie_duration,
        ) {
            return None;
        }

        let mut tfras_size: u64 = 0;
        for track in &self.tracks {
            if track.fragments.len() as u64 > u64::from(u32::MAX) {
                return None;
            }
            tfras_size = tfras_size.checked_add(24 + (track.fragments.len() as u64) * 19)?;
        }
        let mfra_size_u64 = 8u64.checked_add(tfras_size)?.checked_add(16)?;
        let mfra_size = u32::try_from(mfra_size_u64).ok()?;

        let mut tail = Vec::with_capacity(mfra_size as usize);
        tail.extend_from_slice(&mfra_size.to_be_bytes());
        tail.extend_from_slice(b"mfra");

        for track in &self.tracks {
            let tfra_size = (24 + track.fragments.len() * 19) as u32;
            tail.extend_from_slice(&tfra_size.to_be_bytes());
            tail.extend_from_slice(b"tfra");
            tail.extend_from_slice(&0x0100_0000u32.to_be_bytes());
            tail.extend_from_slice(&track.id.to_be_bytes());
            tail.extend_from_slice(&0u32.to_be_bytes());
            tail.extend_from_slice(&(track.fragments.len() as u32).to_be_bytes());
            for frag in &track.fragments {
                tail.extend_from_slice(&frag.time.to_be_bytes());
                tail.extend_from_slice(&frag.moof_offset.to_be_bytes());
                tail.push(frag.traf_index);
                tail.push(1);
                tail.push(1);
            }
        }

        tail.extend_from_slice(&16u32.to_be_bytes());
        tail.extend_from_slice(b"mfro");
        tail.extend_from_slice(&0u32.to_be_bytes());
        tail.extend_from_slice(&mfra_size.to_be_bytes());

        Some(Mp4Finalization {
            head_offset: self.moov_offset,
            head,
            tail,
        })
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.payload_buf.clear();
        self.moov_bytes = None;
        self.tracks.clear();
        self.track_by_id.clear();
    }

    fn complete_buffered_box(&mut self) {
        let buf = std::mem::take(&mut self.payload_buf);
        if &self.box_type == b"moov" {
            if !self.parse_moov(buf, self.box_header_size) {
                self.disable();
            }
        } else if &self.box_type == b"moof" && !self.parse_moof(&buf, self.box_header_size) {
            self.disable();
        }
    }

    fn parse_moov(&mut self, buf: Vec<u8>, header_size: usize) -> bool {
        if self.moov_bytes.is_some() {
            return false;
        }
        let moov_offset = self.box_offset;
        let mut mvhd: Option<MvhdInfo> = None;
        let mut tracks: Vec<TrackState> = Vec::new();
        let mut track_by_id: HashMap<u32, usize> = HashMap::new();
        let mut trex_defaults: HashMap<u32, u32> = HashMap::new();

        let ok = walk_child_boxes(&buf, header_size, buf.len(), |kind, body, end| {
            if kind == b"mvhd" {
                if body + 4 > end {
                    return false;
                }
                let version = buf[body];
                if version != 0 && version != 1 {
                    return false;
                }
                let min_body = if version == 0 { 20 } else { 32 };
                if body + min_body > end {
                    return false;
                }
                let timescale = read_u32(&buf, body + if version == 0 { 12 } else { 20 });
                if timescale == 0 {
                    return false;
                }
                mvhd = Some(MvhdInfo {
                    version,
                    timescale,
                    duration_offset: body + if version == 0 { 16 } else { 24 },
                });
                return true;
            }
            if kind == b"trak" {
                let mut tkhd_id: Option<u32> = None;
                let mut mdhd_timescale: Option<u32> = None;
                let trak_ok =
                    walk_child_boxes(&buf, body, end, |child_type, child_body, child_end| {
                        if child_type == b"tkhd" {
                            if child_body + 4 > child_end {
                                return false;
                            }
                            let version = buf[child_body];
                            if version != 0 && version != 1 {
                                return false;
                            }
                            let min_body = if version == 0 { 24 } else { 36 };
                            if child_body + min_body > child_end {
                                return false;
                            }
                            tkhd_id = Some(read_u32(
                                &buf,
                                child_body + if version == 0 { 12 } else { 20 },
                            ));
                            return true;
                        }
                        if child_type == b"mdia" {
                            return walk_child_boxes(
                                &buf,
                                child_body,
                                child_end,
                                |mdia_type, mdia_body, mdia_end| {
                                    if mdia_type != b"mdhd" {
                                        return true;
                                    }
                                    if mdia_body + 4 > mdia_end {
                                        return false;
                                    }
                                    let version = buf[mdia_body];
                                    if version != 0 && version != 1 {
                                        return false;
                                    }
                                    let min_body = if version == 0 { 20 } else { 32 };
                                    if mdia_body + min_body > mdia_end {
                                        return false;
                                    }
                                    let timescale = read_u32(
                                        &buf,
                                        mdia_body + if version == 0 { 12 } else { 20 },
                                    );
                                    if timescale == 0 {
                                        return false;
                                    }
                                    mdhd_timescale = Some(timescale);
                                    true
                                },
                            );
                        }
                        true
                    });
                let (true, Some(id), Some(timescale)) = (trak_ok, tkhd_id, mdhd_timescale) else {
                    return false;
                };
                if track_by_id.contains_key(&id) {
                    return false;
                }
                let idx = tracks.len();
                tracks.push(TrackState {
                    id,
                    timescale,
                    end: 0,
                    fragments: Vec::new(),
                });
                track_by_id.insert(id, idx);
                return true;
            }
            if kind == b"mvex" {
                return walk_child_boxes(&buf, body, end, |mvex_type, mvex_body, mvex_end| {
                    if mvex_type != b"trex" {
                        return true;
                    }
                    if mvex_body + 16 > mvex_end {
                        return false;
                    }
                    let track_id = read_u32(&buf, mvex_body + 4);
                    let default_duration = read_u32(&buf, mvex_body + 12);
                    trex_defaults.insert(track_id, default_duration);
                    true
                });
            }
            true
        });

        let (true, Some(mvhd)) = (ok && !tracks.is_empty(), mvhd) else {
            return false;
        };
        self.moov_offset = moov_offset;
        self.moov_bytes = Some(buf);
        self.mvhd = Some(mvhd);
        self.tracks = tracks;
        self.track_by_id = track_by_id;
        self.trex_defaults.extend(trex_defaults);
        true
    }

    fn parse_moof(&mut self, buf: &[u8], header_size: usize) -> bool {
        if self.moov_bytes.is_none() {
            return false;
        }
        let moof_offset = self.box_offset;
        let mut traf_index: u16 = 0;

        walk_child_boxes(buf, header_size, buf.len(), |kind, body, end| {
            if kind != b"traf" {
                return true;
            }
            traf_index += 1;
            let Ok(traf_number) = u8::try_from(traf_index) else {
                return false;
            };

            let mut track_id: Option<u32> = None;
            let mut default_duration: Option<u32> = None;
            let mut base: Option<u64> = None;
            let mut explicit_total: u128 = 0;
            let mut default_sample_count: u128 = 0;

            let traf_ok = walk_child_boxes(buf, body, end, |child_type, child_body, child_end| {
                if child_type == b"tfhd" {
                    if child_body + 8 > child_end {
                        return false;
                    }
                    let flags = read_u32(buf, child_body) & 0x00ff_ffff;
                    track_id = Some(read_u32(buf, child_body + 4));
                    let mut p = child_body + 8;
                    if flags & 0x1 != 0 {
                        p += 8;
                    }
                    if flags & 0x2 != 0 {
                        p += 4;
                    }
                    if flags & 0x8 != 0 {
                        if p + 4 > child_end {
                            return false;
                        }
                        default_duration = Some(read_u32(buf, p));
                        p += 4;
                    }
                    return p <= child_end;
                }
                if child_type == b"tfdt" {
                    if child_body + 4 > child_end {
                        return false;
                    }
                    let version = buf[child_body];
                    if version == 0 {
                        if child_body + 8 > child_end {
                            return false;
                        }
                        base = Some(u64::from(read_u32(buf, child_body + 4)));
                        return true;
                    }
                    if version == 1 {
                        if child_body + 12 > child_end {
                            return false;
                        }
                        base = Some(read_u64(buf, child_body + 4));
                        return true;
                    }
                    return false;
                }
                if child_type == b"trun" {
                    if child_body + 8 > child_end {
                        return false;
                    }
                    let flags = read_u32(buf, child_body) & 0x00ff_ffff;
                    let count = read_u32(buf, child_body + 4) as usize;
                    let mut p = child_body + 8;
                    if flags & 0x1 != 0 {
                        p += 4;
                    }
                    if flags & 0x4 != 0 {
                        p += 4;
                    }
                    let mut per_sample = 0usize;
                    if flags & 0x100 != 0 {
                        per_sample += 4;
                    }
                    if flags & 0x200 != 0 {
                        per_sample += 4;
                    }
                    if flags & 0x400 != 0 {
                        per_sample += 4;
                    }
                    if flags & 0x800 != 0 {
                        per_sample += 4;
                    }
                    let Some(samples_len) = count.checked_mul(per_sample) else {
                        return false;
                    };
                    if p + samples_len > child_end {
                        return false;
                    }
                    if flags & 0x100 != 0 {
                        for _ in 0..count {
                            explicit_total += u128::from(read_u32(buf, p));
                            p += per_sample;
                        }
                    } else {
                        default_sample_count += count as u128;
                    }
                    return true;
                }
                true
            });

            let (true, Some(resolved_track_id), Some(resolved_base)) = (traf_ok, track_id, base)
            else {
                return false;
            };
            if let Some(&track_idx) = self.track_by_id.get(&resolved_track_id) {
                let fallback = u128::from(
                    default_duration
                        .or_else(|| self.trex_defaults.get(&resolved_track_id).copied())
                        .unwrap_or(0),
                );
                let end_media =
                    u128::from(resolved_base) + explicit_total + default_sample_count * fallback;
                let track = &mut self.tracks[track_idx];
                track.fragments.push(FragmentEntry {
                    time: resolved_base,
                    moof_offset,
                    traf_index: traf_number,
                });
                if end_media > track.end {
                    track.end = end_media;
                }
            }
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Fmp4Index, Mp4Finalization, read_u32, read_u64};

    fn concat(parts: &[&[u8]]) -> Vec<u8> {
        let total: usize = parts.iter().map(|p| p.len()).sum();
        let mut out = Vec::with_capacity(total);
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    fn u32_be(value: u32) -> [u8; 4] {
        value.to_be_bytes()
    }

    fn u64_be(value: u64) -> [u8; 8] {
        value.to_be_bytes()
    }

    fn mp4_box(kind: &[u8; 4], payloads: &[&[u8]]) -> Vec<u8> {
        let body = concat(payloads);
        let size = (8 + body.len()) as u32;
        concat(&[&u32_be(size), kind, &body])
    }

    fn full_box(kind: &[u8; 4], version: u8, flags: u32, payloads: &[&[u8]]) -> Vec<u8> {
        let vf = (u32::from(version) << 24) | (flags & 0x00ff_ffff);
        let vf_bytes = u32_be(vf);
        let mut all: Vec<&[u8]> = Vec::with_capacity(1 + payloads.len());
        all.push(&vf_bytes);
        all.extend_from_slice(payloads);
        mp4_box(kind, &all)
    }

    fn build_mvhd(timescale: u32, duration: u64, version: u8) -> Vec<u8> {
        let pad = [0u8; 80];
        if version == 0 {
            full_box(
                b"mvhd",
                0,
                0,
                &[
                    &u32_be(0),
                    &u32_be(0),
                    &u32_be(timescale),
                    &u32_be(duration as u32),
                    &pad,
                ],
            )
        } else {
            full_box(
                b"mvhd",
                1,
                0,
                &[
                    &u64_be(0),
                    &u64_be(0),
                    &u32_be(timescale),
                    &u64_be(duration),
                    &pad,
                ],
            )
        }
    }

    fn build_tkhd(track_id: u32, duration: u64, version: u8) -> Vec<u8> {
        let pad = [0u8; 60];
        if version == 0 {
            full_box(
                b"tkhd",
                0,
                3,
                &[
                    &u32_be(0),
                    &u32_be(0),
                    &u32_be(track_id),
                    &u32_be(0),
                    &u32_be(duration as u32),
                    &pad,
                ],
            )
        } else {
            full_box(
                b"tkhd",
                1,
                3,
                &[
                    &u64_be(0),
                    &u64_be(0),
                    &u32_be(track_id),
                    &u32_be(0),
                    &u64_be(duration),
                    &pad,
                ],
            )
        }
    }

    fn build_mdhd(timescale: u32, duration: u64, version: u8) -> Vec<u8> {
        if version == 0 {
            full_box(
                b"mdhd",
                0,
                0,
                &[
                    &u32_be(0),
                    &u32_be(0),
                    &u32_be(timescale),
                    &u32_be(duration as u32),
                    &u32_be(0),
                ],
            )
        } else {
            full_box(
                b"mdhd",
                1,
                0,
                &[
                    &u64_be(0),
                    &u64_be(0),
                    &u32_be(timescale),
                    &u64_be(duration),
                    &u32_be(0),
                ],
            )
        }
    }

    fn build_trak(track_id: u32, media_timescale: u32, version: u8) -> Vec<u8> {
        let stbl = mp4_box(b"stbl", &[]);
        let minf = mp4_box(b"minf", &[&stbl]);
        let mdhd = build_mdhd(media_timescale, 0, version);
        let mdia = mp4_box(b"mdia", &[&mdhd, &minf]);
        let tkhd = build_tkhd(track_id, 0, version);
        mp4_box(b"trak", &[&tkhd, &mdia])
    }

    fn build_trex(track_id: u32, default_sample_duration: u32) -> Vec<u8> {
        full_box(
            b"trex",
            0,
            0,
            &[
                &u32_be(track_id),
                &u32_be(1),
                &u32_be(default_sample_duration),
                &u32_be(0),
                &u32_be(0),
            ],
        )
    }

    fn build_audio_traf(track_id: u32, base_time: u32, sample_count: u32) -> Vec<u8> {
        let tfhd = full_box(b"tfhd", 0, 0x020000, &[&u32_be(track_id)]);
        let tfdt = full_box(b"tfdt", 0, 0, &[&u32_be(base_time)]);
        let mut sample_sizes = Vec::with_capacity(sample_count as usize * 4);
        for _ in 0..sample_count {
            sample_sizes.extend_from_slice(&u32_be(16));
        }
        let trun = full_box(
            b"trun",
            0,
            0x000201,
            &[&u32_be(sample_count), &u32_be(0), &sample_sizes],
        );
        mp4_box(b"traf", &[&tfhd, &tfdt, &trun])
    }

    fn build_video_traf(track_id: u32, base_time: u64, durations: &[u32]) -> Vec<u8> {
        let tfhd = full_box(b"tfhd", 0, 0x020002, &[&u32_be(track_id), &u32_be(1)]);
        let tfdt = full_box(b"tfdt", 1, 0, &[&u64_be(base_time)]);
        let mut entries = Vec::with_capacity(durations.len() * 8);
        for &d in durations {
            entries.extend_from_slice(&u32_be(d));
            entries.extend_from_slice(&u32_be(100));
        }
        let trun = full_box(
            b"trun",
            0,
            0x000305,
            &[
                &u32_be(durations.len() as u32),
                &u32_be(0),
                &u32_be(0x0200_0000),
                &entries,
            ],
        );
        mp4_box(b"traf", &[&tfhd, &tfdt, &trun])
    }

    struct SyntheticFmp4 {
        bytes: Vec<u8>,
        ftyp: Vec<u8>,
        moov: Vec<u8>,
        moof_offsets: Vec<u64>,
        audio_bases: [u32; 3],
        video_bases: [u64; 3],
        last_moof_offset: usize,
        last_moof_size: usize,
    }

    fn build_synthetic_fmp4() -> SyntheticFmp4 {
        let ftyp = mp4_box(b"ftyp", &[b"isom", &u32_be(512), b"isom", b"iso6", b"mp41"]);
        let mvhd = build_mvhd(1000, 0, 0);
        let trak1 = build_trak(1, 48000, 0);
        let trak2 = build_trak(2, 30000, 0);
        let trex1 = build_trex(1, 960);
        let trex2 = build_trex(2, 0);
        let mvex = mp4_box(b"mvex", &[&trex1, &trex2]);
        let moov = mp4_box(b"moov", &[&mvhd, &trak1, &trak2, &mvex]);

        let audio_bases = [0u32, 4800, 9600];
        let video_bases = [0u64, 3000, 6000];
        let video_durations: [&[u32]; 3] = [
            &[1000, 1000, 1000],
            &[1000, 1000, 1000],
            &[1000, 1000, 1500],
        ];

        let mut stream = concat(&[&ftyp, &moov]);
        let mut moof_offsets = Vec::new();
        let mut last_moof_offset = 0;
        let mut last_moof_size = 0;

        for i in 0..3 {
            let mfhd = full_box(b"mfhd", 0, 0, &[&u32_be((i + 1) as u32)]);
            let traf_a = build_audio_traf(1, audio_bases[i], 5);
            let traf_v = build_video_traf(2, video_bases[i], video_durations[i]);
            let moof = mp4_box(b"moof", &[&mfhd, &traf_a, &traf_v]);
            let mdat = mp4_box(b"mdat", &[&[(i + 1) as u8, 2, 3, 4, 5]]);
            moof_offsets.push(stream.len() as u64);
            last_moof_offset = stream.len();
            last_moof_size = moof.len();
            stream.extend_from_slice(&moof);
            stream.extend_from_slice(&mdat);
        }

        SyntheticFmp4 {
            bytes: stream,
            ftyp,
            moov,
            moof_offsets,
            audio_bases,
            video_bases,
            last_moof_offset,
            last_moof_size,
        }
    }

    struct ParsedDurations {
        mvhd_duration: u64,
        tracks: HashMap<u32, (u64, u64)>,
    }

    fn read_moov_durations(moov_bytes: &[u8]) -> ParsedDurations {
        let mut mvhd_duration = u64::MAX;
        let mut tracks = HashMap::new();

        fn walk(
            moov_bytes: &[u8],
            start: usize,
            end: usize,
            mvhd_duration: &mut u64,
            tracks: &mut HashMap<u32, (u64, u64)>,
            mut current_track: Option<&mut (Option<u32>, Option<u64>, Option<u64>)>,
        ) {
            let mut off = start;
            while off < end {
                let size = read_u32(moov_bytes, off) as usize;
                let kind = &moov_bytes[off + 4..off + 8];
                let body = off + 8;
                if kind == b"mvhd" {
                    let v = moov_bytes[body];
                    *mvhd_duration = if v == 0 {
                        u64::from(read_u32(moov_bytes, body + 16))
                    } else {
                        read_u64(moov_bytes, body + 24)
                    };
                } else if kind == b"trak" {
                    let mut track_state = (None, None, None);
                    walk(
                        moov_bytes,
                        body,
                        off + size,
                        mvhd_duration,
                        tracks,
                        Some(&mut track_state),
                    );
                    tracks.insert(
                        track_state.0.unwrap(),
                        (track_state.1.unwrap(), track_state.2.unwrap()),
                    );
                } else if kind == b"tkhd" {
                    if let Some(ref mut ct) = current_track {
                        let v = moov_bytes[body];
                        ct.0 = Some(read_u32(moov_bytes, body + if v == 0 { 12 } else { 20 }));
                        ct.1 = Some(if v == 0 {
                            u64::from(read_u32(moov_bytes, body + 20))
                        } else {
                            read_u64(moov_bytes, body + 28)
                        });
                    }
                } else if kind == b"mdia" {
                    if let Some(ref mut ct) = current_track {
                        walk(
                            moov_bytes,
                            body,
                            off + size,
                            mvhd_duration,
                            tracks,
                            Some(*ct),
                        );
                    }
                } else if kind == b"mdhd"
                    && let Some(ref mut ct) = current_track
                {
                    let v = moov_bytes[body];
                    ct.2 = Some(if v == 0 {
                        u64::from(read_u32(moov_bytes, body + 16))
                    } else {
                        read_u64(moov_bytes, body + 24)
                    });
                }
                off += size;
            }
        }

        walk(
            moov_bytes,
            8,
            moov_bytes.len(),
            &mut mvhd_duration,
            &mut tracks,
            None,
        );
        ParsedDurations {
            mvhd_duration,
            tracks,
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ParsedTfraEntry {
        time: u64,
        moof_offset: u64,
        traf_number: u8,
        trun_number: u8,
        sample_number: u8,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ParsedTfra {
        track_id: u32,
        version: u8,
        flags: u32,
        lengths: u32,
        entries: Vec<ParsedTfraEntry>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ParsedMfro {
        size: u32,
        version_flags: u32,
        parent_size: u32,
    }

    struct ParsedMfra {
        mfra_size: u32,
        mfra_type: [u8; 4],
        tfras: Vec<ParsedTfra>,
        mfro: Option<ParsedMfro>,
    }

    fn parse_mfra(tail: &[u8]) -> ParsedMfra {
        let mfra_size = read_u32(tail, 0);
        let mfra_type = [tail[4], tail[5], tail[6], tail[7]];
        let mut tfras = Vec::new();
        let mut mfro = None;

        let mut off = 8;
        while off < tail.len() {
            let size = read_u32(tail, off) as usize;
            let kind = &tail[off + 4..off + 8];
            if kind == b"tfra" {
                let vf = read_u32(tail, off + 8);
                let version = (vf >> 24) as u8;
                let flags = vf & 0x00ff_ffff;
                let track_id = read_u32(tail, off + 12);
                let lengths = read_u32(tail, off + 16);
                let count = read_u32(tail, off + 20) as usize;
                let mut entries = Vec::with_capacity(count);
                let mut p = off + 24;
                for _ in 0..count {
                    entries.push(ParsedTfraEntry {
                        time: read_u64(tail, p),
                        moof_offset: read_u64(tail, p + 8),
                        traf_number: tail[p + 16],
                        trun_number: tail[p + 17],
                        sample_number: tail[p + 18],
                    });
                    p += 19;
                }
                tfras.push(ParsedTfra {
                    track_id,
                    version,
                    flags,
                    lengths,
                    entries,
                });
            } else if kind == b"mfro" {
                mfro = Some(ParsedMfro {
                    size: size as u32,
                    version_flags: read_u32(tail, off + 8),
                    parent_size: read_u32(tail, off + 12),
                });
            }
            off += size;
        }

        ParsedMfra {
            mfra_size,
            mfra_type,
            tfras,
            mfro,
        }
    }

    fn finalize_bytes(orig: &[u8], fin: &Mp4Finalization) -> Vec<u8> {
        let head_off = fin.head_offset as usize;
        concat(&[
            &orig[..head_off],
            &fin.head,
            &orig[head_off + fin.head.len()..],
            &fin.tail,
        ])
    }

    #[test]
    fn patches_only_the_mvhd_duration_in_the_moov_head_without_changing_size() {
        let sample = build_synthetic_fmp4();
        let mut indexer = Fmp4Index::default();
        indexer.push(&sample.bytes);
        let fin = indexer.finish().expect("finalization");

        assert_eq!(fin.head_offset, sample.ftyp.len() as u64);
        assert_eq!(fin.head.len(), sample.moov.len());

        let durations = read_moov_durations(&fin.head);
        assert_eq!(durations.mvhd_duration, 317);
        assert_eq!(durations.tracks.get(&1), Some(&(0, 0)));
        assert_eq!(durations.tracks.get(&2), Some(&(0, 0)));
    }

    #[test]
    fn produces_identical_output_when_fed_byte_by_byte_vs_as_a_single_buffer() {
        let sample = build_synthetic_fmp4();

        let mut single = Fmp4Index::default();
        single.push(&sample.bytes);
        let single_fin = single.finish().expect("single");

        let mut streamed = Fmp4Index::default();
        for i in 0..sample.bytes.len() {
            streamed.push(&sample.bytes[i..i + 1]);
        }
        let streamed_fin = streamed.finish().expect("streamed");

        assert_eq!(streamed_fin, single_fin);
    }

    #[test]
    fn appends_a_valid_mfra_box_with_matching_tfra_entries_and_mfro_size() {
        let sample = build_synthetic_fmp4();
        let mut indexer = Fmp4Index::default();
        indexer.push(&sample.bytes);
        let fin = indexer.finish().expect("finalization");

        let parsed = parse_mfra(&fin.tail);
        assert_eq!(&parsed.mfra_type, b"mfra");
        assert_eq!(parsed.mfra_size as usize, fin.tail.len());
        assert_eq!(
            parsed.mfro,
            Some(ParsedMfro {
                size: 16,
                version_flags: 0,
                parent_size: fin.tail.len() as u32,
            })
        );
        assert_eq!(parsed.tfras.len(), 2);

        assert_eq!(parsed.tfras[0].track_id, 1);
        assert_eq!(parsed.tfras[0].version, 1);
        assert_eq!(parsed.tfras[0].flags, 0);
        assert_eq!(parsed.tfras[0].lengths, 0);
        assert_eq!(
            parsed.tfras[0].entries,
            sample
                .audio_bases
                .iter()
                .enumerate()
                .map(|(idx, &time)| ParsedTfraEntry {
                    time: u64::from(time),
                    moof_offset: sample.moof_offsets[idx],
                    traf_number: 1,
                    trun_number: 1,
                    sample_number: 1,
                })
                .collect::<Vec<_>>()
        );

        assert_eq!(parsed.tfras[1].track_id, 2);
        assert_eq!(parsed.tfras[1].version, 1);
        assert_eq!(parsed.tfras[1].flags, 0);
        assert_eq!(parsed.tfras[1].lengths, 0);
        assert_eq!(
            parsed.tfras[1].entries,
            sample
                .video_bases
                .iter()
                .enumerate()
                .map(|(idx, &time)| ParsedTfraEntry {
                    time,
                    moof_offset: sample.moof_offsets[idx],
                    traf_number: 2,
                    trun_number: 1,
                    sample_number: 1,
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn returns_none_for_webm_like_input_whose_first_box_is_not_ftyp() {
        let webm_header = [
            0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x86, 0x81, 0x01, 0x42, 0xf7, 0x81, 0x01,
        ];
        let mut indexer = Fmp4Index::default();
        indexer.push(&webm_header);
        assert!(indexer.finish().is_none());
    }

    #[test]
    fn returns_none_when_input_is_truncated_in_the_middle_of_the_last_moof() {
        let sample = build_synthetic_fmp4();
        let cut_point = sample.last_moof_offset + sample.last_moof_size / 2;
        let mut indexer = Fmp4Index::default();
        indexer.push(&sample.bytes[..cut_point]);
        assert!(indexer.finish().is_none());
    }

    #[test]
    fn returns_none_when_the_last_box_runs_to_the_end_of_the_file() {
        let sample = build_synthetic_fmp4();
        let open_ended = concat(&[&u32_be(0), b"mdat", &[9, 9, 9]]);
        let mut indexer = Fmp4Index::default();
        indexer.push(&concat(&[&sample.bytes, &open_ended]));
        assert!(indexer.finish().is_none());
    }

    #[test]
    fn replaces_head_at_head_offset_and_appends_tail() {
        let sample = build_synthetic_fmp4();
        let mut indexer = Fmp4Index::default();
        indexer.push(&sample.bytes);
        let fin = indexer.finish().expect("finalization");

        let out_bytes = finalize_bytes(&sample.bytes, &fin);
        assert_eq!(out_bytes.len(), sample.bytes.len() + fin.tail.len());
        let head_off = fin.head_offset as usize;
        assert_eq!(&out_bytes[..head_off], &sample.bytes[..head_off]);
        assert_eq!(&out_bytes[head_off..head_off + fin.head.len()], &fin.head);
        assert_eq!(
            &out_bytes[head_off + fin.head.len()..sample.bytes.len()],
            &sample.bytes[head_off + fin.head.len()..]
        );
        assert_eq!(&out_bytes[sample.bytes.len()..], &fin.tail);
    }

    #[test]
    fn returns_none_when_a_traf_has_no_tfdt_or_a_v0_duration_overflows_u32() {
        let ftyp = mp4_box(b"ftyp", &[b"isom", &u32_be(512), b"isom"]);
        let mvhd = build_mvhd(1000, 0, 0);
        let trak = build_trak(1, 48000, 0);
        let trex = build_trex(1, 960);
        let mvex = mp4_box(b"mvex", &[&trex]);
        let moov = mp4_box(b"moov", &[&mvhd, &trak, &mvex]);

        let tfhd = full_box(b"tfhd", 0, 0x020000, &[&u32_be(1)]);
        let trun = full_box(b"trun", 0, 0, &[&u32_be(5)]);
        let traf_no_tfdt = mp4_box(b"traf", &[&tfhd, &trun]);
        let mfhd = full_box(b"mfhd", 0, 0, &[&u32_be(1)]);
        let moof_no_tfdt = mp4_box(b"moof", &[&mfhd, &traf_no_tfdt]);
        let mdat = mp4_box(b"mdat", &[&u32_be(0)]);
        let no_tfdt_bytes = concat(&[&ftyp, &moov, &moof_no_tfdt, &mdat]);
        let mut no_tfdt_indexer = Fmp4Index::default();
        no_tfdt_indexer.push(&no_tfdt_bytes);
        assert!(no_tfdt_indexer.finish().is_none());

        let tfdt_huge = full_box(b"tfdt", 1, 0, &[&u64_be(0x30_0000_0000)]);
        let trun_one = full_box(b"trun", 0, 0, &[&u32_be(1)]);
        let huge_base_traf = mp4_box(b"traf", &[&tfhd, &tfdt_huge, &trun_one]);
        let moof_overflow = mp4_box(b"moof", &[&mfhd, &huge_base_traf]);
        let overflow_bytes = concat(&[&ftyp, &moov, &moof_overflow, &mdat]);
        let mut overflow_indexer = Fmp4Index::default();
        overflow_indexer.push(&overflow_bytes);
        assert!(overflow_indexer.finish().is_none());
    }

    #[test]
    fn supports_version_1_headers_64_bit_box_sizes_and_tfhd_default_sample_duration() {
        let ftyp = mp4_box(b"ftyp", &[b"isom", &u32_be(512), b"isom"]);
        let mvhd = build_mvhd(1000, 0, 1);
        let trak = build_trak(1, 1000, 1);
        let trex = build_trex(1, 0);
        let mvex = mp4_box(b"mvex", &[&trex]);
        let moov_body = concat(&[&mvhd, &trak, &mvex]);
        let moov_large = concat(&[
            &u32_be(1),
            b"moov",
            &u64_be((16 + moov_body.len()) as u64),
            &moov_body,
        ]);
        let tfhd = full_box(b"tfhd", 0, 0x020008, &[&u32_be(1), &u32_be(500)]);
        let tfdt = full_box(b"tfdt", 1, 0, &[&u64_be(5_000_000_000)]);
        let trun = full_box(b"trun", 0, 0, &[&u32_be(4)]);
        let traf = mp4_box(b"traf", &[&tfhd, &tfdt, &trun]);
        let mfhd = full_box(b"mfhd", 0, 0, &[&u32_be(1)]);
        let moof = mp4_box(b"moof", &[&mfhd, &traf]);
        let mdat = concat(&[&u32_be(1), b"mdat", &u64_be(20), &u32_be(0xdead_beef)]);
        let stream = concat(&[&ftyp, &moov_large, &moof, &mdat]);

        let mut indexer = Fmp4Index::default();
        indexer.push(&stream);
        let fin = indexer.finish().expect("finalization");

        assert_eq!(fin.head.len(), moov_large.len());
        assert_eq!(read_u64(&fin.head, 16 + 8 + 24), 5_000_002_000);
    }

    fn build_ingest_traf(track_id: u32, base_time: u64, durations: &[u32]) -> Vec<u8> {
        let tfhd = full_box(b"tfhd", 0, 0x020000, &[&u32_be(track_id)]);
        let tfdt = full_box(b"tfdt", 1, 0, &[&u64_be(base_time)]);
        let mut entries = Vec::with_capacity(durations.len() * 12);
        for &d in durations {
            entries.extend_from_slice(&u32_be(d));
            entries.extend_from_slice(&u32_be(256));
            entries.extend_from_slice(&u32_be(0x0101_0000));
        }
        let trun = full_box(
            b"trun",
            0,
            0x000701,
            &[&u32_be(durations.len() as u32), &u32_be(0), &entries],
        );
        mp4_box(b"traf", &[&tfhd, &tfdt, &trun])
    }

    #[test]
    fn indexes_ingest_shaped_init_and_three_segments() {
        let ftyp = mp4_box(b"ftyp", &[b"iso5", &u32_be(512), b"iso5", b"iso6", b"mp41"]);
        let mvhd = build_mvhd(1000, 0, 0);
        let trak_v = build_trak(1, 12800, 0);
        let trak_a = build_trak(2, 44100, 0);
        let trex_v = build_trex(1, 0);
        let trex_a = build_trex(2, 0);
        let mvex = mp4_box(b"mvex", &[&trex_v, &trex_a]);
        let moov = mp4_box(b"moov", &[&mvhd, &trak_v, &trak_a, &mvex]);
        let init = concat(&[&ftyp, &moov]);

        let v_bases = [0u64, 51200, 102400];
        let v_durs: [&[u32]; 3] = [
            &[12800, 12800, 12800, 12800],
            &[12800, 12800, 12800, 12800],
            &[12800, 12800, 6400],
        ];
        let a_bases = [0u64, 176400, 352800];
        let a_durs: [&[u32]; 3] = [
            &[44100, 44100, 44100, 44100],
            &[44100, 44100, 44100, 44100],
            &[44100, 44100, 44100],
        ];

        let mut indexer = Fmp4Index::default();
        indexer.push(&init);

        let mut offset = init.len() as u64;
        let mut moof_offsets = Vec::new();
        for i in 0..3 {
            let mfhd = full_box(b"mfhd", 0, 0, &[&u32_be((i + 1) as u32)]);
            let traf1 = build_ingest_traf(1, v_bases[i], v_durs[i]);
            let traf2 = build_ingest_traf(2, a_bases[i], a_durs[i]);
            let moof = mp4_box(b"moof", &[&mfhd, &traf1, &traf2]);
            let mdat = mp4_box(b"mdat", &[&[0u8; 16]]);
            let seg = concat(&[&moof, &mdat]);
            moof_offsets.push(offset);
            offset += seg.len() as u64;
            indexer.push(&seg);
        }

        let fin = indexer.finish().expect("ingest finalization");
        let durations = read_moov_durations(&fin.head);
        assert_eq!(durations.mvhd_duration, 11000);

        let parsed = parse_mfra(&fin.tail);
        assert_eq!(parsed.tfras.len(), 2);
        for (track_idx, bases) in [(0usize, &v_bases), (1usize, &a_bases)] {
            assert_eq!(
                parsed.tfras[track_idx].entries,
                bases
                    .iter()
                    .enumerate()
                    .map(|(seg_idx, &time)| ParsedTfraEntry {
                        time,
                        moof_offset: moof_offsets[seg_idx],
                        traf_number: (track_idx + 1) as u8,
                        trun_number: 1,
                        sample_number: 1,
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
}
