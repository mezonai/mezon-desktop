#![allow(
    clippy::collapsible_if,
    clippy::new_without_default,
    clippy::type_complexity,
    clippy::unnecessary_sort_by
)]

pub mod accent;
pub mod assets;
pub mod client;
pub mod grammar_checker;
pub mod neural;
pub mod onnx;
pub mod pronoun;
pub mod rules;
pub mod syllable;
pub mod tokenizer;
pub mod trie;

pub use assets::{
    SYLLABLE_VOCAB_FILE, is_download_enabled, prefetch_all_assets, set_download_enabled,
};
pub use client::{KeystrokeEvent, MAX_CORRECTION_CHAR_COUNT, MezonGrammarClient};
pub use neural::restore_sentence;
pub use pronoun::{
    SUPPORTED_PRONOUNS, extract_mentioned_pronoun, extract_target_pronoun,
    generate_dynamic_replies, is_unreplyable_or_ack_message, mask_non_pronouns,
    render_dynamic_template, resolve_pronoun_pair, suggest_quick_replies,
};
pub use rules::{CorrectionEdit, CorrectionResult};
pub use syllable::{
    candidates_for_syllable, is_valid_vietnamese_syllable as is_valid_syllable_vocab, syllable_id,
};
pub use tokenizer::VietnameseTokenizer;
pub use trie::{WordSuggestion, WordTrie};
