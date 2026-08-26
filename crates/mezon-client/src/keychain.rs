use anyhow::Result;

use crate::session::Session;

fn parse_session(json: &str) -> Option<Session> {
    match serde_json::from_str::<Session>(json) {
        Ok(session) => Some(session),
        Err(e) => {
            tracing::warn!("Failed to deserialise stored session, ignoring: {e}");
            None
        }
    }
}

#[cfg(any(debug_assertions, feature = "dev-session"))]
mod dev_file {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::Context;

    use super::*;

    static COMMIT_LOCK: Mutex<()> = Mutex::new(());
    static STAGED_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn persist_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mezon")
            .join("dev-session.json")
    }

    fn import_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(path) = std::env::var("MEZON_IMPORT_SESSION_FILE") {
            paths.push(PathBuf::from(path));
        }
        if let Ok(cwd) = std::env::current_dir() {
            paths.push(cwd.join(".session.json"));
        }
        paths.push(persist_path());
        paths
    }

    /// The debug store keeps the token pair in the clear, and the default umask would leave it
    /// readable by every local account. The rename below carries this mode onto the live file.
    fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(bytes)?;
            file.sync_all()
        }
        #[cfg(not(unix))]
        std::fs::write(path, bytes)
    }

    fn read_from_path(path: &Path) -> Option<Session> {
        let json = std::fs::read_to_string(path).ok()?;
        parse_session(&json).inspect(|session| {
            tracing::debug!(
                "Session loaded from {} (user_id={})",
                path.display(),
                session.user_id
            );
        })
    }

    pub fn save_session(session: &Session) -> Result<()> {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let json = serde_json::to_string(session)?;

        let _commit = COMMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let sequence = STAGED_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staged = path.with_extension(format!("json.{}.{sequence}.tmp", std::process::id()));
        write_private(&staged, json.as_bytes())
            .with_context(|| format!("write {}", staged.display()))?;
        if let Err(e) = std::fs::rename(&staged, &path) {
            let _ = std::fs::remove_file(&staged);
            return Err(e).with_context(|| format!("rename into {}", path.display()));
        }
        tracing::debug!(
            "Session saved to {} (user_id={})",
            path.display(),
            session.user_id
        );
        Ok(())
    }

    pub fn load_session() -> Option<Session> {
        import_paths()
            .into_iter()
            .find_map(|path| read_from_path(&path))
    }

    pub fn clear_session() -> Result<()> {
        let path = persist_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::debug!("Session cleared from {}", path.display());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("remove {}", path.display()));
            }
        }
        Ok(())
    }
}

#[cfg(all(
    not(any(debug_assertions, feature = "dev-session")),
    target_os = "windows"
))]
mod dpapi_store {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use anyhow::{Context, bail};
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;

    use super::*;

    const LEGACY_SERVICE: &str = "mezon-desktop";
    const LEGACY_USERNAME: &str = "session";

    static COMMIT_LOCK: Mutex<()> = Mutex::new(());
    static STAGED_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn session_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mezon")
            .join("session.dat")
    }

    fn staged_path(path: &Path) -> PathBuf {
        let sequence = STAGED_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        path.with_extension(format!("dat.{}.{sequence}.tmp", std::process::id()))
    }

    fn take_blob(blob: &mut CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if blob.pbData.is_null() {
            return Vec::new();
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
        unsafe { LocalFree(Some(HLOCAL(blob.pbData.cast()))) };
        blob.pbData = std::ptr::null_mut();
        blob.cbData = 0;
        bytes
    }

    fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(plain.len()).context("session is too large to encrypt")?,
            pbData: plain.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .context("CryptProtectData failed")?;
        let protected = take_blob(&mut output);
        if protected.is_empty() {
            bail!("CryptProtectData returned an empty blob");
        }
        Ok(protected)
    }

    fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(cipher.len())
                .context("stored session is too large to decrypt")?,
            pbData: cipher.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .context("CryptUnprotectData failed")?;
        Ok(take_blob(&mut output))
    }

    fn load_from_file(path: &Path) -> Option<Session> {
        let cipher = match std::fs::read(path) {
            Ok(cipher) => cipher,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!("Failed to read {}: {e}", path.display());
                return None;
            }
        };
        let plain = match unprotect(&cipher) {
            Ok(plain) => plain,
            Err(e) => {
                tracing::warn!("Failed to decrypt the stored session, ignoring: {e:#}");
                return None;
            }
        };
        let json = String::from_utf8(plain).ok()?;
        parse_session(&json).inspect(|session| {
            tracing::debug!(
                "Session loaded from {} (user_id={})",
                path.display(),
                session.user_id
            );
        })
    }

    fn load_legacy_credential() -> Option<Session> {
        let entry = keyring::Entry::new(LEGACY_SERVICE, LEGACY_USERNAME).ok()?;
        parse_session(&entry.get_password().ok()?)
    }

    fn clear_legacy_credential() {
        if let Ok(entry) = keyring::Entry::new(LEGACY_SERVICE, LEGACY_USERNAME) {
            let _ = entry.delete_credential();
        }
    }

    pub fn save_session(session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)?;
        let protected = protect(json.as_bytes())?;
        let path = session_path();
        let parent = path.parent().context("session path has no parent")?;
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

        let _commit = COMMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let staged = staged_path(&path);
        std::fs::write(&staged, &protected)
            .with_context(|| format!("write {}", staged.display()))?;
        if let Err(e) = std::fs::rename(&staged, &path) {
            let _ = std::fs::remove_file(&staged);
            return Err(e).with_context(|| format!("rename into {}", path.display()));
        }
        tracing::debug!(
            "Session saved to {} (user_id={})",
            path.display(),
            session.user_id
        );
        Ok(())
    }

    pub fn load_session() -> Option<Session> {
        let path = session_path();
        if let Some(session) = load_from_file(&path) {
            return Some(session);
        }
        let session = load_legacy_credential()?;
        tracing::info!("Migrating the stored session out of the credential store");
        match save_session(&session) {
            Ok(()) => clear_legacy_credential(),
            Err(e) => tracing::warn!("Failed to migrate the stored session: {e:#}"),
        }
        Some(session)
    }

    pub fn clear_session() -> Result<()> {
        let path = session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {}", path.display())),
        }
        clear_legacy_credential();
        tracing::debug!("Session cleared from {}", path.display());
        Ok(())
    }
}

#[cfg(all(
    not(any(debug_assertions, feature = "dev-session")),
    not(target_os = "windows")
))]
mod keychain_store {
    use std::sync::Mutex;

    use keyring::Entry;

    use super::*;

    const SERVICE: &str = "mezon-desktop";
    const USERNAME: &str = "session";

    static COMMIT_LOCK: Mutex<()> = Mutex::new(());

    pub fn save_session(session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)?;
        let _commit = COMMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let entry = Entry::new(SERVICE, USERNAME)?;
        entry.set_password(&json)?;
        tracing::debug!("Session saved to keychain (user_id={})", session.user_id);
        Ok(())
    }

    pub fn load_session() -> Option<Session> {
        let entry = Entry::new(SERVICE, USERNAME).ok()?;
        let json = entry.get_password().ok()?;
        parse_session(&json).inspect(|session| {
            tracing::debug!("Session loaded from keychain (user_id={})", session.user_id);
        })
    }

    pub fn clear_session() -> Result<()> {
        let entry = Entry::new(SERVICE, USERNAME)?;
        entry.delete_credential()?;
        tracing::debug!("Session cleared from keychain");
        Ok(())
    }
}

#[cfg(any(debug_assertions, feature = "dev-session"))]
pub use dev_file::{clear_session, load_session, save_session};

#[cfg(all(
    not(any(debug_assertions, feature = "dev-session")),
    target_os = "windows"
))]
pub use dpapi_store::{clear_session, load_session, save_session};

#[cfg(all(
    not(any(debug_assertions, feature = "dev-session")),
    not(target_os = "windows")
))]
pub use keychain_store::{clear_session, load_session, save_session};
