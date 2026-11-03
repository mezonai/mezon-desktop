use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};

pub const FILE_INGEST_MAX_SIZE: u64 = 1024 * 1024 * 1024;
const CHUNK_SIZE: u64 = 8 * 1024 * 1024;
const STATUS_POLL: Duration = Duration::from_secs(2);
const RETRY_DELAYS_SECS: [u64; 10] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];
const FRESH_UPLOAD_RETRIES: usize = 3;
const STORE_WAIT: Duration = Duration::from_secs(15 * 60);
const RETRY_STATUSES: [u16; 3] = [408, 500, 503];
const VIDEO_EXTENSIONS: [&str; 16] = [
    "3g2", "3gp", "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "ogv",
    "qt", "webm", "wmv",
];

static INGEST_BASES: OnceLock<Vec<(String, String)>> = OnceLock::new();

pub fn configure_file_ingest(spec: &str) {
    let _ = INGEST_BASES.set(parse_ingest_bases(spec));
}

fn ingest_bases() -> &'static [(String, String)] {
    INGEST_BASES.get().map_or(&[], Vec::as_slice)
}

fn parse_ingest_bases(spec: &str) -> Vec<(String, String)> {
    spec.split(',')
        .filter_map(|pair| {
            let (host, base) = pair.split_once('=')?;
            let host = host.trim().to_ascii_lowercase();
            let base = base.trim().trim_end_matches('/');
            (!host.is_empty() && is_origin(base)).then(|| (host, base.to_string()))
        })
        .collect()
}

fn is_origin(base: &str) -> bool {
    base.strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
        .is_some_and(|rest| !rest.is_empty() && !rest.contains(['/', '?', '#']))
}

fn is_video(filename: &str, filetype: &str) -> bool {
    filetype.starts_with("video")
        || filename.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && VIDEO_EXTENSIONS
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

pub fn is_ingest_candidate(filename: &str, filetype: &str, size: u64) -> bool {
    candidate_in(ingest_bases(), filename, filetype, size)
}

fn candidate_in(bases: &[(String, String)], filename: &str, filetype: &str, size: u64) -> bool {
    !bases.is_empty() && (1..=FILE_INGEST_MAX_SIZE).contains(&size) && is_video(filename, filetype)
}

pub fn ingest_url(
    presigned_url: &str,
    filename: &str,
    filetype: &str,
    size: u64,
) -> Option<String> {
    ingest_url_in(ingest_bases(), presigned_url, filename, filetype, size)
}

fn ingest_url_in(
    bases: &[(String, String)],
    presigned_url: &str,
    filename: &str,
    filetype: &str,
    size: u64,
) -> Option<String> {
    if !candidate_in(bases, filename, filetype, size) {
        return None;
    }
    let rest = presigned_url
        .strip_prefix("https://")
        .or_else(|| presigned_url.strip_prefix("http://"))?;
    let path_at = rest.find('/')?;
    let (host, path) = rest.split_at(path_at);
    if host.is_empty() || host.contains(['?', '#']) {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let (_, base) = bases.iter().find(|(known, _)| *known == host)?;
    Some(format!("{base}{path}"))
}

fn next_chunk(size: u64, received: &[(u64, u64)], moov: Option<u64>) -> Option<(u64, u64)> {
    let order = match moov {
        Some(moov) if moov > 0 && moov < size => vec![(moov, size - 1), (0, moov - 1)],
        _ => vec![(0, size.checked_sub(1)?)],
    };
    for (first, last) in order {
        let mut start = first;
        for &(from, to) in received {
            if from > start {
                break;
            }
            start = start.max(to.saturating_add(1));
        }
        if start > last {
            continue;
        }
        let following = received
            .iter()
            .find(|(from, _)| *from > start)
            .map_or(last, |(from, _)| from - 1);
        let end = (start + CHUNK_SIZE - 1).min(last).min(following);
        return Some((start, end));
    }
    None
}

fn moov_after_mdat(file: &mut (impl Read + Seek), size: u64) -> Option<u64> {
    let mut offset = 0u64;
    let mut seen_mdat = false;
    while offset + 8 <= size {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut head = [0u8; 16];
        let read = read_up_to(file, &mut head)?;
        if read < 8 {
            return None;
        }
        let mut box_size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let kind = &head[4..8];
        if box_size == 1 {
            if read < 16 {
                return None;
            }
            box_size = u64::from_be_bytes([
                head[8], head[9], head[10], head[11], head[12], head[13], head[14], head[15],
            ]);
        } else if box_size == 0 {
            box_size = size - offset;
        }
        if kind == b"moov" {
            return seen_mdat.then_some(offset);
        }
        if kind == b"mdat" {
            seen_mdat = true;
        }
        if box_size < 8 || box_size > size - offset {
            return None;
        }
        offset += box_size;
    }
    None
}

fn read_up_to(file: &mut impl Read, buf: &mut [u8]) -> Option<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(filled)
}

async fn moov_offset(path: PathBuf, size: u64) -> Option<u64> {
    crate::transport_runtime::runtime()
        .spawn_blocking(move || {
            let mut file = std::fs::File::open(&path).ok()?;
            moov_after_mdat(&mut file, size)
        })
        .await
        .ok()
        .flatten()
}

fn presigned_url_expired(url: &str, now_seconds: i64) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    let param = |name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    };
    let (Some(date), Some(expires)) = (
        param("X-Amz-Date").and_then(amz_date_seconds),
        param("X-Amz-Expires").and_then(|v| v.parse::<i64>().ok()),
    ) else {
        return false;
    };
    expires > 0 && now_seconds >= date + expires
}

fn amz_date_seconds(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(9..11)?, number(11..13)?, number(13..15)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

#[derive(Debug, Default, serde::Deserialize)]
struct IngestStatus {
    #[serde(default)]
    received: Vec<(u64, u64)>,
    #[serde(default)]
    stored: bool,
}

impl IngestStatus {
    fn held(&self) -> u64 {
        self.received
            .iter()
            .map(|(from, to)| to.saturating_sub(*from) + 1)
            .sum()
    }
}

pub struct IngestUpload {
    pub ingest_url: String,
    pub presigned_url: String,
    pub path: PathBuf,
    pub size: u64,
    pub content_type: String,
}

enum Answer {
    Status(IngestStatus),
    Code(u16),
    Unreachable,
}

impl Answer {
    fn from_response(response: Result<(u16, Vec<u8>)>) -> Self {
        match response {
            Ok((200, body)) => serde_json::from_slice(&body).map_or(Self::Code(200), Self::Status),
            Ok((code, _)) => Self::Code(code),
            Err(_) => Self::Unreachable,
        }
    }
}

struct Pacing {
    status_poll: Duration,
    retry_delays: Vec<Duration>,
    fresh_upload_retries: usize,
    store_wait: Duration,
}

impl Pacing {
    fn contract() -> Self {
        Self {
            status_poll: STATUS_POLL,
            retry_delays: RETRY_DELAYS_SECS
                .iter()
                .map(|secs| Duration::from_secs(*secs))
                .collect(),
            fresh_upload_retries: FRESH_UPLOAD_RETRIES,
            store_wait: STORE_WAIT,
        }
    }
}

pub async fn upload(job: &IngestUpload, on_sent: impl Fn()) -> Result<()> {
    upload_paced(job, &Pacing::contract(), on_sent).await
}

async fn upload_paced(job: &IngestUpload, pacing: &Pacing, on_sent: impl Fn()) -> Result<()> {
    let moov = moov_offset(job.path.clone(), job.size).await;
    let mut status =
        match Answer::from_response(crate::transport_runtime::get_body(&job.ingest_url).await) {
            Answer::Status(resumed) => Some(resumed),
            Answer::Code(_) | Answer::Unreachable => None,
        };
    let mut retries = 0usize;
    let mut all_sent_at: Option<Instant> = None;
    loop {
        let received = status.as_ref().map_or(&[][..], |s| s.received.as_slice());
        let chunk = next_chunk(job.size, received, moov);
        if chunk.is_none() {
            if status.as_ref().is_some_and(|s| s.stored) {
                return Ok(());
            }
            let sent_at = *all_sent_at.get_or_insert_with(|| {
                on_sent();
                Instant::now()
            });
            if sent_at.elapsed() >= pacing.store_wait {
                bail!("the file ingest did not store the upload in time");
            }
        }
        let answer = match chunk {
            Some((first, last)) => {
                let bytes = crate::transport_runtime::read_file_range(
                    job.path.clone(),
                    first,
                    (last - first + 1) as usize,
                )
                .await?;
                Answer::from_response(
                    crate::transport_runtime::put_range(
                        &job.ingest_url,
                        bytes,
                        (first, last),
                        job.size,
                    )
                    .await,
                )
            }
            None => {
                crate::transport_runtime::sleep(pacing.status_poll).await;
                Answer::from_response(crate::transport_runtime::get_body(&job.ingest_url).await)
            }
        };
        match answer {
            Answer::Status(next) => {
                tracing::debug!(
                    held = next.held(),
                    size = job.size,
                    stored = next.stored,
                    "file ingest status"
                );
                status = Some(next);
                retries = 0;
                continue;
            }
            Answer::Code(403) if chunk.is_some() => return direct_put(job).await,
            Answer::Code(503 | 413) if chunk.is_some() && status.is_none() => {
                return direct_put(job).await;
            }
            Answer::Code(404) if chunk.is_none() => {
                restart(job, &mut status)?;
                continue;
            }
            Answer::Code(code) if !RETRY_STATUSES.contains(&code) => {
                bail!("the file ingest answered {code}")
            }
            Answer::Code(_) | Answer::Unreachable => {}
        }
        loop {
            if status.is_none() && retries >= pacing.fresh_upload_retries {
                return direct_put(job).await;
            }
            let delay = pacing
                .retry_delays
                .get(retries)
                .ok_or_else(|| anyhow!("the file ingest is unreachable"))?;
            retries += 1;
            crate::transport_runtime::sleep(*delay).await;
            match Answer::from_response(crate::transport_runtime::get_body(&job.ingest_url).await) {
                Answer::Status(next) => {
                    status = Some(next);
                    break;
                }
                Answer::Code(404) if status.is_some() => {
                    restart(job, &mut status)?;
                    break;
                }
                Answer::Code(404) => break,
                Answer::Code(code) if !RETRY_STATUSES.contains(&code) => {
                    bail!("the file ingest answered {code}")
                }
                Answer::Code(_) | Answer::Unreachable => {}
            }
        }
    }
}

fn restart(job: &IngestUpload, status: &mut Option<IngestStatus>) -> Result<()> {
    if presigned_url_expired(&job.presigned_url, now_seconds()) {
        bail!("the file ingest lost the upload and its URL expired");
    }
    *status = None;
    Ok(())
}

async fn direct_put(job: &IngestUpload) -> Result<()> {
    tracing::info!("file ingest unusable for this upload; sending it straight to storage");
    crate::transport_runtime::put_file(
        &job.presigned_url,
        job.path.clone(),
        job.size,
        &job.content_type,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn dev_bases() -> Vec<(String, String)> {
        parse_ingest_bases("cdn-api-dev.nccsoft.vn=https://streaming.nccsoft.vn/")
    }

    fn mp4_box(kind: &[u8; 4], body_len: usize) -> Vec<u8> {
        let mut out = ((body_len + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend(std::iter::repeat_n(0u8, body_len));
        out
    }

    #[test]
    fn the_ingest_map_is_read_from_the_build_setting() {
        assert_eq!(
            parse_ingest_bases(
                " cdn-api-dev.nccsoft.vn = https://streaming.nccsoft.vn/ ,bad,x=ftp://y,=https://z,a.b=https://c.d/path"
            ),
            vec![(
                "cdn-api-dev.nccsoft.vn".to_string(),
                "https://streaming.nccsoft.vn".to_string()
            )]
        );
        assert!(parse_ingest_bases("").is_empty());
    }

    #[test]
    fn only_videos_up_to_a_gib_on_a_mapped_host_go_through_the_ingest() {
        let bases = dev_bases();
        let presigned = "https://cdn-api-dev.nccsoft.vn/dev-mezon/1cf9/208_clip.mp4?X-Amz-Algorithm=AWS4&X-Amz-Signature=ab";
        assert_eq!(
            ingest_url_in(&bases, presigned, "clip.mp4", "video/mp4", 10 * MIB).as_deref(),
            Some(
                "https://streaming.nccsoft.vn/dev-mezon/1cf9/208_clip.mp4?X-Amz-Algorithm=AWS4&X-Amz-Signature=ab"
            )
        );
        assert!(ingest_url_in(&bases, presigned, "IMG.MOV", "", 1).is_some());
        assert!(ingest_url_in(&bases, presigned, "clip.mp4", "video/mp4", 0).is_none());
        assert!(
            ingest_url_in(
                &bases,
                presigned,
                "clip.mp4",
                "video/mp4",
                FILE_INGEST_MAX_SIZE + 1
            )
            .is_none()
        );
        assert!(ingest_url_in(&bases, presigned, "photo.png", "image/png", MIB).is_none());
        assert!(
            ingest_url_in(
                &bases,
                "https://pub-r2.example/1cf9/208_clip.mp4?sig",
                "clip.mp4",
                "video/mp4",
                MIB
            )
            .is_none()
        );
        assert!(ingest_url_in(&[], presigned, "clip.mp4", "video/mp4", MIB).is_none());
    }

    #[test]
    fn a_tail_moov_is_sent_before_the_rest() {
        let size = 20 * MIB;
        let moov = 18 * MIB;
        assert_eq!(next_chunk(size, &[], Some(moov)), Some((moov, size - 1)));
        assert_eq!(
            next_chunk(size, &[(moov, size - 1)], Some(moov)),
            Some((0, CHUNK_SIZE - 1))
        );
        assert_eq!(
            next_chunk(size, &[(0, CHUNK_SIZE - 1), (moov, size - 1)], Some(moov)),
            Some((CHUNK_SIZE, 2 * CHUNK_SIZE - 1))
        );
        assert_eq!(
            next_chunk(
                size,
                &[(0, 2 * CHUNK_SIZE - 1), (moov, size - 1)],
                Some(moov)
            ),
            Some((2 * CHUNK_SIZE, moov - 1))
        );
        assert_eq!(next_chunk(size, &[(0, size - 1)], Some(moov)), None);
    }

    #[test]
    fn a_resumed_upload_sends_only_the_missing_bytes() {
        let size = 30 * MIB;
        assert_eq!(next_chunk(size, &[], None), Some((0, CHUNK_SIZE - 1)));
        assert_eq!(
            next_chunk(size, &[(0, 99), (200, 299)], None),
            Some((100, 199))
        );
        assert_eq!(
            next_chunk(size, &[(0, size - 2)], None),
            Some((size - 1, size - 1))
        );
        assert_eq!(next_chunk(0, &[], None), None);
    }

    #[test]
    fn moov_counts_only_when_it_follows_mdat() {
        let mut tail = mp4_box(b"ftyp", 16);
        tail.extend(mp4_box(b"mdat", 64));
        let moov_at = tail.len() as u64;
        tail.extend(mp4_box(b"moov", 32));
        let size = tail.len() as u64;
        assert_eq!(moov_after_mdat(&mut Cursor::new(tail), size), Some(moov_at));

        let mut head = mp4_box(b"ftyp", 16);
        head.extend(mp4_box(b"moov", 32));
        head.extend(mp4_box(b"mdat", 64));
        let size = head.len() as u64;
        assert_eq!(moov_after_mdat(&mut Cursor::new(head), size), None);

        let damaged = vec![0, 0, 0, 4, b'm', b'd', b'a', b't', 0, 0];
        assert_eq!(moov_after_mdat(&mut Cursor::new(damaged), 10), None);
    }

    #[test]
    fn a_large_mdat_with_a_64_bit_size_is_skipped() {
        let mut file = mp4_box(b"ftyp", 16);
        let mdat_body = 40u64;
        file.extend(1u32.to_be_bytes());
        file.extend(b"mdat");
        file.extend((16 + mdat_body).to_be_bytes());
        file.extend(std::iter::repeat_n(0u8, mdat_body as usize));
        let moov_at = file.len() as u64;
        file.extend(mp4_box(b"moov", 8));
        let size = file.len() as u64;
        assert_eq!(moov_after_mdat(&mut Cursor::new(file), size), Some(moov_at));
    }

    #[test]
    fn the_presigned_url_expires_after_its_window() {
        let url = "https://h/k?X-Amz-Date=20261103T120000Z&X-Amz-Expires=900&X-Amz-Signature=x";
        let signed_at = amz_date_seconds("20261103T120000Z").unwrap();
        assert_eq!(signed_at, 1_793_707_200);
        assert!(!presigned_url_expired(url, signed_at + 899));
        assert!(presigned_url_expired(url, signed_at + 900));
        assert!(!presigned_url_expired(
            "https://h/k?X-Amz-Signature=x",
            i64::MAX
        ));
        assert!(!presigned_url_expired("https://h/k", i64::MAX));
    }

    #[test]
    fn a_status_reports_how_much_the_ingest_holds() {
        let status: IngestStatus = serde_json::from_str(
            r#"{"size":41947168,"received":[[0,8388607],[41943072,41947167]],"stored":false,"hls":"playable"}"#,
        )
        .unwrap();
        assert_eq!(status.held(), 8_388_608 + 4_096);
        assert!(!status.stored);
    }

    #[derive(Clone, Copy, PartialEq)]
    enum IngestMode {
        Stores,
        FullOnFirstChunk,
        NeverStores,
    }

    #[derive(Default)]
    struct FakeIngest {
        size: u64,
        spool: Vec<u8>,
        received: Vec<(u64, u64)>,
        polls_after_sent: usize,
        chunks: Vec<(u64, u64)>,
        direct: Vec<(usize, bool)>,
    }

    impl FakeIngest {
        fn status(&self, stores: bool) -> String {
            let complete = self.received == vec![(0, self.size - 1)];
            let ranges: Vec<String> = self
                .received
                .iter()
                .map(|(a, b)| format!("[{a},{b}]"))
                .collect();
            format!(
                r#"{{"size":{},"received":[{}],"stored":{},"hls":"playable"}}"#,
                self.size,
                ranges.join(","),
                stores && complete && self.polls_after_sent >= 2
            )
        }

        fn take(&mut self, first: u64, last: u64, body: &[u8]) {
            self.spool[first as usize..=last as usize].copy_from_slice(body);
            self.chunks.push((first, last));
            let mut ranges = self.received.clone();
            ranges.push((first, last));
            ranges.sort();
            let mut merged: Vec<(u64, u64)> = Vec::new();
            for (a, b) in ranges {
                match merged.last_mut() {
                    Some(prev) if a <= prev.1 + 1 => prev.1 = prev.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            self.received = merged;
        }
    }

    async fn read_request(
        stream: &mut tokio::net::TcpStream,
    ) -> Option<(String, Vec<(String, String)>, Vec<u8>)> {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut piece = [0u8; 65536];
        let head_end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at;
            }
            let n = stream.read(&mut piece).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&piece[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut lines = head.split("\r\n");
        let request_line = lines.next()?.to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let length = headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < length {
            let n = stream.read(&mut piece).await.ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&piece[..n]);
        }
        Some((request_line, headers, body))
    }

    async fn fake_ingest(
        mode: IngestMode,
        size: u64,
    ) -> (String, std::sync::Arc<std::sync::Mutex<FakeIngest>>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = std::sync::Arc::new(std::sync::Mutex::new(FakeIngest {
            size,
            spool: vec![0; size as usize],
            ..Default::default()
        }));
        let shared = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let state = shared.clone();
                tokio::spawn(async move {
                    let Some((line, headers, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let header = |name: &str| {
                        headers
                            .iter()
                            .find(|(k, _)| k == name)
                            .map(|(_, v)| v.clone())
                    };
                    let (code, reply) = {
                        let mut ingest = state.lock().unwrap();
                        if line.starts_with("PUT") {
                            match header("content-range") {
                                Some(range) => {
                                    let range = range.trim_start_matches("bytes ");
                                    let (span, _) = range.split_once('/').unwrap();
                                    let (a, b) = span.split_once('-').unwrap();
                                    let (a, b) = (a.parse().unwrap(), b.parse().unwrap());
                                    if mode == IngestMode::FullOnFirstChunk
                                        && ingest.received.is_empty()
                                    {
                                        (503, r#"{"error":"full"}"#.to_string())
                                    } else {
                                        ingest.take(a, b, &body);
                                        (200, ingest.status(mode == IngestMode::Stores))
                                    }
                                }
                                None => {
                                    let chunked = header("transfer-encoding").is_some();
                                    ingest.direct.push((body.len(), chunked));
                                    (200, String::new())
                                }
                            }
                        } else if ingest.received.is_empty() {
                            (404, r#"{"error":"no upload"}"#.to_string())
                        } else {
                            if ingest.received == vec![(0, size - 1)] {
                                ingest.polls_after_sent += 1;
                            }
                            (200, ingest.status(mode == IngestMode::Stores))
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (origin, state)
    }

    fn quick() -> Pacing {
        Pacing {
            status_poll: Duration::from_millis(5),
            retry_delays: vec![Duration::from_millis(5); 10],
            fresh_upload_retries: 3,
            store_wait: Duration::from_millis(300),
        }
    }

    fn tail_moov_file(name: &str) -> (PathBuf, Vec<u8>) {
        let mut bytes = mp4_box(b"ftyp", 16);
        bytes.extend(mp4_box(b"mdat", (17 * MIB) as usize));
        bytes.extend(mp4_box(b"moov", 64));
        let mdat_body = 32..32 + (17 * MIB) as usize;
        for (i, byte) in bytes[mdat_body].iter_mut().enumerate() {
            *byte = (i % 251) as u8;
        }
        let path = std::env::temp_dir().join(format!("mezon-ingest-{name}-{}", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        (path, bytes)
    }

    fn job(origin: &str, ingest_origin: &str, path: PathBuf, size: u64) -> IngestUpload {
        let key = "/bucket/1cf9/208_clip.mp4?X-Amz-Date=20990101T000000Z&X-Amz-Expires=900&X-Amz-Signature=x";
        IngestUpload {
            ingest_url: format!("{ingest_origin}{key}"),
            presigned_url: format!("{origin}{key}"),
            path,
            size,
            content_type: "video/mp4".into(),
        }
    }

    #[tokio::test]
    async fn an_upload_sends_the_tail_first_and_waits_until_stored() {
        let (path, bytes) = tail_moov_file("stores");
        let size = bytes.len() as u64;
        let (origin, state) = fake_ingest(IngestMode::Stores, size).await;
        let sent = std::sync::atomic::AtomicUsize::new(0);
        let result = upload_paced(&job(&origin, &origin, path.clone(), size), &quick(), || {
            sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 1);
        let ingest = state.lock().unwrap();
        let moov_at = size - 72;
        assert_eq!(ingest.chunks.first(), Some(&(moov_at, size - 1)));
        assert_eq!(ingest.chunks[1], (0, CHUNK_SIZE - 1));
        assert!(ingest.spool == bytes);
        assert!(ingest.polls_after_sent >= 2);
        assert!(ingest.direct.is_empty());
    }

    #[tokio::test]
    async fn an_ingest_that_never_answers_falls_back_to_the_presigned_url() {
        let (path, bytes) = tail_moov_file("down");
        let size = bytes.len() as u64;
        let (origin, state) = fake_ingest(IngestMode::Stores, size).await;
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port())
        };
        let result =
            upload_paced(&job(&origin, &closed, path.clone(), size), &quick(), || {}).await;
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "{:#}", result.unwrap_err());
        assert_eq!(state.lock().unwrap().direct, vec![(size as usize, false)]);
    }

    #[tokio::test]
    async fn a_full_ingest_falls_back_on_the_first_chunk() {
        let (path, bytes) = tail_moov_file("full");
        let size = bytes.len() as u64;
        let (origin, state) = fake_ingest(IngestMode::FullOnFirstChunk, size).await;
        let result =
            upload_paced(&job(&origin, &origin, path.clone(), size), &quick(), || {}).await;
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok(), "{result:?}");
        let ingest = state.lock().unwrap();
        assert!(ingest.chunks.is_empty());
        assert_eq!(ingest.direct, vec![(size as usize, false)]);
    }

    #[tokio::test]
    async fn an_upload_the_ingest_never_stores_gives_up() {
        let (path, bytes) = tail_moov_file("never");
        let size = bytes.len() as u64;
        let (origin, _) = fake_ingest(IngestMode::NeverStores, size).await;
        let result =
            upload_paced(&job(&origin, &origin, path.clone(), size), &quick(), || {}).await;
        let _ = std::fs::remove_file(&path);
        let error = result.expect_err("must give up");
        assert!(error.to_string().contains("in time"), "{error}");
    }
}
