use crate::{AudioError, DecodedPcm};

pub const PROCESSED_SOUND_SAMPLE_RATE: u32 = 22_050;

pub fn estimated_mono_wav_bytes(duration_secs: f64) -> Option<u64> {
    let samples = (duration_secs.max(0.0) * f64::from(PROCESSED_SOUND_SAMPLE_RATE)).ceil() as u64;
    samples.checked_mul(2)?.checked_add(44)
}

pub fn process_pcm_to_wav(
    pcm: &DecodedPcm,
    start_secs: f64,
    end_secs: f64,
    volume: f32,
) -> Result<Vec<u8>, AudioError> {
    let processed = process_pcm(pcm, start_secs, end_secs, volume)?;
    let processed = downsample_mono(&processed, PROCESSED_SOUND_SAMPLE_RATE)?;
    encode_mono_wav(&processed)
}

pub fn process_pcm(
    pcm: &DecodedPcm,
    start_secs: f64,
    end_secs: f64,
    volume: f32,
) -> Result<DecodedPcm, AudioError> {
    if pcm.channels == 0 || pcm.sample_rate == 0 || pcm.samples.is_empty() {
        return Err(AudioError::Decode("invalid pcm data".into()));
    }

    let frames = pcm.frames();
    let start_frame = (start_secs.max(0.0) * pcm.sample_rate as f64).floor() as usize;
    let end_frame = (end_secs.max(start_secs) * pcm.sample_rate as f64).ceil() as usize;
    let start_frame = start_frame.min(frames);
    let end_frame = end_frame.min(frames);
    if start_frame >= end_frame {
        return Err(AudioError::Decode("empty audio selection".into()));
    }

    let gain = volume.max(0.0);
    let mut samples = Vec::with_capacity(end_frame - start_frame);
    for frame in start_frame..end_frame {
        let offset = frame * pcm.channels;
        let mono = pcm.samples[offset..offset + pcm.channels]
            .iter()
            .copied()
            .sum::<f32>()
            / pcm.channels as f32;
        samples.push((mono * gain).clamp(-1.0, 1.0));
    }
    Ok(DecodedPcm {
        samples: samples.into(),
        channels: 1,
        sample_rate: pcm.sample_rate,
    })
}

fn downsample_mono(pcm: &DecodedPcm, output_rate: u32) -> Result<DecodedPcm, AudioError> {
    if pcm.channels != 1 || pcm.sample_rate == 0 || output_rate == 0 {
        return Err(AudioError::Decode("invalid pcm data".into()));
    }
    if pcm.sample_rate <= output_rate {
        return Ok(DecodedPcm {
            samples: pcm.samples.clone(),
            channels: 1,
            sample_rate: pcm.sample_rate,
        });
    }

    let ratio = f64::from(pcm.sample_rate) / f64::from(output_rate);
    let output_len = (pcm.samples.len() as f64 / ratio).floor() as usize;
    if output_len == 0 {
        return Err(AudioError::Decode("empty audio selection".into()));
    }
    let mut samples = Vec::with_capacity(output_len);
    for index in 0..output_len {
        let start = (index as f64 * ratio).floor() as usize;
        let end = (((index + 1) as f64 * ratio).floor() as usize)
            .max(start + 1)
            .min(pcm.samples.len());
        let sum = pcm.samples[start..end].iter().copied().sum::<f32>();
        samples.push(sum / (end - start) as f32);
    }
    Ok(DecodedPcm {
        samples: samples.into(),
        channels: 1,
        sample_rate: output_rate,
    })
}

fn encode_mono_wav(pcm: &DecodedPcm) -> Result<Vec<u8>, AudioError> {
    let data_len = pcm
        .samples
        .len()
        .checked_mul(std::mem::size_of::<i16>())
        .and_then(|len| u32::try_from(len).ok())
        .ok_or_else(|| AudioError::Decode("audio selection is too large".into()))?;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    wav.extend_from_slice(&(pcm.sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm.samples.iter() {
        let encoded = (sample * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&encoded.to_le_bytes());
    }
    Ok(wav)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_pcm_to_wav_trims_mixes_and_applies_gain() {
        let pcm = DecodedPcm {
            samples: vec![0.25, 0.75, -0.5, 0.5, 0.4, 0.8, -1.0, -0.5].into(),
            channels: 2,
            sample_rate: 2,
        };
        let wav = process_pcm_to_wav(&pcm, 0.5, 1.5, 0.5).expect("processed wav");

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        let samples = wav[44..]
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        assert_eq!(samples, vec![0, 9830]);
    }

    #[test]
    fn process_pcm_to_wav_rejects_empty_selection() {
        let pcm = DecodedPcm {
            samples: vec![0.0, 0.0].into(),
            channels: 1,
            sample_rate: 2,
        };
        assert!(process_pcm_to_wav(&pcm, 0.5, 0.5, 1.0).is_err());
    }

    #[test]
    fn processed_wav_size_is_bounded_for_ten_seconds() {
        assert_eq!(estimated_mono_wav_bytes(10.0), Some(441_044));
    }

    #[test]
    fn processed_wav_downsamples_to_target_rate() {
        let pcm = DecodedPcm {
            samples: vec![0.5; 44_100].into(),
            channels: 1,
            sample_rate: 44_100,
        };
        let wav = process_pcm_to_wav(&pcm, 0.0, 1.0, 1.0).expect("processed wav");
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            PROCESSED_SOUND_SAMPLE_RATE
        );
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 44_100);
    }
}
