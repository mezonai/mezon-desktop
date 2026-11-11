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
    let parsed: CollocationFile = crate::assets::load_asset_str(crate::assets::COLLOCATIONS_FILE)
        .and_then(|json| serde_json::from_str(&json).ok())
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
        ("mili", "giay", "mili", "giây"),
        ("micro", "giay", "micro", "giây"),
        ("nano", "giay", "nano", "giây"),
        ("mili", "met", "mili", "mét"),
        ("kilo", "met", "kilo", "mét"),
        ("kilo", "gam", "kilo", "gam"),
    ];
    for (u1, u2, w1, w2) in priority_bigrams {
        bigram.insert(pair_key(u1, u2, None), (w1.to_string(), w2.to_string()));
    }
    for &(u1, u2) in BAD_COLLOCATIONS.iter() {
        bigram.remove(&pair_key(u1, u2, None));
    }
    for &(u1, u2, u3) in BAD_TRIGRAMS.iter() {
        trigram.remove(&pair_key(u1, u2, Some(u3)));
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

static BAD_COLLOCATIONS: LazyLock<HashSet<(&'static str, &'static str)>> = LazyLock::new(|| {
    [
        ("lon", "tao"),
        ("viec", "bai"),
        ("kem", "phan"),
        ("san", "ban"),
        ("da", "tay"),
        ("thay", "da"),
        ("vai", "kieu"),
        ("dip", "duoc"),
        ("neu", "tai"),
        ("de", "dung"),
        ("rat", "de"),
        ("mot", "nhan"),
        ("bom", "tu"),
        ("sang", "hoc"),
        ("trong", "thanh"),
        ("nhat", "va"),
        ("tai", "le"),
        ("gay", "mui"),
        ("sau", "tai"),
        ("sam", "mau"),
        ("may", "ban"),
        ("thu", "bay"),
        ("quyen", "cung"),
        ("co", "tram"),
        ("truong", "sitc"),
        ("ban", "nghe"),
        ("nhan", "dinh"),
        ("dang", "tai"),
        ("vai", "lan"),
        ("vai", "nguoi"),
        ("buoc", "tren"),
        ("tren", "san"),
        ("vao", "nam"),
        ("chi", "nguyen"),
        ("di", "thi"),
        ("nhan", "hieu"),
        ("dung", "day"),
        ("vang", "toi"),
        ("co", "khoi"),
    ]
    .into_iter()
    .collect()
});

static BAD_TRIGRAMS: LazyLock<HashSet<(&'static str, &'static str, &'static str)>> =
    LazyLock::new(|| [("gian", "vi", "co")].into_iter().collect());

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
    let precleaned = crate::rules::preclean_phonetics_and_units(text);
    if RE_SENTENCE.is_match(&precleaned) {
        return split_keeping_delimiters(&precleaned)
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
    add_accents_chunk(&precleaned, infer)
}

pub fn restore_sentence(
    text: &str,
    capitalize: bool,
    resolve_teencode: bool,
    infer: &dyn Fn(&str) -> Option<Vec<f32>>,
) -> String {
    if text.trim().is_empty() || is_predominantly_english_sentence(text) {
        return text.to_string();
    }

    // 1. Punctuation spacing normalization
    let (punc_norm, _) = crate::rules::normalize_punctuation_spacing(text);
    let mut processed = punc_norm;

    // 2. Pre-clean units & phonetics
    processed = crate::rules::preclean_phonetics_and_units(&processed);

    // 3. Teencode expansion if requested
    if resolve_teencode {
        let (expanded, _) = crate::rules::normalize_teencode(&processed);
        processed = expanded;
    }

    // 4. Multi-word phrase pre-pass
    processed = apply_confusable_prepass(&processed);

    // 5. Neural accent restoration
    let restored = add_accents(&processed, infer);

    // 6. Sentence-start capitalization recovery
    if capitalize {
        let (cap, _) = crate::rules::normalize_sentence_capitalization(&restored);
        cap
    } else {
        restored
    }
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
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > ACCENT_MAX_CHARS {
        static RE_CLAUSE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"([,;]\s+|\s+-\s+)").unwrap());
        if RE_CLAUSE.is_match(&text) {
            let mut subparts = Vec::new();
            let mut last = 0usize;
            for matched in RE_CLAUSE.find_iter(&text) {
                if matched.start() > last {
                    subparts.push(&text[last..matched.start()]);
                }
                subparts.push(matched.as_str());
                last = matched.end();
            }
            if last < text.len() {
                subparts.push(&text[last..]);
            }
            let mut sub_res = Vec::new();
            let mut curr_buf = String::new();
            for sp in subparts {
                if curr_buf.chars().count() + sp.chars().count() > ACCENT_MAX_CHARS
                    && !curr_buf.is_empty()
                {
                    sub_res.push(add_accents_chunk(&curr_buf, infer));
                    curr_buf = sp.to_string();
                } else {
                    curr_buf.push_str(sp);
                }
            }
            if !curr_buf.is_empty() {
                sub_res.push(add_accents_chunk(&curr_buf, infer));
            }
            return rescore_with_collocations(&sub_res.concat(), &text);
        } else {
            let words: Vec<&str> = text.split(' ').collect();
            let mut chunks = Vec::new();
            let mut curr_chunk: Vec<&str> = Vec::new();
            let mut curr_len = 0usize;
            for w in words {
                let w_len = w.chars().count();
                if curr_len + w_len + 1 > (ACCENT_MAX_CHARS - 15) && !curr_chunk.is_empty() {
                    chunks.push(curr_chunk.join(" "));
                    curr_chunk = vec![w];
                    curr_len = w_len;
                } else {
                    curr_chunk.push(w);
                    curr_len += w_len + 1;
                }
            }
            if !curr_chunk.is_empty() {
                chunks.push(curr_chunk.join(" "));
            }
            if chunks.len() > 1 {
                let res_chunks: Vec<String> =
                    chunks.iter().map(|c| add_accents_chunk(c, infer)).collect();
                return rescore_with_collocations(&res_chunks.join(" "), &text);
            } else {
                // Single huge word with no spaces or clauses; chunk strictly by char length
                let mut char_chunks = Vec::new();
                for chunk in chars.chunks(ACCENT_MAX_CHARS) {
                    let s: String = chunk.iter().collect();
                    char_chunks.push(add_accents_single_chunk(&s, infer));
                }
                return rescore_with_collocations(&char_chunks.concat(), &text);
            }
        }
    }
    add_accents_single_chunk(&text, infer)
}

fn add_accents_single_chunk(text: &str, infer: &dyn Fn(&str) -> Option<Vec<f32>>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let protected = protected_indices(text);
    let limit = chars.len().min(ACCENT_MAX_CHARS);
    let prefix: String = chars.iter().take(limit).collect();
    let Some(logits) = infer(&prefix) else {
        return text.to_string();
    };
    if logits.len() < limit.saturating_mul(ACCENT_VOCAB_SIZE) {
        return text.to_string();
    }
    let decoded = decode_beam(&chars[..limit], &logits, &protected);
    let mut raw = decoded;
    raw.extend(chars.iter().skip(limit));
    rescore_with_collocations(&raw.iter().collect::<String>(), text)
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
    if crate::syllable::is_valid_vietnamese_syllable(&syllable) {
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

fn has_punct_between(tokens: &[String], start_idx: usize, end_idx: usize) -> bool {
    const PUNCT: &[char] = &[
        ',', '.', '?', '!', ':', ';', '(', ')', '[', ']', '{', '}', '"', '\'', '-', '/', '\\', '–',
        '—',
    ];
    for token in &tokens[start_idx + 1..end_idx] {
        if token.chars().any(|c| PUNCT.contains(&c)) {
            return true;
        }
    }
    false
}

fn rescore_with_collocations(text: &str, original: &str) -> String {
    let mut tokens = tokenize(text);
    let original_tokens = tokenize(original);
    let alpha_indices: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.chars().any(|ch| ch.is_alphabetic()))
        .map(|(index, _)| index)
        .collect();

    if alpha_indices.is_empty() {
        return text.to_string();
    }

    let mut unaccented_mask = vec![true; tokens.len()];
    if original_tokens.len() == tokens.len() {
        for index in 0..tokens.len() {
            let source = &original_tokens[index];
            unaccented_mask[index] = source.to_lowercase() == strip_accents(source).to_lowercase();
        }
    }

    let k_len = alpha_indices.len();
    let mut candidates_per_pos: Vec<Vec<String>> = Vec::with_capacity(k_len);

    for k in 0..k_len {
        let idx = alpha_indices[k];
        let tok = &tokens[idx];
        let u = strip_accents(tok).to_lowercase();
        let mut cands = vec![tok.clone()];

        if unaccented_mask[idx] {
            // Left bigram suggestion
            if k > 0 && !has_punct_between(&tokens, alpha_indices[k - 1], idx) {
                let u_prev = strip_accents(&tokens[alpha_indices[k - 1]]).to_lowercase();
                if !BAD_COLLOCATIONS.contains(&(u_prev.as_str(), u.as_str())) {
                    if let Some((_, c1)) = COLLOCATIONS.bigram.get(&pair_key(&u_prev, &u, None)) {
                        cands.push(match_case(tok, c1));
                    }
                }
            }

            // Right bigram suggestion
            if k + 1 < k_len && !has_punct_between(&tokens, idx, alpha_indices[k + 1]) {
                let u_next = strip_accents(&tokens[alpha_indices[k + 1]]).to_lowercase();
                if !BAD_COLLOCATIONS.contains(&(u.as_str(), u_next.as_str())) {
                    if let Some((c0, _)) = COLLOCATIONS.bigram.get(&pair_key(&u, &u_next, None)) {
                        cands.push(match_case(tok, c0));
                    }
                }
            }

            // Trigram suggestions
            if k >= 2 && !has_punct_between(&tokens, alpha_indices[k - 2], idx) {
                let u_pp = strip_accents(&tokens[alpha_indices[k - 2]]).to_lowercase();
                let u_p = strip_accents(&tokens[alpha_indices[k - 1]]).to_lowercase();
                if let Some((_, _, c2)) = COLLOCATIONS.trigram.get(&pair_key(&u_pp, &u_p, Some(&u)))
                {
                    cands.push(match_case(tok, c2));
                }
            }

            if k >= 1
                && k + 1 < k_len
                && !has_punct_between(&tokens, alpha_indices[k - 1], alpha_indices[k + 1])
            {
                let u_p = strip_accents(&tokens[alpha_indices[k - 1]]).to_lowercase();
                let u_n = strip_accents(&tokens[alpha_indices[k + 1]]).to_lowercase();
                if let Some((_, c1, _)) = COLLOCATIONS.trigram.get(&pair_key(&u_p, &u, Some(&u_n)))
                {
                    cands.push(match_case(tok, c1));
                }
            }

            if k + 2 < k_len && !has_punct_between(&tokens, idx, alpha_indices[k + 2]) {
                let u_n = strip_accents(&tokens[alpha_indices[k + 1]]).to_lowercase();
                let u_nn = strip_accents(&tokens[alpha_indices[k + 2]]).to_lowercase();
                if let Some((c0, _, _)) = COLLOCATIONS
                    .trigram
                    .get(&pair_key(&u_n, &u_nn, None))
                    .or_else(|| COLLOCATIONS.trigram.get(&pair_key(&u, &u_n, Some(&u_nn))))
                {
                    cands.push(match_case(tok, c0));
                }
            }
        }

        // Deduplicate candidates preserving order (Top-K per position, up to 6)
        let mut unique_cands = Vec::new();
        let mut seen = HashSet::new();
        for c in cands {
            let cl = c.to_lowercase();
            if seen.insert(cl) {
                unique_cands.push(c);
                if unique_cands.len() >= 6 {
                    break;
                }
            }
        }
        candidates_per_pos.push(unique_cands);
    }

    let emission = |k: usize, c: &str| -> f32 {
        let idx = alpha_indices[k];
        let tok = &tokens[idx];
        let mut score = 0.0f32;
        if c == tok {
            score += 2.5;
        }
        if !unaccented_mask[idx] {
            score += if c == tok { 100.0 } else { -100.0 };
        }
        score
    };

    let trans2 = |k: usize, c_prev: &str, c_curr: &str| -> f32 {
        let idx_prev = alpha_indices[k - 1];
        let idx_curr = alpha_indices[k];
        if has_punct_between(&tokens, idx_prev, idx_curr) {
            return 0.0;
        }
        let u_prev = strip_accents(c_prev).to_lowercase();
        let u_curr = strip_accents(c_curr).to_lowercase();
        if BAD_COLLOCATIONS.contains(&(u_prev.as_str(), u_curr.as_str())) {
            return -15.0;
        }
        if let Some((t_prev, t_curr)) = COLLOCATIONS.bigram.get(&pair_key(&u_prev, &u_curr, None)) {
            if c_prev.eq_ignore_ascii_case(t_prev) && c_curr.eq_ignore_ascii_case(t_curr) {
                return 6.0;
            } else {
                return -3.0;
            }
        }
        0.0
    };

    let trans3 = |k: usize, c_pprev: &str, c_prev: &str, c_curr: &str| -> f32 {
        let idx_pprev = alpha_indices[k - 2];
        let idx_curr = alpha_indices[k];
        if has_punct_between(&tokens, idx_pprev, idx_curr) {
            return 0.0;
        }
        let u_pprev = strip_accents(c_pprev).to_lowercase();
        let u_prev = strip_accents(c_prev).to_lowercase();
        let u_curr = strip_accents(c_curr).to_lowercase();
        if BAD_TRIGRAMS.contains(&(u_pprev.as_str(), u_prev.as_str(), u_curr.as_str())) {
            return -15.0;
        }
        if let Some((t_pprev, t_prev, t_curr)) =
            COLLOCATIONS
                .trigram
                .get(&pair_key(&u_pprev, &u_prev, Some(&u_curr)))
        {
            if c_pprev.eq_ignore_ascii_case(t_pprev)
                && c_prev.eq_ignore_ascii_case(t_prev)
                && c_curr.eq_ignore_ascii_case(t_curr)
            {
                return 10.0;
            } else {
                return -4.0;
            }
        }
        0.0
    };

    if k_len == 1 {
        let mut best_c = &candidates_per_pos[0][0];
        let mut best_score = -f32::INFINITY;
        for c in &candidates_per_pos[0] {
            let sc = emission(0, c);
            if sc > best_score {
                best_score = sc;
                best_c = c;
            }
        }
        tokens[alpha_indices[0]] = best_c.clone();
        let rescored = apply_confusable_prepass(&tokens.concat());
        return crate::rules::preclean_phonetics_and_units(&rescored);
    }

    // 2nd-Order Viterbi Dynamic Programming
    let mut v: Vec<HashMap<(usize, usize), f32>> = vec![HashMap::new(); k_len];
    let mut bp: Vec<HashMap<(usize, usize), usize>> = vec![HashMap::new(); k_len];

    // Step k = 0
    for (c_idx, c) in candidates_per_pos[0].iter().enumerate() {
        v[0].insert((0, c_idx), emission(0, c));
    }

    // Step k = 1
    for (p_idx, p) in candidates_per_pos[0].iter().enumerate() {
        let prev_v = v[0].get(&(0, p_idx)).copied().unwrap_or(0.0);
        for (c_idx, c) in candidates_per_pos[1].iter().enumerate() {
            let score = prev_v + trans2(1, p, c) + emission(1, c);
            v[1].insert((p_idx, c_idx), score);
        }
    }

    // Step k >= 2
    for k in 2..k_len {
        for (p_idx, p) in candidates_per_pos[k - 1].iter().enumerate() {
            for (c_idx, c) in candidates_per_pos[k].iter().enumerate() {
                let mut best_score = -f32::INFINITY;
                let mut best_pp = None;
                let t2 = trans2(k, p, c);
                let em = emission(k, c);

                for (pp_idx, pp) in candidates_per_pos[k - 2].iter().enumerate() {
                    if let Some(&prev_sc) = v[k - 1].get(&(pp_idx, p_idx)) {
                        let sc = prev_sc + t2 + trans3(k, pp, p, c) + em;
                        if sc > best_score {
                            best_score = sc;
                            best_pp = Some(pp_idx);
                        }
                    }
                }
                if let Some(pp_idx) = best_pp {
                    v[k].insert((p_idx, c_idx), best_score);
                    bp[k].insert((p_idx, c_idx), pp_idx);
                }
            }
        }
    }

    // Traceback to find globally optimal sequence
    let mut best_pair = (0, 0);
    let mut max_total_score = -f32::INFINITY;
    for (&(p_idx, c_idx), &score) in &v[k_len - 1] {
        if score > max_total_score {
            max_total_score = score;
            best_pair = (p_idx, c_idx);
        }
    }

    let mut result_indices = vec![0usize; k_len];
    result_indices[k_len - 1] = best_pair.1;
    result_indices[k_len - 2] = best_pair.0;

    for k in (2..k_len).rev() {
        let pair = (result_indices[k - 1], result_indices[k]);
        let pp_idx = bp[k].get(&pair).copied().unwrap_or(0);
        result_indices[k - 2] = pp_idx;
    }

    for k in 0..k_len {
        tokens[alpha_indices[k]] = candidates_per_pos[k][result_indices[k]].clone();
    }

    let rescored = apply_confusable_prepass(&tokens.concat());
    crate::rules::preclean_phonetics_and_units(&rescored)
}

fn tokenize(text: &str) -> Vec<String> {
    RE_TOKEN
        .find_iter(text)
        .map(|matched| matched.as_str().to_string())
        .collect()
}
