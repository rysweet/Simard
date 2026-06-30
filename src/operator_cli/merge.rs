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

/// Validate an `owner/repo` slug: exactly one `/`, non-empty halves, no
/// whitespace. Keeps a malformed value from reaching the `gh --repo` argument.
fn validate_repo(repo: &str) -> Result<(), Box<dyn std::error::Error>> {
    let parts: Vec<&str> = repo.split('/').collect();
    let well_formed = parts.len() == 2
        && parts.iter().all(|p| !p.is_empty())
        && !repo.chars().any(char::is_whitespace);
    if well_formed {
        Ok(())
    } else {
        Err(format!("invalid --repo '{repo}' (expected <owner/repo>)").into())
    }
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
