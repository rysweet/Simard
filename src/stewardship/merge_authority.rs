//! Merge authority — Simard's gated authority to squash-merge a pull request
//! once it has independently demonstrated merge-readiness.
//!
//! See `prompt_assets/simard/engineer_system.md` (Merge-Ready Contract) and
//! `~/.copilot/skills/merge-ready/SKILL.md` for the canonical six criteria.
//!
//! Pipeline:
//! 1. `gh pr view <PR> --json body,statusCheckRollup,mergeable,reviewDecision`
//! 2. Parse the PR body for the six merge-ready evidence headings (each
//!    must contain non-trivial evidence, not just a placeholder).
//! 3. Verify `mergeable == "MERGEABLE"`.
//! 4. Verify every entry in `statusCheckRollup` is `SUCCESS`, `NEUTRAL`, or
//!    `SKIPPED`. Any `FAILURE`, `CANCELLED`, `TIMED_OUT`, `STARTUP_FAILURE`,
//!    `ACTION_REQUIRED`, `PENDING`, `QUEUED`, or `IN_PROGRESS` blocks the merge.
//! 5. If all checks pass: `gh pr merge <PR> --squash --delete-branch
//!    --repo rysweet/Simard` and return [`MergeOutcome::Merged`].
//! 6. Otherwise return [`MergeOutcome::Refused`] with a single human-readable
//!    reason (the *first* failing gate, in order, so the operator gets a
//!    deterministic message).
//!
//! TODO(brain-wiring): the OODA brain currently has no action kind for "merge
//! a PR I worked on". When the brain grows a `merge_pr` action, wire
//! [`merge_pr_if_merge_ready`] in via `src/ooda_actions/`. Until then it is
//! reachable via the operator CLI subcommand `simard merge-pr <PR>` (see
//! `src/operator_cli/merge.rs`) and via direct library calls.

use crate::error::{SimardError, SimardResult};

/// The six merge-ready evidence headings that MUST appear (and contain
/// non-trivial evidence) in the PR body. The headings come from
/// `~/.copilot/skills/merge-ready/pr-description-template.md`.
///
/// Order matters: refusal messages report the *first* missing section.
pub const REQUIRED_EVIDENCE_HEADINGS: [&str; 6] = [
    "### QA-team evidence",
    "### Documentation",
    "### Quality-audit",
    "### CI",
    "### PR description evidence",
    "### Scope",
];

/// Result of a merge-authority evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The PR satisfied every gate and was successfully squash-merged.
    Merged { pr_number: u32, repo: String },
    /// The PR did not satisfy a gate, or `gh pr merge` itself refused.
    /// `reason` is a single human-readable sentence the operator can act on.
    Refused { pr_number: u32, reason: String },
}

/// Snapshot of `gh pr view --json body,statusCheckRollup,mergeable,reviewDecision`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PrSnapshot {
    pub body: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<CheckRollupEntry>,
}

/// One row from `statusCheckRollup`. Both check runs and statuses get
/// normalised into this shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRollupEntry {
    /// Display name (`name` for check runs, `context` for statuses).
    pub name: String,
    /// One of: `SUCCESS`, `NEUTRAL`, `SKIPPED`, `FAILURE`, `CANCELLED`,
    /// `TIMED_OUT`, `STARTUP_FAILURE`, `ACTION_REQUIRED`, `PENDING`,
    /// `QUEUED`, `IN_PROGRESS`, or any state the gh CLI invents next week.
    /// We treat unknown values as failing-by-default.
    pub state: String,
}

/// Abstract `gh pr` operations used by [`merge_pr_if_merge_ready`]. The trait
/// keeps the evaluation logic testable; production wires it to
/// [`RealPrGhClient`] which shells out to `gh`.
pub trait PrGhClient {
    /// `gh pr view <pr> --repo <repo> --json body,statusCheckRollup,mergeable,reviewDecision`.
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot>;
    /// `gh pr merge <pr> --squash --delete-branch --repo <repo>`.
    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()>;
}

/// Production implementation that shells out to the `gh` binary.
#[derive(Default)]
pub struct RealPrGhClient;

impl RealPrGhClient {
    pub fn new() -> Self {
        Self
    }
}

impl PrGhClient for RealPrGhClient {
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot> {
        let pr = pr_number.to_string();
        let output = std::process::Command::new("gh")
            .args([
                "pr",
                "view",
                &pr,
                "--repo",
                repo,
                "--json",
                "body,statusCheckRollup,mergeable,reviewDecision",
            ])
            .output()
            .map_err(|e| SimardError::MergeAuthorityGhCommandFailed {
                reason: format!("failed to spawn `gh pr view`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: format!(
                    "`gh pr view {pr} --repo {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        parse_pr_view_json(&output.stdout)
    }

    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()> {
        let pr = pr_number.to_string();
        let output = std::process::Command::new("gh")
            .args([
                "pr",
                "merge",
                &pr,
                "--repo",
                repo,
                "--squash",
                "--delete-branch",
            ])
            .output()
            .map_err(|e| SimardError::MergeAuthorityGhCommandFailed {
                reason: format!("failed to spawn `gh pr merge`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: format!(
                    "`gh pr merge {pr} --repo {repo} --squash --delete-branch` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(())
    }
}

/// Parse `gh pr view --json body,statusCheckRollup,mergeable,reviewDecision`
/// stdout into a [`PrSnapshot`]. Public so the CLI can reuse it for dry-run
/// flows; tests cover both happy and malformed paths.
pub fn parse_pr_view_json(stdout: &[u8]) -> SimardResult<PrSnapshot> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        body: String,
        #[serde(default)]
        mergeable: String,
        #[serde(default, rename = "reviewDecision")]
        review_decision: String,
        #[serde(default, rename = "statusCheckRollup")]
        status_check_rollup: Vec<RawCheck>,
    }
    #[derive(serde::Deserialize)]
    struct RawCheck {
        // Check runs use `name`+`conclusion`/`status`; statuses use `context`+`state`.
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context: Option<String>,
        #[serde(default)]
        conclusion: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        state: Option<String>,
    }
    let raw: Raw = serde_json::from_slice(stdout).map_err(|e| {
        SimardError::MergeAuthorityEvaluationFailed {
            reason: format!("could not parse `gh pr view` JSON: {e}"),
        }
    })?;
    let checks = raw
        .status_check_rollup
        .into_iter()
        .map(|c| {
            let name = c
                .name
                .or(c.context)
                .unwrap_or_else(|| "<unnamed-check>".to_string());
            // gh reports a check-run as IN_PROGRESS via `status` until
            // `conclusion` is populated; once complete `conclusion` is the
            // truthful field. Statuses use `state`. Fall through in that
            // order so a half-finished check doesn't masquerade as success.
            let state = match (c.conclusion, c.status, c.state) {
                (Some(s), _, _) if !s.is_empty() => s,
                (_, Some(s), _) if !s.is_empty() => s,
                (_, _, Some(s)) if !s.is_empty() => s,
                _ => "UNKNOWN".to_string(),
            };
            CheckRollupEntry { name, state }
        })
        .collect();
    Ok(PrSnapshot {
        body: raw.body,
        mergeable: raw.mergeable,
        review_decision: raw.review_decision,
        checks,
    })
}

/// Minimum bytes of evidence under each heading before we consider it
/// "non-trivial". A pure placeholder line like
/// `### QA-team evidence\n\n- Scenario files: <path/to/scenario.yaml>` is ~60
/// bytes; a real entry is hundreds. We require **at least 80 bytes of
/// non-whitespace content under the heading AND at least one line without an
/// unfilled `<...>` placeholder bracket.**
const MIN_EVIDENCE_BYTES: usize = 80;

fn evidence_section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let start = body.find(heading)?;
    let after = &body[start + heading.len()..];
    let end = after.find("\n### ").unwrap_or(after.len());
    let end_h2 = after.find("\n## ").unwrap_or(after.len());
    Some(&after[..end.min(end_h2)])
}

fn evidence_is_nontrivial(section: &str) -> bool {
    let stripped: String = section.chars().filter(|c| !c.is_whitespace()).collect();
    if stripped.len() < MIN_EVIDENCE_BYTES {
        return false;
    }
    // Reject sections that are *only* placeholder lines like `<path/to/x>`.
    // Real sections must have at least one non-blank, non-list-marker line
    // that does not contain `<...>` placeholder brackets.
    section.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        if trimmed.starts_with("###") {
            return false;
        }
        // Strip leading `- ` from list items before checking.
        let body = trimmed.trim_start_matches("- ").trim();
        if body.is_empty() {
            return false;
        }
        !body.contains('<') || !body.contains('>')
    })
}

/// First gate that fails (in order). Returns `Ok(())` if every gate passes.
fn evaluate_gates(snapshot: &PrSnapshot) -> Result<(), String> {
    // Gate 1-6: each evidence heading present + non-trivial.
    for heading in REQUIRED_EVIDENCE_HEADINGS {
        match evidence_section(&snapshot.body, heading) {
            None => {
                return Err(format!(
                    "PR body is missing the required merge-ready section '{heading}'"
                ));
            }
            Some(section) if !evidence_is_nontrivial(section) => {
                return Err(format!(
                    "PR body section '{heading}' is empty or contains only template placeholders"
                ));
            }
            Some(_) => {}
        }
    }
    // Gate 7: mergeable
    if snapshot.mergeable != "MERGEABLE" {
        return Err(format!(
            "PR mergeable status is '{}' (expected 'MERGEABLE')",
            snapshot.mergeable
        ));
    }
    // Gate 8: every check is success-ish
    for check in &snapshot.checks {
        if !is_passing_state(&check.state) {
            return Err(format!(
                "CI check '{}' has state '{}' (expected SUCCESS/NEUTRAL/SKIPPED)",
                check.name, check.state
            ));
        }
    }
    Ok(())
}

fn is_passing_state(state: &str) -> bool {
    matches!(state, "SUCCESS" | "NEUTRAL" | "SKIPPED")
}

/// Evaluate the six merge-ready gates for `pr_number` against `repo`. If
/// every gate passes, squash-merge with branch deletion and return
/// [`MergeOutcome::Merged`]. Otherwise return [`MergeOutcome::Refused`] with
/// the single most-actionable reason (the first failing gate).
///
/// Errors (as opposed to [`MergeOutcome::Refused`]) only surface when we
/// could not even *evaluate* the PR — `gh` failed to run, returned malformed
/// JSON, or `gh pr merge` itself failed at the network layer despite the
/// gates being satisfied.
pub fn merge_pr_if_merge_ready(
    pr_number: u32,
    repo: &str,
    gh: &dyn PrGhClient,
) -> SimardResult<MergeOutcome> {
    let snapshot = gh.view_pr(repo, pr_number)?;
    if let Err(reason) = evaluate_gates(&snapshot) {
        return Ok(MergeOutcome::Refused { pr_number, reason });
    }
    gh.squash_merge(repo, pr_number)?;
    Ok(MergeOutcome::Merged {
        pr_number,
        repo: repo.to_string(),
    })
}

// ─────────────────────────── Tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A canonical, fully-populated PR body that should pass all six
    /// evidence gates. Each section has a paragraph of real prose (not
    /// `<placeholder>` lines).
    fn good_pr_body() -> String {
        r#"# feat: example PR

## Merge readiness

### QA-team evidence

Scenario file: tests/scenarios/merge_authority.yaml. Validation command
gadugi-test validate tests/scenarios/merge_authority.yaml passed with 0
errors. Run command gadugi-test run tests/scenarios/merge_authority.yaml
passed against the local env on 2025-11-25, 12 of 12 cases green; full log
at artifacts/2025-11-25-gadugi.log.

### Documentation

User-facing docs impact: yes. Updated docs/concepts/merge-authority.md to
describe the gated merge action, including refusal reasons. PR description
links to the new doc above.

### Quality-audit

Cycle 1 (SEEK 6 findings, all medium-low): VALIDATE confirmed 4 real, FIX
landed in commit a1b2c3d. Cycle 2 (SEEK 2 findings, both low): VALIDATE
confirmed both, FIX landed in commit d4e5f6a. Cycle 3 (SEEK 0 findings):
clean — final cycle, convergence reached.

### CI

Checks command: gh pr checks 1500 --repo rysweet/Simard. Result: all green
across the 14 GitHub Actions jobs (cargo test, clippy, fmt, doctests, MSRV,
release-build, OS matrix). Skipped checks: none. Flaky reruns performed:
none. Real failures fixed: none in this PR.

### PR description evidence

This PR description itself contains the six merge-ready evidence sections,
each with concrete artifacts (file paths, command output, commit SHAs)
rather than template placeholders.

### Scope

Changed files reviewed via git diff --name-only origin/main...HEAD.
Touched: src/stewardship/merge_authority.rs, src/operator_cli/merge.rs,
prompt_assets/simard/ooda_decide.md. Unrelated changes: none.
"#
        .to_string()
    }

    fn good_snapshot() -> PrSnapshot {
        PrSnapshot {
            body: good_pr_body(),
            mergeable: "MERGEABLE".to_string(),
            review_decision: "APPROVED".to_string(),
            checks: vec![
                CheckRollupEntry {
                    name: "build".into(),
                    state: "SUCCESS".into(),
                },
                CheckRollupEntry {
                    name: "clippy".into(),
                    state: "SUCCESS".into(),
                },
                CheckRollupEntry {
                    name: "license-scan".into(),
                    state: "NEUTRAL".into(),
                },
            ],
        }
    }

    #[derive(Default)]
    struct FakePrGhClient {
        snapshot: Mutex<Option<SimardResult<PrSnapshot>>>,
        merge_result: Mutex<Option<SimardResult<()>>>,
        view_calls: Mutex<Vec<(String, u32)>>,
        merge_calls: Mutex<Vec<(String, u32)>>,
    }

    impl FakePrGhClient {
        fn new() -> Self {
            Self::default()
        }
        fn seed_view(&self, result: SimardResult<PrSnapshot>) {
            *self.snapshot.lock().unwrap() = Some(result);
        }
        fn seed_merge(&self, result: SimardResult<()>) {
            *self.merge_result.lock().unwrap() = Some(result);
        }
        fn merge_call_count(&self) -> usize {
            self.merge_calls.lock().unwrap().len()
        }
    }

    impl PrGhClient for FakePrGhClient {
        fn view_pr(&self, repo: &str, pr: u32) -> SimardResult<PrSnapshot> {
            self.view_calls.lock().unwrap().push((repo.to_string(), pr));
            self.snapshot
                .lock()
                .unwrap()
                .clone()
                .expect("FakePrGhClient: no view_pr response seeded")
        }
        fn squash_merge(&self, repo: &str, pr: u32) -> SimardResult<()> {
            self.merge_calls
                .lock()
                .unwrap()
                .push((repo.to_string(), pr));
            self.merge_result.lock().unwrap().clone().unwrap_or(Ok(()))
        }
    }

    // ── Happy path ──

    #[test]
    fn merges_when_all_gates_pass() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        gh.seed_merge(Ok(()));
        let outcome = merge_pr_if_merge_ready(1500, "rysweet/Simard", &gh).unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                pr_number: 1500,
                repo: "rysweet/Simard".to_string(),
            }
        );
        assert_eq!(gh.merge_call_count(), 1);
    }

    // ── Missing-evidence variants (one per heading) ──

    #[test]
    fn refuses_when_qa_evidence_missing() {
        let mut snap = good_snapshot();
        // Remove the QA section heading entirely.
        snap.body = snap
            .body
            .replace("### QA-team evidence", "### qa-something-else");
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(42, "rysweet/Simard", &gh).unwrap();
        match outcome {
            MergeOutcome::Refused { pr_number, reason } => {
                assert_eq!(pr_number, 42);
                assert!(
                    reason.contains("### QA-team evidence"),
                    "reason should mention the missing heading: {reason}"
                );
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(
            gh.merge_call_count(),
            0,
            "must not call gh pr merge on refusal"
        );
    }

    #[test]
    fn refuses_when_documentation_evidence_is_only_placeholder() {
        let mut snap = good_snapshot();
        // Replace the entire Documentation section with the template
        // boilerplate (all `<placeholder>` lines).
        let placeholder = "### Documentation\n\n\
            - User-facing docs impact: <yes / no>\n\
            - Updated docs: <doc path>\n\
            - PR description links added: <list of links>\n\
            - Rationale if not applicable: <list of changed surfaces>\n\n";
        let start = snap.body.find("### Documentation").unwrap();
        let after = &snap.body[start..];
        let end_offset = after[1..].find("\n### ").map(|i| i + 1).unwrap();
        let mut new_body = snap.body[..start].to_string();
        new_body.push_str(placeholder);
        new_body.push_str(&snap.body[start + end_offset..]);
        snap.body = new_body;

        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(42, "rysweet/Simard", &gh).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(
                    reason.contains("### Documentation"),
                    "reason should call out Documentation section: {reason}"
                );
                assert!(
                    reason.contains("placeholder") || reason.contains("empty"),
                    "reason should explain why: {reason}"
                );
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    // ── CI failure ──

    #[test]
    fn refuses_on_ci_failure() {
        let mut snap = good_snapshot();
        snap.checks.push(CheckRollupEntry {
            name: "integration-tests".into(),
            state: "FAILURE".into(),
        });
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("integration-tests"), "{reason}");
                assert!(reason.contains("FAILURE"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
    }

    #[test]
    fn refuses_on_pending_check() {
        let mut snap = good_snapshot();
        snap.checks.push(CheckRollupEntry {
            name: "slow-bench".into(),
            state: "PENDING".into(),
        });
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap();
        assert!(matches!(outcome, MergeOutcome::Refused { .. }));
    }

    // ── Mergeable=CONFLICTING ──

    #[test]
    fn refuses_when_mergeable_conflicting() {
        let mut snap = good_snapshot();
        snap.mergeable = "CONFLICTING".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("CONFLICTING"), "{reason}");
                assert!(reason.contains("MERGEABLE"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
    }

    #[test]
    fn refuses_when_mergeable_unknown() {
        let mut snap = good_snapshot();
        snap.mergeable = "UNKNOWN".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap();
        assert!(matches!(outcome, MergeOutcome::Refused { .. }));
    }

    // ── Propagation: gh failures bubble as SimardError ──

    #[test]
    fn propagates_gh_view_failure() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: "gh: not found".into(),
        }));
        let err = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap_err();
        assert!(matches!(
            err,
            SimardError::MergeAuthorityGhCommandFailed { .. }
        ));
    }

    #[test]
    fn propagates_gh_merge_failure_after_passing_gates() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        gh.seed_merge(Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: "branch protection requires CODEOWNERS approval".into(),
        }));
        let err = merge_pr_if_merge_ready(1500, "rysweet/Simard", &gh).unwrap_err();
        match err {
            SimardError::MergeAuthorityGhCommandFailed { reason } => {
                assert!(reason.contains("branch protection"), "{reason}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ── Order-determinism: missing evidence reported before CI failure ──

    #[test]
    fn reports_first_failing_gate_in_canonical_order() {
        let mut snap = good_snapshot();
        // Remove the QA evidence AND inject a CI failure. The QA missing
        // section comes first in the canonical order, so the message must
        // mention QA, not CI.
        snap.body = snap.body.replace("### QA-team evidence", "### qa-other");
        snap.checks.push(CheckRollupEntry {
            name: "x".into(),
            state: "FAILURE".into(),
        });
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let outcome = merge_pr_if_merge_ready(7, "rysweet/Simard", &gh).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("QA-team evidence"), "{reason}");
                assert!(!reason.contains("FAILURE"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    // ── parse_pr_view_json ──

    #[test]
    fn parses_check_run_with_conclusion() {
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "statusCheckRollup": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "lint",  "status": "IN_PROGRESS", "conclusion": ""}
            ]
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.checks.len(), 2);
        assert_eq!(snap.checks[0].state, "SUCCESS");
        assert_eq!(snap.checks[1].state, "IN_PROGRESS");
    }

    #[test]
    fn parses_status_with_state_and_context() {
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "REVIEW_REQUIRED",
            "statusCheckRollup": [
                {"context": "ci/legacy", "state": "SUCCESS"},
                {"context": "ci/old",    "state": "PENDING"}
            ]
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.checks.len(), 2);
        assert_eq!(snap.checks[0].name, "ci/legacy");
        assert_eq!(snap.checks[0].state, "SUCCESS");
        assert_eq!(snap.checks[1].state, "PENDING");
    }

    #[test]
    fn parse_pr_view_json_rejects_garbage() {
        let err = parse_pr_view_json(b"not json at all").unwrap_err();
        assert!(matches!(
            err,
            SimardError::MergeAuthorityEvaluationFailed { .. }
        ));
    }

    // ── REQUIRED_EVIDENCE_HEADINGS sanity ──

    #[test]
    fn required_headings_match_skill_template() {
        // Hard-pin the six headings to prevent accidental reordering;
        // see ~/.copilot/skills/merge-ready/pr-description-template.md.
        assert_eq!(
            REQUIRED_EVIDENCE_HEADINGS,
            [
                "### QA-team evidence",
                "### Documentation",
                "### Quality-audit",
                "### CI",
                "### PR description evidence",
                "### Scope",
            ]
        );
    }
}
