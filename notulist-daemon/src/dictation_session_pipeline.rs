use std::sync::Arc;
use std::time::Duration;

use notulist_client::{DictationFailure, LevelFrame, SessionOutcome, TranscriptEdit};
use tokio::sync::{mpsc, oneshot};

use crate::audio::level_bucketer::LevelBucketer;
use crate::audio::{self, AudioSource, CapturedAudio, StopCapture};
use crate::dictation_signal_emitter::DictationSignalEmitter;
use crate::error::{CaptureError, MicrophoneError};
use crate::spoken_text_processor::TextProcessor;
use crate::transcribe::sliding_window::SlidingWindowStreamingTranscriber;
use crate::transcribe::streaming::{StreamHandle, StreamingEvent};

const LEVEL_BATCH_INTERVAL: Duration = Duration::from_millis(33);
const STREAMING_SAMPLES_QUEUE_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMilestone {
    AudioEnded,
    Finished(SessionOutcome),
    Failed(DictationFailure),
}

pub struct SessionControls {
    stop_tx: Option<oneshot::Sender<()>>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

impl SessionControls {
    pub fn request_stop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
    }

    pub fn request_cancel(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
    }
}

pub fn start_dictation_session(
    source: &AudioSource,
    max_duration: Duration,
    engine: &SlidingWindowStreamingTranscriber,
    text_processor: Arc<TextProcessor>,
    signals: DictationSignalEmitter,
    milestones: mpsc::Sender<SessionMilestone>,
) -> Result<SessionControls, CaptureError> {
    let CapturedAudio {
        chunks,
        stop,
        disconnect,
    } = audio::start_capture(source)?;
    let (samples_tx, samples_rx) = mpsc::channel(STREAMING_SAMPLES_QUEUE_DEPTH);
    let stream = engine.start_stream(samples_rx);
    let (stop_tx, stop_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let pipeline = SessionPipeline {
        chunks: Some(chunks),
        stop: Some(stop),
        disconnect,
        samples_tx: Some(samples_tx),
        stream,
        bucketer: LevelBucketer::new(source.sample_rate_hz()),
        pending_frames: Vec::new(),
        text_processor,
        signals,
        milestones,
        max_duration,
    };
    tokio::spawn(pipeline.run(stop_rx, cancel_rx));
    Ok(SessionControls {
        stop_tx: Some(stop_tx),
        cancel_tx: Some(cancel_tx),
    })
}

struct SessionPipeline {
    chunks: Option<mpsc::Receiver<Vec<f32>>>,
    stop: Option<StopCapture>,
    disconnect: oneshot::Receiver<MicrophoneError>,
    samples_tx: Option<mpsc::Sender<Vec<f32>>>,
    stream: StreamHandle,
    bucketer: LevelBucketer,
    pending_frames: Vec<LevelFrame>,
    text_processor: Arc<TextProcessor>,
    signals: DictationSignalEmitter,
    milestones: mpsc::Sender<SessionMilestone>,
    max_duration: Duration,
}

impl SessionPipeline {
    async fn run(mut self, mut stop_rx: oneshot::Receiver<()>, cancel_rx: oneshot::Receiver<()>) {
        let mut level_batches = tokio::time::interval(LEVEL_BATCH_INTERVAL);
        let max_duration_reached = tokio::time::sleep(self.max_duration);
        tokio::pin!(max_duration_reached);
        tokio::pin!(cancel_rx);
        let mut stop_requested = false;
        let mut watch_disconnect = true;

        let end = loop {
            tokio::select! {
                chunk = next_chunk(&mut self.chunks) => match chunk {
                    Some(chunk) => self.forward_chunk(chunk),
                    None => self.end_audio_input().await,
                },
                _ = level_batches.tick() => self.flush_levels().await,
                _ = &mut stop_rx, if !stop_requested => {
                    stop_requested = true;
                    self.stop_capture();
                }
                _ = &mut max_duration_reached, if !stop_requested => {
                    tracing::info!("Maximum session duration reached; finishing the session");
                    stop_requested = true;
                    self.stop_capture();
                }
                _ = &mut cancel_rx => {
                    self.stop_capture();
                    break SessionMilestone::Finished(SessionOutcome::Cancelled);
                }
                disconnected = &mut self.disconnect, if watch_disconnect => match disconnected {
                    Ok(error) => {
                        tracing::error!(%error, "Microphone disconnected during the session");
                        self.stop_capture();
                        break SessionMilestone::Failed(DictationFailure::MicrophoneDisconnected);
                    }
                    Err(_) => watch_disconnect = false,
                },
                event = self.stream.events.recv() => match event {
                    Some(StreamingEvent::Ended) | None => {
                        break SessionMilestone::Finished(SessionOutcome::Committed);
                    }
                    Some(StreamingEvent::Error(error)) => {
                        tracing::error!(%error, "Streaming transcription failed");
                        break SessionMilestone::Failed(DictationFailure::InferenceFailed);
                    }
                    Some(event) => self.publish_event(event).await,
                },
            }
        };

        self.flush_levels().await;
        let StreamHandle { cancel, task, .. } = self.stream;
        if end != SessionMilestone::Finished(SessionOutcome::Committed) {
            let _ = cancel.send(());
        }
        let _ = self.milestones.send(end).await;
        if let Err(error) = task.await {
            tracing::error!(%error, "Streaming transcription task failed");
        }
    }

    fn forward_chunk(&mut self, chunk: Vec<f32>) {
        self.bucketer.push(&chunk, &mut self.pending_frames);
        if let Some(samples_tx) = &self.samples_tx {
            if samples_tx.try_send(chunk).is_err() {
                tracing::trace!("Streaming transcriber is behind; dropping an audio chunk");
            }
        }
    }

    async fn end_audio_input(&mut self) {
        self.chunks = None;
        self.samples_tx = None;
        let _ = self.milestones.send(SessionMilestone::AudioEnded).await;
    }

    fn stop_capture(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.stop();
        }
    }

    async fn flush_levels(&mut self) {
        if self.pending_frames.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.pending_frames);
        self.signals.levels(frames).await;
    }

    async fn publish_event(&self, event: StreamingEvent) {
        match event {
            StreamingEvent::Decoding(active) => self.signals.decoding_changed(active).await,
            event => {
                if let Some(edit) =
                    transcript_edit_for(event, &self.text_processor).filter(|edit| !edit.is_noop())
                {
                    self.signals.transcript_edited(edit).await;
                }
            }
        }
    }
}

async fn next_chunk(chunks: &mut Option<mpsc::Receiver<Vec<f32>>>) -> Option<Vec<f32>> {
    match chunks {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn transcript_edit_for(
    event: StreamingEvent,
    text_processor: &TextProcessor,
) -> Option<TranscriptEdit> {
    match event {
        StreamingEvent::Partial { text } => Some(TranscriptEdit::append(text)),
        StreamingEvent::Final { text } => {
            Some(TranscriptEdit::append(text_processor.process(&text)))
        }
        StreamingEvent::Replace { backspace, text } => Some(TranscriptEdit {
            delete_chars: u32::try_from(backspace).unwrap_or(u32::MAX),
            insert: text_processor.process(&text),
        }),
        StreamingEvent::Decoding(_) | StreamingEvent::Ended | StreamingEvent::Error(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TextCleanupConfig;

    fn processor_with_replacement() -> TextProcessor {
        let config: TextCleanupConfig = toml::from_str(
            r#"
            word_replacements = { "vox type" = "notulist" }
            "#,
        )
        .expect("valid text cleanup config");
        TextProcessor::new(&config)
    }

    #[test]
    fn partials_stay_raw_so_later_backspace_counts_match() {
        let edit = transcript_edit_for(
            StreamingEvent::Partial {
                text: "vox type".into(),
            },
            &processor_with_replacement(),
        );
        assert_eq!(edit, Some(TranscriptEdit::append("vox type")));
    }

    #[test]
    fn finals_and_replacements_go_through_text_processing() {
        let processor = processor_with_replacement();
        let final_edit = transcript_edit_for(
            StreamingEvent::Final {
                text: "vox type".into(),
            },
            &processor,
        );
        assert_eq!(final_edit.map(|edit| edit.insert), Some("notulist".into()));
        let replace_edit = transcript_edit_for(
            StreamingEvent::Replace {
                backspace: 3,
                text: "vox type".into(),
            },
            &processor,
        );
        assert_eq!(
            replace_edit,
            Some(TranscriptEdit {
                delete_chars: 3,
                insert: "notulist".into()
            })
        );
    }

    #[test]
    fn decoding_and_lifecycle_events_produce_no_edit() {
        let processor = processor_with_replacement();
        assert_eq!(
            transcript_edit_for(StreamingEvent::Decoding(true), &processor),
            None
        );
        assert_eq!(transcript_edit_for(StreamingEvent::Ended, &processor), None);
    }
}
