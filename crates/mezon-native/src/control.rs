#[cfg(unix)]
use crate::instance::SingleInstance;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;

pub const APP_NOT_RUNNING_MSG: &str =
    "Mezon app is not running. Open the Mezon desktop app, then retry this command.";

pub fn control_server_enabled() -> bool {
    std::env::var("MEZON_CONTROL_SERVER")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !matches!(value.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

pub type ControlHandler = Arc<dyn Fn(ControlRequest) -> ControlResponse + Send + Sync>;

pub struct ControlClient;

impl ControlClient {
    pub fn request(method: &str, params: Value) -> anyhow::Result<Value> {
        let request = ControlRequest {
            id: 1,
            method: method.to_string(),
            params,
        };
        let response = Self::send(request)?;
        if let Some(error) = response.error {
            anyhow::bail!(error);
        }
        response
            .result
            .ok_or_else(|| anyhow::anyhow!("Control response missing result"))
    }

    fn send(request: ControlRequest) -> anyhow::Result<ControlResponse> {
        #[cfg(unix)]
        return Self::send_unix(request);

        #[cfg(windows)]
        return Self::send_windows(request);

        #[cfg(not(any(unix, windows)))]
        {
            let _ = request;
            anyhow::bail!(APP_NOT_RUNNING_MSG)
        }
    }

    #[cfg(unix)]
    fn send_unix(request: ControlRequest) -> anyhow::Result<ControlResponse> {
        use std::os::unix::net::UnixStream;

        let mut last_error = None;
        for path in control_socket_paths() {
            match UnixStream::connect(&path) {
                Ok(stream) => return Self::exchange(stream, request),
                Err(e) => last_error = Some(e),
            }
        }
        let error = last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "No control socket path".to_string());
        anyhow::bail!("{APP_NOT_RUNNING_MSG} ({error})")
    }

    #[cfg(windows)]
    fn send_windows(request: ControlRequest) -> anyhow::Result<ControlResponse> {
        use std::fs::OpenOptions;

        let mut pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE_NAME)
            .map_err(|_| anyhow::anyhow!(APP_NOT_RUNNING_MSG))?;
        Self::exchange(&mut pipe, request)
    }

    fn exchange(
        mut stream: impl Read + Write,
        request: ControlRequest,
    ) -> anyhow::Result<ControlResponse> {
        let payload = serde_json::to_string(&request)?;
        writeln!(stream, "{payload}")?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            anyhow::bail!("Empty control response");
        }
        Ok(serde_json::from_str(line.trim())?)
    }
}

pub struct ControlServer {
    handler: ControlHandler,
    #[cfg(unix)]
    _socket_path: Option<std::path::PathBuf>,
    #[cfg(unix)]
    listener: Option<std::os::unix::net::UnixListener>,
    #[cfg(windows)]
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl ControlServer {
    pub fn bind(handler: ControlHandler) -> anyhow::Result<Self> {
        #[cfg(unix)]
        return Self::bind_unix(handler);

        #[cfg(windows)]
        return Self::bind_windows(handler);

        #[cfg(not(any(unix, windows)))]
        {
            let _ = handler;
            Ok(Self {})
        }
    }

    #[cfg(unix)]
    fn bind_unix(handler: ControlHandler) -> anyhow::Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixStream;

        let mut last_error = None;
        for path in control_socket_paths() {
            if let Some(parent) = path.parent() {
                if let Err(e) = create_secure_socket_dir(parent) {
                    last_error = Some(e);
                    continue;
                }
                if let Err(e) = check_current_user_owned(parent) {
                    last_error = Some(e);
                    continue;
                }
            }
            if path.exists() {
                if let Err(e) = check_current_user_owned(&path) {
                    last_error = Some(e);
                    continue;
                }
                match UnixStream::connect(&path) {
                    Ok(_) => {
                        tracing::debug!("Control socket already in use at {}", path.display());
                        last_error = Some(std::io::Error::new(
                            std::io::ErrorKind::AddrInUse,
                            format!("Control socket already in use at {}", path.display()),
                        ));
                        continue;
                    }
                    Err(_) => {
                        // Stale socket file — remove before rebinding.
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            match std::os::unix::net::UnixListener::bind(&path) {
                Ok(listener) => {
                    if let Err(e) =
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    {
                        last_error = Some(e);
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    tracing::debug!("Control server listening at {}", path.display());
                    return Ok(Self {
                        handler,
                        _socket_path: Some(path),
                        listener: Some(listener),
                    });
                }
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error
            .unwrap_or_else(|| std::io::Error::other("No control socket path"))
            .into())
    }

    #[cfg(windows)]
    const PIPE_NAME: &'static str = r"\\.\pipe\mezon-control";

    #[cfg(windows)]
    fn bind_windows(handler: ControlHandler) -> anyhow::Result<Self> {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let handler_for_thread = handler.clone();
        std::thread::Builder::new()
            .name("mezon-control-server".into())
            .spawn(move || Self::windows_server_loop(handler_for_thread, stop_rx))
            .map_err(|e| anyhow::anyhow!("Failed to spawn control server thread: {e}"))?;
        Ok(Self {
            handler,
            stop_tx: Some(stop_tx),
        })
    }

    pub fn run_in_background(&self) {
        #[cfg(unix)]
        self.run_unix_background();

        #[cfg(windows)]
        {
            let _ = self;
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = self;
        }
    }

    #[cfg(unix)]
    fn run_unix_background(&self) {
        let Some(listener) = self
            .listener
            .as_ref()
            .and_then(|listener| listener.try_clone().ok())
        else {
            return;
        };
        let handler = self.handler.clone();
        std::thread::Builder::new()
            .name("mezon-control-server".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let handler = handler.clone();
                            std::thread::spawn(move || {
                                if let Err(e) = Self::serve_connection(stream, handler) {
                                    tracing::debug!("Control connection error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("Control accept error: {e}");
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                }
            })
            .map_err(|e| tracing::error!("Failed to spawn control server thread: {e}"))
            .ok();
    }

    #[cfg(unix)]
    fn serve_connection(
        stream: std::os::unix::net::UnixStream,
        handler: ControlHandler,
    ) -> anyhow::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            return Ok(());
        }
        let request: ControlRequest = serde_json::from_str(line.trim())?;
        let response = handler(request);
        let payload = serde_json::to_string(&response)?;
        writeln!(writer, "{payload}")?;
        writer.flush()?;
        Ok(())
    }

    #[cfg(windows)]
    fn windows_server_loop(handler: ControlHandler, stop_rx: std::sync::mpsc::Receiver<()>) {
        use std::time::Duration;
        use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
        };
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
            PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };

        let pipe_name: Vec<u16> = Self::PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            let handle = unsafe {
                CreateNamedPipeW(
                    windows::core::PCWSTR(pipe_name.as_ptr()),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    65536,
                    65536,
                    0,
                    None,
                )
            };

            if handle == INVALID_HANDLE_VALUE {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }

            let connected = match unsafe { ConnectNamedPipe(handle, None) } {
                Ok(()) => true,
                Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
            };

            if connected {
                let raw = handle.0 as usize;
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let h = windows::Win32::Foundation::HANDLE(raw as *mut std::ffi::c_void);
                    if let Err(e) = Self::serve_windows_handle(h, handler) {
                        tracing::debug!("Windows control connection error: {e}");
                    }
                    unsafe {
                        let _ = DisconnectNamedPipe(h);
                        let _ = windows::Win32::Foundation::CloseHandle(h);
                    }
                });
            } else {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(handle);
                }
            }
        }
    }

    #[cfg(windows)]
    fn serve_windows_handle(
        handle: windows::Win32::Foundation::HANDLE,
        handler: ControlHandler,
    ) -> anyhow::Result<()> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::Storage::FileSystem::WriteFile;

        let mut buf = [0u8; 65536];
        let mut bytes_read = 0u32;
        let ok = unsafe { ReadFile(handle, Some(&mut buf), Some(&mut bytes_read), None).is_ok() };
        if !ok || bytes_read == 0 {
            return Ok(());
        }
        let line = std::str::from_utf8(&buf[..bytes_read as usize])?.trim();
        if line.is_empty() {
            return Ok(());
        }
        let request: ControlRequest = serde_json::from_str(line)?;
        let response = handler(request);
        let payload = format!("{}\n", serde_json::to_string(&response)?);
        let payload = payload.as_bytes();
        let mut bytes_written = 0u32;
        unsafe {
            WriteFile(handle, Some(payload), Some(&mut bytes_written), None)?;
        }
        Ok(())
    }
}

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\mezon-control";

#[cfg(unix)]
pub fn control_socket_paths() -> Vec<std::path::PathBuf> {
    let user = std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty())
        .unwrap_or_else(|| "user".to_owned());

    control_socket_paths_for(
        std::env::var("MEZON_CONTROL_SOCKET").ok().as_deref(),
        dirs::runtime_dir().as_deref(),
        &std::env::temp_dir(),
        &user,
    )
}

#[cfg(unix)]
fn control_socket_paths_for(
    override_path: Option<&str>,
    runtime_dir: Option<&std::path::Path>,
    tmp: &std::path::Path,
    user: &str,
) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    // An explicit override is the caller's business: hand it back untouched and
    // let `bind` report whatever is wrong with it.
    if let Some(override_path) = override_path.filter(|path| !path.is_empty()) {
        let override_path = std::path::PathBuf::from(override_path);
        if override_path.is_absolute() {
            paths.push(override_path);
        }
    }

    let mut derived = Vec::new();
    if let Some(runtime_dir) = runtime_dir {
        derived.push(runtime_dir.join("mezon-ctl.sock"));
    }
    derived.push(
        tmp.join(format!("mezon-desktop-{user}"))
            .join("mezon-ctl.sock"),
    );

    // Same `sun_path` cap as the single-instance socket: a sandboxed build runs
    // with a container `$TMPDIR`, and a long login name pushes the readable
    // path above past the limit — which would leave MCP silently unavailable.
    // Keep a fixed-width per-user directory that fits, still 0700 via
    // `create_secure_socket_dir`. The file name is shortened too: both halves
    // count against the same 103 bytes.
    derived.push(
        tmp.join(format!(
            "mezon-{:08x}",
            SingleInstance::user_digest(user) as u32
        ))
        .join("ctl.sock"),
    );
    derived.retain(|path| path.as_os_str().len() <= SingleInstance::MAX_SOCKET_PATH);

    paths.extend(derived);
    paths
}

#[cfg(unix)]
fn create_secure_socket_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
fn check_current_user_owned(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::metadata(path)?;
    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("path is not owned by current user: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::control_socket_paths_for;
    use crate::instance::SingleInstance;
    use std::path::PathBuf;

    /// The container `$TMPDIR` a sandboxed macOS build actually runs with.
    fn container_tmp(user: &str) -> PathBuf {
        PathBuf::from(format!(
            "/Users/{user}/Library/Containers/app.mezon.ai/Data/tmp"
        ))
    }

    #[test]
    fn derived_candidates_always_fit_sun_path() {
        for len in 1..=30 {
            let user = "u".repeat(len);
            let paths = control_socket_paths_for(None, None, &container_tmp(&user), &user);
            assert!(
                !paths.is_empty(),
                "no candidate for a {len}-char login name"
            );
            for path in paths {
                assert!(
                    path.as_os_str().len() <= SingleInstance::MAX_SOCKET_PATH,
                    "{} is {} bytes",
                    path.display(),
                    path.as_os_str().len()
                );
            }
        }
    }

    /// A long login name inside a sandbox container pushed the readable path
    /// past `sun_path`, which used to leave MCP with nothing to bind.
    #[test]
    fn long_user_in_sandbox_container_still_has_a_candidate() {
        let user = "hoangphuongnguyen";
        let tmp = container_tmp(user);

        let readable = tmp
            .join(format!("mezon-desktop-{user}"))
            .join("mezon-ctl.sock");
        assert!(readable.as_os_str().len() > SingleInstance::MAX_SOCKET_PATH);

        let paths = control_socket_paths_for(None, None, &tmp, user);
        assert!(!paths.is_empty());
        assert!(!paths.contains(&readable));
    }

    /// The length filter applies to what we derive, never to what the caller
    /// asked for explicitly.
    #[test]
    fn explicit_override_is_never_filtered() {
        let long = format!("/tmp/{}/mezon-ctl.sock", "d".repeat(120));
        let paths = control_socket_paths_for(Some(&long), None, &container_tmp("ngoc"), "ngoc");
        assert_eq!(paths.first(), Some(&PathBuf::from(&long)));
    }

    #[test]
    fn relative_override_is_ignored() {
        let paths = control_socket_paths_for(
            Some("relative/mezon-ctl.sock"),
            None,
            &container_tmp("ngoc"),
            "ngoc",
        );
        assert!(paths.iter().all(|path| path.is_absolute()));
    }
}
