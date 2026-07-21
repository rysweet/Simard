//! `simard probe <target>` — lightweight liveness probes for operators and the
//! self-deploy canary.
//!
//! Currently one target: `rpc`, the RPC / cognitive-memory health check the
//! self-deploy canary's `rpc-health` gate shells out to
//! (`<candidate> probe rpc --timeout <n>`). It opens the canonical reader
//! client — the daemon socket when one is up, else a direct on-disk open — and
//! confirms the endpoint answers a statistics round-trip.
//!
//! Exit code: 0 when the RPC / memory endpoint is healthy; non-zero otherwise.
//!
//! A genuinely *absent* daemon is NOT an error here: [`open_reader_client`]
//! legitimately falls back to a direct store open, so an isolated fresh-build
//! canary (no running daemon) still reports healthy. That is what lets a
//! healthy candidate pass the `rpc-health` gate instead of reddening the canary
//! on a subcommand that used to not exist. A daemon socket that is *present but
//! unconnectable* still fails closed (bug #2896), so a genuinely broken RPC
//! endpoint is never mistaken for healthy.

use crate::memory_ipc::open_reader_client;

pub(super) const PROBE_HELP: &str = "\
Simard probe subcommand — lightweight liveness probes

Usage:
  simard probe rpc [--timeout <seconds>]

Targets:
  rpc    Check the RPC / cognitive-memory endpoint is alive and answers a
         statistics round-trip. Opens the canonical reader client (the daemon
         socket when up, else a direct on-disk open, fail-closed on a present-
         but-unconnectable socket).

Options:
  --timeout <seconds>   Accepted for compatibility with the self-deploy canary
                        gate invocation; the in-process check returns promptly.

Exit code: 0 when the endpoint is healthy; non-zero otherwise.
";

/// Dispatch `simard probe <target>`.
pub(super) fn dispatch_probe_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(target) = args.next() else {
        return Err("probe: missing target (see `simard probe rpc --help`)".into());
    };
    if matches!(target.as_str(), "--help" | "-h" | "help") {
        print!("{PROBE_HELP}");
        return Ok(());
    }
    match target.as_str() {
        "rpc" => probe_rpc(args),
        other => Err(format!("unsupported probe target '{other}' (expected 'rpc')").into()),
    }
}

/// `simard probe rpc` — open the canonical reader client and confirm the RPC /
/// cognitive-memory endpoint answers a statistics round-trip.
fn probe_rpc(args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    // Accept (and otherwise ignore) `--timeout <n>` / `--timeout=<n>` for
    // compatibility with the canary gate's `probe rpc --timeout <n>` call. The
    // in-process reader open and statistics round-trip return promptly, so no
    // timer is armed — but the value is still validated so a malformed timeout
    // is a clear error rather than silently swallowed.
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" | "help" => {
                print!("{PROBE_HELP}");
                return Ok(());
            }
            "--timeout" => {
                let value = iter
                    .next()
                    .ok_or("`--timeout` requires a value (seconds)")?;
                validate_timeout(&value)?;
            }
            other if other.starts_with("--timeout=") => {
                validate_timeout(&other["--timeout=".len()..])?;
            }
            other => {
                return Err(format!(
                    "unexpected argument '{other}' (see `simard probe rpc --help`)"
                )
                .into());
            }
        }
    }

    let state_root = crate::state_root::simard_state_root();
    // Opening the reader exercises the RPC / socket connect (fail-closed on a
    // present-but-unconnectable daemon socket, bug #2896); a genuinely absent
    // daemon legitimately falls back to a direct on-disk open.
    let reader = open_reader_client(&state_root)?;
    // A statistics round-trip confirms the endpoint actually answers rather
    // than merely constructing a client handle.
    let stats = reader.ops().get_statistics()?;
    tracing::info!(
        target: "simard::probe",
        facts = stats.total(),
        "rpc health check passed"
    );
    Ok(())
}

/// Validate a `--timeout` value as a non-negative integer number of seconds.
fn validate_timeout(value: &str) -> Result<(), Box<dyn std::error::Error>> {
    value
        .parse::<u64>()
        .map(|_| ())
        .map_err(|e| format!("invalid --timeout '{value}': {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::HermeticState;
    use serial_test::serial;

    fn dispatch(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        dispatch_probe_command(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn missing_target_is_error() {
        assert!(dispatch(&[]).is_err());
    }

    #[test]
    fn help_target_prints_and_succeeds() {
        assert!(dispatch(&["--help"]).is_ok());
        assert!(dispatch(&["rpc", "--help"]).is_ok());
    }

    #[test]
    fn unknown_target_is_error() {
        let err = dispatch(&["nonsense"]).unwrap_err();
        assert!(
            err.to_string().contains("unsupported probe target"),
            "{err}"
        );
    }

    #[test]
    fn rpc_rejects_unexpected_argument() {
        let err = dispatch(&["rpc", "--bogus"]).unwrap_err();
        assert!(err.to_string().contains("unexpected argument"), "{err}");
    }

    #[test]
    fn rpc_rejects_malformed_timeout() {
        assert!(dispatch(&["rpc", "--timeout", "soon"]).is_err());
        assert!(dispatch(&["rpc", "--timeout=nope"]).is_err());
        assert!(dispatch(&["rpc", "--timeout"]).is_err());
    }

    #[test]
    #[serial(cognitive_memory)]
    fn rpc_health_check_passes_against_a_live_store() {
        // A hermetic state root with no running daemon: `open_reader_client`
        // falls back to a direct on-disk open (endpoint legitimately absent),
        // the statistics round-trip answers, and the probe exits healthy — the
        // exact path that lets an isolated fresh-build canary pass the gate.
        let _state = HermeticState::new();
        assert!(
            dispatch(&["rpc", "--timeout", "30"]).is_ok(),
            "probe rpc must report healthy against a fresh on-disk store"
        );
        // `--timeout=<n>` form and the bare (no-timeout) form work too.
        assert!(dispatch(&["rpc", "--timeout=5"]).is_ok());
        assert!(dispatch(&["rpc"]).is_ok());
    }
}
