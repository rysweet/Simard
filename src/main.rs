use simard::dispatch_operator_cli;

fn main() -> std::process::ExitCode {
    // Initialize structured tracing. RUST_LOG controls verbosity
    // (default: info). Set SIMARD_LOG_JSON=1 for JSON output.
    let use_json = std::env::var("SIMARD_LOG_JSON")
        .map(|v| matches!(v.as_str(), "1" | "true"))
        .unwrap_or(false);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    }

    match dispatch_operator_cli(std::env::args().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "command failed");
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
