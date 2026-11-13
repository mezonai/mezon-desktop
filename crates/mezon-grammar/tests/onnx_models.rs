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

    let s1 = "tuoi tac tang len dong nghia voi viec the luc dang di xuong.";
    let res1 = client.restore_sentence(s1);
    assert!(res1.contains("thể lực đang đi xuống"));
    assert!(res1.starts_with("Tuổi tác"));

    let s2 = "toi o ha noi, con ban o dau?";
    let res2 = client.restore_sentence(s2);
    assert!(res2.contains("còn bạn ở đâu?"));
    assert!(res2.starts_with("Tôi"));

    let s3 = "ko co viec j kho, chi so long khong ben.";
    let res3 = client.restore_sentence(s3);
    assert!(res3.contains("Không có việc gì khó") || res3.contains("không có việc gì khó"));
    assert!(res3.contains("chỉ sợ lòng không bền"));

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

    mezon_grammar::set_download_enabled(false);
    assert!(!mezon_grammar::is_download_enabled());
    let missing = mezon_grammar::assets::load_asset_bytes(mezon_grammar::assets::CONFIG_FILE);
    assert!(missing.is_none());
    assert!(!temp.join(mezon_grammar::assets::CONFIG_FILE).is_file());

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

#[test]
fn test_teencode_diacritics_restoration() {
    assert_eq!(mezon_grammar::decode_teencode_diacritics("o^"), "ô");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("o+"), "ơ");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("a^"), "â");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("e~"), "ẽ");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("e."), "ẹ");

    assert_eq!(mezon_grammar::decode_teencode_diacritics("to+'i"), "tới");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("to+i'"), "tới");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("to^'i"), "tối");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("to^.i"), "tội");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("me. oi"), "mẹ oi");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("ba.n"), "bạn");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("e?"), "ẻ");
}

#[test]
fn test_telecom_voip_call_sentence_restoration() {
    let client = MezonGrammarClient::new();
    let s_teencode = "Vua co call 1-1 to+'i. Ca mobile la^n~ desktop do^? chuong. Anh bat may tren mobile. Desktop ta(t' chuong, mobile nghe binh thuong. Good!!!!";
    let res = client.restore_sentence(s_teencode);
    let expected = "Vừa có call 1-1 tới. Cả mobile lẫn desktop đổ chuông. Anh bật máy trên mobile. Desktop tắt chuông, mobile nghe bình thường. Good!!!!";
    assert_eq!(res, expected);

    let res1 = client.restore_sentence("to^i muon an com");
    assert_eq!(res1, "Tôi muốn ăn cơm");

    let res2 = client.restore_sentence("to^'i muon an com");
    assert_eq!(res2, "Tối muốn ăn cơm");

    let res3 = client.restore_sentence("to^.i nghiep chu' meo");
    assert_eq!(res3, "Tội nghiệp chú mèo");
}

#[test]
fn test_phonotactic_syllable_validation() {
    for valid_w in ["giúp", "gió", "giỏi", "giọt", "giọng", "quí", "quít"] {
        assert!(
            mezon_grammar::is_valid_syllable_vocab(valid_w),
            "expected {valid_w} to be valid syllable"
        );
    }
}

#[test]
fn test_lang_filter_vn_uu_preserved() {
    for vn_w in ["cuu", "luu", "huu", "muu"] {
        assert!(
            !mezon_grammar::rules::is_foreign_or_tech_word(vn_w),
            "expected {vn_w} not to be foreign or tech word"
        );
    }
}

#[test]
fn test_tach_biet_confusable_and_collocations() {
    let client = MezonGrammarClient::new();
    assert!(client.correct("tach biet").corrected.to_lowercase().contains("tách biệt"));
    assert!(client.correct("dac biet").corrected.to_lowercase().contains("đặc biệt"));
    assert!(client.correct("khac biet").corrected.to_lowercase().contains("khác biệt"));
    assert!(client.correct("phan biet").corrected.to_lowercase().contains("phân biệt"));
}

#[test]
fn test_split_coda_teencode_space_restoration() {
    assert_eq!(mezon_grammar::decode_teencode_diacritics("to^. i"), "tội");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("ba? n"), "bản");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("ba. n"), "bạn");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("Bả n"), "Bản");
    assert_eq!(mezon_grammar::decode_teencode_diacritics("Tộ i"), "Tội");
}
