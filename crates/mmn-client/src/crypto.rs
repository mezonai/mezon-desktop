use std::cmp::Ordering;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore as _;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

use crate::error::{MmnError, Result};
use crate::types::{EphemeralKeyPair, TX_TYPE_TRANSFER_BY_KEY, TxMsg};

const ED25519_SEED_LEN: usize = 32;
const ED25519_PUBLIC_KEY_LEN: usize = 32;

#[derive(Serialize)]
struct UserSig {
    #[serde(rename = "PubKey")]
    pub_key: String,
    #[serde(rename = "Sig")]
    sig: String,
}

pub fn address_from_user_id(user_id: &str) -> String {
    let digest = Sha256::digest(user_id.as_bytes());
    bs58::encode(digest).into_string()
}

pub fn validate_address(addr: &str) -> bool {
    match bs58::decode(addr).into_vec() {
        Ok(bytes) => bytes.len() == ED25519_PUBLIC_KEY_LEN,
        Err(_) => false,
    }
}

fn raw_ed25519_to_pkcs8_hex(seed: &[u8; ED25519_SEED_LEN]) -> String {
    let mut pkcs8 = Vec::with_capacity(48);
    pkcs8.extend_from_slice(&[0x30, 0x2e]);
    pkcs8.extend_from_slice(&[0x02, 0x01, 0x00]);
    pkcs8.extend_from_slice(&[0x30, 0x0b, 0x06, 0x03, 0x2b, 0x65, 0x70]);
    pkcs8.extend_from_slice(&[0x04, 0x22, 0x04, 0x20]);
    pkcs8.extend_from_slice(seed);
    let hex = hex::encode(&pkcs8);
    pkcs8.zeroize();
    hex
}

pub fn generate_ephemeral_key_pair() -> Result<EphemeralKeyPair> {
    let mut seed = Zeroizing::new([0u8; ED25519_SEED_LEN]);
    rand::rng().fill_bytes(seed.as_mut());

    let signing_key = SigningKey::from_bytes(&seed);
    let public_key_bytes = signing_key.verifying_key().to_bytes();

    let private_key = raw_ed25519_to_pkcs8_hex(&seed);
    let public_key = bs58::encode(public_key_bytes).into_string();

    Ok(EphemeralKeyPair {
        private_key,
        public_key,
    })
}

pub fn serialize_transaction(tx: &TxMsg) -> Vec<u8> {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        tx.tx_type, tx.sender, tx.recipient, tx.amount, tx.text_data, tx.nonce, tx.extra_info
    )
    .into_bytes()
}

pub fn sign_transaction(tx: &TxMsg, private_key_hex: &str) -> Result<String> {
    let serialized = serialize_transaction(tx);
    if serialized.is_empty() {
        return Err(MmnError::Crypto("failed to serialize transaction".into()));
    }

    let mut private_key_der = hex::decode(private_key_hex)
        .map_err(|e| MmnError::Crypto(format!("invalid private key hex: {e}")))?;
    if private_key_der.len() < ED25519_SEED_LEN {
        private_key_der.zeroize();
        return Err(MmnError::Crypto(format!(
            "invalid private key length: expected at least {ED25519_SEED_LEN} bytes, got {}",
            private_key_der.len()
        )));
    }

    let mut seed = Zeroizing::new([0u8; ED25519_SEED_LEN]);
    seed.copy_from_slice(&private_key_der[private_key_der.len() - ED25519_SEED_LEN..]);
    private_key_der.zeroize();
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key_bytes = signing_key.verifying_key().to_bytes();
    let signature = signing_key.sign(&serialized).to_bytes();

    if tx.tx_type == TX_TYPE_TRANSFER_BY_KEY {
        return Ok(bs58::encode(signature).into_string());
    }

    let user_sig = UserSig {
        pub_key: BASE64.encode(public_key_bytes),
        sig: BASE64.encode(signature),
    };
    let json = serde_json::to_string(&user_sig)?;
    Ok(bs58::encode(json.as_bytes()).into_string())
}

pub fn scale_amount_to_decimals(original_amount: &str, decimals: u32) -> Result<String> {
    let base: i128 = original_amount
        .trim()
        .parse()
        .map_err(|_| MmnError::Invalid(format!("invalid amount: {original_amount}")))?;
    let factor = 10i128
        .checked_pow(decimals)
        .ok_or_else(|| MmnError::Invalid(format!("decimals too large: {decimals}")))?;
    let scaled = base
        .checked_mul(factor)
        .ok_or_else(|| MmnError::Invalid("amount overflow while scaling".into()))?;
    Ok(scaled.to_string())
}

pub fn compare_big_decimal(a: &str, b: &str) -> Ordering {
    let a = normalize_decimal(a);
    let b = normalize_decimal(b);
    let (a_neg, a_digits) = a;
    let (b_neg, b_digits) = b;

    match (a_neg, b_neg) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }

    let magnitude = match a_digits.len().cmp(&b_digits.len()) {
        Ordering::Equal => a_digits.cmp(&b_digits),
        other => other,
    };

    if a_neg {
        magnitude.reverse()
    } else {
        magnitude
    }
}

fn normalize_decimal(value: &str) -> (bool, String) {
    let trimmed = value.trim();
    let (neg, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let digits = rest.trim_start_matches('0');
    if digits.is_empty() {
        (false, "0".to_string())
    } else {
        (neg, digits.to_string())
    }
}

pub fn is_integer_amount(value: &str) -> bool {
    let trimmed = value.trim();
    let rest = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('+'))
        .unwrap_or(trimmed);
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

pub fn validate_amount(balance: &str, amount_scaled: &str) -> bool {
    if !is_integer_amount(balance) || !is_integer_amount(amount_scaled) {
        return false;
    }
    if amount_scaled.trim_start().starts_with('-') {
        return false;
    }
    compare_big_decimal(amount_scaled, balance) != Ordering::Greater
}
