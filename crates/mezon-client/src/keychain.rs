//! OS keychain storage for the Mezon session token.
//!
//! Uses the [`keyring`] crate which delegates to:
//!   - macOS: Keychain Services
//!   - Windows: Windows Credential Manager
//!   - Linux: libsecret / Secret Service D-Bus
//!
//! The session is JSON-serialised and stored under the service name `"mezon-desktop"`.

use anyhow::Result;
use keyring::Entry;

use crate::session::Session;

const SERVICE: &str = "mezon-desktop";
const USERNAME: &str = "session";

/// Persist `session` to the OS keychain.
pub fn save_session(session: &Session) -> Result<()> {
    let json = serde_json::to_string(session)?;
    let entry = Entry::new(SERVICE, USERNAME)?;
    entry.set_password(&json)?;
    tracing::debug!("Session saved to keychain (user_id={})", session.user_id);
    Ok(())
}

/// Load the stored session from the OS keychain.
///
/// Returns `None` if no session is stored or if the stored data is corrupt.
pub fn load_session() -> Option<Session> {
    let entry = Entry::new(SERVICE, USERNAME).ok()?;
    let json = entry.get_password().ok()?;
    match serde_json::from_str::<Session>(&json) {
        Ok(session) => {
            tracing::debug!("Session loaded from keychain (user_id={})", session.user_id);
            Some(session)
        }
        Err(e) => {
            tracing::warn!("Failed to deserialise stored session, ignoring: {e}");
            None
        }
    }
}

/// Remove the stored session from the OS keychain.
pub fn clear_session() -> Result<()> {
    let entry = Entry::new(SERVICE, USERNAME)?;
    entry.delete_credential()?;
    tracing::debug!("Session cleared from keychain");
    Ok(())
}
