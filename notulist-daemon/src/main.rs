use std::path::PathBuf;

use clap::{Parser, Subcommand};
use notulist_daemon::audio::AudioSource;
use notulist_daemon::config::DaemonConfig;
use notulist_daemon::dictation_cli::{self, RecordAction};
use notulist_daemon::dictation_daemon::DictationDaemon;
use notulist_daemon::error::DaemonError;
use notulist_daemon::terminal_logging::install_terminal_logging;
use notulist_daemon::wav_file_transcription::transcribe_wav_file;

#[derive(Parser)]
#[command(
    name = "notulist",
    version,
    about = "Streaming whisper dictation over D-Bus"
)]
struct NotulistCli {
    #[arg(long, global = true, env = "NOTULIST_CONFIG")]
    config: Option<PathBuf>,

    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: NotulistCommand,
}

#[derive(Subcommand)]
enum NotulistCommand {
    #[command(about = "Run the dictation daemon on the session bus")]
    Daemon,
    #[command(about = "Control a dictation session in the running daemon")]
    Record {
        #[command(subcommand)]
        action: RecordAction,
        #[arg(
            long,
            global = true,
            help = "Block until the session ends and print its transcript to stdout"
        )]
        wait: bool,
    },
    #[command(about = "Print the current dictation phase")]
    Status {
        #[arg(long, help = "Keep printing one line per phase change")]
        follow: bool,
    },
    #[command(about = "Transcribe a WAV file with the configured whisper model")]
    Transcribe { file: PathBuf },
}

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    let cli = NotulistCli::parse();
    install_terminal_logging(cli.verbose);
    match cli.command {
        NotulistCommand::Daemon => run_daemon(DaemonConfig::load(cli.config.as_deref())?).await,
        NotulistCommand::Record { action, wait } => dictation_cli::record(action, wait).await,
        NotulistCommand::Status { follow } => dictation_cli::status(follow).await,
        NotulistCommand::Transcribe { file } => {
            let config = DaemonConfig::load(cli.config.as_deref())?;
            println!("{}", transcribe_wav_file(&config, &file)?);
            Ok(())
        }
    }
}

async fn run_daemon(config: DaemonConfig) -> Result<(), DaemonError> {
    let source = AudioSource::Microphone(config.microphone.clone());
    let daemon = DictationDaemon::bind(config, source).await?;
    tokio::select! {
        _ = daemon.run() => {}
        _ = shutdown_requested() => tracing::info!("Shutting down"),
    }
    Ok(())
}

async fn shutdown_requested() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler installs");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
