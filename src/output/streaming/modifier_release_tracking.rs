use super::StreamingSession;

impl StreamingSession {
    pub fn modifiers_released_since_start(&self) -> bool {
        self.modifiers_released_since_start
    }

    pub fn mark_modifiers_released(&mut self) {
        self.modifiers_released_since_start = true;
    }
}
