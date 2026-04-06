use std::path::Path;
use std::time::Instant;

use super::transcript::{
    classify_startup, classify_startup_timeout, classify_submit, classify_submit_timeout,
    copilot_last_meaningful_output_line, copilot_transcript_preview, copilot_visible_fragments,
    scan_transcript,
};
use super::types::{
    COPILOT_SUBMIT_ACTION, COPILOT_SUBMIT_ADAPTER_IDENTITY, COPILOT_SUBMIT_BASE_TYPE,
    COPILOT_SUBMIT_FLOW_ASSET_PATH, COPILOT_SUBMIT_MEMORY_KEY, COPILOT_SUBMIT_RUNTIME_NODE,
    COPILOT_SUBMIT_SHELL_LABEL, CopilotSubmitFlowAsset, CopilotSubmitOutcome, CopilotSubmitReport,
    POLL_INTERVAL, StartupStatus, SubmitStatus,
};
use crate::base_types::BaseTypeId;
use crate::copilot_status_probe::{CopilotStatusProbeResult, probe_local_copilot_status};
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore};
use crate::handoff::{CopilotSubmitAudit, RuntimeHandoffSnapshot};
use crate::identity::OperatingMode;
use crate::memory::{CognitiveMemoryType, FileBackedMemoryStore, MemoryRecord, MemoryStore};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::terminal_engineer_bridge::{ScopedHandoffMode, persist_handoff_artifacts};
use crate::terminal_session::{
    PtyTerminalSession, TerminalSessionCapture, compact_terminal_evidence_value,
};

pub(super) struct StartupObservation {
    pub(super) status: StartupStatus,
    pub(super) ordered_steps: Vec<String>,
    pub(super) observed_checkpoints: Vec<String>,
    pub(super) terminate: bool,
}

pub(super) struct SubmitObservation {
    pub(super) status: SubmitStatus,
    pub(super) ordered_steps: Vec<String>,
    pub(super) observed_checkpoints: Vec<String>,
    pub(super) terminate: bool,
}

pub(super) struct PersistReportInputs<'a> {
    pub(super) state_root: &'a Path,
    pub(super) topology: RuntimeTopology,
    pub(super) flow: &'a CopilotSubmitFlowAsset,
    pub(super) ordered_steps: Vec<String>,
    pub(super) observed_checkpoints: Vec<String>,
    pub(super) transcript: &'a str,
    pub(super) outcome: CopilotSubmitOutcome,
    pub(super) reason_code: Option<String>,
    pub(super) working_directory: &'a Path,
}

pub(super) fn observe_startup(
    session: &mut PtyTerminalSession,
    flow: &CopilotSubmitFlowAsset,
) -> SimardResult<StartupObservation> {
    let start = Instant::now();
    let timeout = flow.wait_timeout();
    loop {
        let transcript = session.read_transcript()?;
        let scan = scan_transcript(&transcript, flow);
        let exited = session.status()?.is_some();
        let status = classify_startup(&scan, exited);
        let ordered_steps = scan.startup_ordered_steps(flow);
        let observed_checkpoints = scan.observed_checkpoints();
        match status {
            StartupStatus::Ready => {
                return Ok(StartupObservation {
                    status,
                    ordered_steps,
                    observed_checkpoints,
                    terminate: false,
                });
            }
            StartupStatus::Unsupported(reason_code) => {
                return Ok(StartupObservation {
                    status: StartupStatus::Unsupported(reason_code),
                    ordered_steps,
                    observed_checkpoints,
                    terminate: !exited,
                });
            }
            StartupStatus::Wait => {
                if start.elapsed() >= timeout {
                    if let Some(reason_code) = classify_startup_timeout(&scan) {
                        return Ok(StartupObservation {
                            status: StartupStatus::Unsupported(reason_code),
                            ordered_steps,
                            observed_checkpoints,
                            terminate: !exited,
                        });
                    }
                    return Err(SimardError::ActionExecutionFailed {
                        action: COPILOT_SUBMIT_ACTION.to_string(),
                        reason: format!(
                            "runtime-failure: local PTY observation timed out after {}s before copilot-submit reached a classified startup state",
                            flow.wait_timeout_seconds
                        ),
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

pub(super) fn observe_submit(
    session: &mut PtyTerminalSession,
    flow: &CopilotSubmitFlowAsset,
) -> SimardResult<SubmitObservation> {
    let start = Instant::now();
    let timeout = flow.wait_timeout();
    loop {
        let transcript = session.read_transcript()?;
        let scan = scan_transcript(&transcript, flow);
        let exited = session.status()?.is_some();
        let ordered_steps = scan.submit_ordered_steps(flow);
        let observed_checkpoints = scan.observed_checkpoints();
        match classify_submit(&scan, exited) {
            SubmitStatus::Success => {
                return Ok(SubmitObservation {
                    status: SubmitStatus::Success,
                    ordered_steps,
                    observed_checkpoints,
                    terminate: true,
                });
            }
            SubmitStatus::Unsupported(reason_code) => {
                return Ok(SubmitObservation {
                    status: SubmitStatus::Unsupported(reason_code),
                    ordered_steps,
                    observed_checkpoints,
                    terminate: !exited,
                });
            }
            SubmitStatus::Wait => {
                if start.elapsed() >= timeout {
                    return Ok(SubmitObservation {
                        status: SubmitStatus::Unsupported(classify_submit_timeout(&scan, flow)),
                        ordered_steps,
                        observed_checkpoints,
                        terminate: true,
                    });
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

pub(super) fn persist_report(inputs: PersistReportInputs<'_>) -> SimardResult<CopilotSubmitReport> {
    let PersistReportInputs {
        state_root,
        topology,
        flow,
        ordered_steps,
        observed_checkpoints,
        transcript,
        outcome,
        reason_code,
        working_directory,
    } = inputs;
    let session_ids = UuidSessionIdGenerator;
    let mut session = SessionRecord::new(
        OperatingMode::Engineer,
        flow.payload.clone(),
        BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
        &session_ids,
    );
    for phase in [
        SessionPhase::Preparation,
        SessionPhase::Planning,
        SessionPhase::Execution,
        SessionPhase::Reflection,
        SessionPhase::Persistence,
        SessionPhase::Complete,
    ] {
        session.advance(phase)?;
    }

    let visible_fragments = copilot_visible_fragments(transcript, flow);
    let last_meaningful_output_line = copilot_last_meaningful_output_line(&visible_fragments, flow);
    let transcript_preview = copilot_transcript_preview(&visible_fragments, flow);
    let audit = CopilotSubmitAudit {
        flow_asset: COPILOT_SUBMIT_FLOW_ASSET_PATH.to_string(),
        payload_id: flow.payload_id.clone(),
        outcome: outcome.as_str().to_string(),
        reason_code: reason_code.clone(),
        ordered_steps: ordered_steps.clone(),
        observed_checkpoints: observed_checkpoints.clone(),
        last_meaningful_output_line: last_meaningful_output_line.clone(),
        transcript_preview: transcript_preview.clone(),
    };
    let report = CopilotSubmitReport {
        selected_base_type: COPILOT_SUBMIT_BASE_TYPE.to_string(),
        flow_asset: audit.flow_asset.clone(),
        outcome,
        reason_code,
        payload_id: audit.payload_id.clone(),
        ordered_steps: audit.ordered_steps.clone(),
        observed_checkpoints: audit.observed_checkpoints.clone(),
        last_meaningful_output_line: audit.last_meaningful_output_line.clone(),
        transcript_preview: audit.transcript_preview.clone(),
    };

    let runtime_node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
    let runtime_address = RuntimeAddress::local(&runtime_node);
    let memory_record = MemoryRecord {
        key: format!("{}-{COPILOT_SUBMIT_MEMORY_KEY}", session.id),
        memory_type: CognitiveMemoryType::Episodic,
        value: format!(
            "copilot-submit outcome={} payload_id={} reason_code={}",
            report.outcome.as_str(),
            report.payload_id,
            report.reason_code.as_deref().unwrap_or("<none>")
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Complete,
    };
    session.attach_memory(memory_record.key.clone());

    let evidence_records = build_evidence_records(
        &session,
        flow,
        &ordered_steps,
        &audit,
        working_directory,
        &runtime_address,
    );
    for record in &evidence_records {
        session.attach_evidence(record.id.clone());
    }

    FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?
        .put(memory_record.clone())?;
    let evidence_store =
        FileBackedEvidenceStore::try_new(state_root.join("evidence_records.json"))?;
    for record in &evidence_records {
        evidence_store.record(record.clone())?;
    }

    let snapshot = RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Stopped,
        identity_name: "simard-engineer".to_string(),
        selected_base_type: BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
        topology,
        source_runtime_node: runtime_node,
        source_mailbox_address: runtime_address,
        session: Some(session.redacted_for_handoff()),
        memory_records: vec![memory_record],
        evidence_records: evidence_records.clone(),
        copilot_submit_audit: Some(audit),
    };
    persist_handoff_artifacts(state_root, ScopedHandoffMode::Terminal, &snapshot)?;

    Ok(report)
}

fn build_evidence_records(
    session: &SessionRecord,
    flow: &CopilotSubmitFlowAsset,
    ordered_steps: &[String],
    audit: &CopilotSubmitAudit,
    working_directory: &Path,
    runtime_address: &RuntimeAddress,
) -> Vec<EvidenceRecord> {
    let mut details = vec![
        format!("selected-base-type={COPILOT_SUBMIT_BASE_TYPE}"),
        format!("backend-implementation={COPILOT_SUBMIT_ADAPTER_IDENTITY}"),
        format!("shell={COPILOT_SUBMIT_SHELL_LABEL}"),
        format!(
            "terminal-working-directory={}",
            compact_terminal_evidence_value(&working_directory.display().to_string(), 160)
        ),
        "terminal-command-count=1".to_string(),
        format!(
            "terminal-wait-count={}",
            3 + usize::from(flow.post_submit_checkpoint.is_some())
        ),
        format!(
            "terminal-wait-timeout-seconds={}",
            flow.wait_timeout_seconds
        ),
        format!("terminal-step-count={}", ordered_steps.len()),
        format!("terminal-transcript-preview={}", audit.transcript_preview),
        format!("runtime-node={COPILOT_SUBMIT_RUNTIME_NODE}"),
        format!("mailbox-address={runtime_address}"),
        format!("copilot-flow-asset={}", audit.flow_asset),
        format!("copilot-submit-outcome={}", audit.outcome),
        format!("copilot-payload-id={}", audit.payload_id),
    ];
    if let Some(reason_code) = &audit.reason_code {
        details.push(format!("copilot-reason-code={reason_code}"));
    }
    if let Some(last_meaningful_output_line) = &audit.last_meaningful_output_line {
        details.push(format!(
            "terminal-last-output-line={last_meaningful_output_line}"
        ));
    }
    for (index, step) in ordered_steps.iter().enumerate() {
        details.push(format!(
            "terminal-step-{}={}",
            index + 1,
            compact_terminal_evidence_value(step, 160)
        ));
    }
    for (index, checkpoint) in audit.observed_checkpoints.iter().enumerate() {
        details.push(format!(
            "terminal-checkpoint-{}={}",
            index + 1,
            compact_terminal_evidence_value(checkpoint, 160)
        ));
    }

    details
        .into_iter()
        .enumerate()
        .map(|(index, detail)| EvidenceRecord {
            id: format!("{}-copilot-submit-evidence-{}", session.id, index + 1),
            session_id: session.id.clone(),
            phase: SessionPhase::Complete,
            detail,
            source: EvidenceSource::BaseType(BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE)),
        })
        .collect()
}

pub(super) fn finalize_session(
    mut session: PtyTerminalSession,
    terminate: bool,
) -> SimardResult<TerminalSessionCapture> {
    if terminate {
        session.terminate()?;
    }
    session.finish()
}

pub(super) fn ensure_copilot_submit_is_launchable() -> SimardResult<()> {
    match probe_local_copilot_status() {
        CopilotStatusProbeResult::Available { .. } => Ok(()),
        CopilotStatusProbeResult::Unavailable {
            reason_code,
            detail,
        }
        | CopilotStatusProbeResult::Unsupported {
            reason_code,
            detail,
        } => Err(SimardError::ActionExecutionFailed {
            action: COPILOT_SUBMIT_ACTION.to_string(),
            reason: format!("runtime-failure: {reason_code}: {detail}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_flow_asset() -> CopilotSubmitFlowAsset {
        CopilotSubmitFlowAsset {
            launch_command: "echo test".to_string(),
            working_directory: PathBuf::from("/test/wd"),
            wait_timeout_seconds: 30,
            startup_banner: "banner".to_string(),
            guidance_checkpoint: "guidance".to_string(),
            submit_hint: "hint".to_string(),
            post_submit_checkpoint: None,
            trust_prompt: None,
            wrapper_error_signal: None,
            workflow_noise_signals: vec![],
            payload_id: "payload-001".to_string(),
            payload: "test payload".to_string(),
        }
    }

    fn test_audit() -> CopilotSubmitAudit {
        CopilotSubmitAudit {
            flow_asset: COPILOT_SUBMIT_FLOW_ASSET_PATH.to_string(),
            payload_id: "payload-001".to_string(),
            outcome: "success".to_string(),
            reason_code: None,
            ordered_steps: vec!["step-1".to_string()],
            observed_checkpoints: vec![],
            last_meaningful_output_line: None,
            transcript_preview: "preview text".to_string(),
        }
    }

    fn test_session() -> SessionRecord {
        SessionRecord::new(
            OperatingMode::Engineer,
            "test objective",
            BaseTypeId::new(COPILOT_SUBMIT_BASE_TYPE),
            &UuidSessionIdGenerator,
        )
    }

    fn test_runtime_address() -> (RuntimeNodeId, RuntimeAddress) {
        let node = RuntimeNodeId::new(COPILOT_SUBMIT_RUNTIME_NODE);
        let addr = RuntimeAddress::local(&node);
        (node, addr)
    }

    // ── StartupObservation construction ─────────────────────────────────

    #[test]
    fn startup_observation_ready() {
        let obs = StartupObservation {
            status: StartupStatus::Ready,
            ordered_steps: vec!["step1".to_string()],
            observed_checkpoints: vec!["cp1".to_string()],
            terminate: false,
        };
        assert!(matches!(obs.status, StartupStatus::Ready));
        assert!(!obs.terminate);
        assert_eq!(obs.ordered_steps.len(), 1);
        assert_eq!(obs.observed_checkpoints.len(), 1);
    }

    #[test]
    fn startup_observation_wait() {
        let obs = StartupObservation {
            status: StartupStatus::Wait,
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            terminate: false,
        };
        assert!(matches!(obs.status, StartupStatus::Wait));
    }

    #[test]
    fn startup_observation_unsupported() {
        let obs = StartupObservation {
            status: StartupStatus::Unsupported("test-reason"),
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            terminate: true,
        };
        assert!(matches!(
            obs.status,
            StartupStatus::Unsupported("test-reason")
        ));
        assert!(obs.terminate);
    }

    // ── SubmitObservation construction ───────────────────────────────────

    #[test]
    fn submit_observation_success() {
        let obs = SubmitObservation {
            status: SubmitStatus::Success,
            ordered_steps: vec!["s1".to_string(), "s2".to_string()],
            observed_checkpoints: vec!["c1".to_string()],
            terminate: true,
        };
        assert!(matches!(obs.status, SubmitStatus::Success));
        assert!(obs.terminate);
        assert_eq!(obs.ordered_steps.len(), 2);
    }

    #[test]
    fn submit_observation_wait() {
        let obs = SubmitObservation {
            status: SubmitStatus::Wait,
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            terminate: false,
        };
        assert!(matches!(obs.status, SubmitStatus::Wait));
        assert!(!obs.terminate);
    }

    #[test]
    fn submit_observation_unsupported() {
        let obs = SubmitObservation {
            status: SubmitStatus::Unsupported("no-binary"),
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            terminate: false,
        };
        assert!(matches!(obs.status, SubmitStatus::Unsupported("no-binary")));
    }

    // ── PersistReportInputs construction ────────────────────────────────

    #[test]
    fn persist_report_inputs_construction() {
        let flow = test_flow_asset();
        let wd = PathBuf::from("/test/wd");
        let dir = tempfile::TempDir::new().unwrap();
        let inputs = PersistReportInputs {
            state_root: dir.path(),
            topology: RuntimeTopology::SingleProcess,
            flow: &flow,
            ordered_steps: vec!["s1".to_string()],
            observed_checkpoints: vec!["c1".to_string()],
            transcript: "test transcript",
            outcome: CopilotSubmitOutcome::Success,
            reason_code: None,
            working_directory: &wd,
        };
        assert!(matches!(inputs.outcome, CopilotSubmitOutcome::Success));
        assert!(inputs.reason_code.is_none());
        assert_eq!(inputs.ordered_steps.len(), 1);
    }

    // ── CopilotSubmitOutcome ────────────────────────────────────────────

    #[test]
    fn copilot_submit_outcome_as_str_success() {
        assert_eq!(CopilotSubmitOutcome::Success.as_str(), "success");
    }

    #[test]
    fn copilot_submit_outcome_as_str_unsupported() {
        assert_eq!(CopilotSubmitOutcome::Unsupported.as_str(), "unsupported");
    }

    // ── build_evidence_records ──────────────────────────────────────────

    #[test]
    fn build_evidence_records_basic() {
        let session = test_session();
        let flow = test_flow_asset();
        let audit = test_audit();
        let working_directory = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(
            &session,
            &flow,
            &["step-1".to_string()],
            &audit,
            &working_directory,
            &addr,
        );

        assert!(!records.is_empty());
        // First record should contain selected base type
        assert!(records[0].detail.contains(COPILOT_SUBMIT_BASE_TYPE));
        // All records should be in Complete phase
        for record in &records {
            assert!(matches!(record.phase, SessionPhase::Complete));
        }
    }

    #[test]
    fn build_evidence_records_includes_reason_code_when_present() {
        let session = test_session();
        let flow = test_flow_asset();
        let mut audit = test_audit();
        audit.reason_code = Some("startup-failed".to_string());
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records =
            build_evidence_records(&session, &flow, &["step-1".to_string()], &audit, &wd, &addr);

        let has_reason = records
            .iter()
            .any(|r| r.detail.contains("copilot-reason-code=startup-failed"));
        assert!(has_reason, "expected reason code in evidence records");
    }

    #[test]
    fn build_evidence_records_includes_last_output_line_when_present() {
        let session = test_session();
        let flow = test_flow_asset();
        let mut audit = test_audit();
        audit.last_meaningful_output_line = Some("last output here".to_string());
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        let has_last_line = records.iter().any(|r| {
            r.detail
                .contains("terminal-last-output-line=last output here")
        });
        assert!(
            has_last_line,
            "expected last output line in evidence records"
        );
    }

    #[test]
    fn build_evidence_records_indexes_steps_from_one() {
        let session = test_session();
        let flow = test_flow_asset();
        let audit = test_audit();
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();
        let steps = vec![
            "step-a".to_string(),
            "step-b".to_string(),
            "step-c".to_string(),
        ];

        let records = build_evidence_records(&session, &flow, &steps, &audit, &wd, &addr);

        let has_step_1 = records
            .iter()
            .any(|r| r.detail.contains("terminal-step-1="));
        let has_step_3 = records
            .iter()
            .any(|r| r.detail.contains("terminal-step-3="));
        assert!(has_step_1, "expected terminal-step-1");
        assert!(has_step_3, "expected terminal-step-3");
    }

    #[test]
    fn build_evidence_records_indexes_checkpoints_from_one() {
        let session = test_session();
        let flow = test_flow_asset();
        let mut audit = test_audit();
        audit.observed_checkpoints = vec!["cp-a".to_string(), "cp-b".to_string()];
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        let has_cp_1 = records
            .iter()
            .any(|r| r.detail.contains("terminal-checkpoint-1="));
        let has_cp_2 = records
            .iter()
            .any(|r| r.detail.contains("terminal-checkpoint-2="));
        assert!(has_cp_1);
        assert!(has_cp_2);
    }

    #[test]
    fn build_evidence_records_wait_count_without_post_submit() {
        let session = test_session();
        let flow = test_flow_asset(); // post_submit_checkpoint is None
        let audit = test_audit();
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        let wait_record = records
            .iter()
            .find(|r| r.detail.contains("terminal-wait-count="));
        assert!(wait_record.is_some());
        assert!(
            wait_record
                .unwrap()
                .detail
                .contains("terminal-wait-count=3"),
            "expected 3 waits without post-submit checkpoint"
        );
    }

    #[test]
    fn build_evidence_records_wait_count_with_post_submit() {
        let session = test_session();
        let mut flow = test_flow_asset();
        flow.post_submit_checkpoint = Some("post-check".to_string());
        let audit = test_audit();
        let wd = PathBuf::from("/test/dir");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        let wait_record = records
            .iter()
            .find(|r| r.detail.contains("terminal-wait-count="));
        assert!(wait_record.is_some());
        assert!(
            wait_record
                .unwrap()
                .detail
                .contains("terminal-wait-count=4"),
            "expected 4 waits with post-submit checkpoint"
        );
    }

    #[test]
    fn build_evidence_record_ids_are_unique() {
        let session = test_session();
        let flow = test_flow_asset();
        let audit = test_audit();
        let wd = PathBuf::from("/test");
        let (_node, addr) = test_runtime_address();

        let records =
            build_evidence_records(&session, &flow, &["s1".to_string()], &audit, &wd, &addr);

        let ids: std::collections::HashSet<_> = records.iter().map(|r| &r.id).collect();
        assert_eq!(
            ids.len(),
            records.len(),
            "evidence record IDs should be unique"
        );
    }

    #[test]
    fn build_evidence_records_contains_runtime_node() {
        let session = test_session();
        let flow = test_flow_asset();
        let audit = test_audit();
        let wd = PathBuf::from("/test");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        let has_runtime_node = records.iter().any(|r| {
            r.detail
                .contains(&format!("runtime-node={COPILOT_SUBMIT_RUNTIME_NODE}"))
        });
        assert!(has_runtime_node, "expected runtime node in evidence");
    }

    #[test]
    fn build_evidence_records_all_sources_are_base_type() {
        let session = test_session();
        let flow = test_flow_asset();
        let audit = test_audit();
        let wd = PathBuf::from("/test");
        let (_node, addr) = test_runtime_address();

        let records = build_evidence_records(&session, &flow, &[], &audit, &wd, &addr);

        for record in &records {
            assert!(
                matches!(&record.source, EvidenceSource::BaseType(bt) if bt.as_str() == COPILOT_SUBMIT_BASE_TYPE),
                "expected all evidence sources to be BaseType"
            );
        }
    }

    // ── ensure_copilot_submit_is_launchable ─────────────────────────────

    #[test]
    fn ensure_copilot_submit_is_launchable_does_not_panic() {
        let result = ensure_copilot_submit_is_launchable();
        match result {
            Ok(()) => {} // amplihack happens to be available
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("runtime-failure") || msg.contains("copilot-submit"),
                    "error should mention runtime failure: {msg}"
                );
            }
        }
    }

    // ── persist_report (integration-style with tempdir) ─────────────────

    #[test]
    fn persist_report_creates_success_report() {
        let dir = tempfile::TempDir::new().unwrap();
        let flow = test_flow_asset();
        let wd = PathBuf::from("/test/wd");
        let inputs = PersistReportInputs {
            state_root: dir.path(),
            topology: RuntimeTopology::SingleProcess,
            flow: &flow,
            ordered_steps: vec!["step-1".to_string()],
            observed_checkpoints: vec!["cp-1".to_string()],
            transcript: "some transcript content",
            outcome: CopilotSubmitOutcome::Success,
            reason_code: None,
            working_directory: &wd,
        };

        let report = persist_report(inputs).unwrap();
        assert_eq!(report.outcome.as_str(), "success");
        assert!(report.reason_code.is_none());
        assert_eq!(report.payload_id, "payload-001");
        assert_eq!(report.selected_base_type, COPILOT_SUBMIT_BASE_TYPE);
        assert_eq!(report.ordered_steps, vec!["step-1"]);
        assert_eq!(report.observed_checkpoints, vec!["cp-1"]);
    }

    #[test]
    fn persist_report_creates_unsupported_report() {
        let dir = tempfile::TempDir::new().unwrap();
        let flow = test_flow_asset();
        let wd = PathBuf::from("/test/wd");
        let inputs = PersistReportInputs {
            state_root: dir.path(),
            topology: RuntimeTopology::SingleProcess,
            flow: &flow,
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            transcript: "",
            outcome: CopilotSubmitOutcome::Unsupported,
            reason_code: Some("startup-error".to_string()),
            working_directory: &wd,
        };

        let report = persist_report(inputs).unwrap();
        assert_eq!(report.outcome.as_str(), "unsupported");
        assert_eq!(report.reason_code, Some("startup-error".to_string()));
    }

    #[test]
    fn persist_report_writes_memory_and_evidence_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let flow = test_flow_asset();
        let wd = PathBuf::from("/test/wd");
        let inputs = PersistReportInputs {
            state_root: dir.path(),
            topology: RuntimeTopology::SingleProcess,
            flow: &flow,
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            transcript: "test",
            outcome: CopilotSubmitOutcome::Success,
            reason_code: None,
            working_directory: &wd,
        };

        persist_report(inputs).unwrap();
        assert!(dir.path().join("memory_records.json").exists());
        assert!(dir.path().join("evidence_records.json").exists());
    }

    #[test]
    fn persist_report_flow_asset_matches_constant() {
        let dir = tempfile::TempDir::new().unwrap();
        let flow = test_flow_asset();
        let wd = PathBuf::from("/test/wd");
        let inputs = PersistReportInputs {
            state_root: dir.path(),
            topology: RuntimeTopology::SingleProcess,
            flow: &flow,
            ordered_steps: vec![],
            observed_checkpoints: vec![],
            transcript: "",
            outcome: CopilotSubmitOutcome::Success,
            reason_code: None,
            working_directory: &wd,
        };

        let report = persist_report(inputs).unwrap();
        assert_eq!(report.flow_asset, COPILOT_SUBMIT_FLOW_ASSET_PATH);
    }

    // ── CopilotSubmitFlowAsset methods ──────────────────────────────────

    #[test]
    fn flow_asset_wait_timeout_converts_seconds() {
        let flow = test_flow_asset();
        assert_eq!(flow.wait_timeout(), std::time::Duration::from_secs(30));
    }

    #[test]
    fn flow_asset_launch_step_format() {
        let flow = test_flow_asset();
        let step = flow.launch_step();
        assert!(
            step.contains("echo test"),
            "launch step should contain the command"
        );
    }

    #[test]
    fn flow_asset_post_submit_step_none_when_no_checkpoint() {
        let flow = test_flow_asset();
        assert!(flow.post_submit_step().is_none());
    }

    #[test]
    fn flow_asset_post_submit_step_some_when_checkpoint_present() {
        let mut flow = test_flow_asset();
        flow.post_submit_checkpoint = Some("done-marker".to_string());
        assert!(flow.post_submit_step().is_some());
    }
}
