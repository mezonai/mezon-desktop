mod tokenizer;

use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use ort::session::Session;
use ort::value::Tensor;
use regex::Regex;
use tokenizer::{Piece, Tokenizer, nfc_text};
use unicode_normalization::UnicodeNormalization;

const MODEL_BYTES: &[u8] = include_bytes!("../assets/model.onnx");
const MAX_SPANS: usize = 8;
const MAX_TOKEN_PROB: f32 = 0.3;
const MIN_REPLACEMENT_PROB: f32 = 0.05;
const MIN_PROB_RATIO: f32 = 2.0;
const MAX_REPLIES: usize = 3;
const MAX_REPLY_TOKENS: usize = 12;
const MIN_REPLY_PROB: f32 = 0.002;
const MAX_PROMPT_CHARS: usize = 400;

#[derive(Clone, Debug, PartialEq)]
pub struct Correction {
    pub byte_range: Range<usize>,
    pub original: String,
    pub replacement: String,
}

pub fn check(text: &str, protected: &[Range<usize>]) -> Vec<Correction> {
    if text.is_empty() {
        return Vec::new();
    }
    let Some(text) = nfc_text(text) else {
        return Vec::new();
    };
    let tokenizer = bundled_tokenizer();
    if tokenizer.vocab_size == 0 || tokenizer.max_seq_len == 0 {
        return Vec::new();
    }
    let pieces = tokenizer.tokenize(&text);
    let skipped = skip_ranges(&text);
    let window_start = pieces.len().saturating_sub(tokenizer.max_seq_len);
    let window = &pieces[window_start..];
    let targets: Vec<usize> = window
        .iter()
        .enumerate()
        .filter_map(|(local, piece)| {
            (local > 0 && is_target(&text, piece, protected, &skipped)).then_some(local)
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let ids: Vec<i64> = window.iter().map(|piece| piece.id).collect();
    let Some(probs) = forward(&ids, tokenizer.vocab_size) else {
        return Vec::new();
    };
    targets
        .into_iter()
        .filter_map(|local| {
            let piece = window.get(local)?;
            let row_start = (local - 1) * tokenizer.vocab_size;
            let row = probs.get(row_start..row_start + tokenizer.vocab_size)?;
            correction_for(piece, &text, row, &tokenizer.id_to_token)
        })
        .take(MAX_SPANS)
        .collect()
}

pub fn quick_replies(prompt: &str) -> Vec<String> {
    let prompt: String = prompt.trim().chars().nfc().collect();
    if prompt.is_empty() {
        return Vec::new();
    }
    let prompt = clip_chars(&prompt, MAX_PROMPT_CHARS);
    let tokenizer = bundled_tokenizer();
    if tokenizer.vocab_size == 0 || tokenizer.max_seq_len < 2 {
        return Vec::new();
    }
    let ids = prompt_ids(tokenizer, prompt);
    let Some(row) = last_row(&ids, tokenizer.vocab_size) else {
        return Vec::new();
    };
    let mut replies: Vec<String> = Vec::new();
    for seed in reply_seeds(&row, &tokenizer.id_to_token, tokenizer, MAX_REPLIES) {
        if replies.len() == MAX_REPLIES {
            break;
        }
        let Some(reply) = complete_reply(tokenizer, ids.clone(), seed) else {
            continue;
        };
        if !replies.contains(&reply) {
            replies.push(reply);
        }
    }
    replies
}

fn clip_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

fn prompt_ids(tokenizer: &Tokenizer, text: &str) -> Vec<i64> {
    let mut ids = vec![tokenizer.bos_token_id];
    ids.extend(tokenizer.tokenize(text).into_iter().map(|piece| piece.id));
    let room = tokenizer.max_seq_len - 1;
    if ids.len() > room {
        let drop = ids.len() - room;
        ids.drain(1..1 + drop);
    }
    ids
}

fn last_row(ids: &[i64], vocab_size: usize) -> Option<Vec<f32>> {
    let probs = forward(ids, vocab_size)?;
    let start = ids.len().saturating_sub(1) * vocab_size;
    probs.get(start..start + vocab_size).map(<[f32]>::to_vec)
}

fn ranked(probs: &[f32]) -> Vec<(usize, f32)> {
    let mut ranked: Vec<(usize, f32)> = probs.iter().copied().enumerate().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked
}

fn reply_seeds(probs: &[f32], tokens: &[String], tokenizer: &Tokenizer, limit: usize) -> Vec<i64> {
    let mut seeds = Vec::new();
    for (index, prob) in ranked(probs) {
        if seeds.len() == limit || prob < MIN_REPLY_PROB {
            break;
        }
        let Ok(id) = i64::try_from(index) else {
            continue;
        };
        if is_control(tokenizer, id) {
            continue;
        }
        if tokens
            .get(index)
            .is_some_and(|token| token.chars().any(char::is_alphabetic))
        {
            seeds.push(id);
        }
    }
    seeds
}

fn complete_reply(tokenizer: &Tokenizer, mut ids: Vec<i64>, seed: i64) -> Option<String> {
    let mut generated = vec![seed];
    ids.push(seed);
    while generated.len() < MAX_REPLY_TOKENS && ids.len() < tokenizer.max_seq_len {
        let row = last_row(&ids, tokenizer.vocab_size)?;
        let Some(next) = argmax_token(&row, tokenizer) else {
            break;
        };
        let token = tokenizer.id_to_token.get(usize::try_from(next).ok()?)?;
        generated.push(next);
        ids.push(next);
        if stops_reply(token) {
            break;
        }
        if is_collapsed(&generated) {
            return None;
        }
    }
    if is_collapsed(&generated) || content_words(&generated, &tokenizer.id_to_token) < 2 {
        return None;
    }
    let text = decode_ids(&generated, &tokenizer.id_to_token);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn is_collapsed(ids: &[i64]) -> bool {
    let mut run = 1usize;
    for pair in ids.windows(2) {
        if pair[0] == pair[1] {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 1;
        }
    }
    false
}

fn content_words(ids: &[i64], tokens: &[String]) -> usize {
    decode_ids(ids, tokens)
        .split_whitespace()
        .filter(|word| word.chars().filter(|ch| ch.is_alphabetic()).count() >= 2)
        .count()
}

fn argmax_token(probs: &[f32], tokenizer: &Tokenizer) -> Option<i64> {
    let (index, _) = ranked(probs).into_iter().find(|(index, _)| {
        i64::try_from(*index)
            .is_ok_and(|id| !is_control(tokenizer, id) || id == tokenizer.eos_token_id)
    })?;
    let id = i64::try_from(index).ok()?;
    (id != tokenizer.eos_token_id).then_some(id)
}

fn is_control(tokenizer: &Tokenizer, id: i64) -> bool {
    id == tokenizer.pad_token_id
        || id == tokenizer.unk_token_id
        || id == tokenizer.bos_token_id
        || id == tokenizer.eos_token_id
        || id == tokenizer.mask_token_id
}

fn stops_reply(token: &str) -> bool {
    matches!(token, "." | "?" | "!" | "…" | "\n")
}

fn decode_ids(ids: &[i64], tokens: &[String]) -> String {
    ids.iter()
        .filter_map(|id| tokens.get(usize::try_from(*id).ok()?))
        .map(String::as_str)
        .collect()
}

fn bundled_tokenizer() -> &'static Tokenizer {
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(Tokenizer::bundled)
}

fn is_target(
    text: &str,
    piece: &Piece,
    protected: &[Range<usize>],
    skipped: &[Range<usize>],
) -> bool {
    piece.whole_word
        && word_is_closed(text, piece.range.end)
        && !sigil_prefixed(text, piece.range.start)
        && !protected
            .iter()
            .chain(skipped)
            .any(|range| ranges_overlap(&piece.range, range))
}

fn word_is_closed(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_alphanumeric())
}

fn sigil_prefixed(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '@' | '#' | ':'))
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn skip_ranges(text: &str) -> Vec<Range<usize>> {
    static SKIP: OnceLock<Option<Regex>> = OnceLock::new();
    let pattern = SKIP.get_or_init(|| {
        Regex::new(
            r"(?i)\bhttps?://\S+|\bwww\.\S+|\b[\w.+-]+@[\w-]+(?:\.[\w-]+)+|(?s)```.*?```|`[^`\n]+`|(?m)(?:^|\s)(/[\p{L}\p{N}_-]+)",
        )
        .ok()
    });
    pattern
        .as_ref()
        .map(|pattern| pattern.find_iter(text).map(|found| found.range()).collect())
        .unwrap_or_default()
}

fn correction_for(
    piece: &Piece,
    text: &str,
    probs: &[f32],
    id_to_token: &[String],
) -> Option<Correction> {
    let actual = *probs.get(usize::try_from(piece.id).ok()?)?;
    if actual >= MAX_TOKEN_PROB {
        return None;
    }
    let original = text.get(piece.range.clone())?;
    let (index, prob) = ranked(probs)
        .into_iter()
        .find(|(index, _)| i64::try_from(*index).ok() != Some(piece.id))?;
    if prob < MIN_REPLACEMENT_PROB || prob < actual * MIN_PROB_RATIO {
        return None;
    }
    let token = id_to_token.get(index)?;
    if !is_word_token(token) {
        return None;
    }
    let replacement = match_case(original, token);
    if replacement.eq_ignore_ascii_case(original) {
        return None;
    }
    Some(Correction {
        byte_range: piece.range.clone(),
        original: original.to_string(),
        replacement,
    })
}

fn is_word_token(token: &str) -> bool {
    token.chars().count() >= 2 && token.chars().all(char::is_alphabetic)
}

fn match_case(original: &str, replacement: &str) -> String {
    let letters = || original.chars().filter(|ch| ch.is_alphabetic());
    if letters().next().is_some() && letters().all(char::is_uppercase) {
        return replacement.to_uppercase();
    }
    if original.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = replacement.chars();
        let Some(first) = chars.next() else {
            return String::new();
        };
        let mut out: String = first.to_uppercase().collect();
        out.extend(chars);
        return out;
    }
    replacement.to_string()
}

enum ModelSlot {
    Pending,
    Ready(Session),
    Unavailable,
}

fn session_slot() -> &'static Mutex<ModelSlot> {
    static SESSION: OnceLock<Mutex<ModelSlot>> = OnceLock::new();
    SESSION.get_or_init(|| Mutex::new(ModelSlot::Pending))
}

fn load_session() -> Result<Session, ort::Error> {
    Session::builder()?
        .with_intra_threads(1)?
        .commit_from_memory(MODEL_BYTES)
}

fn forward(ids: &[i64], vocab_size: usize) -> Option<Vec<f32>> {
    if ids.is_empty() || vocab_size == 0 {
        return None;
    }
    let mut slot = session_slot()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if matches!(*slot, ModelSlot::Pending) {
        *slot = match load_session() {
            Ok(session) => ModelSlot::Ready(session),
            Err(error) => {
                tracing::warn!("spellcheck model unavailable: {error}");
                ModelSlot::Unavailable
            }
        };
    }
    let ModelSlot::Ready(session) = &mut *slot else {
        return None;
    };
    let input = Tensor::from_array(([1usize, ids.len()], ids.to_vec())).ok()?;
    let outputs = session.run(ort::inputs!["input_ids" => input]).ok()?;
    let (_, logits) = outputs.get("logits")?.try_extract_tensor::<f32>().ok()?;
    if logits.len() != ids.len() * vocab_size {
        tracing::warn!(
            got = logits.len(),
            expected = ids.len() * vocab_size,
            "spellcheck logits had an unexpected length"
        );
        return None;
    }
    Some(logits.chunks(vocab_size).flat_map(softmax).collect())
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|logit| (logit - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum == 0.0 {
        return vec![0.0; logits.len()];
    }
    exps.into_iter().map(|value| value / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_logits_cover_every_position() {
        let tokenizer = bundled_tokenizer();
        let ids = prompt_ids(tokenizer, "xin chào bạn");
        let probs = forward(&ids, tokenizer.vocab_size).expect("model output");
        assert_eq!(probs.len(), ids.len() * tokenizer.vocab_size);
        for row in probs.chunks(tokenizer.vocab_size) {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn corrections_stay_inside_the_text() {
        let text = "xin chào bạn ";
        for correction in check(text, &[]) {
            assert_eq!(
                text.get(correction.byte_range.clone()),
                Some(correction.original.as_str())
            );
            assert!(!correction.replacement.is_empty());
        }
    }

    #[test]
    fn word_still_being_typed_is_not_a_target() {
        let tokenizer = bundled_tokenizer();
        let pieces = tokenizer.tokenize("xin");
        assert!(!is_target("xin", &pieces[0], &[], &[]));
        assert!(is_target("xin ", &pieces[0], &[], &[]));
    }

    #[test]
    fn links_mail_code_and_commands_are_skipped() {
        let text = "xem https://mezon.ai/a và a@b.co `code` /kick x";
        let skipped = skip_ranges(text);
        for needle in ["https://mezon.ai/a", "a@b.co", "code", "/kick"] {
            let at = text.find(needle).expect("needle");
            assert!(skipped.iter().any(|range| range.contains(&at)));
        }
    }

    #[test]
    fn control_tokens_never_seed_a_reply() {
        let tokenizer = bundled_tokenizer();
        let mut probs = vec![0.0f32; tokenizer.vocab_size];
        let bos = usize::try_from(tokenizer.bos_token_id).expect("bos");
        probs[bos] = 0.9;
        assert!(reply_seeds(&probs, &tokenizer.id_to_token, tokenizer, 3).is_empty());
    }

    #[test]
    fn non_nfc_text_is_left_alone() {
        let decomposed: String = "\u{0065}\u{0302}".nfd().collect();
        assert!(check(&decomposed, &[]).is_empty());
    }

    #[test]
    fn blank_prompt_has_no_replies() {
        assert!(quick_replies("   ").is_empty());
    }
}
