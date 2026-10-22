use crate::rules::CorrectionEdit;
use regex::Regex;

struct PatternRule {
    pattern: &'static str,
    replacement: &'static str,
    reason: &'static str,
}

const REDUNDANT: &[PatternRule] = &[
    PatternRule {
        pattern: r"(?i)\b(rất)\s+(là\s+rất)\b",
        replacement: "$1",
        reason: "Lặp từ thừa ('rất là rất' -> 'rất')",
    },
    PatternRule {
        pattern: r"(?i)\b(quá)\s+(là\s+quá)\b",
        replacement: "$1",
        reason: "Lặp từ thừa ('quá là quá' -> 'quá')",
    },
    PatternRule {
        pattern: r"(?i)\b(cực\s+kỳ)\s+(là\s+cực\s+kỳ)\b",
        replacement: "$1",
        reason: "Lặp từ thừa",
    },
    PatternRule {
        pattern: r"(?i)\b(nhưng\s+mà)\s+(nhưng\s+mà)\b",
        replacement: "nhưng mà",
        reason: "Lặp liên từ",
    },
    PatternRule {
        pattern: r"(?i)\b(thì)\s+(thì)\b",
        replacement: "thì",
        reason: "Lặp hư từ ('thì thì' -> 'thì')",
    },
    PatternRule {
        pattern: r"(?i)\b(là)\s+(là)\b",
        replacement: "là",
        reason: "Lặp hư từ ('là là' -> 'là')",
    },
    PatternRule {
        pattern: r"(?i)\b(mà)\s+(mà)\b",
        replacement: "mà",
        reason: "Lặp hư từ ('mà mà' -> 'mà')",
    },
    PatternRule {
        pattern: r"(?i)\b(và)\s+(và)\b",
        replacement: "và",
        reason: "Lặp liên từ ('và và' -> 'và')",
    },
    PatternRule {
        pattern: r"(?i)\b(hoặc)\s+(hoặc)\b",
        replacement: "hoặc",
        reason: "Lặp liên từ",
    },
];

const IMPROPER_CLASSIFIERS: &[PatternRule] = &[
    PatternRule {
        pattern: r"(?i)\bcái\s+người\b",
        replacement: "người",
        reason: "Dùng sai loại từ ('cái người' -> 'người')",
    },
    PatternRule {
        pattern: r"(?i)\bcon\s+cái\s+bàn\b",
        replacement: "cái bàn",
        reason: "Dùng sai loại từ ('con cái bàn' -> 'cái bàn')",
    },
    PatternRule {
        pattern: r"(?i)\bcon\s+cái\s+ghế\b",
        replacement: "cái ghế",
        reason: "Dùng sai loại từ ('con cái ghế' -> 'cái ghế')",
    },
];

fn apply_rules(text: &str, rules: &[PatternRule]) -> (String, Vec<CorrectionEdit>) {
    let mut current = text.to_string();
    let mut edits = Vec::new();
    for rule in rules {
        let re = Regex::new(rule.pattern).expect("grammar pattern");
        if let Some(m) = re.find(&current) {
            let original = m.as_str().to_string();
            current = re.replace_all(&current, rule.replacement).to_string();
            edits.push(CorrectionEdit {
                original,
                replacement: rule.replacement.to_string(),
                reason: rule.reason.to_string(),
            });
        }
    }
    (current, edits)
}

fn collapse_triple_word_runs(text: &str) -> (String, Vec<CorrectionEdit>) {
    let re = Regex::new(r"\b\p{L}+\b").expect("word");
    let words: Vec<(usize, usize)> = re.find_iter(text).map(|m| (m.start(), m.end())).collect();
    let mut drop_spans: Vec<(usize, usize, String)> = Vec::new();
    let mut index = 0usize;
    while index < words.len() {
        let (start, end) = words[index];
        let key = text[start..end].to_lowercase();
        let mut last = index;
        while last + 1 < words.len() {
            let (next_start, next_end) = words[last + 1];
            let between = &text[words[last].1..next_start];
            if !between.chars().all(|c| c.is_whitespace()) {
                break;
            }
            if text[next_start..next_end].to_lowercase() != key {
                break;
            }
            last += 1;
        }
        let run = last - index + 1;
        if run >= 3 {
            let original = text[start..words[last].1].to_string();
            drop_spans.push((start, words[last].1, original));
        }
        index = last + 1;
    }
    if drop_spans.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut edits = Vec::new();
    let mut current = text.to_string();
    for (start, end, original) in drop_spans.into_iter().rev() {
        let kept = text[start..end]
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        current.replace_range(start..end, &kept);
        edits.push(CorrectionEdit {
            original,
            replacement: kept,
            reason: "Lặp từ ba lần trở lên".to_string(),
        });
    }
    edits.reverse();
    (current, edits)
}

pub fn check_grammar_syntax(text: &str) -> (String, Vec<CorrectionEdit>) {
    let (current, edits_repeat) = collapse_triple_word_runs(text);
    let (current, edits_redundant) = apply_rules(&current, REDUNDANT);
    let (current, edits_classifiers) = apply_rules(&current, IMPROPER_CLASSIFIERS);
    let mut edits = edits_repeat;
    edits.extend(edits_redundant);
    edits.extend(edits_classifiers);
    (current, edits)
}
