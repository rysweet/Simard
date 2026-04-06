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
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
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
        scope: MemoryScope::SessionSummary,
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

pub(super) fn build_evidence_records(
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
