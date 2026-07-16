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
    RealGhWorkflowClient, file_issues_for_report, render_human, report_to_json, sweep_fixture,
    sweep_live_with_options,
};
use crate::stewardship::{RealGhClient, StewardshipOutcome};

pub(super) const CI_HEALTH_HELP: &str = "\
Simard ci-health subcommand

Usage: simard ci-health [--json] [--no-cache] [--file-issues] [--from-json <path>]

  --json               Emit the FleetReport as JSON (default: human table).
  --no-cache           Force a full re-collection of every repo, ignoring the
                       last-known-green head-SHA cache (the cache is still
                       refreshed from this sweep). Alias: --refresh.
  --file-issues        For each distinct actionable failure, file a deduplicated
                       tracking issue in the failing repo (dedupes against any
                       open issue already carrying the same
                       stewardship-signature). Read-only by default; this flag
                       opts in to the write. Rejected with --from-json.
  --from-json <path>   Classify an offline snapshot fixture instead of calling
                       `gh` (the fixture shape mirrors the live snapshot).

Sweeps every active default-branch workflow across the amplihack ecosystem
(Simard + governed sibling repos). A workflow is reported as an actionable
failure only when it is ENABLED and its latest default-branch run concluded
failure / timed_out / startup_failure. Disabled workflows and non-failure
conclusions (cancelled, skipped, neutral, action_required, stale) are ignored
with a reason.

To avoid re-auditing an already-green fleet every cycle, each repo's
last-known-green default-branch head commit SHA is cached; a repo whose head SHA
is unchanged (and whose CI is commit-driven) is served from cache as green
instead of being re-collected. Use --no-cache to force a full sweep.

With --file-issues, each distinct actionable failure (one per broken
repo+workflow) is converted into a deduplicated GitHub issue in the failing
repo, reusing the stewardship-signature dedup contract so an already-tracked
broken workflow is never re-filed.

Exit code: 0 when the fleet is green; non-zero when any actionable failure
exists.
";

/// Parsed `ci-health` flags.
struct Flags {
    json: bool,
    no_cache: bool,
    file_issues: bool,
    from_json: Option<String>,
}

fn parse_flags(args: impl Iterator<Item = String>) -> Result<Flags, Box<dyn std::error::Error>> {
    let mut json = false;
    let mut no_cache = false;
    let mut file_issues = false;
    let mut from_json = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--no-cache" | "--refresh" => no_cache = true,
            "--file-issues" => file_issues = true,
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
    if file_issues && from_json.is_some() {
        return Err(
            "flag `--file-issues` cannot be combined with `--from-json`: filing issues \
                    requires a live sweep, not an offline fixture"
                .into(),
        );
    }
    Ok(Flags {
        json,
        no_cache,
        file_issues,
        from_json,
    })
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
        None => sweep_live_with_options(&RealGhWorkflowClient::new(), !flags.no_cache)?,
    };

    if flags.json {
        println!("{}", report_to_json(&report)?);
    } else {
        print!("{}", render_human(&report));
    }

    if flags.file_issues {
        file_actionable_issues(&report)?;
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

/// File a deduplicated tracking issue for each distinct actionable failure and
/// print the outcomes to stderr (so `--json` stdout stays pure report JSON). A
/// green fleet is a no-op. Any `gh` failure propagates so the caller sees a
/// non-zero exit (never a silent partial filing).
fn file_actionable_issues(
    report: &crate::ci_health::FleetReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if report.green {
        eprintln!("ci-health: fleet green; no actionable failures to file.");
        return Ok(());
    }
    let outcomes = file_issues_for_report(report, &RealGhClient::new())?;
    eprintln!(
        "ci-health: filed/matched {} deduplicated tracking issue(s):",
        outcomes.len()
    );
    for outcome in &outcomes {
        match outcome {
            StewardshipOutcome::FiledNew {
                repo,
                issue_number,
                url,
                signature,
            } => eprintln!("  filed   {repo}#{issue_number} [{signature}] {url}"),
            StewardshipOutcome::MatchedExisting {
                repo,
                issue_number,
                url,
                signature,
            } => eprintln!("  matched {repo}#{issue_number} [{signature}] {url}"),
        }
    }
    Ok(())
}
