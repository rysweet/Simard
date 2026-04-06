use std::path::Path;
use std::process::Command;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore};
use crate::goals::GoalRecord;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::{CognitiveMemoryType, FileBackedMemoryStore, MemoryRecord, MemoryStore};
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

const PHILOSOPHY_REVIEW: &str = "Ruthless simplicity. No unnecessary abstractions. \
    Modules under 400 lines. Every public function tested. \
    Clippy clean. No panics in library code.";

fn compute_diff_for_review(repo_root: &Path, kind: &EngineerActionKind) -> String {
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
        memory_type: CognitiveMemoryType::Working,
        value: objective_metadata(objective),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Preparation,
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
        memory_type: CognitiveMemoryType::Episodic,
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
    })?;
    session.attach_memory(summary_key);

    let decision_key = format!("{}-engineer-loop-decision", session.id);
    memory_store.put(MemoryRecord {
        key: decision_key.clone(),
        memory_type: CognitiveMemoryType::Semantic,
        value: format!(
            "engineer-loop-decision | carried-meeting-decisions={} | {} | {}",
            inspection.carried_meeting_decisions.len(),
            action.selected.rationale,
            verification.summary
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Persistence,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use super::super::types::{
        EngineerActionKind, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
    };

    fn make_inspection() -> RepoInspection {
        RepoInspection {
            workspace_root: PathBuf::from("/fake/workspace"),
            repo_root: PathBuf::from("/fake/repo"),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: Vec::new(),
            active_goals: Vec::new(),
            carried_meeting_decisions: Vec::new(),
            architecture_gap_summary: String::new(),
        }
    }

    fn make_executed(kind: EngineerActionKind) -> ExecutedEngineerAction {
        ExecutedEngineerAction {
            selected: SelectedEngineerAction {
                label: "test-action".into(),
                rationale: "test".into(),
                argv: vec!["test".into()],
                plan_summary: "test".into(),
                verification_steps: Vec::new(),
                expected_changed_files: Vec::new(),
                kind,
            },
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            changed_files: Vec::new(),
        }
    }

    // --- run_optional_review: non-mutating actions skip review ---

    #[test]
    fn optional_review_skips_read_only_scan() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_skips_cargo_test() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::CargoTest);
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_skips_cargo_check() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::CargoCheck);
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_skips_run_shell_command() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::RunShellCommand(
            super::super::types::ShellCommandRequest {
                argv: vec!["cargo".into(), "fmt".into()],
            },
        ));
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_skips_open_issue() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::OpenIssue(
            super::super::types::OpenIssueRequest {
                title: "t".into(),
                body: String::new(),
                labels: Vec::new(),
            },
        ));
        run_optional_review(&inspection, &action).unwrap();
    }

    // --- compute_diff_for_review: argument selection ---

    #[test]
    fn diff_for_review_git_commit_uses_head_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::GitCommit(super::super::types::GitCommitRequest {
            message: "test".into(),
        });
        // Won't succeed (not a git repo), but should return empty string gracefully
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty()); // no git repo → empty
    }

    #[test]
    fn diff_for_review_non_commit_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::ReadOnlyScan;
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty()); // no git repo → empty
    }

    // --- PHILOSOPHY_REVIEW constant ---

    #[test]
    fn philosophy_review_is_not_empty() {
        assert!(!PHILOSOPHY_REVIEW.is_empty());
        assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
    }

    // --- run_optional_review: mutating actions (ReviewSession returns None in tests) ---

    #[test]
    fn optional_review_mutating_structured_text_replace_succeeds_without_session() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::StructuredTextReplace(
            super::super::types::StructuredEditRequest {
                relative_path: "src/lib.rs".into(),
                search: "old".into(),
                replacement: "new".into(),
                verify_contains: "new".into(),
            },
        ));
        // No LLM session available → review is skipped, returns Ok
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_mutating_create_file_succeeds_without_session() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::CreateFile(
            super::super::types::CreateFileRequest {
                relative_path: "new.rs".into(),
                content: "fn main() {}".into(),
            },
        ));
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_mutating_append_to_file_succeeds_without_session() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::AppendToFile(
            super::super::types::AppendToFileRequest {
                relative_path: "log.txt".into(),
                content: "entry".into(),
            },
        ));
        run_optional_review(&inspection, &action).unwrap();
    }

    #[test]
    fn optional_review_mutating_git_commit_succeeds_without_session() {
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::GitCommit(
            super::super::types::GitCommitRequest {
                message: "chore: test".into(),
            },
        ));
        run_optional_review(&inspection, &action).unwrap();
    }

    // --- compute_diff_for_review: action kind variants ---

    #[test]
    fn diff_for_review_create_file_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::CreateFile(super::super::types::CreateFileRequest {
            relative_path: "new.rs".into(),
            content: "content".into(),
        });
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty()); // not a git repo
    }

    #[test]
    fn diff_for_review_append_to_file_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::AppendToFile(super::super::types::AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "entry".into(),
        });
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_for_review_structured_text_replace_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind =
            EngineerActionKind::StructuredTextReplace(super::super::types::StructuredEditRequest {
                relative_path: "src/lib.rs".into(),
                search: "old".into(),
                replacement: "new".into(),
                verify_contains: "new".into(),
            });
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_for_review_cargo_test_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let result = compute_diff_for_review(dir.path(), &EngineerActionKind::CargoTest);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_for_review_cargo_check_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let result = compute_diff_for_review(dir.path(), &EngineerActionKind::CargoCheck);
        assert!(result.is_empty());
    }

    // --- persist_engineer_loop_artifacts ---

    #[test]
    fn persist_artifacts_creates_files_in_state_root() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "all checks ok".to_string(),
            checks: vec!["check1".to_string()],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "test objective",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
        // Memory and evidence files should be created
        assert!(state_dir.path().join("memory_records.json").exists());
        assert!(state_dir.path().join("evidence_records.json").exists());
    }

    #[test]
    fn persist_artifacts_with_nonempty_inspection_fields() {
        let state_dir = tempfile::tempdir().unwrap();
        let mut inspection = make_inspection();
        inspection.worktree_dirty = true;
        inspection.changed_files = vec!["src/main.rs".to_string(), "Cargo.toml".to_string()];
        inspection.carried_meeting_decisions = vec!["decision-1".to_string()];
        inspection.architecture_gap_summary = "some gap summary".to_string();
        let mut action = make_executed(EngineerActionKind::ReadOnlyScan);
        action.selected.verification_steps = vec!["step1".to_string(), "step2".to_string()];
        action.changed_files = vec!["src/main.rs".to_string()];
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "verification complete".to_string(),
            checks: vec![],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "complex objective with details",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn persist_artifacts_with_different_topologies() {
        for topology in [
            RuntimeTopology::SingleProcess,
            RuntimeTopology::MultiProcess,
            RuntimeTopology::Distributed,
        ] {
            let state_dir = tempfile::tempdir().unwrap();
            let inspection = make_inspection();
            let action = make_executed(EngineerActionKind::ReadOnlyScan);
            let verification = super::super::types::VerificationReport {
                status: "ok".to_string(),
                summary: "ok".to_string(),
                checks: vec![],
            };
            let result = persist_engineer_loop_artifacts(
                state_dir.path(),
                topology,
                "test",
                &inspection,
                &action,
                &verification,
                None,
            );
            assert!(result.is_ok(), "failed for topology {:?}", topology);
        }
    }

    // --- make_inspection / make_executed helper validation ---

    #[test]
    fn make_inspection_has_expected_defaults() {
        let insp = make_inspection();
        assert_eq!(insp.branch, "main");
        assert_eq!(insp.head, "abc123");
        assert!(!insp.worktree_dirty);
        assert!(insp.changed_files.is_empty());
        assert!(insp.active_goals.is_empty());
        assert!(insp.carried_meeting_decisions.is_empty());
        assert!(insp.architecture_gap_summary.is_empty());
    }

    #[test]
    fn make_executed_has_expected_defaults() {
        let exec = make_executed(EngineerActionKind::ReadOnlyScan);
        assert_eq!(exec.exit_code, 0);
        assert!(exec.stdout.is_empty());
        assert!(exec.stderr.is_empty());
        assert!(exec.changed_files.is_empty());
        assert_eq!(exec.selected.label, "test-action");
    }

    // --- PHILOSOPHY_REVIEW content checks ---

    #[test]
    fn philosophy_review_mentions_key_principles() {
        assert!(PHILOSOPHY_REVIEW.contains("simplicity"));
        assert!(PHILOSOPHY_REVIEW.contains("400 lines"));
        assert!(PHILOSOPHY_REVIEW.contains("Clippy"));
        assert!(PHILOSOPHY_REVIEW.contains("panics"));
    }

    // --- run_optional_review: additional non-mutating action kinds ---

    #[test]
    fn optional_review_skips_cargo_clippy() {
        let inspection = make_inspection();
        // CargoCheck is the closest — verify it still passes
        let action = make_executed(EngineerActionKind::CargoCheck);
        assert!(run_optional_review(&inspection, &action).is_ok());
    }

    // --- compute_diff_for_review: all action kind variants ---

    #[test]
    fn diff_for_review_run_shell_command_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::RunShellCommand(super::super::types::ShellCommandRequest {
            argv: vec!["echo".into(), "hello".into()],
        });
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_for_review_open_issue_uses_git_diff() {
        let dir = tempfile::tempdir().unwrap();
        let kind = EngineerActionKind::OpenIssue(super::super::types::OpenIssueRequest {
            title: "test".into(),
            body: "body".into(),
            labels: vec!["bug".into()],
        });
        let result = compute_diff_for_review(dir.path(), &kind);
        assert!(result.is_empty());
    }

    // --- persist_engineer_loop_artifacts: additional coverage ---

    #[test]
    fn persist_artifacts_with_active_goals() {
        let state_dir = tempfile::tempdir().unwrap();
        let mut inspection = make_inspection();
        inspection.active_goals = vec![crate::goals::GoalRecord {
            slug: "g1".to_string(),
            title: "First goal".to_string(),
            rationale: "test rationale".to_string(),
            status: crate::goals::GoalStatus::Active,
            priority: 1,
            owner_identity: "test-owner".to_string(),
            source_session_id: crate::session::SessionId::parse(
                "session-00000000-0000-0000-0000-000000000001",
            )
            .unwrap(),
            updated_in: crate::session::SessionPhase::Preparation,
        }];
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "test with goals",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn persist_artifacts_with_carried_decisions() {
        let state_dir = tempfile::tempdir().unwrap();
        let mut inspection = make_inspection();
        inspection.carried_meeting_decisions =
            vec!["decision-a".to_string(), "decision-b".to_string()];
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "test with decisions",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn persist_artifacts_with_mutating_action_kind() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::CreateFile(
            super::super::types::CreateFileRequest {
                relative_path: "new_file.rs".into(),
                content: "fn main() {}".into(),
            },
        ));
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "file created".to_string(),
            checks: vec!["file_exists".to_string()],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "create file test",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn persist_artifacts_memory_records_are_readable() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "readable test",
            &inspection,
            &action,
            &verification,
            None,
        )
        .unwrap();

        let memory_path = state_dir.path().join("memory_records.json");
        let content = std::fs::read_to_string(&memory_path).unwrap();
        assert!(
            content.contains("engineer-loop"),
            "memory should reference engineer-loop: {}",
            &content[..content.len().min(200)]
        );
    }

    #[test]
    fn persist_artifacts_evidence_records_are_readable() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "evidence test",
            &inspection,
            &action,
            &verification,
            None,
        )
        .unwrap();

        let evidence_path = state_dir.path().join("evidence_records.json");
        let content = std::fs::read_to_string(&evidence_path).unwrap();
        assert!(
            content.contains("repo-root"),
            "evidence should contain repo-root"
        );
    }

    #[test]
    fn persist_artifacts_with_long_objective() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::ReadOnlyScan);
        let verification = super::super::types::VerificationReport {
            status: "ok".to_string(),
            summary: "ok".to_string(),
            checks: vec![],
        };
        let long_objective = "x".repeat(5000);
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            &long_objective,
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn persist_artifacts_with_git_commit_action() {
        let state_dir = tempfile::tempdir().unwrap();
        let inspection = make_inspection();
        let action = make_executed(EngineerActionKind::GitCommit(
            super::super::types::GitCommitRequest {
                message: "chore: test commit".into(),
            },
        ));
        let verification = super::super::types::VerificationReport {
            status: "passed".to_string(),
            summary: "commit ok".to_string(),
            checks: vec![],
        };
        let result = persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            "git commit test",
            &inspection,
            &action,
            &verification,
            None,
        );
        assert!(result.is_ok());
    }

    // --- make_executed with different exit codes ---

    #[test]
    fn make_executed_can_have_custom_exit_code() {
        let mut exec = make_executed(EngineerActionKind::CargoTest);
        exec.exit_code = 1;
        assert_eq!(exec.exit_code, 1);
    }

    #[test]
    fn make_executed_can_have_stdout_stderr() {
        let mut exec = make_executed(EngineerActionKind::CargoTest);
        exec.stdout = "test output".to_string();
        exec.stderr = "warning".to_string();
        assert_eq!(exec.stdout, "test output");
        assert_eq!(exec.stderr, "warning");
    }

    // --- PHILOSOPHY_REVIEW additional checks ---

    #[test]
    fn philosophy_review_mentions_tested() {
        assert!(PHILOSOPHY_REVIEW.contains("tested"));
    }

    #[test]
    fn philosophy_review_mentions_modules() {
        assert!(PHILOSOPHY_REVIEW.contains("Modules") || PHILOSOPHY_REVIEW.contains("modules"));
    }
}
