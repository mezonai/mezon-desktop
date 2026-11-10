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

#[test]
fn test_model2_precision_collocations() {
    let client = MezonGrammarClient::new();
    let res1 = client
        .correct("tho lanh nghe cung duoc sang loc ky hon")
        .corrected;
    assert!(res1.to_lowercase().contains("thợ lành nghề"));

    let res2 = client
        .correct("cac tu lieu hoc tap: phieu hoc tap")
        .corrected;
    assert!(res2.to_lowercase().contains("tư liệu học tập"));

    let res3 = client
        .correct("doanh nghiep co gang dam phan de co loi nhat")
        .corrected;
    assert!(res3.to_lowercase().contains("cố gắng đàm phán"));

    let res4 = client.correct("goc giua chung bang 90 do").corrected;
    assert!(res4.to_lowercase().contains("góc giữa chúng") && res4.contains("90 độ"));
}

#[test]
fn test_restore_sentence_tab_prediction() {
    let client = MezonGrammarClient::new();

    // 1. Homophone sentence with start capitalization
    let s1 = "tuoi tac tang len dong nghia voi viec the luc dang di xuong.";
    let res1 = client.restore_sentence(s1);
    assert!(res1.contains("thể lực đang đi xuống"));
    assert!(res1.starts_with("Tuổi tác"));

    // 2. Chat question with 'còn bạn ở đâu?'
    let s2 = "toi o ha noi, con ban o dau?";
    let res2 = client.restore_sentence(s2);
    assert!(res2.contains("còn bạn ở đâu?"));
    assert!(res2.starts_with("Tôi"));

    // 3. Proverb with teencode: 'ko co viec j kho, chi so long khong ben.'
    let s3 = "ko co viec j kho, chi so long khong ben.";
    let res3 = client.restore_sentence(s3);
    assert!(res3.contains("Không có việc gì khó") || res3.contains("không có việc gì khó"));
    assert!(res3.contains("chỉ sợ lòng không bền"));

    // 4. Ultra-long sentence (> 150 characters) without truncation
    let s4 = "tuoi tac tang len dong nghia voi viec the luc dang di xuong va chung ta can phai tap luyen the duc the thao thuong xuyen de duy tri suc khoe tot nhat cho ban than";
    let res4 = client.restore_sentence(s4);
    assert!(res4.contains("thể lực đang đi xuống"));
    assert!(res4.contains("cho bạn thân") || res4.contains("cho bản thân"));
    assert!(res4.starts_with("Tuổi tác"));
}

#[test]
fn test_download_only_when_config_enabled() {
    let temp = std::env::temp_dir().join(format!("mezon_test_cfg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    unsafe {
        std::env::set_var("MEZON_GRAMMAR_DIR", &temp);
    }

    // When disabled, load_asset_bytes should NOT download and return None
    mezon_grammar::set_download_enabled(false);
    assert!(!mezon_grammar::is_download_enabled());
    let missing = mezon_grammar::assets::load_asset_bytes(mezon_grammar::assets::CONFIG_FILE);
    assert!(missing.is_none());
    assert!(!temp.join(mezon_grammar::assets::CONFIG_FILE).is_file());

    // When enabled, download succeeds
    mezon_grammar::set_download_enabled(true);
    assert!(mezon_grammar::is_download_enabled());
    let loaded = mezon_grammar::assets::load_asset_str(mezon_grammar::assets::CONFIG_FILE);
    assert!(loaded.is_some());
    assert!(temp.join(mezon_grammar::assets::CONFIG_FILE).is_file());

    let _ = std::fs::remove_dir_all(&temp);
    unsafe {
        std::env::remove_var("MEZON_GRAMMAR_DIR");
    }
}

#[test]
fn test_syllable_vocab_download_and_loading() {
    assert_eq!(mezon_grammar::SYLLABLE_VOCAB_FILE, "syllable_vocab.json");
    let loaded = mezon_grammar::assets::load_asset_str(mezon_grammar::SYLLABLE_VOCAB_FILE);
    assert!(
        loaded.is_some(),
        "syllable_vocab.json should be loaded/downloadable"
    );
    assert!(mezon_grammar::is_valid_syllable_vocab("người"));
    assert!(mezon_grammar::is_valid_syllable_vocab("việt"));
    assert!(mezon_grammar::is_valid_syllable_vocab("nam"));
    let cands = mezon_grammar::candidates_for_syllable("muon");
    assert!(cands.is_some());
    let list = cands.unwrap();
    assert!(list.iter().any(|s| s == "muốn"));
}

#[test]
fn test_model2_mined_pairs() {
    let client = MezonGrammarClient::new();
    assert!(
        client
            .correct("16 máu")
            .corrected
            .to_lowercase()
            .contains("16 màu")
    );
    assert!(
        client
            .correct("be thui")
            .corrected
            .to_lowercase()
            .contains("bê thui")
    );
    assert!(
        client
            .correct("bo xung")
            .corrected
            .to_lowercase()
            .contains("bổ sung")
    );
    assert!(
        client
            .correct("con luon")
            .corrected
            .to_lowercase()
            .contains("con lươn")
    );
    assert!(
        client
            .correct("chuan doan")
            .corrected
            .to_lowercase()
            .contains("chẩn đoán")
    );
    assert!(
        client
            .correct("xap sep")
            .corrected
            .to_lowercase()
            .contains("sắp xếp")
    );
}
