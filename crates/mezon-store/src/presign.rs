pub const PRESIGN_PENDING_MAX_AGE_SEC: i64 = 600;

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

pub fn parse_presign_finish_keys(content: &str) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let keys = value.as_object()?.get("presign_finish")?.as_array()?;
    Some(
        keys.iter()
            .filter_map(|v| v.as_str())
            .map(normalize_presign_key)
            .collect(),
    )
}

pub fn is_mezon_cdn(url: &str, base_img_url: &str) -> bool {
    let (Some(host), Some(allowed)) = (url_host(url), url_host(base_img_url)) else {
        return false;
    };
    host == allowed || host.ends_with(&format!(".{allowed}"))
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
    !keys.iter().any(|k| k == &key)
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
) -> bool {
    if create_time_seconds <= 0 || keys.is_none() {
        return false;
    }
    if !presign_pending(url, keys, base_img_url) {
        return false;
    }
    now_seconds - create_time_seconds >= PRESIGN_PENDING_MAX_AGE_SEC
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

#[cfg(test)]
mod tests {
    use super::*;

    const CDN: &str = "https://cdn.mezon.ai";

    #[test]
    fn normalize_strips_query_path_and_extension() {
        assert_eq!(
            normalize_presign_key("https://cdn.mezon.ai/a/b/photo.png?x=1"),
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
        assert!(is_mezon_cdn("https://cdn.mezon.ai/x/photo.png", CDN));
        assert!(is_mezon_cdn("https://media.cdn.mezon.ai/x/photo.png", CDN));
        assert!(!is_mezon_cdn("https://example.com/x/photo.png", CDN));
        assert!(!is_mezon_cdn("blob:https://cdn.mezon.ai/uuid", CDN));
    }

    #[test]
    fn cdn_url_absent_from_finish_keys_is_pending_but_non_cdn_is_not() {
        let keys = vec!["other".to_string()];
        assert!(presign_pending(
            "https://cdn.mezon.ai/uploads/photo.png",
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
            "https://cdn.mezon.ai/uploads/photo.png",
            Some(&finished),
            CDN
        ));
        assert!(!presign_pending(
            "https://cdn.mezon.ai/uploads/photo.png",
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
        let url = "https://cdn.mezon.ai/uploads/photo.png";
        assert!(!is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 599
        ));
        assert!(is_expired_presign_attachment(
            url,
            Some(&keys),
            CDN,
            1000,
            1000 + 600
        ));
        let finished = vec!["photo".to_string()];
        assert!(!is_expired_presign_attachment(
            url,
            Some(&finished),
            CDN,
            1000,
            1_000_000
        ));
    }
}
