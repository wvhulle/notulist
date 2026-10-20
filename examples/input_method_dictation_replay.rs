use std::time::Duration;

use voxtype::config::OutputConfig;
use voxtype::error::OutputError;
use voxtype::output::input_method::{
    text_field_focus_changes, InputMethodFieldWriter, TextFieldFocus,
};

enum ReplayStep {
    Append(&'static str),
    ReplaceTrailingChars(usize, &'static str),
}

const DICTATION_SCRIPT: &[ReplayStep] = &[
    ReplayStep::Append("this is a"),
    ReplayStep::Append(" test of"),
    ReplayStep::Append(" streaming diction"),
    ReplayStep::ReplaceTrailingChars(7, "dictation"),
    ReplayStep::Append(" that corrects"),
    ReplayStep::Append(" it shelf"),
    ReplayStep::ReplaceTrailingChars(8, "itself in place"),
    ReplayStep::Append(" and never"),
    ReplayStep::Append(" presses a key."),
    ReplayStep::ReplaceTrailingChars(30, "place and never touches the keyboard."),
];

const STEP_PAUSE: Duration = Duration::from_millis(900);
const FOCUS_COUNTDOWN_SECS: u64 = 5;

#[tokio::main]
async fn main() {
    let config = OutputConfig::default();
    let Some(mut focus_changes) = text_field_focus_changes(&config) else {
        eprintln!("The input-method driver is not enabled in this build or config.");
        return;
    };

    println!("Focus the text field to dictate into; replay starts in {FOCUS_COUNTDOWN_SECS} s.");
    for remaining in (1..=FOCUS_COUNTDOWN_SECS).rev() {
        println!("  {remaining}…");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let field_focus = *focus_changes.borrow_and_update();
    match field_focus {
        TextFieldFocus::Focused { field } => println!("Dictating into text field #{field}."),
        TextFieldFocus::Unavailable => {
            eprintln!(
                "Another input method holds the seat. Stop it first, for example: systemctl --user stop voxtype"
            );
            return;
        }
        TextFieldFocus::Connecting | TextFieldFocus::Unfocused => {
            eprintln!(
                "No focused text field accepts input-method text (state: {field_focus:?}). \
                 Try a GTK app, Firefox or kitty."
            );
            return;
        }
    }

    let Some(mut writer) = InputMethodFieldWriter::for_focused_field(&config) else {
        eprintln!("The text field lost focus before the replay started.");
        return;
    };

    for step in DICTATION_SCRIPT {
        let sent = match step {
            ReplayStep::Append(text) => {
                println!("append   {text:?}");
                writer.append(text)
            }
            ReplayStep::ReplaceTrailingChars(removed_chars, text) => {
                println!("replace  last {removed_chars} chars with {text:?}");
                writer.replace_trailing_chars(*removed_chars, text)
            }
        };
        if let Err(OutputError::TextFieldNotFocused) = sent {
            println!("Focus left the text field; stopping like the daemon would.");
            return;
        }
        tokio::time::sleep(STEP_PAUSE).await;
    }

    drop(writer);
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("Committed the remaining provisional words.");
}
