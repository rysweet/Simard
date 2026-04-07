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
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, VerificationReport,
};
use super::{ENGINEER_BASE_TYPE, ENGINEER_IDENTITY, EXECUTION_SCOPE};

/// Run the optional LLM-driven review on mutating actions.
///
/// Skips (returns `Ok`) for read-only actions, test/check actions, and when
/// no LLM session is available. Blocks with [`SimardError::ReviewBlocked`]
/// if the review finds high-severity bugs or security issues.
pub(crate) fn run_optional_review(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
) -> SimardResult<()> {
    let is_mutating = matches!(
        action.selected.kind,
        EngineerActionKind::StructuredTextReplace(_)
            | EngineerActionKind::CreateFile(_)
            | EngineerActionKind::AppendToFile(_)
            | EngineerActionKind::GitCommit(_)
    );
    if !is_mutating {
        return Ok(());
    }

    let mut review_session = match crate::review_pipeline::ReviewSession::open() {
        Some(s) => s,
        None => return Ok(()),
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

pub(crate) fn persist_engineer_loop_artifacts(
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
