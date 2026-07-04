//! Recipe-runner-backed [`MergeJudge`] — delegates the LLM call to
//! `recipe-runner-rs` executing the
//! `prompt_assets/simard/recipes/merge-readiness-judge.yaml` recipe.
//!
//! This replaces [`super::merge_judge::LlmMergeJudge`] for deployments
//! where recipe-runner-rs is available, aligning with the architectural
//! direction in issue #1971.
//!
//! ## Verdict transport (issue #2419 family: #2428/#2430/#2435/#2462/#2463)
//!
//! The shim invokes `recipe-runner-rs` with `--output-format json` and extracts
//! the agent's REAL output from the JSON envelope's final step result
//! ([`extract_recipe_decision_output`]) before parsing the structured verdict.
//! This is the same fix landed for the engineer-lifecycle brain in #2419: in the
//! default `text` output mode `recipe-runner-rs` prints only a human SUCCESS
//! banner (`Recipe: merge-readiness-judge ... SUCCESS ...`) to stdout — which
//! contains no `ready/not_ready/unclear` verdict — so every gated merge was
//! blocked with "no verdict keyword found".
//!
//! The extracted output is parsed by [`parse_merge_outcome`]: the structured
//! `{"verdict": …}` JSON first (via [`parse_judge_response`]), then a keyword
//! fallback ([`parse_merge_verdict_from_text`]) for prose. On a parse-miss the
//! judge runs the shared confidence-gated escalation ladder
//! ([`run_brain_ladder`]) — schema-repair → high-effort re-prompt — and only
//! then FAILS CLOSED to [`Verdict::Unclear`] (never a `ready`-without-verdict).
//! A `brain_verdict_parsed_total{phase="merge_judge"}` metric is emitted on
//! both the parsed and the defaulted branch (issue #2429).
//!
//! Genuine recipe-runner failures (spawn / nonzero exit / envelope decode)
//! still propagate as `SimardError` — the merge authority handles them — so an
//! infra failure is never masked by a fail-closed verdict.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{SimardError, SimardResult};
use crate::ooda_brain::{
    BrainPhase, EscalationConfig, LadderRung, LifecycleParseOutcome, build_phase_escalation_note,
    extract_recipe_decision_output, record_verdict_parse_metric, run_brain_ladder,
};

use super::merge_authority::PrSnapshot;
use super::merge_judge::{JudgeOutcome, MergeJudge, MergeJudgeKind, Verdict, parse_judge_response};

const ADAPTER_TAG: &str = "recipe-merge-judge";
const RECIPE_FILENAME: &str = "merge-readiness-judge.yaml";

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME` env var (mirrors `disk_health::resolve_recipe_path`).
/// Production passes `None`, falling back to [`dirs::home_dir`].
fn resolve_recipe_path(
    repo_root: &std::path::Path,
    home_override: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let home = home_override.map(PathBuf::from).or_else(dirs::home_dir);
    if let Some(home) = home {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Recipe-runner-backed merge-readiness judge.
pub struct RecipeMergeJudge {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeMergeJudge {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        Self::new_with_home(repo_root, None)
    }

    /// Like [`RecipeMergeJudge::new`], but accepts a `home_override` for the
    /// hot-reload lookup so tests stay hermetic against the ambient
    /// `~/.simard/prompt_assets` directory. Production calls `new` (`None`).
    fn new_with_home(
        repo_root: &std::path::Path,
        home_override: Option<&std::path::Path>,
    ) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, home_override)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
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

impl MergeJudge for RecipeMergeJudge {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        // `pr-<N>` plays the role of `goal_id` in the shared ladder/metric so
        // the merge-judge phase lines up with decide/orient in `metrics.jsonl`.
        let pr_label = format!("pr-{pr_number}");
        let invoke = |rung: LadderRung, prior: &str| {
            self.invoke_judge_raw(pr_number, repo, snapshot, rung, prior)
        };

        // Base (cheap) attempt. A genuine recipe-runner failure (spawn / nonzero
        // exit / envelope decode) propagates as `Err` so an infra fault is never
        // masked — only a *parse-miss* on a successful run fails closed below.
        let base_raw = invoke(LadderRung::Base, "")?;
        let (judgment, outcome) = parse_merge_outcome(&base_raw);
        if !outcome.is_parse_failure() {
            record_verdict_parse_metric(BrainPhase::MergeJudge, &pr_label, outcome, "ok", 1);
            crate::recipe_output::record_parse_outcome("merge_judge", true);
            return Ok(judgment);
        }

        // Parse-miss → confidence-gated escalation ladder, then fail closed to
        // `Verdict::Unclear` (acceptance: never SUCCESS-without-verdict). The
        // merge authority refuses on `Unclear`, so the merge does not proceed.
        let cfg = EscalationConfig::from_env();
        let (final_judgment, final_outcome, attempts, termination) = run_brain_ladder(
            "merge_judge",
            &pr_label,
            &base_raw,
            outcome,
            &cfg,
            invoke,
            parse_merge_outcome,
            || fail_closed_unclear(&base_raw),
            |o| verdict_label(&o.verdict).to_string(),
        );
        // issue #2496: record the precise `LadderTermination` cause (previously
        // discarded as `_termination`) so a fail-closed `Unclear` row attributes
        // the default to its terminal path, consistent with decide/orient.
        record_verdict_parse_metric(
            BrainPhase::MergeJudge,
            &pr_label,
            final_outcome,
            termination.cause_label(),
            attempts,
        );
        crate::recipe_output::record_parse_outcome(
            "merge_judge",
            !final_outcome.is_parse_failure(),
        );
        Ok(final_judgment)
    }

    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Recipe
    }
}

impl RecipeMergeJudge {
    /// Invoke the merge-readiness recipe once for a ladder rung, returning the
    /// agent's raw output (the JSON envelope's final step result). Passes
    /// `--output-format json` (issue #2428) and the (possibly empty)
    /// `escalation_note`. Genuine recipe-runner failures propagate as `Err`.
    fn invoke_judge_raw(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
        rung: LadderRung,
        prior_output: &str,
    ) -> SimardResult<String> {
        let escalation_note = build_merge_escalation_note(rung, prior_output);
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            // issue #2428: text mode prints only the SUCCESS banner to stdout;
            // the agent's `{"verdict": …}` is exposed only via the JSON envelope.
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("pr_number={pr_number}"))
            .arg("-c")
            .arg(format!("repo={repo}"))
            .arg("-c")
            .arg(format!("pr_body={}", snapshot.body))
            // issue #2432: the (possibly empty) escalation/schema-repair note.
            .arg("-c")
            .arg(format!("escalation_note={escalation_note}"))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        extract_recipe_decision_output(&output.stdout, ADAPTER_TAG)
    }
}

/// Parse the agent's extracted output into a [`JudgeOutcome`] AND a
/// [`LifecycleParseOutcome`] classification (issue #2428 / #2429).
///
/// Strategy: the structured `{"verdict": …}` JSON first (via
/// [`parse_judge_response`], which tolerates fences/prose-wrapping), then a
/// keyword fallback ([`parse_merge_verdict_from_text`]) for plain prose. When
/// NEITHER yields a verdict the judge FAILS CLOSED to [`Verdict::Unclear`] and
/// the outcome is `DefaultEmpty` (no output) or `DefaultMalformed` (output
/// present but unparseable) — the merge authority refuses on `Unclear`, so the
/// merge never proceeds without a real verdict.
pub(crate) fn parse_merge_outcome(text: &str) -> (JudgeOutcome, LifecycleParseOutcome) {
    if let Ok(out) = parse_judge_response(text) {
        return (out, LifecycleParseOutcome::Parsed);
    }
    if let Ok(out) = parse_merge_verdict_from_text(text) {
        return (out, LifecycleParseOutcome::Parsed);
    }
    let miss = if text.trim().is_empty() {
        LifecycleParseOutcome::DefaultEmpty
    } else {
        LifecycleParseOutcome::DefaultMalformed
    };
    (fail_closed_unclear(text), miss)
}

/// The loud fail-closed verdict: `Unclear` with a rationale naming the
/// parse-miss so a defaulted verdict is never mistaken for a real `ready`/
/// `not_ready` call (acceptance: never SUCCESS-without-verdict).
fn fail_closed_unclear(raw: &str) -> JudgeOutcome {
    JudgeOutcome {
        verdict: Verdict::Unclear,
        rationale: format!(
            "{ADAPTER_TAG}: recipe produced no parseable verdict after escalation; failing closed \
             to unclear (raw={:?})",
            truncate(raw, 200)
        ),
        blockers: vec![],
    }
}

/// Build the merge-judge `escalation_note` for a ladder rung, reminding the
/// agent of the structured verdict contract (issue #2428 / #2432).
fn build_merge_escalation_note(rung: LadderRung, prior_output: &str) -> String {
    build_phase_escalation_note(
        rung,
        prior_output,
        "Return EXACTLY one JSON object and nothing else: \
         {\"verdict\": \"ready\"|\"not_ready\"|\"unclear\", \"rationale\": \"...\"}. \
         No prose, no markdown fences around it.",
        "Re-read the six merge-ready evidence sections carefully BEFORE answering, then output \
         ONLY the JSON verdict object.",
    )
}

/// The snake_case label for a verdict, used as the `decision` field in
/// escalation-ladder logging.
fn verdict_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Ready => "ready",
        Verdict::NotReady => "not_ready",
        Verdict::Unclear => "unclear",
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

/// Parse recipe stdout text for merge-readiness verdict keywords.
///
/// The recipe runs an agent that produces natural language output.
/// We scan for verdict keywords and use the surrounding text as rationale.
/// No JSON parsing needed — the agent already makes the decision.
///
/// Scan rules (case-insensitive):
/// - "not_ready" or "not ready" → NotReady
/// - "unclear" → Unclear
/// - "ready" (without "not" prefix) → Ready
/// - None found → error
pub fn parse_merge_verdict_from_text(text: &str) -> Result<JudgeOutcome, String> {
    // Strip ANSI escapes + drop whole tracing-log / runner-banner lines first
    // (shared #2484 extractor) so a noise-obscured verdict is not silently
    // missed and a dropped log line's keyword substring (e.g. "already"
    // containing "ready") cannot masquerade as a verdict.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err(format!("{ADAPTER_TAG}: recipe returned empty output"));
    }

    let lower = trimmed.to_ascii_lowercase();
    let rationale = truncate(trimmed, 500);

    if lower.contains("not_ready") || lower.contains("not ready") {
        Ok(JudgeOutcome {
            verdict: Verdict::NotReady,
            rationale,
            blockers: vec![],
        })
    } else if lower.contains("unclear") {
        Ok(JudgeOutcome {
            verdict: Verdict::Unclear,
            rationale,
            blockers: vec![],
        })
    } else if lower.contains("ready") {
        Ok(JudgeOutcome {
            verdict: Verdict::Ready,
            rationale,
            blockers: vec![],
        })
    } else {
        Err(format!(
            "{ADAPTER_TAG}: no verdict keyword (ready/not_ready/unclear) found in recipe output; raw={:?}",
            truncate(trimmed, 200)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let home = tempfile::tempdir().unwrap();
        let judge = RecipeMergeJudge::new_with_home(
            std::path::Path::new("/nonexistent"),
            Some(home.path()),
        );
        assert!(judge.is_none());
    }

    #[test]
    fn kind_returns_recipe() {
        let judge = RecipeMergeJudge {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        assert_eq!(judge.kind(), MergeJudgeKind::Recipe);
        assert!(judge.kind().is_configured());
    }

    // ------------------------------------------------------------------
    // Text-based merge verdict parser (issue #1980)
    // ------------------------------------------------------------------

    #[test]
    fn text_verdict_ready() {
        let text = "After reviewing the PR body, I find it ready for merge. All six sections are present and substantive.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
        assert!(out.rationale.contains("ready"));
    }

    #[test]
    fn text_verdict_not_ready() {
        let text = "The PR is not_ready because the Quality-audit section is missing.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_not_ready_with_space() {
        let text = "This PR is not ready — the test plan section is empty.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_unclear() {
        let text = "The PR body appears truncated. My verdict is unclear.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Unclear);
    }

    #[test]
    fn text_verdict_case_insensitive() {
        let text = "READY - all criteria met";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn text_verdict_not_ready_wins_over_ready() {
        // "not_ready" contains "ready" but should match not_ready first
        let text = "The PR is not_ready due to missing sections.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
    }

    #[test]
    fn text_verdict_empty_is_error() {
        let result = parse_merge_verdict_from_text("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn text_verdict_no_keyword_is_error() {
        let result = parse_merge_verdict_from_text("The PR looks interesting but I cannot decide.");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no verdict keyword"));
    }

    #[test]
    fn text_verdict_multiline_response() {
        let text = "## Merge Readiness Assessment\n\nAfter reviewing all sections:\n\n- Problem statement: ✓\n- Solution: ✓\n- Test plan: ✓\n\nVerdict: ready\n";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn text_verdict_rationale_is_full_text() {
        let text = "Comprehensive analysis shows this PR is ready.";
        let out = parse_merge_verdict_from_text(text).unwrap();
        assert!(
            out.rationale.contains("Comprehensive"),
            "rationale should include full text: {}",
            out.rationale
        );
    }

    #[test]
    fn text_verdict_drops_ready_substring_in_noise_log_line() {
        // #2484: "already" contains "ready"; on a raw scan the tracing-log line
        // would falsely yield Verdict::Ready. After shared noise-stripping the
        // log line is dropped and the remaining prose carries no verdict, so the
        // judge correctly errors (→ fail-closed) instead of fabricating a Ready.
        let text = "2026-06-28T08:08:58.151133Z  INFO judge: already scored pr\n\
                    The assessment is complete but the prose is inconclusive.";
        let err = parse_merge_verdict_from_text(text).unwrap_err();
        assert!(err.contains("no verdict keyword"), "got: {err}");
    }

    #[test]
    fn text_verdict_recovers_not_ready_past_ansi_log_prefix() {
        // The verdict trails an ANSI-coloured tracing-log line. The shared
        // extractor strips both so the real not_ready verdict is recovered and
        // its rationale carries no log/ANSI noise.
        let esc = '\u{1b}';
        let text = format!(
            "{esc}[2m2026-06-28T08:08:58.151133Z{esc}[0m  INFO judge: scoring\n\
             Verdict: not_ready — missing test plan."
        );
        let out = parse_merge_verdict_from_text(&text).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
        assert!(
            !out.rationale.contains("INFO judge"),
            "log line must be dropped"
        );
    }
}

// =====================================================================
// issue_2428_tests — merge-judge verdict-parse cluster
// (#2428 / #2430 / #2435 / #2462 / #2463)
//
// Root cause (identical to the #2419 lifecycle bug, different surface):
// `RecipeMergeJudge::judge` invokes `recipe-runner-rs` in its DEFAULT `text`
// output mode, so `output.stdout` is only the human SUCCESS banner
// (`Recipe: merge-readiness-judge ... SUCCESS ...`). The agent's actual
// `{"verdict": ...}` is exposed ONLY via `--output-format json`
// (`step_results[].output`). `parse_merge_verdict_from_text` then finds no
// verdict keyword and the merge is blocked for EVERY PR.
//
// These tests are the executable specification for the fix:
//   - pin the production banner as NON-parseable (the caller must NOT read it);
//   - prove that extracting the JSON-envelope final step output and running
//     `parse_judge_response` (JSON) with a `parse_merge_verdict_from_text`
//     (keyword) fallback recovers the real verdict;
//   - pin fail-closed semantics: empty/malformed agent output yields an error
//     from BOTH parsers, so `judge` must map it to `Verdict::Unclear`
//     (fail closed) — never a SUCCESS-without-verdict.
//
// The envelope-extraction contract is mirrored inline (the production helper
// `extract_recipe_decision_output` lives in `ooda_brain` and is being lifted
// into a shared `brain_ladder` module by the implementation step). The unit
// under test here is the parse composition the judge must wire in; the live
// `recipe-runner-rs --output-format json` boundary is covered by the
// `tests/gadugi/merge-judge-verdict.sh` outside-in scenario.
// =====================================================================
#[cfg(test)]
mod issue_2428_tests {
    use super::super::merge_judge::{Verdict, parse_judge_response};
    use super::parse_merge_verdict_from_text;

    /// The EXACT text-mode banner the operators reported (#2462 / #2463 /
    /// #2435). It is the only thing on `recipe-runner-rs` stdout in `text`
    /// mode and contains no verdict keyword.
    const PRODUCTION_BANNER: &str = "Recipe: merge-readiness-judge (v1.0.0)\nSteps: 1\n\nRecipe 'merge-readiness-judge': SUCCESS (32.0s)\n  [completed] judge-merge-readiness (32.0s)\n\n";

    /// Mirror of the production `extract_recipe_decision_output` contract:
    /// decode the `recipe-runner-rs --output-format json` envelope and return
    /// the FINAL step's `output`. `None` when the bytes are not a valid
    /// envelope (e.g. the text-mode banner) or the envelope carries no step.
    fn extract_final_step_output(envelope_json: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(envelope_json).ok()?;
        if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
            return None;
        }
        let steps = v.get("step_results")?.as_array()?;
        let last = steps.last()?;
        Some(last.get("output")?.as_str()?.to_string())
    }

    // --- (1) regression pin: the banner is NOT a parseable verdict ----------

    #[test]
    fn production_banner_has_no_verdict_keyword() {
        // This is the #2462/#2463 failure reproduced at the parse layer:
        // "merge-readiness-judge" contains "readiness" (NOT "ready"), so no
        // verdict keyword matches and the judge errors → merge blocked.
        let err = parse_merge_verdict_from_text(PRODUCTION_BANNER).unwrap_err();
        assert!(
            err.contains("no verdict keyword"),
            "the banner must be unparseable so the fix is forced to read the \
             JSON envelope instead; got: {err}"
        );
    }

    #[test]
    fn text_banner_is_not_a_valid_json_envelope() {
        // Proves the judge MUST pass `--output-format json`: the text banner
        // can never be mistaken for a decodable envelope, so a missing
        // `--output-format json` flag fails loudly rather than silently.
        assert!(
            extract_final_step_output(PRODUCTION_BANNER).is_none(),
            "the text-mode banner is not a JSON envelope"
        );
    }

    // --- (2) the fix: JSON envelope final step output carries the verdict ---

    #[test]
    fn json_envelope_fenced_verdict_parses_ready() {
        // The shape #2463 explicitly asks for: a json envelope whose FINAL
        // step output is a fenced ```json {"verdict":"ready",...}``` block.
        let envelope = r#"{
            "recipe_name": "merge-readiness-judge",
            "success": true,
            "step_results": [
                {"step_id": "judge-merge-readiness",
                 "output": "Here is my assessment:\n```json\n{\"verdict\": \"ready\", \"rationale\": \"All six skill criteria present and substantive.\"}\n```\n",
                 "error": "", "duration": 32.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope)
            .expect("envelope final step output must be extractable");
        let out = parse_judge_response(&inner).expect("fenced JSON verdict must parse");
        assert_eq!(out.verdict, Verdict::Ready);
        assert!(out.rationale.contains("six skill criteria"));
    }

    #[test]
    fn json_envelope_bare_json_not_ready_preserves_blockers() {
        let envelope = r#"{
            "success": true,
            "step_results": [
                {"step_id": "judge-merge-readiness",
                 "output": "{\"verdict\": \"not_ready\", \"rationale\": \"Quality-audit thin.\", \"blockers\": [{\"section\": \"Quality-audit\", \"severity\": \"high\", \"observation\": \"No cycles.\", \"fix\": \"Run three cycles.\"}]}",
                 "error": "", "duration": 30.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope).unwrap();
        let out = parse_judge_response(&inner).expect("bare JSON verdict must parse");
        assert_eq!(out.verdict, Verdict::NotReady);
        assert_eq!(
            out.blockers.len(),
            1,
            "blockers must survive extraction+parse"
        );
        assert_eq!(out.blockers[0].section, "Quality-audit");
    }

    #[test]
    fn json_envelope_unclear_verdict_parses() {
        let envelope = r#"{
            "success": true,
            "step_results": [
                {"step_id": "judge-merge-readiness",
                 "output": "{\"verdict\": \"unclear\", \"rationale\": \"PR body appears truncated.\"}",
                 "error": "", "duration": 12.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope).unwrap();
        let out = parse_judge_response(&inner).unwrap();
        assert_eq!(out.verdict, Verdict::Unclear);
    }

    #[test]
    fn json_envelope_prose_falls_back_to_keyword_verdict() {
        // If the agent emits PROSE (no JSON object) inside the envelope, the
        // strict JSON parser fails but the keyword fallback
        // (`parse_merge_verdict_from_text`) must still surface a real verdict.
        let envelope = r#"{
            "success": true,
            "step_results": [
                {"step_id": "judge-merge-readiness",
                 "output": "After reviewing all six sections I find this PR ready to merge.",
                 "error": "", "duration": 28.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope).unwrap();
        assert!(
            parse_judge_response(&inner).is_err(),
            "prose has no JSON object; strict JSON parse must fail first"
        );
        let out = parse_merge_verdict_from_text(&inner)
            .expect("keyword fallback must surface a verdict from prose");
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn json_envelope_uses_final_step_output() {
        // Multi-step recipe: the verdict is the TERMINAL step's output, not an
        // earlier prelude step.
        let envelope = r#"{
            "success": true,
            "step_results": [
                {"step_id": "prep", "output": "gathering context", "error": "", "duration": 1.0},
                {"step_id": "judge-merge-readiness",
                 "output": "{\"verdict\": \"ready\", \"rationale\": \"ok\"}",
                 "error": "", "duration": 20.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope).unwrap();
        let out = parse_judge_response(&inner).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    // --- (3) fail-closed pin: empty/malformed output is NEVER a SUCCESS -----

    #[test]
    fn empty_final_step_output_fails_closed() {
        // When the agent step produces empty output, BOTH the JSON parser and
        // the keyword parser must error. The judge therefore CANNOT return a
        // verdict and MUST fail closed to `Verdict::Unclear` (acceptance:
        // "never SUCCESS-without-verdict"). This test pins the precondition;
        // the gadugi scenario pins the end-to-end fail-closed behaviour.
        let envelope = r#"{
            "success": true,
            "step_results": [
                {"step_id": "judge-merge-readiness", "output": "", "error": "", "duration": 5.0}
            ]
        }"#;
        let inner = extract_final_step_output(envelope).unwrap();
        assert!(inner.trim().is_empty());
        assert!(
            parse_judge_response(&inner).is_err(),
            "empty output must not parse as a JSON verdict"
        );
        assert!(
            parse_merge_verdict_from_text(&inner).is_err(),
            "empty output must not parse as a keyword verdict — judge must \
             fail closed to Unclear, not SUCCESS"
        );
    }

    #[test]
    fn success_false_envelope_is_not_extractable() {
        // A `success:false` envelope means the recipe itself failed — the
        // judge must surface that loudly (Err), not mine a verdict out of it.
        let envelope = r#"{"success": false, "step_results": []}"#;
        assert!(
            extract_final_step_output(envelope).is_none(),
            "success=false must not yield a verdict"
        );
    }
}

// =====================================================================
// issue_2428_production_tests — the production parse/fail-closed wiring
// (#2428 / #2430 / #2435 / #2462 / #2463 / #2429).
//
// `issue_2428_tests` above pins the parse composition with an inline envelope
// helper. These pin the production [`parse_merge_outcome`] + fail-closed
// contract the `judge()` method actually wires in: the agent's extracted output
// is mapped to `(JudgeOutcome, LifecycleParseOutcome)`, and an unparseable
// (but successful) run fails CLOSED to `Verdict::Unclear` with a
// `DefaultEmpty`/`DefaultMalformed` outcome — never a `ready`-without-verdict.
// =====================================================================
#[cfg(test)]
mod issue_2428_production_tests {
    use super::*;
    use crate::ooda_brain::LifecycleParseOutcome;

    #[test]
    fn json_verdict_parses_as_parsed() {
        let (out, oc) = parse_merge_outcome(
            "```json\n{\"verdict\": \"ready\", \"rationale\": \"all six sections present\"}\n```",
        );
        assert_eq!(out.verdict, Verdict::Ready);
        assert_eq!(oc, LifecycleParseOutcome::Parsed);
        assert!(!oc.is_parse_failure());
    }

    #[test]
    fn prose_keyword_parses_as_parsed() {
        // No JSON object — the keyword fallback still surfaces a real verdict.
        let (out, oc) = parse_merge_outcome("After review I find this PR ready to merge.");
        assert_eq!(out.verdict, Verdict::Ready);
        assert_eq!(oc, LifecycleParseOutcome::Parsed);
    }

    #[test]
    fn not_ready_json_preserves_blockers_and_parses() {
        let (out, oc) = parse_merge_outcome(
            "{\"verdict\": \"not_ready\", \"rationale\": \"thin\", \"blockers\": [{\"section\": \"Quality-audit\", \"severity\": \"high\", \"observation\": \"none\", \"fix\": \"add cycles\"}]}",
        );
        assert_eq!(out.verdict, Verdict::NotReady);
        assert_eq!(out.blockers.len(), 1);
        assert_eq!(oc, LifecycleParseOutcome::Parsed);
    }

    #[test]
    fn empty_output_fails_closed_to_unclear_default_empty() {
        let (out, oc) = parse_merge_outcome("   ");
        assert_eq!(
            out.verdict,
            Verdict::Unclear,
            "empty output must fail closed to Unclear, never SUCCESS-without-verdict"
        );
        assert_eq!(oc, LifecycleParseOutcome::DefaultEmpty);
        assert!(oc.is_parse_failure());
        assert!(out.rationale.contains("failing closed"));
    }

    #[test]
    fn unparseable_prose_fails_closed_to_unclear_default_malformed() {
        // Prose with NO verdict keyword and no JSON: must fail closed, not error,
        // and must NOT be mistaken for a real verdict.
        let (out, oc) = parse_merge_outcome("The PR looks interesting but I cannot decide.");
        assert_eq!(out.verdict, Verdict::Unclear);
        assert_eq!(oc, LifecycleParseOutcome::DefaultMalformed);
        assert!(oc.is_parse_failure());
    }

    #[test]
    fn production_banner_fails_closed_not_parsed() {
        // The reported #2462/#2463 banner: only `readiness` (not `ready`), no
        // JSON. If it ever reached the parser it must fail closed, never SUCCESS.
        let banner = "Recipe: merge-readiness-judge (v1.0.0)\nSteps: 1\n\nRecipe 'merge-readiness-judge': SUCCESS (32.0s)\n  [completed] judge-merge-readiness (32.0s)\n\n";
        let (out, oc) = parse_merge_outcome(banner);
        assert_eq!(out.verdict, Verdict::Unclear);
        assert!(oc.is_parse_failure());
    }

    #[test]
    fn verdict_label_covers_all_variants() {
        assert_eq!(verdict_label(&Verdict::Ready), "ready");
        assert_eq!(verdict_label(&Verdict::NotReady), "not_ready");
        assert_eq!(verdict_label(&Verdict::Unclear), "unclear");
    }

    #[test]
    fn merge_escalation_note_empty_on_base_and_demands_json_on_repair() {
        assert_eq!(build_merge_escalation_note(LadderRung::Base, "x"), "");
        let n = build_merge_escalation_note(LadderRung::SchemaRepair, "Recipe: banner");
        assert!(n.contains("SCHEMA REPAIR"), "note: {n}");
        assert!(
            n.contains("\"verdict\""),
            "repair note must demand the verdict JSON"
        );
        assert!(
            n.contains("Recipe: banner"),
            "note must feed prior output back"
        );
    }
}

// =====================================================================
// issue_2501_2555_real_envelope_tests — the REAL recipe-runner wire format.
//
// #2501 and #2555 are duplicates of the #2428/#2496 verdict-capture family:
// `simard merge-pr` aborted with
//   `no verdict keyword (ready/not_ready/unclear) found in recipe output;
//    raw="Recipe: merge-readiness-judge … SUCCESS …"`
// The root cause (agent-step stdout not surfaced to the verdict extractor —
// only the text-mode SUCCESS banner) was fixed by the `--output-format json`
// transport (#2486) and the shared launcher-preamble strip (#2496/#2504); the
// residual sightings were a STALE deployed binary.
//
// The modules above pin the parse composition with an INLINE envelope mirror
// (`extract_final_step_output`) and synthetic clean step outputs. This module
// closes the remaining gap: it drives the PRODUCTION
// `extract_recipe_decision_output` on the EXACT `recipe-runner-rs 0.3.6
// --output-format json --agent-binary copilot` envelope — where the GitHub
// Copilot CLI launcher preamble line (`ℹ … NODE_OPTIONS=… (saved preference)…`)
// is PREPENDED to the verdict inside the agent step's `output` — and then runs
// the production `parse_merge_outcome`, so the whole capture → strip → parse
// path is validated end-to-end against the real wire format that produced the
// bug reports. Captured live: see `tests/gadugi/merge-judge-verdict.sh` step (d).
// =====================================================================
#[cfg(test)]
mod issue_2501_2555_real_envelope_tests {
    use super::*;
    use crate::ooda_brain::LifecycleParseOutcome;

    /// The GitHub Copilot CLI launcher preamble line the agent binary prints to
    /// stdout *before* its real answer. Captured verbatim from a live
    /// `recipe-runner-rs --output-format json --agent-binary copilot` run; it
    /// lands inside the agent step's `output` field, ahead of the verdict.
    const LAUNCHER_PREAMBLE: &str = "ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). \
         To change: /home/azureuser/.amplihack/config";

    /// Build a real `recipe-runner-rs 0.3.6` JSON envelope whose single agent
    /// step's `output` is `agent_stdout` (the launcher preamble + whatever the
    /// judge emitted). Mirrors the exact shape observed live.
    fn real_envelope(agent_stdout: &str) -> Vec<u8> {
        let envelope = serde_json::json!({
            "recipe_name": "merge-readiness-judge",
            "success": true,
            "step_results": [{
                "step_id": "judge-merge-readiness",
                "status": "completed",
                "output": agent_stdout,
                "error": "",
                "duration": 32.0
            }],
            "context": { "judge_result": agent_stdout },
            "duration": 32.0
        });
        serde_json::to_vec(&envelope).expect("envelope serialises")
    }

    #[test]
    fn real_copilot_envelope_recovers_ready_verdict() {
        // The exact #2501/#2555 scenario, now proven to recover a verdict: the
        // launcher preamble is prepended to a bare `{"verdict":"ready",…}` JSON
        // inside the agent step output. The production extractor must surface
        // that output and `parse_merge_outcome` must recover a REAL `ready`
        // (Parsed) — NOT the fail-closed `unclear`, and NEVER the old
        // "no verdict keyword" abort.
        let agent_stdout = format!(
            "{LAUNCHER_PREAMBLE}\n\
             {{\"verdict\": \"ready\", \"rationale\": \"All six skill criteria present and substantive.\"}}"
        );
        let stdout = real_envelope(&agent_stdout);

        let extracted = extract_recipe_decision_output(&stdout, ADAPTER_TAG)
            .expect("production extractor must decode the real 0.3.6 envelope");
        assert!(
            extracted.contains("\"verdict\""),
            "extracted agent output must carry the verdict JSON, not the banner: {extracted:?}"
        );

        let (outcome, parse) = parse_merge_outcome(&extracted);
        assert_eq!(
            outcome.verdict,
            Verdict::Ready,
            "the preamble-prefixed verdict JSON must parse as ready"
        );
        assert_eq!(parse, LifecycleParseOutcome::Parsed);
        assert!(
            !parse.is_parse_failure(),
            "a real verdict was recovered — this must NOT count as a parse failure"
        );
    }

    #[test]
    fn real_copilot_envelope_recovers_not_ready_with_blockers() {
        // Same wire format, `not_ready` with a structured blocker: the blocker
        // must survive extraction + parse (operators act on it).
        let agent_stdout = format!(
            "{LAUNCHER_PREAMBLE}\n\
             {{\"verdict\": \"not_ready\", \"rationale\": \"Quality-audit section is one line.\", \
             \"blockers\": [{{\"section\": \"Quality-audit\", \"severity\": \"high\", \
             \"observation\": \"No SEEK/VALIDATE/FIX cycle counts.\", \"fix\": \"Run three cycles.\"}}]}}"
        );
        let stdout = real_envelope(&agent_stdout);

        let extracted = extract_recipe_decision_output(&stdout, ADAPTER_TAG).unwrap();
        let (outcome, parse) = parse_merge_outcome(&extracted);
        assert_eq!(outcome.verdict, Verdict::NotReady);
        assert_eq!(
            outcome.blockers.len(),
            1,
            "blocker must survive the real wire format"
        );
        assert_eq!(outcome.blockers[0].section, "Quality-audit");
        assert_eq!(parse, LifecycleParseOutcome::Parsed);
    }

    #[test]
    fn real_copilot_envelope_prose_verdict_falls_back_to_keyword() {
        // The judge sometimes answers in prose. The launcher preamble must be
        // stripped (it is not a verdict) and the keyword fallback must surface
        // the real `ready` — proving the preamble never masks a prose verdict.
        let agent_stdout = format!(
            "{LAUNCHER_PREAMBLE}\n\
             After reviewing all six sections I find this PR ready to merge."
        );
        let stdout = real_envelope(&agent_stdout);

        let extracted = extract_recipe_decision_output(&stdout, ADAPTER_TAG).unwrap();
        let (outcome, parse) = parse_merge_outcome(&extracted);
        assert_eq!(outcome.verdict, Verdict::Ready);
        assert_eq!(parse, LifecycleParseOutcome::Parsed);
        assert!(
            !outcome.rationale.contains("NODE_OPTIONS"),
            "the launcher preamble must be stripped from the rationale: {}",
            outcome.rationale
        );
    }

    #[test]
    fn real_copilot_envelope_preamble_only_fails_closed_never_ready() {
        // Defence-in-depth: if the agent emits ONLY the launcher preamble (no
        // verdict at all), the preamble must NOT be mined as a `ready` — the
        // judge must fail CLOSED to `unclear` (a parse failure), so a stripped
        // banner can never fabricate a merge authorisation.
        let stdout = real_envelope(LAUNCHER_PREAMBLE);

        let extracted = extract_recipe_decision_output(&stdout, ADAPTER_TAG).unwrap();
        let (outcome, parse) = parse_merge_outcome(&extracted);
        assert_eq!(
            outcome.verdict,
            Verdict::Unclear,
            "preamble-only output must fail closed to unclear, never a false ready"
        );
        assert!(
            parse.is_parse_failure(),
            "preamble-only is a parse-miss (fail-closed), not a real verdict"
        );
        assert!(
            outcome.rationale.contains("failing closed"),
            "fail-closed rationale must name the miss: {}",
            outcome.rationale
        );
    }

    #[test]
    fn text_mode_banner_bytes_are_a_loud_infra_error_not_a_silent_default() {
        // If a STALE runner (or a missing `--output-format json`) ever feeds the
        // text-mode SUCCESS banner in as the envelope bytes, the production
        // extractor must fail LOUDLY (the banner is not a JSON envelope) rather
        // than silently returning it for the keyword scanner — which is exactly
        // how the original `no verdict keyword; raw="Recipe: … SUCCESS"` abort
        // arose. This keeps a stale-binary environment diagnosable as infra.
        let banner = b"Recipe: merge-readiness-judge (v1.0.0)\nSteps: 1\n\n\
                       Recipe 'merge-readiness-judge': SUCCESS (32.0s)\n  \
                       [completed] judge-merge-readiness (32.0s)\n\n";
        let err = extract_recipe_decision_output(banner, ADAPTER_TAG)
            .expect_err("the text-mode banner is not a decodable JSON envelope");
        let msg = err.to_string();
        assert!(
            msg.contains("deserialize") || msg.contains("recipe-merge-judge"),
            "a stale-runner text banner must surface as a loud decode error: {msg}"
        );
    }
}

/// Issue #2570: `is_copilot_launcher_line` / `strip_recipe_noise` is a shared
/// chokepoint. The distillation fact-yield fix exempts JSON structural-token
/// lines (`{`, `"`, `[`) from launcher-noise classification; this merge-judge
/// consumer must keep stripping the REAL launcher preamble (which never begins
/// with a JSON token) so a noise-obscured verdict is still read. This is the
/// merge-judge slice of the cross-consumer regression coverage.
#[cfg(test)]
mod issue_2570_cross_consumer_tests {
    use super::*;
    use crate::ooda_brain::LifecycleParseOutcome;

    fn launcher_preamble() -> String {
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: cfg\n\
         INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
         Run 'copilot update' to check for updates.\n"
            .to_string()
    }

    #[test]
    fn merge_judge_still_strips_real_launcher_preamble() {
        let raw = format!(
            "{}not_ready the quality audit is incomplete",
            launcher_preamble()
        );
        let (out, outcome) = parse_merge_outcome(&raw);
        assert_eq!(
            outcome,
            LifecycleParseOutcome::Parsed,
            "merge-judge must still strip the launcher preamble after the #2570 guard"
        );
        assert_eq!(
            out.verdict,
            Verdict::NotReady,
            "the model's real verdict must be read, not launcher noise"
        );
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
