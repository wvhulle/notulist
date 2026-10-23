use std::collections::VecDeque;

use notulist_client::LevelFrame;

use crate::ime_config::LevelMeterConfig;

#[derive(Debug, Clone)]
pub struct LevelBars {
    bars: VecDeque<f32>,
    frames_per_bar: usize,
    silence_floor_dbfs: f64,
    pending_peak_dbfs: f64,
    pending_frames: usize,
}

impl LevelBars {
    pub fn from_config(meter: &LevelMeterConfig) -> Self {
        Self {
            bars: std::iter::repeat_n(0.0, meter.bar_count).collect(),
            frames_per_bar: meter.frames_per_bar(),
            silence_floor_dbfs: meter.silence_floor_dbfs,
            pending_peak_dbfs: meter.silence_floor_dbfs,
            pending_frames: 0,
        }
    }

    pub fn push_frames(&mut self, frames: &[LevelFrame]) {
        frames.iter().for_each(|frame| {
            self.pending_peak_dbfs = self.pending_peak_dbfs.max(frame.peak_dbfs);
            self.pending_frames += 1;
            if self.pending_frames >= self.frames_per_bar {
                self.close_bar();
            }
        });
    }

    pub fn heights(&self) -> Vec<f32> {
        self.bars.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.bars.iter_mut().for_each(|bar| *bar = 0.0);
        self.pending_peak_dbfs = self.silence_floor_dbfs;
        self.pending_frames = 0;
    }

    fn close_bar(&mut self) {
        let height = bar_height_for_dbfs(self.pending_peak_dbfs, self.silence_floor_dbfs);
        self.bars.pop_front();
        self.bars.push_back(height);
        self.pending_peak_dbfs = self.silence_floor_dbfs;
        self.pending_frames = 0;
    }
}

fn bar_height_for_dbfs(peak_dbfs: f64, silence_floor_dbfs: f64) -> f32 {
    let span_dbfs = (-silence_floor_dbfs).max(1.0);
    ((peak_dbfs - silence_floor_dbfs) / span_dbfs).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_at(peak_dbfs: f64) -> LevelFrame {
        LevelFrame {
            peak_dbfs,
            ..LevelFrame::default()
        }
    }

    fn meter(bar_count: usize, visible_history_ms: u64) -> LevelMeterConfig {
        LevelMeterConfig {
            bar_count,
            visible_history_ms,
            ..LevelMeterConfig::default()
        }
    }

    #[test]
    fn starts_flat_with_the_configured_bar_count() {
        let bars = LevelBars::from_config(&meter(8, 400));
        assert_eq!(bars.heights(), vec![0.0; 8]);
    }

    #[test]
    fn loud_frames_scroll_in_from_the_right() {
        let mut bars = LevelBars::from_config(&meter(3, 90));
        bars.push_frames(&[frame_at(0.0); 3]);
        assert_eq!(bars.heights(), vec![0.0, 0.0, 1.0]);
        bars.push_frames(&[frame_at(-60.0); 3]);
        assert_eq!(bars.heights(), vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn each_bar_shows_the_loudest_frame_it_covers() {
        let mut bars = LevelBars::from_config(&meter(1, 50));
        bars.push_frames(&[
            frame_at(-120.0),
            frame_at(-30.0),
            frame_at(-120.0),
            frame_at(-120.0),
            frame_at(-120.0),
        ]);
        assert!((bars.heights()[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn incomplete_bar_waits_for_more_frames() {
        let mut bars = LevelBars::from_config(&meter(2, 40));
        bars.push_frames(&[frame_at(0.0)]);
        assert_eq!(bars.heights(), vec![0.0, 0.0]);
    }

    #[test]
    fn silence_floor_sets_the_quietest_visible_bar() {
        let quiet_floor = LevelMeterConfig {
            silence_floor_dbfs: -30.0,
            ..meter(1, 10)
        };
        let mut bars = LevelBars::from_config(&quiet_floor);
        bars.push_frames(&[frame_at(-40.0)]);
        assert_eq!(bars.heights(), vec![0.0]);
        let mut loud = LevelBars::from_config(&quiet_floor);
        loud.push_frames(&[frame_at(-15.0)]);
        assert_eq!(loud.heights(), vec![0.5]);
    }

    #[test]
    fn clear_flattens_the_bars() {
        let mut bars = LevelBars::from_config(&meter(2, 20));
        bars.push_frames(&[frame_at(0.0), frame_at(0.0)]);
        bars.clear();
        assert_eq!(bars.heights(), vec![0.0, 0.0]);
    }
}
