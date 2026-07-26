//! Operator subcommand `simard merge-pr <PR> [--repo <owner/repo>]` — invokes
//! Simard's gated merge authority against a PR.
//!
//! The merge authority (`stewardship::merge_pr_if_merge_ready`) re-checks the
//! objective gates (base-branch allow-list, `mergeable == MERGEABLE`, every
//! required check green) and the agentic merge-readiness judge before it ever
//! runs `gh pr merge --squash --delete-branch --repo <repo>`. The target repo
//! is a parameter that **defaults to `rysweet/Simard`** for back-compat, so the
//! same gated authority lands cross-repo PRs (e.g. supply-chain hardening PRs in
//! `rysweet/amplihack-rs`, `rysweet/RustyClawd`) instead of a bare
//! `gh pr merge` that skips the gates.

use crate::stewardship::{MergeOutcome, PrGhClient, RealPrGhClient, merge_pr_if_merge_ready};

use crate::state_root::simard_state_root;
use crate::stewardship::merge_verdict_store::{
    MergeVerdictRecord, VerdictKind, validate_repo_slug, write_record,
};
use std::path::PathBuf;

/// Repo Simard ships from — the default target when `--repo` is omitted.
const DEFAULT_REPO: &str = "rysweet/Simard";

pub(super) const MERGE_PR_HELP: &str = "\
Simard merge-pr subcommand

Usage: simard merge-pr <PR-number> [--repo <owner/repo>]

Squash-merges the given PR through Simard's gated merge authority (objective
gates + merge-readiness judge) if it is merge-ready. Defaults to rysweet/Simard;
pass --repo <owner/repo> to land a PR in any other repo Simard governs.
";

/// Parsed `merge-pr` invocation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MergePrArgs {
    pub(crate) pr_number: u32,
    pub(crate) repo: String,
}

/// Result of parsing the `merge-pr` argument list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergePrCommand {
    /// `--help` / `-h` / `help` was requested.
    Help,
    /// A concrete merge request.
    Merge(MergePrArgs),
}

/// Validate an `owner/repo` slug before it reaches the `gh --repo` argument.
/// Delegates to the store's single traversal-safe guard ([`validate_repo_slug`])
/// so `merge-pr` and `record-verdict` enforce the *same* well-formedness rule,
/// while preserving this subcommand's `invalid --repo '<repo>'` error contract.
fn validate_repo(repo: &str) -> Result<(), Box<dyn std::error::Error>> {
    validate_repo_slug(repo)
        .map(|_| ())
        .map_err(|_| format!("invalid --repo '{repo}' (expected <owner/repo>)").into())
}

/// Parse `merge-pr` arguments. Accepts the PR number as the single positional
/// and an optional `--repo <owner/repo>` (or `--repo=<owner/repo>`) flag in any
/// position. Omitting `--repo` defaults to [`DEFAULT_REPO`].
pub(crate) fn parse_merge_pr_args(
    args: impl Iterator<Item = String>,
) -> Result<MergePrCommand, Box<dyn std::error::Error>> {
    let tokens: Vec<String> = args.collect();
    if let Some(first) = tokens.first() {
        if matches!(first.as_str(), "--help" | "-h" | "help") {
            return Ok(MergePrCommand::Help);
        }
    } else {
        return Err("expected PR number".into());
    }

    let mut repo: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut iter = tokens.into_iter();
    while let Some(token) = iter.next() {
        if token == "--repo" {
            let value = iter.next().ok_or("expected <owner/repo> after --repo")?;
            if repo.replace(value).is_some() {
                return Err("--repo specified more than once".into());
            }
        } else if let Some(value) = token.strip_prefix("--repo=") {
            if repo.replace(value.to_string()).is_some() {
                return Err("--repo specified more than once".into());
            }
        } else {
            positionals.push(token);
        }
    }

    let pr_str = match positionals.as_slice() {
        [pr] => pr.clone(),
        [] => return Err("expected PR number".into()),
        [_, extras @ ..] => {
            return Err(format!("unexpected trailing arguments: {}", extras.join(" ")).into());
        }
    };
    let pr_number: u32 = pr_str
        .parse()
        .map_err(|_| format!("invalid PR number '{pr_str}'"))?;

    let repo = repo.unwrap_or_else(|| DEFAULT_REPO.to_string());
    validate_repo(&repo)?;

    Ok(MergePrCommand::Merge(MergePrArgs { pr_number, repo }))
}

/// Run the gated merge for `pr_number` in `repo` using `gh`, printing the
/// outcome. Separated from [`dispatch_merge_pr_command`] so the wiring is
/// exercised without a real `gh` subprocess.
fn run_merge(
    pr_number: u32,
    repo: &str,
    gh: &dyn PrGhClient,
) -> Result<(), Box<dyn std::error::Error>> {
    match merge_pr_if_merge_ready(pr_number, repo, gh)? {
        MergeOutcome::Merged { pr_number, repo } => {
            println!("merged: PR #{pr_number} in {repo} (squash + delete-branch)");
            Ok(())
        }
        MergeOutcome::Refused { pr_number, reason } => {
            // Refusal is *expected output*, not an error — the operator
            // asked us to evaluate. Print to stderr and exit non-zero so
            // shell scripts can detect "blocked" without losing the reason.
            eprintln!("refused: PR #{pr_number} not merge-ready: {reason}");
            Err(format!("merge refused: {reason}").into())
        }
    }
}

pub(crate) fn dispatch_merge_pr_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    match parse_merge_pr_args(args)? {
        MergePrCommand::Help => {
            print!("{MERGE_PR_HELP}");
            Ok(())
        }
        MergePrCommand::Merge(MergePrArgs { pr_number, repo }) => {
            let gh = RealPrGhClient::new();
            run_merge(pr_number, &repo, &gh)
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// `simard merge record-verdict` — the agent-facing gated WRITE tool (#4721)
// ───────────────────────────────────────────────────────────────────────────
//
// The merge-readiness recipe reasons over a PR and RECORDS its typed verdict by
// calling this tool (act-via-tool, like `simard memory remember`), printing no
// JSON envelope for the daemon to scrape. The thin deterministic rail
// (`stewardship::recipe_merge_judge`) reads the freshness-checked record and
// INDEPENDENTLY re-verifies the hard safety gates before authorizing any merge.
// All validation lives in this tool.

/// The verdict cleared validation and was durably recorded.
const EXIT_RECORDED: i32 = 0;
/// A required flag was missing or malformed (bad verdict word, non-numeric
/// `--pr`, empty `--reason`, malformed `--repo`, …).
const EXIT_USAGE: i32 = 2;
/// The record could not be written (state-root / IO / serialize error).
const EXIT_IO: i32 = 3;

pub(super) const MERGE_HELP: &str = "\
Simard merge subcommand

Usage:
  simard merge record-verdict --pr <N> --repo <owner/name> \\
        --verdict merge|hold --reason \"<text>\" --run-token <token> \\
        [--state-root <path>]

record-verdict durably records the merge-readiness judge's typed verdict so the
deterministic rail can read it back and INDEPENDENTLY re-verify the hard safety
gates (mergeable, not draft, CI green, allow-listed base) before authorizing a
squash-merge. It NEVER merges and NEVER weakens a gate. The verdict is advisory:
a `merge` against a red/draft/non-mergeable PR is refused by the rail.

Both `--flag value` and `--flag=value` forms are accepted.

Exit codes:
  0  recorded       the verdict cleared validation and was written
  2  usage error    a required flag was missing or malformed
  3  io error       the record could not be written (state-root/IO)
";

/// Parsed `simard merge record-verdict` invocation. Scalar flags only — a large
/// rationale would ride a file, never argv (the acceptance criteria keep
/// `--reason` bounded).
#[derive(Debug)]
pub(crate) struct RecordVerdictArgs {
    pub pr: u32,
    pub repo: String,
    pub verdict: VerdictKind,
    pub reason: String,
    pub run_token: String,
    pub state_root: Option<PathBuf>,
}

/// Take a flag's value: inline (`--flag=value`) if present, else the next argv
/// token (`--flag value`). Errors if neither is available.
fn record_flag_value(
    flag: &str,
    inline: Option<String>,
    next: &mut dyn Iterator<Item = String>,
) -> Result<String, String> {
    match inline {
        Some(v) => Ok(v),
        None => next
            .next()
            .ok_or_else(|| format!("--{flag} requires a value")),
    }
}

/// Parse + validate `record-verdict` argv into typed [`RecordVerdictArgs`]. All
/// enforcement lives here: unknown/missing flags, an invalid verdict word, a
/// non-numeric `--pr`, an empty `--reason`, and a malformed `--repo` all fail.
pub(crate) fn parse_record_verdict_args(args: Vec<String>) -> Result<RecordVerdictArgs, String> {
    let mut pr: Option<String> = None;
    let mut repo: Option<String> = None;
    let mut verdict: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut run_token: Option<String> = None;
    let mut state_root: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let Some(rest) = arg.strip_prefix("--") else {
            return Err(format!("unexpected positional argument {arg:?}"));
        };
        let (key, inline) = match rest.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (rest.to_string(), None),
        };
        match key.as_str() {
            "pr" => pr = Some(record_flag_value("pr", inline, &mut iter)?),
            "repo" => repo = Some(record_flag_value("repo", inline, &mut iter)?),
            "verdict" => verdict = Some(record_flag_value("verdict", inline, &mut iter)?),
            "reason" => reason = Some(record_flag_value("reason", inline, &mut iter)?),
            "run-token" => run_token = Some(record_flag_value("run-token", inline, &mut iter)?),
            "state-root" => {
                state_root = Some(PathBuf::from(record_flag_value(
                    "state-root",
                    inline,
                    &mut iter,
                )?))
            }
            other => return Err(format!("unknown flag --{other}")),
        }
    }

    let pr = pr.ok_or_else(|| "missing required --pr".to_string())?;
    let pr: u32 = pr
        .parse()
        .map_err(|_| format!("--pr must be a non-negative integer, got {pr:?}"))?;

    let repo = repo.ok_or_else(|| "missing required --repo".to_string())?;
    // Reuse the store's traversal-safe slug guard so a malformed/unsafe repo is
    // rejected at parse time, before any path is derived.
    validate_repo_slug(&repo)?;

    let verdict = verdict.ok_or_else(|| "missing required --verdict".to_string())?;
    let verdict = match verdict.as_str() {
        "merge" => VerdictKind::Merge,
        "hold" => VerdictKind::Hold,
        other => {
            return Err(format!(
                "--verdict must be exactly `merge` or `hold` (lowercase), got {other:?}"
            ));
        }
    };

    let reason = reason.ok_or_else(|| "missing required --reason".to_string())?;
    if reason.trim().is_empty() {
        return Err("--reason must be non-empty".to_string());
    }

    let run_token = run_token.ok_or_else(|| "missing required --run-token".to_string())?;
    if run_token.trim().is_empty() {
        return Err("--run-token must be non-empty".to_string());
    }

    Ok(RecordVerdictArgs {
        pr,
        repo,
        verdict,
        reason,
        run_token,
        state_root,
    })
}

/// Run `simard merge record-verdict`, returning the process exit code
/// ([`EXIT_RECORDED`] / [`EXIT_USAGE`] / [`EXIT_IO`]). Emits `[simard]`-prefixed
/// diagnostics to stderr.
pub(crate) fn run_record_verdict(args: Vec<String>) -> i32 {
    let parsed = match parse_record_verdict_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] merge record-verdict: usage error: {e}");
            return EXIT_USAGE;
        }
    };

    let state_root = parsed.state_root.clone().unwrap_or_else(simard_state_root);
    let record = MergeVerdictRecord::new(
        parsed.pr,
        &parsed.repo,
        parsed.verdict,
        &parsed.reason,
        &parsed.run_token,
    );
    match write_record(&state_root, &record) {
        Ok(()) => {
            eprintln!(
                "[simard] merge record-verdict: recorded `{:?}` for {}#{} (token {}).",
                parsed.verdict, parsed.repo, parsed.pr, parsed.run_token
            );
            EXIT_RECORDED
        }
        Err(e) => {
            eprintln!("[simard] merge record-verdict: could not write record: {e}");
            EXIT_IO
        }
    }
}

/// Dispatch `simard merge <subcommand>`. Currently only `record-verdict`.
pub(crate) fn dispatch_merge_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = match args.next() {
        Some(s) => s,
        None => {
            print!("{MERGE_HELP}");
            return Ok(());
        }
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{MERGE_HELP}");
            Ok(())
        }
        "record-verdict" => {
            let argv: Vec<String> = args.collect();
            if argv
                .iter()
                .any(|a| a == "--help" || a == "-h" || a == "help")
            {
                print!("{MERGE_HELP}");
                return Ok(());
            }
            std::process::exit(run_record_verdict(argv));
        }
        other => Err(format!("unsupported command 'merge {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<MergePrCommand, Box<dyn std::error::Error>> {
        parse_merge_pr_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn dispatch_rejects_missing_pr_number() {
        let result = dispatch_merge_pr_command(std::iter::empty());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PR number"));
    }

    #[test]
    fn dispatch_rejects_non_numeric_pr() {
        let args = vec!["abc".to_string()].into_iter();
        let result = dispatch_merge_pr_command(args);
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid PR number"), "{err}");
        assert!(err.contains("abc"), "{err}");
    }

    #[test]
    fn dispatch_rejects_extra_args() {
        let args = vec!["1500".to_string(), "extra".to_string()].into_iter();
        let result = dispatch_merge_pr_command(args);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("unexpected"),
            "should reject extra args"
        );
    }

    #[test]
    fn parse_defaults_repo_to_home_when_omitted() {
        let cmd = parse(&["1500"]).unwrap();
        assert_eq!(
            cmd,
            MergePrCommand::Merge(MergePrArgs {
                pr_number: 1500,
                repo: "rysweet/Simard".to_string(),
            })
        );
    }

    #[test]
    fn parse_accepts_repo_flag_after_pr() {
        let cmd = parse(&["820", "--repo", "rysweet/amplihack-rs"]).unwrap();
        assert_eq!(
            cmd,
            MergePrCommand::Merge(MergePrArgs {
                pr_number: 820,
                repo: "rysweet/amplihack-rs".to_string(),
            })
        );
    }

    #[test]
    fn parse_accepts_repo_flag_before_pr() {
        let cmd = parse(&["--repo", "rysweet/RustyClawd", "42"]).unwrap();
        assert_eq!(
            cmd,
            MergePrCommand::Merge(MergePrArgs {
                pr_number: 42,
                repo: "rysweet/RustyClawd".to_string(),
            })
        );
    }

    #[test]
    fn parse_accepts_repo_equals_form() {
        let cmd = parse(&["7", "--repo=rysweet/amplihack-memory-lib"]).unwrap();
        assert_eq!(
            cmd,
            MergePrCommand::Merge(MergePrArgs {
                pr_number: 7,
                repo: "rysweet/amplihack-memory-lib".to_string(),
            })
        );
    }

    #[test]
    fn parse_rejects_repo_flag_without_value() {
        let err = parse(&["7", "--repo"]).unwrap_err().to_string();
        assert!(err.contains("--repo"), "{err}");
    }

    #[test]
    fn parse_rejects_malformed_repo() {
        let err = parse(&["7", "--repo", "not-a-slug"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid --repo"), "{err}");
    }

    #[test]
    fn parse_rejects_duplicate_repo_flag() {
        let err = parse(&["7", "--repo", "a/b", "--repo", "c/d"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn parse_help_sentinel() {
        for h in ["--help", "-h", "help"] {
            assert_eq!(parse(&[h]).unwrap(), MergePrCommand::Help);
        }
    }
}
