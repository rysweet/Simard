use std::path::PathBuf;

use crate::CopilotSubmitAudit;

use super::read_view::TerminalReadView;
use super::test_support::{
    make_evidence, make_handoff, make_session_record, required_evidence_records,
};

// --- TerminalReadView::from_handoff error cases ---

#[test]
fn from_handoff_errors_when_session_is_none() {
    let handoff = make_handoff(None, required_evidence_records());
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(result.is_err(), "should fail when session is None");
}

#[test]
fn from_handoff_errors_when_backend_implementation_missing() {
    let evidence = vec![
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user"),
        make_evidence("terminal-command-count=1"),
        make_evidence("terminal-transcript-preview=preview"),
    ];
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail when backend-implementation evidence is missing"
    );
}

#[test]
fn from_handoff_errors_when_shell_missing() {
    let evidence = vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("terminal-working-directory=/home/user"),
        make_evidence("terminal-command-count=1"),
        make_evidence("terminal-transcript-preview=preview"),
    ];
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail when shell evidence is missing"
    );
}

#[test]
fn from_handoff_errors_when_working_directory_missing() {
    let evidence = vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-command-count=1"),
        make_evidence("terminal-transcript-preview=preview"),
    ];
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail when working-directory evidence is missing"
    );
}

#[test]
fn from_handoff_errors_when_command_count_missing() {
    let evidence = vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user"),
        make_evidence("terminal-transcript-preview=preview"),
    ];
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail when command-count evidence is missing"
    );
}

#[test]
fn from_handoff_errors_when_transcript_preview_missing() {
    let evidence = vec![
        make_evidence("backend-implementation=test-adapter"),
        make_evidence("shell=/bin/bash"),
        make_evidence("terminal-working-directory=/home/user"),
        make_evidence("terminal-command-count=1"),
    ];
    let handoff = make_handoff(Some(make_session_record()), evidence);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail when transcript-preview evidence is missing"
    );
}

#[test]
fn from_handoff_errors_with_empty_evidence() {
    let handoff = make_handoff(Some(make_session_record()), vec![]);
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    );
    assert!(result.is_err(), "should fail with no evidence records");
}

#[test]
fn from_handoff_with_invalid_objective_metadata() {
    let mut session = make_session_record();
    session.objective = "not a valid metadata format".to_string();
    let handoff = make_handoff(Some(session), required_evidence_records());
    let result = TerminalReadView::from_handoff(
        PathBuf::from("/test"),
        handoff,
        "h.json".to_string(),
        None,
    );
    assert!(
        result.is_err(),
        "should fail for invalid objective metadata format"
    );
}

// --- print methods (smoke tests that don't panic) ---

#[test]
fn print_does_not_panic() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        Some("shared default state-root".to_string()),
    )
    .unwrap();
    view.print();
}

#[test]
fn print_terminal_run_does_not_panic() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    view.print_terminal_run(
        &["capability-a".to_string(), "capability-b".to_string()],
        "execution completed",
        "reflection completed",
    );
}

#[test]
fn print_with_steps_and_checkpoints_does_not_panic() {
    let mut evidence = required_evidence_records();
    evidence.push(make_evidence("terminal-step-1=run cargo test"));
    evidence.push(make_evidence("terminal-step-2=verify output"));
    evidence.push(make_evidence("terminal-checkpoint-1=tests pass"));
    evidence.push(make_evidence("terminal-last-output-line=All tests passed"));

    let handoff = make_handoff(Some(make_session_record()), evidence);
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        Some("shared explicit state-root".to_string()),
    )
    .unwrap();
    view.print();
}

#[test]
fn print_with_copilot_audit_does_not_panic() {
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
    view.print();
}

#[test]
fn print_with_copilot_audit_no_reason_code_does_not_panic() {
    let mut handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    handoff.copilot_submit_audit = Some(CopilotSubmitAudit {
        flow_asset: "test-flow".to_string(),
        payload_id: "payload-456".to_string(),
        outcome: "skipped".to_string(),
        reason_code: None,
        ..CopilotSubmitAudit::default()
    });

    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test/state-root"),
        handoff,
        "latest_terminal_handoff.json".to_string(),
        None,
    )
    .unwrap();
    view.print();
}

#[test]
fn print_terminal_run_empty_capabilities_does_not_panic() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test"),
        handoff,
        "h.json".to_string(),
        None,
    )
    .unwrap();
    view.print_terminal_run(&[], "summary", "reflection");
}

#[test]
fn print_terminal_run_multiple_capabilities() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test"),
        handoff,
        "h.json".to_string(),
        Some("explicit".to_string()),
    )
    .unwrap();
    view.print_terminal_run(
        &["cap-a".into(), "cap-b".into(), "cap-c".into()],
        "exec done",
        "reflect done",
    );
}

#[test]
fn print_with_no_last_output_line_does_not_panic() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test"),
        handoff,
        "h.json".to_string(),
        None,
    )
    .unwrap();
    assert!(view.last_output_line.is_none());
    view.print();
}

#[test]
fn print_with_continuity_source_does_not_panic() {
    let handoff = make_handoff(Some(make_session_record()), required_evidence_records());
    let view = TerminalReadView::from_handoff(
        PathBuf::from("/test"),
        handoff,
        "h.json".to_string(),
        Some("test-continuity-source".to_string()),
    )
    .unwrap();
    view.print();
}
