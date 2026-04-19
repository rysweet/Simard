//! LLM-driven multi-step planning for the engineer loop.
//!
//! Provides [`Plan`] and [`PlanStep`] types, [`plan_objective`] (LLM-based
//! planning), and [`execute_plan`] (sequential execution with verification).

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeTurnInput;
use crate::engineer_loop::{AnalyzedAction, RepoInspection};
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::SessionBuilder;

/// A single step in an LLM-generated plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub action: AnalyzedAction,
    pub target: String,
    pub expected_outcome: String,
    pub verification_command: String,
}

/// An ordered sequence of [`PlanStep`]s produced by [`plan_objective`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    steps: Vec<PlanStep>,
}

impl Plan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps }
    }
    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }
    pub fn len(&self) -> usize {
        self.steps.len()
    }
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Outcome of a single plan step's verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStepResult {
    pub step: PlanStep,
    pub passed: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Result of executing an entire plan via [`execute_plan`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanExecutionResult {
    pub completed: Vec<PlanStepResult>,
    pub stopped_early: bool,
}

const PLANNING_INSTRUCTIONS: &str = include_str!("../prompt_assets/simard/engineer_planning.md");

fn build_planning_prompt(objective: &str, inspection: &RepoInspection) -> String {
    let files = if inspection.changed_files.is_empty() {
        "none".to_string()
    } else {
        inspection.changed_files.join(", ")
    };
    let dirty = if inspection.worktree_dirty {
        "dirty"
    } else {
        "clean"
    };
    let goals: Vec<&str> = inspection
        .active_goals
        .iter()
        .map(|g| g.title.as_str())
        .collect();
    let goals_list = if goals.is_empty() {
        "none".to_string()
    } else {
        goals.join("; ")
    };

    format!(
        "{}\n\nObjective: {objective}\nBranch: {branch}\nWorktree: {dirty}\n\
         Changed files: {files}\nActive goals: {goals_list}",
        PLANNING_INSTRUCTIONS.trim(),
        objective = objective,
        branch = inspection.branch,
    )
}

/// Attempt to locate a JSON array within a mixed text/JSON response.
///
/// LLMs sometimes wrap valid JSON in markdown fences, prose preambles, or
/// trailing commentary.  This function tries progressively looser extraction
/// strategies:
///   1. Direct parse of the full (trimmed) text.
///   2. Strip markdown ```json … ``` fences.
///   3. Find the first `[` … last `]` substring and parse that.
///
/// Returns `Err(PlanningUnavailable)` only when none of these strategies
/// produce a valid `Vec<PlanStep>`.
fn parse_plan_response(text: &str) -> SimardResult<Plan> {
    let trimmed = text.trim();
    let skipped = skip_preamble(trimmed);

    // Strategy 1: direct parse (after skipping any non-JSON preamble such as
    // the Copilot SDK adapter dispatch metadata line — see issue #944).
    if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(skipped) {
        return Ok(Plan::new(steps));
    }

    // Strategy 2: strip markdown fences. Re-apply preamble skipping after
    // defencing to handle preamble-then-fence sequences like
    // "Here is the plan:\n```json\n[...]\n```".
    let defenced = strip_markdown_fences(skipped);
    if defenced != skipped {
        let defenced_skipped = skip_preamble(defenced);
        if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(defenced_skipped) {
            return Ok(Plan::new(steps));
        }
    }

    // Strategy 3: locate outermost JSON array brackets in the preamble-skipped
    // slice. This also recovers responses where an earlier `{` in the preamble
    // would otherwise misanchor strategies 1 and 2.
    if let Some(json_text) = extract_json_array(skipped) {
        if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(json_text) {
            tracing::info!(
                "recovered JSON plan from mixed LLM response ({} bytes of surrounding text stripped)",
                trimmed.len() - json_text.len()
            );
            return Ok(Plan::new(steps));
        }
    }

    Err(SimardError::PlanningUnavailable {
        reason: format!(
            "failed to parse LLM plan response after trying direct, fenced, and bracket-extraction strategies. \
             Response begins with: {:?}",
            &trimmed[..trimmed.len().min(120)]
        ),
    })
}

/// Skip any non-JSON preamble at the start of `s` by advancing to the
/// earliest `[` or `{`. Returns the original slice when neither delimiter
/// is present. Used to tolerate adapter dispatch lines (e.g. the Copilot SDK
/// "Copilot SDK adapter dispatched objective-metadata via …" prefix from
/// issue #944) before JSON plan payloads.
fn skip_preamble(s: &str) -> &str {
    let bracket = s.find('[');
    let brace = s.find('{');
    let pos = match (bracket, brace) {
        (Some(b), Some(c)) => b.min(c),
        (Some(b), None) => b,
        (None, Some(c)) => c,
        (None, None) => return s,
    };
    &s[pos..]
}

/// Remove markdown code fences from text. Handles ```json and bare ```.
fn strip_markdown_fences(text: &str) -> &str {
    let inner = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text);
    inner.strip_suffix("```").unwrap_or(inner).trim()
}

/// Find the first `[` and last `]` in the text and return the substring.
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

/// Ask the LLM for a multi-step plan to accomplish `objective`.
///
/// Returns `Err(PlanningUnavailable)` when no LLM session can be opened or the
/// response is unparseable.  Callers use keyword-based `analyze_objective`.
pub fn plan_objective(objective: &str, inspection: &RepoInspection) -> SimardResult<Plan> {
    let mut session = SessionBuilder::new(OperatingMode::Engineer)
        .node_id("engineer-planner")
        .address("engineer-planner://local")
        .adapter_tag("engineer-planner-rustyclawd")
        .open()
        .map_err(|reason| SimardError::PlanningUnavailable { reason })?;

    let prompt = build_planning_prompt(objective, inspection);
    let outcome = session
        .run_turn(BaseTypeTurnInput::objective_only(prompt))
        .map_err(|e| SimardError::PlanningUnavailable {
            reason: format!("LLM turn failed: {e}"),
        })?;
    let _ = session.close();
    parse_plan_response(&outcome.plan)
}

/// Execute a plan sequentially, running each step's verification command.
/// Stops on the first verification failure and returns partial results.
pub fn execute_plan(plan: &Plan, repo_root: &Path) -> PlanExecutionResult {
    let mut completed = Vec::with_capacity(plan.len());
    let mut stopped_early = false;

    for step in plan.steps() {
        if step.verification_command.is_empty() {
            completed.push(PlanStepResult {
                step: step.clone(),
                passed: true,
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
            continue;
        }
        match Command::new("sh")
            .arg("-c")
            .arg(&step.verification_command)
            .current_dir(repo_root)
            .output()
        {
            Ok(out) => {
                let exit_code = out.status.code().unwrap_or(-1);
                let passed = exit_code == 0;
                completed.push(PlanStepResult {
                    step: step.clone(),
                    passed,
                    exit_code,
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                });
                if !passed {
                    stopped_early = true;
                    break;
                }
            }
            Err(e) => {
                completed.push(PlanStepResult {
                    step: step.clone(),
                    passed: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("failed to run verification command: {e}"),
                });
                stopped_early = true;
                break;
            }
        }
    }
    PlanExecutionResult {
        completed,
        stopped_early,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::{SessionId, SessionPhase};
    use std::path::PathBuf;

    fn test_inspection() -> RepoInspection {
        RepoInspection {
            workspace_root: PathBuf::from("/tmp/test-ws"),
            repo_root: PathBuf::from("/tmp/test-repo"),
            branch: "main".to_string(),
            head: "abc1234".to_string(),
            worktree_dirty: false,
            changed_files: vec!["src/lib.rs".to_string()],
            active_goals: vec![GoalRecord {
                slug: "g".to_string(),
                title: "Finish planning".to_string(),
                rationale: "needed".to_string(),
                status: GoalStatus::Active,
                priority: 1,
                owner_identity: "test".to_string(),
                source_session_id: SessionId::from_uuid(uuid::Uuid::nil()),
                updated_in: SessionPhase::Execution,
            }],
            carried_meeting_decisions: Vec::new(),
            architecture_gap_summary: String::new(),
        }
    }

    fn step(action: AnalyzedAction, cmd: &str) -> PlanStep {
        PlanStep {
            action,
            target: ".".into(),
            expected_outcome: "ok".into(),
            verification_command: cmd.into(),
        }
    }

    #[test]
    fn plan_step_serialization_round_trip() {
        let s = step(AnalyzedAction::CreateFile, "test -f src/new.rs");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(s, serde_json::from_str::<PlanStep>(&json).unwrap());
    }

    #[test]
    fn plan_serialization_round_trip() {
        let plan = Plan::new(vec![
            step(AnalyzedAction::CreateFile, "test -f src/a.rs"),
            step(AnalyzedAction::CargoTest, "cargo test"),
        ]);
        let json = serde_json::to_string(&plan).unwrap();
        assert_eq!(plan, serde_json::from_str::<Plan>(&json).unwrap());
    }

    #[test]
    fn plan_convenience_methods() {
        assert!(Plan::new(Vec::new()).is_empty());
        let plan = Plan::new(vec![step(AnalyzedAction::ReadOnlyScan, "")]);
        assert_eq!(plan.len(), 1);
        assert!(!plan.is_empty());
    }

    #[test]
    fn plan_objective_without_api_key_returns_unavailable() {
        // Force RustyClawd provider without ANTHROPIC_API_KEY → session may open
        // but run_turn will fail.
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::set_var("SIMARD_LLM_PROVIDER", "rustyclawd");
        };
        let result = plan_objective("create a new module", &test_inspection());
        unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };
        match result {
            Err(SimardError::PlanningUnavailable { .. }) => {
                // Any PlanningUnavailable is correct — whether from open() or run_turn().
            }
            other => panic!("expected PlanningUnavailable, got: {other:?}"),
        }
    }

    #[test]
    fn uses_keyword_analysis_when_planning_unavailable() {
        unsafe {
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::set_var("SIMARD_LLM_PROVIDER", "rustyclawd");
        };
        assert!(plan_objective("create a new file", &test_inspection()).is_err());
        unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };
        assert_eq!(
            crate::engineer_loop::analyze_objective("create a new file at src/hello.rs"),
            AnalyzedAction::CreateFile,
        );
    }

    #[test]
    fn parse_plan_response_valid_json() {
        let json = r#"[{"action":"create_file","target":"src/plan.rs","expected_outcome":"exists","verification_command":"test -f src/plan.rs"},{"action":"cargo_test","target":"all","expected_outcome":"pass","verification_command":"cargo test"}]"#;
        let plan = parse_plan_response(json).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.steps()[0].action, AnalyzedAction::CreateFile);
        assert_eq!(plan.steps()[1].action, AnalyzedAction::CargoTest);
    }

    #[test]
    fn parse_plan_response_with_markdown_fences() {
        let json = "```json\n[{\"action\":\"read_only_scan\",\"target\":\".\",\"expected_outcome\":\"ok\",\"verification_command\":\"ls\"}]\n```";
        let plan = parse_plan_response(json).unwrap();
        assert_eq!(plan.steps()[0].action, AnalyzedAction::ReadOnlyScan);
    }

    #[test]
    fn parse_plan_response_invalid_json() {
        match parse_plan_response("not json at all").unwrap_err() {
            SimardError::PlanningUnavailable { reason } => {
                assert!(reason.contains("failed to parse"))
            }
            other => panic!("expected PlanningUnavailable, got: {other}"),
        }
    }

    #[test]
    fn parse_plan_response_json_with_prose_preamble() {
        let mixed = r#"Here is my plan for you:

[{"action":"cargo_test","target":".","expected_outcome":"pass","verification_command":"cargo test"}]

I hope this helps!"#;
        let plan = parse_plan_response(mixed).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.steps()[0].action, AnalyzedAction::CargoTest);
    }

    #[test]
    fn parse_plan_response_json_with_trailing_explanation() {
        let mixed = r#"[{"action":"read_only_scan","target":".","expected_outcome":"ok","verification_command":"ls"}]
This plan inspects the repository without making changes."#;
        let plan = parse_plan_response(mixed).unwrap();
        assert_eq!(plan.steps()[0].action, AnalyzedAction::ReadOnlyScan);
    }

    #[test]
    fn parse_plan_response_completely_non_json() {
        let prose = "I think you should run cargo test and then check the results manually.";
        assert!(parse_plan_response(prose).is_err());
    }

    #[test]
    fn parse_plan_response_error_includes_response_preview() {
        let bad = "This is not valid JSON and contains no brackets";
        match parse_plan_response(bad).unwrap_err() {
            SimardError::PlanningUnavailable { reason } => {
                assert!(reason.contains("bracket-extraction"));
                assert!(reason.contains("Response begins with"));
            }
            other => panic!("expected PlanningUnavailable, got: {other}"),
        }
    }

    #[test]
    fn strip_markdown_fences_with_json_tag() {
        let input = "```json\n{\"key\": 1}\n```";
        assert_eq!(strip_markdown_fences(input), "{\"key\": 1}");
    }

    #[test]
    fn strip_markdown_fences_bare() {
        let input = "```\ncontent\n```";
        assert_eq!(strip_markdown_fences(input), "content");
    }

    #[test]
    fn strip_markdown_fences_no_fences() {
        assert_eq!(strip_markdown_fences("plain text"), "plain text");
    }

    #[test]
    fn extract_json_array_from_mixed_text() {
        let text = "Here is the plan: [{\"a\":1}] end.";
        assert_eq!(extract_json_array(text), Some("[{\"a\":1}]"));
    }

    #[test]
    fn extract_json_array_no_brackets() {
        assert_eq!(extract_json_array("no brackets here"), None);
    }

    #[test]
    fn extract_json_array_reversed_brackets() {
        assert_eq!(extract_json_array("]before["), None);
    }

    #[test]
    fn parse_plan_response_empty_array() {
        assert!(parse_plan_response("[]").unwrap().is_empty());
    }

    #[test]
    fn build_planning_prompt_contains_context() {
        let prompt = build_planning_prompt("fix the bug", &test_inspection());
        assert!(prompt.contains("fix the bug"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("Finish planning"));
        assert!(prompt.contains("clean"));
    }

    #[test]
    fn build_planning_prompt_dirty_and_empty() {
        let mut insp = test_inspection();
        insp.worktree_dirty = true;
        insp.changed_files.clear();
        insp.active_goals.clear();
        let prompt = build_planning_prompt("t", &insp);
        assert!(prompt.contains("dirty"));
        assert!(prompt.contains("Changed files: none"));
        assert!(prompt.contains("Active goals: none"));
    }

    #[test]
    fn execute_plan_passes_on_true_command() {
        let plan = Plan::new(vec![step(AnalyzedAction::ReadOnlyScan, "true")]);
        let result = execute_plan(&plan, Path::new("/tmp"));
        assert!(!result.stopped_early);
        assert!(result.completed[0].passed);
    }

    #[test]
    fn execute_plan_stops_on_failure() {
        let plan = Plan::new(vec![
            step(AnalyzedAction::ReadOnlyScan, "true"),
            step(AnalyzedAction::RunShellCommand, "false"),
            step(AnalyzedAction::CargoTest, "true"),
        ]);
        let result = execute_plan(&plan, Path::new("/tmp"));
        assert!(result.stopped_early);
        assert_eq!(result.completed.len(), 2);
        assert!(result.completed[0].passed);
        assert!(!result.completed[1].passed);
    }

    #[test]
    fn execute_plan_skips_empty_verification_and_empty_plan() {
        let plan = Plan::new(vec![step(AnalyzedAction::GitCommit, "")]);
        let r = execute_plan(&plan, Path::new("/tmp"));
        assert!(r.completed[0].passed);

        let r2 = execute_plan(&Plan::new(Vec::new()), Path::new("/tmp"));
        assert!(!r2.stopped_early);
        assert!(r2.completed.is_empty());
    }

    // ---------------------------------------------------------------------
    // Issue #944: LLM plan parser must skip preamble (e.g. Copilot SDK
    // adapter dispatch metadata) before each parsing strategy.
    // ---------------------------------------------------------------------

    #[test]
    fn parse_plan_response_skips_copilot_sdk_preamble() {
        let raw = "Copilot SDK adapter dispatched objective-metadata via \
                   'gh-copilot' on 'gpt-5' (turn 3).\n\
                   [{\"action\":\"read_only_scan\",\"target\":\"logs\",\
                   \"expected_outcome\":\"checked\",\
                   \"verification_command\":\"ls\"}]";
        let plan = parse_plan_response(raw).expect("preamble must be skipped");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.steps()[0].action, AnalyzedAction::ReadOnlyScan);
        assert_eq!(plan.steps()[0].target, "logs");
    }

    #[test]
    fn parse_plan_response_skips_preamble_with_fenced_json() {
        let raw = "Copilot SDK adapter dispatched objective-metadata via \
                   'gh-copilot' on 'gpt-5' (turn 3).\n\
                   ```json\n\
                   [{\"action\":\"cargo_test\",\"target\":\".\",\
                   \"expected_outcome\":\"pass\",\
                   \"verification_command\":\"cargo test\"}]\n\
                   ```";
        let plan =
            parse_plan_response(raw).expect("preamble + fenced JSON must parse");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.steps()[0].action, AnalyzedAction::CargoTest);
    }

    #[test]
    fn skip_preamble_is_noop_on_clean_json_array() {
        let s = "[{\"a\":1}]";
        assert_eq!(skip_preamble(s), s);
    }

    #[test]
    fn skip_preamble_is_noop_on_clean_json_object() {
        let s = "{\"a\":1}";
        assert_eq!(skip_preamble(s), s);
    }

    #[test]
    fn skip_preamble_returns_original_when_no_brackets() {
        let s = "no json delimiters here at all";
        assert_eq!(skip_preamble(s), s);
    }

    #[test]
    fn skip_preamble_finds_earliest_of_array_or_object() {
        // Array delimiter appears first → return slice from '['.
        assert_eq!(
            skip_preamble("preamble [1,2,3] then {x:1}"),
            "[1,2,3] then {x:1}"
        );
        // Object delimiter appears first → return slice from '{'.
        assert_eq!(
            skip_preamble("preamble {x:1} then [1,2,3]"),
            "{x:1} then [1,2,3]"
        );
    }

    #[test]
    fn skip_preamble_handles_brace_in_preamble_before_array() {
        // Preamble itself contains '{'; helper anchors at the earlier '{'.
        // Strategy 3 (bracket-extraction) provides the recovery fallback,
        // so the overall parse still succeeds.
        let raw = "Status: {ok}\n\
                   [{\"action\":\"read_only_scan\",\"target\":\".\",\
                   \"expected_outcome\":\"ok\",\
                   \"verification_command\":\"ls\"}]";
        let plan = parse_plan_response(raw)
            .expect("bracket-extraction must recover from brace-bearing preamble");
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn parse_plan_response_skips_preamble_for_direct_strategy() {
        // Even with no fences and no trailing prose, direct strategy must
        // succeed once preamble is skipped.
        let raw = "preamble text\n[]";
        let plan = parse_plan_response(raw).expect("must parse empty array after preamble");
        assert!(plan.is_empty());
    }

    #[test]
    fn analyzed_action_all_variants_serialize() {
        for v in [
            AnalyzedAction::CreateFile,
            AnalyzedAction::AppendToFile,
            AnalyzedAction::RunShellCommand,
            AnalyzedAction::GitCommit,
            AnalyzedAction::OpenIssue,
            AnalyzedAction::StructuredTextReplace,
            AnalyzedAction::CargoTest,
            AnalyzedAction::ReadOnlyScan,
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(v, serde_json::from_str::<AnalyzedAction>(&json).unwrap());
        }
    }
}
