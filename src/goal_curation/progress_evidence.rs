//! Progress-evidence gatekeeper for goal-board progress updates.
//!
//! Implements the hallucinated-progress meta-bug fix (issue #1967): a
//! proposed progress *increase* on an active goal is accepted only when
//! verifiable git evidence supports it. Without evidence, the gate
//! refuses to mutate the goal board and records a
//! `"brain hallucination detected: …"` cognitive-memory episode.
//!
//! Surface:
//! * [`ProgressEvidenceChecker`] — trait gating one decision.
//! * [`DefaultProgressEvidenceChecker`] — production checker (git +
//!   `gh` shellouts via [`GitRunner`] / [`GhRunner`] test seams).
//! * [`NoopProgressEvidenceChecker`] — kill-switch + test default.
//! * Façade [`crate::goal_curation::update_goal_progress_with_evidence`]
//!   wires this trait into the existing OODA loop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::types::ActiveGoal;

/// Outcome of a progress-evidence check.
///
/// Both variants are returned as `Ok(...)` by the gate façade — the
/// caller distinguishes `Accept` from `Reject` by pattern match, not by
/// `Result` discrimination. See `update_goal_progress_with_evidence`.
#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceDecision {
    /// Evidence found — the caller may apply the progress update.
    Accept { reason: String },
    /// No evidence — the caller must keep the prior percent and emit
    /// a hallucination audit episode.
    Reject { reason: String },
}

/// Gate trait — decides whether a proposed progress increase is backed
/// by verifiable git artifacts.
///
/// `Send + Sync` so a single `Arc<dyn ProgressEvidenceChecker>` can be
/// installed on `OodaBridges` and shared across OODA actions.
pub trait ProgressEvidenceChecker: Send + Sync {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        since: DateTime<Utc>,
    ) -> EvidenceDecision;
}

/// Test seam for the local-git half of [`DefaultProgressEvidenceChecker`].
pub trait GitRunner: Send + Sync {
    fn list_branches(&self, repo_root: &Path, pattern: &str) -> std::io::Result<Vec<String>>;
    fn commits_since(
        &self,
        repo_root: &Path,
        branch: &str,
        since: DateTime<Utc>,
    ) -> std::io::Result<Vec<String>>;
}

/// Test seam for the GitHub half of [`DefaultProgressEvidenceChecker`].
pub trait GhRunner: Send + Sync {
    fn search_prs(&self, repo_slug: &str, query: &str) -> std::io::Result<Vec<GhPr>>;
}

/// Minimal PR shape consumed by the checker. Deserialised from
/// `gh pr list ... --json number,title,body,state,createdAt,mergedAt`.
#[derive(Clone, Debug, Deserialize)]
pub struct GhPr {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "mergedAt", default)]
    pub merged_at: Option<DateTime<Utc>>,
}

// ===========================================================================
// DefaultProgressEvidenceChecker — production wiring
// ===========================================================================

/// Production checker: shells out to `git` and `gh` to verify that real
/// artifacts back a proposed progress increase.
pub struct DefaultProgressEvidenceChecker {
    pub repo_root: PathBuf,
    pub remote_slug: String,
    pub git: Arc<dyn GitRunner>,
    pub gh: Arc<dyn GhRunner>,
}

impl DefaultProgressEvidenceChecker {
    /// Production constructor — wires [`SystemGitRunner`] and
    /// [`SystemGhRunner`] for actual shellouts.
    pub fn new(repo_root: PathBuf, remote_slug: impl Into<String>) -> Self {
        Self {
            repo_root,
            remote_slug: remote_slug.into(),
            git: Arc::new(SystemGitRunner),
            gh: Arc::new(SystemGhRunner),
        }
    }
}

impl ProgressEvidenceChecker for DefaultProgressEvidenceChecker {
    fn check(
        &self,
        goal: &ActiveGoal,
        _old_percent: u32,
        _new_percent: u32,
        since: DateTime<Utc>,
    ) -> EvidenceDecision {
        let slug = slug_for(&goal.id);
        let branch_pattern = format!("engineer/{slug}-*");
        let mut reject_parts: Vec<String> = Vec::new();

        // ── Rule 1: engineer-branch commit since `since` ─────────────
        //
        // We probe both:
        //   (a) the canonical `engineer/<slug>` name (always probed, even
        //       if `list_branches` returns empty — guarantees one
        //       `commits_since` call per check so tests can observe the
        //       computed `since` value), and
        //   (b) every glob match `engineer/<slug>-*`.
        let canonical = format!("engineer/{slug}");
        let mut probe_branches: Vec<String> = vec![canonical];
        match self.git.list_branches(&self.repo_root, &branch_pattern) {
            Ok(branches) => {
                for b in branches {
                    if !probe_branches.contains(&b) {
                        probe_branches.push(b);
                    }
                }
            }
            Err(e) => {
                reject_parts.push(format!("git: io error listing engineer/{slug}-* ({e})"));
            }
        }
        let mut found_branch_commit = false;
        for branch in &probe_branches {
            match self.git.commits_since(&self.repo_root, branch, since) {
                Ok(commits) => {
                    if let Some(sha) = commits.first() {
                        let sha7 = sha.chars().take(7).collect::<String>();
                        return EvidenceDecision::Accept {
                            reason: format!("commit {sha7} on {branch} at {}", since.to_rfc3339()),
                        };
                    }
                    found_branch_commit |= !commits.is_empty();
                }
                Err(e) => {
                    reject_parts.push(format!("git: io error ({e}) on {branch}"));
                }
            }
        }
        if !found_branch_commit {
            reject_parts.push(format!("no commits on engineer/{slug}-*"));
        }

        // ── Build wip_refs lookups for rules 2/3 ─────────────────────
        let wip_issue_numbers: Vec<String> = goal
            .wip_refs
            .iter()
            .filter(|w| w.kind.eq_ignore_ascii_case("issue") || w.kind.eq_ignore_ascii_case("pr"))
            .map(|w| w.ref_id.clone())
            .collect();

        // ── Rules 2 & 3: query `gh pr list` ──────────────────────────
        let query = format!("{slug} in:title,body");
        match self.gh.search_prs(&self.remote_slug, &query) {
            Ok(prs) => {
                // Rule 2: any PR (any state) created >= since whose
                // title/body contains the goal slug or a wip_refs issue.
                for pr in &prs {
                    if pr.created_at < since {
                        continue;
                    }
                    let title_lc = pr.title.to_ascii_lowercase();
                    let body_lc = pr.body.as_deref().unwrap_or("").to_ascii_lowercase();
                    let slug_lc = slug.to_ascii_lowercase();
                    let mentions_slug = title_lc.contains(&slug_lc) || body_lc.contains(&slug_lc);
                    let mentions_wip_issue = wip_issue_numbers.iter().any(|n| {
                        let needle = format!("#{n}");
                        title_lc.contains(&needle) || body_lc.contains(&needle)
                    });
                    if mentions_slug || mentions_wip_issue {
                        return EvidenceDecision::Accept {
                            reason: format!("PR #{} references goal", pr.number),
                        };
                    }
                }

                // Rule 3: any merged PR with mergedAt >= since whose body
                // matches `(?i)\b(close[sd]?|fix(?:es|ed)?|resolve[sd]?)\s+#(\d+)\b`
                // and `\2` is in `wip_issue_numbers`.
                for pr in &prs {
                    if !pr.state.eq_ignore_ascii_case("MERGED") {
                        continue;
                    }
                    let Some(merged_at) = pr.merged_at else {
                        continue;
                    };
                    if merged_at < since {
                        continue;
                    }
                    let body = pr.body.as_deref().unwrap_or("");
                    for issue in &wip_issue_numbers {
                        if body_closes_issue(body, issue) {
                            return EvidenceDecision::Accept {
                                reason: format!(
                                    "PR #{} closed #{} at {}",
                                    pr.number,
                                    issue,
                                    merged_at.to_rfc3339()
                                ),
                            };
                        }
                    }
                }
                reject_parts.push("no PRs referencing goal".to_string());
                if !wip_issue_numbers.is_empty() {
                    reject_parts.push(format!(
                        "no merged PRs closing #{} since {}",
                        wip_issue_numbers.join(",#"),
                        since.to_rfc3339()
                    ));
                }
            }
            Err(e) => {
                reject_parts.push(format!("gh: io error ({e})"));
            }
        }

        EvidenceDecision::Reject {
            reason: reject_parts.join(", "),
        }
    }
}

/// Match a `(?i)\b(close[sd]?|fix(?:es|ed)?|resolve[sd]?)\s+#NNNN\b`
/// clause against `body` where `NNNN` equals `issue`.
fn body_closes_issue(body: &str, issue: &str) -> bool {
    let lc = body.to_ascii_lowercase();
    let keywords = [
        "close ",
        "closes ",
        "closed ",
        "fix ",
        "fixes ",
        "fixed ",
        "resolve ",
        "resolves ",
        "resolved ",
    ];
    for kw in keywords {
        let needle = format!("{kw}#{issue}");
        // Word-boundary check: ensure character after #NNNN is a
        // non-digit (or end-of-string).
        let mut search_from = 0usize;
        while let Some(pos) = lc[search_from..].find(&needle) {
            let abs = search_from + pos;
            let end = abs + needle.len();
            let next = lc[end..].chars().next();
            match next {
                None => return true,
                Some(c) if !c.is_ascii_alphanumeric() => return true,
                _ => {}
            }
            search_from = end;
        }
    }
    false
}

/// Slugify a goal id for engineer-branch matching: lowercased,
/// non-alphanumerics replaced with `-`, collapsed.
fn slug_for(goal_id: &str) -> String {
    let mut out = String::with_capacity(goal_id.len());
    let mut prev_dash = false;
    for c in goal_id.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

// ===========================================================================
// NoopProgressEvidenceChecker — kill switch & test default
// ===========================================================================

/// Always returns `Accept { reason: "noop checker (no evidence enforced)" }`.
///
/// Used:
/// 1. By tests' default bridges constructors so existing tests don't
///    need to mock `git`/`gh`.
/// 2. As the operator escape hatch via `SIMARD_PROGRESS_EVIDENCE=off` at
///    daemon boot.
pub struct NoopProgressEvidenceChecker;

impl ProgressEvidenceChecker for NoopProgressEvidenceChecker {
    fn check(
        &self,
        _goal: &ActiveGoal,
        _old_percent: u32,
        _new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        EvidenceDecision::Accept {
            reason: "noop checker (no evidence enforced)".to_string(),
        }
    }
}

// ===========================================================================
// System runners (production shellouts)
// ===========================================================================

/// Real `git` runner — shells out to the `git` binary on `PATH`.
pub struct SystemGitRunner;

impl GitRunner for SystemGitRunner {
    fn list_branches(&self, repo_root: &Path, pattern: &str) -> std::io::Result<Vec<String>> {
        // pattern is `engineer/<slug>-*`. We pass it through `git
        // for-each-ref refs/heads/<pattern>` so the glob is interpreted
        // by git itself.
        let refspec = format!("refs/heads/{pattern}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["for-each-ref", "--format=%(refname:short)"])
            .arg(&refspec)
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(std::io::Error::other(format!(
                "git for-each-ref failed: {stderr}"
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn commits_since(
        &self,
        repo_root: &Path,
        branch: &str,
        since: DateTime<Utc>,
    ) -> std::io::Result<Vec<String>> {
        let since_str = since.to_rfc3339();
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["log", "--since", &since_str, "--pretty=%H", branch])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(std::io::Error::other(format!(
                "git log failed on {branch}: {stderr}"
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }
}

/// Real `gh` runner — shells out to the `gh` CLI on `PATH`.
pub struct SystemGhRunner;

impl GhRunner for SystemGhRunner {
    fn search_prs(&self, repo_slug: &str, query: &str) -> std::io::Result<Vec<GhPr>> {
        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--repo",
                repo_slug,
                "--search",
                query,
                "--state",
                "all",
                "--json",
                "number,title,body,state,createdAt,mergedAt",
                "--limit",
                "50",
            ])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(std::io::Error::other(format!(
                "gh pr list failed: {stderr}"
            )));
        }
        let stdout = output.stdout;
        let prs: Vec<GhPr> = serde_json::from_slice(&stdout)
            .map_err(|e| std::io::Error::other(format!("gh pr list parse: {e}")))?;
        Ok(prs)
    }
}

// ===========================================================================
// Process-start fallback timestamp
// ===========================================================================

/// Returns the daemon's process-start timestamp (cached via `OnceLock`).
///
/// Last-resort `since` value when a goal has no
/// `last_progress_update_at` and no prior `"goal progress accepted: …"`
/// memory episode. Guarantees the gate is never a silent open door on a
/// fresh daemon process.
pub fn process_start() -> DateTime<Utc> {
    static START: OnceLock<DateTime<Utc>> = OnceLock::new();
    *START.get_or_init(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_for_simple_id() {
        assert_eq!(
            slug_for("improve-cognitive-memory-persistence"),
            "improve-cognitive-memory-persistence"
        );
    }

    #[test]
    fn slug_for_strips_non_alnum() {
        assert_eq!(slug_for("Foo Bar/Baz!"), "foo-bar-baz");
    }

    #[test]
    fn slug_for_collapses_dashes() {
        assert_eq!(slug_for("a__b   c"), "a-b-c");
    }

    #[test]
    fn body_closes_issue_basic() {
        assert!(body_closes_issue("Fixes #1967", "1967"));
        assert!(body_closes_issue("This closes #1967.", "1967"));
        assert!(body_closes_issue("resolved #1967\n", "1967"));
        assert!(body_closes_issue("RESOLVES #1967", "1967"));
    }

    #[test]
    fn body_closes_issue_word_boundary() {
        // #19670 should not match #1967.
        assert!(!body_closes_issue("Fixes #19670", "1967"));
        // #1967a is not a number boundary; we don't match either.
        assert!(!body_closes_issue("Fixes #1967a", "1967"));
    }

    #[test]
    fn body_closes_issue_no_match() {
        assert!(!body_closes_issue("references #1967", "1967"));
        assert!(!body_closes_issue("see #1967", "1967"));
    }

    #[test]
    fn noop_checker_always_accepts() {
        let g = ActiveGoal {
            id: "x".into(),
            description: "y".into(),
            priority: 1,
            status: crate::goal_curation::GoalProgress::InProgress { percent: 10 },
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        };
        let dec = NoopProgressEvidenceChecker.check(&g, 10, 20, Utc::now());
        match dec {
            EvidenceDecision::Accept { reason } => assert!(reason.contains("noop")),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn process_start_is_stable_across_calls() {
        let a = process_start();
        let b = process_start();
        assert_eq!(a, b);
    }
}
