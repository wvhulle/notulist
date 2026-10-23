use std::str::FromStr;

use evdev::KeyCode;
use serde::Deserialize;

use crate::error::ShortcutError;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub enabled: bool,
    pub hold_keys: Vec<String>,
    pub toggle_keys: Vec<String>,
    pub cancel_keys: Vec<String>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hold_keys: vec!["LEFTMETA".to_owned(), "V".to_owned()],
            toggle_keys: vec!["LEFTMETA".to_owned(), "T".to_owned()],
            cancel_keys: vec![
                "LEFTMETA".to_owned(),
                "LEFTSHIFT".to_owned(),
                "V".to_owned(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutChords {
    pub hold_keys: Vec<KeyCode>,
    pub toggle_keys: Vec<KeyCode>,
    pub cancel_keys: Vec<KeyCode>,
}

impl ShortcutChords {
    pub fn is_empty(&self) -> bool {
        self.hold_keys.is_empty() && self.toggle_keys.is_empty() && self.cancel_keys.is_empty()
    }
}

impl ShortcutConfig {
    pub fn parse_chords(&self) -> Result<ShortcutChords, ShortcutError> {
        Ok(ShortcutChords {
            hold_keys: parse_key_names(&self.hold_keys)?,
            toggle_keys: parse_key_names(&self.toggle_keys)?,
            cancel_keys: parse_key_names(&self.cancel_keys)?,
        })
    }
}

fn parse_key_names(names: &[String]) -> Result<Vec<KeyCode>, ShortcutError> {
    names.iter().map(|name| parse_key_name(name)).collect()
}

fn parse_key_name(name: &str) -> Result<KeyCode, ShortcutError> {
    let bare_name = name.strip_prefix("KEY_").unwrap_or(name);
    let Ok(key) = KeyCode::from_str(&format!("KEY_{}", bare_name.to_ascii_uppercase())) else {
        tracing::error!(
            name,
            "[daemon.shortcut] contains an unknown evdev key name; use names like LEFTMETA, LEFTCTRL or V"
        );
        return Err(ShortcutError::UnknownKeyName);
    };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chords_are_super_v_super_t_and_super_shift_v() {
        let chords = ShortcutConfig::default().parse_chords().expect("parses");
        assert_eq!(
            chords.cancel_keys,
            vec![
                KeyCode::KEY_LEFTMETA,
                KeyCode::KEY_LEFTSHIFT,
                KeyCode::KEY_V
            ]
        );
        assert_eq!(
            chords.hold_keys,
            vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_V]
        );
        assert_eq!(
            chords.toggle_keys,
            vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_T]
        );
    }

    #[test]
    fn key_names_accept_prefix_and_lowercase() {
        let config = ShortcutConfig {
            hold_keys: vec!["KEY_LEFTCTRL".to_owned(), "space".to_owned()],
            ..ShortcutConfig::default()
        };
        assert_eq!(
            config.parse_chords().expect("parses").hold_keys,
            vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_SPACE]
        );
    }

    #[test]
    fn unknown_key_name_is_rejected() {
        let config = ShortcutConfig {
            toggle_keys: vec!["LEFTMETA".to_owned(), "NOTAKEY".to_owned()],
            ..ShortcutConfig::default()
        };
        assert!(matches!(
            config.parse_chords(),
            Err(ShortcutError::UnknownKeyName)
        ));
    }

    #[test]
    fn empty_lists_disable_chords() {
        let config = ShortcutConfig {
            hold_keys: Vec::new(),
            toggle_keys: Vec::new(),
            cancel_keys: Vec::new(),
            ..ShortcutConfig::default()
        };
        assert!(config.parse_chords().expect("parses").is_empty());
    }
}
