use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfuCloseAction {
    Stop,
    RefreshToken,
    ResetTransport,
    Retry,
}

pub fn sfu_close_action(code: Option<u16>) -> SfuCloseAction {
    match code {
        Some(1000 | 4006 | 4011 | 4012) => SfuCloseAction::Stop,
        Some(4003 | 4004 | 4005) => SfuCloseAction::RefreshToken,
        Some(4013) => SfuCloseAction::ResetTransport,
        _ => SfuCloseAction::Retry,
    }
}

/// Zero-based attempts, equal jitter: 0.5–1 / 1–2 / 2–4 / 4–8 seconds.
pub fn sfu_reconnect_delay(attempt: u32) -> Duration {
    // RandomState uses independently seeded hashes; no shared reconnect cadence.
    reconnect_delay_with_jitter(attempt, RandomState::new().build_hasher().finish())
}

fn reconnect_delay_with_jitter(attempt: u32, jitter: u64) -> Duration {
    let ceiling = 1_000u64 << attempt.min(3);
    let half = ceiling / 2;
    Duration::from_millis(half + jitter % (half + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_contract() {
        for code in [1000, 4006, 4011, 4012] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::Stop);
        }
        for code in [4003, 4004, 4005] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::RefreshToken);
        }
        assert_eq!(sfu_close_action(Some(4013)), SfuCloseAction::ResetTransport);
        for code in [1001, 1006, 1011, 4001, 4008, 4999] {
            assert_eq!(sfu_close_action(Some(code)), SfuCloseAction::Retry);
        }
        assert_eq!(sfu_close_action(None), SfuCloseAction::Retry);
    }
    #[test]
    fn backoff_is_exponential_bounded_and_jittered() {
        for attempt in 0..32 {
            let cap = 1_000u64 << attempt.min(3);
            assert_eq!(
                reconnect_delay_with_jitter(attempt, 0).as_millis(),
                u128::from(cap / 2)
            );
            assert_eq!(
                reconnect_delay_with_jitter(attempt, cap / 2).as_millis(),
                u128::from(cap)
            );
            for _ in 0..20 {
                assert!(
                    (u128::from(cap / 2)..=u128::from(cap))
                        .contains(&sfu_reconnect_delay(attempt).as_millis())
                );
            }
        }
    }
}
