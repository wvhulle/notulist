use tracing_subscriber::EnvFilter;

const LOGGED_CRATES: [&str; 3] = ["notulist", "notulist_daemon", "notulist_client"];

pub fn install_terminal_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let crate_directives = LOGGED_CRATES
        .iter()
        .map(|crate_name| format!(",{crate_name}={level}"))
        .collect::<String>();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("warn{crate_directives}")));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
