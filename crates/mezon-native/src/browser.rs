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
/// same way a normal browser tab does and nothing reports when it closes.
/// [`open_url_managed_app_window`] is the variant for callers that need that
/// report: it runs a dedicated profile so the window is its own process.
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
        None => {
            tracing::info!("no Chromium-based default browser, opening a browser tab instead");
            crate::open_url(url)
        }
    }
}

/// Opens a Chromium app in a dedicated profile and blocks until that window
/// closes. Without a Chromium default browser (or inside the macOS app sandbox)
/// it opens a normal browser tab instead and returns at once.
pub fn open_url_managed_app_window(url: &str, key: &str) -> anyhow::Result<()> {
    crate::ensure_http_url(url)?;
    if app_sandbox_strips_launch_arguments() {
        tracing::info!("app sandbox drops browser switches, opening a browser tab instead");
        return crate::open_url(url);
    }
    let Some(browser) = default_chromium_browser() else {
        tracing::info!("no Chromium-based default browser, opening a browser tab instead");
        return crate::open_url(url);
    };
    let profile = managed_profile_dir(key);
    let spawned = std::fs::create_dir_all(&profile)
        .map_err(anyhow::Error::from)
        .and_then(|()| spawn_managed_app_window(&browser, url, &profile));
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(
                "managed app-window launch failed, falling back to a browser tab: {error:#}"
            );
            return crate::open_url(url);
        }
    };
    let closed = wait_for_managed_app_window(&mut child);
    if let Err(error) = std::fs::remove_dir_all(&profile) {
        tracing::debug!(path = %profile.display(), "managed browser profile cleanup failed: {error}");
    }
    closed
}

fn managed_profile_dir(key: &str) -> PathBuf {
    let safe_key: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    std::env::temp_dir()
        .join("mezon-managed-apps")
        .join(safe_key)
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

fn managed_switches(url: &str, profile: &Path) -> [String; 5] {
    [
        format!("--app={url}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-mode".into(),
    ]
}

#[cfg(target_os = "windows")]
fn spawn_managed_app_window(
    browser: &Path,
    url: &str,
    profile: &Path,
) -> anyhow::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new(browser);
    command
        .args(managed_switches(url, profile))
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    Ok(silenced(&mut command).spawn()?)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_managed_app_window(
    exec: &[String],
    url: &str,
    profile: &Path,
) -> anyhow::Result<std::process::Child> {
    let argv = exec_with_app_url(exec, url);
    let Some((program, args)) = argv.split_first() else {
        anyhow::bail!("browser desktop entry has an empty Exec line");
    };
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .args(managed_switches(url, profile).into_iter().skip(1));
    Ok(silenced(&mut command).spawn()?)
}

#[cfg(target_os = "macos")]
fn spawn_managed_app_window(
    browser: &Path,
    url: &str,
    profile: &Path,
) -> anyhow::Result<std::process::Child> {
    let name = browser
        .file_stem()
        .ok_or_else(|| anyhow::anyhow!("invalid browser bundle"))?;
    let executable = browser.join("Contents").join("MacOS").join(name);
    let mut command = std::process::Command::new(&executable);
    command.args(managed_switches(url, profile));
    Ok(silenced(&mut command).spawn()?)
}

#[cfg(not(target_os = "macos"))]
fn wait_for_managed_app_window(child: &mut std::process::Child) -> anyhow::Result<()> {
    let status = child.wait()?;
    if !status.success() {
        tracing::warn!("managed browser exited with {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn wait_for_managed_app_window(child: &mut std::process::Child) -> anyhow::Result<()> {
    const POLL: std::time::Duration = std::time::Duration::from_millis(500);
    const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
    let pid = child.id();
    let mut watch = ManagedWindowWatch::new(std::time::Instant::now());
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        let windows = macos::app_window_count(pid);
        if watch.observe(windows, std::time::Instant::now()) == WindowWatch::Quit {
            break;
        }
        std::thread::sleep(POLL);
    }
    macos::request_quit(pid);
    let quit_requested = std::time::Instant::now();
    while child.try_wait()?.is_none() {
        if quit_requested.elapsed() >= QUIT_GRACE {
            child.kill()?;
            child.wait()?;
            break;
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum WindowWatch {
    Keep,
    Quit,
}

#[cfg(any(target_os = "macos", test))]
struct ManagedWindowWatch {
    started: std::time::Instant,
    window_seen: bool,
}

#[cfg(any(target_os = "macos", test))]
impl ManagedWindowWatch {
    const STARTUP_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

    fn new(started: std::time::Instant) -> Self {
        Self {
            started,
            window_seen: false,
        }
    }

    fn observe(&mut self, windows: usize, now: std::time::Instant) -> WindowWatch {
        if windows > 0 {
            self.window_seen = true;
            return WindowWatch::Keep;
        }
        if self.window_seen || now.duration_since(self.started) >= Self::STARTUP_GRACE {
            WindowWatch::Quit
        } else {
            WindowWatch::Keep
        }
    }
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
fn launch_app_window(exec: &[String], url: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let argv = exec_with_app_url(exec, url);
    let Some((program, args)) = argv.split_first() else {
        anyhow::bail!("browser desktop entry has an empty Exec line");
    };
    let mut command = std::process::Command::new(program);
    command.args(args);
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
        .map_err(|e| anyhow::anyhow!("failed to open {program}: {e}"))?;
    child
        .wait()
        .map_err(|e| anyhow::anyhow!("failed to open {program}: {e}"))?;
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
    use std::ffi::c_void;
    use std::path::PathBuf;

    use core_foundation::array::{CFArray, CFArrayGetValueAtIndex};
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::error::{CFError, CFErrorRef};
    use core_foundation::number::{CFNumber, CFNumberRef};
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

    const CG_WINDOW_LIST_OPTION_ALL: u32 = 0;
    const CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
    const CG_NULL_WINDOW_ID: u32 = 0;
    const APP_WINDOW_MIN_SIDE: f64 = 64.;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect)
        -> bool;
    }

    #[repr(C)]
    #[derive(Default)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    pub(super) fn app_window_count(pid: u32) -> usize {
        let raw = unsafe {
            CGWindowListCopyWindowInfo(
                CG_WINDOW_LIST_OPTION_ALL | CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
                CG_NULL_WINDOW_ID,
            )
        };
        if raw.is_null() {
            return 0;
        }
        let list: CFArray = unsafe { TCFType::wrap_under_create_rule(raw as _) };
        (0..list.len())
            .filter(|&index| {
                let entry = unsafe { CFArrayGetValueAtIndex(list.as_concrete_TypeRef(), index) };
                if entry.is_null() {
                    return false;
                }
                let dict: CFDictionary = unsafe { TCFType::wrap_under_get_rule(entry as _) };
                is_app_window_of(&dict, pid)
            })
            .count()
    }

    fn is_app_window_of(dict: &CFDictionary, pid: u32) -> bool {
        number_value(dict, "kCGWindowOwnerPID") == Some(i64::from(pid))
            && number_value(dict, "kCGWindowLayer") == Some(0)
            && window_bounds(dict).is_some_and(|rect| {
                rect.width >= APP_WINDOW_MIN_SIDE && rect.height >= APP_WINDOW_MIN_SIDE
            })
    }

    fn number_value(dict: &CFDictionary, key: &str) -> Option<i64> {
        let key = CFString::new(key);
        let value = dict.find(key.as_concrete_TypeRef() as *const c_void)?;
        let number = unsafe { CFNumber::wrap_under_get_rule(*value as CFNumberRef) };
        number.to_i64()
    }

    fn window_bounds(dict: &CFDictionary) -> Option<CGRect> {
        let key = CFString::new("kCGWindowBounds");
        let value = dict.find(key.as_concrete_TypeRef() as *const c_void)?;
        let mut rect = CGRect::default();
        unsafe { CGRectMakeWithDictionaryRepresentation(*value as CFDictionaryRef, &mut rect) }
            .then_some(rect)
    }

    pub(super) fn request_quit(pid: u32) {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
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

/// The `Exec` argv of the default browser's desktop entry when that browser is
/// Chromium-based, field codes left in place for [`exec_with_app_url`].
///
/// Going through the desktop entry rather than `$PATH` is what makes Flatpak
/// (`com.google.Chrome.desktop` → `flatpak run … com.google.Chrome`) and Snap
/// (`chromium_chromium.desktop` → `/snap/bin/chromium`) installs launchable at
/// all: neither has a Chromium-named binary on `$PATH`.
#[cfg(all(unix, not(target_os = "macos")))]
fn default_chromium_browser() -> Option<Vec<String>> {
    let desktop_id = default_browser_desktop_id()?;
    let exec = match find_desktop_entry(&desktop_id) {
        Some(path) => {
            let contents = std::fs::read_to_string(&path).ok()?;
            let exec = desktop_entry_exec(&contents)?;
            split_exec(&exec)
        }
        // No entry on disk (unusual `XDG_DATA_DIRS`): fall back to the binary
        // the desktop id is named after, which is what a distro package ships.
        None => {
            let stem = chromium_desktop_stem(&desktop_id)?;
            vec![
                which_binary(stem)?.to_string_lossy().into_owned(),
                "%U".to_owned(),
            ]
        }
    };
    if is_chromium_exec(&exec) {
        Some(exec)
    } else {
        tracing::info!("default browser {desktop_id} is not Chromium-based: {exec:?}");
        None
    }
}

/// `xdg-settings` is what `xdg-open` consults, so prefer its answer; it is a
/// thin wrapper over `xdg-mime` on every modern desktop, which is the fallback
/// when the wrapper is missing or cannot recognise the desktop environment.
#[cfg(all(unix, not(target_os = "macos")))]
fn default_browser_desktop_id() -> Option<String> {
    const QUERIES: [&[&str]; 2] = [
        &["xdg-settings", "get", "default-web-browser"],
        &["xdg-mime", "query", "default", "x-scheme-handler/http"],
    ];
    QUERIES.iter().find_map(|argv| {
        let output = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8(output.stdout).ok()?;
        let id = stdout.split(';').next()?.trim();
        (!id.is_empty()).then(|| id.to_owned())
    })
}

/// `<data dir>/applications` for every XDG data dir, plus the Flatpak and Snap
/// export dirs that a login shell adds to `XDG_DATA_DIRS` but a `.desktop`
/// launch of this app may not see.
#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_entry_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => dirs.push(PathBuf::from(dir)),
        None => dirs.extend(home.as_ref().map(|home| home.join(".local/share"))),
    }
    match std::env::var_os("XDG_DATA_DIRS").filter(|dirs| !dirs.is_empty()) {
        Some(data_dirs) => dirs.extend(std::env::split_paths(&data_dirs)),
        None => dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from)),
    }
    dirs.extend(
        home.as_ref()
            .map(|home| home.join(".local/share/flatpak/exports/share")),
    );
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop"));
    dirs.into_iter()
        .map(|dir| dir.join("applications"))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn find_desktop_entry(desktop_id: &str) -> Option<PathBuf> {
    let desktop_id = desktop_id.trim().rsplit('/').next()?;
    desktop_entry_dirs()
        .into_iter()
        .map(|dir| dir.join(desktop_id))
        .find(|candidate| candidate.is_file())
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

/// The `Exec=` value of the `[Desktop Entry]` group, untouched.
#[allow(dead_code)]
fn desktop_entry_exec(contents: &str) -> Option<String> {
    let mut in_entry = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some(rest) = line.strip_prefix("Exec") else {
            continue;
        };
        if let Some(value) = rest.trim_start().strip_prefix('=') {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

/// Split an `Exec=` value into argv per the Desktop Entry spec: the string
/// escapes (`\s`, `\n`, `\t`, `\r`, `\\`) come off first, then arguments are
/// separated by unquoted whitespace, with backslash escapes honoured inside
/// double quotes. Field codes (`%U` …) stay as their own arguments.
#[allow(dead_code)]
fn split_exec(exec: &str) -> Vec<String> {
    let mut unescaped = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            unescaped.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => unescaped.push(' '),
            Some('n') => unescaped.push('\n'),
            Some('t') => unescaped.push('\t'),
            Some('r') => unescaped.push('\r'),
            Some('\\') => unescaped.push('\\'),
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }

    let mut argv = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quoted = false;
    let mut chars = unescaped.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_token = true;
            }
            '\\' if quoted => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if in_token {
                    argv.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                current.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        argv.push(current);
    }
    argv
}

#[allow(dead_code)]
const CHROMIUM_FLATPAK_IDS: &[&str] = &[
    "com.google.Chrome",
    "com.google.ChromeDev",
    "org.chromium.Chromium",
    "com.microsoft.Edge",
    "com.microsoft.EdgeDev",
    "com.brave.Browser",
    "com.vivaldi.Vivaldi",
];

/// Whether an `Exec` argv runs a Chromium-based browser, whatever wraps it:
/// a plain binary, `env VAR=… /snap/bin/chromium`, or
/// `flatpak run … --command=/app/bin/chrome … com.google.Chrome`.
#[allow(dead_code)]
fn is_chromium_exec(argv: &[String]) -> bool {
    argv.iter().any(|arg| {
        let arg = arg.strip_prefix("--command=").unwrap_or(arg);
        CHROMIUM_FLATPAK_IDS.contains(&arg)
            || executable_stem(Path::new(arg)).is_some_and(|stem| is_chromium_stem(&stem))
    })
}

/// Resolve the field codes of an `Exec` argv for an app-window launch: the
/// first `%f`/`%F`/`%u`/`%U` becomes `--app=<url>` (appended when there is
/// none), every other field code is dropped, `%%` is a literal percent.
#[allow(dead_code)]
fn exec_with_app_url(argv: &[String], url: &str) -> Vec<String> {
    let app_switch = format!("--app={url}");
    let mut placed = false;
    let mut out: Vec<String> = argv
        .iter()
        .filter_map(|arg| match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                (!std::mem::replace(&mut placed, true)).then(|| app_switch.clone())
            }
            "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => None,
            other => Some(other.replace("%%", "%")),
        })
        .collect();
    if !placed {
        out.push(app_switch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_window_watch_quits_once_a_seen_window_is_gone() {
        let start = std::time::Instant::now();
        let mut watch = ManagedWindowWatch::new(start);
        assert_eq!(watch.observe(0, start), WindowWatch::Keep);
        assert_eq!(
            watch.observe(0, start + std::time::Duration::from_secs(5)),
            WindowWatch::Keep
        );
        assert_eq!(
            watch.observe(1, start + std::time::Duration::from_secs(6)),
            WindowWatch::Keep
        );
        assert_eq!(
            watch.observe(0, start + std::time::Duration::from_secs(7)),
            WindowWatch::Quit
        );
    }

    #[test]
    fn managed_window_watch_gives_up_when_no_window_ever_appears() {
        let start = std::time::Instant::now();
        let mut watch = ManagedWindowWatch::new(start);
        assert_eq!(
            watch.observe(0, start + ManagedWindowWatch::STARTUP_GRACE / 2),
            WindowWatch::Keep
        );
        assert_eq!(
            watch.observe(0, start + ManagedWindowWatch::STARTUP_GRACE),
            WindowWatch::Quit
        );
    }

    #[test]
    fn managed_profile_dir_is_stable_per_key_and_filesystem_safe() {
        let first = managed_profile_dir("voice-app/1 x");
        let second = managed_profile_dir("voice-app/1 x");
        assert_eq!(first, second);
        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("voice-app_1_x")
        );
        assert_ne!(first, managed_profile_dir("voice-app-2"));
    }

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

    #[test]
    fn reads_exec_from_the_desktop_entry_group_only() {
        let contents = "[Desktop Action new-window]\nExec=/usr/bin/google-chrome-stable\n\
                        [Desktop Entry]\nName=Google Chrome\nTryExec=/usr/bin/google-chrome-stable\n\
                        Exec = /usr/bin/google-chrome-stable %U\n";
        assert_eq!(
            desktop_entry_exec(contents).as_deref(),
            Some("/usr/bin/google-chrome-stable %U")
        );
        assert_eq!(desktop_entry_exec("[Desktop Entry]\nName=x\n"), None);
        assert_eq!(desktop_entry_exec("[Desktop Entry]\nExec=\n"), None);
    }

    #[test]
    fn splits_exec_lines_like_the_spec_says() {
        let flatpak = "/usr/bin/flatpak run --branch=stable --arch=x86_64 \
                       --command=/app/bin/chrome --file-forwarding com.google.Chrome @@u %U @@";
        assert_eq!(
            split_exec(flatpak),
            [
                "/usr/bin/flatpak",
                "run",
                "--branch=stable",
                "--arch=x86_64",
                "--command=/app/bin/chrome",
                "--file-forwarding",
                "com.google.Chrome",
                "@@u",
                "%U",
                "@@"
            ]
        );
        assert_eq!(
            split_exec(r#""/opt/My Browser/browser" --flag="a b" %u"#),
            ["/opt/My Browser/browser", "--flag=a b", "%u"]
        );
        assert_eq!(
            split_exec(r#""/opt/one\stwo/app" "q\"uote\\\\back" %U"#),
            ["/opt/one two/app", r#"q"uote\back"#, "%U"]
        );
        assert_eq!(split_exec("   "), Vec::<String>::new());
    }

    #[test]
    fn recognises_chromium_exec_lines_behind_wrappers() {
        let argv = |line: &str| split_exec(line);
        assert!(is_chromium_exec(&argv("/usr/bin/google-chrome-stable %U")));
        assert!(is_chromium_exec(&argv(
            "env BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/chromium_chromium.desktop /snap/bin/chromium %U"
        )));
        assert!(is_chromium_exec(&argv(
            "/usr/bin/flatpak run --command=/app/bin/chrome --file-forwarding com.google.Chrome @@u %U @@"
        )));
        assert!(is_chromium_exec(&argv(
            "/usr/bin/flatpak run --command=brave --file-forwarding com.brave.Browser @@u %U @@"
        )));
        assert!(!is_chromium_exec(&argv("firefox %u")));
        assert!(!is_chromium_exec(&argv(
            "/usr/bin/flatpak run --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@"
        )));
        assert!(!is_chromium_exec(&argv("")));
    }

    #[test]
    fn substitutes_the_url_field_code_with_the_app_switch() {
        let url = "https://app.example.com/x?y=1";
        let argv = |line: &str| split_exec(line);
        assert_eq!(
            exec_with_app_url(&argv("/usr/bin/google-chrome-stable %U"), url),
            [
                "/usr/bin/google-chrome-stable",
                "--app=https://app.example.com/x?y=1"
            ]
        );
        assert_eq!(
            exec_with_app_url(&argv("chromium --icon=x %i %c %k %u %F"), url),
            [
                "chromium",
                "--icon=x",
                "--app=https://app.example.com/x?y=1"
            ]
        );
        assert_eq!(
            exec_with_app_url(&argv("chromium --profile=100%%"), url),
            [
                "chromium",
                "--profile=100%",
                "--app=https://app.example.com/x?y=1"
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "environment dependent: reports the default browser of the machine it runs on"]
    fn reports_the_default_browser() {
        println!("default http handler: {:?}", macos::default_http_handler());
        println!("chromium app mode target: {:?}", default_chromium_browser());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[ignore = "environment dependent: reports the default browser of the machine it runs on"]
    fn reports_the_default_browser() {
        println!(
            "default browser desktop id: {:?}",
            default_browser_desktop_id()
        );
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
