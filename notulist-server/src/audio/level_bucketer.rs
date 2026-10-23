use notulist_client::LevelFrame;

pub const LEVEL_FRAMES_PER_SECOND: u32 = 100;

const SILENCE_DBFS: f64 = -120.0;

pub struct LevelBucketer {
    samples_per_frame: usize,
    accumulated: usize,
    min: f32,
    max: f32,
    peak_abs: f32,
    seq: u32,
}

impl LevelBucketer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            samples_per_frame: (sample_rate / LEVEL_FRAMES_PER_SECOND).max(1) as usize,
            accumulated: 0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            peak_abs: 0.0,
            seq: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32], completed_frames: &mut Vec<LevelFrame>) {
        samples.iter().for_each(|&sample| {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
            self.peak_abs = self.peak_abs.max(sample.abs());
            self.accumulated += 1;
            if self.accumulated >= self.samples_per_frame {
                completed_frames.push(self.take_frame());
            }
        });
    }

    fn take_frame(&mut self) -> LevelFrame {
        let peak_dbfs = if self.peak_abs <= 1e-6 {
            SILENCE_DBFS
        } else {
            20.0 * f64::from(self.peak_abs).log10()
        };
        let frame = LevelFrame {
            seq: self.seq,
            min: f64::from(self.min),
            max: f64::from(self.max),
            peak_dbfs,
        };
        self.seq = self.seq.wrapping_add(1);
        self.accumulated = 0;
        self.min = f32::INFINITY;
        self.max = f32::NEG_INFINITY;
        self.peak_abs = 0.0;
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES_PER_FRAME: usize = 160;

    fn frames_for(samples: &[f32]) -> Vec<LevelFrame> {
        let mut bucketer = LevelBucketer::new(16_000);
        let mut frames = Vec::new();
        bucketer.push(samples, &mut frames);
        frames
    }

    #[test]
    fn emits_one_frame_per_ten_milliseconds() {
        let frames = frames_for(&[0.5; 1600]);
        assert_eq!(frames.len(), 10);
        assert_eq!(frames[9].seq, 9);
        assert!((frames[0].max - 0.5).abs() < 1e-6);
        assert!((frames[0].min - 0.5).abs() < 1e-6);
    }

    #[test]
    fn incomplete_window_waits_for_more_samples() {
        let mut bucketer = LevelBucketer::new(16_000);
        let mut frames = Vec::new();
        bucketer.push(&[0.1; 100], &mut frames);
        assert!(frames.is_empty());
        bucketer.push(&[0.2; 60], &mut frames);
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn silence_and_full_scale_map_to_dbfs_extremes() {
        assert_eq!(
            frames_for(&[0.0; SAMPLES_PER_FRAME])[0].peak_dbfs,
            SILENCE_DBFS
        );
        let mut full_scale = vec![0.0; SAMPLES_PER_FRAME];
        full_scale[42] = 1.0;
        assert!(frames_for(&full_scale)[0].peak_dbfs.abs() < 1e-3);
    }

    #[test]
    fn min_and_max_keep_polarity() {
        let mut samples = vec![0.0; SAMPLES_PER_FRAME];
        samples[0] = -0.8;
        samples[1] = 0.4;
        let frame = frames_for(&samples)[0];
        assert!((frame.min - -0.8).abs() < 1e-6);
        assert!((frame.max - 0.4).abs() < 1e-6);
    }
}
