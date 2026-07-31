pub mod audio;
pub mod autostart;
pub mod badge;
pub mod cli_install;
pub mod control;
pub mod deep_link;
pub mod instance;
pub mod location;
pub mod notifications;
pub mod power;
pub mod tray;
pub mod window_icon;

/// Opens a URL in the system default browser.
///
/// Only `http://` and `https://` URLs are accepted; other schemes are rejected.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(anyhow::anyhow!(
            "open_url rejected: only http(s) scheme is allowed"
        ));
    }
    open::that_detached(url).map_err(|e| anyhow::anyhow!("Failed to open URL: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_rejects_non_http_schemes() {
        assert!(open_url("ftp://mezon.ai/").is_err());
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("data:text/html,<h1>hi</h1>").is_err());
        assert!(open_url("").is_err());
    }
}
