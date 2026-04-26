use super::orchestration::*;
use super::types::{
    COPILOT_SUBMIT_BASE_TYPE, COPILOT_SUBMIT_FLOW_ASSET_PATH, COPILOT_SUBMIT_RUNTIME_NODE,
    CopilotSubmitFlowAsset, CopilotSubmitOutcome, StartupStatus, SubmitStatus,
};

use crate::base_types::BaseTypeId;
use crate::evidence::EvidenceSource;
use crate::handoff::CopilotSubmitAudit;
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

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
