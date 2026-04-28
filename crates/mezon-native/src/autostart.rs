/// Auto-start on login using the `auto-launch` crate.
///
/// On macOS: adds a Login Item via LaunchServices.
/// On Windows: writes to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
/// On Linux: creates/removes `~/.config/autostart/mezon.desktop`.
use anyhow::Result;
use auto_launch::AutoLaunchBuilder;

/// Sync the auto-start state with what is stored in Settings.
/// Call once at app startup after settings are loaded.
pub fn sync_auto_start(enabled: bool) {
    if let Err(e) = set_auto_start(enabled) {
        tracing::warn!("Failed to sync auto-start (enabled={enabled}): {e}");
    } else {
        tracing::debug!("Auto-start synced: enabled={enabled}");
    }
}

/// Enable or disable the login item.
pub fn set_auto_start(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let auto = AutoLaunchBuilder::new()
        .set_app_name("Mezon")
        .set_app_path(exe.to_str().unwrap_or("mezon"))
        .build()?;

    if enabled {
        auto.enable()?;
    } else {
        // Only disable if currently enabled to avoid spurious errors.
        if auto.is_enabled()? {
            auto.disable()?;
        }
    }
    Ok(())
}
