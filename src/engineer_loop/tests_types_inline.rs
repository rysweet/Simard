use super::types::*;

// ── analyze_objective ────────────────────────────────────────────

#[test]
fn analyze_objective_create_file() {
    assert_eq!(
        analyze_objective("create a new file"),
        AnalyzedAction::CreateFile
    );
    assert_eq!(
        analyze_objective("add file foo.rs"),
        AnalyzedAction::CreateFile
    );
}

#[test]
fn analyze_objective_append() {
    assert_eq!(
        analyze_objective("append to README"),
        AnalyzedAction::AppendToFile
    );
    assert_eq!(
        analyze_objective("add to the config"),
        AnalyzedAction::AppendToFile
    );
}

#[test]
fn analyze_objective_git_commit() {
    assert_eq!(
        analyze_objective("commit the changes"),
        AnalyzedAction::GitCommit
    );
    assert_eq!(
        analyze_objective("save changes now"),
        AnalyzedAction::GitCommit
    );
}

#[test]
fn analyze_objective_open_issue() {
    assert_eq!(
        analyze_objective("open an issue for tracking"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("file a bug report"),
        AnalyzedAction::OpenIssue
    );
    assert_eq!(
        analyze_objective("submit feature request"),
        AnalyzedAction::OpenIssue
    );
}

#[test]
fn analyze_objective_cargo_test() {
    assert_eq!(
        analyze_objective("cargo test --lib"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(analyze_objective("run tests"), AnalyzedAction::CargoTest);
    assert_eq!(
        analyze_objective("run the tests now"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("test suite validation"),
        AnalyzedAction::CargoTest
    );
    assert_eq!(
        analyze_objective("test the module"),
        AnalyzedAction::CargoTest
    );
}

#[test]
fn analyze_objective_shell_command() {
    assert_eq!(
        analyze_objective("run cargo clippy"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("execute the script"),
        AnalyzedAction::RunShellCommand
    );
    assert_eq!(
        analyze_objective("check the output"),
        AnalyzedAction::RunShellCommand
    );
}

#[test]
fn analyze_objective_structured_replace() {
    assert_eq!(
        analyze_objective("fix the bug"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("change the config"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("update the version"),
        AnalyzedAction::StructuredTextReplace
    );
    assert_eq!(
        analyze_objective("replace the string"),
        AnalyzedAction::StructuredTextReplace
    );
}

#[test]
fn analyze_objective_read_only_default() {
    assert_eq!(
        analyze_objective("inspect the codebase"),
        AnalyzedAction::ReadOnlyScan
    );
    assert_eq!(
        analyze_objective("look at the structure"),
        AnalyzedAction::ReadOnlyScan
    );
}

// ── extract_command_from_objective ────────────────────────────────

#[test]
fn extract_command_none() {
    // "inspect the files" has no run/execute keyword
    assert!(
        analyze_objective("inspect the files") == AnalyzedAction::ReadOnlyScan,
        "inspect should default to ReadOnlyScan"
    );
}

// ── AnalyzedAction::is_mutating ──────────────────────────────────

#[test]
fn is_mutating_true_for_create_file() {
    assert!(AnalyzedAction::CreateFile.is_mutating());
}

#[test]
fn is_mutating_true_for_append_to_file() {
    assert!(AnalyzedAction::AppendToFile.is_mutating());
}

#[test]
fn is_mutating_true_for_git_commit() {
    assert!(AnalyzedAction::GitCommit.is_mutating());
}

#[test]
fn is_mutating_true_for_open_issue() {
    assert!(AnalyzedAction::OpenIssue.is_mutating());
}

#[test]
fn is_mutating_true_for_structured_text_replace() {
    assert!(AnalyzedAction::StructuredTextReplace.is_mutating());
}

#[test]
fn is_mutating_false_for_read_only_scan() {
    assert!(!AnalyzedAction::ReadOnlyScan.is_mutating());
}

#[test]
fn is_mutating_false_for_cargo_test() {
    assert!(!AnalyzedAction::CargoTest.is_mutating());
}

#[test]
fn is_mutating_false_for_run_shell_command() {
    assert!(!AnalyzedAction::RunShellCommand.is_mutating());
}

// ── PhaseTrace::session_phase mapping (issue #2100) ──────────────

#[test]
fn phase_trace_maps_inspect_to_intake() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "inspect".to_string(),
        duration: std::time::Duration::from_millis(10),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Intake);
}

#[test]
fn phase_trace_maps_pre_mutation_guard_to_intake() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "pre-mutation-guard".to_string(),
        duration: std::time::Duration::from_millis(1),
        outcome: PhaseOutcome::Failed("dirty".to_string()),
    };
    assert_eq!(trace.session_phase(), SessionPhase::Intake);
}

#[test]
fn phase_trace_maps_load_bridge_context_to_preparation() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "load-bridge-context".to_string(),
        duration: std::time::Duration::from_millis(5),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Preparation);
}

#[test]
fn phase_trace_maps_agent_prompt_build_to_planning() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "agent-prompt-build".to_string(),
        duration: std::time::Duration::from_millis(2),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Planning);
}

#[test]
fn phase_trace_maps_agent_spawn_to_execution() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "agent-spawn".to_string(),
        duration: std::time::Duration::from_millis(100),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Execution);
}

#[test]
fn phase_trace_maps_agent_wait_to_execution() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "agent-wait".to_string(),
        duration: std::time::Duration::from_secs(30),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Execution);
}

#[test]
fn phase_trace_maps_review_to_reflection() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "review".to_string(),
        duration: std::time::Duration::from_millis(500),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Reflection);
}

#[test]
fn phase_trace_maps_persist_to_persistence() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "persist".to_string(),
        duration: std::time::Duration::from_millis(50),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Persistence);
}

#[test]
fn phase_trace_maps_unknown_to_execution_default() {
    use crate::session::SessionPhase;
    let trace = PhaseTrace {
        name: "some-unknown-phase".to_string(),
        duration: std::time::Duration::from_millis(1),
        outcome: PhaseOutcome::Success,
    };
    assert_eq!(trace.session_phase(), SessionPhase::Execution);
}

// ── SessionErrorReflection ───────────────────────────────────────

#[test]
fn session_error_reflection_serialization_round_trip() {
    let reflection = SessionErrorReflection {
        objective: "fix the bug".to_string(),
        failed_phase: "agent-wait".to_string(),
        error_message: "LLM timeout".to_string(),
        phase_traces: vec![PhaseTrace {
            name: "inspect".to_string(),
            duration: std::time::Duration::from_millis(42),
            outcome: PhaseOutcome::Success,
        }],
        session_id: Some("session-test-001".to_string()),
    };
    let json = serde_json::to_string(&reflection).expect("serialize");
    let restored: SessionErrorReflection = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(reflection, restored);
}

#[test]
fn session_error_reflection_captures_all_fields() {
    let reflection = SessionErrorReflection {
        objective: "update config".to_string(),
        failed_phase: "review".to_string(),
        error_message: "review blocked".to_string(),
        phase_traces: vec![],
        session_id: None,
    };
    assert_eq!(reflection.objective, "update config");
    assert_eq!(reflection.failed_phase, "review");
    assert_eq!(reflection.error_message, "review blocked");
    assert!(reflection.phase_traces.is_empty());
}

#[test]
fn session_error_reflection_session_id_backward_compat() {
    // Old JSON without session_id field must deserialize with session_id = None
    let json =
        r#"{"objective":"test","failed_phase":"inspect","error_message":"err","phase_traces":[]}"#;
    let restored: SessionErrorReflection = serde_json::from_str(json).expect("deserialize");
    assert_eq!(restored.session_id, None);
}

#[test]
fn engineer_loop_run_session_record_backward_compat() {
    // EngineerLoopRun without session_record field should deserialize with None
    let run = EngineerLoopRun {
        state_root: std::path::PathBuf::from("/tmp"),
        execution_scope: "local-only".to_string(),
        inspection: crate::engineer_loop::types::RepoInspection {
            workspace_root: std::path::PathBuf::from("/tmp"),
            repo_root: std::path::PathBuf::from("/tmp"),
            branch: "main".to_string(),
            head: "abc".to_string(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        },
        action: crate::engineer_loop::types::ExecutedEngineerAction {
            selected: crate::engineer_loop::types::SelectedEngineerAction {
                label: "test".to_string(),
                rationale: "test".to_string(),
                argv: vec![],
                plan_summary: "test".to_string(),
                verification_steps: vec![],
                expected_changed_files: vec![],
                kind: crate::engineer_loop::types::EngineerActionKind::ReadOnlyScan,
            },
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            changed_files: vec![],
        },
        verification: crate::engineer_loop::types::VerificationReport {
            status: "ok".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        },
        terminal_bridge_context: None,
        elapsed_duration: std::time::Duration::from_millis(100),
        phase_traces: vec![],
        session_record: None,
    };
    // Round-trip serialization with session_record = None
    let json = serde_json::to_string(&run).unwrap();
    assert!(!json.contains("session_record"), "None should be skipped");
    let restored: EngineerLoopRun = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.session_record, None);
}
