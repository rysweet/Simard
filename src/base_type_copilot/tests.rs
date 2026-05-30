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

use crate::base_types::{BaseTypeFactory, BaseTypeSessionRequest, BaseTypeTurnInput};
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

// ---------------------------------------------------------------------------
// Session creation with all OperatingModes (regression + meeting)
// ---------------------------------------------------------------------------

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
    let outcome = session.run_turn(input).unwrap();
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
    let outcome = session.run_turn(input).unwrap();
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
    let outcome = session.run_turn(input).unwrap();
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
    let outcome = session.run_turn(input).unwrap();
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
