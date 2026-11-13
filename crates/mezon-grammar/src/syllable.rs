use crate::assets::{SYLLABLE_VOCAB_FILE, load_asset_str};
use std::collections::HashMap;
use std::sync::LazyLock;

const PUNCT_TOKENS: &[&str] = &[",", ".", "?", "!", ";", ":"];

pub static SYLLABLE_TO_ID: LazyLock<HashMap<String, i64>> = LazyLock::new(|| {
    load_asset_str(SYLLABLE_VOCAB_FILE)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
});

pub static ID_TO_SYLLABLE: LazyLock<HashMap<i64, String>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for (k, &v) in SYLLABLE_TO_ID.iter() {
        m.insert(v, k.clone());
    }
    m
});

pub static SYLLABLE_CANDIDATES: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    for tok in SYLLABLE_TO_ID.keys() {
        if !tok.starts_with('<') && !PUNCT_TOKENS.contains(&tok.as_str()) {
            let base = crate::rules::strip_accents(&tok.to_lowercase());
            m.entry(base).or_default().push(tok.clone());
        }
    }
    m
});

pub fn syllable_id(syl: &str) -> Option<i64> {
    SYLLABLE_TO_ID.get(syl).copied()
}

pub fn id_to_syllable(id: i64) -> Option<&'static str> {
    ID_TO_SYLLABLE.get(&id).map(|s| s.as_str())
}

pub fn candidates_for_syllable(base: &str) -> Option<&'static [String]> {
    SYLLABLE_CANDIDATES.get(base).map(|v| v.as_slice())
}

pub fn is_valid_vietnamese_syllable(syl: &str) -> bool {
    let lower = syl.to_lowercase();
    if PUNCT_TOKENS.contains(&lower.as_str()) {
        return false;
    }
    SYLLABLE_TO_ID.contains_key(&lower)
}
