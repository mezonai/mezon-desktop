use std::time::Duration;

use anyhow::Result;
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, HttpRequestExt as _, RedirectPolicy, http};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use url::Url;

use crate::transport_runtime::{http_client, runtime};

const SOCIAL_TIMEOUT: Duration = Duration::from_secs(10);
const OEMBED_MAX_BYTES: usize = 64 * 1024;
const TIKTOK_MAX_REDIRECTS: usize = 3;
const SOCIAL_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; MezonDesktop/1.0; +https://mezon.ai) LinkPreview";
const YOUTUBE_ID_LEN: usize = 11;
const YOUTUBE_POSTER_BASE: &str = "https://i.ytimg.com/vi/";
const YOUTUBE_POSTER_QUALITIES: [&str; 2] = ["maxresdefault.jpg", "mqdefault.jpg"];
const YOUTUBE_MARKERS: [&str; 6] = [
    "youtube.com/watch?v=",
    "youtube.com/embed/",
    "youtube.com/shorts/",
    "youtube.com/v/",
    "youtube.com/e/",
    "youtu.be/",
];

pub fn youtube_video_id(url: &str) -> Option<&str> {
    let rest = YOUTUBE_MARKERS
        .iter()
        .find_map(|marker| url.split_once(marker).map(|(_, rest)| rest))?;
    let id = rest
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .next()?;
    (id.len() >= YOUTUBE_ID_LEN).then(|| &id[..YOUTUBE_ID_LEN])
}

pub fn youtube_poster_url(video_id: &str) -> String {
    format!(
        "{YOUTUBE_POSTER_BASE}{video_id}/{}",
        YOUTUBE_POSTER_QUALITIES[0]
    )
}

pub fn youtube_poster_fallback(poster_url: &str) -> Option<String> {
    let (video_id, quality) = poster_url
        .strip_prefix(YOUTUBE_POSTER_BASE)?
        .split_once('/')?;
    let current = YOUTUBE_POSTER_QUALITIES
        .iter()
        .position(|candidate| *candidate == quality)?;
    let next = YOUTUBE_POSTER_QUALITIES.get(current + 1)?;
    Some(format!("{YOUTUBE_POSTER_BASE}{video_id}/{next}"))
}

/// `?t=90`, `?t=1m30s` and `?start=90` all mean "start 90 seconds in".
fn youtube_start_seconds(link: &str) -> u64 {
    let Ok(url) = Url::parse(link) else {
        return 0;
    };
    let Some((_, raw)) = url
        .query_pairs()
        .find(|(key, _)| key == "t" || key == "start")
    else {
        return 0;
    };
    if let Ok(seconds) = raw.parse::<u64>() {
        return seconds;
    }
    let mut total = 0u64;
    let mut digits = 0u64;
    let mut seen_digit = false;
    for c in raw.chars() {
        match c {
            '0'..='9' => {
                digits = digits
                    .saturating_mul(10)
                    .saturating_add((c as u64) - ('0' as u64));
                seen_digit = true;
            }
            'h' | 'H' => {
                total += digits * 3600;
                digits = 0;
            }
            'm' | 'M' => {
                total += digits * 60;
                digits = 0;
            }
            's' | 'S' => {
                total += digits;
                digits = 0;
            }
            _ => return 0,
        }
    }
    if seen_digit { total } else { 0 }
}

/// Public (no-login) web route that iframes a single YouTube video.
pub const YOUTUBE_EMBED_ROUTE: &str = "/embed/youtube";

/// A YouTube link opens on the web client's own player page instead of on the
/// whole youtube.com site — what the reader clicked is the video, not the site
/// around it.
///
/// Returns `None` for every other link, so callers fall back to the link itself.
pub fn build_youtube_embed_route_url(domain_url: &str, link: &str) -> Option<String> {
    let video_id = youtube_video_id(link)?;
    let base = domain_url.trim_end_matches('/');
    let start = youtube_start_seconds(link);
    if start > 0 {
        Some(format!(
            "{base}{YOUTUBE_EMBED_ROUTE}?v={video_id}&t={start}"
        ))
    } else {
        Some(format!("{base}{YOUTUBE_EMBED_ROUTE}?v={video_id}"))
    }
}

pub fn is_youtube_shorts(url: &str) -> bool {
    url.contains("youtube.com/shorts/")
}

pub fn is_tiktok_link(url: &str) -> bool {
    url.contains("tiktok.com/")
}

fn is_tiktok_video_page(url: &str) -> bool {
    url.split_once("tiktok.com/@")
        .and_then(|(_, rest)| rest.split_once("/video/"))
        .and_then(|(_, id)| id.chars().next())
        .is_some_and(|first| first.is_ascii_digit())
}

fn tiktok_oembed_url(video_url: &str) -> String {
    format!(
        "https://www.tiktok.com/oembed?url={}",
        utf8_percent_encode(video_url, NON_ALPHANUMERIC)
    )
}

fn tiktok_poster_from_oembed(body: &[u8]) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_slice(body).ok()?;
    let url = payload.get("thumbnail_url")?.as_str()?.trim();
    url.starts_with("https://").then(|| url.to_string())
}

pub async fn fetch_tiktok_poster(url: &str) -> Result<String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        anyhow::bail!("unsupported url scheme");
    }
    let url = url.to_string();
    runtime()
        .spawn(async move {
            let outcome = tokio::time::timeout(SOCIAL_TIMEOUT, async move {
                let page = resolve_tiktok_page(url).await?;
                let request = http::Request::builder()
                    .method(http::Method::GET)
                    .uri(tiktok_oembed_url(&page))
                    .header(http::header::USER_AGENT, SOCIAL_USER_AGENT)
                    .header(http::header::ACCEPT, "application/json")
                    .body(AsyncBody::empty())?;
                let mut response = http_client().send(request).await?;
                if !response.status().is_success() {
                    anyhow::bail!("tiktok oembed failed with status {}", response.status());
                }
                let mut bytes = Vec::new();
                response
                    .body_mut()
                    .take(OEMBED_MAX_BYTES as u64)
                    .read_to_end(&mut bytes)
                    .await?;
                tiktok_poster_from_oembed(&bytes)
                    .ok_or_else(|| anyhow::anyhow!("tiktok oembed carried no thumbnail"))
            })
            .await;
            match outcome {
                Ok(inner) => inner,
                Err(_) => anyhow::bail!(
                    "tiktok poster fetch timed out after {}s",
                    SOCIAL_TIMEOUT.as_secs()
                ),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("tiktok poster fetch task failed: {e}"))?
}

async fn resolve_tiktok_page(url: String) -> Result<String> {
    let mut current = url;
    for _ in 0..TIKTOK_MAX_REDIRECTS {
        if is_tiktok_video_page(&current) {
            return Ok(current);
        }
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(&current)
            .header(http::header::USER_AGENT, SOCIAL_USER_AGENT)
            .follow_redirects(RedirectPolicy::NoFollow)
            .body(AsyncBody::empty())?;
        let response = http_client().send(request).await?;
        if !response.status().is_redirection() {
            break;
        }
        let Some(location) = response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
        else {
            break;
        };
        current = location.to_string();
    }
    if is_tiktok_video_page(&current) {
        Ok(current)
    } else {
        anyhow::bail!("not a tiktok video url")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_ids_come_from_every_link_shape() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=lHW3fsJQ1sg"),
            Some("lHW3fsJQ1sg")
        );
        assert_eq!(
            youtube_video_id("https://youtu.be/lHW3fsJQ1sg?t=42"),
            Some("lHW3fsJQ1sg")
        );
        assert_eq!(
            youtube_video_id("https://m.youtube.com/shorts/lHW3fsJQ1sg"),
            Some("lHW3fsJQ1sg")
        );
        assert_eq!(
            youtube_video_id("https://www.youtube.com/embed/lHW3fsJQ1sg?rel=0"),
            Some("lHW3fsJQ1sg")
        );
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=lHW3fsJQ1sg&list=PL1"),
            Some("lHW3fsJQ1sg")
        );
    }

    #[test]
    fn youtube_id_needs_a_full_length_id() {
        assert_eq!(youtube_video_id("https://youtu.be/abc"), None);
        assert_eq!(youtube_video_id("https://example.com/watch?v=abc"), None);
    }

    #[test]
    fn youtube_poster_is_the_hq_still() {
        assert_eq!(
            youtube_poster_url("lHW3fsJQ1sg"),
            "https://i.ytimg.com/vi/lHW3fsJQ1sg/maxresdefault.jpg"
        );
        assert_eq!(
            youtube_poster_fallback("https://i.ytimg.com/vi/lHW3fsJQ1sg/maxresdefault.jpg")
                .as_deref(),
            Some("https://i.ytimg.com/vi/lHW3fsJQ1sg/mqdefault.jpg")
        );
        assert_eq!(
            youtube_poster_fallback("https://i.ytimg.com/vi/lHW3fsJQ1sg/mqdefault.jpg"),
            None
        );
        assert_eq!(
            youtube_poster_fallback("https://p16.tiktokcdn.com/a.image"),
            None
        );
        assert!(is_youtube_shorts(
            "https://www.youtube.com/shorts/lHW3fsJQ1sg"
        ));
        assert!(!is_youtube_shorts(
            "https://www.youtube.com/watch?v=lHW3fsJQ1sg"
        ));
    }

    #[test]
    fn tiktok_video_pages_need_a_numeric_id() {
        assert!(is_tiktok_video_page(
            "https://www.tiktok.com/@user/video/123"
        ));
        assert!(!is_tiktok_video_page(
            "https://www.tiktok.com/@user/video/١٢٣"
        ));
        assert!(!is_tiktok_video_page("https://vm.tiktok.com/ZSjnnkQeP/"));
        assert!(is_tiktok_link("https://vm.tiktok.com/ZSjnnkQeP/"));
    }

    #[test]
    fn tiktok_oembed_url_encodes_the_video_url() {
        assert_eq!(
            tiktok_oembed_url("https://www.tiktok.com/@user/video/123"),
            "https://www.tiktok.com/oembed?url=https%3A%2F%2Fwww%2Etiktok%2Ecom%2F%40user%2Fvideo%2F123"
        );
    }

    #[test]
    fn tiktok_poster_reads_the_oembed_thumbnail() {
        let body = br#"{"type":"video","thumbnail_url":"https://p16.tiktokcdn.com/a.image?x=1"}"#;
        assert_eq!(
            tiktok_poster_from_oembed(body).as_deref(),
            Some("https://p16.tiktokcdn.com/a.image?x=1")
        );
        assert_eq!(tiktok_poster_from_oembed(br#"{"type":"video"}"#), None);
        assert_eq!(tiktok_poster_from_oembed(b"not json"), None);
    }

    #[test]
    fn a_youtube_link_points_at_the_public_embed_route() {
        assert_eq!(
            build_youtube_embed_route_url(
                "https://mezon.ai/",
                "https://www.youtube.com/watch?v=lHW3fsJQ1sg"
            )
            .as_deref(),
            Some("https://mezon.ai/embed/youtube?v=lHW3fsJQ1sg")
        );
    }

    #[test]
    fn carries_the_start_offset_over_to_the_embed_route() {
        assert_eq!(
            build_youtube_embed_route_url("https://mezon.ai", "https://youtu.be/lHW3fsJQ1sg?t=90")
                .as_deref(),
            Some("https://mezon.ai/embed/youtube?v=lHW3fsJQ1sg&t=90")
        );
        assert_eq!(
            build_youtube_embed_route_url(
                "https://mezon.ai",
                "https://youtu.be/lHW3fsJQ1sg?t=1h2m3s"
            )
            .as_deref(),
            Some("https://mezon.ai/embed/youtube?v=lHW3fsJQ1sg&t=3723")
        );
    }

    #[test]
    fn leaves_every_other_link_alone() {
        assert_eq!(
            build_youtube_embed_route_url("https://mezon.ai", "https://www.tiktok.com/@u/video/1"),
            None
        );
    }
}
