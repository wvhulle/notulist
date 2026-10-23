pub mod dictation_indicator_painter;
pub mod indicator_phase;
mod indicator_popup;
pub mod level_bars;
pub mod preedit_commit_tracker;
mod shm;
pub mod wayland_input_method_connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldFocus {
    Connecting,
    Unfocused,
    Focused { field: u32, sensitive: bool },
    Unavailable,
}

impl TextFieldFocus {
    pub fn focused_field(self) -> Option<u32> {
        match self {
            TextFieldFocus::Focused { field, .. } => Some(field),
            TextFieldFocus::Connecting
            | TextFieldFocus::Unfocused
            | TextFieldFocus::Unavailable => None,
        }
    }

    pub fn writable_field(self) -> Option<u32> {
        match self {
            TextFieldFocus::Focused {
                field,
                sensitive: false,
            } => Some(field),
            _ => None,
        }
    }

    pub fn is_sensitive(self) -> bool {
        matches!(
            self,
            TextFieldFocus::Focused {
                sensitive: true,
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_fields_are_focused_but_not_writable() {
        let password_field = TextFieldFocus::Focused {
            field: 3,
            sensitive: true,
        };
        assert_eq!(password_field.focused_field(), Some(3));
        assert_eq!(password_field.writable_field(), None);
        assert!(password_field.is_sensitive());
    }
}
