use notulist_client::{SessionOutcome, TranscriptEdit, REVISION_LAG_WORDS};

use crate::input_method::preedit_commit_tracker::{InputMethodBatch, PreeditCommitTracker};
use crate::input_method::TextFieldFocus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypingAction {
    Nothing,
    SendBatch { field: u32, batch: InputMethodBatch },
    TypeWithVirtualKeyboard(TranscriptEdit),
}

#[derive(Debug)]
enum TypingTarget {
    InputMethodField {
        field: u32,
        tracker: PreeditCommitTracker,
    },
    VirtualKeyboard {
        typed_chars: usize,
    },
    SensitiveField,
    Discard,
}

#[derive(Debug)]
pub struct DictationTypingSession {
    target: TypingTarget,
}

impl DictationTypingSession {
    pub fn begin(focus: TextFieldFocus, fall_back_to_dotool: bool) -> Self {
        let target = match focus {
            TextFieldFocus::Focused {
                sensitive: true, ..
            } => TypingTarget::SensitiveField,
            TextFieldFocus::Focused { field, .. } => TypingTarget::InputMethodField {
                field,
                tracker: PreeditCommitTracker::new(REVISION_LAG_WORDS + 1),
            },
            TextFieldFocus::Connecting
            | TextFieldFocus::Unfocused
            | TextFieldFocus::Unavailable
                if fall_back_to_dotool =>
            {
                TypingTarget::VirtualKeyboard { typed_chars: 0 }
            }
            TextFieldFocus::Connecting
            | TextFieldFocus::Unfocused
            | TextFieldFocus::Unavailable => TypingTarget::Discard,
        };
        Self { target }
    }

    pub fn input_method_field(&self) -> Option<u32> {
        match self.target {
            TypingTarget::InputMethodField { field, .. } => Some(field),
            TypingTarget::VirtualKeyboard { .. }
            | TypingTarget::SensitiveField
            | TypingTarget::Discard => None,
        }
    }

    pub fn types_with_virtual_keyboard(&self) -> bool {
        matches!(self.target, TypingTarget::VirtualKeyboard { .. })
    }

    pub fn refuses_input(&self) -> bool {
        matches!(self.target, TypingTarget::SensitiveField)
    }

    pub fn discards_text(&self) -> bool {
        matches!(self.target, TypingTarget::Discard)
    }

    pub fn lost_its_field(&self, focus: TextFieldFocus) -> bool {
        match self.target {
            TypingTarget::InputMethodField { field, .. } => focus.writable_field() != Some(field),
            TypingTarget::VirtualKeyboard { .. }
            | TypingTarget::SensitiveField
            | TypingTarget::Discard => false,
        }
    }

    pub fn apply(&mut self, edit: &TranscriptEdit) -> TypingAction {
        match &mut self.target {
            TypingTarget::InputMethodField { field, tracker } => TypingAction::SendBatch {
                field: *field,
                batch: tracker.replace_trailing_chars(edit.delete_chars as usize, &edit.insert),
            },
            TypingTarget::VirtualKeyboard { typed_chars } => {
                let deleted = (edit.delete_chars as usize).min(*typed_chars);
                *typed_chars = *typed_chars - deleted + edit.insert.chars().count();
                TypingAction::TypeWithVirtualKeyboard(TranscriptEdit {
                    delete_chars: deleted as u32,
                    insert: edit.insert.clone(),
                })
            }
            TypingTarget::SensitiveField | TypingTarget::Discard => TypingAction::Nothing,
        }
    }

    pub fn finish(mut self, outcome: SessionOutcome) -> TypingAction {
        match (&mut self.target, outcome) {
            (TypingTarget::InputMethodField { field, tracker }, SessionOutcome::Committed) => {
                TypingAction::SendBatch {
                    field: *field,
                    batch: tracker.commit_everything(),
                }
            }
            (TypingTarget::InputMethodField { field, tracker }, SessionOutcome::Cancelled) => {
                TypingAction::SendBatch {
                    field: *field,
                    batch: tracker.erase_everything(),
                }
            }
            (TypingTarget::VirtualKeyboard { typed_chars }, SessionOutcome::Cancelled)
                if *typed_chars > 0 =>
            {
                TypingAction::TypeWithVirtualKeyboard(TranscriptEdit {
                    delete_chars: *typed_chars as u32,
                    insert: String::new(),
                })
            }
            _ => TypingAction::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN_FIELD: TextFieldFocus = TextFieldFocus::Focused {
        field: 4,
        sensitive: false,
    };

    fn batch(delete_before_bytes: u32, commit_text: &str, preedit_text: &str) -> InputMethodBatch {
        InputMethodBatch {
            delete_before_bytes,
            commit_text: commit_text.to_owned(),
            preedit_text: preedit_text.to_owned(),
        }
    }

    #[test]
    fn focused_field_receives_preedit_and_a_final_commit() {
        let mut session = DictationTypingSession::begin(PLAIN_FIELD, true);
        assert_eq!(session.input_method_field(), Some(4));
        assert_eq!(
            session.apply(&TranscriptEdit::append("hello there")),
            TypingAction::SendBatch {
                field: 4,
                batch: batch(0, "", "hello there")
            }
        );
        assert_eq!(
            session.finish(SessionOutcome::Committed),
            TypingAction::SendBatch {
                field: 4,
                batch: batch(0, "hello there", "")
            }
        );
    }

    #[test]
    fn cancelling_erases_what_the_field_received() {
        let mut session = DictationTypingSession::begin(PLAIN_FIELD, true);
        session.apply(&TranscriptEdit::append("one two three four five six seven"));
        let TypingAction::SendBatch { batch, .. } = session.finish(SessionOutcome::Cancelled)
        else {
            panic!("expected a batch");
        };
        assert!(batch.delete_before_bytes > 0);
        assert!(batch.preedit_text.is_empty());
    }

    #[test]
    fn without_a_text_input_field_dictation_uses_the_virtual_keyboard() {
        let mut session = DictationTypingSession::begin(TextFieldFocus::Unfocused, true);
        assert!(session.types_with_virtual_keyboard());
        session.apply(&TranscriptEdit::append("abc"));
        assert_eq!(
            session.apply(&TranscriptEdit {
                delete_chars: 10,
                insert: "xy".into()
            }),
            TypingAction::TypeWithVirtualKeyboard(TranscriptEdit {
                delete_chars: 3,
                insert: "xy".into()
            })
        );
        assert_eq!(
            session.finish(SessionOutcome::Cancelled),
            TypingAction::TypeWithVirtualKeyboard(TranscriptEdit {
                delete_chars: 2,
                insert: String::new()
            })
        );
    }

    #[test]
    fn without_fallback_unfocused_dictation_types_nothing() {
        let mut session = DictationTypingSession::begin(TextFieldFocus::Unfocused, false);
        assert!(session.discards_text());
        assert!(!session.types_with_virtual_keyboard());
        assert_eq!(
            session.apply(&TranscriptEdit::append("abc")),
            TypingAction::Nothing
        );
        assert_eq!(
            session.finish(SessionOutcome::Cancelled),
            TypingAction::Nothing
        );
    }

    #[test]
    fn sensitive_fields_never_receive_text() {
        let mut session = DictationTypingSession::begin(
            TextFieldFocus::Focused {
                field: 9,
                sensitive: true,
            },
            true,
        );
        assert!(session.refuses_input());
        assert_eq!(
            session.apply(&TranscriptEdit::append("secret")),
            TypingAction::Nothing
        );
    }

    #[test]
    fn moving_focus_away_loses_the_field() {
        let session = DictationTypingSession::begin(PLAIN_FIELD, true);
        assert!(!session.lost_its_field(PLAIN_FIELD));
        assert!(session.lost_its_field(TextFieldFocus::Unfocused));
        assert!(session.lost_its_field(TextFieldFocus::Focused {
            field: 5,
            sensitive: false
        }));
    }
}
