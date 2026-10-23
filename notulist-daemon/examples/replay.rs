use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use futures_util::StreamExt;
use notulist_client::{
    connect_dictation_proxy, DictationPhase, TranscriptAccumulator, TranscriptEdit,
};
use notulist_daemon::audio::AudioSource;
use notulist_daemon::config::DaemonConfig;
use notulist_daemon::dictation_daemon::DictationDaemon;
use notulist_daemon::error::DaemonError;

#[derive(Parser)]
#[command(
    about = "Replay a WAV file through the whole dictation pipeline in real time and print a timeline"
)]
struct ReplayArgs {
    #[arg(help = "WAV file to replay as the microphone input")]
    wav: PathBuf,
    #[arg(
        long,
        default_value_t = 1.0,
        help = "Playback speed relative to real time"
    )]
    speed: f32,
    #[arg(
        long,
        env = "NOTULIST_CONFIG",
        help = "Config file to load instead of the default"
    )]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    notulist_daemon::terminal_logging::install_terminal_logging(0);
    match replay(ReplayArgs::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn replay(args: ReplayArgs) -> Result<(), DaemonError> {
    let config = DaemonConfig::load(args.config.as_deref())?;
    let source = AudioSource::WavFile {
        path: args.wav,
        playback_speed: args.speed,
    };
    let daemon = DictationDaemon::bind(config, source).await?;
    tokio::select! {
        _ = daemon.run() => Ok(()),
        result = print_timeline() => result,
    }
}

fn elapsed_label(session_clock: Instant) -> String {
    format!("{:>7.2}s", session_clock.elapsed().as_secs_f32())
}

async fn print_timeline() -> Result<(), DaemonError> {
    let proxy = connect_dictation_proxy().await?;
    let mut phase_changes = proxy.receive_phase_changed().await?;
    let mut decoding = proxy.receive_decoding_changed().await?;
    let mut edits = proxy.receive_transcript_edited().await?;
    let mut failures = proxy.receive_session_failed().await?;
    let mut endings = proxy.receive_session_ended().await?;

    let mut session_clock = Instant::now();
    let mut transcript = TranscriptAccumulator::default();
    let mut started = false;

    let initial_phase = proxy.current_phase().await?;
    println!(
        "{} phase  {}",
        elapsed_label(session_clock),
        initial_phase.label()
    );
    if initial_phase == DictationPhase::Idle {
        session_clock = Instant::now();
        proxy.start().await?;
        started = true;
    }

    loop {
        tokio::select! {
            Some(change) = phase_changes.next() => {
                let phase = change.args()?.phase;
                println!("{} phase  {}", elapsed_label(session_clock), phase.label());
                if !started && phase == DictationPhase::Idle {
                    session_clock = Instant::now();
                    proxy.start().await?;
                    started = true;
                }
            }
            Some(change) = decoding.next() => {
                let active = change.args()?.active;
                let state = if active { "start" } else { "stop" };
                println!("{} decode {state}", elapsed_label(session_clock));
            }
            Some(change) = edits.next() => {
                let edit: TranscriptEdit = change.args()?.edit;
                let kind = if edit.delete_chars > 0 { "CORRECT" } else { "append " };
                println!(
                    "{} edit   {kind} -{} {:?}",
                    elapsed_label(session_clock),
                    edit.delete_chars,
                    edit.insert
                );
                transcript.apply(&edit);
            }
            Some(change) = failures.next() => {
                let failure = change.args()?.failure;
                println!("{} failed {failure:?}", elapsed_label(session_clock));
            }
            Some(change) = endings.next() => {
                let outcome = change.args()?.outcome;
                println!("{} ended  {outcome:?}", elapsed_label(session_clock));
                println!("\n{}", transcript.text());
                return Ok(());
            }
            else => return Ok(()),
        }
    }
}
