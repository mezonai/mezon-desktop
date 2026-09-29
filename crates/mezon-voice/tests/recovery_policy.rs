// Run recovery policy tests without building GPUI or native WebRTC:
// rustc --edition=2024 --test crates/mezon-voice/tests/recovery_policy.rs -o /tmp/mezon-sfu-recovery-tests
// /tmp/mezon-sfu-recovery-tests
#[path = "../src/playback_health.rs"]
mod playback_health;

#[path = "../src/reconnect.rs"]
mod reconnect;
