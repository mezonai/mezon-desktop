pub const PRESIGN_PENDING_MAX_AGE_SEC: i64 = 600;
pub const PRESIGN_INGEST_PENDING_MAX_AGE_SEC: i64 = 24 * 60 * 60;

pub fn normalize_presign_key(key: &str) -> String {
    let segment = key
        .split('?')
        .next()
        .unwrap_or(key)
        .split('/')
        .rfind(|s| !s.is_empty())
        .unwrap_or(key);
    match segment.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < segment.len() => segment[..dot].to_string(),
        _ => segment.to_string(),
    }
}

fn upload_snowflake(key: &str) -> Option<&str> {
    let (head, _) = key.split_once('_')?;
    (!head.is_empty() && head.bytes().all(|b| b.is_ascii_digit())).then_some(head)
}

pub fn presign_keys_match(a: &str, b: &str) -> bool {
    a == b || upload_snowflake(a).is_some_and(|id| upload_snowflake(b) == Some(id))
}

pub fn parse_presign_finish_keys(content: &str) -> Option<Vec<String>> {
    parse_presign_keys(content, "presign_finish")
}

pub fn parse_presign_ingest_keys(content: &str) -> Vec<String> {
    parse_presign_keys(content, "presign_ingest").unwrap_or_default()
}

fn parse_presign_keys(content: &str, field: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let keys = value.as_object()?.get(field)?.as_array()?;
    Some(
        keys.iter()
            .filter_map(|v| v.as_str())
            .map(normalize_presign_key)
            .collect(),
    )
}

pub fn is_ingest_attachment(url: &str, ingest_keys: &[String]) -> bool {
    !ingest_keys.is_empty()
        && presign_key_from_url(url)
            .is_some_and(|key| ingest_keys.iter().any(|k| presign_keys_match(k, &key)))
}

pub fn pending_max_age_sec(ingest: bool) -> i64 {
    if ingest {
        PRESIGN_INGEST_PENDING_MAX_AGE_SEC
    } else {
        PRESIGN_PENDING_MAX_AGE_SEC
    }
}

/// Every CDN an attachment can be presigned onto, mirroring the web client
/// (`isMezonCdnUrl`). The configured `base_img_url` is only one of them, and a
/// message can carry an attachment uploaded by a client pointed at another one —
/// gating solely on our own host would let those through to imgproxy before the
/// upload is confirmed, which caches a not-found for the real object.
const PRESIGN_CDN_HOSTS: [&str; 3] = ["cdn.komu.vn", "cdn.komu.ai", "cdn.mezon.ai"];

fn host_matches(host: &str, allowed: &str) -> bool {
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

pub fn is_mezon_cdn(url: &str, base_img_url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return false;
    };
    if PRESIGN_CDN_HOSTS
        .iter()
        .any(|allowed| host_matches(&host, allowed))
    {
        return true;
    }
    [
        base_img_url,
        mezon_client::cdn_read_base_url(1, ""),
        mezon_client::cdn_read_base_url(2, ""),
    ]
    .into_iter()
    .filter_map(url_host)
    .any(|allowed| host_matches(&host, &allowed))
}

pub fn presign_pending(url: &str, keys: Option<&[String]>, base_img_url: &str) -> bool {
    let Some(keys) = keys else {
        return false;
    };
    if !is_mezon_cdn(url, base_img_url) {
        return false;
    }
    let Some(key) = presign_key_from_url(url) else {
        return false;
    };
    !keys.iter().any(|k| presign_keys_match(k, &key))
}

pub fn all_presign_finished(presignable_count: usize, finish_key_count: usize) -> bool {
    presignable_count > 0 && finish_key_count >= presignable_count
}

pub fn is_expired_presign_attachment(
    url: &str,
    keys: Option<&[String]>,
    base_img_url: &str,
    create_time_seconds: i64,
    now_seconds: i64,
    ingest: bool,
) -> bool {
    if create_time_seconds <= 0 || keys.is_none() {
        return false;
    }
    if !presign_pending(url, keys, base_img_url) {
        return false;
    }
    now_seconds - create_time_seconds >= pending_max_age_sec(ingest)
}

fn presign_key_from_url(url: &str) -> Option<String> {
    if url.is_empty() || url.starts_with("blob:") {
        return None;
    }
    let key = normalize_presign_key(url);
    (!key.is_empty()).then_some(key)
}

fn url_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port.split(':').next().unwrap_or(host_port);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadState {
    Uploading,
    Uploaded,
    Failed,
}

#[derive(Default)]
struct UploadRegistry {
    states: std::collections::HashMap<String, UploadState>,
    local_sources: std::collections::HashMap<String, std::path::PathBuf>,
}

static UPLOADS: std::sync::LazyLock<std::sync::Mutex<UploadRegistry>> =
    std::sync::LazyLock::new(Default::default);

fn uploads() -> std::sync::MutexGuard<'static, UploadRegistry> {
    UPLOADS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn begin_uploading<'a>(keys: impl IntoIterator<Item = &'a String>) {
    let mut registry = uploads();
    for key in keys {
        registry.states.insert(key.clone(), UploadState::Uploading);
    }
}

pub fn finish_uploading(key: &str, uploaded: bool) {
    let state = if uploaded {
        UploadState::Uploaded
    } else {
        UploadState::Failed
    };
    uploads().states.insert(key.to_string(), state);
}

pub fn mark_failed(key: &str) {
    uploads()
        .states
        .insert(key.to_string(), UploadState::Failed);
}

pub fn settle(key: &str) {
    let mut registry = uploads();
    registry.states.remove(key);
    registry.local_sources.remove(key);
}

pub fn clear_failed() {
    let mut registry = uploads();
    let failed: Vec<String> = registry
        .states
        .iter()
        .filter(|(_, state)| **state == UploadState::Failed)
        .map(|(key, _)| key.clone())
        .collect();
    for key in failed {
        registry.states.remove(&key);
        registry.local_sources.remove(&key);
    }
}

pub fn upload_state(key: &str) -> Option<UploadState> {
    uploads().states.get(key).copied()
}

pub fn is_uploading(key: &str) -> bool {
    upload_state(key) == Some(UploadState::Uploading)
}

pub fn remember_local_source(key: &str, path: &std::path::Path) {
    uploads()
        .local_sources
        .insert(key.to_string(), path.to_path_buf());
}

pub fn local_source(key: &str) -> Option<std::path::PathBuf> {
    uploads().local_sources.get(key).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CDN: &str = "https://cdn.example";

    #[test]
    fn a_key_cut_at_the_first_dot_still_clears_its_upload() {
        let url = format!("{CDN}/1cf93b197d401000/2107310745128538112_Screen_Shot_10.17.15.png");
        let web_key = "2107310745128538112_Screen_Shot_10".to_string();
        assert!(!presign_pending(&url, Some(&[web_key]), CDN));
        let other_upload = "2107310745128538113_Screen_Shot_10".to_string();
        assert!(presign_pending(&url, Some(&[other_upload]), CDN));
    }

    #[test]
    fn keys_without_a_snowflake_prefix_must_match_exactly() {
        assert!(presign_keys_match(
            "2107310745128538112",
            "2107310745128538112"
        ));
        assert!(!presign_keys_match(
            "2107310745128538112",
            "2107310745128538113"
        ));
        assert!(!presign_keys_match("photo_a", "photo_b"));
        assert!(presign_keys_match(
            "2107310745128538112_a.b",
            "2107310745128538112_a"
        ));
    }

    #[test]
    fn a_key_moves_from_uploading_to_uploaded_or_failed_until_settled() {
        let (done, broken) = ("registry-done".to_string(), "registry-broken".to_string());
        begin_uploading([&done, &broken]);
        assert!(is_uploading(&done));
        finish_uploading(&done, true);
        finish_uploading(&broken, false);
        assert_eq!(upload_state(&done), Some(UploadState::Uploaded));
        assert_eq!(upload_state(&broken), Some(UploadState::Failed));
        remember_local_source(&done, std::path::Path::new("/tmp/done.png"));
        settle(&done);
        assert_eq!(upload_state(&done), None);
        assert_eq!(local_source(&done), None);
        settle(&broken);
    }

    #[test]
    fn clearing_failed_keys_drops_their_local_sources_but_keeps_running_uploads() {
        let (failed, running) = (
            "registry-clear-failed".to_string(),
            "registry-clear-running".to_string(),
        );
        mark_failed(&failed);
        remember_local_source(&failed, std::path::Path::new("/tmp/failed.png"));
        begin_uploading([&running]);
        remember_local_source(&running, std::path::Path::new("/tmp/running.png"));
        clear_failed();
        assert_eq!(upload_state(&failed), None);
        assert_eq!(local_source(&failed), None);
        assert!(is_uploading(&running));
        assert!(local_source(&running).is_some());
        settle(&running);
    }

    #[test]
    fn normalize_strips_query_path_and_extension() {
        assert_eq!(
            normalize_presign_key("https://cdn.example/a/b/photo.png?x=1"),
            "photo"
        );
        assert_eq!(normalize_presign_key("photo.tar.gz"), "photo.tar");
        assert_eq!(normalize_presign_key("noext"), "noext");
    }

    #[test]
    fn parse_finish_keys_requires_json_object_with_array() {
        assert_eq!(parse_presign_finish_keys("plain text"), None);
        assert_eq!(parse_presign_finish_keys(r#"{"t":"hi"}"#), None);
        assert_eq!(
            parse_presign_finish_keys(r#"{"presign_finish":["a/b/photo.png?x=1","clip.mp4"]}"#),
            Some(vec!["photo".to_string(), "clip".to_string()])
        );
    }

    #[test]
    fn is_mezon_cdn_matches_host_and_subdomain_only() {
        assert!(is_mezon_cdn("https://cdn.example/x/photo.png", CDN));
        assert!(is_mezon_cdn("https://media.cdn.example/x/photo.png", CDN));
        assert!(!is_mezon_cdn("https://example.com/x/photo.png", CDN));
        assert!(!is_mezon_cdn("blob:https://cdn.example/uuid", CDN));
    }

    #[test]
    fn every_mezon_cdn_is_gated_even_when_it_is_not_our_configured_host() {
        for url in [
            "https://cdn.mezon.ai/uploads/photo.png",
            "https://cdn.komu.vn/uploads/photo.png",
            "https://cdn.komu.ai/uploads/photo.png",
        ] {
            assert!(is_mezon_cdn(url, CDN), "{url} must be gated");
            assert!(
                presign_pending(url, Some(&["other".to_string()]), CDN),
                "{url} must stay pending until its key arrives"
            );
        }
    }

    #[test]
    fn cdn_url_absent_from_finish_keys_is_pending_but_non_cdn_is_not() {
        let keys = vec!["other".to_string()];
        assert!(presign_pending(
            "https://cdn.example/uploads/photo.png",
            Some(&keys),
            CDN
        ));
        assert!(!presign_pending(
            "https://example.com/photo.png",
            Some(&keys),
            CDN
        ));

        let finished = vec!["photo".to_string()];
        assert!(!presign_pending(
            "https://cdn.example/uploads/photo.png",
            Some(&finished),
            CDN
        ));
        assert!(!presign_pending(
            "https://cdn.example/uploads/photo.png",
            None,
            CDN
        ));
    }

    #[test]
    fn all_finished_when_key_count_reaches_presignable_count() {
        assert!(!all_presign_finished(0, 0));
        assert!(!all_presign_finished(2, 1));
        assert!(all_presign_finished(2, 2));
        assert!(all_presign_finished(1, 3));
    }

    #[test]
    fn pending_attachment_expires_after_ten_minutes() {
        let keys = vec!["other".to_string()];
        let url = "https://cdn.example/uploads/photo.png";
        assert!(!is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 599,
            false
        ));
        assert!(is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 600,
            false
        ));
        let finished = vec!["photo".to_string()];
        assert!(!is_expired_presign_attachment(
            url,
            Some(&finished),
            CDN,
            1000,
            1_000_000,
            false
        ));
    }

    #[test]
    fn an_ingest_upload_stays_pending_for_a_day() {
        let keys = vec!["other".to_string()];
        let url = "https://cdn.example/uploads/clip.mp4";
        assert!(!is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 86_399,
            true
        ));
        assert!(is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 86_400,
            true
        ));
    }

    #[test]
    fn ingest_keys_name_the_attachments_that_stream_while_uploading() {
        let content = r#"{"t":"","presign_finish":[],"presign_ingest":["1cf93b197d401000/2080000000000000001_clip.mp4"]}"#;
        let keys = parse_presign_ingest_keys(content);
        assert_eq!(keys, vec!["2080000000000000001_clip".to_string()]);
        assert!(is_ingest_attachment(
            "https://cdn.mezon.ai/1cf93b197d401000/2080000000000000001_clip.mp4",
            &keys
        ));
        assert!(!is_ingest_attachment(
            "https://cdn.mezon.ai/1cf93b197d401000/2080000000000000002_other.mp4",
            &keys
        ));
        assert!(parse_presign_ingest_keys(r#"{"t":"","presign_finish":[]}"#).is_empty());
        assert!(parse_presign_ingest_keys("plain text").is_empty());
    }
}
