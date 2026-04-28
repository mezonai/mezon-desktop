// mezon-updater: Auto-update logic
// Polls cdn.mezon.ai/release/ for new versions, downloads and verifies.
// Stub implementation — will be expanded in Stage 0 CI work.

pub const UPDATE_URL: &str = "https://cdn.mezon.ai/release/";

pub async fn check_for_updates(current_version: &str) -> anyhow::Result<Option<String>> {
    tracing::info!("Checking for updates (current: {})", current_version);
    // TODO: implement update check against UPDATE_URL
    Ok(None)
}
