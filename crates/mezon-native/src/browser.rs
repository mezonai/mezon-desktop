use std::path::{Path, PathBuf};

#[allow(dead_code)]
const CHROMIUM_STEMS: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "google-chrome-unstable",
    "chrome",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "msedge",
    "brave",
    "brave-browser",
    "vivaldi",
    "vivaldi-stable",
];

#[allow(dead_code)]
const CHROMIUM_MAC_APPS: &[&str] = &[
    "google chrome",
    "google chrome beta",
    "google chrome canary",
    "chromium",
    "microsoft edge",
    "microsoft edge beta",
    "brave browser",
    "brave browser beta",
    "vivaldi",
];

#[allow(dead_code)]
fn is_chromium_stem(stem: &str) -> bool {
    let stem = stem.trim().to_ascii_lowercase();
    CHROMIUM_STEMS.iter().any(|known| stem == *known)
}

#[allow(dead_code)]
fn is_chromium_mac_app(app_name: &str) -> bool {
    let lowered = app_name.trim().to_ascii_lowercase();
    let name = lowered.strip_suffix(".app").unwrap_or(&lowered);
    CHROMIUM_MAC_APPS.contains(&name)
}

#[allow(dead_code)]
fn desktop_entry_stem(desktop_file: &str) -> &str {
    let file = desktop_file.trim().rsplit('/').next().unwrap_or("");
    file.strip_suffix(".desktop").unwrap_or(file)
}

#[allow(dead_code)]
fn executable_stem(path: &Path) -> Option<String> {
    Some(path.file_stem()?.to_string_lossy().to_ascii_lowercase())
}

#[allow(dead_code)]
fn chromium_app_bundle(handler: PathBuf) -> Option<PathBuf> {
    let name = handler.file_name()?.to_string_lossy().to_string();
    is_chromium_mac_app(&name).then_some(handler)
}

#[allow(dead_code)]
fn chromium_executable(handler: PathBuf) -> Option<PathBuf> {
    let stem = executable_stem(&handler)?;
    is_chromium_stem(&stem).then_some(handler)
}

#[allow(dead_code)]
fn chromium_desktop_stem(xdg_settings_output: &str) -> Option<&str> {
    let stem = desktop_entry_stem(xdg_settings_output);
    is_chromium_stem(stem).then_some(stem)
}

/// Open `url` in a chromeless browser window when the user's default browser can
/// do it, otherwise fall back to a normal tab.
///
/// Chromium `--app=` is the only cross-platform way to get a window with no
/// toolbar without embedding a webview, which this project deliberately does not
/// do (the wry/WebKitGTK dependency was removed on purpose).
///
/// The window runs in the user's own browser profile, so it outlives the app the
/// same way a normal browser tab does. Placing it over the app window would mean
/// a dedicated profile — Chromium drops `--window-position`/`--window-size` when
/// it hands the command line to a running instance — and a dedicated profile
/// leaves instances behind that the app cannot reliably find and close again.
///
/// Probing the default browser blocks (a LaunchServices round-trip on macOS, an
/// `xdg-settings` subprocess on Linux), so callers must run this off the UI
/// thread — `PlatformStore::app_window_opener` exists to make that the only
/// reachable shape.
pub fn open_url_app_window(url: &str) -> anyhow::Result<()> {
    crate::ensure_http_url(url)?;
    if app_sandbox_strips_launch_arguments() {
        tracing::info!("app sandbox drops browser switches, opening a browser tab instead");
        return crate::open_url(url);
    }
    match default_chromium_browser() {
        Some(browser) => launch_app_window(&browser, url).or_else(|error| {
            tracing::warn!("app-window launch failed, falling back to a browser tab: {error:#}");
            crate::open_url(url)
        }),
        None => crate::open_url(url),
    }
}

fn app_sandbox_strips_launch_arguments() -> bool {
    cfg!(target_os = "macos") && std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some()
}

fn silenced(command: &mut std::process::Command) -> &mut std::process::Command {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
}

#[cfg(target_os = "macos")]
fn launch_app_window(browser: &Path, url: &str) -> anyhow::Result<()> {
    let mut command = std::process::Command::new("/usr/bin/open");
    command
        .arg("-n")
        .arg("-a")
        .arg(browser)
        .arg("--args")
        .arg(format!("--app={url}"));
    let status = silenced(&mut command)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", browser.display()))?;
    if !status.success() {
        anyhow::bail!("failed to open {}: {status}", browser.display());
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_app_window(browser: &Path, url: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(browser);
    command.arg(format!("--app={url}"));
    silenced(&mut command);
    unsafe {
        command.pre_exec(|| {
            match libc::fork() {
                -1 => return Err(std::io::Error::last_os_error()),
                0 => (),
                _ => libc::_exit(0),
            }
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", browser.display()))?;
    child
        .wait()
        .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", browser.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn launch_app_window(browser: &Path, url: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = std::process::Command::new(browser);
    command
        .arg(format!("--app={url}"))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    silenced(&mut command)
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("failed to open {}: {e}", browser.display()))
}

#[cfg(target_os = "macos")]
fn default_chromium_browser() -> Option<PathBuf> {
    chromium_app_bundle(macos::default_http_handler()?)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;

    use core_foundation::base::TCFType;
    use core_foundation::error::{CFError, CFErrorRef};
    use core_foundation::string::CFString;
    use core_foundation::url::{CFURL, CFURLCreateWithString, CFURLRef};

    const LS_ROLES_ALL: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultApplicationURLForURL(
            in_url: CFURLRef,
            in_role_mask: u32,
            out_error: *mut CFErrorRef,
        ) -> CFURLRef;
    }

    pub(super) fn default_http_handler() -> Option<PathBuf> {
        let probe = CFString::new("https://example.com");
        let probe_url = unsafe {
            let raw = CFURLCreateWithString(
                std::ptr::null(),
                probe.as_concrete_TypeRef(),
                std::ptr::null(),
            );
            if raw.is_null() {
                return None;
            }
            CFURL::wrap_under_create_rule(raw)
        };

        let mut error: CFErrorRef = std::ptr::null_mut();
        let raw = unsafe {
            LSCopyDefaultApplicationURLForURL(
                probe_url.as_concrete_TypeRef(),
                LS_ROLES_ALL,
                &mut error,
            )
        };
        if !error.is_null() {
            unsafe { drop(CFError::wrap_under_create_rule(error)) };
        }
        if raw.is_null() {
            return None;
        }
        let handler = unsafe { CFURL::wrap_under_create_rule(raw) };
        handler.to_path()
    }
}

#[cfg(target_os = "windows")]
fn default_chromium_browser() -> Option<PathBuf> {
    chromium_executable(windows_impl::default_http_handler()?)
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::path::PathBuf;

    use windows::Win32::UI::Shell::{ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE, AssocQueryStringW};
    use windows::core::{PCWSTR, w};

    pub(super) fn default_http_handler() -> Option<PathBuf> {
        let mut len: u32 = 0;
        unsafe {
            let _ = AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                ASSOCSTR_EXECUTABLE,
                w!("http"),
                PCWSTR::null(),
                None,
                &mut len,
            );
        }
        if len == 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize];
        let queried = unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                ASSOCSTR_EXECUTABLE,
                w!("http"),
                PCWSTR::null(),
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                &mut len,
            )
        };
        if queried.is_err() {
            return None;
        }
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        let path = String::from_utf16(&buffer[..end]).ok()?;
        (!path.trim().is_empty()).then(|| PathBuf::from(path))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_chromium_browser() -> Option<PathBuf> {
    let output = std::process::Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let desktop = String::from_utf8(output.stdout).ok()?;
    which_binary(chromium_desktop_stem(&desktop)?)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn which_binary(stem: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(stem))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_chromium_executables() {
        assert!(is_chromium_stem("chrome"));
        assert!(is_chromium_stem("MSEdge"));
        assert!(is_chromium_stem(" google-chrome-stable "));
        assert!(is_chromium_stem("brave-browser"));
    }

    #[test]
    fn rejects_non_chromium_executables() {
        assert!(!is_chromium_stem("firefox"));
        assert!(!is_chromium_stem("safari"));
        assert!(!is_chromium_stem("librewolf"));
        assert!(!is_chromium_stem(""));
        assert!(!is_chromium_stem("chrome-remote-desktop"));
    }

    #[test]
    fn recognises_chromium_mac_apps() {
        assert!(is_chromium_mac_app("Google Chrome.app"));
        assert!(is_chromium_mac_app("Microsoft Edge"));
        assert!(is_chromium_mac_app("Brave Browser.app"));
        assert!(is_chromium_mac_app("Google Chrome.APP"));
    }

    #[test]
    fn rejects_non_chromium_mac_apps() {
        assert!(!is_chromium_mac_app("Safari.app"));
        assert!(!is_chromium_mac_app("Firefox.app"));
        assert!(!is_chromium_mac_app("Google Chrome Helper.app"));
        assert!(!is_chromium_mac_app("Chromium.app.app"));
    }

    #[test]
    fn reads_the_stem_of_a_desktop_entry() {
        assert_eq!(
            desktop_entry_stem("google-chrome.desktop\n"),
            "google-chrome"
        );
        assert_eq!(
            desktop_entry_stem("/usr/share/applications/firefox.desktop"),
            "firefox"
        );
        assert_eq!(desktop_entry_stem(""), "");
    }

    #[test]
    fn reads_the_stem_of_an_executable() {
        assert_eq!(
            executable_stem(Path::new("/opt/google/chrome/chrome")).as_deref(),
            Some("chrome")
        );
        assert_eq!(
            executable_stem(Path::new("MSEdge.exe")).as_deref(),
            Some("msedge")
        );
        assert_eq!(executable_stem(Path::new("")), None);
    }

    #[test]
    fn selects_only_chromium_app_bundles() {
        let chrome = PathBuf::from("/Applications/Google Chrome.app");
        assert_eq!(chromium_app_bundle(chrome.clone()), Some(chrome));
        assert_eq!(
            chromium_app_bundle(PathBuf::from("/Applications/Safari.app")),
            None
        );
        assert_eq!(chromium_app_bundle(PathBuf::from("/")), None);
    }

    #[test]
    fn selects_only_chromium_executables() {
        let edge = PathBuf::from("/Program Files/Microsoft/Edge/msedge.exe");
        assert_eq!(chromium_executable(edge.clone()), Some(edge));
        assert_eq!(
            chromium_executable(PathBuf::from("/Windows/system32/OpenWith.exe")),
            None
        );
        assert_eq!(
            chromium_executable(PathBuf::from("/usr/lib/firefox/firefox")),
            None
        );
    }

    #[test]
    fn selects_only_chromium_desktop_entries() {
        assert_eq!(
            chromium_desktop_stem("google-chrome.desktop\n"),
            Some("google-chrome")
        );
        assert_eq!(chromium_desktop_stem("firefox.desktop\n"), None);
        assert_eq!(chromium_desktop_stem("com.google.Chrome.desktop\n"), None);
        assert_eq!(chromium_desktop_stem(""), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "environment dependent: reports the default browser of the machine it runs on"]
    fn reports_the_default_browser() {
        println!("default http handler: {:?}", macos::default_http_handler());
        println!("chromium app mode target: {:?}", default_chromium_browser());
    }

    #[test]
    fn app_window_rejects_non_http_schemes() {
        assert!(crate::ensure_http_url("file:///etc/passwd").is_err());
        assert!(crate::ensure_http_url("javascript:alert(1)").is_err());
        assert!(crate::ensure_http_url("--app=evil").is_err());
        assert!(crate::ensure_http_url("").is_err());
        assert!(crate::ensure_http_url("https://mezon.ai/app").is_ok());
    }
}
