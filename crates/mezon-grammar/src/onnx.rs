use crate::tokenizer::VietnameseTokenizer;
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

const INTENT_MODEL_BYTES: &[u8] = include_bytes!("../assets/model.onnx");
const ACCENT_MODEL_BYTES: &[u8] = include_bytes!("../assets/accent_model.onnx");
const ACCENT_VOCAB_JSON: &str = include_str!("../assets/accent_vocab.json");

pub const INTENT_MIN_CONFIDENCE: f32 = 0.80;
pub const ACCENT_MAX_CHARS: usize = 128;
pub(crate) const ACCENT_VOCAB_SIZE: usize = 256;
const INTENT_MAX_SEQ_LEN: usize = 128;

pub static INTENT_LABELS: [&str; 12] = [
    "action_general_ack",
    "action_greeting_reply",
    "action_thanks_reply",
    "action_apology_forgive",
    "action_question_inquiry",
    "action_task_accept",
    "action_leave_ack",
    "action_praise_receive",
    "action_no_reply_needed",
    "action_confirm_yes",
    "action_health_ack",
    "action_meal_social",
];

pub static TARGET_PRONOUNS: [&str; 14] = [
    "anh",
    "chị",
    "em",
    "sếp",
    "cậu",
    "ông",
    "bà",
    "bác",
    "chú",
    "cô",
    "thầy",
    "bạn",
    "cả nhà",
    "cháu",
];

#[derive(Debug, Clone, Deserialize)]
struct AccentVocabFile {
    char_to_id: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct IntentPrediction {
    pub intent: String,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct PronounPrediction {
    pub pronoun: String,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct DualPrediction {
    pub intent: IntentPrediction,
    pub pronoun: Option<PronounPrediction>,
}

static ACCENT_CHAR_TO_ID: LazyLock<HashMap<String, i64>> = LazyLock::new(|| {
    let parsed: AccentVocabFile =
        serde_json::from_str(ACCENT_VOCAB_JSON).unwrap_or_else(|_| AccentVocabFile {
            char_to_id: HashMap::new(),
        });
    parsed.char_to_id
});

pub(crate) fn accent_char_id(ch: char) -> usize {
    ACCENT_CHAR_TO_ID.get(&ch.to_string()).copied().unwrap_or(1) as usize
}

enum SessionSlot {
    Pending,
    Ready(Session),
    Failed,
}

static INTENT_SESSION: OnceLock<Mutex<SessionSlot>> = OnceLock::new();
static ACCENT_SESSION: OnceLock<Mutex<SessionSlot>> = OnceLock::new();

fn load_session(bytes: &[u8]) -> Result<Session, ort::Error> {
    Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .with_intra_op_spinning(false)?
        .with_inter_op_spinning(false)?
        .commit_from_memory(bytes)
}

fn intent_slot() -> &'static Mutex<SessionSlot> {
    INTENT_SESSION.get_or_init(|| Mutex::new(SessionSlot::Pending))
}

fn accent_slot() -> &'static Mutex<SessionSlot> {
    ACCENT_SESSION.get_or_init(|| Mutex::new(SessionSlot::Pending))
}

fn ensure_intent_session() -> Option<()> {
    let mut guard = intent_slot().lock().unwrap();
    match &*guard {
        SessionSlot::Ready(_) => return Some(()),
        SessionSlot::Failed => return None,
        SessionSlot::Pending => {}
    }
    match load_session(INTENT_MODEL_BYTES) {
        Ok(session) => {
            *guard = SessionSlot::Ready(session);
            Some(())
        }
        Err(_) => {
            *guard = SessionSlot::Failed;
            None
        }
    }
}

fn ensure_accent_session() -> Option<()> {
    let mut guard = accent_slot().lock().unwrap();
    match &*guard {
        SessionSlot::Ready(_) => return Some(()),
        SessionSlot::Failed => return None,
        SessionSlot::Pending => {}
    }
    match load_session(ACCENT_MODEL_BYTES) {
        Ok(session) => {
            *guard = SessionSlot::Ready(session);
            Some(())
        }
        Err(_) => {
            *guard = SessionSlot::Failed;
            None
        }
    }
}

fn encode_intent_input(tokenizer: &VietnameseTokenizer, text: &str) -> Vec<i64> {
    let bos = 2i64;
    let eos = 3i64;
    let mut ids = vec![bos];
    ids.extend(tokenizer.encode(text));
    ids.push(eos);
    if ids.len() > INTENT_MAX_SEQ_LEN {
        ids = ids[ids.len() - INTENT_MAX_SEQ_LEN..].to_vec();
    }
    ids
}

fn softmax_top(logits: &[f32], labels: &[&str]) -> Option<(String, f32)> {
    let n = labels.len();
    if logits.len() < n {
        return None;
    }
    let mut max_logit = logits[0];
    for &v in logits.iter().take(n) {
        if v > max_logit {
            max_logit = v;
        }
    }
    let mut sum_exp = 0f32;
    let mut probs = vec![0f32; n];
    for i in 0..n {
        let p = (logits[i] - max_logit).exp();
        probs[i] = p;
        sum_exp += p;
    }
    if sum_exp <= 0.0 {
        return None;
    }
    let mut top_idx = 0usize;
    let mut top_prob = probs[0] / sum_exp;
    for (i, prob) in probs.iter().enumerate().take(n).skip(1) {
        let p = *prob / sum_exp;
        if p > top_prob {
            top_prob = p;
            top_idx = i;
        }
    }
    Some((labels[top_idx].to_string(), top_prob))
}

pub fn predict_dual(tokenizer: &VietnameseTokenizer, text: &str) -> Option<DualPrediction> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    ensure_intent_session()?;
    let ids = encode_intent_input(tokenizer, trimmed);
    let seq_len = ids.len();
    let input = match Tensor::from_array(([1i64, seq_len as i64], ids)) {
        Ok(t) => t,
        Err(_) => return None,
    };
    let mut guard = intent_slot().lock().unwrap();
    let SessionSlot::Ready(session) = &mut *guard else {
        return None;
    };
    let outputs = match session.run(ort::inputs!["input_ids" => input]) {
        Ok(o) => o,
        Err(_) => return None,
    };
    let (_shape, intent_logits) = match outputs[0].try_extract_tensor::<f32>() {
        Ok(v) => v,
        Err(_) => return None,
    };
    let (top_intent, top_intent_prob) = softmax_top(intent_logits, &INTENT_LABELS)?;
    let intent = IntentPrediction {
        intent: top_intent,
        confidence: top_intent_prob,
    };
    let pronoun = if outputs.len() > 1 {
        outputs[1]
            .try_extract_tensor::<f32>()
            .ok()
            .and_then(|(_shape, pron_logits)| softmax_top(pron_logits, &TARGET_PRONOUNS))
            .map(|(p, c)| PronounPrediction {
                pronoun: p,
                confidence: c,
            })
    } else {
        None
    };
    Some(DualPrediction { intent, pronoun })
}

pub fn predict_intent(tokenizer: &VietnameseTokenizer, text: &str) -> Option<IntentPrediction> {
    predict_dual(tokenizer, text).map(|d| d.intent)
}

pub fn predict_pronoun(tokenizer: &VietnameseTokenizer, text: &str) -> Option<PronounPrediction> {
    predict_dual(tokenizer, text).and_then(|d| d.pronoun)
}

pub fn apply_neural_accent_onnx(text: &str) -> String {
    crate::accent::add_accents(text, &accent_logits)
}

fn accent_logits(text: &str) -> Option<Vec<f32>> {
    if text.is_empty() || ensure_accent_session().is_none() {
        return None;
    }
    let input_ids: Vec<i64> = text.chars().map(|ch| accent_char_id(ch) as i64).collect();
    let seq_len = input_ids.len();
    let input = Tensor::from_array(([1i64, seq_len as i64], input_ids)).ok()?;
    let mut guard = accent_slot().lock().unwrap();
    let SessionSlot::Ready(session) = &mut *guard else {
        return None;
    };
    let outputs = session.run(ort::inputs!["input_ids" => input]).ok()?;
    let (_shape, logits) = outputs[0].try_extract_tensor::<f32>().ok()?;
    Some(logits.to_vec())
}

pub fn quick_replies_with_intent(
    tokenizer: &VietnameseTokenizer,
    previous_message: &str,
    pronoun_override: Option<&str>,
    top_k: usize,
) -> Vec<String> {
    let dual = predict_dual(tokenizer, previous_message);
    let override_intent = match &dual {
        Some(d) if d.intent.confidence >= INTENT_MIN_CONFIDENCE => {
            if matches!(
                d.intent.intent.as_str(),
                "other" | "action_no_reply_needed" | "reply_none"
            ) {
                return Vec::new();
            }
            Some(d.intent.intent.as_str())
        }
        Some(_) | None => return Vec::new(),
    };
    let effective_pronoun = pronoun_override.or_else(|| {
        dual.as_ref()
            .and_then(|d| d.pronoun.as_ref())
            .filter(|p| p.confidence >= INTENT_MIN_CONFIDENCE && p.pronoun != "bạn")
            .map(|p| p.pronoun.as_str())
    });
    crate::pronoun::suggest_quick_replies(
        previous_message,
        effective_pronoun,
        top_k,
        override_intent,
    )
}
