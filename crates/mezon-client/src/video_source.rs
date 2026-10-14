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
const HLS_PLAYLIST_SUFFIX: &str = ".m3u8";
const HLS_PLAYLIST_TAG: &[u8] = b"#EXTM3U";
const HLS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const HLS_TRANSCODE_MIN_SIZE: i32 = 10 * 1024 * 1024;
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
    hls_playlist_candidate(url, &TRANSCODING_ORIGINS)
}

pub fn may_be_transcoded(url: &str) -> bool {
    transcodable_file(url, &TRANSCODING_ORIGINS)
}

pub async fn transcoded_hls_playlist(url: &str) -> Option<String> {
    hls_playlist_on(url, &TRANSCODING_ORIGINS, probe_nonce()).await
}

async fn hls_playlist_on(url: &str, origins: &[&str], nonce: u64) -> Option<String> {
    let playlist = hls_playlist_candidate(url, origins)?;
    let head = crate::transport_runtime::fetch_prefix(
        &format!("{playlist}?probe={nonce:x}"),
        HLS_PLAYLIST_TAG.len(),
        HLS_PROBE_TIMEOUT,
    )
    .await?;
    (head == HLS_PLAYLIST_TAG).then_some(playlist)
}

fn hls_playlist_candidate(url: &str, origins: &[&str]) -> Option<String> {
    transcodable_path(url, origins)
        .map(|(origin, path)| format!("{origin}/{HLS_OUTPUT_PREFIX}{path}/{HLS_MASTER_PLAYLIST}"))
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{
        CDN_MEZON_MINIO, HLS_TRANSCODE_MIN_SIZE, TRANSCODING_ORIGINS, hls_playlist_candidate,
        hls_playlist_on, may_be_transcoded, wants_hls_transcode,
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
}
