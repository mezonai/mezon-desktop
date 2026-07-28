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

#[cfg(debug_assertions)]
mod dev_file {
    use std::path::{Path, PathBuf};

    use anyhow::Context;

    use super::*;

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
        std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
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

#[cfg(not(debug_assertions))]
mod keychain_store {
    use keyring::Entry;

    use super::*;

    const SERVICE: &str = "mezon-desktop";
    const USERNAME: &str = "session";

    pub fn save_session(session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)?;
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

#[cfg(debug_assertions)]
pub use dev_file::{clear_session, load_session, save_session};

#[cfg(not(debug_assertions))]
pub use keychain_store::{clear_session, load_session, save_session};
