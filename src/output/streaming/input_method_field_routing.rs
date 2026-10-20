use super::StreamingSession;
use crate::error::OutputError;
use crate::output::input_method::InputMethodFieldWriter;
use crate::output::{output_with_fallback, OutputOptions, TextOutput};
use crate::text::TextProcessor;

impl StreamingSession {
    pub fn with_input_method_field(mut self, field: Option<InputMethodFieldWriter>) -> Self {
        self.input_method_field = field;
        self
    }

    pub fn writes_into_input_method_field(&self) -> bool {
        self.input_method_field.is_some()
    }

    pub fn input_method_field(&self) -> Option<u32> {
        self.input_method_field
            .as_ref()
            .map(InputMethodFieldWriter::field)
    }

    pub(super) async fn emit_text(
        &mut self,
        chain: &[Box<dyn TextOutput>],
        text: &str,
        opts: OutputOptions<'_>,
    ) -> Result<(), OutputError> {
        match self.input_method_field.as_mut() {
            Some(field) => field.append(text),
            None => output_with_fallback(chain, text, opts).await,
        }
    }

    pub(super) fn replace_in_input_method_field(
        &mut self,
        backspace: usize,
        text: &str,
        text_processor: Option<&TextProcessor>,
    ) -> Result<(), OutputError> {
        let removed_chars = backspace.min(self.typed_chars);
        let processed = text_processor.map(|tp| tp.process(text));
        let text = processed.as_deref().unwrap_or(text);
        if let Some(field) = self.input_method_field.as_mut() {
            field.replace_trailing_chars(removed_chars, text)?;
        }
        let kept_partial_chars = self.partial.chars().count().saturating_sub(removed_chars);
        self.partial = self.partial.chars().take(kept_partial_chars).collect();
        self.typed_chars = self.typed_chars - removed_chars + text.chars().count();
        if !text.is_empty() {
            let finalized_tail = format!("{}{}", self.partial, text);
            self.finalized_text.push_str(&finalized_tail);
        }
        self.clear_partial();
        Ok(())
    }

    pub(super) fn rewind_input_method_field(&mut self) -> Option<Result<(), OutputError>> {
        let field = self.input_method_field.as_mut()?;
        let erased = field.erase_everything();
        if erased.is_ok() {
            self.typed_chars = 0;
        }
        Some(erased)
    }
}
