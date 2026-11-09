use crate::onnx::{apply_neural_accent_onnx, restore_sentence_onnx};

pub fn apply_neural_accent(text: &str) -> String {
    apply_neural_accent_onnx(text)
}

pub fn restore_sentence(text: &str, capitalize: bool, resolve_teencode: bool) -> String {
    restore_sentence_onnx(text, capitalize, resolve_teencode)
}
