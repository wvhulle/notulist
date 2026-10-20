use crate::output::streaming::StreamingSession;
use crate::transcribe::streaming::StreamingEvent;

#[cfg(target_os = "linux")]
const START_CHORD_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub async fn held_modifier_interrupts_typing(
    session: Option<&mut StreamingSession>,
    file_output: bool,
    event: Option<&StreamingEvent>,
) -> bool {
    let Some(session) = session else {
        return false;
    };
    if file_output || !carries_typed_text(event) {
        return false;
    }
    let interrupted = !modifiers_settled_for_typing(session).await;
    if interrupted {
        tracing::info!(
            "Modifier key pressed while streaming; stopping the session instead of typing into a shortcut"
        );
    }
    interrupted
}

fn carries_typed_text(event: Option<&StreamingEvent>) -> bool {
    matches!(
        event,
        Some(
            StreamingEvent::Partial { .. }
                | StreamingEvent::Final { .. }
                | StreamingEvent::Replace { .. }
        )
    )
}

#[cfg(target_os = "linux")]
async fn modifiers_settled_for_typing(session: &mut StreamingSession) -> bool {
    let mut guard = crate::output::modifier_guard::ModifierGuard::new();
    if !guard.any_modifier_held() {
        session.mark_modifiers_released();
        return true;
    }
    if session.modifiers_released_since_start() {
        return false;
    }
    let released = guard
        .wait_for_release(START_CHORD_RELEASE_TIMEOUT)
        .await
        .is_ok();
    if released {
        session.mark_modifiers_released();
    }
    released
}

#[cfg(not(target_os = "linux"))]
async fn modifiers_settled_for_typing(_session: &mut StreamingSession) -> bool {
    true
}
