use std::path::Path;
use std::time::{Duration, Instant};

use evdev::{AttributeSet, Device, KeyCode as Key};

const MODIFIER_KEYS: &[Key] = &[
    Key::KEY_LEFTCTRL,
    Key::KEY_RIGHTCTRL,
    Key::KEY_LEFTALT,
    Key::KEY_RIGHTALT,
    Key::KEY_LEFTSHIFT,
    Key::KEY_RIGHTSHIFT,
    Key::KEY_LEFTMETA,
    Key::KEY_RIGHTMETA,
];

pub enum ModifierGuard {
    Active { devices: Vec<Device> },
    Disabled,
}

impl Default for ModifierGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ModifierGuard {
    pub fn new() -> Self {
        let Ok(entries) = std::fs::read_dir("/dev/input") else {
            tracing::debug!("modifier_guard: /dev/input not readable, disabling");
            return Self::Disabled;
        };
        let devices: Vec<Device> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_event_node(path))
            .filter_map(|path| Device::open(path).ok())
            .filter(is_keyboard)
            .collect();
        if devices.is_empty() {
            tracing::debug!("modifier_guard: no readable keyboard devices, disabling");
            Self::Disabled
        } else {
            tracing::debug!(
                count = devices.len(),
                "modifier_guard: tracking keyboard device(s)"
            );
            Self::Active { devices }
        }
    }

    pub fn any_modifier_held(&mut self) -> bool {
        let Self::Active { devices } = self else {
            return false;
        };
        devices.iter_mut().any(|device| {
            device
                .get_key_state()
                .is_ok_and(|state: AttributeSet<Key>| {
                    MODIFIER_KEYS.iter().any(|key| state.contains(*key))
                })
        })
    }

    pub async fn wait_for_release(&mut self, timeout: Duration) -> Result<(), Timeout> {
        if matches!(self, Self::Disabled) {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        let poll = Duration::from_millis(15);
        while self.any_modifier_held() {
            if Instant::now() >= deadline {
                return Err(Timeout);
            }
            tokio::time::sleep(poll).await;
        }
        Ok(())
    }
}

fn is_event_node(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("event"))
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(Key::KEY_A) && keys.contains(Key::KEY_Z) && keys.contains(Key::KEY_ENTER)
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Timeout;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_guard_returns_immediately() {
        let mut guard = ModifierGuard::Disabled;
        let start = Instant::now();
        let res = guard.wait_for_release(Duration::from_secs(60)).await;
        assert!(res.is_ok());
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn disabled_guard_reports_no_modifier_held() {
        let mut guard = ModifierGuard::Disabled;
        assert!(!guard.any_modifier_held());
    }

    #[test]
    fn new_does_not_panic_without_permission() {
        let _ = ModifierGuard::new();
    }
}
