use crate::grammar_checker::check_grammar_syntax;
use crate::neural::apply_neural_accent;
use crate::onnx::quick_replies_with_intent;
use crate::pronoun::{extract_target_pronoun, suggest_quick_replies};
use crate::rules::{
    COMPILED_CONFUSABLES, CorrectionEdit, CorrectionResult, TEENCODE_MAP, is_foreign_or_tech_word,
    is_predominantly_english_sentence, mask_protected_literals, match_case,
    normalize_punctuation_spacing, normalize_sentence_capitalization, normalize_tone_placement,
    restore_diacritics, unmask_protected_literals,
};
use crate::tokenizer::VietnameseTokenizer;
use crate::trie::{WordSuggestion, WordTrie};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub const MAX_CORRECTION_CHAR_COUNT: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeystrokeEvent {
    pub input: String,
    pub correction: CorrectionResult,
    pub suggestions: Vec<WordSuggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronoun: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_replies: Option<Vec<String>>,
}

pub struct MezonGrammarClient {
    pub trie: WordTrie,
    pub tokenizer: VietnameseTokenizer,
}

impl Default for MezonGrammarClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MezonGrammarClient {
    pub fn new() -> Self {
        Self {
            trie: WordTrie::new(),
            tokenizer: VietnameseTokenizer::new(),
        }
    }

    pub fn shared() -> &'static Self {
        static CLIENT: LazyLock<MezonGrammarClient> = LazyLock::new(MezonGrammarClient::new);
        &CLIENT
    }

    pub fn correct_within_limit(&self, text: &str) -> Option<CorrectionResult> {
        if text.chars().count() > MAX_CORRECTION_CHAR_COUNT {
            return None;
        }
        Some(self.correct(text))
    }

    pub fn correct(&self, text: &str) -> CorrectionResult {
        if text.trim().is_empty() {
            return CorrectionResult {
                original: text.to_string(),
                corrected: text.to_string(),
                has_errors: false,
                edits: Vec::new(),
            };
        }

        let original_input = text;
        let (masked, literals) = mask_protected_literals(text);
        let mut edits = Vec::new();

        let (mut current, edits_punc) = normalize_punctuation_spacing(&masked);
        edits.extend(edits_punc);

        if is_predominantly_english_sentence(text) {
            let (cap, edits_cap) = normalize_sentence_capitalization(&current);
            edits.extend(edits_cap);
            let trimmed = unmask_protected_literals(cap.trim(), &literals);
            let has_errors = trimmed != original_input.trim();
            return CorrectionResult {
                original: original_input.to_string(),
                corrected: trimmed,
                has_errors,
                edits,
            };
        }

        for phrase in COMPILED_CONFUSABLES.iter() {
            if phrase.re.is_match(&current) {
                let replacement = phrase.replacement;
                current = phrase
                    .re
                    .replace_all(&current, |caps: &regex::Captures| {
                        let matched = &caps[0];
                        let rep = match_case(matched, replacement);
                        edits.push(CorrectionEdit {
                            original: matched.to_string(),
                            replacement: rep.clone(),
                            reason: "Sửa chính tả / từ dễ lẫn".to_string(),
                        });
                        rep
                    })
                    .to_string();
            }
        }

        static RE_WORD: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\b[\p{L}\p{N}]+\b").unwrap());
        current = RE_WORD
            .replace_all(&current, |caps: &regex::Captures| {
                let word = &caps[0];
                if is_foreign_or_tech_word(word) {
                    return word.to_string();
                }
                let lower = word.to_lowercase();
                if let Some(&rep_base) = TEENCODE_MAP.get(lower.as_str()) {
                    let rep = match_case(word, rep_base);
                    edits.push(CorrectionEdit {
                        original: word.to_string(),
                        replacement: rep.clone(),
                        reason: "Thay thế từ viết tắt / teencode".to_string(),
                    });
                    return rep;
                }
                word.to_string()
            })
            .to_string();

        let (restored, edits_diacritics) = restore_diacritics(&current);
        edits.extend(edits_diacritics);
        current = restored;

        let (toned, edits_tone) = normalize_tone_placement(&current);
        edits.extend(edits_tone);
        current = toned;

        let (grammar_fixed, edits_grammar) = check_grammar_syntax(&current);
        edits.extend(edits_grammar);
        current = grammar_fixed;

        let before_neural = current.clone();
        current = apply_neural_accent(&current);
        if current != before_neural {
            edits.push(CorrectionEdit {
                original: before_neural,
                replacement: current.clone(),
                reason: "Khôi phục dấu thanh bằng mô hình học máy (Neural Accent Model)"
                    .to_string(),
            });
        }

        let (capitalized, edits_cap) = normalize_sentence_capitalization(&current);
        edits.extend(edits_cap);
        current = capitalized;

        let re_spaces = Regex::new(r"[ \t]+").unwrap();
        current = re_spaces.replace_all(current.trim(), " ").to_string();
        current = unmask_protected_literals(&current, &literals);

        let has_errors = current != original_input.trim();

        CorrectionResult {
            original: original_input.to_string(),
            corrected: current,
            has_errors,
            edits,
        }
    }

    pub fn suggest(&self, text: &str, top_k: usize) -> Vec<WordSuggestion> {
        let trimmed = text.trim();
        let ends_with_space = text.ends_with(' ') || text.ends_with('\n') || text.ends_with('\t');
        let ends_with_terminal_punct =
            trimmed.ends_with('.') || trimmed.ends_with('?') || trimmed.ends_with('!');

        if ends_with_terminal_punct && !ends_with_space {
            return Vec::new();
        }

        let re_words = Regex::new(r"\b\p{L}+\b").unwrap();
        let words: Vec<&str> = re_words.find_iter(text).map(|m| m.as_str()).collect();

        let ends_with_letter = text.chars().last().is_some_and(|c| c.is_alphabetic());
        if ends_with_letter && !words.is_empty() {
            let last_word = words[words.len() - 1];
            let completions = self.trie.complete_prefix(last_word, top_k + 2);
            let filtered: Vec<WordSuggestion> = completions
                .into_iter()
                .filter(|c| !c.word.eq_ignore_ascii_case(last_word))
                .collect();
            if !filtered.is_empty() {
                return filtered.into_iter().take(top_k).collect();
            }
            if words.len() == 1 {
                return self.trie.complete_prefix(last_word, top_k);
            }
            return Vec::new();
        }

        let context = if words.len() >= 3 {
            &words[words.len() - 3..]
        } else {
            &words[..]
        };

        self.trie.predict_next_word(context, top_k)
    }

    pub fn suggest_quick_replies(
        &self,
        previous_message: &str,
        pronoun_override: Option<&str>,
        top_k: usize,
        override_intent: Option<&str>,
    ) -> Vec<String> {
        suggest_quick_replies(previous_message, pronoun_override, top_k, override_intent)
    }

    pub fn suggest_quick_replies_with_model(
        &self,
        previous_message: &str,
        pronoun_override: Option<&str>,
        top_k: usize,
    ) -> Vec<String> {
        quick_replies_with_intent(&self.tokenizer, previous_message, pronoun_override, top_k)
    }

    pub fn is_unreplyable_or_ack_message(&self, text: &str) -> bool {
        crate::pronoun::is_unreplyable_or_ack_message(text)
    }

    pub fn extract_mentioned_pronoun(&self, text: &str) -> &'static str {
        crate::pronoun::extract_mentioned_pronoun(text)
    }

    pub fn on_keystroke(
        &self,
        text: &str,
        previous_message: Option<&str>,
        pronoun_override: Option<&str>,
        top_k: usize,
    ) -> KeystrokeEvent {
        let correction = self
            .correct_within_limit(text)
            .unwrap_or_else(|| CorrectionResult {
                original: text.to_string(),
                corrected: text.to_string(),
                has_errors: false,
                edits: Vec::new(),
            });
        let suggest_base = if correction.has_errors {
            correction.corrected.as_str()
        } else {
            text
        };
        let suggestions = self.suggest(suggest_base, top_k);

        let (pronoun, quick_replies) = match previous_message {
            Some(msg) if !msg.trim().is_empty() => {
                let detected_pronoun = pronoun_override
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| extract_target_pronoun(msg).to_string());
                let replies =
                    quick_replies_with_intent(&self.tokenizer, msg, pronoun_override, top_k);
                (Some(detected_pronoun), Some(replies))
            }
            _ => (None, None),
        };

        KeystrokeEvent {
            input: text.to_string(),
            correction,
            suggestions,
            pronoun,
            quick_replies,
        }
    }
}
