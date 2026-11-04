use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app_api::CDN_MEZON_MINIO;

const TRANSCODING_ORIGINS: [&str; 4] = [
    CDN_MEZON_MINIO,
    "http://cdn.mezon.ai",
    "https://cdn.mezon.vn",
    "http://cdn.mezon.vn",
];
const HLS_OUTPUT_PREFIX: &str = "hls/";
const HLS_MASTER_PLAYLIST: &str = "master.m3u8";
const HLS_INDEX_PLAYLIST: &str = "index.m3u8";
const HLS_INIT_SEGMENT: &str = "init.mp4";
const HLS_PLAYLIST_SUFFIX: &str = ".m3u8";
const HLS_PLAYLIST_TAG: &[u8] = b"#EXTM3U";
const HLS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const INGEST_PROBE_WINDOW_MS: u128 = 3000;
const INIT_SEGMENT_MAX_BYTES: usize = 64 * 1024;
const TRACK_MATRIX_ONE: i32 = 0x10000;
const HLS_END_TAG: &str = "#EXT-X-ENDLIST";
const HLS_VARIANT_TAG: &str = "#EXT-X-STREAM-INF";
const PLAYLIST_MAX_BYTES: usize = 2 * 1024 * 1024;
const GROWING_PLAYLIST_NEEDS_SNAPSHOT: bool = cfg!(target_os = "linux");

#[derive(Debug, Clone, PartialEq)]
pub struct HlsSnapshot {
    pub url: String,
    pub path: std::path::PathBuf,
    pub duration: f64,
    pub segment_starts: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GrowingPlaylist {
    Snapshot(HlsSnapshot),
    Finished(Vec<f64>),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestStream {
    Waiting,
    Playable,
    Unplayable,
}
const HLS_TRANSCODE_MIN_SIZE: i32 = 50 * 1024 * 1024;
const HLS_TRANSCODE_EXTENSIONS: [&str; 7] = ["mp4", "mov", "mkv", "avi", "flv", "m4v", "wmv"];

pub fn wants_hls_transcode(filename: &str, size: i32) -> bool {
    size >= HLS_TRANSCODE_MIN_SIZE
        && filename.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && HLS_TRANSCODE_EXTENSIONS
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

pub fn master_playlist_url(url: &str) -> Option<String> {
    hls_playlist_candidate(url, &hls_origins())
}

pub fn may_be_transcoded(url: &str) -> bool {
    transcodable_file(url, &TRANSCODING_ORIGINS)
}

pub fn may_have_hls_stream(url: &str) -> bool {
    transcodable_file(url, &hls_origins())
}

pub async fn transcoded_hls_playlist(url: &str) -> Option<String> {
    hls_playlist_on(url, &hls_origins(), probe_nonce()).await
}

pub async fn ingest_hls_playlist(url: &str) -> Option<String> {
    ingest_playlist_on(url, &hls_origins(), ingest_probe_window()).await
}

pub async fn playable_hls_stream(url: &str, ingest: bool) -> Option<String> {
    let playlist = if ingest {
        ingest_hls_playlist(url).await
    } else {
        transcoded_hls_playlist(url).await
    }?;
    if hls_video_is_turned(&playlist).await {
        return None;
    }
    Some(playlist)
}

pub async fn ingest_stream(url: &str) -> IngestStream {
    let Some(playlist) = ingest_hls_playlist(url).await else {
        return IngestStream::Waiting;
    };
    if hls_video_is_turned(&playlist).await {
        IngestStream::Unplayable
    } else {
        IngestStream::Playable
    }
}

pub async fn growing_playlist(playlist: &str) -> GrowingPlaylist {
    if !GROWING_PLAYLIST_NEEDS_SNAPSHOT {
        return GrowingPlaylist::Unknown;
    }
    let Some(text) = crate::transport_runtime::fetch_text_unsigned(
        &format!("{playlist}?probe={:x}", probe_nonce()),
        PLAYLIST_MAX_BYTES,
        HLS_PROBE_TIMEOUT,
    )
    .await
    else {
        return GrowingPlaylist::Unknown;
    };
    if text.contains(HLS_END_TAG) || text.contains(HLS_VARIANT_TAG) {
        return GrowingPlaylist::Finished(segment_starts(&text));
    }
    snapshot_file(playlist, &text)
        .await
        .map_or(GrowingPlaylist::Unknown, GrowingPlaylist::Snapshot)
}

pub fn safe_resume_point(position: f64, segment_starts: &[f64]) -> f64 {
    segment_starts
        .iter()
        .copied()
        .filter(|start| *start > 0.0)
        .find(|start| position > start - 0.1 && position < start + 0.6)
        .map_or(position, |start| (start - 1.0).max(0.0))
}

fn segment_starts(text: &str) -> Vec<f64> {
    let mut starts = Vec::new();
    let mut at = 0.0;
    for duration in text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXTINF:"))
        .filter_map(|rest| rest.split(',').next()?.trim().parse::<f64>().ok())
    {
        starts.push(at);
        at += duration;
    }
    starts
}

async fn snapshot_file(playlist: &str, text: &str) -> Option<HlsSnapshot> {
    let (snapshot, duration) = snapshot_playlist(playlist, text)?;
    let path = std::env::temp_dir()
        .join("mezon-hls")
        .join(format!("{:016x}.m3u8", stable_hash(playlist)));
    let written = path.clone();
    crate::transport_runtime::runtime()
        .spawn_blocking(move || {
            if let Some(dir) = written.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&written, snapshot)
        })
        .await
        .ok()?
        .ok()?;
    Some(HlsSnapshot {
        url: format!("file://{}", path.display()),
        path,
        duration,
        segment_starts: segment_starts(text),
    })
}

fn snapshot_playlist(playlist: &str, text: &str) -> Option<(String, f64)> {
    if !text.starts_with("#EXTM3U") || text.contains(HLS_END_TAG) {
        return None;
    }
    let dir = playlist.split(['?', '#']).next()?.rsplit_once('/')?.0;
    let absolute = |uri: &str| {
        if uri.contains("://") {
            uri.to_string()
        } else {
            format!("{dir}/{uri}")
        }
    };
    let mut duration = 0.0;
    let mut segments = 0usize;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            let (uri, tail) = rest.split_once('"')?;
            out.push(format!("#EXT-X-MAP:URI=\"{}\"{tail}", absolute(uri)));
        } else if line.starts_with("#EXT-X-PLAYLIST-TYPE") {
            out.push("#EXT-X-PLAYLIST-TYPE:VOD".to_string());
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            duration += rest.split(',').next()?.trim().parse::<f64>().ok()?;
            out.push(line.to_string());
        } else if line.starts_with('#') {
            out.push(line.to_string());
        } else {
            segments += 1;
            out.push(absolute(line));
        }
    }
    if segments == 0 {
        return None;
    }
    out.push(HLS_END_TAG.to_string());
    Some((out.join("\n") + "\n", duration))
}

fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn hls_origins() -> Vec<&'static str> {
    let configured = crate::app_api::cdn_read_base_url(1, "").trim_end_matches('/');
    let mut origins = TRANSCODING_ORIGINS.to_vec();
    if !configured.is_empty() && !origins.contains(&configured) {
        origins.push(configured);
    }
    origins
}

async fn hls_playlist_on(url: &str, origins: &[&str], nonce: u64) -> Option<String> {
    let playlist = hls_playlist_candidate(url, origins)?;
    answers_playlist(&format!("{playlist}?probe={nonce:x}"))
        .await
        .then_some(playlist)
}

async fn ingest_playlist_on(url: &str, origins: &[&str], window: u128) -> Option<String> {
    let playlist = index_playlist_candidate(url, origins)?;
    answers_playlist(&format!("{playlist}?probe={window}"))
        .await
        .then_some(playlist)
}

async fn answers_playlist(probe_url: &str) -> bool {
    crate::transport_runtime::fetch_prefix(probe_url, HLS_PLAYLIST_TAG.len(), HLS_PROBE_TIMEOUT)
        .await
        .is_some_and(|head| head == HLS_PLAYLIST_TAG)
}

async fn hls_video_is_turned(playlist: &str) -> bool {
    let Some(init) = sibling_url(playlist, HLS_INIT_SEGMENT) else {
        return false;
    };
    crate::transport_runtime::fetch_prefix(&init, INIT_SEGMENT_MAX_BYTES, HLS_PROBE_TIMEOUT)
        .await
        .is_some_and(|bytes| has_turned_video_track(&bytes))
}

fn sibling_url(url: &str, name: &str) -> Option<String> {
    let base = url.split(['?', '#']).next()?;
    let (dir, _) = base.rsplit_once('/')?;
    Some(format!("{dir}/{name}"))
}

fn has_turned_video_track(bytes: &[u8]) -> bool {
    boxes_hold_turned_track(bytes, 0, bytes.len())
}

fn boxes_hold_turned_track(bytes: &[u8], start: usize, end: usize) -> bool {
    let mut at = start;
    while at + 8 <= end {
        let Some(declared) = read_u32(bytes, at) else {
            return false;
        };
        let size = if declared == 0 {
            end - at
        } else {
            declared as usize
        };
        if size < 8 || at + size > end {
            return false;
        }
        let kind = &bytes[at + 4..at + 8];
        if (kind == b"moov" || kind == b"trak") && boxes_hold_turned_track(bytes, at + 8, at + size)
        {
            return true;
        }
        if kind == b"tkhd" && is_turned_track_header(bytes, at, size) {
            return true;
        }
        at += size;
    }
    false
}

fn is_turned_track_header(bytes: &[u8], at: usize, size: usize) -> bool {
    if size < 92 {
        return false;
    }
    let matrix = at + 8 + if bytes[at + 8] == 1 { 52 } else { 40 };
    if matrix + 44 > at + size {
        return false;
    }
    let shown = |offset: usize| read_u32(bytes, matrix + offset).is_some_and(|v| v != 0);
    if !shown(36) || !shown(40) {
        return false;
    }
    let cell = |offset: usize| read_u32(bytes, matrix + offset).map(|v| v as i32);
    cell(0) != Some(TRACK_MATRIX_ONE)
        || cell(4) != Some(0)
        || cell(12) != Some(0)
        || cell(16) != Some(TRACK_MATRIX_ONE)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
}

fn hls_playlist_candidate(url: &str, origins: &[&str]) -> Option<String> {
    hls_file_candidate(url, origins, HLS_MASTER_PLAYLIST)
}

fn index_playlist_candidate(url: &str, origins: &[&str]) -> Option<String> {
    hls_file_candidate(url, origins, HLS_INDEX_PLAYLIST)
}

fn hls_file_candidate(url: &str, origins: &[&str], name: &str) -> Option<String> {
    transcodable_path(url, origins)
        .map(|(origin, path)| format!("{origin}/{HLS_OUTPUT_PREFIX}{path}/{name}"))
}

fn transcodable_file(url: &str, origins: &[&str]) -> bool {
    transcodable_path(url, origins).is_some()
}

fn transcodable_path<'a>(url: &'a str, origins: &[&'a str]) -> Option<(&'a str, &'a str)> {
    let (origin, path) = origins.iter().find_map(|origin| {
        let path = url.strip_prefix(*origin)?.strip_prefix('/')?;
        Some((*origin, path))
    })?;
    let file = path.rsplit('/').next().unwrap_or_default();
    let transcodable = !path.contains(['?', '#'])
        && !path.starts_with(HLS_OUTPUT_PREFIX)
        && !file.is_empty()
        && !file
            .get(file.len().saturating_sub(HLS_PLAYLIST_SUFFIX.len())..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(HLS_PLAYLIST_SUFFIX));
    transcodable.then_some((origin, path))
}

fn probe_nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
}

fn ingest_probe_window() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() / INGEST_PROBE_WINDOW_MS)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        CDN_MEZON_MINIO, HLS_TRANSCODE_MIN_SIZE, TRANSCODING_ORIGINS, has_turned_video_track,
        hls_playlist_candidate, hls_playlist_on, index_playlist_candidate, ingest_playlist_on,
        may_be_transcoded, safe_resume_point, segment_starts, sibling_url, snapshot_playlist,
        wants_hls_transcode,
    };

    const VIDEO: &str = "1cf93b197d401000/2080000000000000001_clip.mp4";
    const MASTER: &str = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-STREAM-INF:BANDWIDTH=3137344,RESOLUTION=1280x720\nstream_0/playlist.m3u8\n";

    async fn fake_minio(objects: Vec<(String, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let objects: Arc<HashMap<String, &'static str>> = Arc::new(objects.into_iter().collect());
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let objects = objects.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    log.lock().unwrap().push(target.clone());
                    let key = target.split('?').next().unwrap_or("");
                    let (status, body) = match objects.get(key) {
                        Some(body) => ("200 OK", *body),
                        None => ("404 Not Found", "<Error><Code>NoSuchKey</Code></Error>"),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (origin, seen)
    }

    #[test]
    fn only_large_videos_ask_for_hls() {
        let large = HLS_TRANSCODE_MIN_SIZE;
        for name in [
            "clip.mp4",
            "IMG_0001.MOV",
            "movie.mkv",
            "old.avi",
            "a.flv",
            "b.m4v",
            "c.wmv",
        ] {
            assert!(wants_hls_transcode(name, large), "{name}");
            assert!(!wants_hls_transcode(name, large - 1), "{name}");
        }
        for name in [
            "clip.webm",
            "photo.png",
            "song.mp3",
            "notes.pdf",
            "mp4",
            ".mp4",
            "clip.mp4.zip",
        ] {
            assert!(!wants_hls_transcode(name, i32::MAX), "{name}");
        }
    }

    #[test]
    fn videos_under_fifty_mib_upload_without_hls() {
        const MIB: i32 = 1024 * 1024;
        assert!(!wants_hls_transcode("clip.mp4", 10 * MIB));
        assert!(!wants_hls_transcode("clip.mp4", 20 * MIB));
        assert!(!wants_hls_transcode("clip.mp4", 50 * MIB - 1));
        assert!(wants_hls_transcode("clip.mp4", 50 * MIB));
        assert!(wants_hls_transcode("IMG_0001.MOV", 200 * MIB));
    }

    #[test]
    fn a_video_on_the_transcoding_cdn_points_at_its_master_playlist() {
        for origin in TRANSCODING_ORIGINS {
            let url = format!("{origin}/{VIDEO}");
            assert_eq!(
                hls_playlist_candidate(&url, &TRANSCODING_ORIGINS),
                Some(format!("{origin}/hls/{VIDEO}/master.m3u8"))
            );
            assert!(may_be_transcoded(&url));
        }
    }

    #[test]
    fn videos_the_transcoder_never_sees_are_not_probed() {
        for url in [
            format!("https://cdn.komu.vn/{VIDEO}"),
            format!("{CDN_MEZON_MINIO}.evil.example/{VIDEO}"),
            format!("{CDN_MEZON_MINIO}/{VIDEO}?1700000000-mac"),
            format!("{CDN_MEZON_MINIO}/{VIDEO}#t=3"),
            format!("{CDN_MEZON_MINIO}/{VIDEO}/master.m3u8"),
            format!("{CDN_MEZON_MINIO}/{VIDEO}/INDEX.M3U8"),
            format!("{CDN_MEZON_MINIO}/hls/{VIDEO}"),
            format!("{CDN_MEZON_MINIO}/hls/{VIDEO}/stream_0/data000.ts"),
            format!("{CDN_MEZON_MINIO}/1cf93b197d401000/"),
            CDN_MEZON_MINIO.to_string(),
        ] {
            assert_eq!(
                hls_playlist_candidate(&url, &TRANSCODING_ORIGINS),
                None,
                "{url}"
            );
            assert!(!may_be_transcoded(&url), "{url}");
        }
    }

    #[tokio::test]
    async fn a_transcoded_video_plays_from_its_playlist() {
        let (origin, _) = fake_minio(vec![(format!("/hls/{VIDEO}/master.m3u8"), MASTER)]).await;
        let url = format!("{origin}/{VIDEO}");
        assert_eq!(
            hls_playlist_on(&url, &[origin.as_str()], 0xbeef).await,
            Some(format!("{origin}/hls/{VIDEO}/master.m3u8"))
        );
    }

    #[tokio::test]
    async fn the_probe_never_requests_the_url_the_player_will_open() {
        let (origin, seen) = fake_minio(vec![(format!("/hls/{VIDEO}/master.m3u8"), MASTER)]).await;
        let url = format!("{origin}/{VIDEO}");
        hls_playlist_on(&url, &[origin.as_str()], 0xbeef).await;
        hls_playlist_on(&url, &[origin.as_str()], 0xcafe).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                format!("/hls/{VIDEO}/master.m3u8?probe=beef"),
                format!("/hls/{VIDEO}/master.m3u8?probe=cafe"),
            ]
        );
    }

    #[tokio::test]
    async fn a_video_not_transcoded_yet_keeps_its_original_url() {
        let (origin, _) = fake_minio(vec![(format!("/{VIDEO}"), "mp4 bytes")]).await;
        assert_eq!(
            hls_playlist_on(&format!("{origin}/{VIDEO}"), &[origin.as_str()], 1).await,
            None
        );
    }

    #[tokio::test]
    async fn a_playlist_url_that_answers_something_else_is_ignored() {
        let (origin, _) = fake_minio(vec![(
            format!("/hls/{VIDEO}/master.m3u8"),
            "<html>ok</html>",
        )])
        .await;
        assert_eq!(
            hls_playlist_on(&format!("{origin}/{VIDEO}"), &[origin.as_str()], 1).await,
            None
        );
    }

    const INDEX: &str = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg00000.m4s\n";
    const UPRIGHT: [i32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    const TURNED_90: [i32; 9] = [0, 0x10000, 0, -0x10000, 0, 0, 0, 0, 0x4000_0000];
    const TURNED_180: [i32; 9] = [-0x10000, 0, 0, 0, -0x10000, 0, 0, 0, 0x4000_0000];
    const MIRRORED: [i32; 9] = [-0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

    fn boxed(kind: &[u8; 4], body: Vec<u8>) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend(body);
        out
    }

    fn track_header(version: u8, matrix: [i32; 9], width: u32, height: u32) -> Vec<u8> {
        let mut body = vec![version, 0, 0, 0];
        body.extend(std::iter::repeat_n(0u8, if version == 1 { 32 } else { 20 }));
        body.extend([0u8; 16]);
        for cell in matrix {
            body.extend(cell.to_be_bytes());
        }
        body.extend((width << 16).to_be_bytes());
        body.extend((height << 16).to_be_bytes());
        boxed(b"tkhd", body)
    }

    fn init_segment(tracks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut moov = Vec::new();
        for header in tracks {
            moov.extend(boxed(b"trak", header));
        }
        let mut init = boxed(b"ftyp", b"iso6mp41".to_vec());
        init.extend(boxed(b"moov", moov));
        init
    }

    #[test]
    fn the_ingest_playlist_sits_next_to_the_transcoder_one() {
        let url = format!("{CDN_MEZON_MINIO}/{VIDEO}");
        assert_eq!(
            index_playlist_candidate(&url, &TRANSCODING_ORIGINS),
            Some(format!("{CDN_MEZON_MINIO}/hls/{VIDEO}/index.m3u8"))
        );
        assert_eq!(
            index_playlist_candidate(
                &format!("https://cdn.komu.vn/{VIDEO}"),
                &TRANSCODING_ORIGINS
            ),
            None
        );
        assert_eq!(
            index_playlist_candidate(
                &format!("https://cdn-dev.nccsoft.vn/{VIDEO}"),
                &["https://cdn-dev.nccsoft.vn"]
            ),
            Some(format!("https://cdn-dev.nccsoft.vn/hls/{VIDEO}/index.m3u8"))
        );
    }

    #[tokio::test]
    async fn viewers_share_one_ingest_probe_per_window() {
        let (origin, seen) = fake_minio(vec![(format!("/hls/{VIDEO}/index.m3u8"), INDEX)]).await;
        let url = format!("{origin}/{VIDEO}");
        assert_eq!(
            ingest_playlist_on(&url, &[origin.as_str()], 593_000_123).await,
            Some(format!("{origin}/hls/{VIDEO}/index.m3u8"))
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec![format!("/hls/{VIDEO}/index.m3u8?probe=593000123")]
        );
    }

    #[tokio::test]
    async fn an_ingest_without_a_playlist_yet_keeps_waiting() {
        let (origin, _) = fake_minio(vec![(format!("/hls/{VIDEO}/master.m3u8"), MASTER)]).await;
        assert_eq!(
            ingest_playlist_on(&format!("{origin}/{VIDEO}"), &[origin.as_str()], 1).await,
            None
        );
    }

    #[test]
    fn the_init_segment_is_read_next_to_the_playlist() {
        assert_eq!(
            sibling_url(
                "https://cdn.mezon.ai/hls/a/b.mp4/index.m3u8?probe=1",
                "init.mp4"
            )
            .as_deref(),
            Some("https://cdn.mezon.ai/hls/a/b.mp4/init.mp4")
        );
    }

    #[test]
    fn only_a_turned_or_mirrored_picture_counts_as_turned() {
        let audio = track_header(0, TURNED_90, 0, 0);
        assert!(!has_turned_video_track(&init_segment(vec![track_header(
            0, UPRIGHT, 1920, 1080
        )])));
        assert!(!has_turned_video_track(&init_segment(vec![audio.clone()])));
        for matrix in [TURNED_90, TURNED_180, MIRRORED] {
            assert!(has_turned_video_track(&init_segment(vec![track_header(
                0, matrix, 1920, 1080
            )])));
            assert!(has_turned_video_track(&init_segment(vec![track_header(
                1, matrix, 1920, 1080
            )])));
        }
        assert!(has_turned_video_track(&init_segment(vec![
            audio,
            track_header(0, TURNED_90, 1920, 1080)
        ])));
    }

    #[test]
    fn a_damaged_init_segment_counts_as_upright() {
        let init = init_segment(vec![track_header(0, TURNED_90, 1920, 1080)]);
        assert!(!has_turned_video_track(&init[..init.len() - 10]));
        assert!(!has_turned_video_track(&[
            0, 0, 0, 4, b'm', b'o', b'o', b'v'
        ]));
        assert!(!has_turned_video_track(&[]));
    }

    #[test]
    fn a_growing_playlist_becomes_a_finished_one_with_absolute_uris() {
        let playlist = "https://cdn.mezon.ai/hls/1cf9/208_clip.mp4/index.m3u8";
        let growing = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-START:TIME-OFFSET=0\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.000,\nseg00000.m4s\n#EXTINF:3.5,\nseg00001.m4s\n";
        let (text, duration) = snapshot_playlist(playlist, growing).expect("snapshot");
        assert_eq!(duration, 7.5);
        assert!(text.contains("#EXT-X-PLAYLIST-TYPE:VOD\n"));
        assert!(
            text.contains(
                "#EXT-X-MAP:URI=\"https://cdn.mezon.ai/hls/1cf9/208_clip.mp4/init.mp4\"\n"
            )
        );
        assert!(text.contains("\nhttps://cdn.mezon.ai/hls/1cf9/208_clip.mp4/seg00001.m4s\n"));
        assert!(text.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn a_finished_or_empty_playlist_needs_no_snapshot() {
        let playlist = "https://cdn.mezon.ai/hls/a/b.mp4/index.m3u8";
        assert_eq!(
            snapshot_playlist(playlist, "#EXTM3U\n#EXTINF:4,\nseg0.m4s\n#EXT-X-ENDLIST\n"),
            None
        );
        assert_eq!(
            snapshot_playlist(playlist, "#EXTM3U\n#EXT-X-VERSION:7\n"),
            None
        );
        assert_eq!(snapshot_playlist(playlist, "<html>"), None);
    }

    #[test]
    fn a_resume_never_lands_right_at_a_segment_start() {
        let starts = segment_starts("#EXTM3U\n#EXTINF:4.0,\na\n#EXTINF:4.0,\nb\n#EXTINF:3.5,\nc\n");
        assert_eq!(starts, vec![0.0, 4.0, 8.0]);
        assert_eq!(safe_resume_point(4.0, &starts), 3.0);
        assert_eq!(safe_resume_point(3.95, &starts), 3.0);
        assert_eq!(safe_resume_point(8.3, &starts), 7.0);
        assert_eq!(safe_resume_point(5.5, &starts), 5.5);
        assert_eq!(safe_resume_point(0.0, &starts), 0.0);
        assert_eq!(safe_resume_point(11.5, &starts), 11.5);
    }
}
