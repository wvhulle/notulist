use rubato::{FftFixedIn, Resampler};

use crate::error::ResamplerError;

const CHUNK: usize = 1024;
const FLUSH_STEP_LIMIT: usize = 3;

pub struct StreamResampler {
    inner: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    ratio: f64,
    frames_in: u64,
    frames_out: u64,
}

impl StreamResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Result<Self, ResamplerError> {
        let inner = if from_rate == to_rate {
            None
        } else {
            Some(FftFixedIn::<f32>::new(
                from_rate as usize,
                to_rate as usize,
                CHUNK,
                1,
                1,
            )?)
        };
        Ok(Self {
            inner,
            pending: Vec::with_capacity(CHUNK * 2),
            ratio: f64::from(to_rate) / f64::from(from_rate),
            frames_in: 0,
            frames_out: 0,
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let Some(inner) = self.inner.as_mut() else {
            return samples.to_vec();
        };

        self.pending.extend_from_slice(samples);
        self.frames_in += samples.len() as u64;

        let mut out = Vec::new();
        while self.pending.len() >= CHUNK {
            let chunk: Vec<f32> = self.pending.drain(..CHUNK).collect();
            match inner.process(&[chunk], None) {
                Ok(mut converted) => {
                    if let Some(channel) = converted.pop() {
                        out.extend(channel);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Resampler step failed; dropping a chunk");
                }
            }
        }
        self.frames_out += out.len() as u64;
        out
    }

    pub fn flush(&mut self) -> Vec<f32> {
        let ratio = self.ratio;
        let Some(inner) = self.inner.as_mut() else {
            let rest = std::mem::take(&mut self.pending);
            self.frames_out += rest.len() as u64;
            return rest;
        };

        let expected = (self.frames_in as f64 * ratio).round() as u64;
        let owed = expected.saturating_sub(self.frames_out) as usize;
        if owed == 0 && self.pending.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(owed);
        let mut pending = std::mem::take(&mut self.pending);
        let mut steps = 0;
        while out.len() < owed && steps < FLUSH_STEP_LIMIT {
            steps += 1;
            pending.resize(CHUNK, 0.0);
            match inner.process(&[std::mem::take(&mut pending)], None) {
                Ok(mut converted) => {
                    if let Some(channel) = converted.pop() {
                        out.extend(channel);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Resampler flush failed; dropping the tail");
                    break;
                }
            }
        }

        out.truncate(owed);
        self.frames_out += out.len() as u64;
        out
    }
}

pub fn resample_buffer(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    match StreamResampler::new(from_rate, to_rate) {
        Ok(mut resampler) => {
            let mut out = resampler.push(samples);
            out.extend(resampler.flush());
            out
        }
        Err(error) => {
            tracing::error!(%error, "Passing audio through unconverted");
            samples.to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(freq: f32, rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn energy_at(samples: &[f32], freq: f32, rate: u32) -> f32 {
        let (re, im) = samples
            .iter()
            .enumerate()
            .fold((0.0f32, 0.0f32), |(re, im), (i, s)| {
                let phase = TAU * freq * i as f32 / rate as f32;
                (re + s * phase.cos(), im + s * phase.sin())
            });
        ((re * re + im * im).sqrt()) / samples.len() as f32
    }

    #[test]
    fn downsampling_does_not_fold_energy_above_nyquist() {
        let input = tone(12_000.0, 48_000, 48_000);
        let out = resample_buffer(&input, 48_000, 16_000);

        let alias = energy_at(&out, 4_000.0, 16_000);
        let reference = energy_at(&input, 12_000.0, 48_000);

        assert!(
            alias < reference * 0.1,
            "12kHz folded to 4kHz at {alias:.4} against a source amplitude of \
             {reference:.4}; the anti-aliasing filter is not working"
        );
    }

    #[test]
    fn downsampling_preserves_audible_speech_band() {
        let input = tone(1_000.0, 48_000, 48_000);
        let out = resample_buffer(&input, 48_000, 16_000);

        assert!(
            energy_at(&out, 1_000.0, 16_000) > 0.2,
            "a 1kHz tone should pass through largely intact"
        );
    }

    #[test]
    fn streaming_matches_one_shot_length() {
        let input = tone(440.0, 48_000, 48_000);

        let one_shot = resample_buffer(&input, 48_000, 16_000);

        let mut resampler = StreamResampler::new(48_000, 16_000).unwrap();
        let mut streamed = Vec::new();
        input
            .chunks(577)
            .for_each(|block| streamed.extend(resampler.push(block)));
        streamed.extend(resampler.flush());

        let diff = (one_shot.len() as i64 - streamed.len() as i64).abs();
        assert!(
            diff <= CHUNK as i64,
            "chunked and one-shot lengths diverged by {diff} samples"
        );
    }

    #[test]
    fn flush_drains_the_delay_line() {
        let mut resampler = StreamResampler::new(48_000, 16_000).unwrap();
        assert!(
            resampler.push(&vec![0.5; 300]).is_empty(),
            "a sub-chunk push cannot have produced output yet"
        );
        let tail = resampler.flush();
        assert_eq!(
            tail.len(),
            100,
            "300 frames at 3:1 owe 100 samples, all of which flush must return"
        );

        let mut resampler = StreamResampler::new(48_000, 16_000).unwrap();
        let pushed: usize = (0..4)
            .map(|_| resampler.push(&vec![0.25; CHUNK]).len())
            .sum();
        let total = pushed + resampler.flush().len();
        let expected = (4 * CHUNK) / 3;
        assert!(
            (total as i64 - expected as i64).abs() <= 2,
            "4 chunks in should yield about {expected} out, got {total}"
        );
    }

    #[test]
    fn matching_rates_pass_through_untouched() {
        let input = tone(440.0, 16_000, 4_000);
        assert_eq!(resample_buffer(&input, 16_000, 16_000), input);

        let mut resampler = StreamResampler::new(16_000, 16_000).unwrap();
        assert_eq!(resampler.push(&input), input);
        assert!(resampler.flush().is_empty());
    }

    #[test]
    fn upsampling_produces_proportionally_more_samples() {
        let input = tone(440.0, 8_000, 8_000);
        let out = resample_buffer(&input, 8_000, 16_000);
        let expected = input.len() * 2;
        assert!(
            (out.len() as i64 - expected as i64).abs() <= CHUNK as i64,
            "8k -> 16k produced {} samples, expected about {expected}",
            out.len()
        );
    }

    #[test]
    fn empty_and_invalid_inputs_are_handled() {
        assert!(resample_buffer(&[], 48_000, 16_000).is_empty());
        assert!(StreamResampler::new(0, 16_000).is_err());
        assert!(StreamResampler::new(48_000, 0).is_err());
    }
}
