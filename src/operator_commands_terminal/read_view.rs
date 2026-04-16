use std::path::PathBuf;

use crate::operator_commands::{
    optional_terminal_evidence_value, print_display, print_text,
    render_redacted_objective_metadata, required_terminal_evidence_value, terminal_evidence_values,
    validated_terminal_read_artifacts,
};
use crate::terminal_engineer_bridge::{
    TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY, load_runtime_handoff_snapshot,
};
use crate::{
    CopilotSubmitAudit, FileBackedEvidenceStore, FileBackedMemoryStore, RuntimeHandoffSnapshot,
};

pub(super) struct TerminalReadView {
    pub(super) state_root: PathBuf,
    pub(super) handoff_source: String,
    pub(super) identity: String,
    pub(super) selected_base_type: String,
    pub(super) topology: String,
    pub(super) session_phase: String,
    pub(super) objective_metadata: String,
    pub(super) adapter_implementation: String,
    pub(super) shell: String,
    pub(super) working_directory: String,
    pub(super) command_count: String,
    pub(super) wait_count: String,
    pub(super) wait_timeout_seconds: String,
    pub(super) step_count: usize,
    pub(super) steps: Vec<String>,
    pub(super) checkpoints: Vec<String>,
    pub(super) last_output_line: Option<String>,
    pub(super) transcript_preview: String,
    pub(super) continuity_source: Option<String>,
    pub(super) copilot_submit_audit: Option<CopilotSubmitAudit>,
    pub(super) memory_record_count: usize,
    pub(super) evidence_record_count: usize,
}

impl TerminalReadView {
    pub(super) fn load(state_root: PathBuf) -> crate::SimardResult<Self> {
        let artifacts = validated_terminal_read_artifacts(&state_root)?;
        let handoff = load_runtime_handoff_snapshot(
            &crate::terminal_engineer_bridge::SelectedHandoffArtifact {
                path: artifacts.handoff_path.clone(),
                file_name: match artifacts.handoff_file_name.as_str() {
                    TERMINAL_HANDOFF_FILE_NAME => TERMINAL_HANDOFF_FILE_NAME,
                    _ => crate::terminal_engineer_bridge::COMPATIBILITY_HANDOFF_FILE_NAME,
                },
            },
            "terminal read",
        )?;

        FileBackedMemoryStore::try_new(artifacts.memory_path)?;
        FileBackedEvidenceStore::try_new(artifacts.evidence_path)?;

        Self::from_handoff(state_root, handoff, artifacts.handoff_file_name, None)
    }

    pub(super) fn from_handoff(
        state_root: PathBuf,
        handoff: RuntimeHandoffSnapshot,
        handoff_source: String,
        continuity_source: Option<String>,
    ) -> crate::SimardResult<Self> {
        let handoff_source_label = handoff_source.clone();
        let session = handoff.session.as_ref().ok_or_else(|| {
            crate::SimardError::InvalidHandoffSnapshot {
                field: "session".to_string(),
                reason: format!(
                    "terminal read requires {handoff_source} to contain a persisted session snapshot"
                )
                    .to_string(),
            }
        })?;

        Ok(Self {
            state_root,
            handoff_source,
            identity: handoff.identity_name,
            selected_base_type: handoff.selected_base_type.to_string(),
            topology: handoff.topology.to_string(),
            session_phase: session.phase.to_string(),
            objective_metadata: render_redacted_objective_metadata(&session.objective)?,
            adapter_implementation: required_terminal_evidence_value(
                &handoff.evidence_records,
                "backend-implementation=",
                &handoff_source_label,
            )?
            .to_string(),
            shell: required_terminal_evidence_value(
                &handoff.evidence_records,
                "shell=",
                &handoff_source_label,
            )?
            .to_string(),
            working_directory: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-working-directory=",
                &handoff_source_label,
            )?
            .to_string(),
            command_count: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-command-count=",
                &handoff_source_label,
            )?
            .to_string(),
            wait_count: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-wait-count=",
            )
            .unwrap_or("0")
            .to_string(),
            wait_timeout_seconds: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-wait-timeout-seconds=",
            )
            .unwrap_or("5")
            .to_string(),
            step_count: terminal_evidence_values(&handoff.evidence_records, "terminal-step-").len(),
            steps: terminal_evidence_values(&handoff.evidence_records, "terminal-step-"),
            checkpoints: terminal_evidence_values(
                &handoff.evidence_records,
                "terminal-checkpoint-",
            ),
            last_output_line: optional_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-last-output-line=",
            )
            .map(ToOwned::to_owned),
            transcript_preview: required_terminal_evidence_value(
                &handoff.evidence_records,
                "terminal-transcript-preview=",
                &handoff_source_label,
            )?
            .to_string(),
            continuity_source,
            copilot_submit_audit: handoff.copilot_submit_audit,
            memory_record_count: handoff.memory_records.len(),
            evidence_record_count: handoff.evidence_records.len(),
        })
    }

    pub(super) fn print(&self) {
        println!("Probe mode: terminal-read");
        self.print_terminal_audit_header();
        self.print_terminal_audit_body();
    }

    pub(super) fn print_terminal_run(
        &self,
        adapter_capabilities: &[String],
        execution_summary: &str,
        reflection_summary: &str,
    ) {
        println!("Probe mode: terminal-run");
        self.print_terminal_audit_header();
        print_text("Adapter capabilities", adapter_capabilities.join(", "));
        self.print_terminal_audit_body();
        print_text("Execution summary", execution_summary);
        print_text("Reflection summary", reflection_summary);
    }

    fn print_terminal_audit_header(&self) {
        print_text("Terminal handoff source", &self.handoff_source);
        print_text("Mode boundary", TERMINAL_MODE_BOUNDARY);
        print_text("Identity", &self.identity);
        print_text("Selected base type", &self.selected_base_type);
        print_text("Topology", &self.topology);
        print_display("State root", self.state_root.display());
        print_text("Session phase", &self.session_phase);
        print_text("Objective metadata", &self.objective_metadata);
        print_text("Adapter implementation", &self.adapter_implementation);
    }

    fn print_terminal_audit_body(&self) {
        print_text("Shell", &self.shell);
        print_text("Working directory", &self.working_directory);
        print_text("Terminal command count", &self.command_count);
        print_text("Terminal wait count", &self.wait_count);
        print_text("Terminal wait timeout seconds", &self.wait_timeout_seconds);
        println!("Terminal steps count: {}", self.step_count);
        if self.steps.is_empty() {
            println!("Terminal steps: <none>");
        } else {
            for (index, step) in self.steps.iter().enumerate() {
                print_text(&format!("Terminal step {}", index + 1), step);
            }
        }
        println!("Terminal checkpoints count: {}", self.checkpoints.len());
        if self.checkpoints.is_empty() {
            println!("Terminal checkpoints: <none>");
        } else {
            for (index, checkpoint) in self.checkpoints.iter().enumerate() {
                print_text(&format!("Terminal checkpoint {}", index + 1), checkpoint);
            }
        }
        if let Some(last_output_line) = &self.last_output_line {
            print_text("Terminal last output line", last_output_line);
        }
        print_text("Terminal transcript preview", &self.transcript_preview);
        if let Some(audit) = &self.copilot_submit_audit {
            print_text("Copilot flow asset", &audit.flow_asset);
            print_text("Copilot submit outcome", &audit.outcome);
            if let Some(reason_code) = &audit.reason_code {
                print_text("Copilot reason code", reason_code);
            }
            print_text("Copilot payload id", &audit.payload_id);
        }
        if let Some(continuity_source) = &self.continuity_source {
            print_text("Next step source", continuity_source);
        }
        println!("Next steps count: 1");
        println!(
            "Next step 1: run 'simard engineer run <topology> <workspace-root> <objective> {}' to start the separate repo-grounded bounded loop",
            self.state_root.display()
        );
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

    fn required_evidence() -> Vec<EvidenceRecord> {
        vec![
            make_evidence("backend-implementation=test-adapter"),
            make_evidence("shell=/bin/bash"),
            make_evidence("terminal-working-directory=/home/user/project"),
            make_evidence("terminal-command-count=5"),
            make_evidence("terminal-transcript-preview=$ echo hello"),
        ]
    }

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
    fn from_handoff_succeeds_with_required_fields() {
        let handoff = make_handoff(Some(make_session()), required_evidence());
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        );
        assert!(view.is_ok(), "expected success, got: {:?}", view.err());
    }

    #[test]
    fn from_handoff_extracts_identity_and_topology() {
        let handoff = make_handoff(Some(make_session()), required_evidence());
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        )
        .unwrap();
        assert_eq!(view.identity, "simard-engineer");
        assert_eq!(view.selected_base_type, "terminal-shell");
        assert_eq!(view.topology, "single-process");
    }

    #[test]
    fn from_handoff_fails_without_session() {
        let handoff = make_handoff(None, required_evidence());
        let result = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        );
        assert!(result.is_err(), "should fail without session");
    }

    #[test]
    fn from_handoff_uses_default_wait_values() {
        let handoff = make_handoff(Some(make_session()), required_evidence());
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        )
        .unwrap();
        assert_eq!(view.wait_count, "0");
        assert_eq!(view.wait_timeout_seconds, "5");
    }

    #[test]
    fn from_handoff_captures_step_and_checkpoint_counts() {
        let mut evidence = required_evidence();
        evidence.push(make_evidence("terminal-step-1=run cargo check"));
        evidence.push(make_evidence("terminal-step-2=run cargo test"));
        evidence.push(make_evidence("terminal-checkpoint-1=tests pass"));
        let handoff = make_handoff(Some(make_session()), evidence);
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        )
        .unwrap();
        assert_eq!(view.step_count, 2);
        assert_eq!(view.steps.len(), 2);
        assert_eq!(view.checkpoints.len(), 1);
    }

    #[test]
    fn from_handoff_captures_copilot_submit_audit() {
        let mut handoff = make_handoff(Some(make_session()), required_evidence());
        handoff.copilot_submit_audit = Some(CopilotSubmitAudit {
            flow_asset: "test-flow".to_string(),
            payload_id: "payload-1".to_string(),
            outcome: "success".to_string(),
            reason_code: Some("ok".to_string()),
            ordered_steps: vec!["step1".to_string()],
            observed_checkpoints: vec![],
            last_meaningful_output_line: None,
            transcript_preview: "preview".to_string(),
        });
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        )
        .unwrap();
        assert!(view.copilot_submit_audit.is_some());
        assert_eq!(
            view.copilot_submit_audit.as_ref().unwrap().flow_asset,
            "test-flow"
        );
    }

    #[test]
    fn from_handoff_fails_when_required_evidence_missing() {
        // Omit "backend-implementation=" — this is a required field.
        let evidence = vec![
            make_evidence("shell=/bin/bash"),
            make_evidence("terminal-working-directory=/home/user/project"),
            make_evidence("terminal-command-count=5"),
            make_evidence("terminal-transcript-preview=$ echo hello"),
        ];
        let handoff = make_handoff(Some(make_session()), evidence);
        let result = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        );
        assert!(
            result.is_err(),
            "should fail when backend-implementation is absent"
        );
    }

    #[test]
    fn from_handoff_session_phase_is_captured() {
        let handoff = make_handoff(Some(make_session()), required_evidence());
        let view = TerminalReadView::from_handoff(
            PathBuf::from("/test/state"),
            handoff,
            "test-handoff.json".to_string(),
            None,
        )
        .unwrap();
        // The session fixture uses SessionPhase::Complete.
        assert!(!view.session_phase.is_empty());
        assert_eq!(view.session_phase.to_lowercase(), "complete");
    }
}
