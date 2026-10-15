use std::collections::HashMap;
use std::ops::Range;

use regex::Regex;
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

const VOCAB_JSON: &str = include_str!("../assets/vocab.json");
const CONFIG_JSON: &str = include_str!("../assets/config.json");

#[derive(Debug, Deserialize)]
struct ModelFile {
    vocab_size: usize,
    max_seq_len: usize,
    pad_token_id: u32,
    unk_token_id: u32,
    bos_token_id: u32,
    eos_token_id: u32,
    mask_token_id: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Piece {
    pub id: i64,
    pub range: Range<usize>,
    pub whole_word: bool,
}

pub(crate) struct Tokenizer {
    token_to_id: HashMap<String, u32>,
    pub id_to_token: Vec<String>,
    pub vocab_size: usize,
    pub max_seq_len: usize,
    pub pad_token_id: i64,
    pub unk_token_id: i64,
    pub bos_token_id: i64,
    pub eos_token_id: i64,
    pub mask_token_id: i64,
    split: Regex,
}

impl Tokenizer {
    pub fn bundled() -> Self {
        match Self::parse(VOCAB_JSON, CONFIG_JSON) {
            Ok(tokenizer) => tokenizer,
            Err(error) => {
                tracing::warn!("spellcheck vocab failed to load: {error}");
                Self::empty()
            }
        }
    }

    fn empty() -> Self {
        Self {
            token_to_id: HashMap::new(),
            id_to_token: Vec::new(),
            vocab_size: 0,
            max_seq_len: 64,
            pad_token_id: 0,
            unk_token_id: 1,
            bos_token_id: 2,
            eos_token_id: 3,
            mask_token_id: 4,
            split: Regex::new(r"[\w]+|[^\w\s]|\s+").expect("word split"),
        }
    }

    fn parse(vocab_json: &str, config_json: &str) -> Result<Self, serde_json::Error> {
        let token_to_id: HashMap<String, u32> = serde_json::from_str(vocab_json)?;
        let config: ModelFile = serde_json::from_str(config_json)?;
        let mut id_to_token = vec![String::new(); config.vocab_size.max(token_to_id.len())];
        for (token, id) in &token_to_id {
            if let Some(slot) = id_to_token.get_mut(*id as usize) {
                *slot = token.clone();
            }
        }
        Ok(Self {
            token_to_id,
            id_to_token,
            vocab_size: config.vocab_size,
            max_seq_len: config.max_seq_len,
            pad_token_id: i64::from(config.pad_token_id),
            unk_token_id: i64::from(config.unk_token_id),
            bos_token_id: i64::from(config.bos_token_id),
            eos_token_id: i64::from(config.eos_token_id),
            mask_token_id: i64::from(config.mask_token_id),
            split: Regex::new(r"[\w]+|[^\w\s]|\s+").expect("word split"),
        })
    }

    pub fn tokenize(&self, text: &str) -> Vec<Piece> {
        let mut pieces = Vec::new();
        for found in self.split.find_iter(text) {
            self.push_token(&mut pieces, found.as_str(), found.range());
        }
        pieces
    }

    fn push_token(&self, pieces: &mut Vec<Piece>, token: &str, range: Range<usize>) {
        if let Some(id) = self.token_to_id.get(token) {
            pieces.push(Piece {
                id: i64::from(*id),
                range,
                whole_word: is_whole_word(token),
            });
            return;
        }
        let mut offset = range.start;
        for ch in token.chars() {
            let next = offset + ch.len_utf8();
            let mut encoded = [0u8; 4];
            let key = ch.encode_utf8(&mut encoded);
            let id = self
                .token_to_id
                .get(key)
                .copied()
                .map(i64::from)
                .unwrap_or(self.unk_token_id);
            pieces.push(Piece {
                id,
                range: offset..next,
                whole_word: false,
            });
            offset = next;
        }
    }
}

fn is_whole_word(token: &str) -> bool {
    let mut count = 0usize;
    for ch in token.chars() {
        if !ch.is_alphabetic() {
            return false;
        }
        count += 1;
    }
    count >= 2
}

pub(crate) fn nfc_text(text: &str) -> Option<String> {
    let normalized: String = text.nfc().collect();
    (normalized == text).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllables_in_vocab_stay_whole_tokens() {
        let tokenizer = Tokenizer::bundled();
        let text = "xin chào";
        let pieces = tokenizer.tokenize(text);
        let ids: Vec<i64> = pieces.iter().map(|piece| piece.id).collect();
        assert_eq!(ids, vec![348, 5, 360]);
        assert_eq!(pieces[0].range, 0..3);
        assert!(pieces[0].whole_word);
        assert!(!pieces[1].whole_word);
        assert_eq!(pieces[2].range, 4..text.len());
        assert!(pieces[2].whole_word);
    }

    #[test]
    fn unknown_word_falls_back_to_characters() {
        let tokenizer = Tokenizer::bundled();
        let pieces = tokenizer.tokenize("xyzzy");
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|piece| !piece.whole_word));
    }
}
