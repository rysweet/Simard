//! Recipe-runner-backed [`ProgressEvidenceChecker`] — delegates the LLM
//! call to `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/progress-assessment.yaml` recipe.
//!
//! This replaces [`super::progress_reviewer::LlmReviewerProgressChecker`]
//! for deployments where recipe-runner-rs is available, aligning with the
//! architectural direction in issue #1971: Simard should use the amplihack
//! recipe-runner as a design component rather than hand-coding Rust structs
//! that wrap LLM calls.
//!
//! The downward/no-change auto-accept fast path is preserved identically.
//! For upward claims the shim invokes `recipe-runner-rs` as a subprocess
//! with `-c` context vars and parses its stdout using the same
//! `parse_reviewer_response` logic from `progress_reviewer`.
//!
//! Fallback policy splits INFRA failures from SEMANTIC parse-misses
//! (reasoner-reliability; the sibling of the merge judge's
//! fail-closed-to-`Unclear` for the same class of parse-miss):
//!   * **Infra failure** — no usable output was produced (recipe spawn
//!     failure, non-zero exit, or an *empty* stdout): accept with a
//!     diagnostic so goals are never blocked on infrastructure issues.
//!   * **Semantic parse-miss** — the recipe exited 0 with non-empty output
//!     that carries no `accept`/`reject` keyword: reject, refusing the
//!     unverified progress bump so a hallucinated "no verdict" jump cannot
//!     land (`Reject` keeps the prior percent; it does not stall the goal).

use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::process::Command;

use super::progress_evidence::{EvidenceDecision, ProgressEvidenceChecker};
use super::types::ActiveGoal;

const ADAPTER_TAG: &str = "recipe-progress-checker";
const RECIPE_FILENAME: &str = "progress-assessment.yaml";

/// Max chars retained from the rationale before truncation.
const RATIONALE_MAX_CHARS: usize = 240;

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
fn resolve_recipe_path(repo_root: &std::path::Path) -> Option<PathBuf> {
    // Hot-reload path
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    // In-tree fallback
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Recipe-runner-backed progress evidence checker.
pub struct RecipeProgressChecker {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeProgressChecker {
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        // Verify recipe-runner-rs is available
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
    }
}

impl ProgressEvidenceChecker for RecipeProgressChecker {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        // Downward/no-change is always accepted (no recipe call needed).
        if new_percent <= old_percent {
            return EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: downward / no-change ({old_percent} -> {new_percent}) auto-accepted"
                ),
            };
        }

        let plan = goal
            .current_activity
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let wip_summary = render_wip_summary(goal);

        // Bound the free-text goal vars before they ride on argv (issues
        // #2640/#2692): defensively closes the E2BIG argv-overflow class and,
        // reusing the ooda_brain sanitizer, collapses newlines so a multi-line
        // description/plan/WIP summary can never break YAML interpolation
        // (#2127). The cap is generous (8000 chars) so real goal text survives.
        let problem = crate::ooda_brain::sanitize::sanitize_context_var(&goal.description, 8000);
        let plan = crate::ooda_brain::sanitize::sanitize_context_var(&plan, 8000);
        let wip_summary = crate::ooda_brain::sanitize::sanitize_context_var(&wip_summary, 8000);

        let result = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("goal_id={}", goal.id))
            .arg("-c")
            .arg(format!("problem={problem}"))
            .arg("-c")
            .arg(format!("plan={plan}"))
            .arg("-c")
            .arg(format!("prior_pct={old_percent}"))
            .arg("-c")
            .arg(format!("claimed_pct={new_percent}"))
            .arg("-c")
            .arg(format!("wip_summary={wip_summary}"))
            .output();

        let output = match result {
            Ok(o) => o,
            Err(e) => {
                return EvidenceDecision::Accept {
                    reason: format!(
                        "{ADAPTER_TAG}: recipe-runner-rs spawn failed ({e}); accepting to avoid blocking goal"
                    ),
                };
            }
        };

        let raw = String::from_utf8_lossy(&output.stdout).to_string();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: recipe exited with {}; accepting to avoid blocking goal. stderr: {}",
                    output.status,
                    truncate(&stderr, 200)
                ),
            };
        }

        let (decision, matched) = parse_verdict_outcome(&raw);
        crate::recipe_output::record_parse_outcome("progress_checker", matched);
        decision
    }
}

fn render_wip_summary(goal: &ActiveGoal) -> String {
    if goal.wip_refs.is_empty() {
        return String::new();
    }
    use std::fmt::Write;
    let mut s = String::new();
    for (i, w) in goal.wip_refs.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{:?}", w);
    }
    s
}

/// Parse recipe stdout for the progress verdict.
///
/// The recipe's prompt contract emits a JSON object
/// `{"verdict": "accept"|"reject", "rationale": "..."}`. This parser tries the
/// **structured JSON verdict first** — mirroring the merge judge's
/// `parse_judge_response`-first path and the direct-LLM tier's
/// [`super::progress_reviewer::decision_from_response`] — and only falls back
/// to a plain keyword scan for prose. Parsing the object first avoids substring
/// false-positives, e.g. an `accept` verdict whose *rationale* mentions
/// "reject" (which a naive `contains("reject")` scan would wrongly flip to
/// Reject), or "unacceptable" containing the `accept` substring.
///
/// Resolution:
/// 1. Structured `{"verdict": …}` JSON → exact accept/reject; an unknown
///    verdict string is a semantic parse-miss → reject (fail-closed).
/// 2. Prose keyword scan (negative keyword first) for non-JSON output.
/// 3. Non-empty output with no keyword → reject (semantic parse-miss: the
///    recipe ran but gave no verdict, so the unverified bump is refused).
/// 4. Output that strips to empty → accept (infra gap: no verdict produced,
///    keep fail-open so a lost-output hiccup does not block the goal).
pub fn parse_verdict_from_text(text: &str) -> EvidenceDecision {
    parse_verdict_outcome(text).0
}

/// Like [`parse_verdict_from_text`] but also returns whether a real verdict was
/// resolved (`true`) versus a parse-miss default firing (`false`). The flag
/// drives the `recipe_parse_*_total{phase}` counter at the subprocess call site
/// (issue #2484); the pure parser stays metric-free so unit tests write no
/// metrics.
pub fn parse_verdict_outcome(text: &str) -> (EvidenceDecision, bool) {
    // Strip ANSI escapes + drop whole tracing-log / runner-banner lines first
    // (shared #2484 extractor) so a noise-obscured verdict is not silently
    // dropped — e.g. a real "reject" must not be missed because it trailed an
    // ANSI-coloured log prefix, and an "already"/"accepted" substring inside a
    // dropped log line cannot fabricate a verdict.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);

    // 1. Structured JSON verdict first (robust against rationale text that
    //    happens to contain the opposite keyword). Reuses the direct-LLM tier's
    //    tolerant extractor (as-is / fenced / brace-balanced / outermost).
    if let Ok(parsed) = super::progress_reviewer::parse_reviewer_response(&cleaned) {
        let verdict_lc = parsed.verdict.trim().to_ascii_lowercase();
        let rationale = truncate(parsed.rationale.trim(), RATIONALE_MAX_CHARS);
        return match verdict_lc.as_str() {
            "accept" => (
                EvidenceDecision::Accept {
                    reason: format!("{ADAPTER_TAG}: accept — {rationale}"),
                },
                true,
            ),
            "reject" => (
                EvidenceDecision::Reject {
                    reason: format!("{ADAPTER_TAG}: reject — {rationale}"),
                },
                true,
            ),
            // Valid JSON object but an unknown verdict string → semantic
            // parse-miss → fail CLOSED (refuse the unverified progress bump).
            _ => (
                EvidenceDecision::Reject {
                    reason: format!(
                        "{ADAPTER_TAG}: unknown verdict {:?}; rejecting unverified progress",
                        parsed.verdict
                    ),
                },
                false,
            ),
        };
    }

    // 2. Prose keyword fallback (no parseable JSON object). Negative keyword
    //    first so a "cannot accept … reject" ordering resolves to reject.
    let lower = cleaned.to_ascii_lowercase();
    let rationale = truncate(cleaned.trim(), RATIONALE_MAX_CHARS);

    if lower.contains("reject") {
        (
            EvidenceDecision::Reject {
                reason: format!("{ADAPTER_TAG}: reject — {rationale}"),
            },
            true,
        )
    } else if lower.contains("accept") {
        (
            EvidenceDecision::Accept {
                reason: format!("{ADAPTER_TAG}: accept — {rationale}"),
            },
            true,
        )
    } else if cleaned.trim().is_empty() {
        // No reviewable content after noise-stripping — the run produced only
        // banner/log noise (or nothing at all), so no verdict was ever
        // emitted. Treat as an infra gap and keep fail-OPEN so a lost-output
        // hiccup does not block the goal. (Real, substantive prose that simply
        // omits a verdict keyword falls through to the fail-CLOSED branch.)
        (
            EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: empty recipe output; accepting to avoid blocking goal on infra"
                ),
            },
            false,
        )
    } else {
        // Successful, non-empty output but NO accept/reject keyword: the
        // reviewer ran yet produced no recognizable verdict. Fail CLOSED —
        // refuse the unverified progress bump so a hallucinated "no verdict"
        // jump cannot land (reasoner-reliability).
        (
            EvidenceDecision::Reject {
                reason: format!(
                    "{ADAPTER_TAG}: no verdict keyword in non-empty recipe output; rejecting unverified progress"
                ),
            },
            false,
        )
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::types::GoalProgress;

    fn goal_with_activity(activity: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: "test-goal".to_string(),
            description: "do the thing".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 10 },
            assigned_to: None,
            current_activity: activity.map(String::from),
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    #[test]
    fn downward_move_is_auto_accepted_without_recipe_call() {
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        let g = goal_with_activity(None);
        match checker.check(&g, 80, 50, Utc::now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("downward"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn no_change_is_auto_accepted() {
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        let g = goal_with_activity(None);
        assert!(matches!(
            checker.check(&g, 60, 60, Utc::now()),
            EvidenceDecision::Accept { .. }
        ));
    }

    #[test]
    fn upward_claim_with_missing_binary_falls_back_to_accept() {
        let checker = RecipeProgressChecker {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        let g = goal_with_activity(Some("working on it"));
        match checker.check(&g, 10, 20, Utc::now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(
                    reason.contains("recipe") || reason.contains("spawn"),
                    "got: {reason}"
                );
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on infra failure"),
        }
    }

    // ------------------------------------------------------------------
    // Text-based verdict parser (issue #1980)
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Structured JSON verdict first (robustness against rationale text that
    // contains the opposite keyword — the recipe tier now parses the object
    // before the prose keyword scan, matching the merge judge + direct-LLM tier).
    // ------------------------------------------------------------------

    #[test]
    fn json_accept_verdict_with_reject_in_rationale_accepts() {
        // Regression: a naive `contains("reject")`-first scan would wrongly
        // flip this legitimate accept to Reject because the RATIONALE mentions
        // "reject". Parsing the JSON verdict first fixes that.
        let text =
            r#"{"verdict": "accept", "rationale": "8pt delta matches plan; no reason to reject"}"#;
        let (decision, matched) = parse_verdict_outcome(text);
        match decision {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
                assert!(reason.contains("no reason to reject"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => {
                panic!("JSON accept must win even when the rationale mentions 'reject'")
            }
        }
        assert!(matched, "a real JSON verdict must report matched=true");
    }

    #[test]
    fn json_reject_verdict_parses() {
        let text = r#"{"verdict": "reject", "rationale": "100% claim with no shipped artifact"}"#;
        let (decision, matched) = parse_verdict_outcome(text);
        assert!(matches!(decision, EvidenceDecision::Reject { .. }));
        assert!(matched);
    }

    #[test]
    fn json_unknown_verdict_fails_closed() {
        // A valid JSON object with an unrecognized verdict string is a semantic
        // parse-miss → fail CLOSED.
        let text = r#"{"verdict": "accepted", "rationale": "typo verdict"}"#;
        let (decision, matched) = parse_verdict_outcome(text);
        match decision {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("unknown verdict"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => {
                panic!("unknown JSON verdict string must fail closed, not accept")
            }
        }
        assert!(
            !matched,
            "unknown verdict default must report matched=false"
        );
    }

    #[test]
    fn json_fenced_verdict_parses() {
        let text =
            "Here is my verdict:\n```json\n{\"verdict\":\"reject\",\"rationale\":\"stalled\"}\n```";
        assert!(matches!(
            parse_verdict_from_text(text),
            EvidenceDecision::Reject { .. }
        ));
    }

    #[test]
    fn text_verdict_accept_detected() {
        let text = "After reviewing the evidence, I accept the claimed progress.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_reject_detected() {
        let text = "The progress jump is not supported. I reject the claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => panic!("expected reject"),
        }
    }

    #[test]
    fn text_verdict_case_insensitive() {
        let text = "ACCEPT - the evidence checks out";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_reject_takes_priority_over_accept() {
        // If both keywords appear, reject wins (safer default)
        let text = "I cannot accept this, I must reject the claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => panic!("expected reject when both keywords present"),
        }
    }

    #[test]
    fn text_verdict_no_keyword_fails_closed_to_reject() {
        // Non-empty output with no accept/reject keyword is a SEMANTIC
        // parse-miss: the recipe ran but gave no verdict, so the gate must
        // fail CLOSED and refuse the unverified progress bump.
        let text = "The progress looks reasonable for this stage.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("no verdict keyword"), "got: {reason}");
                assert!(reason.contains("non-empty"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => {
                panic!("expected reject fallback when no keyword found in non-empty output")
            }
        }
    }

    #[test]
    fn text_verdict_empty_falls_back_to_accept() {
        // EMPTY output is an infra gap (no verdict produced), so the gate
        // stays fail-OPEN to avoid blocking the goal.
        let text = "";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("empty recipe output"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on empty text"),
        }
    }

    #[test]
    fn text_verdict_includes_rationale_from_text() {
        let text = "Based on the PR and commit history, I accept this progress claim.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(
                    reason.contains("PR and commit"),
                    "rationale should include text: {reason}"
                );
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_multiline_response() {
        let text = "Looking at the evidence:\n\n- PR #2018 has 3 commits\n- Tests pass\n\nI accept the claimed progress from 30% to 45%.";
        match parse_verdict_from_text(text) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("accept"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn text_verdict_recovers_reject_past_ansi_log_prefix() {
        // #2484: the verdict trails an ANSI-coloured tracing-log line. The
        // shared extractor strips both so the real reject is recovered and the
        // rationale carries no log/ANSI noise (a noise-obscured reject must
        // never be silently accepted-to-avoid-blocking).
        let esc = '\u{1b}';
        let text = format!(
            "{esc}[2m2026-06-28T08:08:58.151133Z{esc}[0m  INFO checker: scoring\n\
             Verdict: reject — no measurable progress this cycle."
        );
        match parse_verdict_from_text(&text) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
                assert!(!reason.contains("INFO checker"), "log line must be dropped");
            }
            EvidenceDecision::Accept { .. } => panic!("noise-obscured reject must be recovered"),
        }
    }

    #[test]
    fn parse_verdict_outcome_reports_match_flag_for_counter() {
        // The `bool` drives the `recipe_parse_*_total{progress_checker}` counter:
        // a real keyword ⇒ true (success), a parse-miss default ⇒ false.
        // A non-empty parse-miss now fails CLOSED (reject) rather than the old
        // permissive accept.
        let (decision, matched) = parse_verdict_outcome("just prose, no verdict keyword here");
        assert!(matches!(decision, EvidenceDecision::Reject { .. }));
        assert!(!matched, "parse-miss default must report matched=false");

        // An EMPTY body stays fail-open (infra gap) and also reports matched=false.
        let (empty_decision, empty_matched) = parse_verdict_outcome("   ");
        assert!(matches!(empty_decision, EvidenceDecision::Accept { .. }));
        assert!(
            !empty_matched,
            "empty-output default must report matched=false"
        );

        let (_, matched_reject) = parse_verdict_outcome("Verdict: reject — insufficient evidence");
        assert!(matched_reject, "a real verdict must report matched=true");
    }

    #[test]
    fn no_verdict_keyword_rejects_unverified_progress_bump() {
        // Reasoner-reliability regression (the "0%→100% with no verdict keyword"
        // false-done scenario): a recipe that runs and emits prose WITHOUT an
        // accept/reject keyword must NOT wave through the claimed progress.
        let recipe_output =
            "Looking at the plan and the WIP, the work appears to be moving along nicely.";
        let (decision, matched) = parse_verdict_outcome(recipe_output);
        assert!(
            matches!(decision, EvidenceDecision::Reject { .. }),
            "unverified progress must be rejected, not accepted"
        );
        assert!(!matched, "no keyword ⇒ matched=false");
    }

    #[test]
    fn text_verdict_multiline_no_keyword_fails_closed() {
        let text = "Reviewing the evidence:\n\n- some commits exist\n- plan referenced\n\nOverall this seems fine.";
        assert!(matches!(
            parse_verdict_from_text(text),
            EvidenceDecision::Reject { .. }
        ));
    }

    #[test]
    fn log_noise_only_output_is_infra_accept() {
        // Output that is ONLY strippable log/banner noise that collapses to
        // *nothing* (ISO-timestamp tracing lines, `Recipe:`, `Steps:`,
        // `[completed]`) is treated as an infra gap → fail-OPEN. This is the
        // safety net for a genuinely content-free run; it must NOT be conflated
        // with the fail-CLOSED semantic parse-miss (real prose without verdict).
        let esc = '\u{1b}';
        let noise = format!(
            "{esc}[2m2026-06-28T08:08:58.151133Z{esc}[0m  INFO runner: starting\n\
             Recipe: progress-assessment SUCCESS (12.0s)\n\
             Steps: 1/1 completed\n\
             [completed] assess (12.0s)\n"
        );
        let (decision, matched) = parse_verdict_outcome(&noise);
        match decision {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("empty recipe output"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => {
                panic!("fully-stripped noise-only output must be infra-accept")
            }
        }
        assert!(!matched, "noise-only default must report matched=false");
    }

    #[test]
    fn production_success_banner_without_verdict_fails_closed() {
        // The real recipe-runner-rs SUCCESS banner keeps its
        // `Recipe '<name>': SUCCESS (Ns)` summary line (it starts with
        // `Recipe '`, NOT `Recipe:`, so `strip_recipe_noise` does not drop it).
        // A run that emits ONLY this banner and no agent verdict therefore
        // leaves non-empty residue → fail-CLOSED (Reject), the progress-gate
        // analogue of the merge judge's #2569 banner → `Unclear`. This is the
        // reliability win: a recipe that ran but produced no verdict must not
        // wave through the claimed progress.
        let banner = "Recipe: progress-assessment (v1.0.0)\n\
                      Steps: 1\n\
                      Recipe 'progress-assessment': SUCCESS (30.0s)\n\
                      \x20 [completed] assess (30.0s)\n";
        match parse_verdict_from_text(banner) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("no verdict keyword"), "got: {reason}");
                assert!(reason.contains("non-empty"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => {
                panic!("a SUCCESS-banner-only progress run must fail closed, not accept")
            }
        }
    }
}

/// Issue #2570: `is_copilot_launcher_line` / `strip_recipe_noise` is a shared
/// chokepoint. The distillation fact-yield fix exempts JSON structural-token
/// lines (`{`, `"`, `[`) from launcher-noise classification; this
/// progress-checker consumer must keep stripping the REAL launcher preamble
/// (which never begins with a JSON token) so a noise-obscured verdict is still
/// read. This is the progress-checker slice of the cross-consumer regression
/// coverage.
#[cfg(test)]
mod issue_2570_cross_consumer_tests {
    use super::*;

    fn launcher_preamble() -> String {
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: cfg\n\
         INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
         Run 'copilot update' to check for updates.\n"
            .to_string()
    }

    #[test]
    fn progress_checker_still_strips_real_launcher_preamble() {
        let raw = format!(
            "{}reject — no measurable progress this cycle",
            launcher_preamble()
        );
        let (decision, matched) = parse_verdict_outcome(&raw);
        assert!(
            matched,
            "the real reject must be read, not silently defaulted"
        );
        match decision {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("reject"), "got: {reason}");
                assert!(
                    !reason.contains("launching copilot"),
                    "launcher preamble must be dropped from the rationale: {reason}"
                );
            }
            EvidenceDecision::Accept { .. } => {
                panic!("noise-obscured reject must be recovered after the #2570 guard")
            }
        }
    }

    #[test]
    fn shared_cleaner_preserves_pretty_fact_content_line_quoting_launcher_substring() {
        let content_line =
            "\"content\": \"the agent logged launching copilot binary=/x before answering\"";
        let cleaned = crate::recipe_output::strip_recipe_noise(content_line);
        assert_eq!(
            cleaned.as_ref(),
            content_line,
            "a JSON payload line must survive the shared cleaner"
        );
    }
}
