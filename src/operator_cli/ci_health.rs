//! `simard ci-health` — run the governed-fleet CI-health sweep and print a
//! report. Exit code 0 when the fleet is green (zero actionable failures),
//! non-zero when an active workflow's latest default-branch run failed.
//!
//! By default the sweep reads live GitHub state via `gh`. `--from-json <path>`
//! classifies an offline snapshot fixture instead (used by tests and for
//! reproducing a captured sweep).
//!
//! See `docs/reference/ci-health-sweep.md`.

use crate::ci_health::{
    RealGhWorkflowClient, render_human, report_to_json, sweep_fixture, sweep_live,
};

pub(super) const CI_HEALTH_HELP: &str = "\
Simard ci-health subcommand

Usage: simard ci-health [--json] [--from-json <path>]

  --json               Emit the FleetReport as JSON (default: human table).
  --from-json <path>   Classify an offline snapshot fixture instead of calling
                       `gh` (the fixture shape mirrors the live snapshot).

Sweeps every active default-branch workflow across the amplihack ecosystem
(Simard + governed sibling repos). A workflow is reported as an actionable
failure only when it is ENABLED and its latest default-branch run concluded
failure / timed_out / startup_failure. Disabled workflows and non-failure
conclusions (cancelled, skipped, neutral, action_required, stale) are ignored
with a reason.

Exit code: 0 when the fleet is green; non-zero when any actionable failure
exists.
";

/// Parsed `ci-health` flags.
struct Flags {
    json: bool,
    from_json: Option<String>,
}

fn parse_flags(args: impl Iterator<Item = String>) -> Result<Flags, Box<dyn std::error::Error>> {
    let mut json = false;
    let mut from_json = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--from-json" => {
                let path = args
                    .next()
                    .ok_or_else(|| "flag `--from-json` requires a path argument".to_string())?;
                from_json = Some(path);
            }
            other => {
                if let Some(path) = other.strip_prefix("--from-json=") {
                    from_json = Some(path.to_string());
                } else {
                    return Err(format!(
                        "unexpected argument '{other}' (see `simard ci-health --help`)"
                    )
                    .into());
                }
            }
        }
    }
    Ok(Flags { json, from_json })
}

/// Dispatch `simard ci-health`. Returns `Ok(())` when the fleet is green and an
/// `Err` (non-zero exit) when any actionable failure exists, matching the
/// `self-health` convention.
pub(super) fn dispatch_ci_health_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let flags = parse_flags(args)?;

    let report = match &flags.from_json {
        Some(path) => {
            let bytes =
                std::fs::read(path).map_err(|e| format!("failed to read fixture '{path}': {e}"))?;
            sweep_fixture(&bytes)?
        }
        None => sweep_live(&RealGhWorkflowClient::new())?,
    };

    if flags.json {
        println!("{}", report_to_json(&report)?);
    } else {
        print!("{}", render_human(&report));
    }

    if report.green {
        Ok(())
    } else {
        Err(format!(
            "ci-health: {} actionable failure(s) across the governed fleet",
            report.actionable_failures.len()
        )
        .into())
    }
}
