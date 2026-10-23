use std::path::PathBuf;

use clap::Parser;
use notulist_ime::dictation_bus_client::run_dictation_bus_client;
use notulist_ime::error::ImeError;
use notulist_ime::ime_config::ImeConfig;
use notulist_ime::input_method::wayland_input_method_connection::InputMethodConnection;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "notulist-ime",
    version,
    about = "Wayland input method that types notulist dictation into the focused field"
)]
struct ImeCli {
    #[arg(long, env = "NOTULIST_CONFIG")]
    config: Option<PathBuf>,

    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> Result<(), ImeError> {
    let cli = ImeCli::parse();
    let level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(format!("notulist_ime={level},warn"))),
        )
        .with_target(false)
        .init();

    let config = ImeConfig::load(cli.config)?;
    let (mic_clicks_tx, mic_clicks_rx) = tokio::sync::mpsc::unbounded_channel();
    let connection = InputMethodConnection::spawn(config.popup.clone(), mic_clicks_tx);
    run_dictation_bus_client(connection, mic_clicks_rx, config).await
}
