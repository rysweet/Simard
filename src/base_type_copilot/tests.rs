//! Unit tests for the copilot base-type adapter.
//!
//! Covers two previously-untested surfaces:
//!
//!  1. `transcript.rs` — transcript noise/footer detection and response
//!     extraction (237 LOC, no tests before this commit).
//!  2. `mod.rs` — the `validate_command` defense-in-depth check and the
//!     `CopilotSdkAdapter::with_config` / `::registered` constructors.

use super::transcript::{extract_response_from_transcript, is_copilot_footer_line, strip_ansi};
use super::{CopilotAdapterConfig, CopilotSdkAdapter};
use crate::error::SimardError;

// ---------------------------------------------------------------------------
// strip_ansi
// ---------------------------------------------------------------------------

#[test]
fn strip_ansi_passes_through_plain_ascii() {
    assert_eq!(strip_ansi("hello world"), "hello world");
}

#[test]
fn strip_ansi_removes_simple_csi_color_code() {
    // ESC [ 31 m  red foreground; ESC [ 0 m  reset
    let input = "\x1b[31mred\x1b[0m";
    assert_eq!(strip_ansi(input), "red");
}

#[test]
fn strip_ansi_removes_multi_attribute_csi_codes() {
    let input = "\x1b[1;31;40mbold-red-on-black\x1b[0m text";
    assert_eq!(strip_ansi(input), "bold-red-on-black text");
}

#[test]
fn strip_ansi_handles_consecutive_escape_sequences() {
    let input = "\x1b[0m\x1b[1m\x1b[31mok\x1b[0m";
    assert_eq!(strip_ansi(input), "ok");
}

#[test]
fn strip_ansi_returns_empty_for_only_escape_codes() {
    let input = "\x1b[31m\x1b[0m";
    assert_eq!(strip_ansi(input), "");
}

#[test]
fn strip_ansi_handles_empty_input() {
    assert_eq!(strip_ansi(""), "");
}

#[test]
fn strip_ansi_handles_truncated_escape_sequence() {
    // No final byte — the implementation should not panic and should
    // consume the partial sequence without producing garbage.
    let input = "before\x1b[31";
    let out = strip_ansi(input);
    assert!(out.starts_with("before"), "got {out:?}");
}

#[test]
fn strip_ansi_lone_esc_byte_is_dropped_or_passed_through_without_panic() {
    let input = "a\x1bb";
    // We don't assert exact form — just that it doesn't panic and is finite.
    let _ = strip_ansi(input);
}

// ---------------------------------------------------------------------------
// is_copilot_footer_line
// ---------------------------------------------------------------------------

#[test]
fn footer_recognizes_total_usage_est() {
    assert!(is_copilot_footer_line("Total usage est: 1234 tokens"));
}

#[test]
fn footer_recognizes_api_time_spent() {
    assert!(is_copilot_footer_line("API time spent: 12.3s"));
}

#[test]
fn footer_recognizes_total_session_time() {
    assert!(is_copilot_footer_line("Total session time: 00:01:23"));
}

#[test]
fn footer_recognizes_changes_billing_summary() {
    assert!(is_copilot_footer_line("Changes   +0 -0"));
    assert!(is_copilot_footer_line("Changes   +12 -3"));
}

#[test]
fn footer_recognizes_requests_billing_summary() {
    assert!(is_copilot_footer_line("Requests  7.5 Premium (10s)"));
    assert!(is_copilot_footer_line("Requests  3 Free"));
    assert!(is_copilot_footer_line("Requests  4 (cached)"));
}

#[test]
fn footer_recognizes_tokens_summary_with_arrows() {
    // ↑ U+2191, ↓ U+2193
    assert!(is_copilot_footer_line(
        "Tokens    \u{2191} 29.9k \u{2022} \u{2193} 5 \u{2022} 12.7k (cached)"
    ));
}

#[test]
fn footer_recognizes_tokens_summary_with_cached_word_only() {
    assert!(is_copilot_footer_line("Tokens cached"));
}

#[test]
fn footer_ignores_normal_chat_text() {
    assert!(!is_copilot_footer_line(
        "Here is the answer to your question."
    ));
    assert!(!is_copilot_footer_line(""));
    assert!(!is_copilot_footer_line("Changes are still needed."));
    // Looks like requests but no telemetry markers
    assert!(!is_copilot_footer_line("Requests should be batched"));
}

#[test]
fn footer_changes_without_plus_or_minus_is_not_footer() {
    // Only true Copilot billing lines contain ` +` or ` -`
    assert!(!is_copilot_footer_line("Changes are necessary"));
}

// ---------------------------------------------------------------------------
// extract_response_from_transcript
// ---------------------------------------------------------------------------

#[test]
fn extract_response_returns_empty_when_only_noise() {
    // When bootstrap end and footer start coincide, the parser falls into the
    // fallback branch which only strips known noise lines. Lines that are
    // only consumed by the forward-sweep (XPIA, `cat /tmp/prompt`) can leak
    // through. A transcript that contains *only* fallback-recognized noise
    // (footers + script markers + empty lines) must yield an empty body.
    let transcript = "\
Script started on Mon May 15 10:00:00 2026

Total usage est: 100
Changes   +0 -0
Requests  1 Premium
exit
Script done on Mon May 15 10:00:05 2026
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "");
}

#[test]
fn extract_response_isolates_llm_body_between_bootstrap_and_footer() {
    let transcript = "\
Script started on Mon May 15 10:00:00 2026
bash-5.2$ cat /tmp/prompt
Staged hooks
XPIA defender loaded
The answer is 42.
Total usage est: 100
bash-5.2$ exit
Script done on Mon May 15 10:00:05 2026
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "The answer is 42.");
}

#[test]
fn extract_response_strips_billing_footer_block() {
    let transcript = "\
Script started on x
Staged hook
Hello there
Changes   +0 -0
Requests  1 Premium
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Hello there");
}

#[test]
fn extract_response_strips_tool_call_tree_glyphs() {
    let transcript = "\
Script started on x
Staged hook
\u{25cf} bash: ls
\u{2502} foo.txt
\u{2514} 1 file
Actual response line
Total usage est: 1
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Actual response line");
}

#[test]
fn extract_response_strips_time_builtin_output() {
    let transcript = "\
Script started on x
Staged hook
real\t0m1.234s
user\t0m0.123s
sys\t0m0.011s
Useful content
Total usage est: 1
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Useful content");
}

#[test]
fn extract_response_strips_hook_telemetry_lines() {
    let transcript = "\
Script started on x
Staged hook
Loaded hook foo
Hook fired: bar
[hook] baz
Real reply
Total usage est: 0
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Real reply");
}

#[test]
fn extract_response_strips_file_artefact_lines() {
    let transcript = "\
Script started on x
Staged hook
Created file /tmp/x
Modified file /tmp/y
Deleted file /tmp/z
Wrote file /tmp/w
Reply body
Total usage est: 0
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Reply body");
}

#[test]
fn extract_response_strips_amplihack_update_nag() {
    let transcript = "\
Script started on x
Staged hook
\u{2139} A newer amplihack is available
Run 'amplihack update' to upgrade
Update now?
The actual reply
Total usage est: 0
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "The actual reply");
}

#[test]
fn extract_response_pipe_delimited_preview_format_fallback() {
    // Single-line transcript with " | " separators is the preview format the
    // dashboard uses; the parser should still find the body.
    let transcript =
        "Script started on x | Staged hook | The answer | Total usage est: 1 | bash-5.2$ exit";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "The answer");
}

#[test]
fn extract_response_stops_at_first_footer_not_subsequent() {
    // If two footer-like lines appear, we use the first one as the cut-off
    // so we don't accidentally re-include the second.
    let transcript = "\
Script started on x
Staged hook
Body line
Total usage est: 1
API time spent: 2s
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Body line");
}

#[test]
fn extract_response_when_no_delimiters_strips_known_noise() {
    // When response_start >= response_end (no delimiters found at all), the
    // fallback branch is taken: it should still filter known noise lines.
    let transcript = "\
Just a body line
Total usage est: 1
exit
";
    let body = extract_response_from_transcript(transcript);
    assert!(
        body.contains("Just a body line"),
        "expected body line preserved, got {body:?}"
    );
    assert!(!body.contains("Total usage est"));
    assert!(!body.contains("exit"));
}

#[test]
fn extract_response_handles_empty_transcript() {
    assert_eq!(extract_response_from_transcript(""), "");
}

#[test]
fn extract_response_filters_blank_lines_in_body() {
    let transcript = "\
Script started on x
Staged hook

Line A

Line B

Total usage est: 1
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Line A\nLine B");
}

#[test]
fn extract_response_dollar_exit_marker_terminates_body() {
    let transcript = "\
Script started on x
Staged hook
Body content
bash-5.2$ exit
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Body content");
}

#[test]
fn extract_response_bare_exit_marker_terminates_body() {
    let transcript = "\
Script started on x
Staged hook
Body content
exit
Total usage est: 0
";
    let body = extract_response_from_transcript(transcript);
    assert_eq!(body, "Body content");
}

// ---------------------------------------------------------------------------
// CopilotAdapterConfig defaults & validate_command (indirect, via with_config)
// ---------------------------------------------------------------------------

#[test]
fn config_default_uses_amplihack_copilot_command() {
    let cfg = CopilotAdapterConfig::default();
    assert_eq!(cfg.command, "amplihack copilot");
    assert!(cfg.working_directory.is_none());
}

#[test]
fn registered_constructor_succeeds_with_default_command() {
    let adapter = CopilotSdkAdapter::registered("copilot-test").expect("default must validate");
    assert_eq!(adapter.config().command, "amplihack copilot");
}

#[test]
fn with_config_accepts_simple_command_without_metacharacters() {
    let cfg = CopilotAdapterConfig {
        command: "my-tool --flag value".to_string(),
        working_directory: Some("/tmp/work".to_string()),
    };
    let adapter =
        CopilotSdkAdapter::with_config("copilot-x", cfg).expect("safe command must be accepted");
    assert_eq!(adapter.config().command, "my-tool --flag value");
    assert_eq!(
        adapter.config().working_directory.as_deref(),
        Some("/tmp/work")
    );
}

#[test]
fn with_config_rejects_semicolon() {
    assert_metachar_rejected("cmd; rm -rf /", ';');
}

#[test]
fn with_config_rejects_pipe() {
    assert_metachar_rejected("cmd | cat", '|');
}

#[test]
fn with_config_rejects_ampersand() {
    assert_metachar_rejected("cmd & background", '&');
}

#[test]
fn with_config_rejects_backtick() {
    assert_metachar_rejected("cmd `whoami`", '`');
}

#[test]
fn with_config_rejects_dollar_sign() {
    assert_metachar_rejected("cmd $VAR", '$');
}

#[test]
fn with_config_rejects_empty_command() {
    let cfg = CopilotAdapterConfig {
        command: "   ".to_string(),
        working_directory: None,
    };
    let err = CopilotSdkAdapter::with_config("x", cfg).expect_err("whitespace-only must fail");
    match err {
        SimardError::InvalidConfigValue { key, help, .. } => {
            assert_eq!(key, "command");
            assert!(
                help.contains("must not be empty"),
                "help should mention empty: {help}"
            );
        }
        other => panic!("expected InvalidConfigValue, got {other:?}"),
    }
}

#[test]
fn with_config_rejects_truly_empty_command() {
    let cfg = CopilotAdapterConfig {
        command: String::new(),
        working_directory: None,
    };
    assert!(CopilotSdkAdapter::with_config("x", cfg).is_err());
}

fn assert_metachar_rejected(command: &str, expected_char: char) {
    let cfg = CopilotAdapterConfig {
        command: command.to_string(),
        working_directory: None,
    };
    let err = CopilotSdkAdapter::with_config("x", cfg)
        .expect_err("metachar must be rejected by validate_command");
    match err {
        SimardError::InvalidConfigValue { key, value, help } => {
            assert_eq!(key, "command");
            assert_eq!(value, command);
            assert!(
                help.contains(expected_char),
                "help should mention rejected char '{expected_char}': {help}"
            );
        }
        other => panic!("expected InvalidConfigValue for {command:?}, got {other:?}"),
    }
}

// ===========================================================================
// Meeting-mode tests (TDD: these define contracts BEFORE implementation)
// ===========================================================================
//
// These tests verify the behavioral changes introduced by issue #2170:
//   - Meeting sessions invoke `copilot` directly (not `amplihack copilot`)
//   - Meeting sessions use `--no-custom-instructions --silent --session-id`
//   - Meeting sessions do NOT go through the PTY `execute_terminal_turn` path
//   - Non-meeting sessions remain on the existing PTY path unchanged
//   - A persistent session UUID is generated at `open()` for meeting mode
//   - The session UUID is cleared at `close()`
//
// Tests that inspect internal state (session_uuid) use the CopilotSdkSession
// struct directly (not the trait object) since the struct is crate-private
// but accessible within the same crate's test module.

use crate::base_types::{
    BaseTypeFactory, BaseTypeOutcome, BaseTypeSessionRequest, BaseTypeTurnInput,
};
use crate::identity::OperatingMode;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

/// Helper: create a `BaseTypeSessionRequest` with the given `OperatingMode`.
fn make_request(mode: OperatingMode) -> BaseTypeSessionRequest {
    BaseTypeSessionRequest {
        session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001").unwrap(),
        mode,
        topology: RuntimeTopology::SingleProcess,
        prompt_assets: vec![],
        runtime_node: RuntimeNodeId::new("test-node"),
        mailbox_address: RuntimeAddress::new("test://addr"),
    }
}

/// Helper: check if the `copilot` binary is available on PATH.
fn copilot_on_path() -> bool {
    std::process::Command::new("copilot")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Unwrap a meeting-mode turn outcome, or signal a graceful skip when the real
/// `copilot` subprocess could not complete a turn in this environment (e.g.
/// missing, expired, or rate-limited GitHub auth).
///
/// The behavioral meeting tests below can only assert on a *successful* turn, so
/// having `copilot` on PATH is necessary but not sufficient — the subprocess
/// also needs valid auth, which is ambient and can fail intermittently. When the
/// adapter cannot invoke a turn, return `None` so the caller skips, consistent
/// with the `copilot_on_path()` gate. Any other error is a real bug and panics.
fn meeting_outcome_or_skip(
    result: Result<BaseTypeOutcome, SimardError>,
) -> Option<BaseTypeOutcome> {
    match result {
        Ok(outcome) => Some(outcome),
        Err(SimardError::AdapterInvocationFailed { reason, .. }) => {
            eprintln!("SKIP: copilot meeting turn unavailable in this environment: {reason}");
            None
        }
        Err(other) => panic!("unexpected error from meeting-mode run_turn: {other:?}"),
    }
}

#[test]
fn meeting_outcome_or_skip_skips_on_adapter_invocation_failure() {
    // Reproduces the flaky failure mode deterministically: a real `copilot`
    // subprocess that exits non-zero (e.g. "No authentication information
    // found") surfaces as AdapterInvocationFailed. The behavioral meeting
    // tests must skip on this — not `.unwrap()`-panic — since they can only
    // assert on a successful turn and auth is an ambient, intermittent input.
    let err = Err(SimardError::AdapterInvocationFailed {
        base_type: "copilot-meeting-test".to_string(),
        reason: "copilot meeting subprocess exited with exit status: 1: \
                 Error: No authentication information found."
            .to_string(),
    });
    assert!(
        meeting_outcome_or_skip(err).is_none(),
        "adapter invocation failure should signal a skip (None), not panic"
    );
}

#[test]
fn meeting_outcome_or_skip_returns_outcome_on_success() {
    let ok = Ok(BaseTypeOutcome {
        plan: "meeting plan".to_string(),
        execution_summary: "done".to_string(),
        evidence: vec!["copilot-meeting-session-id=abc".to_string()],
    });
    let outcome = meeting_outcome_or_skip(ok).expect("Ok(outcome) should pass through as Some");
    assert_eq!(outcome.plan, "meeting plan");
    assert_eq!(outcome.evidence, vec!["copilot-meeting-session-id=abc"]);
}

#[test]
fn session_creation_succeeds_for_meeting_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    drop(session);
}

#[test]
fn session_creation_succeeds_for_engineer_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-eng-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Engineer))
        .unwrap();
    drop(session);
}

#[test]
fn session_creation_succeeds_for_curator_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-cur-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Curator))
        .unwrap();
    drop(session);
}

#[test]
fn session_creation_succeeds_for_improvement_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-imp-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Improvement))
        .unwrap();
    drop(session);
}

#[test]
fn session_creation_succeeds_for_gym_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-gym-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Gym))
        .unwrap();
    drop(session);
}

#[test]
fn session_creation_succeeds_for_orchestrator_mode() {
    let adapter = CopilotSdkAdapter::registered("copilot-orch-test").unwrap();
    let session = adapter
        .open_session(make_request(OperatingMode::Orchestrator))
        .unwrap();
    drop(session);
}

// ---------------------------------------------------------------------------
// session_uuid lifecycle (uses CopilotSdkSession directly)
// ---------------------------------------------------------------------------
//
// CopilotSdkSession is crate-private but accessible in this test module.
// These tests construct it directly to inspect the session_uuid field.

#[test]
fn meeting_session_has_no_uuid_before_open() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    assert!(
        session.session_uuid.is_none(),
        "session_uuid should be None before open()"
    );
}

#[test]
fn meeting_session_generates_uuid_on_open() {
    use super::CopilotSdkSession;
    use crate::base_types::BaseTypeSession;
    let mut session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    session.open().unwrap();
    assert!(
        session.session_uuid.is_some(),
        "session_uuid should be Some after open() in meeting mode"
    );
}

#[test]
fn non_meeting_session_has_no_uuid_after_open() {
    use super::CopilotSdkSession;
    use crate::base_types::BaseTypeSession;
    let mut session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer));
    session.open().unwrap();
    assert!(
        session.session_uuid.is_none(),
        "session_uuid should remain None for non-meeting mode"
    );
}

#[test]
fn meeting_session_uuid_cleared_on_close() {
    use super::CopilotSdkSession;
    use crate::base_types::BaseTypeSession;
    let mut session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    session.open().unwrap();
    assert!(session.session_uuid.is_some());
    session.close().unwrap();
    assert!(
        session.session_uuid.is_none(),
        "session_uuid should be None after close()"
    );
}

#[test]
fn meeting_session_uuid_is_valid_uuid_v4_format() {
    use super::CopilotSdkSession;
    use crate::base_types::BaseTypeSession;
    let mut session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    session.open().unwrap();
    let uuid_str = session.session_uuid.as_ref().expect("UUID must be set");
    let parsed = uuid::Uuid::parse_str(uuid_str);
    assert!(
        parsed.is_ok(),
        "session_uuid should be a valid UUID, got: {uuid_str}"
    );
    let uuid = parsed.unwrap();
    assert_eq!(uuid.get_version_num(), 4, "session_uuid should be UUID v4");
}

#[test]
fn meeting_session_uuid_stable_across_reads() {
    use super::CopilotSdkSession;
    use crate::base_types::BaseTypeSession;
    let mut session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    session.open().unwrap();
    let uuid1 = session.session_uuid.clone();
    let uuid2 = session.session_uuid.clone();
    assert_eq!(uuid1, uuid2, "session_uuid must be stable across reads");
}

#[test]
fn is_meeting_mode_returns_true_for_meeting() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Meeting));
    assert!(
        session.is_meeting_mode(),
        "is_meeting_mode() should return true for Meeting"
    );
}

#[test]
fn is_meeting_mode_returns_false_for_engineer() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer));
    assert!(
        !session.is_meeting_mode(),
        "is_meeting_mode() should return false for Engineer"
    );
}

#[test]
fn is_meeting_mode_returns_false_for_curator() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Curator));
    assert!(
        !session.is_meeting_mode(),
        "is_meeting_mode() should return false for Curator"
    );
}

#[test]
fn is_meeting_mode_returns_false_for_improvement() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Improvement));
    assert!(
        !session.is_meeting_mode(),
        "is_meeting_mode() should return false for Improvement"
    );
}

#[test]
fn is_meeting_mode_returns_false_for_gym() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Gym));
    assert!(
        !session.is_meeting_mode(),
        "is_meeting_mode() should return false for Gym"
    );
}

#[test]
fn is_meeting_mode_returns_false_for_orchestrator() {
    use super::CopilotSdkSession;
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Orchestrator));
    assert!(
        !session.is_meeting_mode(),
        "is_meeting_mode() should return false for Orchestrator"
    );
}

// ---------------------------------------------------------------------------
// Meeting-mode turn dispatch (behavioral, via run_turn)
// ---------------------------------------------------------------------------
//
// These tests require the `copilot` binary on PATH. They skip gracefully
// if it isn't available (CI environments).

/// Meeting-mode plan should mention "meeting", not "amplihack copilot".
#[test]
fn meeting_turn_plan_mentions_meeting_mode() {
    if !copilot_on_path() {
        eprintln!("SKIP: copilot binary not on PATH");
        return;
    }
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-turn").unwrap();
    let mut session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    session.open().unwrap();
    let input = BaseTypeTurnInput::objective_only("Hello from meeting test");
    let Some(outcome) = meeting_outcome_or_skip(session.run_turn(input)) else {
        return;
    };
    assert!(
        outcome.plan.to_lowercase().contains("meeting"),
        "meeting-mode plan should mention 'meeting', got: {}",
        outcome.plan
    );
}

/// Meeting-mode evidence should include copilot-meeting-session-id.
#[test]
fn meeting_turn_evidence_includes_session_id() {
    if !copilot_on_path() {
        eprintln!("SKIP: copilot binary not on PATH");
        return;
    }
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-evidence").unwrap();
    let mut session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    session.open().unwrap();
    let input = BaseTypeTurnInput::objective_only("Evidence test");
    let Some(outcome) = meeting_outcome_or_skip(session.run_turn(input)) else {
        return;
    };
    let has_session_id = outcome
        .evidence
        .iter()
        .any(|e| e.starts_with("copilot-meeting-session-id="));
    assert!(
        has_session_id,
        "evidence should include copilot-meeting-session-id, got: {:?}",
        outcome.evidence
    );
}

/// Meeting-mode evidence should NOT contain PTY artifacts.
#[test]
fn meeting_turn_evidence_has_no_pty_artifacts() {
    if !copilot_on_path() {
        eprintln!("SKIP: copilot binary not on PATH");
        return;
    }
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-no-pty").unwrap();
    let mut session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    session.open().unwrap();
    let input = BaseTypeTurnInput::objective_only("No PTY test");
    let Some(outcome) = meeting_outcome_or_skip(session.run_turn(input)) else {
        return;
    };
    let has_transcript = outcome
        .evidence
        .iter()
        .any(|e| e.starts_with("terminal-transcript-full="));
    assert!(
        !has_transcript,
        "meeting mode should NOT produce terminal-transcript-full evidence"
    );
    let has_script = outcome
        .evidence
        .iter()
        .any(|e| e.contains("Script started"));
    assert!(
        !has_script,
        "meeting mode should NOT produce 'Script started' evidence"
    );
}

/// Meeting-mode evidence should show `copilot` (direct), not `amplihack copilot`.
#[test]
fn meeting_turn_evidence_shows_direct_copilot_command() {
    if !copilot_on_path() {
        eprintln!("SKIP: copilot binary not on PATH");
        return;
    }
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-cmd").unwrap();
    let mut session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    session.open().unwrap();
    let input = BaseTypeTurnInput::objective_only("Command check");
    let Some(outcome) = meeting_outcome_or_skip(session.run_turn(input)) else {
        return;
    };
    let cmd_evidence = outcome
        .evidence
        .iter()
        .find(|e| e.starts_with("copilot-adapter-command="));
    assert!(
        cmd_evidence.is_some(),
        "evidence should include copilot-adapter-command"
    );
    let cmd = cmd_evidence.unwrap();
    assert!(
        cmd.contains("copilot-adapter-command=copilot"),
        "meeting mode should use 'copilot' directly, got: {cmd}"
    );
    assert!(
        !cmd.contains("amplihack"),
        "meeting mode should NOT use 'amplihack copilot', got: {cmd}"
    );
}

// ---------------------------------------------------------------------------
// Error handling for meeting-mode subprocess
// ---------------------------------------------------------------------------

/// Missing copilot binary → `AdapterInvocationFailed`, not panic.
#[test]
fn meeting_turn_with_missing_binary_returns_adapter_error() {
    if copilot_on_path() {
        eprintln!("SKIP: copilot binary IS on PATH; can't test missing-binary error");
        return;
    }
    let adapter = CopilotSdkAdapter::registered("copilot-meeting-missing").unwrap();
    let mut session = adapter
        .open_session(make_request(OperatingMode::Meeting))
        .unwrap();
    session.open().unwrap();
    let input = BaseTypeTurnInput::objective_only("This should fail gracefully");
    let result = session.run_turn(input);
    assert!(
        result.is_err(),
        "missing copilot binary should produce an error"
    );
    match result.unwrap_err() {
        SimardError::AdapterInvocationFailed { base_type, reason } => {
            assert!(
                reason.to_lowercase().contains("copilot")
                    || reason.to_lowercase().contains("failed"),
                "error reason should mention copilot failure, got: {reason}"
            );
            assert!(!base_type.is_empty());
        }
        other => panic!("expected AdapterInvocationFailed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// build_copilot_terminal_objective regression (PTY path unchanged)
// ---------------------------------------------------------------------------

/// PTY objective format must remain unchanged (regression).
#[test]
fn build_copilot_terminal_objective_format_unchanged() {
    use super::build_copilot_terminal_objective;
    let config = CopilotAdapterConfig::default();
    let prompt_file = tempfile::NamedTempFile::with_prefix("test-prompt-").unwrap();
    let objective = build_copilot_terminal_objective(&config, prompt_file.path());
    assert!(objective.contains("amplihack copilot"), "got: {objective}");
    assert!(objective.contains("--subprocess-safe"), "got: {objective}");
    assert!(objective.contains("--allow-all-tools"), "got: {objective}");
    assert!(objective.contains("cat"), "got: {objective}");
    assert!(objective.contains("exit"), "got: {objective}");
    assert!(
        !objective.contains("--no-custom-instructions"),
        "PTY path must not have meeting flags"
    );
    assert!(
        !objective.contains("--session-id"),
        "PTY path must not have meeting flags"
    );
}

/// PTY objective with working directory prepends it.
#[test]
fn build_copilot_terminal_objective_with_working_dir() {
    use super::build_copilot_terminal_objective;
    let config = CopilotAdapterConfig {
        command: "amplihack copilot".to_string(),
        working_directory: Some("/home/user/repo".to_string()),
    };
    let prompt_file = tempfile::NamedTempFile::with_prefix("test-wd-").unwrap();
    let objective = build_copilot_terminal_objective(&config, prompt_file.path());
    assert!(
        objective.contains("working-directory: /home/user/repo"),
        "got: {objective}"
    );
}

// ===========================================================================
// Memory + knowledge enrichment wiring (issue #1664)
// ===========================================================================
//
// Before this fix `CopilotSdkAdapter::open_session` hardcoded both bridges to
// `None`, so every production turn ran `prepare_turn_context(objective, None,
// None)` and the memory-facts / known-procedures / domain-knowledge prompt
// blocks were never reached. These tests prove (1) that supplied bridges are
// actually consumed, (2) that the absence of bridges still produces a valid
// objective-only prompt, and (3) that a supplied bridge whose query fails
// surfaces the error rather than silently degrading. The production
// `launch_enrichment_bridges` helper now lives in `base_type_turn` (shared with
// the RustyClawd adapter, issue #2383); its real-bridge / degradation tests
// live there alongside it, and `open_session_with_native_enrichment_*` below
// still guards the Copilot factory seam.

use super::CopilotSdkSession;
use crate::bridge::BridgeErrorPayload;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::knowledge_bridge::KnowledgeBridge;
use crate::memory_bridge::CognitiveMemoryBridge;
use serde_json::json;

/// Mock cognitive-memory bridge returning a single semantic fact for any
/// `search_facts` query and no procedures.
fn enrichment_memory() -> Box<dyn CognitiveMemoryOps> {
    Box::new(CognitiveMemoryBridge::new(Box::new(
        InMemoryBridgeTransport::new("test-copilot-mem", |method, _params| match method {
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "f1",
                    "concept": "rust-ownership",
                    "content": "values have a single owner",
                    "confidence": 0.92,
                    "source_id": "s1",
                    "tags": []
                }]
            })),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            other => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {other}"),
            }),
        }),
    )))
}

/// Mock cognitive-memory bridge whose calls always error — used to verify a
/// supplied bridge's query failure surfaces (no silent swallow) per the
/// `prepare_turn_context` contract.
fn failing_memory() -> Box<dyn CognitiveMemoryOps> {
    Box::new(CognitiveMemoryBridge::new(Box::new(
        InMemoryBridgeTransport::new("test-copilot-mem-fail", |_method, _params| {
            Err(BridgeErrorPayload {
                code: -32000,
                message: "simulated memory backend failure".to_string(),
            })
        }),
    )))
}

/// Mock knowledge bridge with one pack that matches a "rust" objective and a
/// canned non-empty query answer.
fn enrichment_knowledge() -> KnowledgeBridge {
    KnowledgeBridge::new(Box::new(InMemoryBridgeTransport::new(
        "test-copilot-knowledge",
        |method, _params| match method {
            "knowledge.list_packs" => Ok(json!({"packs": [{
                "name": "rust-expert",
                "description": "Rust programming language knowledge",
                "article_count": 100,
                "section_count": 400
            }]})),
            "knowledge.query" => Ok(json!({
                "answer": "Rust enforces ownership at compile time.",
                "sources": [{"title": "Ownership", "section": "Basics"}],
                "confidence": 0.88
            })),
            other => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {other}"),
            }),
        },
    )))
}

/// With both bridges supplied, the enriched prompt must include the memory
/// facts and domain knowledge sections — proving the bridges are consumed.
#[test]
fn enrichment_injects_memory_facts_and_knowledge_into_prompt() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer))
        .with_test_bridges(Some(enrichment_memory()), Some(enrichment_knowledge()));
    let input = BaseTypeTurnInput::objective_only("implement rust ownership feature");

    let prompt_file = session
        .build_meeting_prompt(&input)
        .expect("prompt build must succeed with enrichment bridges");
    let prompt = std::fs::read_to_string(prompt_file.path()).expect("read prompt file");

    assert!(
        prompt.contains("## Relevant Memory Facts"),
        "enriched prompt must include the memory facts section: {prompt}"
    );
    assert!(
        prompt.contains("values have a single owner"),
        "enriched prompt must include the mock fact content: {prompt}"
    );
    assert!(
        prompt.contains("## Domain Knowledge"),
        "enriched prompt must include the domain knowledge section: {prompt}"
    );
    assert!(
        prompt.contains("Rust enforces ownership at compile time."),
        "enriched prompt must include the mock knowledge answer: {prompt}"
    );
}

/// The PTY path also threads enrichment through `build_enriched_objective`:
/// the on-disk prompt file (referenced by the terminal objective) must carry
/// the same memory + knowledge sections.
#[test]
fn enrichment_reaches_pty_prompt_file() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer))
        .with_test_bridges(Some(enrichment_memory()), Some(enrichment_knowledge()));
    let input = BaseTypeTurnInput::objective_only("implement rust ownership feature");

    let (objective, prompt_file) = session
        .build_enriched_objective(&input)
        .expect("enriched objective build must succeed");
    // The PTY objective references the prompt file by path; the enriched
    // content lives in that file.
    assert!(
        objective.contains("cat"),
        "objective should cat the prompt file"
    );
    let prompt = std::fs::read_to_string(prompt_file.path()).expect("read prompt file");
    assert!(prompt.contains("## Relevant Memory Facts"), "got: {prompt}");
    assert!(prompt.contains("## Domain Knowledge"), "got: {prompt}");
}

/// With no bridges, the prompt is objective-only — the clean degraded path
/// that must never break turn dispatch.
#[test]
fn no_enrichment_produces_objective_only_prompt() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer));
    let input = BaseTypeTurnInput::objective_only("plain objective without enrichment");

    let prompt_file = session
        .build_meeting_prompt(&input)
        .expect("prompt build must succeed without bridges");
    let prompt = std::fs::read_to_string(prompt_file.path()).expect("read prompt file");

    assert!(prompt.contains("plain objective without enrichment"));
    assert!(
        !prompt.contains("## Relevant Memory Facts"),
        "no memory section without a memory bridge: {prompt}"
    );
    assert!(
        !prompt.contains("## Domain Knowledge"),
        "no knowledge section without a knowledge bridge: {prompt}"
    );
}

/// A supplied bridge whose query fails must surface the error (the
/// `prepare_turn_context` no-silent-degradation contract), not panic.
#[test]
fn enrichment_query_failure_propagates_not_panics() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer))
        .with_test_bridges(Some(failing_memory()), None);
    let input = BaseTypeTurnInput::objective_only("objective triggering failing memory");

    let result = session.build_meeting_prompt(&input);
    assert!(
        result.is_err(),
        "a supplied bridge whose query fails must surface an error, not silently degrade"
    );
}

/// `open_session` (via `build_session`) must wire both bridges when the
/// adapter has enrichment configured — the direct regression guard for the
/// hardcoded-`None` defect of issue #1664 at the factory seam.
#[test]
#[serial_test::serial(cognitive_memory)]
fn open_session_with_native_enrichment_populates_bridges() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    let adapter = CopilotSdkAdapter::registered("copilot-enrich-wire")
        .unwrap()
        .with_enrichment(state_root);
    let session = adapter
        .build_session(make_request(OperatingMode::Engineer))
        .expect("session must open with enrichment");

    assert!(
        session.enrichment.memory.is_some(),
        "open_session must wire the memory bridge when enrichment is Native"
    );
    assert!(
        session.enrichment.knowledge.is_some(),
        "open_session must wire the knowledge bridge when enrichment is Native"
    );
}

/// The default adapter (no `with_enrichment`) must leave both bridges `None`
/// so unit tests and lightweight callers incur no filesystem side effects.
#[test]
fn open_session_without_enrichment_leaves_bridges_none() {
    let adapter = CopilotSdkAdapter::registered("copilot-no-enrich").unwrap();
    let session = adapter
        .build_session(make_request(OperatingMode::Engineer))
        .expect("session must open without enrichment");

    assert!(
        session.enrichment.memory.is_none(),
        "default adapter must not wire a memory bridge"
    );
    assert!(
        session.enrichment.knowledge.is_none(),
        "default adapter must not wire a knowledge bridge"
    );
}
