use std::path::PathBuf;

use crate::session::{SessionId, SessionPhase};
use crate::{CopilotSubmitAudit, MemoryRecord};

use super::read_view::TerminalReadView;
use super::test_support::{
    make_evidence, make_handoff, make_session_record, required_evidence_records,
};

// --- TerminalReadView::from_handoff success ---

#[test]
fn from_handoff_succeeds_with_valid_data() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        Some("shared default state-root".to_string()),
    );
    assert!(
        view.is_ok(),
        "from_handoff should succeed: {:?}",
        view.err()
    );
}

#[test]
fn from_handoff_extracts_identity() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.identity, "simard-engineer");
}

#[test]
fn from_handoff_extracts_topology() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.topology, "single-process");
}

#[test]
fn from_handoff_extracts_session_phase() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.session_phase, "complete");
}

#[test]
fn from_handoff_extracts_evidence_values() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.adapter_implementation, "test-adapter");
    assert_eq!(view.shell, "/bin/bash");
    assert_eq!(view.working_directory, "/home/user/project");
    assert_eq!(view.command_count, "5");
    assert_eq!(view.transcript_preview, "$ echo hello");
}

#[test]
fn from_handoff_defaults_optional_values() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.wait_count, "0");
    assert_eq!(view.wait_timeout_seconds, "5");
    assert!(view.last_output_line.is_none());
}

#[test]
fn from_handoff_extracts_optional_values_when_present() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-wait-count=3"));
    evidence.push(make_evidence("terminal-wait-timeout-seconds=15"));
    evidence.push(make_evidence("terminal-last-output-line=done"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.wait_count, "3");
    assert_eq!(view.wait_timeout_seconds, "15");
    assert_eq!(view.last_output_line.as_deref(), Some("done"));
}

#[test]
fn from_handoff_collects_steps() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-step-1=run tests"));
    evidence.push(make_evidence("terminal-step-2=check results"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.step_count, 2);
    assert_eq!(view.steps.len(), 2);
}

#[test]
fn from_handoff_collects_checkpoints() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-checkpoint-1=build passed"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.checkpoints.len(), 1);
}

#[test]
fn from_handoff_empty_steps_and_checkpoints() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.step_count, 0);
    assert!(view.steps.is_empty());
    assert!(view.checkpoints.is_empty());
}

#[test]
fn from_handoff_preserves_continuity_source() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        Some("shared explicit state-root".to_string()),
    )
    .unwrap();
    assert_eq!(
        view.continuity_source.as_deref(),
        Some("shared explicit state-root")
    );
}

#[test]
fn from_handoff_preserves_state_root() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/my/state/root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.state_root, PathBuf::from("/my/state/root"));
}

#[test]
fn from_handoff_preserves_handoff_source() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "custom_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.handoff_source, "custom_handoff.json");
}

#[test]
fn from_handoff_tracks_record_counts() {
    let mut handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    handoff.memory_records.push(MemoryRecord {
        key: "test-key".to_string(),
        memory_type: crate::CognitiveMemoryType::Working,
        value: "test-value".to_string(),
        session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
        recorded_in: SessionPhase::Execution,
    });

    let evidence_count = handoff.evidence_records.len();
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert_eq!(view.memory_record_count, 1);
    assert_eq!(view.evidence_record_count, evidence_count);
}

#[test]
fn from_handoff_preserves_copilot_audit() {
    let mut handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    handoff.copilot_submit_audit = Some(CopilotSubmitAudit {
        flow_asset: "test-flow".to_string(),
        payload_id: "payload-123".to_string(),
        outcome: "success".to_string(),
        reason_code: Some("OK".to_string()),
        ..CopilotSubmitAudit::default()
    });

    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    assert!(view.copilot_submit_audit.is_some());
    let audit = view.copilot_submit_audit.unwrap();
    assert_eq!(audit.flow_asset, "test-flow");
    assert_eq!(audit.payload_id, "payload-123");
    assert_eq!(audit.outcome, "success");
    assert_eq!(audit.reason_code.as_deref(), Some("OK"));
}

// --- TerminalReadView::from_handoff additional edge cases ---

#[test]
fn from_handoff_multiple_steps_are_ordered() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-step-1=first"));
    evidence.push(make_evidence("terminal-step-2=second"));
    evidence.push(make_evidence("terminal-step-3=third"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.step_count, 3);
    assert_eq!(view.steps.len(), 3);
}

#[test]
fn from_handoff_multiple_checkpoints() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-checkpoint-1=cp1"));
    evidence.push(make_evidence("terminal-checkpoint-2=cp2"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.checkpoints.len(), 2);
}

#[test]
fn from_handoff_no_continuity_source() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert!(view.continuity_source.is_none());
}

#[test]
fn from_handoff_selected_base_type_string() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.selected_base_type, "terminal-shell");
}

#[test]
fn from_handoff_topology_string() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.topology, "single-process");
}

#[test]
fn from_handoff_no_copilot_audit_by_default() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert!(view.copilot_submit_audit.is_none());
}

#[test]
fn from_handoff_memory_and_evidence_counts_with_multiple() {
    let mut handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    for i in 0..5 {
        handoff.memory_records.push(MemoryRecord {
            key: format!("key-{i}"),
            memory_type: crate::CognitiveMemoryType::Working,
            value: format!("value-{i}"),
            session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            recorded_in: SessionPhase::Execution,
        });
    }
    let evidence_count = handoff.evidence_records.len();
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.memory_record_count, 5);
    assert_eq!(view.evidence_record_count, evidence_count);
}

#[test]
fn from_handoff_step_count_matches_steps_len() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-step-1=a"));
    evidence.push(make_evidence("terminal-step-2=b"));
    evidence.push(make_evidence("terminal-step-3=c"));
    evidence.push(make_evidence("terminal-step-4=d"));
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.step_count, view.steps.len());
    assert_eq!(view.step_count, 4);
}

#[test]
fn from_handoff_objective_metadata_contains_chars() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert!(view.objective_metadata.contains("chars="));
}

#[test]
fn from_handoff_identity_is_simard_engineer() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.identity, "simard-engineer");
}

#[test]
fn from_handoff_default_wait_values() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view =
        TerminalReadView::from_handoff(PathBuf::from("/test"), handoff, "h.json".to_string(), None)
            .unwrap();
    assert_eq!(view.wait_count, "0");
    assert_eq!(view.wait_timeout_seconds, "5");
}
