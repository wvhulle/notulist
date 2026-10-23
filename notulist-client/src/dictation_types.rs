use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

pub const DICTATION_BUS_NAME: &str = "io.github.wvhulle.Notulist";
pub const DICTATION_OBJECT_PATH: &str = "/io/github/wvhulle/Notulist";
pub const DICTATION_INTERFACE: &str = "io.github.wvhulle.Notulist.Dictation1";

pub const REVISION_LAG_WORDS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[zvariant(signature = "s")]
pub enum DictationPhase {
    #[default]
    LoadingModel,
    Idle,
    Listening,
    Finishing,
    Failed,
}

impl DictationPhase {
    pub fn label(self) -> &'static str {
        match self {
            DictationPhase::LoadingModel => "loading_model",
            DictationPhase::Idle => "idle",
            DictationPhase::Listening => "listening",
            DictationPhase::Finishing => "finishing",
            DictationPhase::Failed => "failed",
        }
    }

    pub fn accepts_new_session(self) -> bool {
        matches!(self, DictationPhase::Idle | DictationPhase::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[zvariant(signature = "s")]
pub enum DictationFailure {
    MicrophoneUnavailable,
    MicrophoneDisconnected,
    ModelNotLoaded,
    InferenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[zvariant(signature = "s")]
pub enum SessionOutcome {
    Committed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, Type)]
pub struct LevelFrame {
    pub seq: u32,
    pub min: f64,
    pub max: f64,
    pub peak_dbfs: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
pub struct TranscriptEdit {
    pub delete_chars: u32,
    pub insert: String,
}

impl TranscriptEdit {
    pub fn append(insert: impl Into<String>) -> Self {
        Self {
            delete_chars: 0,
            insert: insert.into(),
        }
    }

    pub fn is_noop(&self) -> bool {
        self.delete_chars == 0 && self.insert.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::serialized::{Context, Format};
    use zbus::zvariant::{to_bytes, LE};

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de> + Type,
    {
        let context = Context::new(Format::DBus, LE, 0);
        let encoded = to_bytes(context, value).expect("encodes");
        encoded.deserialize().expect("decodes").0
    }

    #[test]
    fn phases_travel_as_their_variant_names() {
        assert_eq!(DictationPhase::signature(), "s");
        [
            DictationPhase::LoadingModel,
            DictationPhase::Idle,
            DictationPhase::Listening,
            DictationPhase::Finishing,
            DictationPhase::Failed,
        ]
        .into_iter()
        .for_each(|phase| assert_eq!(round_trip(&phase), phase));
    }

    #[test]
    fn failures_travel_as_their_variant_names() {
        assert_eq!(DictationFailure::signature(), "s");
        [
            DictationFailure::MicrophoneUnavailable,
            DictationFailure::MicrophoneDisconnected,
            DictationFailure::ModelNotLoaded,
            DictationFailure::InferenceFailed,
        ]
        .into_iter()
        .for_each(|failure| assert_eq!(round_trip(&failure), failure));
    }

    #[test]
    fn only_idle_and_failed_accept_a_new_session() {
        assert!(DictationPhase::Idle.accepts_new_session());
        assert!(DictationPhase::Failed.accepts_new_session());
        assert!(!DictationPhase::LoadingModel.accepts_new_session());
        assert!(!DictationPhase::Listening.accepts_new_session());
        assert!(!DictationPhase::Finishing.accepts_new_session());
    }

    #[test]
    fn level_batches_round_trip() {
        let frames = vec![
            LevelFrame {
                seq: 7,
                min: -0.25,
                max: 0.5,
                peak_dbfs: -6.0,
            },
            LevelFrame::default(),
        ];
        assert_eq!(<Vec<LevelFrame>>::signature(), "a(uddd)");
        assert_eq!(round_trip(&frames), frames);
    }

    #[test]
    fn transcript_edits_round_trip() {
        let edit = TranscriptEdit {
            delete_chars: 3,
            insert: "café".into(),
        };
        assert_eq!(TranscriptEdit::signature(), "(us)");
        assert_eq!(round_trip(&edit), edit);
    }

    #[test]
    fn outcome_travels_as_a_string() {
        assert_eq!(
            round_trip(&SessionOutcome::Cancelled),
            SessionOutcome::Cancelled
        );
    }
}
