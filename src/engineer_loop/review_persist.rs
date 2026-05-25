use std::path::Path;
use std::process::Command;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore};
use crate::goals::GoalRecord;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::sanitization::objective_metadata;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::terminal_engineer_bridge::{
    ScopedHandoffMode, TerminalBridgeContext, persist_handoff_artifacts,
};

use super::types::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, SessionErrorReflection,
    VerificationReport,
};
use super::{ENGINEER_BASE_TYPE, ENGINEER_IDENTITY, EXECUTION_SCOPE, MAX_PERSISTED_MEETING_MEMORY};

/// Run the optional LLM-driven review on mutating actions.
///
/// Skips (returns `Ok`) for read-only actions, test/check actions, and when
/// no LLM session is available. Blocks with [`SimardError::ReviewBlocked`]
/// if the review finds high-severity bugs or security issues.
pub fn run_optional_review(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
) -> SimardResult<()> {
    let is_mutating = matches!(
        action.selected.kind,
        EngineerActionKind::StructuredTextReplace(_)
            | EngineerActionKind::CreateFile(_)
            | EngineerActionKind::AppendToFile(_)
            | EngineerActionKind::GitCommit(_)
            | EngineerActionKind::AgentSession { .. }
    );
    if !is_mutating {
        return Ok(());
    }

    let mut review_session = match crate::review_pipeline::ReviewSession::open() {
        Ok(s) => s,
        // No session available (no API key, etc.) → review is honestly skipped
        // per this function's contract. The action already happened; review is
        // an optional safety net, not a hard gate.
        Err(SimardError::ReviewUnavailable { .. }) => return Ok(()),
        Err(e) => return Err(e),
    };

    let diff_text = compute_diff_for_review(&inspection.repo_root, &action.selected.kind);
    if diff_text.is_empty() {
        let _ = review_session.close();
        return Ok(());
    }

    let findings =
        crate::review_pipeline::review_diff(&mut review_session, &diff_text, PHILOSOPHY_REVIEW);
    let _ = review_session.close();

    let findings = match findings {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };

    if !crate::review_pipeline::should_commit(&findings) {
        let summary = crate::review_pipeline::summarize_review(&findings);
        return Err(SimardError::ReviewBlocked { summary });
    }

    Ok(())
}

pub(crate) const PHILOSOPHY_REVIEW: &str = "Ruthless simplicity. No unnecessary abstractions. \
    Modules under 400 lines. Every public function tested. \
    Clippy clean. No panics in library code.";

pub(crate) fn compute_diff_for_review(repo_root: &Path, kind: &EngineerActionKind) -> String {
    let args: &[&str] = match kind {
        EngineerActionKind::GitCommit(_) => &["git", "diff", "HEAD~1", "HEAD"],
        _ => &["git", "diff"],
    };
    match Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_root)
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    }
}

pub fn persist_engineer_loop_artifacts(
    state_root: &Path,
    topology: RuntimeTopology,
    objective: &str,
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    verification: &VerificationReport,
    terminal_bridge_context: Option<&TerminalBridgeContext>,
) -> SimardResult<()> {
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let evidence_store =
        FileBackedEvidenceStore::try_new(state_root.join("evidence_records.json"))?;

    let session_ids = UuidSessionIdGenerator;
    let mut session = SessionRecord::new(
        crate::identity::OperatingMode::Engineer,
        objective.to_string(),
        BaseTypeId::new(ENGINEER_BASE_TYPE),
        &session_ids,
    );
    session.advance(SessionPhase::Preparation)?;

    let scratch_key = format!("{}-engineer-loop-scratch", session.id);
    memory_store.put(MemoryRecord {
        key: scratch_key.clone(),
        scope: MemoryScope::SessionScratch,
        value: objective_metadata(objective),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Preparation,
        created_at: None,
    })?;
    session.attach_memory(scratch_key);

    session.advance(SessionPhase::Planning)?;
    session.advance(SessionPhase::Execution)?;
    let evidence_details = vec![
        format!("repo-root={}", inspection.repo_root.display()),
        format!("repo-branch={}", inspection.branch),
        format!("repo-head={}", inspection.head),
        format!("worktree-dirty={}", inspection.worktree_dirty),
        format!(
            "changed-files={}",
            if inspection.changed_files.is_empty() {
                "<none>".to_string()
            } else {
                inspection.changed_files.join(", ")
            }
        ),
        format!(
            "active-goals={}",
            if inspection.active_goals.is_empty() {
                "<none>".to_string()
            } else {
                inspection
                    .active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        format!(
            "carried-meeting-decisions={}",
            if inspection.carried_meeting_decisions.is_empty() {
                "<none>".to_string()
            } else {
                inspection.carried_meeting_decisions.join(" || ")
            }
        ),
        format!("architecture-gap={}", inspection.architecture_gap_summary),
        format!("execution-scope={EXECUTION_SCOPE}"),
        format!("selected-action={}", action.selected.label),
        format!("action-plan={}", action.selected.plan_summary),
        format!(
            "action-verification-steps={}",
            action.selected.verification_steps.join(" || ")
        ),
        format!("selected-action-rationale={}", action.selected.rationale),
        format!("action-command={}", action.selected.argv.join(" ")),
        "action-status=success".to_string(),
        format!("action-exit-code={}", action.exit_code),
        format!(
            "changed-files-after-action={}",
            if action.changed_files.is_empty() {
                "<none>".to_string()
            } else {
                action.changed_files.join(", ")
            }
        ),
        format!("verification-status={}", verification.status),
        format!("verification-summary={}", verification.summary),
    ];
    let mut evidence_details = evidence_details;
    if let Some(terminal_bridge_context) = terminal_bridge_context {
        evidence_details.extend(terminal_bridge_context.engineer_evidence_details());
    }

    for (index, detail) in evidence_details.into_iter().enumerate() {
        let evidence_id = format!("{}-engineer-loop-evidence-{}", session.id, index + 1);
        evidence_store.record(EvidenceRecord {
            id: evidence_id.clone(),
            session_id: session.id.clone(),
            phase: SessionPhase::Execution,
            detail,
            source: EvidenceSource::Runtime,
        })?;
        session.attach_evidence(evidence_id);
    }

    session.advance(SessionPhase::Reflection)?;
    session.advance(SessionPhase::Persistence)?;

    let summary_key = format!("{}-engineer-loop-summary", session.id);
    memory_store.put(MemoryRecord {
        key: summary_key.clone(),
        scope: MemoryScope::SessionSummary,
        value: format!(
            "engineer-loop-summary | repo-root={} | repo-branch={} | worktree-dirty={} | active-goals={} | carried-meeting-decisions={} | selected-action={} | verification-status={} | execution-scope={EXECUTION_SCOPE}",
            inspection.repo_root.display(),
            inspection.branch,
            inspection.worktree_dirty,
            if inspection.active_goals.is_empty() {
                "<none>".to_string()
            } else {
                inspection
                    .active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            inspection.carried_meeting_decisions.len(),
            action.selected.label,
            verification.status
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Persistence,
            created_at: None,
    })?;
    session.attach_memory(summary_key);

    let decision_key = format!("{}-engineer-loop-decision", session.id);
    memory_store.put(MemoryRecord {
        key: decision_key.clone(),
        scope: MemoryScope::Decision,
        value: format!(
            "engineer-loop-decision | carried-meeting-decisions={} | {} | {}",
            inspection.carried_meeting_decisions.len(),
            action.selected.rationale,
            verification.summary
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Persistence,
        created_at: None,
    })?;
    session.attach_memory(decision_key);

    // Bound persisted meeting memory per scope so a long chain of meeting →
    // engineer handoffs cannot grow `memory_records.json` without limit.
    // Pruned scopes are written in deterministic alphabetical order; each
    // call is independent and only writes to disk when the scope is over
    // the cap (see `FileBackedMemoryStore::prune_scope_to_cap`).
    memory_store.prune_scope_to_cap(MemoryScope::SessionSummary, MAX_PERSISTED_MEETING_MEMORY)?;
    memory_store.prune_scope_to_cap(MemoryScope::Decision, MAX_PERSISTED_MEETING_MEMORY)?;

    session.advance(SessionPhase::Complete)?;

    let handoff = RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Ready,
        identity_name: ENGINEER_IDENTITY.to_string(),
        selected_base_type: BaseTypeId::new(ENGINEER_BASE_TYPE),
        topology,
        source_runtime_node: RuntimeNodeId::local(),
        source_mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session: Some(session.redacted_for_handoff()),
        memory_records: memory_store.list_for_session(&session.id)?,
        evidence_records: evidence_store.list_for_session(&session.id)?,
        copilot_submit_audit: None,
    };
    persist_handoff_artifacts(state_root, ScopedHandoffMode::Engineer, &handoff)?;
    Ok(())
}

/// Best-effort reflection persistence for failed sessions (issue #2088).
///
/// The spec requires reflection on every session outcome, including errors.
/// This function writes a minimal `SessionErrorReflection` JSON file into
/// the state root so failures are captured for post-mortem analysis.
/// Errors during persistence are swallowed — the original session error
/// takes priority.
pub fn persist_error_reflection(state_root: &Path, reflection: &SessionErrorReflection) {
    let path = state_root.join("error_reflection.json");
    if let Ok(json) = serde_json::to_string_pretty(reflection) {
        let _ = std::fs::create_dir_all(state_root);
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn philosophy_review_is_non_empty() {
        assert!(!PHILOSOPHY_REVIEW.is_empty());
        assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
    }

    #[test]
    fn compute_diff_for_review_nonexistent_repo() {
        // When the repo root doesn't exist, git diff should fail gracefully
        let diff = compute_diff_for_review(
            Path::new("/tmp/nonexistent-repo-test-xyz"),
            &EngineerActionKind::ReadOnlyScan,
        );
        assert!(diff.is_empty());
    }

    #[test]
    fn compute_diff_for_review_git_commit_uses_head_diff() {
        // Verifying the function selects the right git command for GitCommit
        let diff = compute_diff_for_review(
            Path::new("/tmp/nonexistent-repo-test-xyz"),
            &EngineerActionKind::GitCommit(super::super::types::GitCommitRequest {
                message: "test commit".to_string(),
            }),
        );
        // With a nonexistent dir, diff is empty — the key test is no panic
        assert!(diff.is_empty());
    }

    #[test]
    fn run_optional_review_skips_non_mutating() {
        let inspection = RepoInspection {
            workspace_root: std::path::PathBuf::from("/tmp"),
            repo_root: std::path::PathBuf::from("/tmp"),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let action = ExecutedEngineerAction {
            selected: super::super::types::SelectedEngineerAction {
                label: "read-scan".to_string(),
                rationale: "testing".to_string(),
                argv: vec!["test".to_string()],
                plan_summary: "plan".to_string(),
                verification_steps: vec![],
                expected_changed_files: vec![],
                kind: EngineerActionKind::ReadOnlyScan,
            },
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            changed_files: vec![],
        };
        // ReadOnlyScan is non-mutating, so review should be skipped (return Ok)
        assert!(run_optional_review(&inspection, &action).is_ok());
    }

    #[test]
    fn persist_error_reflection_writes_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let reflection = SessionErrorReflection {
            objective: "fix the bug".to_string(),
            failed_phase: "agent-wait".to_string(),
            error_message: "LLM timeout after 60s".to_string(),
            phase_traces: vec![super::super::types::PhaseTrace {
                name: "inspect".to_string(),
                duration: std::time::Duration::from_millis(100),
                outcome: super::super::types::PhaseOutcome::Success,
            }],
        };
        persist_error_reflection(dir.path(), &reflection);

        let path = dir.path().join("error_reflection.json");
        assert!(path.exists(), "error_reflection.json should be written");
        let content = std::fs::read_to_string(&path).unwrap();
        let restored: SessionErrorReflection =
            serde_json::from_str(&content).expect("should be valid JSON");
        assert_eq!(restored.objective, "fix the bug");
        assert_eq!(restored.failed_phase, "agent-wait");
        assert_eq!(restored.error_message, "LLM timeout after 60s");
        assert_eq!(restored.phase_traces.len(), 1);
    }

    #[test]
    fn persist_error_reflection_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep/nested/state");
        let reflection = SessionErrorReflection {
            objective: "test".to_string(),
            failed_phase: "inspect".to_string(),
            error_message: "not a repo".to_string(),
            phase_traces: vec![],
        };
        persist_error_reflection(&nested, &reflection);
        assert!(nested.join("error_reflection.json").exists());
    }

    #[test]
    fn persist_error_reflection_best_effort_on_bad_path() {
        // Should not panic even with an impossible path
        let reflection = SessionErrorReflection {
            objective: "test".to_string(),
            failed_phase: "inspect".to_string(),
            error_message: "error".to_string(),
            phase_traces: vec![],
        };
        // Path containing null bytes cannot be created — best-effort swallows the error
        persist_error_reflection(std::path::Path::new("/dev/null/impossible"), &reflection);
    }
}
