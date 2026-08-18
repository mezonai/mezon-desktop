use std::time::{Duration, Instant};

use mmn_client::{DECIMALS, ExtraInfo, TRANSFER_TYPE_TRANSFER_TOKEN, scale_amount_to_decimals};
use serde::{Deserialize, Serialize};

use crate::wallet::SendTokenRequest;

pub const FLOWER_PRICE: i64 = 50_000;
pub const FLOWER_GIFT_TYPE: &str = "flower";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum VoiceInteractiveEventType {
    Gift = 1,
    Recording = 2,
    AppKahoot = 10,
    AppBlackboard = 11,
    AppSlido = 12,
}

impl VoiceInteractiveEventType {
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Gift),
            2 => Some(Self::Recording),
            10 => Some(Self::AppKahoot),
            11 => Some(Self::AppBlackboard),
            12 => Some(Self::AppSlido),
            _ => None,
        }
    }
}
pub const FLOWER_RATE_LIMIT: Duration = Duration::from_secs(1);
pub const FLOWER_ANIMATION_TTL: Duration = Duration::from_secs(7);
pub const FLOWER_PARTICLE_COUNT: usize = 128;
pub const FLOWER_PALETTE_SIZE: u8 = 14;
pub const FLOWER_SPRITE_COUNT: u8 = 14;

const FLOWER_BURST_SECS: f32 = 0.4;
const FLOWER_GROW_SECS: f32 = 2.0;
const FLOWER_SCALE_START: f32 = 1.0;
const FLOWER_SCALE_END: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowerParticle {
    pub sprite: u8,
    pub angle: f32,
    pub speed: f32,
    pub gravity: f32,
    pub spin0: f32,
    pub spin_vel: f32,
    pub size: f32,
    pub delay: f32,
    pub palette: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowerParticlePose {
    pub x: f32,
    pub y: f32,
    pub spin: f32,
    pub opacity: f32,
    pub scale: f32,
}

fn particle_rng(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((*state >> 33) as f32) * (1.0 / ((1u32 << 31) as f32))
}

pub fn flower_particles(seed: u64) -> Vec<FlowerParticle> {
    let mut state = seed | 1;
    (0..FLOWER_PARTICLE_COUNT)
        .map(|index| {
            let sprite = (index % usize::from(FLOWER_SPRITE_COUNT)) as u8;
            let angle = std::f32::consts::TAU * (index as f32 + particle_rng(&mut state))
                / FLOWER_PARTICLE_COUNT as f32;
            let speed = 0.24 + particle_rng(&mut state) * 0.28;
            let size = 24.0 + particle_rng(&mut state) * 8.0;
            FlowerParticle {
                sprite,
                angle,
                speed,
                gravity: 0.045 + particle_rng(&mut state) * 0.08,
                spin0: particle_rng(&mut state) * std::f32::consts::TAU,
                spin_vel: (particle_rng(&mut state) - 0.5) * 8.0,
                size,
                delay: particle_rng(&mut state) * 0.12,
                palette: (particle_rng(&mut state) * f32::from(FLOWER_PALETTE_SIZE)) as u8
                    % FLOWER_PALETTE_SIZE,
            }
        })
        .collect()
}

pub fn flower_particle_pose(
    particle: &FlowerParticle,
    elapsed: f32,
    ttl: f32,
) -> FlowerParticlePose {
    let t = (elapsed - particle.delay).max(0.0);
    let explode = 1.0 - (-t * 5.5).exp();
    let fade_in = (t / 0.12).min(1.0);
    let fade_out = if ttl > 0.0 && elapsed > ttl - 1.0 {
        ((ttl - elapsed) / 1.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let grow_span = FLOWER_BURST_SECS + FLOWER_GROW_SECS;
    let u = if grow_span > 0.0 {
        (t / grow_span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let smooth = u * u * (3.0 - 2.0 * u);
    let scale = FLOWER_SCALE_START + (FLOWER_SCALE_END - FLOWER_SCALE_START) * smooth;
    FlowerParticlePose {
        x: particle.angle.cos() * particle.speed * explode,
        y: particle.angle.sin() * particle.speed * explode + particle.gravity * t * t,
        spin: particle.spin0 + particle.spin_vel * t,
        opacity: fade_in * fade_out,
        scale,
    }
}

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

pub fn parse_flower_interactive_params(params: &str) -> Option<FlowerInteractiveParams> {
    let parsed: FlowerInteractiveParams = serde_json::from_str(params).ok()?;
    if parsed.gift_type != FLOWER_GIFT_TYPE || parsed.receiver_id.is_empty() {
        return None;
    }
    Some(parsed)
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

pub fn flower_event_from_payload(
    event_type: i32,
    giver_id: i64,
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
    let parsed = parse_flower_interactive_params(params)?;
    let giver = giver_id.to_string();
    let key = flower_effect_key(&giver, &parsed.receiver_id, parsed.timestamp);
    Some((giver, parsed.receiver_id, parsed.timestamp, key))
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
        FLOWER_ANIMATION_TTL, FLOWER_PALETTE_SIZE, FLOWER_PARTICLE_COUNT, FLOWER_PRICE,
        FLOWER_RATE_LIMIT, FLOWER_SPRITE_COUNT, FlowerParticle, GiveFlowerDeny,
        VoiceInteractiveEventType, build_flower_transfer, can_afford, can_give_flower,
        flower_effect_key, flower_event_from_payload, flower_menu_blocked, flower_particle_pose,
        flower_particles, flower_price, format_flower_amount, is_uncertain_transfer_error,
        parse_flower_interactive_params, serialize_flower_interactive_params,
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
        let applied =
            flower_event_from_payload(VoiceInteractiveEventType::Gift as i32, 10, 2, &params, 2)
                .expect("apply");
        assert_eq!(applied.0, "10");
        assert_eq!(applied.1, "20");
        assert_eq!(applied.2, 99);
        assert_eq!(applied.3, flower_effect_key("10", "20", 99));
        assert!(
            flower_event_from_payload(VoiceInteractiveEventType::Gift as i32, 10, 2, &params, 3,)
                .is_none()
        );
        assert!(flower_event_from_payload(0, 10, 2, &params, 2).is_none());
        assert!(
            flower_event_from_payload(
                VoiceInteractiveEventType::Recording as i32,
                10,
                2,
                &params,
                2,
            )
            .is_none()
        );
        assert!(
            flower_event_from_payload(
                VoiceInteractiveEventType::AppKahoot as i32,
                10,
                2,
                &params,
                2,
            )
            .is_none()
        );
    }

    #[test]
    fn voice_interactive_event_type_matches_js_enum() {
        assert_eq!(VoiceInteractiveEventType::Gift as i32, 1);
        assert_eq!(VoiceInteractiveEventType::Recording as i32, 2);
        assert_eq!(VoiceInteractiveEventType::AppKahoot as i32, 10);
        assert_eq!(VoiceInteractiveEventType::AppBlackboard as i32, 11);
        assert_eq!(VoiceInteractiveEventType::AppSlido as i32, 12);
        assert_eq!(
            VoiceInteractiveEventType::from_i32(1),
            Some(VoiceInteractiveEventType::Gift)
        );
        assert_eq!(VoiceInteractiveEventType::from_i32(3), None);
    }

    #[test]
    fn flower_particles_are_deterministic_burst() {
        let first = flower_particles(42);
        let second = flower_particles(42);
        assert_eq!(first, second);
        assert_eq!(first.len(), FLOWER_PARTICLE_COUNT);
        assert_eq!(FLOWER_PARTICLE_COUNT, 128);
        assert_eq!(FLOWER_ANIMATION_TTL.as_secs(), 7);
        let sprites = first
            .iter()
            .map(|particle| particle.sprite)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(sprites.len(), usize::from(FLOWER_SPRITE_COUNT));
        assert!(first.iter().all(|particle| {
            particle.sprite < FLOWER_SPRITE_COUNT && (24.0..32.01).contains(&particle.size)
        }));
        assert!(
            first
                .iter()
                .all(|particle| particle.palette < FLOWER_PALETTE_SIZE)
        );
        let flower_palettes = first
            .iter()
            .map(|particle| particle.palette)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(flower_palettes.len() >= 4);
        let flower_span = first
            .iter()
            .map(|particle| particle.angle)
            .fold((f32::MAX, f32::MIN), |(min, max), angle| {
                (min.min(angle), max.max(angle))
            });
        assert!(flower_span.1 - flower_span.0 > std::f32::consts::PI);
        let max_run = first
            .windows(2)
            .fold((1_usize, 1_usize), |(best, run), pair| {
                if pair[0].sprite == pair[1].sprite {
                    (best.max(run + 1), run + 1)
                } else {
                    (best, 1)
                }
            });
        assert!(max_run.0 <= 2);
        assert_ne!(flower_particles(1)[0].angle, flower_particles(2)[0].angle);
    }

    #[test]
    fn flower_particle_moves_outward_then_falls() {
        let particle = FlowerParticle {
            sprite: 1,
            angle: 0.0,
            speed: 0.4,
            gravity: 0.1,
            spin0: 0.0,
            spin_vel: 1.0,
            size: 12.0,
            delay: 0.0,
            palette: 0,
        };
        let ttl = FLOWER_ANIMATION_TTL.as_secs_f32();
        let start = flower_particle_pose(&particle, 0.0, ttl);
        let early = flower_particle_pose(&particle, 0.08, ttl);
        let after_burst = flower_particle_pose(&particle, 0.45, ttl);
        let mid = flower_particle_pose(&particle, 2.5, ttl);
        let late = flower_particle_pose(&particle, ttl - 0.4, ttl);
        assert!(start.opacity < 0.15);
        assert!(early.x > start.x);
        assert!(mid.x > early.x);
        assert!(mid.y > early.y);
        assert!(mid.scale > after_burst.scale + 0.3);
        assert!(after_burst.scale > early.scale);
        assert!(mid.scale > 1.4);
        assert!(late.opacity < 0.5);
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
