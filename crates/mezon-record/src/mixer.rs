use std::collections::VecDeque;

use crate::sink::{RECORD_CHANNELS, RECORD_SAMPLE_RATE};

const MAX_BUFFERED_FRAMES: usize = RECORD_SAMPLE_RATE as usize * 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioSource {
    Remote,
    Mic,
    Screen,
}

struct Resampler {
    rate: u32,
    channels: u32,
    pos: f64,
    prev: [i16; 2],
    primed: bool,
}

impl Resampler {
    fn new() -> Self {
        Self {
            rate: RECORD_SAMPLE_RATE,
            channels: RECORD_CHANNELS,
            pos: 0.0,
            prev: [0, 0],
            primed: false,
        }
    }

    fn reconfigure(&mut self, rate: u32, channels: u32) {
        if self.rate == rate && self.channels == channels {
            return;
        }
        self.rate = rate.max(1);
        self.channels = channels.max(1);
        self.pos = 0.0;
        self.prev = [0, 0];
        self.primed = false;
    }

    fn resample_into(&mut self, samples: &[i16], out: &mut VecDeque<i16>) {
        let channels = self.channels as usize;
        let frame_count = samples.len() / channels;
        if frame_count == 0 {
            return;
        }
        let frame_at = |index: usize| -> [i16; 2] {
            let base = index * channels;
            match channels {
                1 => [samples[base], samples[base]],
                _ => [samples[base], samples[base + 1]],
            }
        };

        if !self.primed {
            self.primed = true;
            self.pos = 0.0;
            self.prev = frame_at(0);
        }

        if self.rate == RECORD_SAMPLE_RATE {
            for index in 0..frame_count {
                let frame = frame_at(index);
                out.push_back(frame[0]);
                out.push_back(frame[1]);
            }
            self.prev = frame_at(frame_count - 1);
            return;
        }

        let sample_at = |index: isize| -> [i16; 2] {
            if index < 0 {
                self.prev
            } else {
                frame_at(index as usize)
            }
        };

        let step = self.rate as f64 / RECORD_SAMPLE_RATE as f64;
        let last_index = frame_count as isize - 2;
        let mut pos = self.pos;
        while (pos.floor() as isize) <= last_index {
            let index = pos.floor() as isize;
            let frac = pos - pos.floor();
            let a = sample_at(index);
            let b = sample_at(index + 1);
            for channel in 0..2 {
                let lerped = a[channel] as f64 + (b[channel] as f64 - a[channel] as f64) * frac;
                out.push_back(lerped.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
            }
            pos += step;
        }
        self.pos = pos - frame_count as f64;
        self.prev = frame_at(frame_count - 1);
    }
}

struct Track {
    resampler: Resampler,
    buf: VecDeque<i16>,
}

impl Track {
    fn new() -> Self {
        Self {
            resampler: Resampler::new(),
            buf: VecDeque::new(),
        }
    }

    fn push(&mut self, samples: &[i16], rate: u32, channels: u32) {
        self.resampler.reconfigure(rate, channels);
        self.resampler.resample_into(samples, &mut self.buf);
        let cap = MAX_BUFFERED_FRAMES * RECORD_CHANNELS as usize;
        while self.buf.len() > cap {
            self.buf.pop_front();
        }
    }

    /// Returns how many samples this track actually contributed, so a source
    /// that never reaches the mix can be told apart from one the mixer dropped.
    fn mix_into(&mut self, out: &mut [i16]) -> u64 {
        let mut mixed = 0;
        for slot in out.iter_mut() {
            let Some(sample) = self.buf.pop_front() else {
                break;
            };
            *slot = (*slot as i32 + sample as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            mixed += 1;
        }
        mixed
    }

    fn frames(&self) -> u64 {
        (self.buf.len() / RECORD_CHANNELS as usize) as u64
    }
}

/// Frames each source contributed to the recording, in the order remote, mic,
/// screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Contributions {
    pub remote: u64,
    pub mic: u64,
    pub screen: u64,
}

pub struct AudioMixer {
    remote: Track,
    mic: Track,
    screen: Track,
    mixed: Contributions,
}

impl AudioMixer {
    pub fn new() -> Self {
        Self {
            remote: Track::new(),
            mic: Track::new(),
            screen: Track::new(),
            mixed: Contributions::default(),
        }
    }

    pub fn push(&mut self, source: AudioSource, samples: &[i16], rate: u32, channels: u32) {
        let track = match source {
            AudioSource::Remote => &mut self.remote,
            AudioSource::Mic => &mut self.mic,
            AudioSource::Screen => &mut self.screen,
        };
        track.push(samples, rate, channels);
    }

    pub fn remote_frames(&self) -> u64 {
        self.remote.frames()
    }

    pub fn drain_block(&mut self, frames: usize, out: &mut Vec<i16>) {
        out.clear();
        out.resize(frames * RECORD_CHANNELS as usize, 0);
        let channels = RECORD_CHANNELS as u64;
        self.mixed.remote += self.remote.mix_into(out) / channels;
        self.mixed.mic += self.mic.mix_into(out) / channels;
        self.mixed.screen += self.screen.mix_into(out) / channels;
    }

    pub fn contributions(&self) -> Contributions {
        self.mixed
    }
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(samples: &[i16]) -> Vec<i16> {
        samples.iter().flat_map(|s| [*s, *s]).collect()
    }

    #[test]
    fn passthrough_keeps_sample_count_and_values() {
        let mut mixer = AudioMixer::new();
        let input = stereo(&[100, -100, 200, -200]);
        mixer.push(AudioSource::Remote, &input, 48_000, 2);
        assert_eq!(mixer.remote_frames(), 4);

        let mut out = Vec::new();
        mixer.drain_block(4, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn mono_input_is_duplicated_across_both_channels() {
        let mut mixer = AudioMixer::new();
        mixer.push(AudioSource::Remote, &[500, -500], 48_000, 1);

        let mut out = Vec::new();
        mixer.drain_block(2, &mut out);
        assert_eq!(out, vec![500, 500, -500, -500]);
    }

    #[test]
    fn upsampling_from_24k_doubles_the_frame_count() {
        let mut mixer = AudioMixer::new();
        for _ in 0..10 {
            mixer.push(AudioSource::Remote, &stereo(&[0; 240]), 24_000, 2);
        }
        assert!(mixer.remote_frames().abs_diff(4_800) <= 4);
    }

    #[test]
    fn downsampling_from_96k_halves_the_frame_count() {
        let mut mixer = AudioMixer::new();
        for _ in 0..10 {
            mixer.push(AudioSource::Remote, &stereo(&[0; 960]), 96_000, 2);
        }
        assert!(mixer.remote_frames().abs_diff(4_800) <= 4);
    }

    #[test]
    fn resampling_keeps_rate_stable_across_many_chunks() {
        let mut mixer = AudioMixer::new();
        for _ in 0..100 {
            mixer.push(AudioSource::Remote, &stereo(&[0; 441]), 44_100, 2);
        }
        let expected = (441.0 * 100.0 * 48_000.0 / 44_100.0) as u64;
        assert!(mixer.remote_frames().abs_diff(expected) <= 2);
    }

    #[test]
    fn sources_are_summed_and_clamped() {
        let mut mixer = AudioMixer::new();
        mixer.push(AudioSource::Remote, &[30_000, 30_000], 48_000, 2);
        mixer.push(AudioSource::Mic, &[10_000, 10_000], 48_000, 2);

        let mut out = Vec::new();
        mixer.drain_block(1, &mut out);
        assert_eq!(out, vec![i16::MAX, i16::MAX]);
    }

    #[test]
    fn a_short_source_is_padded_with_silence_not_dropped() {
        let mut mixer = AudioMixer::new();
        mixer.push(AudioSource::Remote, &stereo(&[100; 4]), 48_000, 2);
        mixer.push(AudioSource::Mic, &stereo(&[50; 2]), 48_000, 2);

        let mut out = Vec::new();
        mixer.drain_block(4, &mut out);
        assert_eq!(out, vec![150, 150, 150, 150, 100, 100, 100, 100]);
    }

    #[test]
    fn buffers_are_bounded_so_a_stalled_writer_cannot_grow_memory() {
        let mut mixer = AudioMixer::new();
        for _ in 0..20 {
            mixer.push(AudioSource::Remote, &stereo(&[1; 48_000]), 48_000, 2);
        }
        assert!(mixer.remote_frames() <= MAX_BUFFERED_FRAMES as u64);
    }
}
