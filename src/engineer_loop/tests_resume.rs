//! Tests for session checkpoint RESUME (resume-on-startup).
//!
//! The companion `tests_checkpoint` module covers the save/load/clear
//! primitives. This module covers the *resume* half added on top of them:
//!
//!   1. `should_resume` / `is_resumable` / `resumable_execution` decision logic.
//!   2. `run_local_engineer_loop` resumes an interrupted session from its last
//!      checkpoint instead of restarting — and, critically, does NOT re-spawn a
//!      completed agent session (no double-PR, no duplicate work).

use std::path::PathBuf;

use crate::base_types::BaseTypeId;
use crate::identity::OperatingMode;
use crate::runtime::RuntimeTopology;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

use super::run_local_engineer_loop;
use super::should_resume;
use super::types::{
    EngineerActionKind, ExecutedEngineerAction, ExecutionPlan, PhaseOutcome, PhaseTrace,
    RepoInspection, SelectedEngineerAction, SessionCheckpoint, VerificationReport,
};

const OBJECTIVE: &str = "resume test objective";

fn session_at(phase: SessionPhase) -> SessionRecord {
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        OBJECTIVE,
        BaseTypeId::new("terminal-shell"),
        &UuidSessionIdGenerator,
    );
    for next in [
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
    ] {
        if session.phase == phase {
            break;
        }
        session.advance(next).unwrap();
    }
    session
}

fn inspection_for(repo_root: PathBuf) -> RepoInspection {
    RepoInspection {
        workspace_root: repo_root.clone(),
        repo_root,
        branch: "main".to_string(),
        head: "deadbeef".to_string(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    }
}

fn agent_action(outcome: &str) -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: SelectedEngineerAction {
            label: "agent-session".to_string(),
            rationale: "resumed".to_string(),
            argv: vec![],
            plan_summary: OBJECTIVE.to_string(),
            verification_steps: vec![],
            expected_changed_files: vec![],
            kind: EngineerActionKind::AgentSession {
                outcome_summary: outcome.to_string(),
            },
        },
        exit_code: 0,
        stdout: outcome.to_string(),
        stderr: String::new(),
        changed_files: vec![],
    }
}

fn execution_checkpoint(repo_root: PathBuf, outcome: &str) -> SessionCheckpoint {
    let session = session_at(SessionPhase::Execution);
    SessionCheckpoint {
        session_id: session.id.to_string(),
        objective: OBJECTIVE.to_string(),
        completed_phase: SessionPhase::Execution,
        inspection: Some(inspection_for(repo_root)),
        terminal_handoff_context: None,
        execution_plan: Some(ExecutionPlan {
            objective: OBJECTIVE.to_string(),
            steps: vec!["resumed".to_string()],
            expected_changed_files: vec![],
            risk_level: "low".to_string(),
            is_mutating: true,
        }),
        action: Some(agent_action(outcome)),
        verification: Some(VerificationReport {
            status: "verified".to_string(),
            summary: "resumed verification".to_string(),
            checks: vec![],
        }),
        session_summary: None,
        phase_traces: vec![PhaseTrace {
            name: "agent-wait".to_string(),
            duration: std::time::Duration::from_secs(1),
            outcome: PhaseOutcome::Success,
        }],
        session_record: session,
    }
}

// ── decision logic ──────────────────────────────────────────────

#[test]
fn should_resume_true_for_matching_objective_and_resumable_phase() {
    for phase in [
        SessionPhase::Intake,
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
    ] {
        let mut cp = execution_checkpoint(PathBuf::from("/tmp/repo"), "x");
        cp.completed_phase = phase;
        assert!(
            should_resume(&cp, OBJECTIVE),
            "phase {phase} with matching objective should resume"
        );
    }
}

#[test]
fn should_resume_false_for_mismatched_objective() {
    let cp = execution_checkpoint(PathBuf::from("/tmp/repo"), "x");
    assert!(
        !should_resume(&cp, "a different goal"),
        "a checkpoint from another goal must never be resumed"
    );
}

#[test]
fn should_resume_false_for_terminal_phases() {
    for phase in [
        SessionPhase::Reflection,
        SessionPhase::Summarize,
        SessionPhase::Persistence,
        SessionPhase::Complete,
        SessionPhase::Failed,
    ] {
        let mut cp = execution_checkpoint(PathBuf::from("/tmp/repo"), "x");
        cp.completed_phase = phase;
        assert!(
            !should_resume(&cp, OBJECTIVE),
            "phase {phase} is terminal and must not be resumed"
        );
    }
}

#[test]
fn resumable_execution_requires_recorded_action_and_verification() {
    let full = execution_checkpoint(PathBuf::from("/tmp/repo"), "done");
    assert!(full.resumable_execution().is_some());

    let mut missing_action = full.clone();
    missing_action.action = None;
    assert!(
        missing_action.resumable_execution().is_none(),
        "without a recorded action, execution must be re-run, not reused"
    );

    let mut pre_execution = full.clone();
    pre_execution.completed_phase = SessionPhase::Planning;
    assert!(
        pre_execution.resumable_execution().is_none(),
        "a Planning checkpoint has no completed execution to reuse"
    );
}

// ── end-to-end resume ───────────────────────────────────────────

fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::util::spawn_retry::retry_spawn_sync(|| {
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
    })
    .unwrap();
    crate::util::spawn_retry::retry_spawn_sync(|| {
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
    })
    .unwrap();
    dir
}

/// The linchpin test: when an Execution checkpoint exists, the loop resumes
/// past Execution and completes WITHOUT spawning an agent. There is no LLM /
/// `amplihack` configured in the test environment, so a fresh run would fail at
/// agent-spawn; reaching a successful completion proves the agent was reused,
/// not re-run (no double-PR, no duplicate work).
#[test]
fn resume_from_execution_checkpoint_skips_agent_spawn() {
    let repo = init_git_repo();
    let state_root = repo.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    let checkpoint = execution_checkpoint(repo.path().to_path_buf(), "resumed-agent-output");
    checkpoint.save(&state_root).unwrap();

    let run = run_local_engineer_loop(
        repo.path(),
        OBJECTIVE,
        RuntimeTopology::SingleProcess,
        &state_root,
    )
    .expect("resume should complete without spawning the agent");

    // The recorded agent result was reused verbatim.
    assert_eq!(run.action.selected.label, "agent-session");
    assert_eq!(run.action.stdout, "resumed-agent-output");
    assert_eq!(run.verification.summary, "resumed verification");

    // The resume is auditable and the original session identity is preserved.
    let names: Vec<&str> = run.phase_traces.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"resume"),
        "phase traces should record the resume marker; got {names:?}"
    );
    assert!(
        !names.contains(&"agent-spawn"),
        "a resumed execution must not spawn a new agent; got {names:?}"
    );
    let session = run.session_record.expect("session record present");
    assert_eq!(session.id.to_string(), checkpoint.session_id);
    assert_eq!(session.phase, SessionPhase::Complete);

    // On success the checkpoint is cleared so a later dispatch starts fresh.
    assert!(
        SessionCheckpoint::load(&state_root).is_none(),
        "checkpoint should be cleared after a successful resumed completion"
    );
}

/// Resuming twice is idempotent: after the first resume completes and clears the
/// checkpoint, re-writing the same checkpoint and resuming again still completes
/// with the same reused result and never spawns an agent.
#[test]
fn resume_is_idempotent_across_repeated_dispatch() {
    let repo = init_git_repo();
    let state_root = repo.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    for _ in 0..2 {
        execution_checkpoint(repo.path().to_path_buf(), "idempotent-output")
            .save(&state_root)
            .unwrap();
        let run = run_local_engineer_loop(
            repo.path(),
            OBJECTIVE,
            RuntimeTopology::SingleProcess,
            &state_root,
        )
        .expect("each resumed dispatch should complete");
        assert_eq!(run.action.stdout, "idempotent-output");
        assert!(SessionCheckpoint::load(&state_root).is_none());
    }
}
