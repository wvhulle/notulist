use std::sync::Arc;
use std::time::Duration;

use notulist_client::REVISION_LAG_WORDS;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};

use super::streaming::{StreamHandle, StreamingEvent};
use super::Transcriber;
use crate::config::StreamingConfig;
use crate::error::TranscribeError;

const HALLUCINATION_PATTERNS: &[&str] = &[
    "you",
    "the",
    "i",
    "a",
    "it",
    "is",
    "and",
    "to",
    "thank you",
    "thanks",
    "bye",
    "okay",
    "thank you for watching",
    "thanks for watching",
    "please subscribe",
    "subscribe",
    "thank you very much",
    "you're welcome",
    "good night",
    "good bye",
    "see you next time",
    "subtitles by the amara.org community",
];

const SLIDING_DIFF_LOOKBACK_WORDS: usize = 20;
const EVENTS_QUEUE_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Delta {
    Append(String),
    Replace { backspace: usize, text: String },
}

fn delta_to_event(delta: Delta, type_partials: bool) -> StreamingEvent {
    match delta {
        Delta::Append(text) if type_partials => StreamingEvent::Partial { text },
        Delta::Append(text) => StreamingEvent::Final { text },
        Delta::Replace { backspace, text } => StreamingEvent::Replace { backspace, text },
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SlidingWindowConfig {
    pub interval_s: f64,
    pub max_buffer_s: f32,
    pub sample_rate: u32,
    pub min_speech_rms: f32,
    pub min_audio_s: f32,
    pub partial_min_words: usize,
    pub type_partials: bool,
    pub revision_mode: bool,
}

impl SlidingWindowConfig {
    pub fn from_streaming_config(streaming: &StreamingConfig, sample_rate: u32) -> Self {
        Self {
            interval_s: f64::from(streaming.retranscribe_interval_secs),
            max_buffer_s: streaming.max_window_secs,
            sample_rate,
            min_speech_rms: streaming.min_window_rms_for_speech,
            min_audio_s: streaming.min_audio_before_first_pass_secs,
            partial_min_words: streaming.min_new_words_per_partial,
            type_partials: streaming.type_partials_live,
            revision_mode: streaming.revise_recent_words,
        }
    }
}

pub struct SlidingWindowStreamingTranscriber {
    base: Arc<dyn Transcriber>,
    config: SlidingWindowConfig,
}

impl SlidingWindowStreamingTranscriber {
    pub fn new(base: Arc<dyn Transcriber>, config: SlidingWindowConfig) -> Self {
        Self { base, config }
    }

    pub fn start_stream(&self, mut samples_rx: mpsc::Receiver<Vec<f32>>) -> StreamHandle {
        let (events_tx, events_rx) = mpsc::channel::<StreamingEvent>(EVENTS_QUEUE_DEPTH);
        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();

        let base = Arc::clone(&self.base);
        let config = self.config;

        let task = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let mut session = Session::new(base, config);
            let mut report_decoding = |active: bool| {
                let _ = runtime.block_on(events_tx.send(StreamingEvent::Decoding(active)));
            };

            let mut ticker = interval(Duration::from_secs_f64(config.interval_s));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            runtime.block_on(ticker.tick());

            enum Outcome {
                Chunk(Vec<f32>),
                Eof,
                Tick,
                Cancelled,
            }

            loop {
                let outcome = runtime.block_on(async {
                    tokio::select! {
                        chunk = samples_rx.recv() => match chunk {
                            Some(c) => Outcome::Chunk(c),
                            None => Outcome::Eof,
                        },
                        _ = ticker.tick() => Outcome::Tick,
                        _ = &mut cancel_rx => Outcome::Cancelled,
                    }
                });

                match outcome {
                    Outcome::Chunk(chunk) => session.feed(&chunk),
                    Outcome::Cancelled => {
                        let _ = runtime.block_on(events_tx.send(StreamingEvent::Ended));
                        return;
                    }
                    Outcome::Eof => {
                        if let Some(tail) = session.final_flush(&mut report_decoding) {
                            let event = delta_to_event(tail, config.type_partials);
                            let _ = runtime.block_on(events_tx.send(event));
                        }
                        let _ = runtime.block_on(events_tx.send(StreamingEvent::Ended));
                        return;
                    }
                    Outcome::Tick => match session.on_tick(&mut report_decoding) {
                        Ok(deltas) => deltas.into_iter().for_each(|delta| {
                            let event = delta_to_event(delta, config.type_partials);
                            let _ = runtime.block_on(events_tx.send(event));
                        }),
                        Err(err) => {
                            let _ = runtime.block_on(events_tx.send(StreamingEvent::Error(err)));
                            let _ = runtime.block_on(events_tx.send(StreamingEvent::Ended));
                            return;
                        }
                    },
                }
            }
        });

        StreamHandle {
            events: events_rx,
            cancel: cancel_tx,
            task,
        }
    }
}

struct Session {
    base: Arc<dyn Transcriber>,
    config: SlidingWindowConfig,

    buffer: Vec<f32>,
    sliding: bool,

    full_text_parts: Vec<String>,
    #[allow(dead_code)]
    prev_whisper: String,
    last_words: Vec<String>,
    confirmed_words: Vec<String>,
    last_delta_words: Vec<String>,

    provisional_words: Vec<String>,
}

impl Session {
    fn new(base: Arc<dyn Transcriber>, config: SlidingWindowConfig) -> Self {
        Self {
            base,
            config,
            buffer: Vec::new(),
            sliding: false,
            full_text_parts: Vec::new(),
            prev_whisper: String::new(),
            last_words: Vec::new(),
            confirmed_words: Vec::new(),
            last_delta_words: Vec::new(),
            provisional_words: Vec::new(),
        }
    }

    fn feed(&mut self, samples: &[f32]) {
        self.buffer.extend_from_slice(samples);

        let max_samples = (self.config.max_buffer_s * self.config.sample_rate as f32) as usize;
        let mut dropped = 0usize;
        if self.buffer.len() > max_samples {
            dropped = self.buffer.len() - max_samples;
            self.buffer.drain(..dropped);
        }
        if dropped > 0 && !self.sliding {
            tracing::debug!("[sliding] Buffer full — entering sliding mode");
            self.sliding = true;
        }
    }

    fn transcribe_buffer(
        &self,
        report_decoding: &mut impl FnMut(bool),
    ) -> Result<Option<String>, TranscribeError> {
        if (self.buffer.len() as f32) < self.config.min_audio_s * self.config.sample_rate as f32 {
            return Ok(None);
        }
        if rms(&self.buffer) < self.config.min_speech_rms {
            return Ok(None);
        }
        let infer_start = std::time::Instant::now();
        report_decoding(true);
        let transcribed = self.base.transcribe(&self.buffer);
        report_decoding(false);
        let text = transcribed?.trim().to_string();
        let infer_secs = infer_start.elapsed().as_secs_f32();
        tracing::trace!(
            "[sliding] tick transcribe -> {text:?} ({} samples, {:.3}s infer, sliding={})",
            self.buffer.len(),
            infer_secs,
            self.sliding,
        );
        if infer_secs > self.config.interval_s as f32 {
            tracing::warn!(
                "[sliding] inference ({:.2}s) exceeded the tick interval ({:.2}s) — \
                 audio chunks may be getting dropped; buffer at {:.1}s",
                infer_secs,
                self.config.interval_s,
                self.buffer.len() as f32 / self.config.sample_rate as f32,
            );
        }
        if text.is_empty() || is_hallucination(&text) {
            return Ok(None);
        }
        Ok(Some(text))
    }

    fn on_tick(
        &mut self,
        report_decoding: &mut impl FnMut(bool),
    ) -> Result<Vec<Delta>, TranscribeError> {
        let Some(curr) = self.transcribe_buffer(report_decoding)? else {
            return Ok(Vec::new());
        };
        let curr_words: Vec<String> = curr.split_whitespace().map(str::to_owned).collect();
        if curr_words.is_empty() {
            return Ok(Vec::new());
        }

        let deltas = if self.config.revision_mode {
            match self.revision_new_tail(&curr) {
                Some(tail) => self.reconcile_revision(tail).into_iter().collect(),
                None => Vec::new(),
            }
        } else {
            self.on_tick_conservative(&curr, &curr_words)
        };

        self.prev_whisper = curr;
        self.last_words = curr_words;
        Ok(deltas)
    }

    fn on_tick_conservative(&mut self, curr: &str, curr_words: &[String]) -> Vec<Delta> {
        let mut deltas = Vec::new();

        if self.sliding {
            let full_emitted = self.full_text_parts.join(" ");
            let already_emitted = last_n_words(&full_emitted, SLIDING_DIFF_LOOKBACK_WORDS);
            let mut delta = extract_new_text_confident(&already_emitted, curr).unwrap_or_default();
            let mut delta_words: Vec<String> =
                delta.split_whitespace().map(str::to_owned).collect();
            tracing::trace!(
                "[sliding] sliding-diff: already_emitted(tail)={already_emitted:?} delta={delta:?} last_delta_words={:?}",
                self.last_delta_words,
            );

            if !self.last_delta_words.is_empty() && !delta_words.is_empty() {
                let stable_n = common_prefix_len(&self.last_delta_words, &delta_words);
                if stable_n >= self.config.partial_min_words {
                    let new = delta_words[..stable_n].join(" ");
                    let new = dedupe_against_emitted(&self.full_text_parts.join(" "), &new);
                    let last_emitted = self.full_text_parts.last();
                    let not_repeat = last_emitted.map(|s| s != &new).unwrap_or(true);
                    if !new.is_empty() && not_repeat {
                        let typed = if self.full_text_parts.is_empty() {
                            new.clone()
                        } else {
                            format!(" {new}")
                        };
                        tracing::debug!("[sliding] COMMIT (sliding mode): {typed:?}");
                        self.full_text_parts.push(new);
                        deltas.push(Delta::Append(typed));
                        let full_emitted = self.full_text_parts.join(" ");
                        let already_emitted =
                            last_n_words(&full_emitted, SLIDING_DIFF_LOOKBACK_WORDS);
                        delta =
                            extract_new_text_confident(&already_emitted, curr).unwrap_or_default();
                        delta_words = delta.split_whitespace().map(str::to_owned).collect();
                    } else if !new.is_empty() {
                        tracing::debug!(
                            "[sliding] would-be commit suppressed as repeat of last: {new:?}"
                        );
                    }
                }
            }
            self.last_delta_words = delta_words;
        } else if !self.last_words.is_empty() {
            let stable_n = common_prefix_len(&self.last_words, curr_words);
            if stable_n > self.confirmed_words.len() {
                let confirmed_new = &curr_words[self.confirmed_words.len()..stable_n];
                if confirmed_new.len() >= self.config.partial_min_words {
                    let new = confirmed_new.join(" ");
                    let new = dedupe_against_emitted(&self.full_text_parts.join(" "), &new);
                    let last_emitted = self.full_text_parts.last();
                    let not_repeat = last_emitted.map(|s| s != &new).unwrap_or(true);
                    if !new.is_empty() && not_repeat {
                        let typed = if self.full_text_parts.is_empty() {
                            new.clone()
                        } else {
                            format!(" {new}")
                        };
                        tracing::debug!("[sliding] COMMIT (growing mode): {typed:?}");
                        self.full_text_parts.push(new);
                        self.confirmed_words.extend(confirmed_new.iter().cloned());
                        deltas.push(Delta::Append(typed));
                    }
                }
            }
        }

        deltas
    }

    fn revision_confirmed_text(&self) -> String {
        last_n_words(
            &if self.sliding {
                self.full_text_parts.join(" ")
            } else {
                self.confirmed_words.join(" ")
            },
            SLIDING_DIFF_LOOKBACK_WORDS,
        )
    }

    fn revision_new_tail(&self, curr: &str) -> Option<Vec<String>> {
        extract_new_text_confident(&self.revision_confirmed_text(), curr)
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
    }

    fn reconcile_revision(&mut self, new_tail_words: Vec<String>) -> Option<Delta> {
        let agree_n = common_prefix_len(&self.provisional_words, &new_tail_words);
        let has_prefix = !self.full_text_parts.is_empty();

        let delta = if agree_n < self.provisional_words.len() {
            let backspace = typed_len_from(&self.provisional_words, agree_n, has_prefix);
            let text = format_typed(&new_tail_words, agree_n, has_prefix);
            tracing::debug!(
                "[sliding] REVISE: backspace {backspace} chars, retype {text:?} \
                 (was {:?})",
                self.provisional_words
            );
            Some(Delta::Replace { backspace, text })
        } else if agree_n < new_tail_words.len() {
            let text = format_typed(&new_tail_words, agree_n, has_prefix);
            if text.is_empty() {
                None
            } else {
                tracing::debug!("[sliding] REVISE append: {text:?}");
                Some(Delta::Append(text))
            }
        } else {
            None
        };

        self.provisional_words = new_tail_words;

        if self.provisional_words.len() > REVISION_LAG_WORDS {
            let confirm_upto = self.provisional_words.len() - REVISION_LAG_WORDS;
            let newly_confirmed: Vec<String> =
                self.provisional_words.drain(..confirm_upto).collect();
            if !newly_confirmed.is_empty() {
                self.full_text_parts.push(newly_confirmed.join(" "));
                self.confirmed_words.extend(newly_confirmed);
            }
        }

        delta
    }

    fn final_flush(&mut self, report_decoding: &mut impl FnMut(bool)) -> Option<Delta> {
        let final_text = match self.transcribe_buffer(report_decoding) {
            Ok(Some(t)) => t,
            _ => return None,
        };

        if self.config.revision_mode {
            let tail = self.revision_new_tail(&final_text).or_else(|| {
                let guess = extract_new_text(&self.revision_confirmed_text(), &final_text);
                if guess.is_empty() {
                    None
                } else {
                    Some(guess.split_whitespace().map(str::to_owned).collect())
                }
            });
            return tail.and_then(|t| self.reconcile_revision(t));
        }

        let full_emitted = self.full_text_parts.join(" ");
        let already_emitted = last_n_words(&full_emitted, SLIDING_DIFF_LOOKBACK_WORDS);
        let delta = extract_new_text(&already_emitted, &final_text);
        let delta = delta.trim().to_string();
        let delta = dedupe_against_emitted(&full_emitted, &delta);
        if delta.is_empty() {
            None
        } else {
            let typed = if self.full_text_parts.is_empty() {
                delta.clone()
            } else {
                format!(" {delta}")
            };
            self.full_text_parts.push(delta);
            Some(Delta::Append(typed))
        }
    }
}

fn word_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim_end_matches(['.', ',', '!', '?', ';', ':'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

fn words_match<T: AsRef<str>>(a: &[T], b: &[T]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| word_eq(x.as_ref(), y.as_ref()))
}

fn common_prefix_len<T: AsRef<str>>(a: &[T], b: &[T]) -> usize {
    a.iter()
        .zip(b.iter())
        .take_while(|(x, y)| word_eq(x.as_ref(), y.as_ref()))
        .count()
}

fn last_n_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let start = words.len().saturating_sub(n);
    words[start..].join(" ")
}

fn typed_len_from(words: &[String], from_idx: usize, has_prefix: bool) -> usize {
    if from_idx >= words.len() {
        return 0;
    }
    let tail = words[from_idx..].join(" ");
    let leading_space = from_idx > 0 || has_prefix;
    tail.chars().count() + usize::from(leading_space)
}

fn format_typed(words: &[String], from_idx: usize, has_prefix: bool) -> String {
    if from_idx >= words.len() {
        return String::new();
    }
    let tail = words[from_idx..].join(" ");
    if from_idx > 0 || has_prefix {
        format!(" {tail}")
    } else {
        tail
    }
}

fn dedupe_against_emitted(already_emitted_full: &str, candidate: &str) -> String {
    let tail = last_n_words(already_emitted_full, SLIDING_DIFF_LOOKBACK_WORDS);
    extract_new_text_confident(&tail, candidate).unwrap_or_else(|| candidate.to_string())
}

fn extract_new_text_confident(prev: &str, curr: &str) -> Option<String> {
    if prev.is_empty() {
        return Some(curr.to_string());
    }
    if curr.is_empty() {
        return Some(String::new());
    }

    let prev_words: Vec<&str> = prev.split_whitespace().collect();
    let curr_words: Vec<&str> = curr.split_whitespace().collect();

    if prev_words.is_empty() || curr_words.is_empty() {
        return Some(curr.to_string());
    }

    let max_check = prev_words.len().min(curr_words.len());
    let best_overlap = (1..=max_check)
        .filter(|&length| {
            words_match(
                &prev_words[prev_words.len() - length..],
                &curr_words[..length],
            )
        })
        .max()
        .unwrap_or(0);
    if best_overlap > 0 {
        return Some(curr_words[best_overlap..].join(" "));
    }

    if let Some(rest) = curr.strip_prefix(prev) {
        return Some(rest.trim().to_string());
    }

    let (matched_prev_words, best_ci) =
        curr_words
            .iter()
            .enumerate()
            .fold((0, 0), |(matched, best), (ci, cw)| {
                if matched < prev_words.len() && word_eq(prev_words[matched], cw) {
                    (matched + 1, ci + 1)
                } else {
                    (matched, best)
                }
            });
    if matched_prev_words as f32 >= prev_words.len() as f32 * 0.5 {
        return Some(curr_words[best_ci..].join(" "));
    }

    None
}

fn extract_new_text(prev: &str, curr: &str) -> String {
    if let Some(text) = extract_new_text_confident(prev, curr) {
        return text;
    }

    let prev_words: Vec<&str> = prev.split_whitespace().collect();
    let curr_words: Vec<&str> = curr.split_whitespace().collect();
    if curr_words.len() <= prev_words.len() + 1 {
        return String::new();
    }
    curr_words[prev_words.len()..].join(" ")
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

fn is_hallucination(text: &str) -> bool {
    let norm = text.trim().to_lowercase();
    let norm = norm.trim_end_matches(['.', ',', '!', '?']);
    HALLUCINATION_PATTERNS.contains(&norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[test]
    fn word_eq_ignores_case_and_trailing_punct() {
        assert!(word_eq("Hello,", "hello"));
        assert!(word_eq("WORLD.", "world"));
        assert!(word_eq("you?", "you"));
        assert!(!word_eq("there", "their"));
    }

    #[test]
    fn common_prefix_len_stops_at_first_divergence() {
        let a = ["hello", "world", "foo"];
        let b = ["hello", "WORLD", "bar"];
        assert_eq!(common_prefix_len(&a, &b), 2);
        let c = ["hello"];
        assert_eq!(common_prefix_len(&a, &c), 1);
    }

    #[test]
    fn extract_new_text_empty_prev_returns_curr() {
        assert_eq!(extract_new_text("", "hello world"), "hello world");
    }

    #[test]
    fn extract_new_text_empty_curr_returns_empty() {
        assert_eq!(extract_new_text("hello", ""), "");
    }

    #[test]
    fn extract_new_text_suffix_prefix_overlap() {
        assert_eq!(extract_new_text("hello world", "world foo bar"), "foo bar");
        assert_eq!(
            extract_new_text("we should deploy now", "deploy now please"),
            "please"
        );
    }

    #[test]
    fn extract_new_text_verbatim_prefix() {
        assert_eq!(extract_new_text("hello", "hello world foo"), "world foo");
    }

    #[test]
    fn extract_new_text_fuzzy_scan() {
        assert_eq!(extract_new_text("hello, world", "hello world foo"), "foo");
    }

    #[test]
    fn extract_new_text_refining_emits_nothing() {
        assert_eq!(extract_new_text("hello world foo", "hello world"), "");
        assert_eq!(extract_new_text("a b c", "a b"), "");
    }

    #[test]
    fn extract_new_text_safety_tail() {
        assert_eq!(extract_new_text("x y", "a b c d"), "c d");
    }

    #[test]
    fn extract_new_text_confident_declines_the_safety_guess() {
        assert_eq!(extract_new_text_confident("x y", "a b c d"), None);
    }

    #[test]
    fn sliding_mode_duplicate_reproduction_and_fix() {
        let prev = "roses are red violets";
        let curr = "sky is blue sugar is sweet and so are you my friend";
        assert_eq!(extract_new_text_confident(prev, curr), None);

        let guessed = extract_new_text(prev, curr);
        assert_eq!(guessed, "is sweet and so are you my friend");
    }

    #[test]
    fn dedupe_strips_overlap_at_growing_to_sliding_transition() {
        let already_emitted = "It takes a little bit, bit, and then it spits it out.";
        let candidate =
            "and then it spits it out. Yeah, that's what we like to see. I think this is nice.";
        assert_eq!(
            dedupe_against_emitted(already_emitted, candidate),
            "Yeah, that's what we like to see. I think this is nice."
        );
    }

    #[test]
    fn dedupe_leaves_genuinely_new_text_untouched() {
        let already_emitted = "hello world";
        let candidate = "completely unrelated new content";
        assert_eq!(
            dedupe_against_emitted(already_emitted, candidate),
            candidate
        );
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn revision_session() -> Session {
        let mut cfg = streaming_config();
        cfg.revision_mode = true;
        Session::new(Arc::new(FakeTranscriber), cfg)
    }

    #[test]
    fn final_flush_falls_back_to_a_best_effort_guess_when_no_confident_anchor_exists() {
        let mut session = revision_session();
        session.confirmed_words = words("x y");
        session.feed(&loud_samples(1.5));
        session.base = Arc::new(RevisingTranscriber::new(vec!["a b c d"]));

        let delta = session
            .final_flush(&mut |_| {})
            .expect("must fall back to a guess instead of silently giving up");

        assert_eq!(
            delta,
            Delta::Append("c d".to_string()),
            "should type the safety-guess tail rather than leave nothing corrected"
        );
    }

    #[test]
    fn revision_new_tail_growing_mode_is_immune_to_reworded_prefix_drift() {
        let mut session = revision_session();
        assert!(
            !session.sliding,
            "this scenario is specifically growing mode"
        );
        session.confirmed_words = words("This works way better.");

        let curr = "This works way much better. No hang ups.";
        let tail = session.revision_new_tail(curr).unwrap();

        assert_eq!(
            tail,
            words("No hang ups."),
            "must not re-include any part of the reworded confirmed sentence"
        );
    }

    #[test]
    fn typed_len_from_counts_leading_space_only_when_prefixed() {
        let w = words("alpha beta gamma");
        assert_eq!(typed_len_from(&w, 0, true), 17);
        assert_eq!(typed_len_from(&w, 0, false), 16);
        assert_eq!(typed_len_from(&w, 1, true), 11);
        assert_eq!(typed_len_from(&w, 1, false), 11);
        assert_eq!(typed_len_from(&w, 3, true), 0);
    }

    #[test]
    fn format_typed_matches_typed_len_from() {
        let w = words("alpha beta gamma");
        assert_eq!(format_typed(&w, 0, true), " alpha beta gamma");
        assert_eq!(format_typed(&w, 0, false), "alpha beta gamma");
        assert_eq!(format_typed(&w, 1, true), " beta gamma");
        [(0, true), (0, false), (1, true), (2, false)]
            .into_iter()
            .for_each(|(from_idx, has_prefix)| {
                let removed = format_typed(&w, from_idx, has_prefix);
                assert_eq!(
                    typed_len_from(&w, from_idx, has_prefix),
                    removed.chars().count()
                );
            });
    }

    #[test]
    fn reconcile_revision_pure_extension_has_zero_backspace() {
        let mut session = revision_session();
        let delta = session.reconcile_revision(words("hello"));
        assert_eq!(delta, Some(Delta::Append("hello".to_string())));
        assert_eq!(session.provisional_words, words("hello"));

        let delta = session.reconcile_revision(words("hello world"));
        assert_eq!(delta, Some(Delta::Append(" world".to_string())));
        assert_eq!(session.provisional_words, words("hello world"));
    }

    #[test]
    fn reconcile_revision_no_change_emits_nothing() {
        let mut session = revision_session();
        session.reconcile_revision(words("hello world"));
        let delta = session.reconcile_revision(words("hello world"));
        assert_eq!(delta, None);
    }

    #[test]
    fn reconcile_revision_divergence_emits_char_exact_backspace() {
        let mut session = revision_session();
        session.reconcile_revision(words("turn on the lamp"));

        let delta = session.reconcile_revision(words("turn on the light"));
        match delta {
            Some(Delta::Replace { backspace, text }) => {
                assert_eq!(backspace, " lamp".chars().count());
                assert_eq!(text, " light");
            }
            other => panic!("expected a Replace correction, got {other:?}"),
        }
        assert_eq!(session.provisional_words, words("turn on the light"));
    }

    #[test]
    fn reconcile_revision_divergence_mid_tail_only_touches_wrong_suffix() {
        let mut session = revision_session();
        session.reconcile_revision(words("the cat sat on the mat"));
        assert_eq!(session.provisional_words, words("sat on the mat"));

        let delta = session.reconcile_revision(words("sat on the rug"));
        match delta {
            Some(Delta::Replace { backspace, text }) => {
                assert_eq!(backspace, " mat".chars().count());
                assert_eq!(text, " rug");
            }
            other => panic!("expected a Replace correction, got {other:?}"),
        }
    }

    #[test]
    fn reconcile_revision_confirms_behind_the_lag_and_never_revisits_it() {
        let mut session = revision_session();
        let sentence = "one two three four five six seven";
        let all_words = words(sentence);
        (1..=all_words.len()).for_each(|n| {
            let confirmed_len = session.confirmed_words.len();
            session.reconcile_revision(all_words[confirmed_len..n].to_vec());
        });
        assert!(session.provisional_words.len() <= REVISION_LAG_WORDS);
        let confirmed_and_provisional: Vec<String> = session
            .full_text_parts
            .iter()
            .flat_map(|s| s.split_whitespace().map(str::to_owned))
            .chain(session.provisional_words.iter().cloned())
            .collect();
        assert_eq!(confirmed_and_provisional, all_words);
    }

    #[test]
    fn last_n_words_returns_tail_or_whole_string() {
        assert_eq!(last_n_words("a b c d e", 3), "c d e");
        assert_eq!(last_n_words("a b", 3), "a b");
        assert_eq!(last_n_words("", 3), "");
        assert_eq!(last_n_words("a b c", 0), "");
    }

    #[test]
    fn long_session_stall_reproduction_and_fix() {
        let committed_history: Vec<&str> = (0..80)
            .map(|i| if i % 2 == 0 { "word" } else { "other" })
            .collect();
        let full_emitted = committed_history.join(" ");

        let curr = "brand new content the user just said in this window \
                     that should absolutely without question be committed \
                     as a real delta and not silently dropped on the floor";
        let curr_words: Vec<&str> = curr.split_whitespace().collect();
        assert!(curr_words.len() > SLIDING_DIFF_LOOKBACK_WORDS);

        assert_eq!(
            extract_new_text(&full_emitted, curr),
            "",
            "sanity check: this is the exact failure mode observed live"
        );

        let bounded = last_n_words(&full_emitted, SLIDING_DIFF_LOOKBACK_WORDS);
        assert_eq!(
            bounded.split_whitespace().count(),
            SLIDING_DIFF_LOOKBACK_WORDS
        );
        let expected = curr_words[SLIDING_DIFF_LOOKBACK_WORDS..].join(" ");
        assert_eq!(extract_new_text(&bounded, curr), expected);
        assert!(!expected.is_empty());
    }

    #[test]
    fn is_hallucination_matches_and_strips_punct() {
        assert!(is_hallucination("thank you."));
        assert!(is_hallucination("  YOU  "));
        assert!(!is_hallucination("hello world"));
        assert!(!is_hallucination("the quick brown fox"));
    }

    #[test]
    fn rms_measures_energy() {
        assert_eq!(rms(&[]), 0.0);
        let silence = vec![0.0; 100];
        assert_eq!(rms(&silence), 0.0);
        let loud = vec![0.5; 100];
        assert!((rms(&loud) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn from_streaming_config_maps_the_renamed_keys() {
        let streaming: StreamingConfig = toml::from_str(
            r#"
            retranscribe_interval_secs = 1.5
            min_new_words_per_partial = 2
            "#,
        )
        .expect("valid streaming config");
        let resolved = SlidingWindowConfig::from_streaming_config(&streaming, 16000);
        assert_eq!(resolved.interval_s, 1.5_f32 as f64);
        assert_eq!(resolved.partial_min_words, 2);
        assert_eq!(resolved.max_buffer_s, 29.0);
        assert_eq!(resolved.sample_rate, 16000);
        assert!(resolved.type_partials);
        assert!(!resolved.revision_mode);
    }

    struct FakeTranscriber;

    impl Transcriber for FakeTranscriber {
        fn transcribe(&self, samples: &[f32]) -> Result<String, TranscribeError> {
            let secs = samples.len() as f32 / 16_000.0;
            let text = if secs < 1.0 {
                ""
            } else if secs < 2.0 {
                "alpha beta"
            } else if secs < 3.0 {
                "alpha beta gamma"
            } else {
                "alpha beta gamma delta"
            };
            Ok(text.to_string())
        }
    }

    fn loud_samples(secs: f32) -> Vec<f32> {
        let n = (secs * 16_000.0) as usize;
        (0..n)
            .map(|i| if i % 2 == 0 { 0.05 } else { -0.05 })
            .collect()
    }

    fn streaming_config() -> SlidingWindowConfig {
        SlidingWindowConfig {
            interval_s: 0.02,
            max_buffer_s: 29.0,
            sample_rate: 16_000,
            min_speech_rms: 0.004,
            min_audio_s: 1.0,
            partial_min_words: 1,
            type_partials: true,
            revision_mode: false,
        }
    }

    async fn feed_paced(tx: &mpsc::Sender<Vec<f32>>, chunks: Vec<Vec<f32>>) {
        futures_util::stream::iter(chunks)
            .for_each(|chunk| async move {
                tx.send(chunk).await.unwrap();
                tokio::time::sleep(Duration::from_millis(60)).await;
            })
            .await;
    }

    async fn collect_until_ended(handle: &mut StreamHandle) -> Vec<StreamingEvent> {
        let mut events = Vec::new();
        while let Some(ev) = handle.events.recv().await {
            let ended = matches!(ev, StreamingEvent::Ended);
            events.push(ev);
            if ended {
                break;
            }
        }
        events
    }

    async fn run_session(config: SlidingWindowConfig) -> Vec<StreamingEvent> {
        let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber);
        let engine = SlidingWindowStreamingTranscriber::new(transcriber, config);

        let (tx, rx) = mpsc::channel::<Vec<f32>>(32);
        let mut handle = engine.start_stream(rx);

        feed_paced(&tx, (0..6).map(|_| loud_samples(0.5)).collect()).await;
        drop(tx);

        let events = collect_until_ended(&mut handle).await;
        handle.task.await.expect("task ok");
        events
    }

    fn emitted_text(events: &[StreamingEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|ev| match ev {
                StreamingEvent::Partial { text, .. } | StreamingEvent::Final { text, .. } => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect()
    }

    fn reconstruct_typed_text(events: &[StreamingEvent]) -> String {
        let mut screen = String::new();
        events.iter().for_each(|ev| match ev {
            StreamingEvent::Partial { text, .. } | StreamingEvent::Final { text, .. } => {
                screen.push_str(text);
            }
            StreamingEvent::Replace {
                backspace, text, ..
            } => {
                let keep = screen.chars().count().saturating_sub(*backspace);
                screen = screen.chars().take(keep).collect();
                screen.push_str(text);
            }
            _ => {}
        });
        screen
    }

    struct RevisingTranscriber {
        calls: std::sync::Mutex<usize>,
        sequence: Vec<&'static str>,
    }

    impl RevisingTranscriber {
        fn new(sequence: Vec<&'static str>) -> Self {
            Self {
                calls: std::sync::Mutex::new(0),
                sequence,
            }
        }

        fn call_count(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl Transcriber for RevisingTranscriber {
        fn transcribe(&self, _samples: &[f32]) -> Result<String, TranscribeError> {
            let mut calls = self.calls.lock().unwrap();
            let idx = (*calls).min(self.sequence.len() - 1);
            *calls += 1;
            Ok(self.sequence[idx].to_string())
        }
    }

    async fn run_session_with(
        transcriber: Arc<dyn Transcriber>,
        config: SlidingWindowConfig,
        ticks: usize,
    ) -> Vec<StreamingEvent> {
        let engine = SlidingWindowStreamingTranscriber::new(transcriber, config);
        let (tx, rx) = mpsc::channel::<Vec<f32>>(32);
        let mut handle = engine.start_stream(rx);

        feed_paced(&tx, (0..ticks).map(|_| loud_samples(0.5)).collect()).await;
        drop(tx);

        let events = collect_until_ended(&mut handle).await;
        handle.task.await.expect("task ok");
        events
    }

    #[tokio::test]
    async fn session_emits_stable_deltas_then_final_flush_and_ended() {
        let events = run_session(streaming_config()).await;

        assert!(matches!(events.last(), Some(StreamingEvent::Ended)));
        assert!(events.len() >= 3, "expected partials + final + ended");

        let parts = emitted_text(&events);
        assert_eq!(parts.concat(), "alpha beta gamma delta");
        (1..parts.len()).for_each(|i| {
            let tail = parts[..i].concat();
            assert!(!tail.contains(parts[i].trim()), "repeated text: {parts:?}");
        });
    }

    #[tokio::test]
    async fn commit_only_mode_emits_final_deltas() {
        let mut cfg = streaming_config();
        cfg.type_partials = false;
        let events = run_session(cfg).await;

        let parts = emitted_text(&events);
        assert_eq!(parts.concat(), "alpha beta gamma delta");
        assert!(events
            .iter()
            .all(|ev| !matches!(ev, StreamingEvent::Partial { .. })));
    }

    #[tokio::test]
    async fn revision_mode_end_to_end_reconstructs_final_transcript_via_growth_only() {
        let mut cfg = streaming_config();
        cfg.revision_mode = true;
        let events = run_session(cfg).await;

        assert!(matches!(events.last(), Some(StreamingEvent::Ended)));
        assert_eq!(reconstruct_typed_text(&events), "alpha beta gamma delta");
    }

    #[tokio::test]
    async fn revision_mode_end_to_end_corrects_a_misheard_word() {
        let transcriber: Arc<dyn Transcriber> = Arc::new(RevisingTranscriber::new(vec![
            "",
            "",
            "turn on the lamp",
            "turn on the lamp",
            "turn on the light",
            "turn on the light",
        ]));
        let mut cfg = streaming_config();
        cfg.revision_mode = true;
        let events = run_session_with(transcriber, cfg, 6).await;

        assert!(matches!(events.last(), Some(StreamingEvent::Ended)));
        assert!(
            events
                .iter()
                .any(|ev| matches!(ev, StreamingEvent::Replace { .. })),
            "expected a correction event, got {events:?}"
        );
        assert_eq!(reconstruct_typed_text(&events), "turn on the light");
    }

    #[tokio::test]
    async fn decoding_events_pair_around_every_transcription() {
        let revising = Arc::new(RevisingTranscriber::new(vec![
            "",
            "",
            "turn on the lamp",
            "turn on the lamp",
            "turn on the light",
            "turn on the light",
        ]));
        let transcriber: Arc<dyn Transcriber> = revising.clone();
        let mut cfg = streaming_config();
        cfg.revision_mode = true;
        let events = run_session_with(transcriber, cfg, 6).await;

        let decoding: Vec<bool> = events
            .iter()
            .filter_map(|ev| match ev {
                StreamingEvent::Decoding(active) => Some(*active),
                _ => None,
            })
            .collect();
        let alternating = decoding
            .iter()
            .enumerate()
            .all(|(i, active)| *active == (i % 2 == 0));
        assert!(
            alternating,
            "decoding must start and finish in pairs: {decoding:?}"
        );
        assert_eq!(decoding.len(), 2 * revising.call_count());
        assert_ne!(decoding.last(), Some(&true));
    }

    #[tokio::test]
    async fn silence_only_buffer_emits_no_events() {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(32);
        let engine =
            SlidingWindowStreamingTranscriber::new(Arc::new(FakeTranscriber), streaming_config());
        let mut handle = engine.start_stream(rx);

        tx.send(vec![0.0; 16000]).await.unwrap();
        tx.send(vec![0.0; 16000]).await.unwrap();
        tx.send(vec![0.0; 16000]).await.unwrap();
        drop(tx);

        let events = collect_until_ended(&mut handle).await;
        assert!(matches!(events.last(), Some(StreamingEvent::Ended)));
        assert_eq!(
            emitted_text(&events).len(),
            0,
            "silence should not transcribe"
        );
        handle.task.await.unwrap();
    }
}
