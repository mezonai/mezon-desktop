use mezon_grammar::MAX_CORRECTION_CHAR_COUNT;
use mezon_grammar::client::MezonGrammarClient;
use mezon_grammar::grammar_checker::check_grammar_syntax;
use mezon_grammar::pronoun::extract_target_pronoun;
use mezon_grammar::rules::{normalize_punctuation_spacing, restore_diacritics};

#[test]
fn test_teencode_and_confusables() {
    let client = MezonGrammarClient::new();
    let res = client.correct("hnay mik di lam rat zui , xap sep rat sang lang");
    assert_eq!(
        res.corrected,
        "Hôm nay mình đi làm rất vui, sắp xếp rất xán lạn"
    );
    assert!(res.has_errors);
}

#[test]
fn test_grammar_redundancy_step() {
    let (fixed, edits) = check_grammar_syntax("rất là rất vui");
    assert_eq!(fixed, "rất vui");
    assert!(!edits.is_empty());
}

#[test]
fn test_triple_word_run_collapses() {
    let (fixed, edits) = check_grammar_syntax("hôm nay vui vui vui quá");
    assert_eq!(fixed, "hôm nay vui quá");
    assert!(!edits.is_empty());
}

#[test]
fn test_char_limit_gate() {
    let client = MezonGrammarClient::new();
    let long = "a".repeat(MAX_CORRECTION_CHAR_COUNT + 1);
    assert!(client.correct_within_limit(&long).is_none());
    let ok = "a".repeat(MAX_CORRECTION_CHAR_COUNT);
    assert!(client.correct_within_limit(&ok).is_some());
}

#[test]
fn test_unicode_char_limit_not_bytes() {
    let client = MezonGrammarClient::new();
    let within = "ă".repeat(MAX_CORRECTION_CHAR_COUNT);
    assert!(client.correct_within_limit(&within).is_some());
    let over = "ă".repeat(MAX_CORRECTION_CHAR_COUNT + 1);
    assert!(client.correct_within_limit(&over).is_none());
}

#[test]
fn test_low_confidence_intent_yields_no_model_quick_replies() {
    let client = MezonGrammarClient::new();
    let replies = client.suggest_quick_replies_with_model("123456", None, 4);
    assert!(replies.is_empty());
}

#[test]
fn test_junior_workplace_proposal_senior_response() {
    let client = MezonGrammarClient::new();
    let msg = "Chỗ assign manually, em thấy bên zoom đang như thế này ạ, nếu room nhiều người thì UI như này có vẻ gọn ạ.";
    let replies = client.suggest_quick_replies_with_model(msg, None, 4);
    assert!(!replies.is_empty());
    assert!(!replies.iter().any(|r| r.starts_with("Dạ")));
    assert!(replies.iter().any(|r| r.contains("Anh thấy") || r.contains("anh")));
    assert!(replies.iter().any(|r| r.contains("em")));

    let replies_chi = client.suggest_quick_replies_with_model(msg, Some("chị"), 4);
    assert!(!replies_chi.is_empty());
    assert!(!replies_chi.iter().any(|r| r.starts_with("Dạ")));
    assert!(replies_chi.iter().any(|r| r.contains("Chị thấy") || r.contains("chị")));
    assert!(replies_chi.iter().any(|r| r.contains("em")));
}

#[test]
fn test_action_leave_ack_maps_to_leave_replies() {
    let client = MezonGrammarClient::new();
    let replies = client.suggest_quick_replies(
        "Sáng mai anh nghỉ làm nhé",
        None,
        4,
        Some("action_leave_ack"),
    );
    assert!(replies.iter().any(|reply| reply.contains("nghỉ")));
}

#[test]
fn test_quick_replies_override_other() {
    let client = MezonGrammarClient::new();
    let replies = client.suggest_quick_replies("https://example.com", None, 4, Some("other"));
    assert!(replies.is_empty());
}

#[test]
fn greeting_vocative_flips_to_the_senders_role() {
    use mezon_grammar::pronoun::suggest_quick_replies;

    assert_eq!(extract_target_pronoun("chào em"), "anh");
    assert_eq!(extract_target_pronoun("chào anh"), "em");
    assert_eq!(extract_target_pronoun("chào chị"), "em");
    let replies = suggest_quick_replies("chào em", None, 4, Some("action_greeting_reply"));
    assert!(replies.iter().any(|reply| reply.contains("anh")));
    assert!(
        !replies
            .iter()
            .any(|reply| { reply == "Chào em nhé!" || reply == "Anh nghe đây em." })
    );
}

#[test]
fn dynamic_pair_follows_python_examples() {
    use mezon_grammar::{generate_dynamic_replies, render_dynamic_template, resolve_pronoun_pair};

    assert_eq!(
        render_dynamic_template("Cảm ơn [X], [Y] khỏe.", "chú", "cháu"),
        "Cảm ơn cháu, chú khỏe."
    );
    assert_eq!(
        render_dynamic_template("Cảm ơn [X], [Y] khỏe.", "anh", "em"),
        "Cảm ơn em, anh khỏe."
    );
    assert_eq!(
        render_dynamic_template("Cảm ơn [X], [Y] khỏe.", "em", "anh"),
        "Cảm ơn anh, em khỏe."
    );
    assert_eq!(
        render_dynamic_template("Dạ [Y] cảm ơn [X], [Y] vẫn khỏe ạ.", "em", "sếp"),
        "Dạ em cảm ơn sếp, em vẫn khỏe ạ."
    );
    assert_eq!(
        render_dynamic_template("Cảm ơn [X], [Y] khỏe.", "chú", "em"),
        "Cảm ơn em, anh khỏe."
    );
    assert_eq!(
        render_dynamic_template("Cảm ơn [X], [Y] khỏe.", "anh", "chú"),
        "Cảm ơn chú, anh khỏe."
    );

    let (speaker, addressee) = resolve_pronoun_pair("", Some("em"), Some("chú"));
    assert_eq!(speaker, "anh");
    assert_eq!(addressee, "em");

    assert!(
        generate_dynamic_replies("action_health_ack", "chú", "cháu", 4)
            .iter()
            .any(|reply| reply == "Cảm ơn cháu, chú khỏe.")
    );
    assert!(
        generate_dynamic_replies("action_health_ack", "anh", "em", 4)
            .iter()
            .any(|reply| reply == "Cảm ơn em, anh khỏe.")
    );
    assert!(
        generate_dynamic_replies("action_health_ack", "em", "anh", 4)
            .iter()
            .any(|reply| reply == "Cảm ơn anh, em khỏe.")
    );
    assert!(
        generate_dynamic_replies("action_health_ack", "em", "sếp", 4)
            .iter()
            .any(|reply| reply == "Dạ em cảm ơn sếp, em vẫn khỏe ạ.")
    );
    let chu = generate_dynamic_replies("action_health_ack", "anh", "chú", 4);
    assert!(chu.iter().any(|reply| reply == "Cảm ơn chú, anh khỏe."));
    assert!(chu.iter().any(|reply| reply == "Anh khỏe, cảm ơn chú nhé."));
}

#[test]
fn example_quick_replies_use_dynamic_pair_after_intent() {
    use mezon_grammar::suggest_quick_replies;

    let replies = suggest_quick_replies(
        "cai mezon grammar cung xong model roi. Anh em add vao suggest luon. Mode nay 447kb.",
        None,
        4,
        Some("action_task_accept"),
    );
    assert_eq!(
        replies,
        vec![
            "OK, anh.".to_string(),
            "Dạ vâng anh.".to_string(),
            "Để em add và test luôn.".to_string(),
            "Dạ để em xử lý ngay ạ.".to_string(),
        ]
    );

    let health = suggest_quick_replies("Hôm nay anh khỏe không?", Some("em"), 4, None);
    assert!(health.iter().any(|reply| reply == "Cảm ơn em, anh khỏe."));
    let health_chu = suggest_quick_replies("Hôm nay anh khỏe không?", Some("chú"), 4, None);
    assert!(
        health_chu
            .iter()
            .any(|reply| reply == "Cảm ơn chú, anh khỏe.")
    );
    let wedding = suggest_quick_replies("Mai anh đi ăn cưới không?", Some("chú"), 4, None);
    assert!(
        wedding
            .iter()
            .any(|reply| reply == "Có chứ chú, mai anh đi.")
    );
}

#[test]
fn test_reciprocal_pronoun_extraction() {
    assert_eq!(
        extract_target_pronoun("Em ơi check giúp anh task này nhé"),
        "anh"
    );
    assert_eq!(
        extract_target_pronoun(
            "cai mezon grammar cung xong model roi. Anh em add vao suggest luon."
        ),
        "anh"
    );
}

#[test]
fn test_punctuation_keeps_url_emoji_and_code() {
    let (fixed, _) =
        normalize_punctuation_spacing("xem https://mezon.ai nhe :smile: va `a.b` ```code.x```");
    assert!(fixed.contains("https://mezon.ai"), "{fixed}");
    assert!(fixed.contains(":smile:"), "{fixed}");
    assert!(fixed.contains("`a.b`"), "{fixed}");
    assert!(fixed.contains("```code.x```"), "{fixed}");
    assert!(!fixed.contains("https: //"), "{fixed}");
    let (spaced, _) = normalize_punctuation_spacing("hello ,world");
    assert_eq!(spaced, "hello, world");
}

#[test]
fn test_thx_and_weekday_teencode() {
    let client = MezonGrammarClient::new();
    assert_eq!(client.correct("thx").corrected, "Cảm ơn");
    assert_eq!(client.correct("t2").corrected, "Thứ hai");
}

#[test]
fn test_keeps_already_accented_ngay() {
    let client = MezonGrammarClient::new();
    assert_eq!(client.correct("em sẽ làm ngay").corrected, "Em sẽ làm ngay");
    assert_eq!(
        client.correct("anh đi ngay nhé").corrected,
        "Anh đi ngay nhé"
    );
}

#[test]
fn test_keeps_dotted_and_at_tokens() {
    let samples = [
        "mezon.ai",
        "google.com",
        "huy@ncc.asia",
        "main.rs",
        "@huy.lexuan",
    ];
    let client = MezonGrammarClient::new();
    for sample in samples {
        let (fixed, _) = normalize_punctuation_spacing(sample);
        assert_eq!(fixed, sample, "{sample}");
        assert_eq!(client.correct(sample).corrected, sample, "{sample}");
    }
    let (restored, _) = restore_diacritics("google.com");
    assert_eq!(restored, "google.com");
    let corrected = client.correct("hoi @huy.lexuan di").corrected;
    assert!(corrected.contains("@huy.lexuan"), "{corrected}");
    assert!(!corrected.contains("@huy. Lexuan"), "{corrected}");
}
