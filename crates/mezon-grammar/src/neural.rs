use crate::onnx::apply_neural_accent_onnx;

pub fn apply_neural_accent(text: &str) -> String {
    apply_neural_accent_onnx(text)
}
