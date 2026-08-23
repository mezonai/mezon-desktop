#[cfg(unix)]
use std::path::PathBuf;

/// Outcome of trying to take ownership of one socket path.
#[cfg(unix)]
enum Claim {
    /// We bound the socket and now hold the lock.
    Ours(std::os::unix::net::UnixListener),
    /// Another live instance owns it; the stream reaches that instance.
    Taken(std::os::unix::net::UnixStream),
    /// Unusable path — try the next candidate.
    Failed(std::io::Error),
}

/// Ensures only one instance of the app runs at a time.
///
/// Unix (macOS / Linux): Unix domain socket at `$XDG_RUNTIME_DIR/mezon.sock`.
/// Windows            : Named pipe `\\.\pipe\mezon-single-instance`.
///
/// A second instance can forward a URL to the first by writing it to the
/// socket / pipe before exiting (used for deep link handling).
pub struct SingleInstance {
    #[cfg(unix)]
    socket_path: Option<PathBuf>,
    #[cfg(unix)]
    _listener: Option<std::os::unix::net::UnixListener>,

    #[cfg(windows)]
    _pipe_name: String,
    #[cfg(windows)]
    url_rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<String>>>,
}

pub const ACTIVATE_MESSAGE: &str = "mezon-activate";

impl SingleInstance {
    /// Try to acquire the single-instance lock.
    ///
    /// Returns `Ok(Some(instance))` — this is the first instance.
    /// Returns `Ok(None)` — another instance is already running.
    pub fn try_acquire() -> anyhow::Result<Option<Self>> {
        #[cfg(unix)]
        return Self::try_acquire_unix(Some(ACTIVATE_MESSAGE));

        #[cfg(windows)]
        return Self::try_acquire_windows(Some(ACTIVATE_MESSAGE));

        #[cfg(not(any(unix, windows)))]
        Ok(Some(Self {}))
    }

    /// Same as `try_acquire`, but additionally forwards `url` to the running
    /// first instance if one exists.
    pub fn try_acquire_or_forward(url: &str) -> anyhow::Result<Option<Self>> {
        #[cfg(unix)]
        return Self::try_acquire_unix(Some(url));

        #[cfg(windows)]
        return Self::try_acquire_windows(Some(url));

        #[cfg(not(any(unix, windows)))]
        {
            let _ = url;
            Ok(Some(Self {}))
        }
    }

    // ── Unix ──────────────────────────────────────────────────────────────────

    #[cfg(unix)]
    fn try_acquire_unix(forward_url: Option<&str>) -> anyhow::Result<Option<Self>> {
        use std::io::Write as _;

        let mut last_bind_error = None;
        for socket_path in Self::socket_paths() {
            if let Some(parent) = socket_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                last_bind_error = Some(e);
                continue;
            }

            match Self::claim(&socket_path) {
                Claim::Ours(listener) => {
                    tracing::debug!("Single instance lock acquired at {}", socket_path.display());
                    return Ok(Some(Self {
                        socket_path: Some(socket_path),
                        _listener: Some(listener),
                    }));
                }
                Claim::Taken(mut stream) => {
                    tracing::info!("Another instance is already running");
                    if let Some(url) = forward_url {
                        let _ = stream.write_all(url.as_bytes());
                        let safe = url.split(['?', '#']).next().unwrap_or_default();
                        tracing::debug!("Forwarded URL to running instance: {safe}");
                    }
                    return Ok(None);
                }
                Claim::Failed(e) => {
                    last_bind_error = Some(e);
                }
            }
        }

        let error = last_bind_error
            .unwrap_or_else(|| std::io::Error::other("no Unix socket path candidates available"));

        // Losing the lock must never stop the app from starting: a second copy
        // running is a far smaller problem than a window that never appears.
        tracing::warn!("Single instance lock disabled because no socket could be bound: {error}");
        Ok(Some(Self {
            socket_path: None,
            _listener: None,
        }))
    }

    /// Try to become the owner of `socket_path`.
    #[cfg(unix)]
    fn claim(socket_path: &std::path::Path) -> Claim {
        // A peer that answers owns the lock; a refused connection means the
        // file outlived the process that made it and can be cleared.
        if socket_path.exists() {
            match std::os::unix::net::UnixStream::connect(socket_path) {
                Ok(stream) => return Claim::Taken(stream),
                Err(_) => {
                    let _ = std::fs::remove_file(socket_path);
                }
            }
        }

        Self::bind_or_detect(socket_path)
    }

    /// `bind` half of [`Self::claim`], split out so the lost-race branch can be
    /// tested directly.
    #[cfg(unix)]
    fn bind_or_detect(socket_path: &std::path::Path) -> Claim {
        match std::os::unix::net::UnixListener::bind(socket_path) {
            Ok(listener) => Claim::Ours(listener),
            // Somebody claimed the path between the probe above and this bind.
            // Ask who: a live peer means we are the second instance after all,
            // and must not wander off to bind a different candidate.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                match std::os::unix::net::UnixStream::connect(socket_path) {
                    Ok(stream) => Claim::Taken(stream),
                    Err(_) => Claim::Failed(e),
                }
            }
            Err(e) => Claim::Failed(e),
        }
    }

    /// `sockaddr_un::sun_path` holds 104 bytes including the NUL terminator, so
    /// a bindable socket path is at most 103 bytes. Longer paths fail `bind`
    /// with `InvalidInput` rather than an OS errno, which is easy to mistake
    /// for a bug in the caller.
    #[cfg(unix)]
    pub(crate) const MAX_SOCKET_PATH: usize = 103;

    /// Stable across processes and runs: `DefaultHasher::new` is seeded with a
    /// fixed key, unlike `RandomState`.
    #[cfg(unix)]
    pub(crate) fn user_digest(user: &str) -> u64 {
        use std::hash::{Hash as _, Hasher as _};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        user.hash(&mut hasher);
        hasher.finish()
    }

    #[cfg(unix)]
    fn socket_paths() -> Vec<PathBuf> {
        let user = std::env::var("USER")
            .ok()
            .filter(|user| !user.is_empty())
            .unwrap_or_else(|| "user".to_owned());

        Self::socket_paths_for(dirs::runtime_dir().as_deref(), &std::env::temp_dir(), &user)
    }

    #[cfg(unix)]
    fn socket_paths_for(
        runtime_dir: Option<&std::path::Path>,
        tmp: &std::path::Path,
        user: &str,
    ) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Some(runtime_dir) = runtime_dir {
            paths.push(runtime_dir.join("mezon.sock"));
        }

        paths.push(tmp.join(format!("mezon-desktop-{user}")).join("mezon.sock"));

        // A sandboxed (App Store) build gets a container `$TMPDIR` under
        // `~/Library/Containers/<bundle id>/Data/tmp/`, which already eats ~66
        // bytes before the user name. The readable path above then overruns
        // `sun_path` for anything but a short login name, so keep a
        // fixed-width per-user fallback that always fits.
        paths.push(tmp.join(format!("mezon-{:08x}.sock", Self::user_digest(user) as u32)));

        paths.retain(|path| path.as_os_str().len() <= Self::MAX_SOCKET_PATH);
        paths
    }

    // ── Windows ───────────────────────────────────────────────────────────────

    #[cfg(windows)]
    const PIPE_NAME: &'static str = r"\\.\pipe\mezon-single-instance";

    #[cfg(windows)]
    fn try_acquire_windows(forward_url: Option<&str>) -> anyhow::Result<Option<Self>> {
        use std::io::Write as _;

        match std::fs::OpenOptions::new()
            .write(true)
            .open(Self::PIPE_NAME)
        {
            Ok(mut pipe) => {
                tracing::info!("Another instance is already running (Windows named pipe)");
                if let Some(url) = forward_url {
                    let _ = pipe.write_all(url.as_bytes());
                    let safe = url.split(['?', '#']).next().unwrap_or_default();
                    tracing::debug!("Forwarded URL to running instance via named pipe: {safe}");
                }
                return Ok(None);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No server yet — we become the server.
            }
            Err(e) => {
                tracing::warn!("Named pipe connect error (treating as no server): {e}");
            }
        }

        let url_rx = Self::create_pipe_server()?;

        tracing::debug!("Single instance lock acquired (Windows named pipe)");
        Ok(Some(Self {
            _pipe_name: Self::PIPE_NAME.to_owned(),
            url_rx: std::sync::Mutex::new(Some(url_rx)),
        }))
    }

    #[cfg(windows)]
    fn create_pipe_server() -> anyhow::Result<std::sync::mpsc::Receiver<String>> {
        use std::time::Duration;
        use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND,
        };
        use windows::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            PIPE_WAIT,
        };

        const MIN_BACKOFF: Duration = Duration::from_millis(100);
        const MAX_BACKOFF: Duration = Duration::from_secs(5);

        let pipe_name: Vec<u16> = Self::PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            CreateNamedPipeW(
                windows::core::PCWSTR(pipe_name.as_ptr()),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                None,
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(anyhow::anyhow!(
                "CreateNamedPipeW failed (another instance may already hold the lock): {}",
                std::io::Error::last_os_error()
            ));
        }

        // Extract raw pointer value; HANDLE is Copy so just store the value
        let raw = handle.0 as usize;

        let (tx, rx) = std::sync::mpsc::channel::<String>();

        // Transfer ownership into the listener thread.
        // The thread keeps the handle alive and continuously accepts + reads connections.
        std::thread::Builder::new()
            .name("mezon-single-instance".into())
            .spawn(move || {
                let h = windows::Win32::Foundation::HANDLE(raw as *mut std::ffi::c_void);
                let mut backoff = MIN_BACKOFF;
                loop {
                    let connected =
                        match unsafe { windows::Win32::System::Pipes::ConnectNamedPipe(h, None) } {
                            Ok(()) => true,
                            Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
                        };
                    if !connected {
                        unsafe {
                            let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(h);
                        }
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                    backoff = MIN_BACKOFF;

                    let mut buf = [0u8; 4096];
                    let mut bytes_read = 0u32;
                    let ok = unsafe {
                        windows::Win32::Storage::FileSystem::ReadFile(
                            h,
                            Some(&mut buf),
                            Some(&mut bytes_read),
                            None,
                        )
                        .is_ok()
                    };
                    if ok && bytes_read > 0 {
                        if let Ok(url) = std::str::from_utf8(&buf[..bytes_read as usize]) {
                            let url = url.trim().to_owned();
                            let safe = url.split(['?', '#']).next().unwrap_or_default();
                            tracing::debug!(
                                "Named pipe: received URL from secondary instance: {safe}"
                            );
                            let _ = tx.send(url);
                        }
                    }
                    unsafe {
                        let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(h);
                    }
                }
            })
            .map_err(|e| {
                anyhow::anyhow!("Failed to spawn single-instance pipe listener thread: {e}")
            })?;

        Ok(rx)
    }

    // ── Shared: URL forwarding listener ───────────────────────────────────────

    /// Spawn a thread that accepts connections and calls `callback` with any
    /// URL strings sent by secondary instances.
    pub fn listen_for_urls(&self, callback: impl Fn(String) + Send + 'static) {
        #[cfg(unix)]
        self.listen_for_urls_unix(callback);

        #[cfg(windows)]
        self.listen_for_urls_windows(callback);

        #[cfg(not(any(unix, windows)))]
        {
            let _ = callback;
        }
    }

    #[cfg(unix)]
    fn listen_for_urls_unix(&self, callback: impl Fn(String) + Send + 'static) {
        let Some(listener) = &self._listener else {
            tracing::debug!(
                "Unix URL forwarding disabled because single-instance lock is disabled"
            );
            let _ = callback;
            return;
        };

        let listener = match listener.try_clone() {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Could not clone UnixListener for URL forwarding: {e}");
                return;
            }
        };

        std::thread::spawn(move || {
            use std::io::{BufReader, Read as _};
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        let mut buf = String::new();
                        let ok = BufReader::new(s)
                            .take(4096)
                            .read_to_string(&mut buf)
                            .is_ok();
                        if ok && !buf.is_empty() {
                            let url = buf.trim().to_owned();
                            let safe = url.split(['?', '#']).next().unwrap_or_default();
                            tracing::debug!(
                                "Unix socket: received deep link from secondary instance: {safe}"
                            );
                            callback(url);
                        }
                    }
                    Err(e) => {
                        if Self::is_transient_accept_error(&e) {
                            tracing::debug!("Transient Unix socket accept error, retrying: {e}");
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            continue;
                        }
                        tracing::warn!("Unix socket accept error (stopping URL forwarding): {e}");
                        break;
                    }
                }
            }
        });
    }

    #[cfg(unix)]
    fn is_transient_accept_error(e: &std::io::Error) -> bool {
        use std::io::ErrorKind;
        if matches!(
            e.kind(),
            ErrorKind::ConnectionAborted | ErrorKind::Interrupted | ErrorKind::WouldBlock
        ) {
            return true;
        }
        matches!(e.raw_os_error(), Some(23) | Some(24) | Some(55) | Some(105))
    }

    #[cfg(windows)]
    fn listen_for_urls_windows(&self, callback: impl Fn(String) + Send + 'static) {
        let Some(rx) = self.url_rx.lock().ok().and_then(|mut guard| guard.take()) else {
            tracing::debug!("Windows URL forwarding already wired or unavailable");
            let _ = callback;
            return;
        };

        if let Err(e) = std::thread::Builder::new()
            .name("mezon-pipe-url-listener".into())
            .spawn(move || {
                for url in rx {
                    let safe = url.split(['?', '#']).next().unwrap_or_default();
                    tracing::debug!("Named pipe URL listener: delivering {safe}");
                    callback(url);
                }
            })
        {
            tracing::warn!("Failed to spawn pipe URL listener thread: {e}");
        }
    }
}

// ── Cleanup ───────────────────────────────────────────────────────────────────

#[cfg(unix)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let Some(socket_path) = &self.socket_path {
            let _ = std::fs::remove_file(socket_path);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Claim, SingleInstance};
    use std::path::{Path, PathBuf};

    /// The container `$TMPDIR` a sandboxed macOS build actually runs with.
    fn container_tmp(user: &str) -> PathBuf {
        PathBuf::from(format!(
            "/Users/{user}/Library/Containers/app.mezon.ai/Data/tmp"
        ))
    }

    #[test]
    fn candidates_always_fit_sun_path() {
        for user in ["a", "ngoc", "hoangphuongnguyen", &"x".repeat(64)] {
            for tmp in [
                container_tmp(user),
                PathBuf::from("/var/folders/ml/53wfzsjn4n1cx3lmty3p8npw0000gn/T"),
                PathBuf::from("/tmp"),
            ] {
                for path in SingleInstance::socket_paths_for(None, &tmp, user) {
                    assert!(
                        path.as_os_str().len() <= SingleInstance::MAX_SOCKET_PATH,
                        "{} is {} bytes",
                        path.display(),
                        path.as_os_str().len()
                    );
                }
            }
        }
    }

    /// The regression: a long login name plus a sandbox container `$TMPDIR`
    /// pushed the only candidate past `sun_path`, so `bind` failed and the app
    /// exited before opening a window.
    #[test]
    fn long_user_in_sandbox_container_still_has_a_candidate() {
        let user = "hoangphuongnguyen";
        let tmp = container_tmp(user);

        let readable = tmp.join(format!("mezon-desktop-{user}")).join("mezon.sock");
        assert!(
            readable.as_os_str().len() > SingleInstance::MAX_SOCKET_PATH,
            "expected the readable path to overrun sun_path"
        );

        let paths = SingleInstance::socket_paths_for(None, &tmp, user);
        assert!(!paths.is_empty(), "no bindable candidate left");
        assert!(!paths.contains(&readable));
    }

    /// The fallback has to survive realistic login names, not just the one that
    /// triggered the bug — `bind` failing again would silently drop the guard.
    #[test]
    fn sandbox_container_leaves_a_candidate_for_realistic_user_names() {
        for len in 1..=30 {
            let user = "u".repeat(len);
            let paths = SingleInstance::socket_paths_for(None, &container_tmp(&user), &user);
            assert!(
                !paths.is_empty(),
                "no candidate for a {len}-char login name"
            );
        }
    }

    #[test]
    fn short_user_keeps_the_readable_path_first() {
        let user = "ngoc";
        let tmp = container_tmp(user);

        let paths = SingleInstance::socket_paths_for(None, &tmp, user);
        assert_eq!(
            paths.first(),
            Some(&tmp.join(format!("mezon-desktop-{user}")).join("mezon.sock"))
        );
    }

    #[test]
    fn runtime_dir_is_dropped_when_it_cannot_fit() {
        let long = PathBuf::from(format!("/run/user/1000/{}", "d".repeat(120)));
        let paths = SingleInstance::socket_paths_for(Some(&long), Path::new("/tmp"), "ngoc");
        assert!(paths.iter().all(|path| !path.starts_with(&long)));
    }

    /// Short-lived unique path: `sun_path` leaves no room for long temp names.
    fn scratch_socket() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};

        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "mz-{}-{}.sock",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn claims_a_free_path() {
        let path = scratch_socket();
        assert!(matches!(SingleInstance::claim(&path), Claim::Ours(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn detects_a_live_owner() {
        let path = scratch_socket();
        let _owner = std::os::unix::net::UnixListener::bind(&path).expect("bind owner");

        assert!(matches!(SingleInstance::claim(&path), Claim::Taken(_)));
        let _ = std::fs::remove_file(&path);
    }

    /// Losing the race between the `exists` probe and `bind` must report the
    /// winner, not fall through to a different candidate — that would leave two
    /// instances running with the guard silently gone.
    #[test]
    fn a_lost_bind_race_reports_the_winner() {
        let path = scratch_socket();
        let _winner = std::os::unix::net::UnixListener::bind(&path).expect("bind winner");

        // `bind_or_detect` is what the loser reaches once the probe has already
        // decided the path was free.
        assert!(matches!(
            SingleInstance::bind_or_detect(&path),
            Claim::Taken(_)
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_stale_socket_file_is_cleared() {
        let path = scratch_socket();
        std::fs::write(&path, b"not a socket").expect("write stale file");

        assert!(matches!(SingleInstance::claim(&path), Claim::Ours(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn user_digest_is_stable_and_per_user() {
        assert_eq!(
            SingleInstance::user_digest("ngoc"),
            SingleInstance::user_digest("ngoc")
        );
        assert_ne!(
            SingleInstance::user_digest("ngoc"),
            SingleInstance::user_digest("hoangphuongnguyen")
        );
    }
}
