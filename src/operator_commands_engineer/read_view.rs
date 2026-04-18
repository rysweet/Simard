use std::path::PathBuf;

use crate::operator_commands::{
    print_display, print_terminal_bridge_section, print_text, render_redacted_objective_metadata,
    validated_engineer_read_artifacts,
};
use crate::terminal_engineer_bridge::{
    ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE,
    TerminalBridgeContext, load_runtime_handoff_snapshot,
};
use crate::{FileBackedEvidenceStore, FileBackedMemoryStore};

use super::parsing::{
    parse_carried_meeting_decisions, parse_engineer_summary_list, required_engineer_evidence_value,
};

pub(super) struct EngineerReadView {
    state_root: PathBuf,
    handoff_source: String,
    identity: String,
    selected_base_type: String,
    topology: String,
    session_phase: String,
    objective_metadata: String,
    repo_root: PathBuf,
    repo_branch: String,
    repo_head: String,
    worktree_dirty: String,
    changed_files: String,
    active_goals: Vec<String>,
    carried_meeting_decisions: Vec<String>,
    selected_action: String,
    action_plan: String,
    verification_steps: String,
    action_status: String,
    changed_files_after_action: String,
    verification_status: String,
    verification_summary: String,
    terminal_bridge_context: Option<TerminalBridgeContext>,
    memory_record_count: usize,
    evidence_record_count: usize,
}

impl EngineerReadView {
    pub(super) fn load(state_root: PathBuf) -> crate::SimardResult<Self> {
        let artifacts = validated_engineer_read_artifacts(&state_root)?;
        let handoff_source = artifacts.handoff_file_name.clone();
        let handoff = load_runtime_handoff_snapshot(
            &crate::terminal_engineer_bridge::SelectedHandoffArtifact {
                path: artifacts.handoff_path.clone(),
                file_name: match handoff_source.as_str() {
                    ENGINEER_HANDOFF_FILE_NAME => ENGINEER_HANDOFF_FILE_NAME,
                    _ => crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME,
                },
            },
            "engineer read",
        )?;
        let session =
            handoff
                .session
                .as_ref()
                .ok_or_else(|| crate::SimardError::InvalidHandoffSnapshot {
                    field: "session".to_string(),
                    reason: format!(
                        "engineer read requires {} to contain a persisted session snapshot",
                        artifacts.handoff_file_name
                    ),
                })?;

        FileBackedMemoryStore::try_new(artifacts.memory_path)?;
        FileBackedEvidenceStore::try_new(artifacts.evidence_path)?;

        Ok(Self {
            state_root,
            handoff_source: handoff_source.clone(),
            identity: handoff.identity_name,
            selected_base_type: handoff.selected_base_type.to_string(),
            topology: handoff.topology.to_string(),
            session_phase: session.phase.to_string(),
            objective_metadata: render_redacted_objective_metadata(&session.objective)?,
            repo_root: PathBuf::from(required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-root=",
                &handoff_source,
            )?),
            repo_branch: required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-branch=",
                &handoff_source,
            )?
            .to_string(),
            repo_head: required_engineer_evidence_value(
                &handoff.evidence_records,
                "repo-head=",
                &handoff_source,
            )?
            .to_string(),
            worktree_dirty: required_engineer_evidence_value(
                &handoff.evidence_records,
                "worktree-dirty=",
                &handoff_source,
            )?
            .to_string(),
            changed_files: required_engineer_evidence_value(
                &handoff.evidence_records,
                "changed-files=",
                &handoff_source,
            )?
            .to_string(),
            active_goals: parse_engineer_summary_list(
                required_engineer_evidence_value(
                    &handoff.evidence_records,
                    "active-goals=",
                    &handoff_source,
                )?,
                ", ",
            ),
            carried_meeting_decisions: parse_carried_meeting_decisions(
                required_engineer_evidence_value(
                    &handoff.evidence_records,
                    "carried-meeting-decisions=",
                    &handoff_source,
                )?,
            )?,
            selected_action: required_engineer_evidence_value(
                &handoff.evidence_records,
                "selected-action=",
                &handoff_source,
            )?
            .to_string(),
            action_plan: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-plan=",
                &handoff_source,
            )?
            .to_string(),
            verification_steps: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-verification-steps=",
                &handoff_source,
            )?
            .to_string(),
            action_status: required_engineer_evidence_value(
                &handoff.evidence_records,
                "action-status=",
                &handoff_source,
            )?
            .to_string(),
            changed_files_after_action: required_engineer_evidence_value(
                &handoff.evidence_records,
                "changed-files-after-action=",
                &handoff_source,
            )?
            .to_string(),
            verification_status: required_engineer_evidence_value(
                &handoff.evidence_records,
                "verification-status=",
                &handoff_source,
            )?
            .to_string(),
            verification_summary: required_engineer_evidence_value(
                &handoff.evidence_records,
                "verification-summary=",
                &handoff_source,
            )?
            .to_string(),
            terminal_bridge_context: TerminalBridgeContext::from_engineer_evidence(
                &handoff.evidence_records,
            )?,
            memory_record_count: handoff.memory_records.len(),
            evidence_record_count: handoff.evidence_records.len(),
        })
    }

    pub(super) fn print(&self) {
        println!("Probe mode: engineer-read");
        print_text("Engineer handoff source", &self.handoff_source);
        print_text("Mode boundary", ENGINEER_MODE_BOUNDARY);
        print_text("Identity", &self.identity);
        print_text("Selected base type", &self.selected_base_type);
        print_text("Topology", &self.topology);
        print_display("State root", self.state_root.display());
        print_text("Session phase", &self.session_phase);
        print_text("Objective metadata", &self.objective_metadata);
        print_display("Repo root", self.repo_root.display());
        print_text("Repo branch", &self.repo_branch);
        print_text("Repo head", &self.repo_head);
        print_text("Worktree dirty", &self.worktree_dirty);
        print_text("Changed files", &self.changed_files);
        println!("Active goals count: {}", self.active_goals.len());
        for (index, goal) in self.active_goals.iter().enumerate() {
            print_text(&format!("Active goal {}", index + 1), goal);
        }
        println!(
            "Carried meeting decisions: {}",
            self.carried_meeting_decisions.len()
        );
        for (index, decision) in self.carried_meeting_decisions.iter().enumerate() {
            print_text(&format!("Carried meeting decision {}", index + 1), decision);
        }
        print_terminal_bridge_section(
            self.terminal_bridge_context.as_ref(),
            self.terminal_bridge_context
                .as_ref()
                .map_or(SHARED_DEFAULT_STATE_ROOT_SOURCE, |context| {
                    context.continuity_source.as_str()
                }),
        );
        print_text("Selected action", &self.selected_action);
        print_text("Action plan", &self.action_plan);
        print_text("Verification steps", &self.verification_steps);
        print_text("Action status", &self.action_status);
        print_text(
            "Changed files after action",
            &self.changed_files_after_action,
        );
        print_text("Verification status", &self.verification_status);
        print_text("Verification summary", &self.verification_summary);
        println!("Memory records: {}", self.memory_record_count);
        println!("Evidence records: {}", self.evidence_record_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::EvidenceSource;
    use crate::session::{SessionId, SessionPhase};
    use crate::{
        BaseTypeId, EvidenceRecord, OperatingMode, RuntimeAddress, RuntimeHandoffSnapshot,
        RuntimeNodeId, RuntimeState, RuntimeTopology,
    };

    fn make_evidence(detail: &str) -> EvidenceRecord {
        EvidenceRecord {
            id: "ev-test".to_string(),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            phase: SessionPhase::Execution,
            detail: detail.to_string(),
            source: EvidenceSource::Runtime,
        }
    }

    #[allow(dead_code)]
    fn make_session() -> crate::session::SessionRecord {
        crate::session::SessionRecord {
            id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            mode: OperatingMode::Engineer,
            objective: "objective-metadata(chars=42, words=8, lines=2)".to_string(),
            phase: SessionPhase::Complete,
            selected_base_type: BaseTypeId::from("terminal-shell"),
            evidence_ids: vec![],
            memory_keys: vec![],
        }
    }

    fn engineer_evidence() -> Vec<EvidenceRecord> {
        vec![
            make_evidence("repo-root=/home/user/project"),
            make_evidence("repo-branch=main"),
            make_evidence("repo-head=abc123"),
            make_evidence("worktree-dirty=false"),
            make_evidence("changed-files=<none>"),
            make_evidence("active-goals=<none>"),
            make_evidence("carried-meeting-decisions=<none>"),
            make_evidence("selected-action=cargo-check"),
            make_evidence("action-plan=run cargo check"),
            make_evidence("action-verification-steps=verify clean build"),
            make_evidence("action-status=success"),
            make_evidence("changed-files-after-action=<none>"),
            make_evidence("verification-status=passed"),
            make_evidence("verification-summary=all checks passed"),
        ]
    }

    #[allow(dead_code)]
    fn make_handoff(
        session: Option<crate::session::SessionRecord>,
        evidence: Vec<EvidenceRecord>,
    ) -> RuntimeHandoffSnapshot {
        RuntimeHandoffSnapshot {
            exported_state: RuntimeState::Ready,
            identity_name: "simard-engineer".to_string(),
            selected_base_type: BaseTypeId::from("terminal-shell"),
            topology: RuntimeTopology::SingleProcess,
            source_runtime_node: RuntimeNodeId::new("test-node"),
            source_mailbox_address: RuntimeAddress::new("test-addr"),
            session,
            memory_records: vec![],
            evidence_records: evidence,
            copilot_submit_audit: None,
        }
    }

    #[test]
    fn engineer_read_view_struct_captures_all_fields() {
        let view = EngineerReadView {
            state_root: PathBuf::from("/test/state"),
            handoff_source: "test-source.json".to_string(),
            identity: "simard-engineer".to_string(),
            selected_base_type: "terminal-shell".to_string(),
            topology: "single-process".to_string(),
            session_phase: "Complete".to_string(),
            objective_metadata: "objective-metadata(chars=42, words=8, lines=2)".to_string(),
            repo_root: PathBuf::from("/home/user/project"),
            repo_branch: "main".to_string(),
            repo_head: "abc123".to_string(),
            worktree_dirty: "false".to_string(),
            changed_files: "<none>".to_string(),
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            selected_action: "cargo-check".to_string(),
            action_plan: "run cargo check".to_string(),
            verification_steps: "verify clean build".to_string(),
            action_status: "success".to_string(),
            changed_files_after_action: "<none>".to_string(),
            verification_status: "passed".to_string(),
            verification_summary: "all checks passed".to_string(),
            terminal_bridge_context: None,
            memory_record_count: 0,
            evidence_record_count: 0,
        };
        assert_eq!(view.identity, "simard-engineer");
        assert_eq!(view.repo_branch, "main");
        assert_eq!(view.verification_status, "passed");
    }

    #[test]
    fn parse_engineer_summary_list_empty_for_none() {
        let result = parse_engineer_summary_list("<none>", ", ");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_engineer_summary_list_splits_items() {
        let result = parse_engineer_summary_list("goal-a, goal-b", ", ");
        assert_eq!(result, vec!["goal-a", "goal-b"]);
    }

    #[test]
    fn parse_carried_meeting_decisions_returns_empty_for_none() {
        let result = parse_carried_meeting_decisions("<none>").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn required_evidence_value_finds_matching_prefix() {
        let records = engineer_evidence();
        let result =
            required_engineer_evidence_value(&records, "repo-branch=", "test-source").unwrap();
        assert_eq!(result, "main");
    }

    #[test]
    fn required_evidence_value_errors_on_missing() {
        let records = engineer_evidence();
        let result =
            required_engineer_evidence_value(&records, "nonexistent-field=", "test-source");
        assert!(result.is_err());
    }

    #[test]
    fn parse_engineer_summary_list_with_single_item() {
        let result = parse_engineer_summary_list("only-one-goal", ", ");
        assert_eq!(result, vec!["only-one-goal"]);
    }

    #[test]
    fn parse_carried_meeting_decisions_with_content() {
        let record = "agenda=Sprint review; updates=[Updated A]; decisions=[Use strategy X | Defer Y]; risks=[Risk 1]; next_steps=[Step 1]; open_questions=[Question 1]; goals=[p1:active:Goal title:Goal rationale]";
        let result = parse_carried_meeting_decisions(record).unwrap();
        assert_eq!(result, vec!["Use strategy X", "Defer Y"]);
    }
}
