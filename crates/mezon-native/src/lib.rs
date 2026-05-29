pub mod audio;
pub mod autostart;
pub mod badge;
pub mod deep_link;
pub mod instance;
pub mod notifications;
pub mod power;
pub mod tray;

/// Opens a URL in the system default browser.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    open::that(url).map_err(|e| anyhow::anyhow!("Failed to open URL: {}", e))
}
