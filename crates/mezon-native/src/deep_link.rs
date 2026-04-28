/// Deep link scheme registration for `mezonapp://`.
///
/// macOS  : Handled automatically by the `Info.plist` `CFBundleURLTypes` entry
///          embedded in the `.app` bundle.  No runtime code required.
///
/// Windows: Self-registers `HKCU\Software\Classes\mezonapp` on first run.
///          This is the standard approach for apps distributed outside the
///          Microsoft Store (Store apps use the manifest instead).
///
/// Linux  : Writes `~/.local/share/applications/mezon.desktop` and calls
///          `xdg-mime default mezon.desktop x-scheme-handler/mezonapp`.

/// Register the `mezonapp://` URL scheme for the current platform.
/// Safe to call multiple times — each platform is idempotent.
pub fn register_deep_link_scheme() {
    #[cfg(target_os = "macos")]
    {
        // Nothing to do at runtime — Info.plist handles it.
        tracing::debug!("mezonapp:// scheme registration: handled by Info.plist (macOS)");
    }

    #[cfg(target_os = "windows")]
    register_windows();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    register_linux();
}

// ─── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn register_windows() {
    if let Err(e) = try_register_windows() {
        tracing::warn!("Failed to register mezonapp:// scheme on Windows: {e}");
    } else {
        tracing::debug!("mezonapp:// scheme registered in HKCU registry (Windows)");
    }
}

#[cfg(target_os = "windows")]
fn try_register_windows() -> anyhow::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE,
        REG_SZ,
    };

    let exe_path = std::env::current_exe()?.to_string_lossy().to_string();
    let open_cmd = format!("\"{}\" \"%1\"", exe_path);

    // Registry layout:
    //   HKCU\Software\Classes\mezonapp
    //     (Default)           = "URL:mezonapp Protocol"
    //     URL Protocol        = ""
    //     \shell\open\command
    //       (Default)         = "<exe path>" "%1"

    let keys: &[(&str, &str, &str)] = &[
        (r"Software\Classes\mezonapp", "", "URL:mezonapp Protocol"),
        (r"Software\Classes\mezonapp", "URL Protocol", ""),
        (
            r"Software\Classes\mezonapp\shell\open\command",
            "",
            &open_cmd,
        ),
    ];

    for (subkey, value_name, data) in keys {
        let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let value_name_wide: Vec<u16> = value_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let data_bytes: Vec<u8> = data
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(|w| w.to_le_bytes())
            .collect();

        let mut hkey = windows::Win32::System::Registry::HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_wide.as_ptr()),
                0,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )?;

            RegSetValueExW(
                hkey,
                PCWSTR(value_name_wide.as_ptr()),
                0,
                REG_SZ,
                Some(&data_bytes),
            )?;
        }
    }

    Ok(())
}

// ─── Linux ────────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn register_linux() {
    if let Err(e) = try_register_linux() {
        tracing::warn!("Failed to register mezonapp:// scheme on Linux: {e}");
    } else {
        tracing::debug!("mezonapp:// scheme registered via .desktop file (Linux)");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn try_register_linux() -> anyhow::Result<()> {
    use std::io::Write as _;

    let exe_path = std::env::current_exe()?.to_string_lossy().to_string();

    let apps_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share"))
        .join("applications");
    std::fs::create_dir_all(&apps_dir)?;

    let desktop_path = apps_dir.join("mezon.desktop");

    let content = format!(
        "[Desktop Entry]\n\
         Name=Mezon\n\
         Comment=Mezon desktop client\n\
         Exec={exe} %u\n\
         Icon=mezon\n\
         Type=Application\n\
         Categories=Network;InstantMessaging;\n\
         MimeType=x-scheme-handler/mezonapp;\n\
         StartupNotify=true\n",
        exe = exe_path,
    );

    let mut file = std::fs::File::create(&desktop_path)?;
    file.write_all(content.as_bytes())?;
    drop(file);

    // Make executable (required by some DEs).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&desktop_path, std::fs::Permissions::from_mode(0o755))?;
    }

    // Register with xdg-mime (best-effort — may not be installed).
    let status = std::process::Command::new("xdg-mime")
        .args(["default", "mezon.desktop", "x-scheme-handler/mezonapp"])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => tracing::warn!("xdg-mime exited with status {s}"),
        Err(e) => tracing::warn!("xdg-mime not found or failed: {e}"),
    }

    // Also call update-desktop-database to refresh the MIME cache.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(apps_dir.to_string_lossy().as_ref())
        .status();

    Ok(())
}
