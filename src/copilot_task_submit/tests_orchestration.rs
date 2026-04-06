use super::orchestration::*;
use super::types::{
    COPILOT_SUBMIT_BASE_TYPE, COPILOT_SUBMIT_FLOW_ASSET_PATH, COPILOT_SUBMIT_RUNTIME_NODE,
    CopilotSubmitFlowAsset, CopilotSubmitOutcome, StartupStatus, SubmitStatus,
};

use crate::evidence::EvidenceSource;
use crate::handoff::CopilotSubmitAudit;
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::base_types::BaseTypeId;

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
