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
    "microsoft-edge-beta",
    "microsoft-edge-dev",
    "msedge",
    "brave",
    "brave-browser",
    "brave-browser-stable",
    "brave-browser-beta",
    "brave-browser-nightly",
    "vivaldi",
    "vivaldi-stable",
    "vivaldi-snapshot",
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
    let launched = match chromium_browser() {
        Some(browser) => launch_app_window(&browser, url),
        None => match launch_firefox_app_window(url) {
            Some(launched) => launched,
            None => {
                tracing::info!("no browser with an app mode, opening a browser tab instead");
                return crate::open_url(url);
            }
        },
    };
    launched.or_else(|error| {
        tracing::warn!("app-window launch failed, falling back to a browser tab: {error:#}");
        crate::open_url(url)
    })
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
    spawn_detached(&exec_with_launch_args(exec, &chromium_app_window_args(url)))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_firefox_app_window(url: &str) -> Option<anyhow::Result<()>> {
    let firefox = firefox_browser()?;
    let profile = firefox_app_profile_dir(
        &firefox,
        &dirs::home_dir()?,
        &dirs::data_local_dir()?,
        SNAP_FIREFOX_PATHS
            .iter()
            .any(|path| Path::new(path).exists()),
    );
    let profile_arg = profile.to_str()?.to_owned();
    tracing::info!("no Chromium-based browser, opening the app window in Firefox {firefox:?}");
    Some(prepare_firefox_app_profile(&profile).and_then(|()| {
        spawn_detached(&exec_with_launch_args(
            &firefox,
            &firefox_app_window_args(&profile_arg, url),
        ))
    }))
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn launch_firefox_app_window(_url: &str) -> Option<anyhow::Result<()>> {
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_detached(argv: &[String]) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

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

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn chromium_browser() -> Option<PathBuf> {
    default_chromium_browser()
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

#[cfg(all(unix, not(target_os = "macos")))]
fn chromium_browser() -> Option<Vec<String>> {
    default_chromium_browser().or_else(installed_chromium_browser)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn installed_chromium_browser() -> Option<Vec<String>> {
    let exec = installed_browser_in(
        &desktop_entry_dirs(),
        CHROMIUM_DESKTOP_IDS,
        is_chromium_exec,
    )
    .or_else(|| browser_on_path(CHROMIUM_STEMS))?;
    tracing::info!(
        "default browser is not Chromium-based, opening the app window in installed {exec:?}"
    );
    Some(exec)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn firefox_browser() -> Option<Vec<String>> {
    installed_browser_in(&desktop_entry_dirs(), FIREFOX_DESKTOP_IDS, is_firefox_exec)
        .or_else(|| browser_on_path(FIREFOX_STEMS))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_on_path(stems: &[&str]) -> Option<Vec<String>> {
    let path = stems.iter().find_map(|stem| which_binary(stem))?;
    Some(vec![path.to_string_lossy().into_owned(), "%U".to_owned()])
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
        Some(path) => desktop_entry_argv(&path)?,
        // No entry on disk (unusual `XDG_DATA_DIRS`): fall back to the binary
        // the desktop id is named after, which is what a distro package ships.
        None => browser_on_path(&[chromium_desktop_stem(&desktop_id)?])?,
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
    find_desktop_entry_in(&desktop_entry_dirs(), desktop_id)
}

#[allow(dead_code)]
fn find_desktop_entry_in(dirs: &[PathBuf], desktop_id: &str) -> Option<PathBuf> {
    let desktop_id = desktop_id.trim().rsplit('/').next()?;
    dirs.iter()
        .map(|dir| dir.join(desktop_id))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
#[allow(dead_code)]
fn desktop_entry_argv(path: &Path) -> Option<Vec<String>> {
    let contents = std::fs::read_to_string(path).ok()?;
    if desktop_entry_value(&contents, "Hidden").is_some_and(|hidden| hidden == "true") {
        return None;
    }
    if desktop_entry_value(&contents, "TryExec").is_some_and(|program| !is_launchable(&program)) {
        return None;
    }
    let argv = split_exec(&desktop_entry_exec(&contents)?);
    is_launchable(argv.first()?).then_some(argv)
}

#[cfg(unix)]
#[allow(dead_code)]
fn is_launchable(program: &str) -> bool {
    if program.contains('/') {
        is_installed_program(Path::new(program))
    } else {
        which_binary(program).is_some()
    }
}

#[cfg(unix)]
#[allow(dead_code)]
fn is_installed_program(path: &Path) -> bool {
    is_executable_file(path) && !is_orphaned_snap_wrapper(path)
}

#[cfg(unix)]
#[allow(dead_code)]
fn is_orphaned_snap_wrapper(path: &Path) -> bool {
    use std::io::Read;

    let mut head = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(SNAP_WRAPPER_SCAN_BYTES).read_to_end(&mut head));
    if read.is_err() || !head.starts_with(b"#!") {
        return false;
    }
    String::from_utf8_lossy(&head)
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';'))
        .filter(|token| token.starts_with("/snap/bin/"))
        .any(|target| !Path::new(target).exists())
}

#[allow(dead_code)]
const SNAP_WRAPPER_SCAN_BYTES: u64 = 4096;

#[cfg(unix)]
#[allow(dead_code)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(unix)]
#[allow(dead_code)]
fn installed_browser_in(
    dirs: &[PathBuf],
    desktop_ids: &[&str],
    runs_browser: fn(&[String]) -> bool,
) -> Option<Vec<String>> {
    desktop_ids
        .iter()
        .filter_map(|desktop_id| find_desktop_entry_in(dirs, desktop_id))
        .filter_map(|path| desktop_entry_argv(&path))
        .find(|argv| runs_browser(argv))
}

#[cfg(unix)]
#[allow(dead_code)]
fn which_binary(stem: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(stem))
        .find(|candidate| is_installed_program(candidate))
}

/// The `Exec=` value of the `[Desktop Entry]` group, untouched.
#[allow(dead_code)]
fn desktop_entry_exec(contents: &str) -> Option<String> {
    desktop_entry_value(contents, "Exec")
}

#[allow(dead_code)]
fn desktop_entry_value(contents: &str, key: &str) -> Option<String> {
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
        let Some(rest) = line.strip_prefix(key) else {
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
    exec_runs(argv, CHROMIUM_FLATPAK_IDS, CHROMIUM_STEMS)
}

#[allow(dead_code)]
fn exec_runs(argv: &[String], flatpak_ids: &[&str], stems: &[&str]) -> bool {
    argv.iter().any(|arg| {
        let arg = arg.strip_prefix("--command=").unwrap_or(arg);
        flatpak_ids.contains(&arg)
            || executable_stem(Path::new(arg)).is_some_and(|stem| stems.contains(&stem.as_str()))
    })
}

#[allow(dead_code)]
fn chromium_app_window_args(url: &str) -> [String; 3] {
    [
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        format!("--app={url}"),
    ]
}

/// Resolve the field codes of an `Exec` argv for an app-window launch: the
/// first `%f`/`%F`/`%u`/`%U` becomes `--app=<url>` (appended when there is
/// none), every other field code is dropped, `%%` is a literal percent.
#[allow(dead_code)]
fn exec_with_app_url(argv: &[String], url: &str) -> Vec<String> {
    exec_with_launch_args(argv, &[format!("--app={url}")])
}

#[allow(dead_code)]
fn exec_with_launch_args(argv: &[String], launch_args: &[String]) -> Vec<String> {
    let mut placed = false;
    let mut out = Vec::with_capacity(argv.len() + launch_args.len());
    for arg in argv {
        match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                if !std::mem::replace(&mut placed, true) {
                    out.extend_from_slice(launch_args);
                }
            }
            "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            other => out.push(other.replace("%%", "%")),
        }
    }
    if !placed {
        out.extend_from_slice(launch_args);
    }
    out
}

#[allow(dead_code)]
const CHROMIUM_DESKTOP_IDS: &[&str] = &[
    "google-chrome.desktop",
    "com.google.Chrome.desktop",
    "google-chrome-beta.desktop",
    "google-chrome-unstable.desktop",
    "com.google.ChromeDev.desktop",
    "chromium.desktop",
    "chromium-browser.desktop",
    "chromium_chromium.desktop",
    "org.chromium.Chromium.desktop",
    "microsoft-edge.desktop",
    "com.microsoft.Edge.desktop",
    "microsoft-edge-beta.desktop",
    "microsoft-edge-dev.desktop",
    "com.microsoft.EdgeDev.desktop",
    "brave-browser.desktop",
    "com.brave.Browser.desktop",
    "brave_brave.desktop",
    "brave-browser-beta.desktop",
    "brave-browser-nightly.desktop",
    "vivaldi-stable.desktop",
    "com.vivaldi.Vivaldi.desktop",
    "vivaldi-snapshot.desktop",
];

#[allow(dead_code)]
const FIREFOX_DESKTOP_IDS: &[&str] = &[
    "firefox_firefox.desktop",
    "org.mozilla.firefox.desktop",
    "firefox.desktop",
    "firefox-esr.desktop",
];

#[allow(dead_code)]
const FIREFOX_STEMS: &[&str] = &["firefox", "firefox-esr"];

#[allow(dead_code)]
const FIREFOX_FLATPAK_ID: &str = "org.mozilla.firefox";

#[allow(dead_code)]
const SNAP_FIREFOX_PATHS: &[&str] = &["/snap/bin/firefox", "/var/lib/snapd/snap/bin/firefox"];

#[allow(dead_code)]
const FIREFOX_APP_PROFILE: &str = "mezon-app-window";

#[allow(dead_code)]
const FIREFOX_APP_WINDOW_PREFS: &str = r#"user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);
user_pref("browser.tabs.inTitlebar", 0);
user_pref("browser.tabs.drawInTitlebar", false);
user_pref("browser.link.open_newwindow", 2);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("browser.startup.page", 0);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.startup.couldRestoreSession.count", -1);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
"#;

#[allow(dead_code)]
const FIREFOX_APP_WINDOW_CHROME_CSS: &str = "#TabsToolbar, #nav-bar, #PersonalToolbar, #sidebar-main, #sidebar-box { visibility: collapse !important; }\n";

#[allow(dead_code)]
fn is_firefox_exec(argv: &[String]) -> bool {
    exec_runs(argv, &[FIREFOX_FLATPAK_ID], FIREFOX_STEMS)
}

#[allow(dead_code)]
fn firefox_app_profile_dir(
    firefox: &[String],
    home: &Path,
    data_dir: &Path,
    snap_firefox_installed: bool,
) -> PathBuf {
    if firefox.iter().any(|arg| arg == FIREFOX_FLATPAK_ID) {
        home.join(".var/app")
            .join(FIREFOX_FLATPAK_ID)
            .join("data")
            .join(FIREFOX_APP_PROFILE)
    } else if snap_firefox_installed || firefox.iter().any(|arg| arg.contains("/snap/bin/")) {
        home.join("snap/firefox/common").join(FIREFOX_APP_PROFILE)
    } else {
        data_dir.join("mezon").join(FIREFOX_APP_PROFILE)
    }
}

#[cfg(unix)]
#[allow(dead_code)]
fn prepare_firefox_app_profile(profile: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let chrome = profile.join("chrome");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&chrome)?;
    std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))?;
    write_if_changed(&profile.join("user.js"), FIREFOX_APP_WINDOW_PREFS)?;
    write_if_changed(
        &chrome.join("userChrome.css"),
        FIREFOX_APP_WINDOW_CHROME_CSS,
    )?;
    Ok(())
}

#[allow(dead_code)]
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    let staged = path.with_extension("mezon-staged");
    std::fs::write(&staged, contents)?;
    std::fs::rename(&staged, path)
}

#[allow(dead_code)]
fn firefox_app_window_args(profile: &str, url: &str) -> [String; 4] {
    [
        "--profile".to_owned(),
        profile.to_owned(),
        "--new-window".to_owned(),
        url.to_owned(),
    ]
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
        assert!(is_chromium_stem("brave-browser-stable"));
        assert!(is_chromium_stem("microsoft-edge-beta"));
        assert!(is_chromium_stem("vivaldi-snapshot"));
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
        assert!(is_chromium_exec(&argv("/usr/bin/brave-browser-stable %U")));
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

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str) -> String {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    fn write_desktop_entry(dir: &Path, desktop_id: &str, keys: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(desktop_id),
            format!("[Desktop Entry]\nName={desktop_id}\n{keys}\n"),
        )
        .unwrap();
    }

    #[test]
    fn skips_first_run_prompts_in_chromium_app_windows() {
        assert_eq!(
            chromium_app_window_args("https://app.example.com"),
            [
                "--no-first-run",
                "--no-default-browser-check",
                "--app=https://app.example.com"
            ]
        );
    }

    #[test]
    fn places_every_launch_arg_at_the_url_field_code() {
        let launch_args = [
            "--profile".to_owned(),
            "/home/u/p".to_owned(),
            "--new-window".to_owned(),
            "https://app.example.com".to_owned(),
        ];
        let argv = |line: &str| split_exec(line);
        assert_eq!(
            exec_with_launch_args(&argv("firefox %u"), &launch_args),
            [
                "firefox",
                "--profile",
                "/home/u/p",
                "--new-window",
                "https://app.example.com"
            ]
        );
        assert_eq!(
            exec_with_launch_args(
                &argv(
                    "/usr/bin/flatpak run --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@"
                ),
                &launch_args
            ),
            [
                "/usr/bin/flatpak",
                "run",
                "--command=firefox",
                "--file-forwarding",
                "org.mozilla.firefox",
                "@@u",
                "--profile",
                "/home/u/p",
                "--new-window",
                "https://app.example.com",
                "@@"
            ]
        );
        assert_eq!(
            exec_with_launch_args(&argv("/usr/lib/firefox/firefox"), &launch_args),
            [
                "/usr/lib/firefox/firefox",
                "--profile",
                "/home/u/p",
                "--new-window",
                "https://app.example.com"
            ]
        );
    }

    #[test]
    fn recognises_firefox_exec_lines_behind_wrappers() {
        let argv = |line: &str| split_exec(line);
        assert!(is_firefox_exec(&argv("firefox %u")));
        assert!(is_firefox_exec(&argv(
            "/usr/lib/firefox-esr/firefox-esr %u"
        )));
        assert!(is_firefox_exec(&argv(
            "env BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop /snap/bin/firefox %u"
        )));
        assert!(is_firefox_exec(&argv(
            "/usr/bin/flatpak run --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@"
        )));
        assert!(!is_firefox_exec(&argv("/usr/bin/google-chrome-stable %U")));
        assert!(!is_firefox_exec(&argv("librewolf %u")));
        assert!(!is_firefox_exec(&argv("")));
    }

    #[test]
    fn keeps_the_firefox_app_profile_where_its_sandbox_can_reach() {
        let home = Path::new("/home/u");
        let data = Path::new("/home/u/.local/share");
        let argv = |line: &str| split_exec(line);
        assert_eq!(
            firefox_app_profile_dir(
                &argv("/usr/bin/flatpak run --command=firefox org.mozilla.firefox @@u %u @@"),
                home,
                data,
                true
            ),
            Path::new("/home/u/.var/app/org.mozilla.firefox/data/mezon-app-window")
        );
        assert_eq!(
            firefox_app_profile_dir(
                &argv("env BAMF_DESKTOP_FILE_HINT=x /snap/bin/firefox %u"),
                home,
                data,
                false
            ),
            Path::new("/home/u/snap/firefox/common/mezon-app-window")
        );
        assert_eq!(
            firefox_app_profile_dir(
                &argv("env BAMF_DESKTOP_FILE_HINT=x /var/lib/snapd/snap/bin/firefox %u"),
                home,
                data,
                false
            ),
            Path::new("/home/u/snap/firefox/common/mezon-app-window")
        );
        assert_eq!(
            firefox_app_profile_dir(&argv("firefox %u"), home, data, true),
            Path::new("/home/u/snap/firefox/common/mezon-app-window")
        );
        assert_eq!(
            firefox_app_profile_dir(&argv("firefox %u"), home, data, false),
            Path::new("/home/u/.local/share/mezon/mezon-app-window")
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_a_firefox_profile_without_toolbars() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("nested");
        prepare_firefox_app_profile(&profile).unwrap();
        std::fs::write(profile.join("user.js"), "stale").unwrap();
        prepare_firefox_app_profile(&profile).unwrap();
        prepare_firefox_app_profile(&profile).unwrap();
        assert!(!profile.join("user.mezon-staged").exists());
        let prefs = std::fs::read_to_string(profile.join("user.js")).unwrap();
        assert!(prefs.contains(
            r#"user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);"#
        ));
        assert!(prefs.contains(r#"user_pref("browser.tabs.inTitlebar", 0);"#));
        let chrome_css =
            std::fs::read_to_string(profile.join("chrome").join("userChrome.css")).unwrap();
        assert!(chrome_css.contains("#nav-bar"));
        assert!(chrome_css.contains("#TabsToolbar"));
        assert_eq!(
            firefox_app_window_args(profile.to_str().unwrap(), "https://app.example.com")[..3],
            [
                "--profile".to_owned(),
                profile.to_string_lossy().into_owned(),
                "--new-window".to_owned()
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_firefox_app_profile_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_firefox_app_profile(&profile).unwrap();
        assert_eq!(mode(&profile), 0o700);

        let fresh = root.path().join("fresh").join("a").join("profile");
        prepare_firefox_app_profile(&fresh).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        assert_eq!(mode(fresh.parent().unwrap()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_preferred_installed_browser_by_desktop_entry() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let user_apps = root.path().join("user");
        let system_apps = root.path().join("system");
        let brave = executable(&bin, "brave-browser-stable");
        let chrome = executable(&bin, "google-chrome-stable");
        let firefox = executable(&bin, "firefox");
        write_desktop_entry(
            &system_apps,
            "brave-browser.desktop",
            &format!("Exec={brave} %U"),
        );
        write_desktop_entry(
            &system_apps,
            "google-chrome.desktop",
            &format!("Exec={chrome} %U"),
        );
        write_desktop_entry(
            &user_apps,
            "chromium.desktop",
            &format!("Exec={firefox} %u"),
        );
        write_desktop_entry(
            &system_apps,
            "firefox.desktop",
            &format!("Exec={firefox} %u"),
        );
        let dirs = [user_apps, system_apps];

        assert_eq!(
            installed_browser_in(&dirs, CHROMIUM_DESKTOP_IDS, is_chromium_exec),
            Some(vec![chrome, "%U".to_owned()])
        );
        assert_eq!(
            installed_browser_in(&dirs, FIREFOX_DESKTOP_IDS, is_firefox_exec),
            Some(vec![firefox, "%u".to_owned()])
        );
        assert_eq!(
            installed_browser_in(&dirs[..1], CHROMIUM_DESKTOP_IDS, is_chromium_exec),
            None
        );
        assert_eq!(
            installed_browser_in(&[], FIREFOX_DESKTOP_IDS, is_firefox_exec),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_hidden_and_uninstalled_desktop_entries() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let apps = root.path().join("apps");
        let brave = executable(&bin, "brave-browser-stable");
        let gone = |name: &str| bin.join(name).to_string_lossy().into_owned();
        write_desktop_entry(
            &apps,
            "google-chrome.desktop",
            &format!("Exec={} %U", gone("google-chrome-stable")),
        );
        write_desktop_entry(
            &apps,
            "com.google.Chrome.desktop",
            &format!("Exec={brave} %U\nHidden=true"),
        );
        write_desktop_entry(
            &apps,
            "chromium.desktop",
            &format!("TryExec={}\nExec={brave} %U", gone("chromium")),
        );
        write_desktop_entry(&apps, "brave-browser.desktop", &format!("Exec={brave} %U"));

        assert_eq!(
            installed_browser_in(&[apps], CHROMIUM_DESKTOP_IDS, is_chromium_exec),
            Some(vec![brave, "%U".to_owned()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn skips_ubuntu_snap_wrappers_whose_snap_is_gone() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let apps = root.path().join("apps");
        let script = |name: &str, body: &str| {
            let path = root.path().join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        };
        let orphaned = script(
            "chromium-browser",
            "#!/bin/sh\nif ! [ -x /snap/bin/mezon-test-gone ]; then\n  echo \"requires the snap\" >&2\n  exit 1\nfi\nexec /snap/bin/mezon-test-gone \"$@\"\n",
        );
        let plain = script(
            "google-chrome-stable",
            "#!/bin/sh\nexec /opt/google/chrome/chrome \"$@\"\n",
        );

        assert!(is_orphaned_snap_wrapper(Path::new(&orphaned)));
        assert!(!is_launchable(&orphaned));
        assert!(!is_orphaned_snap_wrapper(Path::new(&plain)));
        assert!(is_launchable(&plain));

        write_desktop_entry(
            &apps,
            "chromium-browser.desktop",
            &format!("Exec={orphaned} %U"),
        );
        assert_eq!(
            installed_browser_in(
                std::slice::from_ref(&apps),
                CHROMIUM_DESKTOP_IDS,
                is_chromium_exec
            ),
            None
        );
        write_desktop_entry(&apps, "google-chrome.desktop", &format!("Exec={plain} %U"));
        assert_eq!(
            installed_browser_in(&[apps], CHROMIUM_DESKTOP_IDS, is_chromium_exec),
            Some(vec![plain, "%U".to_owned()])
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
        println!("chromium app mode target: {:?}", chromium_browser());
        println!("firefox app mode target: {:?}", firefox_browser());
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
