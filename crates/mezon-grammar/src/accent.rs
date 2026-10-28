use crate::onnx::{ACCENT_MAX_CHARS, ACCENT_VOCAB_SIZE, accent_char_id};
use crate::rules::{
    COMPILED_CONFUSABLES, CONFUSABLE_PHRASES, is_foreign_or_tech_word,
    is_predominantly_english_sentence, match_case, protected_literal_spans, strip_accents,
};
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

const BEAM_WIDTH: usize = 4;

const TONE_ROWS: &[(&str, &str)] = &[
    ("a", "aàáảãạ"),
    ("ă", "ăằắẳẵặ"),
    ("â", "âầấẩẫậ"),
    ("e", "eèéẻẽẹ"),
    ("ê", "êềếểễệ"),
    ("i", "iìíỉĩị"),
    ("o", "oòóỏõọ"),
    ("ô", "ôồốổỗộ"),
    ("ơ", "ơờớởỡợ"),
    ("u", "uùúủũụ"),
    ("ư", "ưừứửữự"),
    ("y", "yỳýỷỹỵ"),
    ("A", "AÀÁẢÃẠ"),
    ("Ă", "ĂẰẮẲẴẶ"),
    ("Â", "ÂẦẤẨẪẬ"),
    ("E", "EÈÉẺẼẸ"),
    ("Ê", "ÊỀẾỂỄỆ"),
    ("I", "IÌÍỈĨỊ"),
    ("O", "OÒÓỎÕỌ"),
    ("Ô", "ÔỒỐỔỖỘ"),
    ("Ơ", "ƠỜỚỞỠỢ"),
    ("U", "UÙÚỦŨỤ"),
    ("Ư", "ƯỪỨỬỮỰ"),
    ("Y", "YỲÝỶỸỴ"),
];

static CHAR_TO_TONE: LazyLock<HashMap<char, (char, u8)>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for (base, row) in TONE_ROWS {
        let base_char = base.chars().next().unwrap_or('a');
        for (tone, ch) in row.chars().enumerate() {
            map.insert(ch, (base_char, tone as u8));
        }
    }
    map
});

static VOWEL_VARIANTS: LazyLock<HashMap<char, Vec<char>>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    let pairs: &[(&str, &str)] = &[
        ("a", "aàáảãạăằắẳẵặâầấẩẫậ"),
        ("e", "eèéẻẽẹêềếểễệ"),
        ("i", "iìíỉĩị"),
        ("o", "oòóỏõọôồốổỗộơờớởỡợ"),
        ("u", "uùúủũụưừứửữự"),
        ("y", "yỳýỷỹỵ"),
        ("d", "dđ"),
    ];
    for (base, variants) in pairs {
        let lower: Vec<char> = variants.chars().collect();
        let base_char = base.chars().next().unwrap_or('a');
        map.insert(base_char, lower.clone());
        map.insert(
            base_char.to_uppercase().next().unwrap_or(base_char),
            lower
                .iter()
                .map(|ch| ch.to_uppercase().next().unwrap_or(*ch))
                .collect(),
        );
    }
    map
});

#[derive(Deserialize)]
struct CollocationFile {
    trigram: Vec<Vec<String>>,
    bigram: Vec<Vec<String>>,
}

struct Collocations {
    trigram: HashMap<String, (String, String, String)>,
    bigram: HashMap<String, (String, String)>,
}

static COLLOCATIONS: LazyLock<Collocations> = LazyLock::new(|| {
    let parsed: CollocationFile = serde_json::from_str(include_str!("../assets/collocations.json"))
        .unwrap_or(CollocationFile {
            trigram: Vec::new(),
            bigram: Vec::new(),
        });
    let mut trigram = HashMap::new();
    for row in parsed.trigram {
        if row.len() == 6 {
            trigram.insert(
                pair_key(&row[0], &row[1], Some(&row[2])),
                (row[3].clone(), row[4].clone(), row[5].clone()),
            );
        }
    }
    let mut bigram = HashMap::new();
    for row in parsed.bigram {
        if row.len() == 4 {
            bigram.insert(
                pair_key(&row[0], &row[1], None),
                (row[2].clone(), row[3].clone()),
            );
        }
    }
    let priority_bigrams = [
        ("di", "ngay", "đi", "ngay"),
        ("lam", "ngay", "làm", "ngay"),
        ("ngay", "nhe", "ngay", "nhé"),
        ("xong", "ngay", "xong", "ngay"),
        ("den", "ngay", "đến", "ngay"),
        ("ve", "ngay", "về", "ngay"),
    ];
    for (u1, u2, w1, w2) in priority_bigrams {
        bigram.insert(pair_key(u1, u2, None), (w1.to_string(), w2.to_string()));
    }
    let priority_trigrams = [
        ("di", "ngay", "nhe", "đi", "ngay", "nhé"),
        ("lam", "ngay", "nhe", "làm", "ngay", "nhé"),
        ("anh", "di", "ngay", "anh", "đi", "ngay"),
        ("em", "se", "lam", "em", "sẽ", "làm"),
        ("se", "lam", "ngay", "sẽ", "làm", "ngay"),
        ("nay", "to", "qua", "này", "to", "quá"),
    ];
    for (u1, u2, u3, w1, w2, w3) in priority_trigrams {
        trigram.insert(
            pair_key(u1, u2, Some(u3)),
            (w1.to_string(), w2.to_string(), w3.to_string()),
        );
    }
    Collocations { trigram, bigram }
});

static RE_SENTENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.?!]\s+").unwrap());
static RE_DELIMITER_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[.?!]\s+$").unwrap());
static RE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+|\W").unwrap());
static RE_VN_SYLLABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zàáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđ]+$")
        .unwrap()
});
static RE_UO_DIPHTHONG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ưừứửữự][ôồốổỗộ]").unwrap());
static RE_ILLEGAL_BEFORE_NH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ăằắẳẵặâầấẩẫậôồốổỗộơờớởỡợưừứửữự](nh|ch)$").unwrap());

fn pair_key(first: &str, second: &str, third: Option<&str>) -> String {
    match third {
        Some(third) => format!("{first}\u{1}{second}\u{1}{third}"),
        None => format!("{first}\u{1}{second}"),
    }
}

pub(crate) fn add_accents(text: &str, infer: &dyn Fn(&str) -> Option<Vec<f32>>) -> String {
    if text.trim().is_empty() || is_predominantly_english_sentence(text) {
        return text.to_string();
    }
    if RE_SENTENCE.is_match(text) {
        return split_keeping_delimiters(text)
            .into_iter()
            .map(|part| {
                if RE_DELIMITER_ONLY.is_match(&part) {
                    part
                } else {
                    add_accents_chunk(&part, infer)
                }
            })
            .collect();
    }
    add_accents_chunk(text, infer)
}

fn split_keeping_delimiters(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut last = 0usize;
    for matched in RE_SENTENCE.find_iter(text) {
        if matched.start() > last {
            parts.push(text[last..matched.start()].to_string());
        }
        parts.push(matched.as_str().to_string());
        last = matched.end();
    }
    if last < text.len() {
        parts.push(text[last..].to_string());
    }
    parts
}

fn add_accents_chunk(text: &str, infer: &dyn Fn(&str) -> Option<Vec<f32>>) -> String {
    let text = apply_confusable_prepass(text);
    let protected = protected_indices(&text);
    let chars: Vec<char> = text.chars().collect();
    let limit = chars.len().min(ACCENT_MAX_CHARS);
    let prefix: String = chars.iter().take(limit).collect();
    let Some(logits) = infer(&prefix) else {
        return text;
    };
    if logits.len() < limit.saturating_mul(ACCENT_VOCAB_SIZE) {
        return text;
    }
    let decoded = decode_beam(&chars[..limit], &logits, &protected);
    let mut raw = decoded;
    raw.extend(chars.iter().skip(limit));
    rescore_with_collocations(&raw.iter().collect::<String>(), &text)
}

fn apply_confusable_prepass(text: &str) -> String {
    let mut current = text.to_string();
    for phrase in COMPILED_CONFUSABLES
        .iter()
        .filter(|phrase| phrase.multi_word)
    {
        if !phrase.re.is_match(&current) {
            continue;
        }
        current = phrase
            .re
            .replace_all(&current, |caps: &regex::Captures| {
                title_if_needed(&caps[0], phrase.replacement)
            })
            .to_string();
    }
    current
}

fn title_if_needed(matched: &str, replacement: &str) -> String {
    let mut chars = replacement.chars();
    let Some(first) = chars.next() else {
        return replacement.to_string();
    };
    if matched.chars().next().is_some_and(|ch| ch.is_uppercase()) {
        first.to_uppercase().collect::<String>() + chars.as_str()
    } else {
        replacement.to_string()
    }
}

fn protected_indices(text: &str) -> HashSet<usize> {
    let mut protected = HashSet::new();
    for &(start, end) in &protected_literal_spans(text) {
        if start >= end || end > text.len() {
            continue;
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        let from = text[..start].chars().count();
        let len = text[start..end].chars().count();
        for idx in from..from + len {
            protected.insert(idx);
        }
    }
    let mut char_pos = 0usize;
    let mut alpha_tokens = Vec::new();
    for matched in RE_TOKEN.find_iter(text) {
        let word = matched.as_str();
        let char_len = word.chars().count();
        if word.chars().any(|ch| ch.is_alphabetic()) {
            alpha_tokens.push(word.to_string());
        }
        if is_foreign_or_tech_word(word) {
            for idx in char_pos..char_pos + char_len {
                protected.insert(idx);
            }
        }
        char_pos += char_len;
    }
    let lower = text.to_lowercase();
    for (_, right) in CONFUSABLE_PHRASES.iter() {
        if right.contains(' ') {
            protect_substring(&lower, &right.to_lowercase(), &mut protected);
        }
    }
    if alpha_tokens.len() <= 1
        && alpha_tokens.first().is_some_and(|word| {
            matches!(
                word.to_lowercase().as_str(),
                "toi" | "ban" | "oi" | "thui" | "da" | "bn"
            )
        })
    {
        for idx in 0..text.chars().count() {
            protected.insert(idx);
        }
    }
    protected
}

fn protect_substring(lower: &str, needle: &str, protected: &mut HashSet<usize>) {
    if needle.is_empty() {
        return;
    }
    let hay: Vec<char> = lower.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    let mut start = 0usize;
    while start + needle.len() <= hay.len() {
        if hay[start..start + needle.len()] == needle[..] {
            let end = start + needle.len();
            let before_ok = start == 0 || !hay[start - 1].is_alphanumeric();
            let after_ok = end == hay.len() || !hay[end].is_alphanumeric();
            if before_ok && after_ok {
                for idx in start..end {
                    protected.insert(idx);
                }
            }
            start = end;
        } else {
            start += 1;
        }
    }
}

#[derive(Clone)]
struct Beam {
    score: f32,
    out: Vec<char>,
    curr_word: Vec<char>,
    words: Vec<String>,
}

fn decode_beam(chars: &[char], logits: &[f32], protected: &HashSet<usize>) -> Vec<char> {
    let log_probs = row_log_probs(logits, chars.len());
    let mut beams = vec![Beam {
        score: 0.0,
        out: Vec::new(),
        curr_word: Vec::new(),
        words: Vec::new(),
    }];
    for (index, ch) in chars.iter().copied().enumerate() {
        let base = strip_accents(&ch.to_string()).chars().next().unwrap_or(ch);
        let candidates = candidates_at(chars, index, ch, base, protected);
        let mut next = Vec::new();
        for beam in &beams {
            let has_tone = beam.curr_word.iter().any(|item| tone_of(*item) != 0);
            for cand in &candidates {
                let cand_tone = tone_of(*cand);
                if has_tone && cand_tone != 0 {
                    continue;
                }
                if beam
                    .curr_word
                    .last()
                    .is_some_and(|prev| "ưừứửữự".contains(*prev))
                    && base == 'o'
                    && "ôồốổỗộ".contains(*cand)
                {
                    continue;
                }
                let mut score = beam.score + log_prob_at(&log_probs, index, *cand);
                let mut out = beam.out.clone();
                out.push(*cand);
                let (curr_word, words) = if cand.is_alphabetic() {
                    let mut curr_word = beam.curr_word.clone();
                    curr_word.push(*cand);
                    (curr_word, beam.words.clone())
                } else if beam.curr_word.is_empty() {
                    (Vec::new(), beam.words.clone())
                } else {
                    let word: String = beam.curr_word.iter().collect();
                    score += syllable_score(&word);
                    let mut words = beam.words.clone();
                    words.push(word);
                    score += collocation_bonus(&words);
                    (Vec::new(), words)
                };
                next.push(Beam {
                    score,
                    out,
                    curr_word,
                    words,
                });
            }
        }
        if next.is_empty() {
            for beam in &beams {
                let mut out = beam.out.clone();
                out.push(ch);
                let mut curr_word = beam.curr_word.clone();
                if ch.is_alphabetic() {
                    curr_word.push(ch);
                }
                next.push(Beam {
                    score: beam.score,
                    out,
                    curr_word,
                    words: beam.words.clone(),
                });
            }
        }
        next.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        next.truncate(BEAM_WIDTH);
        beams = next;
    }
    let mut finished = Vec::new();
    for mut beam in beams {
        if !beam.curr_word.is_empty() {
            let word: String = beam.curr_word.iter().collect();
            beam.score += syllable_score(&word);
            beam.words.push(word);
            beam.score += collocation_bonus(&beam.words);
        }
        finished.push(beam);
    }
    finished.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    finished
        .into_iter()
        .next()
        .map(|beam| beam.out)
        .unwrap_or_else(|| chars.to_vec())
}

fn candidates_at(
    chars: &[char],
    index: usize,
    ch: char,
    base: char,
    protected: &HashSet<usize>,
) -> Vec<char> {
    if protected.contains(&index) || ch != base {
        return vec![ch];
    }
    let Some(variants) = VOWEL_VARIANTS.get(&base) else {
        return vec![ch];
    };
    let mut cand = variants.clone();
    let following: String = chars
        .iter()
        .skip(index + 1)
        .take(2)
        .collect::<String>()
        .to_lowercase();
    if base == 'a' && (following.starts_with("nh") || following.starts_with("ch")) {
        cand.retain(|item| !"ăằắẳẵặâầấẩẫậ".contains(*item));
    }
    if base == 'o' && (following.starts_with("nh") || following.starts_with("ch")) {
        cand.retain(|item| !"ôồốổỗộơờớởỡợ".contains(*item));
    }
    if base == 'u' && (following.starts_with("nh") || following.starts_with("ch")) {
        cand.retain(|item| !"ưừứửữự".contains(*item));
    }
    let mut end = index + 1;
    while end < chars.len() && chars[end].is_alphabetic() {
        end += 1;
    }
    let remainder: String = chars[index + 1..end]
        .iter()
        .collect::<String>()
        .to_lowercase();
    let has_stop = ["ch", "c", "p", "t"]
        .iter()
        .any(|coda| remainder.ends_with(coda));
    if has_stop {
        let has_following_vowel = remainder.chars().any(is_tone_vowel);
        let before = cand.len();
        if has_following_vowel {
            cand.retain(|item| tone_of(*item) == 0);
        } else {
            cand.retain(|item| matches!(tone_of(*item), 2 | 5));
        }
        if cand.is_empty() && before > 0 {
            cand = variants.clone();
        }
    }
    if cand.is_empty() { vec![ch] } else { cand }
}

fn tone_of(ch: char) -> u8 {
    CHAR_TO_TONE.get(&ch).map(|(_, tone)| *tone).unwrap_or(0)
}

fn is_tone_vowel(ch: char) -> bool {
    "aeiouyàáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵ".contains(ch)
}

fn row_log_probs(logits: &[f32], rows: usize) -> Vec<f32> {
    let mut out = vec![0.0; rows * ACCENT_VOCAB_SIZE];
    for row in 0..rows {
        let start = row * ACCENT_VOCAB_SIZE;
        let end = (start + ACCENT_VOCAB_SIZE).min(logits.len());
        let slice = &logits[start..end];
        let max = slice.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let sum: f32 = slice.iter().map(|value| (value - max).exp()).sum();
        let log_z = max + sum.max(f32::MIN_POSITIVE).ln();
        for (col, value) in slice.iter().enumerate() {
            out[start + col] = value - log_z;
        }
    }
    out
}

fn log_prob_at(log_probs: &[f32], index: usize, ch: char) -> f32 {
    let id = accent_char_id(ch);
    log_probs
        .get(index * ACCENT_VOCAB_SIZE + id)
        .copied()
        .unwrap_or(f32::NEG_INFINITY)
}

fn syllable_score(word: &str) -> f32 {
    if is_valid_vietnamese_syllable(word) {
        1.0
    } else {
        -5.0
    }
}

fn collocation_bonus(words: &[String]) -> f32 {
    let mut bonus = 0.0;
    if words.len() >= 2 {
        let w1 = &words[words.len() - 2];
        let w2 = &words[words.len() - 1];
        let u1 = strip_accents(w1).to_lowercase();
        let u2 = strip_accents(w2).to_lowercase();
        if let Some((c1, c2)) = COLLOCATIONS.bigram.get(&pair_key(&u1, &u2, None)) {
            if w1.to_lowercase() == c1.to_lowercase() && w2.to_lowercase() == c2.to_lowercase() {
                bonus += 2.5;
            }
        }
    }
    if words.len() >= 3 {
        let w1 = &words[words.len() - 3];
        let w2 = &words[words.len() - 2];
        let w3 = &words[words.len() - 1];
        let u1 = strip_accents(w1).to_lowercase();
        let u2 = strip_accents(w2).to_lowercase();
        let u3 = strip_accents(w3).to_lowercase();
        if let Some((c1, c2, c3)) = COLLOCATIONS.trigram.get(&pair_key(&u1, &u2, Some(&u3))) {
            if w1.to_lowercase() == c1.to_lowercase()
                && w2.to_lowercase() == c2.to_lowercase()
                && w3.to_lowercase() == c3.to_lowercase()
            {
                bonus += 5.0;
            }
        }
    }
    bonus
}

fn is_valid_vietnamese_syllable(syllable: &str) -> bool {
    let syllable = syllable.trim().to_lowercase();
    if syllable.is_empty() {
        return true;
    }
    if !RE_VN_SYLLABLE.is_match(&syllable) {
        return false;
    }
    if RE_UO_DIPHTHONG.is_match(&syllable) || RE_ILLEGAL_BEFORE_NH.is_match(&syllable) {
        return false;
    }
    let tones = syllable.chars().filter(|ch| tone_of(*ch) != 0).count();
    if tones > 1 {
        return false;
    }
    let (base, tone) = extract_tone(&syllable);
    if ["ch", "c", "p", "t"]
        .iter()
        .any(|coda| base.ends_with(coda))
        && !matches!(tone, 2 | 5)
    {
        return false;
    }
    true
}

fn extract_tone(word: &str) -> (String, u8) {
    let mut tone = 0u8;
    let mut chars = String::new();
    for ch in word.chars() {
        if let Some((base, found)) = CHAR_TO_TONE.get(&ch) {
            if *found > 0 {
                tone = *found;
            }
            chars.push(*base);
        } else {
            chars.push(ch);
        }
    }
    (chars, tone)
}

fn rescore_with_collocations(text: &str, original: &str) -> String {
    let mut tokens = tokenize(text);
    let original_tokens = tokenize(original);
    let mut unaccented = vec![true; tokens.len()];
    if original_tokens.len() == tokens.len() {
        for index in 0..tokens.len() {
            let source = &original_tokens[index];
            unaccented[index] = source.to_lowercase() == strip_accents(source).to_lowercase();
        }
    }
    let mut matched = HashSet::new();
    let alpha: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.chars().any(|ch| ch.is_alphabetic()))
        .map(|(index, _)| index)
        .collect();
    if alpha.len() >= 3 {
        for index in 0..alpha.len() - 2 {
            let (i0, i1, i2) = (alpha[index], alpha[index + 1], alpha[index + 2]);
            let u0 = strip_accents(&tokens[i0]).to_lowercase();
            let u1 = strip_accents(&tokens[i1]).to_lowercase();
            let u2 = strip_accents(&tokens[i2]).to_lowercase();
            if let Some((c0, c1, c2)) = COLLOCATIONS.trigram.get(&pair_key(&u0, &u1, Some(&u2))) {
                if unaccented[i0] {
                    tokens[i0] = match_case(&tokens[i0], c0);
                }
                if unaccented[i1] {
                    tokens[i1] = match_case(&tokens[i1], c1);
                }
                if unaccented[i2] {
                    tokens[i2] = match_case(&tokens[i2], c2);
                }
                matched.insert(i0);
                matched.insert(i1);
                matched.insert(i2);
            }
        }
    }
    if alpha.len() >= 2 {
        for index in 0..alpha.len() - 1 {
            let (i0, i1) = (alpha[index], alpha[index + 1]);
            if matched.contains(&i0) && matched.contains(&i1) {
                continue;
            }
            let u0 = strip_accents(&tokens[i0]).to_lowercase();
            let u1 = strip_accents(&tokens[i1]).to_lowercase();
            if let Some((c0, c1)) = COLLOCATIONS.bigram.get(&pair_key(&u0, &u1, None)) {
                if !matched.contains(&i0) && unaccented[i0] {
                    tokens[i0] = match_case(&tokens[i0], c0);
                    matched.insert(i0);
                }
                if !matched.contains(&i1) && unaccented[i1] {
                    tokens[i1] = match_case(&tokens[i1], c1);
                    matched.insert(i1);
                }
            }
        }
    }
    apply_confusable_prepass(&tokens.concat())
}

fn tokenize(text: &str) -> Vec<String> {
    RE_TOKEN
        .find_iter(text)
        .map(|matched| matched.as_str().to_string())
        .collect()
}
