use std::path::PathBuf;

use crate::base_types::BaseTypeId;
use crate::identity::OperatingMode;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

use super::types::{
    EngineerActionKind, ExecutedEngineerAction, ExecutionPlan, PhaseOutcome, PhaseTrace,
    RepoInspection, SelectedEngineerAction, SessionCheckpoint, SessionSummary, VerificationReport,
};

fn make_test_session() -> SessionRecord {
    SessionRecord::new(
        OperatingMode::Engineer,
        "test objective",
        BaseTypeId::new("terminal-shell"),
        &UuidSessionIdGenerator,
    )
}

fn make_test_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/tmp/workspace"),
        repo_root: PathBuf::from("/tmp/repo"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    }
}

fn make_test_checkpoint(session: &SessionRecord) -> SessionCheckpoint {
    SessionCheckpoint {
        session_id: session.id.to_string(),
        objective: "test objective".to_string(),
        completed_phase: SessionPhase::Intake,
        inspection: Some(make_test_inspection()),
        terminal_engineer_context: None,
        execution_plan: None,
        action: None,
        verification: None,
        session_summary: None,
        phase_traces: vec![PhaseTrace {
            name: "inspect".to_string(),
            duration: std::time::Duration::from_millis(42),
            outcome: PhaseOutcome::Success,
        }],
        session_record: session.clone(),
    }
}

// ── SessionCheckpoint save/load/clear ───────────────────────────

#[test]
fn checkpoint_save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_test_session();
    let checkpoint = make_test_checkpoint(&session);

    checkpoint.save(dir.path()).unwrap();
    let loaded = SessionCheckpoint::load(dir.path()).expect("should load checkpoint");

    assert_eq!(loaded.session_id, checkpoint.session_id);
    assert_eq!(loaded.objective, "test objective");
    assert_eq!(loaded.completed_phase, SessionPhase::Intake);
    assert!(loaded.inspection.is_some());
    assert!(loaded.action.is_none());
    assert_eq!(loaded.phase_traces.len(), 1);
    assert_eq!(loaded.session_record.phase, SessionPhase::Intake);
}

#[test]
fn checkpoint_load_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SessionCheckpoint::load(dir.path()).is_none());
}

#[test]
fn checkpoint_clear_removes_file() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_test_session();
    let checkpoint = make_test_checkpoint(&session);

    checkpoint.save(dir.path()).unwrap();
    assert!(dir.path().join("session_checkpoint.json").exists());

    SessionCheckpoint::clear(dir.path());
    assert!(!dir.path().join("session_checkpoint.json").exists());
}

#[test]
fn checkpoint_clear_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    // Clearing a non-existent checkpoint should not panic
    SessionCheckpoint::clear(dir.path());
    SessionCheckpoint::clear(dir.path());
}

#[test]
fn checkpoint_save_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("deep/nested/state");
    let session = make_test_session();
    let checkpoint = make_test_checkpoint(&session);

    checkpoint.save(&nested).unwrap();
    assert!(nested.join("session_checkpoint.json").exists());
}

#[test]
fn checkpoint_serialization_with_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = make_test_session();
    session.advance(SessionPhase::Preparation).unwrap();
    session.advance(SessionPhase::Planning).unwrap();
    session.advance(SessionPhase::Execution).unwrap();

    let checkpoint = SessionCheckpoint {
        session_id: session.id.to_string(),
        objective: "full checkpoint test".to_string(),
        completed_phase: SessionPhase::Execution,
        inspection: Some(make_test_inspection()),
        terminal_engineer_context: None,
        execution_plan: Some(ExecutionPlan {
            objective: "full checkpoint test".to_string(),
            steps: vec!["step 1".to_string()],
            expected_changed_files: vec!["src/lib.rs".to_string()],
            risk_level: "medium".to_string(),
            is_mutating: true,
        }),
        action: Some(ExecutedEngineerAction {
            selected: SelectedEngineerAction {
                label: "agent-session".to_string(),
                rationale: "test".to_string(),
                argv: vec![],
                plan_summary: "plan".to_string(),
                verification_steps: vec![],
                expected_changed_files: vec![],
                kind: EngineerActionKind::AgentSession {
                    outcome_summary: "done".to_string(),
                },
            },
            exit_code: 0,
            stdout: "success".to_string(),
            stderr: String::new(),
            changed_files: vec!["src/lib.rs".to_string()],
        }),
        verification: Some(VerificationReport {
            status: "verified".to_string(),
            summary: "all checks passed".to_string(),
            checks: vec!["git: 1 new commit".to_string()],
        }),
        session_summary: Some(SessionSummary {
            objective: "full checkpoint test".to_string(),
            outcome: "success".to_string(),
            changed_files: vec!["src/lib.rs".to_string()],
            key_decisions: vec!["used agent session".to_string()],
            accomplishment: "completed successfully".to_string(),
        }),
        phase_traces: vec![
            PhaseTrace {
                name: "inspect".to_string(),
                duration: std::time::Duration::from_millis(10),
                outcome: PhaseOutcome::Success,
            },
            PhaseTrace {
                name: "agent-wait".to_string(),
                duration: std::time::Duration::from_secs(5),
                outcome: PhaseOutcome::Success,
            },
        ],
        session_record: session,
    };

    checkpoint.save(dir.path()).unwrap();
    let loaded = SessionCheckpoint::load(dir.path()).unwrap();

    assert_eq!(loaded.completed_phase, SessionPhase::Execution);
    assert!(loaded.execution_plan.is_some());
    assert!(loaded.action.is_some());
    assert!(loaded.verification.is_some());
    assert!(loaded.session_summary.is_some());
    assert_eq!(loaded.phase_traces.len(), 2);
    assert_eq!(loaded.action.unwrap().selected.label, "agent-session");
}

#[test]
fn checkpoint_overwrite_replaces_previous() {
    let dir = tempfile::tempdir().unwrap();
    let session = make_test_session();

    let checkpoint1 = SessionCheckpoint {
        session_id: session.id.to_string(),
        objective: "first".to_string(),
        completed_phase: SessionPhase::Intake,
        inspection: None,
        terminal_engineer_context: None,
        execution_plan: None,
        action: None,
        verification: None,
        session_summary: None,
        phase_traces: vec![],
        session_record: session.clone(),
    };
    checkpoint1.save(dir.path()).unwrap();

    let checkpoint2 = SessionCheckpoint {
        session_id: session.id.to_string(),
        objective: "second".to_string(),
        completed_phase: SessionPhase::Preparation,
        inspection: Some(make_test_inspection()),
        terminal_engineer_context: None,
        execution_plan: None,
        action: None,
        verification: None,
        session_summary: None,
        phase_traces: vec![],
        session_record: session.clone(),
    };
    checkpoint2.save(dir.path()).unwrap();

    let loaded = SessionCheckpoint::load(dir.path()).unwrap();
    assert_eq!(loaded.objective, "second");
    assert_eq!(loaded.completed_phase, SessionPhase::Preparation);
}

#[test]
fn checkpoint_load_returns_none_for_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("session_checkpoint.json"), "not valid json").unwrap();
    assert!(SessionCheckpoint::load(dir.path()).is_none());
}
