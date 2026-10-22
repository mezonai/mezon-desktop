//! Vietnamese Syllable and Subword Tokenizer with embedded 511-token vocabulary.
//! Decodes and encodes text for TinyLM transformer inference on Desktop & Mobile.

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

const EMBEDDED_VOCAB_JSON: &str = include_str!("../assets/vocab.json");

pub static TOKEN_TO_ID: LazyLock<HashMap<String, i64>> =
    LazyLock::new(|| serde_json::from_str(EMBEDDED_VOCAB_JSON).unwrap_or_else(|_| HashMap::new()));

pub static ID_TO_TOKEN: LazyLock<HashMap<i64, String>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for (k, &v) in TOKEN_TO_ID.iter() {
        m.insert(v, k.clone());
    }
    m
});

pub struct VietnameseTokenizer;

impl VietnameseTokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn vocab_size(&self) -> usize {
        TOKEN_TO_ID.len()
    }

    /// Splits text into words, punctuation, and whitespace tokens.
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let re = Regex::new(r"[\w]+|[^\w\s]|\s+").unwrap();
        let mut tokens = Vec::new();

        for mat in re.find_iter(text) {
            let part = mat.as_str();
            if TOKEN_TO_ID.contains_key(part) || part.chars().all(|c| c.is_whitespace()) {
                tokens.push(part.to_string());
            } else {
                let lower = part.to_lowercase();
                if TOKEN_TO_ID.contains_key(&lower) {
                    tokens.push(part.to_string());
                } else {
                    // Character fallback
                    for ch in part.chars() {
                        tokens.push(ch.to_string());
                    }
                }
            }
        }
        tokens
    }

    pub fn encode(&self, text: &str) -> Vec<i64> {
        let tokens = self.tokenize(text);
        let mut ids = Vec::with_capacity(tokens.len());
        let unk_id = *TOKEN_TO_ID.get("<unk>").unwrap_or(&1);

        for tok in tokens {
            if let Some(&id) = TOKEN_TO_ID.get(&tok) {
                ids.push(id);
            } else if let Some(&id) = TOKEN_TO_ID.get(&tok.to_lowercase()) {
                ids.push(id);
            } else {
                for ch in tok.chars() {
                    let ch_str = ch.to_string();
                    let id = *TOKEN_TO_ID.get(&ch_str).unwrap_or(&unk_id);
                    ids.push(id);
                }
            }
        }
        ids
    }

    pub fn decode(&self, ids: &[i64]) -> String {
        let mut result = String::new();
        for &id in ids {
            if id < 5 {
                // Skip special tokens (<pad>, <unk>, <s>, </s>, <mask>)
                continue;
            }
            if let Some(tok) = ID_TO_TOKEN.get(&id) {
                result.push_str(tok);
            }
        }
        result
    }
}
