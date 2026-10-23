use crate::dictation_types::TranscriptEdit;

#[derive(Debug, Clone, Default)]
pub struct TranscriptAccumulator {
    text: String,
}

impl TranscriptAccumulator {
    pub fn apply(&mut self, edit: &TranscriptEdit) {
        let kept_chars = self
            .text
            .chars()
            .count()
            .saturating_sub(edit.delete_chars as usize);
        let kept_bytes = self
            .text
            .char_indices()
            .nth(kept_chars)
            .map_or(self.text.len(), |(offset, _)| offset);
        self.text.truncate(kept_bytes);
        self.text.push_str(&edit.insert);
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_revises_by_unicode_scalars() {
        let mut transcript = TranscriptAccumulator::default();
        transcript.apply(&TranscriptEdit::append("open blue café"));
        transcript.apply(&TranscriptEdit {
            delete_chars: 4,
            insert: "bubbles".into(),
        });
        assert_eq!(transcript.text(), "open blue bubbles");
    }

    #[test]
    fn deleting_past_the_start_clears_the_text() {
        let mut transcript = TranscriptAccumulator::default();
        transcript.apply(&TranscriptEdit::append("ab"));
        transcript.apply(&TranscriptEdit {
            delete_chars: 10,
            insert: "x".into(),
        });
        assert_eq!(transcript.into_text(), "x");
    }
}
