//! Operator subcommand `simard merge-pr <PR>` — invokes Simard's merge
//! authority against a PR in the home repo (`rysweet/Simard`).
//!
//! The brain currently has no `merge_pr` action kind (see `TODO(brain-wiring)`
//! in `src/stewardship/merge_authority.rs`); this CLI gives the operator a
//! direct entry point.

use crate::stewardship::{MergeOutcome, RealPrGhClient, merge_pr_if_merge_ready};

use super::args::{next_required, reject_extra_args};

/// Repo Simard ships from. Hard-coded for now because the merge authority is
/// scoped to the home repo per PR1's design notes.
const HOME_REPO: &str = "rysweet/Simard";

pub(super) const MERGE_PR_HELP: &str = "\
Simard merge-pr subcommand

Usage: simard merge-pr <PR-number>

Squash-merges the given PR in rysweet/Simard if it passes merge-readiness checks.
";

pub(crate) fn dispatch_merge_pr_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pr_str = next_required(&mut args, "PR number")?;
    if matches!(pr_str.as_str(), "--help" | "-h" | "help") {
        print!("{MERGE_PR_HELP}");
        return Ok(());
    }
    let pr_number: u32 = pr_str
        .parse()
        .map_err(|_| format!("invalid PR number '{pr_str}'"))?;
    reject_extra_args(args)?;

    let gh = RealPrGhClient::new();
    let outcome = merge_pr_if_merge_ready(pr_number, HOME_REPO, &gh)?;
    match outcome {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
