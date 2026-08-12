use anyhow::Result;
use mmn_client::{EphemeralKeyPair, ZkProof};
use serde::{Deserialize, Serialize};

use crate::wallet::WalletDetail;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedWalletState {
    pub user_id: String,
    pub is_enabled: bool,
    pub wallet: Option<WalletDetail>,
    pub zk_proofs: Option<ZkProof>,
    pub ephemeral: Option<EphemeralKeyPair>,
    #[serde(default)]
    pub proof_minted_at: Option<i64>,
}

fn parse_wallet(json: &str) -> Option<PersistedWalletState> {
    match serde_json::from_str::<PersistedWalletState>(json) {
        Ok(state) if state.is_enabled => Some(state),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("Failed to deserialise stored wallet, ignoring: {e}");
            None
        }
    }
}

#[cfg(debug_assertions)]
mod dev_file {
    use std::path::PathBuf;

    use anyhow::Context;

    use super::*;

    fn persist_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("mezon")
            .join("dev-wallet.json")
    }

    pub fn save_wallet(state: &PersistedWalletState) -> Result<()> {
        let path = persist_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let json = serde_json::to_string(state)?;
        std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn load_wallet() -> Option<PersistedWalletState> {
        let path = persist_path();
        let json = std::fs::read_to_string(&path).ok()?;
        parse_wallet(&json)
    }

    pub fn clear_wallet() -> Result<()> {
        let path = persist_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
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

    const USERNAME: &str = "wallet";

    fn service_name() -> &'static str {
        if crate::running_in_app_sandbox() {
            "mezon-desktop.mas"
        } else {
            "mezon-desktop"
        }
    }

    pub fn save_wallet(state: &PersistedWalletState) -> Result<()> {
        let json = serde_json::to_string(state)?;
        let entry = Entry::new(service_name(), USERNAME)?;
        entry.set_password(&json)?;
        Ok(())
    }

    pub fn load_wallet() -> Option<PersistedWalletState> {
        let entry = Entry::new(service_name(), USERNAME).ok()?;
        let json = entry.get_password().ok()?;
        parse_wallet(&json)
    }

    pub fn clear_wallet() -> Result<()> {
        let entry = Entry::new(service_name(), USERNAME)?;
        entry.delete_credential()?;
        Ok(())
    }
}

#[cfg(debug_assertions)]
pub use dev_file::{clear_wallet, load_wallet, save_wallet};

#[cfg(not(debug_assertions))]
pub use keychain_store::{clear_wallet, load_wallet, save_wallet};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::WalletDetail;

    #[test]
    fn persisted_wallet_roundtrip() {
        let state = PersistedWalletState {
            user_id: "42".to_string(),
            is_enabled: true,
            wallet: Some(WalletDetail {
                address: "0xabc".to_string(),
                balance: "1000000".to_string(),
            }),
            zk_proofs: Some(ZkProof {
                proof: "proof".to_string(),
                public_input: "input".to_string(),
            }),
            ephemeral: Some(EphemeralKeyPair {
                private_key: "priv".to_string(),
                public_key: "pub".to_string(),
            }),
            proof_minted_at: Some(1_700_000_000),
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let parsed = parse_wallet(&json).expect("parse");
        assert_eq!(parsed.user_id, "42");
        assert!(parsed.is_enabled);
        assert_eq!(
            parsed.wallet.as_ref().map(|w| w.address.as_str()),
            Some("0xabc")
        );
    }
}
