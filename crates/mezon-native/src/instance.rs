use std::path::PathBuf;

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

    /// Windows: the server pipe handle kept alive so the listener thread can
    /// keep accepting connections.  Wrapped in an Arc so it can be cloned into
    /// the listener thread.
    #[cfg(windows)]
    _pipe_name: String,
}

impl SingleInstance {
    /// Try to acquire the single-instance lock.
    ///
    /// Returns `Ok(Some(instance))` — this is the first instance.
    /// Returns `Ok(None)` — another instance is already running.
    pub fn try_acquire() -> anyhow::Result<Option<Self>> {
        #[cfg(unix)]
        return Self::try_acquire_unix(None);

        #[cfg(windows)]
        return Self::try_acquire_windows(None);

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

            if socket_path.exists() {
                match std::os::unix::net::UnixStream::connect(&socket_path) {
                    Ok(mut stream) => {
                        tracing::info!("Another instance is already running");
                        if let Some(url) = forward_url {
                            let _ = stream.write_all(url.as_bytes());
                            tracing::debug!("Forwarded URL to running instance: {url}");
                        }
                        return Ok(None);
                    }
                    Err(_) => {
                        // Stale socket — clean up
                        let _ = std::fs::remove_file(&socket_path);
                    }
                }
            }

            match std::os::unix::net::UnixListener::bind(&socket_path) {
                Ok(listener) => {
                    tracing::debug!("Single instance lock acquired at {}", socket_path.display());
                    return Ok(Some(Self {
                        socket_path: Some(socket_path),
                        _listener: Some(listener),
                    }));
                }
                Err(e) => {
                    last_bind_error = Some(e);
                }
            }
        }

        let error = last_bind_error
            .unwrap_or_else(|| std::io::Error::other("no Unix socket path candidates available"));

        if error.kind() == std::io::ErrorKind::PermissionDenied {
            tracing::warn!(
                "Single instance lock disabled because Unix socket binding is not permitted: {error}"
            );
            return Ok(Some(Self {
                socket_path: None,
                _listener: None,
            }));
        }

        Err(error.into())
    }

    #[cfg(unix)]
    fn socket_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Some(runtime_dir) = dirs::runtime_dir() {
            paths.push(runtime_dir.join("mezon.sock"));
        }

        let user = std::env::var("USER")
            .ok()
            .filter(|user| !user.is_empty())
            .unwrap_or_else(|| "user".to_owned());

        paths.push(
            std::env::temp_dir()
                .join(format!("mezon-desktop-{user}"))
                .join("mezon.sock"),
        );

        paths
    }

    // ── Windows ───────────────────────────────────────────────────────────────

    #[cfg(windows)]
    const PIPE_NAME: &'static str = r"\\.\pipe\mezon-single-instance";

    #[cfg(windows)]
    fn try_acquire_windows(forward_url: Option<&str>) -> anyhow::Result<Option<Self>> {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        // Try to connect to an existing server pipe.
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(Self::PIPE_NAME)
        {
            Ok(mut pipe) => {
                tracing::info!("Another instance is already running (Windows named pipe)");
                if let Some(url) = forward_url {
                    let _ = pipe.write_all(url.as_bytes());
                    tracing::debug!("Forwarded URL to running instance via named pipe: {url}");
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

        // Create the named pipe server.
        Self::create_pipe_server()?;

        tracing::debug!("Single instance lock acquired (Windows named pipe)");
        Ok(Some(Self {
            _pipe_name: Self::PIPE_NAME.to_owned(),
        }))
    }

    /// Create the named pipe server instance using the Windows API.
    #[cfg(windows)]
    fn create_pipe_server() -> anyhow::Result<()> {
        use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND,
        };
        use windows::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            PIPE_WAIT,
        };

        let pipe_name: Vec<u16> = Self::PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // Create the first server instance with FILE_FLAG_FIRST_PIPE_INSTANCE
        // so a second server creation attempt fails (mutex semantics).
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
        let raw = unsafe { handle.0 as usize };

        // Transfer ownership into the listener thread.
        // The thread keeps the handle alive and continuously accepts + reads connections.
        std::thread::Builder::new()
            .name("mezon-single-instance".into())
            .spawn(move || {
                // Reconstruct HANDLE from raw value (thread owns it now)
                let h = windows::Win32::Foundation::HANDLE(raw as *mut std::ffi::c_void);
                loop {
                    let connected =
                        unsafe { windows::Win32::System::Pipes::ConnectNamedPipe(h, None).is_ok() };
                    if connected {
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
                                tracing::debug!(
                                    "Named pipe: received URL from secondary instance: {url}"
                                );
                                // No GPUI context here — URL wiring is handled via
                                // the mpsc channel in main.rs after the pipe listener
                                // notifies via listen_for_urls().
                            }
                        }
                        unsafe {
                            let _ = windows::Win32::System::Pipes::DisconnectNamedPipe(h);
                        }
                    }
                }
            })
            .expect("Failed to spawn single-instance pipe listener thread");

        Ok(())
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
            use std::io::Read as _;
            for stream in listener.incoming() {
                match stream {
                    Ok(mut s) => {
                        let mut buf = String::new();
                        if s.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                            let url = buf.trim().to_owned();
                            tracing::debug!(
                                "Unix socket: received deep link from secondary instance: {url}"
                            );
                            callback(url);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Unix socket accept error: {e}");
                        break;
                    }
                }
            }
        });
    }

    /// On Windows we create a *second* pipe server instance (the first is
    /// the acquisition lock; additional instances handle subsequent callers).
    #[cfg(windows)]
    fn listen_for_urls_windows(&self, callback: impl Fn(String) + Send + 'static) {
        let pipe_name: Vec<u16> = Self::PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        std::thread::Builder::new()
            .name("mezon-pipe-url-listener".into())
            .spawn(move || {
                use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
                use windows::Win32::{
                    Foundation::HANDLE,
                    Storage::FileSystem::{
                        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND,
                        ReadFile,
                    },
                    System::Pipes::{
                        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
                    },
                };

                let handle = unsafe {
                    CreateNamedPipeW(
                        windows::core::PCWSTR(pipe_name.as_ptr()),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        4096,
                        4096,
                        0,
                        None,
                    )
                };

                if handle == INVALID_HANDLE_VALUE {
                    tracing::warn!(
                        "Could not create URL-listener pipe: {}",
                        std::io::Error::last_os_error()
                    );
                    return;
                }

                loop {
                    let connected = unsafe { ConnectNamedPipe(handle, None).is_ok() };
                    if connected {
                        let mut buf = [0u8; 4096];
                        let mut bytes_read = 0u32;
                        let ok = unsafe {
                            ReadFile(handle, Some(&mut buf), Some(&mut bytes_read), None).is_ok()
                        };
                        if ok && bytes_read > 0 {
                            if let Ok(url) = std::str::from_utf8(&buf[..bytes_read as usize]) {
                                let url = url.trim().to_owned();
                                tracing::debug!("Named pipe URL listener: received {url}");
                                callback(url);
                            }
                        }
                        unsafe {
                            let _ = DisconnectNamedPipe(handle);
                        }
                    }
                }
            })
            .expect("Failed to spawn pipe URL listener thread");
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
