use mezon_grammar::client::MezonGrammarClient;
use mezon_grammar::onnx::{INTENT_MIN_CONFIDENCE, apply_neural_accent_onnx, predict_intent};
use mezon_grammar::tokenizer::VietnameseTokenizer;

#[test]
fn intent_leave_or_absence_above_threshold() {
    let tokenizer = VietnameseTokenizer::new();
    let prediction = predict_intent(&tokenizer, "Sáng mai anh nghỉ làm nhé");
    let Some(p) = prediction else {
        return;
    };
    assert!(matches!(
        p.intent.as_str(),
        "action_leave_ack" | "leave_or_absence"
    ));
    assert!(p.confidence >= INTENT_MIN_CONFIDENCE);
}

#[test]
fn intent_other_url_blocks_quick_replies() {
    let client = MezonGrammarClient::new();
    let replies = client.suggest_quick_replies_with_model(
        "https://github.com/mezon/grammar/pull/123",
        None,
        4,
    );
    assert!(replies.is_empty());
}

#[test]
fn neural_accent_restores_vietnamese() {
    let out = apply_neural_accent_onnx("Sang mai anh nghi lam nhe");
    assert!(out.contains('ỉ') || out.contains('á') || out.contains('à'));
}

#[test]
fn neural_accent_keeps_can_dung_unikey() {
    let input = "mong cai them dau tieng viet de anh ko can dung unikey nua";
    let out = apply_neural_accent_onnx(input);
    assert!(
        out.contains("cần dùng unikey nữa") || out.contains("cần dùng"),
        "{out}"
    );
    assert!(!out.contains("cần đúng"), "{out}");
    assert!(!out.contains("cần dụng"), "{out}");
    let corrected = MezonGrammarClient::new().correct(input).corrected;
    assert_eq!(
        corrected,
        "Mong cái thêm dấu tiếng Việt để anh không cần dùng unikey nữa"
    );
}

#[test]
fn neural_accent_keeps_desktop_and_tech_terms() {
    let input =
        "thay check 2 cau vua roi xem ben client example thay chay co khop ben desktop khong";
    let out = apply_neural_accent_onnx(input);
    assert_eq!(
        out,
        "thầy check 2 câu vừa rồi xem bên client example thấy chạy có khớp bên desktop không"
    );
    let corrected = MezonGrammarClient::new().correct(input).corrected;
    assert_eq!(
        corrected,
        "Thầy check 2 câu vừa rồi xem bên client example thấy chạy có khớp bên desktop không"
    );
}

#[test]
fn neural_accent_keeps_ok_pronoun() {
    let client = MezonGrammarClient::new();
    assert_eq!(client.correct("ok anh").corrected, "Ok anh");
    assert_eq!(client.correct("ok ánh").corrected, "Ok anh");
    assert_eq!(client.correct("ok sep").corrected, "Ok sếp");
    assert_eq!(client.correct("ok nhe").corrected, "Ok nhé");
    assert!(!apply_neural_accent_onnx("ok anh").contains("ánh"));
}

#[test]
fn neural_accent_restores_minh_sua_roi_nhe() {
    assert_eq!(
        apply_neural_accent_onnx("minh sua roi nhe"),
        "mình sửa rồi nhé"
    );
    assert_eq!(
        apply_neural_accent_onnx("minh sua roi nhé"),
        "mình sửa rồi nhé"
    );
    assert_eq!(apply_neural_accent_onnx("em sẽ làm ngay"), "em sẽ làm ngay");
    let client = MezonGrammarClient::new();
    assert_eq!(
        client.correct("minh sua roi nhe").corrected,
        "Mình sửa rồi nhé"
    );
    assert_eq!(
        client.correct("minh sua roi nhé").corrected,
        "Mình sửa rồi nhé"
    );
    assert!(!apply_neural_accent_onnx("Bob ban oi").contains("Bọn"));
}

#[test]
fn test_neural_model_called_after_pipeline_for_final_spelling_result() {
    let client = MezonGrammarClient::new();
    // Partially accented or unaccented phrase where neural model completes diacritics
    let res = client.correct("anh em gio trao doi o day nhe");
    assert!(
        res.edits
            .iter()
            .any(|e| e.reason.contains("Neural Accent Model")),
        "Edits: {:?}",
        res.edits
    );
    assert!(res.corrected.contains("trao đổi") || res.corrected.contains("ở đây"));
}
