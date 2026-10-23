use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::PathBuf;
use std::process::Stdio;

use notulist_client::TranscriptEdit;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::ImeError;

pub async fn type_edit_with_dotool(edit: &TranscriptEdit) -> Result<(), ImeError> {
    let commands = dotool_commands_for_edit(edit);
    if commands.is_empty() {
        return Ok(());
    }
    let mut command = match live_dotoold_pipe() {
        Some(pipe) => {
            let mut command = Command::new("dotoolc");
            command.env("DOTOOL_PIPE", pipe);
            command
        }
        None => Command::new("dotool"),
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ImeError::Dotool)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(commands.as_bytes())
            .await
            .map_err(ImeError::Dotool)?;
    }
    let output = child.wait_with_output().await.map_err(ImeError::Dotool)?;
    if !output.status.success() {
        tracing::warn!(
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "dotool reported a failure"
        );
        return Err(ImeError::DotoolExit(output.status));
    }
    Ok(())
}

pub fn dotool_commands_for_edit(edit: &TranscriptEdit) -> String {
    let backspaces = "key backspace\n".repeat(edit.delete_chars as usize);
    let typed_lines = edit
        .insert
        .split('\n')
        .map(|line| match line {
            "" => String::new(),
            line => format!("type {line}\n"),
        })
        .collect::<Vec<_>>()
        .join("key enter\n");
    backspaces + &typed_lines
}

fn live_dotoold_pipe() -> Option<PathBuf> {
    let path = std::env::var_os("DOTOOL_PIPE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/dotool-pipe"));
    let is_fifo = std::fs::metadata(&path).ok()?.file_type().is_fifo();
    let has_reader = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .is_ok();
    (is_fifo && has_reader).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspaces_precede_the_typed_text() {
        let commands = dotool_commands_for_edit(&TranscriptEdit {
            delete_chars: 2,
            insert: "hi there".into(),
        });
        assert_eq!(commands, "key backspace\nkey backspace\ntype hi there\n");
    }

    #[test]
    fn newlines_become_enter_presses() {
        let commands = dotool_commands_for_edit(&TranscriptEdit::append("one\ntwo"));
        assert_eq!(commands, "type one\nkey enter\ntype two\n");
    }

    #[test]
    fn empty_edit_sends_nothing() {
        assert!(dotool_commands_for_edit(&TranscriptEdit::default()).is_empty());
    }
}
