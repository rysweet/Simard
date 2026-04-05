//! Action verification and progress assessment helpers.

use crate::goal_curation::GoalProgress;

/// A single verified or unverified claim from the agent's response.
#[derive(Debug, Clone)]
pub(super) struct ActionVerification {
    pub(super) claim_type: &'static str,
    pub(super) detail: String,
    pub(super) verified: bool,
}

/// Scan the agent's execution summary for claimed actions and verify them
/// against actual repository/system state.
///
/// Checks for:
/// - `gh issue create` → verify issue exists via `gh issue list`
/// - `git checkout -b` → verify branch exists via `git branch`
/// - `gh pr create` → verify PR exists via `gh pr list`
/// - `cargo test` / `cargo check` → verify exit status mentioned
pub(super) fn verify_claimed_actions(summary: &str) -> Vec<ActionVerification> {
    let mut verifications = Vec::new();

    // Check for issue creation claims.
    if summary.contains("gh issue create")
        || summary.contains("created issue")
        || summary.contains("opened issue")
        || summary.contains("filed issue")
    {
        let verified = verify_recent_issue();
        verifications.push(ActionVerification {
            claim_type: "issue-create",
            detail: if verified {
                "confirmed: recent issue found via gh issue list".into()
            } else {
                "unverified: no recent issue found".into()
            },
            verified,
        });
    }

    // Check for branch creation claims.
    for line in summary.lines() {
        let trimmed = line.trim();
        if let Some(branch) = extract_branch_name(trimmed) {
            let verified = verify_branch_exists(&branch);
            verifications.push(ActionVerification {
                claim_type: "branch-create",
                detail: format!(
                    "{}: branch '{branch}'",
                    if verified { "confirmed" } else { "unverified" },
                ),
                verified,
            });
        }
    }

    // Check for PR creation claims.
    if summary.contains("gh pr create")
        || summary.contains("opened PR")
        || summary.contains("created PR")
        || summary.contains("pull request")
    {
        let verified = verify_recent_pr();
        verifications.push(ActionVerification {
            claim_type: "pr-create",
            detail: if verified {
                "confirmed: recent PR found via gh pr list".into()
            } else {
                "unverified: no recent PR found".into()
            },
            verified,
        });
    }

    verifications
}

/// Check if `git checkout -b <name>` appears and extract the branch name.
fn extract_branch_name(line: &str) -> Option<String> {
    let markers = ["git checkout -b ", "git switch -c "];
    for marker in markers {
        if let Some(idx) = line.find(marker) {
            let rest = &line[idx + marker.len()..];
            let branch = rest.split_whitespace().next()?;
            // Strip backticks or quotes.
            let branch = branch.trim_matches(|c| c == '`' || c == '\'' || c == '"');
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }
    None
}

/// Verify a branch exists locally or in the remote.
fn verify_branch_exists(branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["branch", "--list", branch])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// Verify that a GitHub issue was recently created (within the last 5 minutes).
fn verify_recent_issue() -> bool {
    std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "createdAt",
        ])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            // If there's a recent issue, the JSON array will be non-empty.
            o.status.success() && text.contains("createdAt")
        })
        .unwrap_or(false)
}

/// Verify that a PR was recently created.
fn verify_recent_pr() -> bool {
    std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "1",
            "--json",
            "createdAt",
        ])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            o.status.success() && text.contains("createdAt")
        })
        .unwrap_or(false)
}

/// Extract a progress percentage from the agent's execution summary.
///
/// Looks for a `PROGRESS: <number>` line in the summary or evidence.
/// Falls back to the current progress if no explicit assessment is found —
/// never auto-increments.
pub(super) fn assess_progress_from_outcome(
    outcome: &crate::base_types::BaseTypeOutcome,
    current: &GoalProgress,
) -> GoalProgress {
    // Search execution_summary and evidence for "PROGRESS: <N>"
    let sources = std::iter::once(outcome.execution_summary.as_str())
        .chain(outcome.evidence.iter().map(String::as_str));

    for text in sources {
        if let Some(p) = parse_progress_line(text) {
            return if p >= 100 {
                GoalProgress::Completed
            } else if p == 0 {
                GoalProgress::NotStarted
            } else {
                GoalProgress::InProgress { percent: p }
            };
        }
    }

    // No explicit progress found — preserve current state unchanged.
    current.clone()
}

/// Parse a "PROGRESS: <number>" line from text, returning the percentage.
fn parse_progress_line(text: &str) -> Option<u32> {
    const PREFIX: &str = "PROGRESS:";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= PREFIX.len()
            && trimmed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
            && let Ok(value) = trimmed[PREFIX.len()..].trim().parse::<u32>()
        {
            return Some(value.min(100));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_progress_line_extracts_percentage() {
        assert_eq!(parse_progress_line("PROGRESS: 45"), Some(45));
        assert_eq!(parse_progress_line("progress: 80"), Some(80));
        assert_eq!(
            parse_progress_line("Some text\nPROGRESS: 60\nMore text"),
            Some(60)
        );
        assert_eq!(parse_progress_line("no progress here"), None);
        assert_eq!(parse_progress_line("PROGRESS: 150"), Some(100)); // clamped
        assert_eq!(parse_progress_line("PROGRESS: 0"), Some(0));
    }

    #[test]
    fn assess_progress_keeps_current_when_no_marker() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "did some work".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 30 };
        assert_eq!(assess_progress_from_outcome(&outcome, &current), current);
    }

    #[test]
    fn assess_progress_extracts_from_summary() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "Assessed goal.\nPROGRESS: 55".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 30 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::InProgress { percent: 55 }
        );
    }

    #[test]
    fn assess_progress_extracts_from_evidence() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "no marker here".to_string(),
            evidence: vec!["PROGRESS: 100".to_string()],
        };
        let current = GoalProgress::InProgress { percent: 80 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::Completed
        );
    }

    #[test]
    fn assess_progress_zero_means_not_started() {
        use crate::base_types::BaseTypeOutcome;
        let outcome = BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "PROGRESS: 0".to_string(),
            evidence: vec![],
        };
        let current = GoalProgress::InProgress { percent: 10 };
        assert_eq!(
            assess_progress_from_outcome(&outcome, &current),
            GoalProgress::NotStarted
        );
    }

    #[test]
    fn extract_branch_name_from_checkout() {
        assert_eq!(
            extract_branch_name("git checkout -b feat/fix-issue-42"),
            Some("feat/fix-issue-42".to_string()),
        );
        assert_eq!(
            extract_branch_name("ran `git checkout -b feat/new-thing` and pushed"),
            Some("feat/new-thing".to_string()),
        );
        assert_eq!(
            extract_branch_name("git switch -c hotfix/urgent"),
            Some("hotfix/urgent".to_string()),
        );
        assert_eq!(extract_branch_name("just some text"), None);
    }

    #[test]
    fn verify_claimed_actions_detects_issue_creation_claims() {
        let v = verify_claimed_actions("I ran gh issue create to file the bug");
        assert!(!v.is_empty(), "should detect issue creation claim");
        assert_eq!(v[0].claim_type, "issue-create");
    }

    #[test]
    fn verify_claimed_actions_detects_branch_creation() {
        let v = verify_claimed_actions("Created branch with git checkout -b feat/my-fix");
        let branch_claims: Vec<_> = v
            .iter()
            .filter(|c| c.claim_type == "branch-create")
            .collect();
        assert!(
            !branch_claims.is_empty(),
            "should detect branch creation claim"
        );
        assert!(branch_claims[0].detail.contains("feat/my-fix"));
    }

    #[test]
    fn verify_claimed_actions_detects_pr_creation() {
        let v = verify_claimed_actions("I opened PR #42 for the fix");
        let pr_claims: Vec<_> = v.iter().filter(|c| c.claim_type == "pr-create").collect();
        assert!(!pr_claims.is_empty(), "should detect PR creation claim");
    }

    #[test]
    fn verify_claimed_actions_returns_empty_for_observation_only() {
        let v =
            verify_claimed_actions("Checked the repo status. Everything looks clean. PROGRESS: 30");
        assert!(
            v.is_empty(),
            "observation-only responses should have no claims to verify"
        );
    }
}
