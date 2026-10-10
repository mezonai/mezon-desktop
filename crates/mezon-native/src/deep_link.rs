//! Deep link scheme registration for `mezonapp://`.
//!
//! macOS  : Handled automatically by the `Info.plist` `CFBundleURLTypes` entry
//!          embedded in the `.app` bundle.  No runtime code required.
//!
//! Windows: Self-registers `HKCU\Software\Classes\mezonapp` on first run.
//!          This is the standard approach for apps distributed outside the
//!          Microsoft Store (Store apps use the manifest instead).
//!
//! Linux  : Writes `~/.local/share/applications/mezon.desktop` and calls
//!          `xdg-mime default mezon.desktop x-scheme-handler/mezonapp`.

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
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCreateKeyExW,
        RegSetValueExW,
    };
    use windows::core::PCWSTR;
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
            let result = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_wide.as_ptr()),
                Some(0),
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            );
            if result != windows::Win32::Foundation::NO_ERROR {
                return Err(anyhow::anyhow!("RegCreateKeyExW failed"));
            }

            let result = RegSetValueExW(
                hkey,
                PCWSTR(value_name_wide.as_ptr()),
                Some(0),
                REG_SZ,
                Some(&data_bytes),
            );
            if result != windows::Win32::Foundation::NO_ERROR {
                return Err(anyhow::anyhow!("Failed to set registry value"));
            }
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
const DESKTOP_ID: &str = "mezon.desktop";

#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
#[derive(Debug, PartialEq, Eq)]
enum LinuxRegistration {
    Keep,
    RemoveGenerated,
    Write,
}

#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
fn generated_desktop_entry(exe_path: &str) -> String {
    format!(
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
    )
}

#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
fn generated_entry_exe(content: &str) -> Option<&str> {
    let exe_path = content
        .lines()
        .find_map(|line| line.strip_prefix("Exec="))?
        .strip_suffix(" %u")?;
    let is_plain_path = exe_path.starts_with('/') && !exe_path.contains(char::is_whitespace);
    (is_plain_path && content == generated_desktop_entry(exe_path)).then_some(exe_path)
}

#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
fn plan_linux_registration(
    user_entry: Option<&str>,
    shared_entry_installed: bool,
    exe_path: &str,
    exe_exists: impl Fn(&str) -> bool,
) -> LinuxRegistration {
    let Some(existing) = user_entry else {
        return if shared_entry_installed {
            LinuxRegistration::Keep
        } else {
            LinuxRegistration::Write
        };
    };
    match generated_entry_exe(existing) {
        None => LinuxRegistration::Keep,
        Some(_) if shared_entry_installed => LinuxRegistration::RemoveGenerated,
        Some(recorded) if recorded != exe_path && !exe_exists(recorded) => LinuxRegistration::Write,
        Some(_) => LinuxRegistration::Keep,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn try_register_linux() -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()?.to_string_lossy().to_string();

    let Some(apps_dir) = dirs::data_local_dir().map(|dir| dir.join("applications")) else {
        return Ok(());
    };
    let desktop_path = apps_dir.join(DESKTOP_ID);
    let user_entry = match std::fs::read_to_string(&desktop_path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let shared_entry_installed = crate::browser::desktop_entry_dirs()
        .iter()
        .filter(|dir| **dir != apps_dir)
        .any(|dir| dir.join(DESKTOP_ID).is_file());

    let plan = plan_linux_registration(
        user_entry.as_deref(),
        shared_entry_installed,
        &exe_path,
        |recorded| std::path::Path::new(recorded).is_file(),
    );
    match plan {
        LinuxRegistration::Keep => Ok(()),
        LinuxRegistration::RemoveGenerated => {
            std::fs::remove_file(&desktop_path)?;
            refresh_desktop_database(&apps_dir);
            Ok(())
        }
        LinuxRegistration::Write => write_desktop_entry(&apps_dir, &desktop_path, &exe_path),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn write_desktop_entry(
    apps_dir: &std::path::Path,
    desktop_path: &std::path::Path,
    exe_path: &str,
) -> anyhow::Result<()> {
    use std::io::Write as _;

    std::fs::create_dir_all(apps_dir)?;

    let mut file = std::fs::File::create(desktop_path)?;
    file.write_all(generated_desktop_entry(exe_path).as_bytes())?;
    drop(file);

    // Make executable (required by some DEs).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(desktop_path, std::fs::Permissions::from_mode(0o755))?;
    }

    // Register with xdg-mime (best-effort — may not be installed).
    let status = std::process::Command::new("xdg-mime")
        .args(["default", DESKTOP_ID, "x-scheme-handler/mezonapp"])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => tracing::warn!("xdg-mime exited with status {s}"),
        Err(e) => tracing::warn!("xdg-mime not found or failed: {e}"),
    }

    refresh_desktop_database(apps_dir);

    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn refresh_desktop_database(apps_dir: &std::path::Path) {
    let _ = std::process::Command::new("update-desktop-database")
        .arg(apps_dir)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/usr/bin/mezon";

    fn plan(user_entry: Option<&str>, shared_entry_installed: bool) -> LinuxRegistration {
        plan_linux_registration(user_entry, shared_entry_installed, EXE, |_| true)
    }

    #[test]
    fn writes_entry_when_none_is_installed() {
        assert_eq!(plan(None, false), LinuxRegistration::Write);
    }

    #[test]
    fn leaves_packaged_entry_alone() {
        assert_eq!(plan(None, true), LinuxRegistration::Keep);
    }

    #[test]
    fn removes_generated_copy_that_shadows_packaged_entry() {
        let generated = generated_desktop_entry(EXE);
        assert_eq!(
            plan(Some(&generated), true),
            LinuxRegistration::RemoveGenerated
        );
        let dev_build = generated_desktop_entry("/home/u/mezon-desktop/target/debug/mezon");
        assert_eq!(
            plan(Some(&dev_build), true),
            LinuxRegistration::RemoveGenerated
        );
    }

    #[test]
    fn never_touches_an_entry_it_did_not_generate() {
        let installer_entry = include_str!("../../../packaging/linux/mezon.desktop").replace(
            "Exec=/usr/bin/mezon",
            "Exec=/home/u/.local/share/mezon/mezon",
        );
        let edited_exec = generated_desktop_entry(EXE).replace("Exec=", "Exec=env FOO=1 ");
        let edited_name = generated_desktop_entry(EXE).replace("Name=Mezon", "Name=Mezon Work");
        for entry in [installer_entry, edited_exec, edited_name] {
            for shared_entry_installed in [false, true] {
                assert_eq!(
                    plan_linux_registration(Some(&entry), shared_entry_installed, EXE, |_| false),
                    LinuxRegistration::Keep
                );
            }
        }
    }

    #[test]
    fn keeps_generated_entry_that_still_launches() {
        assert_eq!(
            plan(Some(&generated_desktop_entry(EXE)), false),
            LinuxRegistration::Keep
        );
        let other_build = generated_desktop_entry("/opt/mezon/mezon");
        assert_eq!(plan(Some(&other_build), false), LinuxRegistration::Keep);
    }

    #[test]
    fn repoints_generated_entry_whose_binary_is_gone() {
        let stale = generated_desktop_entry("/home/u/Downloads/mezon");
        assert_eq!(
            plan_linux_registration(Some(&stale), false, EXE, |recorded| recorded == EXE),
            LinuxRegistration::Write
        );
    }
}
