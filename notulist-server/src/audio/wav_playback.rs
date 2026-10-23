use std::path::Path;
use std::time::Duration;

use hound::{SampleFormat, WavReader};
use tokio::sync::{mpsc, oneshot};

use super::resampler::resample_buffer;
use super::{CapturedAudio, StopCapture};
use crate::error::CaptureError;

const WHISPER_SAMPLE_RATE_HZ: u32 = 16_000;
const PLAYBACK_BLOCK_SAMPLES: usize = 320;
const PLAYBACK_BLOCK_DURATION: Duration = Duration::from_millis(20);
const CHUNK_QUEUE_DEPTH: usize = 64;

pub fn start_wav_playback(path: &Path, playback_speed: f32) -> Result<CapturedAudio, CaptureError> {
    let samples = read_wav_as_whisper_input(path)?;
    let period =
        Duration::try_from_secs_f32(PLAYBACK_BLOCK_DURATION.as_secs_f32() / playback_speed)?;
    let (chunks_tx, chunks_rx) = mpsc::channel(CHUNK_QUEUE_DEPTH);
    let (stop_tx, stop_rx) = oneshot::channel();
    let (_disconnect_tx, disconnect_rx) = oneshot::channel();
    tokio::spawn(play_samples(samples, period, chunks_tx, stop_rx));
    Ok(CapturedAudio {
        chunks: chunks_rx,
        stop: StopCapture::new(stop_tx),
        disconnect: disconnect_rx,
    })
}

async fn play_samples(
    samples: Vec<f32>,
    period: Duration,
    chunks_tx: mpsc::Sender<Vec<f32>>,
    mut stop_rx: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(period);
    let mut blocks = samples.chunks(PLAYBACK_BLOCK_SAMPLES);
    loop {
        let Some(block) = blocks.next() else {
            return;
        };
        tokio::select! {
            _ = &mut stop_rx => return,
            _ = ticker.tick() => {}
        }
        if chunks_tx.send(block.to_vec()).await.is_err() {
            return;
        }
    }
}

pub fn read_wav_as_whisper_input(path: &Path) -> Result<Vec<f32>, CaptureError> {
    let reader = WavReader::open(path)?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => {
            let full_scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 / full_scale))
                .collect::<Result<Vec<f32>, hound::Error>>()?
        }
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<f32>, hound::Error>>()?,
    };
    let mono: Vec<f32> = interleaved
        .chunks(usize::from(spec.channels.max(1)))
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok(if spec.sample_rate == WHISPER_SAMPLE_RATE_HZ {
        mono
    } else {
        resample_buffer(&mono, spec.sample_rate, WHISPER_SAMPLE_RATE_HZ)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech/speech_hello.wav")
    }

    #[test]
    fn speech_fixture_decodes_to_sixteen_kilohertz_mono() {
        let samples = read_wav_as_whisper_input(&fixture_path()).expect("fixture decodes");
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|sample| sample.abs() <= 1.0));
    }

    #[tokio::test]
    async fn high_speed_playback_delivers_every_sample_in_order_then_closes() {
        let expected = read_wav_as_whisper_input(&fixture_path()).expect("fixture decodes");
        let mut playback = start_wav_playback(&fixture_path(), 50.0).expect("playback starts");
        let mut delivered = Vec::new();
        while let Some(chunk) = playback.chunks.recv().await {
            delivered.extend(chunk);
        }
        assert_eq!(delivered, expected);
    }

    #[tokio::test]
    async fn stopping_playback_closes_the_chunk_stream_early() {
        let expected = read_wav_as_whisper_input(&fixture_path()).expect("fixture decodes");
        let playback = start_wav_playback(&fixture_path(), 1.0).expect("playback starts");
        let CapturedAudio {
            mut chunks, stop, ..
        } = playback;
        stop.stop();
        let mut delivered = Vec::new();
        while let Some(chunk) = chunks.recv().await {
            delivered.extend(chunk);
        }
        assert!(delivered.len() < expected.len());
    }
}
