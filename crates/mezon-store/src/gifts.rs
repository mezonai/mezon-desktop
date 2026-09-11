use std::time::{Duration, Instant};

use mmn_client::{DECIMALS, ExtraInfo, TRANSFER_TYPE_TRANSFER_TOKEN, scale_amount_to_decimals};
use serde::{Deserialize, Serialize};

use crate::wallet::SendTokenRequest;

pub const FLOWER_PRICE: i64 = 50_000;
pub const FLOWER_GIFT_TYPE: &str = "flower";
const FLOWER_REACTION_PREFIX: &str = "flower:";
const APP_REACTION_PREFIX: &str = "app:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum VoiceInteractiveEventType {
    Gift = 1,
    Recording = 2,
    AppQuiz = 10,
    AppBlackboard = 11,
    AppInteractive = 12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceInteractiveApp {
    Quiz,
    Blackboard,
    Interactive,
}

impl VoiceInteractiveApp {
    pub fn app_id(self) -> i64 {
        match self {
            Self::Quiz => 2_089_257_413_122_199_552,
            Self::Blackboard => 2_089_294_331_818_020_864,
            Self::Interactive => 2_089_273_739_668_623_360,
        }
    }

    pub fn event_type(self) -> VoiceInteractiveEventType {
        match self {
            Self::Quiz => VoiceInteractiveEventType::AppQuiz,
            Self::Blackboard => VoiceInteractiveEventType::AppBlackboard,
            Self::Interactive => VoiceInteractiveEventType::AppInteractive,
        }
    }

    pub fn from_event_type(value: i32) -> Option<Self> {
        match VoiceInteractiveEventType::from_i32(value) {
            Some(VoiceInteractiveEventType::AppQuiz) => Some(Self::Quiz),
            Some(VoiceInteractiveEventType::AppBlackboard) => Some(Self::Blackboard),
            Some(VoiceInteractiveEventType::AppInteractive) => Some(Self::Interactive),
            _ => None,
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Quiz => "quiz",
            Self::Blackboard => "blackboard",
            Self::Interactive => "interactive",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "quiz" => Some(Self::Quiz),
            "blackboard" => Some(Self::Blackboard),
            "interactive" => Some(Self::Interactive),
            _ => None,
        }
    }
}

impl VoiceInteractiveEventType {
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Gift),
            2 => Some(Self::Recording),
            10 => Some(Self::AppQuiz),
            11 => Some(Self::AppBlackboard),
            12 => Some(Self::AppInteractive),
            _ => None,
        }
    }
}
pub const FLOWER_RATE_LIMIT: Duration = Duration::from_secs(1);
pub const FLOWER_SCENE_TTL: Duration = Duration::from_millis(4000);
pub const FLOWER_DEDUP_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveFlowerDeny {
    SelfTarget,
    WalletUnavailable,
    Pending,
    RateLimited,
    Insufficient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowerInteractiveParams {
    pub receiver_id: String,
    pub gift_type: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GiftParams {
    Flower {
        receiver_id: String,
        timestamp: i64,
    },
    OtherGift,
    Unreadable,
}

#[derive(Deserialize)]
struct RawGiftParams {
    #[serde(default, alias = "receiverId", alias = "ReceiverId")]
    receiver_id: Option<JsonId>,
    #[serde(default, alias = "giftType", alias = "GiftType")]
    gift_type: Option<String>,
    #[serde(default, alias = "Timestamp")]
    timestamp: Option<i64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JsonId {
    Text(String),
    Number(i64),
}

impl JsonId {
    fn into_string(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Number(number) => number.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FlowerExtraAttribute {
    gift_type: String,
    voice_channel_id: String,
}

pub fn flower_price() -> i64 {
    FLOWER_PRICE
}

pub fn format_flower_amount(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    if n < 0 { format!("-{out}") } else { out }
}

pub fn flower_menu_blocked(pending: bool, balance: Option<&str>) -> bool {
    pending || balance.is_some_and(|value| !can_afford(value, flower_price()))
}

pub fn can_afford(balance: &str, price: i64) -> bool {
    let Ok(scaled) = scale_amount_to_decimals(&price.to_string(), DECIMALS) else {
        return false;
    };
    mmn_client::validate_amount(balance, &scaled)
}

pub fn can_give_flower(
    is_self: bool,
    wallet_available: bool,
    pending: bool,
    last_send: Option<Instant>,
    now: Instant,
    balance: Option<&str>,
) -> Result<(), GiveFlowerDeny> {
    if is_self {
        return Err(GiveFlowerDeny::SelfTarget);
    }
    if !wallet_available {
        return Err(GiveFlowerDeny::WalletUnavailable);
    }
    if pending {
        return Err(GiveFlowerDeny::Pending);
    }
    if last_send.is_some_and(|sent| now.duration_since(sent) < FLOWER_RATE_LIMIT) {
        return Err(GiveFlowerDeny::RateLimited);
    }
    match balance {
        None => Ok(()),
        Some(value) if can_afford(value, flower_price()) => Ok(()),
        Some(_) => Err(GiveFlowerDeny::Insufficient),
    }
}

pub fn flower_extra_attribute(voice_channel_id: &str) -> String {
    serde_json::to_string(&FlowerExtraAttribute {
        gift_type: FLOWER_GIFT_TYPE.to_string(),
        voice_channel_id: voice_channel_id.to_string(),
    })
    .unwrap_or_default()
}

pub fn build_flower_transfer(
    sender: String,
    sender_username: String,
    recipient: String,
    clan_id: String,
    voice_channel_id: String,
) -> SendTokenRequest {
    let extra_info = ExtraInfo {
        transfer_type: TRANSFER_TYPE_TRANSFER_TOKEN.to_string(),
        channel_id: Some(voice_channel_id.clone()),
        clan_id: Some(clan_id),
        user_receiver_id: Some(recipient.clone()),
        user_sender_id: Some(sender.clone()),
        user_sender_username: Some(sender_username),
        extra_attribute: Some(flower_extra_attribute(&voice_channel_id)),
        ..Default::default()
    };
    SendTokenRequest {
        sender,
        recipient,
        amount: flower_price(),
        note: Some("giveflower".to_string()),
        extra_info: Some(extra_info),
        by_address: false,
    }
}

pub fn classify_gift_params(params: &str) -> GiftParams {
    let Ok(raw) = serde_json::from_str::<RawGiftParams>(params) else {
        return GiftParams::Unreadable;
    };
    if raw
        .gift_type
        .as_deref()
        .is_some_and(|gift_type| gift_type != FLOWER_GIFT_TYPE)
    {
        return GiftParams::OtherGift;
    }
    let Some(receiver_id) = raw
        .receiver_id
        .map(JsonId::into_string)
        .filter(|receiver_id| !receiver_id.is_empty())
    else {
        return GiftParams::Unreadable;
    };
    GiftParams::Flower {
        receiver_id,
        timestamp: raw.timestamp.unwrap_or_default(),
    }
}

pub fn parse_flower_interactive_params(params: &str) -> Option<FlowerInteractiveParams> {
    match classify_gift_params(params) {
        GiftParams::Flower {
            receiver_id,
            timestamp,
        } => Some(FlowerInteractiveParams {
            receiver_id,
            gift_type: FLOWER_GIFT_TYPE.to_string(),
            timestamp,
        }),
        GiftParams::OtherGift | GiftParams::Unreadable => None,
    }
}

pub fn serialize_flower_interactive_params(receiver_id: &str, timestamp: i64) -> String {
    serde_json::to_string(&FlowerInteractiveParams {
        receiver_id: receiver_id.to_string(),
        gift_type: FLOWER_GIFT_TYPE.to_string(),
        timestamp,
    })
    .unwrap_or_default()
}

pub fn flower_effect_key(giver_id: &str, receiver_id: &str, timestamp: i64) -> String {
    format!("{giver_id}:{receiver_id}:{timestamp}")
}

pub fn flower_pair_key(giver_id: &str, receiver_id: &str) -> String {
    format!("{giver_id}:{receiver_id}")
}

pub fn flower_reaction_token(receiver_id: &str) -> String {
    format!("{FLOWER_REACTION_PREFIX}{receiver_id}")
}

pub fn parse_flower_reaction_token(token: &str) -> Option<&str> {
    let receiver_id = token.strip_prefix(FLOWER_REACTION_PREFIX)?;
    (!receiver_id.is_empty() && receiver_id.bytes().all(|b| b.is_ascii_digit()))
        .then_some(receiver_id)
}

pub fn app_reaction_token(app: VoiceInteractiveApp) -> String {
    format!("{APP_REACTION_PREFIX}{}", app.slug())
}

pub fn parse_app_reaction_token(token: &str) -> Option<VoiceInteractiveApp> {
    VoiceInteractiveApp::from_slug(token.strip_prefix(APP_REACTION_PREFIX)?)
}

pub fn flower_event_from_payload(
    event_type: i32,
    giver_id: i64,
    receiver_id: i64,
    voice_channel_id: i64,
    params: &str,
    joined_channel_id: i64,
) -> Option<(String, String, i64, String)> {
    if VoiceInteractiveEventType::from_i32(event_type) != Some(VoiceInteractiveEventType::Gift) {
        return None;
    }
    if voice_channel_id != joined_channel_id {
        return None;
    }
    let (receiver, timestamp) = match classify_gift_params(params) {
        GiftParams::Flower {
            receiver_id,
            timestamp,
        } => (receiver_id, timestamp),
        GiftParams::OtherGift => return None,
        GiftParams::Unreadable if receiver_id != 0 => (receiver_id.to_string(), 0),
        GiftParams::Unreadable => return None,
    };
    let giver = giver_id.to_string();
    let key = flower_effect_key(&giver, &receiver, timestamp);
    Some((giver, receiver, timestamp, key))
}

pub fn is_uncertain_transfer_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("http error")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("network")
}

#[cfg(test)]
mod tests {
    use super::{
        FLOWER_PRICE, FLOWER_RATE_LIMIT, FLOWER_SCENE_TTL, GiftParams, GiveFlowerDeny,
        VoiceInteractiveApp, VoiceInteractiveEventType, app_reaction_token, build_flower_transfer,
        can_afford, can_give_flower, classify_gift_params, flower_effect_key,
        flower_event_from_payload, flower_menu_blocked, flower_pair_key, flower_price,
        flower_reaction_token, format_flower_amount, is_uncertain_transfer_error,
        parse_app_reaction_token, parse_flower_interactive_params, parse_flower_reaction_token,
        serialize_flower_interactive_params,
    };
    use mmn_client::{DECIMALS, TRANSFER_TYPE_TRANSFER_TOKEN, scale_amount_to_decimals};
    use std::time::{Duration, Instant};

    fn scaled_balance(tokens: i64) -> String {
        scale_amount_to_decimals(&tokens.to_string(), DECIMALS).expect("scale")
    }

    #[test]
    fn flower_price_is_the_single_constant() {
        assert_eq!(flower_price(), FLOWER_PRICE);
        assert_eq!(flower_price(), 50_000);
    }

    #[test]
    fn format_flower_amount_uses_thousands_separators() {
        assert_eq!(format_flower_amount(flower_price()), "50,000");
        assert_eq!(format_flower_amount(0), "0");
        assert_eq!(format_flower_amount(999), "999");
        assert_eq!(format_flower_amount(1_000), "1,000");
    }

    #[test]
    fn can_afford_uses_flower_price() {
        assert!(can_afford(&scaled_balance(flower_price()), flower_price()));
        assert!(can_afford(
            &scaled_balance(flower_price() + 1),
            flower_price()
        ));
        assert!(!can_afford(
            &scaled_balance(flower_price() - 1),
            flower_price()
        ));
        assert!(!can_afford("not-a-number", flower_price()));
    }

    #[test]
    fn can_give_flower_covers_guards() {
        let now = Instant::now();
        let enough = scaled_balance(flower_price());
        assert_eq!(
            can_give_flower(true, true, false, None, now, Some(&enough)),
            Err(GiveFlowerDeny::SelfTarget)
        );
        assert_eq!(
            can_give_flower(false, false, false, None, now, Some(&enough)),
            Err(GiveFlowerDeny::WalletUnavailable)
        );
        assert_eq!(
            can_give_flower(false, true, true, None, now, Some(&enough)),
            Err(GiveFlowerDeny::Pending)
        );
        assert_eq!(
            can_give_flower(false, true, false, Some(now), now, Some(&enough)),
            Err(GiveFlowerDeny::RateLimited)
        );
        assert_eq!(
            can_give_flower(
                false,
                true,
                false,
                Some(now - FLOWER_RATE_LIMIT - Duration::from_millis(1)),
                now,
                Some(&enough)
            ),
            Ok(())
        );
        assert_eq!(
            can_give_flower(false, true, false, None, now, Some("0")),
            Err(GiveFlowerDeny::Insufficient)
        );
        assert_eq!(can_give_flower(false, true, false, None, now, None), Ok(()));
        assert!(!flower_menu_blocked(false, None));
        assert!(flower_menu_blocked(true, None));
        assert!(flower_menu_blocked(false, Some("0")));
        assert!(!flower_menu_blocked(false, Some(&enough)));
        assert_eq!(
            can_give_flower(false, true, false, None, now, Some(&enough)),
            Ok(())
        );
    }

    #[test]
    fn build_flower_transfer_reads_flower_price() {
        let request = build_flower_transfer(
            "1".into(),
            "alice".into(),
            "2".into(),
            "10".into(),
            "20".into(),
        );
        assert_eq!(request.amount, flower_price());
        let extra = request.extra_info.expect("extra");
        assert_eq!(extra.transfer_type, TRANSFER_TYPE_TRANSFER_TOKEN);
        assert_eq!(extra.user_receiver_id.as_deref(), Some("2"));
        assert!(
            extra
                .extra_attribute
                .as_deref()
                .is_some_and(|attr| attr.contains("flower") && attr.contains("20"))
        );
    }

    #[test]
    fn flower_params_round_trip_and_reject_other_gifts() {
        let encoded = serialize_flower_interactive_params("9", 1);
        let parsed = parse_flower_interactive_params(&encoded).expect("params");
        assert_eq!(parsed.receiver_id, "9");
        assert_eq!(parsed.gift_type, "flower");
        assert_eq!(parsed.timestamp, 1);
        assert!(
            parse_flower_interactive_params(
                r#"{"receiver_id":"9","gift_type":"coffee","timestamp":1}"#
            )
            .is_none()
        );
        assert!(
            parse_flower_interactive_params(r#"{"gift_type":"flower","timestamp":1}"#).is_none()
        );
    }

    #[test]
    fn flower_event_from_payload_filters_channel_and_type() {
        let params = serialize_flower_interactive_params("20", 99);
        let applied = flower_event_from_payload(
            VoiceInteractiveEventType::Gift as i32,
            10,
            20,
            2,
            &params,
            2,
        )
        .expect("apply");
        assert_eq!(applied.0, "10");
        assert_eq!(applied.1, "20");
        assert_eq!(applied.2, 99);
        assert_eq!(applied.3, flower_effect_key("10", "20", 99));
        assert!(
            flower_event_from_payload(
                VoiceInteractiveEventType::Gift as i32,
                10,
                20,
                2,
                &params,
                3,
            )
            .is_none()
        );
        assert!(flower_event_from_payload(0, 10, 20, 2, &params, 2).is_none());
        assert!(
            flower_event_from_payload(
                VoiceInteractiveEventType::Recording as i32,
                10,
                20,
                2,
                &params,
                2,
            )
            .is_none()
        );
        assert!(
            flower_event_from_payload(
                VoiceInteractiveEventType::AppQuiz as i32,
                10,
                20,
                2,
                &params,
                2,
            )
            .is_none()
        );
    }

    fn gift(receiver_id: i64, params: &str) -> Option<(String, String, i64, String)> {
        flower_event_from_payload(
            VoiceInteractiveEventType::Gift as i32,
            10,
            receiver_id,
            2,
            params,
            2,
        )
    }

    #[test]
    fn a_sender_that_only_fills_the_event_fields_still_shows_a_flower() {
        for params in ["", "{}", "null", "not json", r#"{"giftId":"7"}"#] {
            let applied = gift(20, params).unwrap_or_else(|| panic!("params {params:?}"));
            assert_eq!(applied.1, "20", "params {params:?}");
            assert_eq!(applied.2, 0, "params {params:?}");
        }
        assert!(gift(0, "").is_none());
    }

    #[test]
    fn the_receiver_is_read_whatever_shape_the_sender_wrote_it_in() {
        for params in [
            r#"{"receiver_id":"20","gift_type":"flower","timestamp":99}"#,
            r#"{"receiverId":"20","giftType":"flower","timestamp":99}"#,
            r#"{"receiver_id":20,"gift_type":"flower","timestamp":99}"#,
        ] {
            let applied = gift(0, params).unwrap_or_else(|| panic!("params {params:?}"));
            assert_eq!(applied.1, "20", "params {params:?}");
            assert_eq!(applied.2, 99, "params {params:?}");
        }
        let no_timestamp = gift(0, r#"{"receiver_id":"20","gift_type":"flower"}"#).expect("gift");
        assert_eq!(no_timestamp.1, "20");
        assert_eq!(no_timestamp.2, 0);
    }

    #[test]
    fn a_gift_that_says_it_is_not_a_flower_is_still_refused() {
        let coffee = r#"{"receiver_id":"20","gift_type":"coffee","timestamp":99}"#;
        assert_eq!(classify_gift_params(coffee), GiftParams::OtherGift);
        assert!(gift(20, coffee).is_none());
    }

    #[test]
    fn voice_interactive_event_type_matches_js_enum() {
        assert_eq!(VoiceInteractiveEventType::Gift as i32, 1);
        assert_eq!(VoiceInteractiveEventType::Recording as i32, 2);
        assert_eq!(VoiceInteractiveEventType::AppQuiz as i32, 10);
        assert_eq!(VoiceInteractiveEventType::AppBlackboard as i32, 11);
        assert_eq!(VoiceInteractiveEventType::AppInteractive as i32, 12);
        assert_eq!(
            VoiceInteractiveEventType::from_i32(1),
            Some(VoiceInteractiveEventType::Gift)
        );
        assert_eq!(VoiceInteractiveEventType::from_i32(3), None);
        assert_eq!(
            VoiceInteractiveApp::Quiz.event_type(),
            VoiceInteractiveEventType::AppQuiz
        );
        assert_eq!(
            VoiceInteractiveApp::Blackboard.event_type(),
            VoiceInteractiveEventType::AppBlackboard
        );
        assert_eq!(
            VoiceInteractiveApp::Interactive.event_type(),
            VoiceInteractiveEventType::AppInteractive
        );
        assert_eq!(
            VoiceInteractiveApp::Quiz.app_id(),
            2_089_257_413_122_199_552
        );
        assert_eq!(
            VoiceInteractiveApp::Blackboard.app_id(),
            2_089_294_331_818_020_864
        );
        assert_eq!(
            VoiceInteractiveApp::Interactive.app_id(),
            2_089_273_739_668_623_360
        );
    }

    #[test]
    fn flower_scene_ttl_matches_web() {
        assert_eq!(FLOWER_SCENE_TTL, Duration::from_millis(4000));
    }

    #[test]
    fn flower_reaction_token_round_trips_the_receiver() {
        let token = flower_reaction_token("1826814768338440192");
        assert_eq!(token, "flower:1826814768338440192");
        assert_eq!(
            parse_flower_reaction_token(&token),
            Some("1826814768338440192")
        );
    }

    #[test]
    fn other_reaction_tokens_are_left_to_their_own_handlers() {
        assert_eq!(parse_flower_reaction_token("raising-up:12"), None);
        assert_eq!(parse_flower_reaction_token("sound:https://x.mp3"), None);
        assert_eq!(parse_flower_reaction_token(":smile:"), None);
        assert_eq!(parse_flower_reaction_token("flower:"), None);
        assert_eq!(parse_flower_reaction_token("flower:not-an-id"), None);
        assert_eq!(parse_flower_reaction_token(""), None);
        assert_eq!(parse_app_reaction_token("flower:12"), None);
        assert_eq!(parse_flower_reaction_token("app:quiz"), None);
    }

    #[test]
    fn every_app_round_trips_through_a_reaction_token() {
        for app in [
            VoiceInteractiveApp::Quiz,
            VoiceInteractiveApp::Blackboard,
            VoiceInteractiveApp::Interactive,
        ] {
            let token = app_reaction_token(app);
            assert_eq!(parse_app_reaction_token(&token), Some(app), "{app:?}");
        }
        assert_eq!(app_reaction_token(VoiceInteractiveApp::Quiz), "app:quiz");
        assert_eq!(parse_app_reaction_token("app:"), None);
        assert_eq!(parse_app_reaction_token("app:solitaire"), None);
        assert_eq!(parse_app_reaction_token("quiz"), None);
    }

    #[test]
    fn the_pair_key_ignores_the_timestamp_the_effect_key_carries() {
        assert_eq!(flower_pair_key("10", "20"), "10:20");
        assert_ne!(flower_pair_key("10", "20"), flower_pair_key("20", "10"));
        assert!(flower_effect_key("10", "20", 99).starts_with(&flower_pair_key("10", "20")));
    }

    #[test]
    fn uncertain_errors_are_network_or_timeout_only() {
        assert!(is_uncertain_transfer_error("request timeout after 15000ms"));
        assert!(is_uncertain_transfer_error("http error: connection reset"));
        assert!(!is_uncertain_transfer_error(
            "Amount exceeds wallet balance"
        ));
        assert!(!is_uncertain_transfer_error("JSON-RPC Error 1: rejected"));
    }
}
