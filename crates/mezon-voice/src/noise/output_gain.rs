// Fixed +3 dB makeup gain compensates modest speech attenuation without AGC.
const MAKEUP_GAIN: f32 = 1.412_537_6;
const PCM_SCALE: f32 = 32768.0;
// Start limiting at -3 dBFS and retain at least 1 dB of output headroom.
const LIMITER_KNEE: f32 = 0.707_945_76;
const LIMITER_CEILING: f32 = 0.891_250_9;

pub(super) fn apply(samples: &mut [i16]) {
    let headroom = LIMITER_CEILING - LIMITER_KNEE;
    for sample in samples {
        let amplified = *sample as f32 / PCM_SCALE * MAKEUP_GAIN;
        let magnitude = amplified.abs();
        // A continuous soft knee keeps ordinary speech linear. This limiter has
        // no envelope state, so neither frame boundaries nor resets change gain.
        let limited = if magnitude > LIMITER_KNEE {
            let excess = magnitude - LIMITER_KNEE;
            LIMITER_KNEE + headroom * excess / (headroom + excess)
        } else {
            magnitude
        };
        *sample = (limited.copysign(amplified) * PCM_SCALE).round() as i16;
    }
}
