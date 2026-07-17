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
    GOVERNED_REPOS, RealCiIssueResolver, RealGhRunDiagnostics, RealGhWorkflowClient,
    file_issues_for_report, render_human, report_to_json, resolve_issues_for_report, sweep_fixture,
    sweep_live_with_options,
};
use crate::stewardship::{RealGhClient, StewardshipOutcome};

pub(super) const CI_HEALTH_HELP: &str = "\
Simard ci-health subcommand

Usage: simard ci-health [--json] [--no-cache] [--file-issues] [--exit-zero] [--from-json <path>]
       simard ci-health --list-repos [--json]

  --list-repos         Print the governed fleet this sweep covers (Simard + its
                       governed sibling repos, by `owner/repo`) and exit 0
                       without touching the network. With --json, emit
                       `{\"count\": N, \"repos\": [...]}`; otherwise one slug per
                       line. This is the auditable answer to \"which repos does
                       `across all governed repos` actually mean?\" — the same
                       list the live sweep iterates, kept in lock-step with the
                       ecosystem table in prompt_assets/simard/engineer_system.md
                       by a drift-guard test. Ignores the sweep flags below.
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
  --exit-zero          Exit 0 even when the fleet has actionable failures, as
                       long as the sweep (and any --file-issues stewardship)
                       itself completed without an operational error. This is
                       for the recurring, unattended scheduled sweep
                       (.github/workflows/ci-health.yml): there the *alarm* for
                       a broken fleet is the deduplicated tracking issue filed
                       by --file-issues, not a red workflow run — and letting
                       the scheduled run go red on a sibling's failure would
                       make Simard's own ci-health run a fresh actionable
                       failure the next sweep re-detects (a self-referential
                       loop). A genuine `gh`/parse error still propagates as a
                       non-zero exit; only the fleet-not-green verdict is
                       suppressed.
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
exists (unless --exit-zero, which suppresses that non-zero verdict for the
unattended scheduled sweep — see above).
";

/// Parsed `ci-health` flags.
struct Flags {
    json: bool,
    no_cache: bool,
    file_issues: bool,
    exit_zero: bool,
    list_repos: bool,
    from_json: Option<String>,
}

fn parse_flags(args: impl Iterator<Item = String>) -> Result<Flags, Box<dyn std::error::Error>> {
    let mut json = false;
    let mut no_cache = false;
    let mut file_issues = false;
    let mut exit_zero = false;
    let mut list_repos = false;
    let mut from_json = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--no-cache" | "--refresh" => no_cache = true,
            "--file-issues" => file_issues = true,
            "--exit-zero" => exit_zero = true,
            "--list-repos" => list_repos = true,
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
        exit_zero,
        list_repos,
        from_json,
    })
}

/// Map a completed sweep's fleet verdict to the process exit result.
///
/// The default contract is fail-loud: a fleet with any actionable failure is an
/// `Err` (non-zero exit), matching the `self-health` convention so a human or a
/// PR gate sees the failure. `exit_zero` overrides only *that* verdict — it
/// returns `Ok(())` for a red fleet — and is meant solely for the unattended
/// scheduled sweep, where the alarm is the filed tracking issue (`--file-issues`)
/// and a red run would itself become an actionable failure the next sweep
/// re-detects. Operational errors (a failed `gh`/parse) are surfaced by the
/// caller *before* this decision, so `exit_zero` never masks a broken sweep —
/// only a truthfully-reported red fleet.
fn exit_result(
    green: bool,
    actionable_failures: usize,
    exit_zero: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if green || exit_zero {
        Ok(())
    } else {
        Err(format!(
            "ci-health: {actionable_failures} actionable failure(s) across the governed fleet"
        )
        .into())
    }
}

/// Render the governed fleet for `--list-repos`. Deterministic and pure (no I/O,
/// no network) so it is exhaustively testable. `json` selects the machine shape
/// `{"count": N, "repos": [...]}`; otherwise one `owner/repo` slug per line. Both
/// forms end in a newline and preserve `GOVERNED_REPOS` order (Simard first).
fn render_governed_repos(repos: &[&str], json: bool) -> String {
    if json {
        // Build via serde so escaping/shape are correct by construction rather
        // than by hand (serde_json is already a core dependency here). Append a
        // trailing newline so both output forms are newline-terminated.
        let value = serde_json::json!({ "count": repos.len(), "repos": repos });
        let mut out = serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| "{\"count\":0,\"repos\":[]}".to_string());
        out.push('\n');
        out
    } else {
        let mut out = String::new();
        for repo in repos {
            out.push_str(repo);
            out.push('\n');
        }
        out
    }
}

/// Dispatch `simard ci-health`. Returns `Ok(())` when the fleet is green and an
/// `Err` (non-zero exit) when any actionable failure exists, matching the
/// `self-health` convention. With `--exit-zero`, a red fleet still returns
/// `Ok(())` (the tracking issue is the alarm); operational errors always
/// propagate. See [`exit_result`].
pub(super) fn dispatch_ci_health_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let flags = parse_flags(args)?;

    // `--list-repos` is a standalone informational mode: print the governed
    // fleet the sweep covers and exit 0 without any network call. It composes
    // only with `--json` (output shape) and deliberately ignores the sweep
    // flags, so a stray `--file-issues` etc. can never turn a "which repos?"
    // query into a live, issue-filing sweep.
    if flags.list_repos {
        print!("{}", render_governed_repos(GOVERNED_REPOS, flags.json));
        return Ok(());
    }

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

    if !report.green && flags.exit_zero {
        eprintln!(
            "ci-health: {} actionable failure(s) across the governed fleet; \
             exit suppressed by --exit-zero (the tracking issue is the alarm).",
            report.actionable_failures.len()
        );
    }
    exit_result(
        report.green,
        report.actionable_failures.len(),
        flags.exit_zero,
    )
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

#[cfg(test)]
mod tests {
    use super::{Flags, exit_result, parse_flags, render_governed_repos};

    fn parse(args: &[&str]) -> Result<Flags, String> {
        parse_flags(args.iter().map(|s| s.to_string())).map_err(|e| e.to_string())
    }

    #[test]
    fn exit_zero_flag_parses_and_defaults_false() {
        let default = parse(&[]).expect("empty args parse");
        assert!(!default.exit_zero, "--exit-zero defaults off");

        let set = parse(&["--exit-zero"]).expect("--exit-zero parses");
        assert!(set.exit_zero);
        // Orthogonal to the other flags — none are implied.
        assert!(
            !set.json
                && !set.no_cache
                && !set.file_issues
                && !set.list_repos
                && set.from_json.is_none()
        );
    }

    #[test]
    fn list_repos_flag_parses_and_defaults_false() {
        let default = parse(&[]).expect("empty args parse");
        assert!(!default.list_repos, "--list-repos defaults off");

        let set = parse(&["--list-repos"]).expect("--list-repos parses");
        assert!(set.list_repos);
        // Standalone informational mode — implies none of the sweep flags.
        assert!(!set.json && !set.no_cache && !set.file_issues && !set.exit_zero);

        let with_json = parse(&["--list-repos", "--json"]).expect("composes with --json");
        assert!(with_json.list_repos && with_json.json);
    }

    #[test]
    fn render_governed_repos_human_is_one_slug_per_line() {
        let out = render_governed_repos(&["rysweet/Simard", "rysweet/azlin"], false);
        assert_eq!(out, "rysweet/Simard\nrysweet/azlin\n");
    }

    #[test]
    fn render_governed_repos_json_carries_count_and_ordered_repos() {
        let out = render_governed_repos(&["rysweet/Simard", "rysweet/azlin"], true);
        // Parse it back to prove it is valid JSON with the promised shape/order.
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["count"], 2);
        assert_eq!(v["repos"][0], "rysweet/Simard");
        assert_eq!(v["repos"][1], "rysweet/azlin");
        assert!(out.ends_with("\n"), "output ends in a newline");
    }

    #[test]
    fn render_governed_repos_json_handles_empty_fleet() {
        let out = render_governed_repos(&[], true);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["count"], 0);
        assert!(v["repos"].as_array().expect("repos is array").is_empty());
    }

    #[test]
    fn exit_zero_composes_with_file_issues_and_no_cache() {
        let f = parse(&["--no-cache", "--file-issues", "--exit-zero"]).expect("compose parses");
        assert!(f.no_cache && f.file_issues && f.exit_zero);
    }

    #[test]
    fn unknown_flag_still_rejected_alongside_exit_zero() {
        let err = match parse(&["--exit-zero", "--bogus"]) {
            Ok(_) => panic!("unknown flag must be rejected"),
            Err(e) => e,
        };
        assert!(
            err.contains("--bogus"),
            "error names the offending flag: {err}"
        );
    }

    #[test]
    fn green_fleet_exits_zero_regardless_of_exit_zero() {
        assert!(exit_result(true, 0, false).is_ok());
        assert!(exit_result(true, 0, true).is_ok());
    }

    #[test]
    fn red_fleet_errors_by_default_but_exit_zero_suppresses_it() {
        // Default fail-loud: a red fleet is a non-zero exit whose message counts
        // the failures (so a human/PR gate sees it).
        let err = exit_result(false, 3, false).expect_err("red fleet errors by default");
        assert!(
            err.to_string().contains("3 actionable failure"),
            "error reports the failure count: {err}"
        );
        // --exit-zero suppresses only that verdict — the scheduled sweep stays
        // green so its own run never becomes a self-referential failure.
        assert!(
            exit_result(false, 3, true).is_ok(),
            "--exit-zero makes a red fleet exit 0"
        );
    }
}
