use std::f64::consts::PI;

use crate::{MezonError, MezonNSEngine, FRAME_SIZE, FRAME_SIZE_48K};

const TAPS: usize = 120;
const PHASE_TAPS: usize = TAPS / 3;

/// Stateful 48 kHz mono adapter for the upstream 16 kHz / 10 ms engine.
/// The Kaiser FIR coefficients and polyphase layout match mezon-ns's desktop adapter.
pub struct Mezon48k {
    engine: MezonNSEngine,
    taps: [f32; TAPS],
    down_history: [f32; TAPS - 1],
    up_history: [f32; PHASE_TAPS - 1],
}

impl Mezon48k {
    pub fn new(engine: MezonNSEngine) -> Self {
        let mut taps = [0.0; TAPS];
        let center = (TAPS - 1) as f64 / 2.0;
        let cutoff = 7200.0 / 48000.0;
        let norm = bessel_i0(5.0);
        let mut sum = 0.0;
        for (n, tap) in taps.iter_mut().enumerate() {
            let offset = n as f64 - center;
            let ratio = offset / center;
            let window = bessel_i0(5.0 * (1.0 - ratio * ratio).max(0.0).sqrt()) / norm;
            let x = 2.0 * cutoff * offset;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let value = 2.0 * cutoff * sinc * window;
            *tap = value as f32;
            sum += value;
        }
        for tap in &mut taps {
            *tap = (*tap as f64 / sum) as f32;
        }
        Self {
            engine,
            taps,
            down_history: [0.0; TAPS - 1],
            up_history: [0.0; PHASE_TAPS - 1],
        }
    }

    pub fn set_suppression_intensity(&mut self, value: f32) {
        self.engine.set_suppression_intensity(value);
    }

    pub fn reset(&mut self) {
        self.engine.reset();
        self.down_history.fill(0.0);
        self.up_history.fill(0.0);
    }

    pub fn process_frame(&mut self, frame: &mut [i16]) -> Result<(), MezonError> {
        if frame.len() != FRAME_SIZE_48K {
            return Err(MezonError::InvalidFrameSize {
                expected: FRAME_SIZE_48K,
                actual: frame.len(),
            });
        }
        let mut down_work = [0.0f32; TAPS - 1 + FRAME_SIZE_48K];
        down_work[..TAPS - 1].copy_from_slice(&self.down_history);
        for (out, input) in down_work[TAPS - 1..].iter_mut().zip(frame.iter()) {
            *out = *input as f32 / 32768.0;
        }
        let mut narrow_in = [0.0f32; FRAME_SIZE];
        for (m, out) in narrow_in.iter_mut().enumerate() {
            let at = TAPS - 1 + 3 * m;
            *out = (0..TAPS).map(|k| self.taps[k] * down_work[at - k]).sum();
        }
        self.down_history
            .copy_from_slice(&down_work[FRAME_SIZE_48K..]);

        let mut narrow_out = [0.0f32; FRAME_SIZE];
        self.engine
            .process_frame_float(&narrow_in, &mut narrow_out)?;

        let mut up_work = [0.0f32; PHASE_TAPS - 1 + FRAME_SIZE];
        up_work[..PHASE_TAPS - 1].copy_from_slice(&self.up_history);
        up_work[PHASE_TAPS - 1..].copy_from_slice(&narrow_out);
        for q in 0..FRAME_SIZE {
            let at = PHASE_TAPS - 1 + q;
            for p in 0..3 {
                let value: f32 = (0..PHASE_TAPS)
                    .map(|j| self.taps[3 * j + p] * up_work[at - j])
                    .sum();
                frame[3 * q + p] = (value * 3.0 * 32768.0)
                    .round()
                    .clamp(i16::MIN as f32, i16::MAX as f32)
                    as i16;
            }
        }
        self.up_history.copy_from_slice(&up_work[FRAME_SIZE..]);
        Ok(())
    }
}

fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..64 {
        term *= (half / k as f64).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}
