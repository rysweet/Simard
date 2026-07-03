//! The `simard signal` operator subcommand — launch the Signal conversation
//! channel (issue #2527).
//!
//! This is the operator-facing entrypoint that actually wires the feature-gated
//! [`crate::signal_conversation`] external-service integration into the running
//! binary: `simard signal run` loads the `[signal]` config, connects to the
//! locally-run signal-cli JSON-RPC daemon, and drives the operator↔Simard
//! meeting conversation (with the sender allowlist + high-risk sign-off gating)
//! until the socket closes.
//!
//! The subcommand is **always recognized** so the operator gets a clear message
//! either way; the Signal implementation itself compiles only under the `signal`
//! Cargo feature (default off). A default build recognizes `simard signal` and
//! tells the operator to rebuild with `--features signal` instead of failing
//! with a bare "unsupported command".
//!
//! # Naming
//!
//! Nothing here is named `adapter`/`Adapter`. This dispatches the first-class
//! Signal conversation channel and is unrelated to the cognitive-memory
//! `ServerTransport`.

use super::args::{next_required, reject_extra_args};

pub(super) const SIGNAL_HELP: &str = "\
Simard signal subcommand

Usage: simard signal <command>

Commands:
  run               Connect to the configured signal-cli JSON-RPC daemon and run
                    the operator Signal conversation channel until the socket
                    closes. Requires a build with `--features signal` and a
                    [signal] config table (endpoint, account, allowlist).
  help, -h, --help  Show this help message and exit.

The Signal channel is OPTIONAL and OFF by default; a plain build has no Signal
code compiled in. See docs/howto/set-up-the-signal-channel.md for full setup.
";

pub fn dispatch_signal_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "signal subcommand (run)")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{SIGNAL_HELP}");
            Ok(())
        }
        "run" => {
            reject_extra_args(args)?;
            run_signal_channel()
        }
        other => Err(format!("unsupported command 'signal {other}'").into()),
    }
}

/// Load the `[signal]` config, build a tokio runtime, and drive the Signal
/// conversation channel to completion. Compiled only under the `signal` feature.
#[cfg(feature = "signal")]
fn run_signal_channel() -> Result<(), Box<dyn std::error::Error>> {
    use crate::signal_conversation::{self, SignalConfig};

    let config = SignalConfig::load()?;
    eprintln!(
        "[simard] signal: connecting to signal-cli daemon at {} (account {}); {} allowlisted operator(s)",
        config.endpoint,
        config.account,
        config.allowlist.len()
    );
    if config.allowlist.is_empty() {
        eprintln!(
            "[simard] signal: WARNING — the operator allowlist is empty; the channel is fail-closed and will accept no commands. Set [signal].allowlist (or SIMARD_SIGNAL_ALLOWLIST)."
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(signal_conversation::run(config))?;
    Ok(())
}

/// Feature-off stub: `simard signal run` is recognized but the Signal channel is
/// not compiled in, so report a clear, actionable message rather than failing
/// with "unsupported command".
#[cfg(not(feature = "signal"))]
fn run_signal_channel() -> Result<(), Box<dyn std::error::Error>> {
    Err("the Signal channel is not compiled into this build; rebuild with `cargo build --features signal` and configure the [signal] table (see docs/howto/set-up-the-signal-channel.md)"
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_subcommand_returns_error() {
        let args = Vec::<String>::new().into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected"));
    }

    #[test]
    fn unsupported_subcommand_returns_error() {
        let args = vec!["nope".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command")
        );
    }

    #[test]
    fn help_flag_exits_ok() {
        let args = vec!["--help".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(
            result.is_ok(),
            "signal --help must exit Ok, got: {result:?}"
        );
    }

    #[test]
    fn short_help_flag_exits_ok() {
        let args = vec!["-h".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_ok(), "signal -h must exit Ok, got: {result:?}");
    }

    #[test]
    fn help_word_exits_ok() {
        let args = vec!["help".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_ok(), "signal help must exit Ok, got: {result:?}");
    }

    #[test]
    fn run_rejects_trailing_args() {
        let args = vec!["run".to_string(), "--extra".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected trailing")
        );
    }

    // Without the `signal` feature, `run` must report a clear, actionable
    // message pointing at the feature flag — never silently no-op.
    #[cfg(not(feature = "signal"))]
    #[test]
    fn run_without_feature_points_at_feature_flag() {
        let args = vec!["run".to_string()].into_iter();
        let result = dispatch_signal_command(args);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("--features signal"),
            "message should name the feature flag, got: {msg}"
        );
    }
}
