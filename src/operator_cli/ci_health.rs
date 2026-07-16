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
    RealCiIssueResolver, RealGhRunDiagnostics, RealGhWorkflowClient, file_issues_for_report,
    render_human, report_to_json, resolve_issues_for_report, sweep_fixture,
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
                       stewardship-signature). New issues embed a root-cause
                       block naming the failing job(s)/step(s). Conversely, any
                       open tracking issue whose workflow is GREEN again this
                       sweep is closed with a green-evidence comment, keeping
                       one open issue per *still-broken* workflow. Read-only by
                       default; this flag opts in to the writes. Rejected with
                       --from-json.
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
broken workflow is never re-filed. Each newly-filed issue embeds a root-cause
block pinpointing which job(s) and step(s) of the failing run failed (read from
`gh run view --json jobs`) so a fixer needn't hunt through the run to find the
failure, and links the run for the failing logs; a diagnosis that cannot be
fetched is recorded as unavailable rather than omitted. The same pass also
*resolves* the other direction: any open tracking issue whose workflow's latest
default-branch run is now green is closed with a green-evidence comment, so the
fleet keeps exactly one open issue per still-broken workflow and none for
already-recovered ones.

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
        steward_issues(&report)?;
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

/// Reconcile the fleet's tracking issues with the sweep: file a deduplicated
/// tracking issue for each distinct actionable failure, and close any open
/// tracking issue whose workflow is green again. Outcomes print to stderr (so
/// `--json` stdout stays pure report JSON).
///
/// Filing runs first (the correctness-critical path — a genuinely-broken
/// workflow must get a tracking issue), then resolution, so a resolution error
/// can never starve filing. Resolution runs even when the fleet is green — a
/// green fleet can still carry stale tracking issues from a since-recovered
/// failure. Any `gh` failure propagates so the caller sees a non-zero exit
/// (never a silent partial reconciliation).
fn steward_issues(
    report: &crate::ci_health::FleetReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if report.green {
        eprintln!("ci-health: fleet green; no actionable failures to file.");
    } else {
        let outcomes =
            file_issues_for_report(report, &RealGhClient::new(), &RealGhRunDiagnostics::new())?;
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
    }

    // Resolution runs after filing so a resolution `gh` error can never starve
    // the correctness-critical filing path above.
    let resolved = resolve_issues_for_report(report, &RealCiIssueResolver::new())?;
    if resolved.is_empty() {
        eprintln!("ci-health: no recovered workflows with an open tracking issue to close.");
    } else {
        eprintln!(
            "ci-health: closed {} tracking issue(s) for now-green workflow(s):",
            resolved.len()
        );
        for outcome in &resolved {
            eprintln!(
                "  closed  {repo}#{num} [{workflow}] {url}",
                repo = outcome.repo,
                num = outcome.issue_number,
                workflow = outcome.workflow,
                url = outcome.url,
            );
        }
    }
    Ok(())
}
