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
fn parse_plan_response_skips_amplihack_launcher_warning_lines() {
    let raw = "Warning: Could not prepare Copilot environment: [Errno 2] \
               No such file or directory: '/home/azureuser/.copilot/agents/amplihack'\n\
               Warning: Could not validate/repair config.json — nested agents may fail\n\
               [{\"action\":\"read_only_scan\",\"target\":\"src\",\
               \"expected_outcome\":\"scanned\",\
               \"verification_command\":\"ls src\"}]";
    let plan = parse_plan_response(raw)
        .expect("amplihack launcher Warning preamble must not break plan parsing (#1175)");
    assert_eq!(plan.len(), 1);
    assert_eq!(plan.steps()[0].action, AnalyzedAction::ReadOnlyScan);
    assert_eq!(plan.steps()[0].target, "src");
}

#[test]
fn strip_log_noise_lines_strips_leading_warning_with_bracketed_errno() {
    let raw = "Warning: Could not prepare Copilot environment: [Errno 2] foo\n[]";
    assert_eq!(strip_log_noise_lines(raw), "[]");
}

#[test]
fn strip_log_noise_lines_is_noop_when_first_line_is_json() {
    let raw = "[{\"a\":1}]";
    assert_eq!(strip_log_noise_lines(raw), raw);
}

#[test]
fn strip_log_noise_lines_stops_at_first_non_noise_line() {
    let raw = "Warning: foo\nHere is the plan:\n[]";
    // "Here is the plan:" is not a log prefix, so stripping stops there.
    assert_eq!(strip_log_noise_lines(raw), "Here is the plan:\n[]");
}

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
    let plan = parse_plan_response(raw).expect("preamble + fenced JSON must parse");
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
