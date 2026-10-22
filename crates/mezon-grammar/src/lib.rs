#![allow(
    clippy::collapsible_if,
    clippy::new_without_default,
    clippy::type_complexity,
    clippy::unnecessary_sort_by
)]

pub mod accent;
pub mod client;
pub mod grammar_checker;
pub mod neural;
pub mod onnx;
pub mod pronoun;
pub mod rules;
pub mod tokenizer;
pub mod trie;

pub use client::{KeystrokeEvent, MAX_CORRECTION_CHAR_COUNT, MezonGrammarClient};
pub use pronoun::{
    SUPPORTED_PRONOUNS, extract_target_pronoun, generate_dynamic_replies, render_dynamic_template,
    resolve_pronoun_pair, suggest_quick_replies,
};
pub use rules::{CorrectionEdit, CorrectionResult};
pub use tokenizer::VietnameseTokenizer;
pub use trie::{WordSuggestion, WordTrie};
