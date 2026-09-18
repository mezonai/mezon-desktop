pub use mezon_client::transport::OutgoingOgp;
pub use mezon_client::{OgpResult, fetch_invite_preview, fetch_ogp};

const EXCLUDED_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "facebook.com",
    "fb.watch",
    "fb.com",
    "tiktok.com",
];

const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"'];

pub fn invite_id_from_url(url: &str) -> Option<String> {
    const NEEDLE: &[u8] = b"/invite/";
    let idx = url
        .as_bytes()
        .windows(NEEDLE.len())
        .position(|window| window.eq_ignore_ascii_case(NEEDLE))?;
    let rest = &url[idx + NEEDLE.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    let id = &rest[..end];
    (!id.is_empty()).then(|| id.to_string())
}

/// The first URL in `text` eligible for an OGP link preview, or `None`.
///
/// Mirrors the mezon-react rule: the first `http`/`https` link that is not an
/// internal-domain link and not a YouTube/Facebook/TikTok link.
pub fn first_previewable_url(text: &str, internal_domain: Option<&str>) -> Option<String> {
    extract_urls(text)
        .into_iter()
        .find(|url| is_previewable(url, internal_domain))
}

fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = text;
    while let Some(pos) = next_scheme(rest) {
        let from = &rest[pos..];
        let end = from
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '|' | '`'))
            .unwrap_or(from.len());
        let token = from[..end].trim_end_matches(TRAILING_PUNCTUATION);
        if token.len() > "https://".len() {
            urls.push(token.to_string());
        }
        let advance = pos + end.max(1);
        if advance >= rest.len() {
            break;
        }
        rest = &rest[advance..];
    }
    urls
}

fn next_scheme(text: &str) -> Option<usize> {
    match (text.find("http://"), text.find("https://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// The invite id in `url`, but only when `url` points at our own domain.
///
/// An invite id is a gateway credential: it is forwarded to the Mezon invite API
/// together with the api key, so a `/invite/<id>` path on a foreign host must
/// never be treated as one.
pub fn internal_invite_id(url: &str, internal_domain: Option<&str>) -> Option<String> {
    is_internal_host(url, internal_domain)
        .then(|| invite_id_from_url(url))
        .flatten()
}

pub fn trusted_invite_id(url: &str, internal_domain: Option<&str>) -> Option<i64> {
    internal_invite_id(url, internal_domain)
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id != 0)
}

/// Whether `url` is a clan invite link — exactly our own host plus a numeric invite id — and
/// so gets the join card rather than an OGP embed. Mirrors mezon-react's `checkInviteLinkValid`
/// (`NX_DOMAIN_URL/invite/` prefix + all-digit id): a `/invite/<code>` path on any other host,
/// a subdomain included (say a third-party app's own invite page), is an ordinary link whose
/// OGP metadata is shown as-is.
pub fn is_clan_invite_url(url: &str, internal_domain: Option<&str>) -> bool {
    let (Some(host), Some(internal)) = (host_of(url), normalized_internal_domain(internal_domain))
    else {
        return false;
    };
    host == internal
        && invite_id_from_url(url).is_some_and(|id| {
            id.bytes().all(|b| b.is_ascii_digit()) && id.bytes().any(|b| b != b'0')
        })
}

fn normalized_internal_domain(internal_domain: Option<&str>) -> Option<String> {
    let internal = internal_domain?
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    (!internal.is_empty()).then_some(internal)
}

fn is_internal_host(url: &str, internal_domain: Option<&str>) -> bool {
    let (Some(host), Some(internal)) = (host_of(url), normalized_internal_domain(internal_domain))
    else {
        return false;
    };
    host_matches(&host, &internal)
}

fn is_previewable(url: &str, internal_domain: Option<&str>) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    if EXCLUDED_HOSTS
        .iter()
        .any(|domain| host_matches(&host, domain))
    {
        return false;
    }
    if is_internal_host(url, internal_domain) {
        return invite_id_from_url(url).is_some();
    }
    true
}

fn host_of(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_plain_url() {
        assert_eq!(
            first_previewable_url("check this https://example.com/post out", None).as_deref(),
            Some("https://example.com/post")
        );
    }

    #[test]
    fn strips_trailing_punctuation() {
        assert_eq!(
            first_previewable_url("see https://example.com/a).", None).as_deref(),
            Some("https://example.com/a")
        );
    }

    #[test]
    fn skips_social_links() {
        assert_eq!(
            first_previewable_url("https://www.youtube.com/watch?v=1", None),
            None
        );
        assert_eq!(first_previewable_url("https://youtu.be/abc", None), None);
        assert_eq!(
            first_previewable_url("a https://tiktok.com/@x https://example.com/b", None).as_deref(),
            Some("https://example.com/b")
        );
    }

    #[test]
    fn skips_internal_domain() {
        assert_eq!(
            first_previewable_url("https://mezon.ai/channels/1", Some("mezon.ai")),
            None
        );
        assert_eq!(
            first_previewable_url("https://app.mezon.ai/x", Some("mezon.ai")),
            None
        );
    }

    #[test]
    fn skips_internal_domain_given_with_a_scheme() {
        assert_eq!(
            first_previewable_url("https://mezon.ai/channels/1/2", Some("https://mezon.ai")),
            None
        );
        assert_eq!(
            first_previewable_url(
                "https://mezon.ai/chat/clans/1/channels/2",
                Some("https://mezon.ai/")
            ),
            None
        );
    }

    #[test]
    fn an_invite_path_on_a_foreign_host_is_not_an_invite() {
        assert_eq!(
            internal_invite_id(
                "https://evil.com/invite/1840670747886882816",
                Some("mezon.ai")
            ),
            None
        );
        assert_eq!(
            internal_invite_id(
                "https://mezon.ai.evil.com/invite/1840670747886882816",
                Some("mezon.ai")
            ),
            None
        );
        assert_eq!(
            internal_invite_id(
                "https://evil.com/x?next=https://mezon.ai/invite/123",
                Some("mezon.ai")
            ),
            None
        );
    }

    #[test]
    fn an_invite_on_our_own_host_is_accepted() {
        assert_eq!(
            internal_invite_id(
                "https://mezon.ai/invite/1840670747886882816",
                Some("https://mezon.ai")
            )
            .as_deref(),
            Some("1840670747886882816")
        );
        assert_eq!(
            internal_invite_id("https://app.mezon.ai/invite/abc-DEF_1", Some("mezon.ai"))
                .as_deref(),
            Some("abc-DEF_1")
        );
    }

    #[test]
    fn no_internal_domain_means_no_invite_is_trusted() {
        assert_eq!(
            internal_invite_id("https://mezon.ai/invite/123", None),
            None
        );
        assert_eq!(
            internal_invite_id("https://mezon.ai/invite/123", Some("")),
            None
        );
    }

    #[test]
    fn only_a_numeric_invite_on_our_domain_is_a_clan_invite() {
        assert!(is_clan_invite_url(
            "https://mezon.ai/invite/1840670747886882816",
            Some("https://mezon.ai")
        ));
        // A third-party app's invite page: OGP embed, not a join card.
        assert!(!is_clan_invite_url(
            "https://meknow.mezon.vn/invite/MZ-3TKK-2D4DZS5N",
            Some("https://mezon.ai")
        ));
        assert!(!is_clan_invite_url(
            "https://mezon.ai/invite/MZ-3TKK-2D4DZS5N",
            Some("https://mezon.ai")
        ));
        // Only the exact host: React matches the `NX_DOMAIN_URL/invite/` prefix, so a
        // subdomain's invite page is not a clan invite either.
        assert!(!is_clan_invite_url(
            "https://app.mezon.ai/invite/1840670747886882816",
            Some("https://mezon.ai")
        ));
        assert!(!is_clan_invite_url(
            "https://mezon.ai/invite/0",
            Some("https://mezon.ai")
        ));
        assert!(!is_clan_invite_url(
            "https://mezon.ai/invite/1840670747886882816",
            None
        ));
    }

    #[test]
    fn trusted_invite_id_requires_internal_host() {
        assert_eq!(
            trusted_invite_id(
                "https://evil.com/invite/1840670747886882816",
                Some("mezon.ai")
            ),
            None
        );
        assert_eq!(
            trusted_invite_id(
                "https://mezon.ai/invite/1840670747886882816",
                Some("mezon.ai")
            ),
            Some(1840670747886882816)
        );
        assert_eq!(
            trusted_invite_id("https://mezon.ai/invite/0", Some("mezon.ai")),
            None
        );
    }

    #[test]
    fn keeps_an_internal_invite_link() {
        assert_eq!(
            first_previewable_url(
                "join https://mezon.ai/invite/1840670747886882816",
                Some("https://mezon.ai")
            )
            .as_deref(),
            Some("https://mezon.ai/invite/1840670747886882816")
        );
    }

    #[test]
    fn falls_through_to_the_next_url_after_an_internal_one() {
        assert_eq!(
            first_previewable_url(
                "https://mezon.ai/channels/1 then https://example.com/b",
                Some("https://mezon.ai")
            )
            .as_deref(),
            Some("https://example.com/b")
        );
    }

    #[test]
    fn an_unset_internal_domain_previews_everything() {
        assert_eq!(
            first_previewable_url("https://mezon.ai/channels/1", Some("")).as_deref(),
            Some("https://mezon.ai/channels/1")
        );
    }

    #[test]
    fn none_when_no_url() {
        assert_eq!(first_previewable_url("just some text", None), None);
    }

    #[test]
    fn extracts_invite_id() {
        assert_eq!(
            invite_id_from_url("https://mezon.ai/invite/1840670747886882816").as_deref(),
            Some("1840670747886882816")
        );
        assert_eq!(
            invite_id_from_url("https://mezon.ai/invite/abc-DEF_123?ref=x").as_deref(),
            Some("abc-DEF_123")
        );
        assert_eq!(
            invite_id_from_url("https://mezon.ai/channels/1").as_deref(),
            None
        );
        assert_eq!(
            invite_id_from_url("https://mezon.ai/invite/").as_deref(),
            None
        );
    }
}
