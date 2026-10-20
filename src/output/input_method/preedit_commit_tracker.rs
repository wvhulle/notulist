#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputMethodBatch {
    pub delete_before_bytes: u32,
    pub commit_text: String,
    pub preedit_text: String,
}

#[derive(Debug, Clone)]
pub struct PreeditCommitTracker {
    committed: String,
    preedit: String,
    revisable_words: usize,
}

impl PreeditCommitTracker {
    pub fn new(revisable_words: usize) -> Self {
        Self {
            committed: String::new(),
            preedit: String::new(),
            revisable_words,
        }
    }

    pub fn typed_chars(&self) -> usize {
        self.committed.chars().count() + self.preedit.chars().count()
    }

    pub fn append(&mut self, text: &str) -> InputMethodBatch {
        self.replace_trailing_chars(0, text)
    }

    pub fn replace_trailing_chars(&mut self, removed_chars: usize, text: &str) -> InputMethodBatch {
        let preedit_chars = self.preedit.chars().count();
        let delete_before_bytes = if removed_chars <= preedit_chars {
            truncate_trailing_chars(&mut self.preedit, removed_chars);
            0
        } else {
            self.preedit.clear();
            truncate_trailing_chars(&mut self.committed, removed_chars - preedit_chars)
        };
        self.preedit.push_str(text);
        let commit_text = self.promote_settled_words();
        InputMethodBatch {
            delete_before_bytes,
            commit_text,
            preedit_text: self.preedit.clone(),
        }
    }

    pub fn commit_everything(&mut self) -> InputMethodBatch {
        let commit_text = std::mem::take(&mut self.preedit);
        self.committed.push_str(&commit_text);
        InputMethodBatch {
            delete_before_bytes: 0,
            commit_text,
            preedit_text: String::new(),
        }
    }

    pub fn erase_everything(&mut self) -> InputMethodBatch {
        self.preedit.clear();
        let delete_before_bytes = byte_count(&std::mem::take(&mut self.committed));
        InputMethodBatch {
            delete_before_bytes,
            commit_text: String::new(),
            preedit_text: String::new(),
        }
    }

    fn promote_settled_words(&mut self) -> String {
        let word_starts: Vec<usize> = word_start_offsets(&self.preedit);
        if word_starts.len() <= self.revisable_words {
            return String::new();
        }
        let settled_end = word_starts
            .get(word_starts.len() - self.revisable_words)
            .copied()
            .unwrap_or(self.preedit.len());
        let settled: String = self.preedit.drain(..settled_end).collect();
        self.committed.push_str(&settled);
        settled
    }
}

fn word_start_offsets(text: &str) -> Vec<usize> {
    let mut previous_was_space = true;
    text.char_indices()
        .filter_map(|(offset, character)| {
            let starts_word = previous_was_space && !character.is_whitespace();
            previous_was_space = character.is_whitespace();
            starts_word.then_some(offset)
        })
        .collect()
}

fn truncate_trailing_chars(text: &mut String, removed_chars: usize) -> u32 {
    let kept_chars = text.chars().count().saturating_sub(removed_chars);
    let kept_bytes = text
        .char_indices()
        .nth(kept_chars)
        .map_or(text.len(), |(offset, _)| offset);
    let removed = text.split_off(kept_bytes);
    byte_count(&removed)
}

fn byte_count(text: &str) -> u32 {
    u32::try_from(text.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(delete_before_bytes: u32, commit_text: &str, preedit_text: &str) -> InputMethodBatch {
        InputMethodBatch {
            delete_before_bytes,
            commit_text: commit_text.to_owned(),
            preedit_text: preedit_text.to_owned(),
        }
    }

    #[test]
    fn appended_words_stay_in_preedit_within_the_revisable_window() {
        let mut tracker = PreeditCommitTracker::new(3);
        assert_eq!(tracker.append("hello there"), batch(0, "", "hello there"));
        assert_eq!(tracker.append(" big"), batch(0, "", "hello there big"));
        assert_eq!(tracker.typed_chars(), 15);
    }

    #[test]
    fn words_beyond_the_revisable_window_are_committed() {
        let mut tracker = PreeditCommitTracker::new(3);
        tracker.append("one two three");
        assert_eq!(
            tracker.append(" four five"),
            batch(0, "one two ", "three four five")
        );
        assert_eq!(tracker.typed_chars(), 23);
    }

    #[test]
    fn replacement_inside_the_preedit_only_edits_the_preedit() {
        let mut tracker = PreeditCommitTracker::new(3);
        tracker.append("open blue button");
        assert_eq!(
            tracker.replace_trailing_chars(6, "bubbles"),
            batch(0, "", "open blue bubbles")
        );
    }

    #[test]
    fn replacement_reaching_into_committed_text_deletes_its_utf8_bytes() {
        let mut tracker = PreeditCommitTracker::new(1);
        assert_eq!(tracker.append("café au"), batch(0, "café ", "au"));
        assert_eq!(tracker.append(" lait"), batch(0, "au ", "lait"));
        assert_eq!(
            tracker.replace_trailing_chars(9, "é noir"),
            batch(6, "é ", "noir")
        );
        assert_eq!(tracker.typed_chars(), 9);
    }

    #[test]
    fn replacement_never_deletes_more_than_this_session_committed() {
        let mut tracker = PreeditCommitTracker::new(1);
        tracker.append("ab cd");
        assert_eq!(tracker.replace_trailing_chars(50, "x"), batch(3, "", "x"));
    }

    #[test]
    fn zero_revisable_words_commits_everything_at_once() {
        let mut tracker = PreeditCommitTracker::new(0);
        assert_eq!(tracker.append("all of it"), batch(0, "all of it", ""));
    }

    #[test]
    fn commit_everything_flushes_the_preedit() {
        let mut tracker = PreeditCommitTracker::new(5);
        tracker.append("final words");
        assert_eq!(tracker.commit_everything(), batch(0, "final words", ""));
        assert_eq!(tracker.typed_chars(), 11);
    }

    #[test]
    fn erase_everything_removes_committed_text_and_preedit() {
        let mut tracker = PreeditCommitTracker::new(1);
        tracker.append("ünï code");
        assert_eq!(tracker.erase_everything(), batch(6, "", ""));
        assert_eq!(tracker.typed_chars(), 0);
    }
}
