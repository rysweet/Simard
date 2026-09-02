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

/// Write a fake `copilot` executable into a fresh temp dir and return the dir
/// (which the caller MUST keep alive for the duration of the turn) plus the
/// absolute binary path.
///
/// The fake drains its stdin — the streamed prompt (issue #2640) — to avoid a
/// broken pipe on the feeder thread, then emits a deterministic response read
/// from a sibling `response.txt` (so the response may contain any bytes without
/// shell-quoting hazards). This lets meeting-turn tests exercise the *real* turn
/// orchestration (`run_meeting_turn`) without spawning the real `copilot`
/// binary — no PATH, auth, network, or clock dependency (issue #2732).
#[cfg(unix)]
fn fake_copilot(response: &str) -> (tempfile::TempDir, String) {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let response_path = dir.path().join("response.txt");
    std::fs::write(&response_path, response).unwrap();

    let bin_path = dir.path().join("copilot");
    // `$0` is the absolute program path (Command::new was given an absolute
    // path), so `dirname "$0"` resolves the temp dir regardless of cwd.
    let script = "#!/bin/sh\n# hermetic fake copilot (issue #2732)\ncat >/dev/null\ncat \"$(dirname \"$0\")/response.txt\"\n";
    let mut file = std::fs::File::create(&bin_path).unwrap();
    file.write_all(script.as_bytes()).unwrap();
    file.flush().unwrap();
    // Close the write handle BEFORE making the file executable / exec'ing it so
    // our process holds no write fd to the binary (shrinks the ETXTBSY window;
    // see `run_fake_meeting_turn`).
    drop(file);
    let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin_path, perms).unwrap();

    (dir, bin_path.to_string_lossy().into_owned())
}

/// Build a meeting-capable adapter whose meeting-mode binary is overridden with
/// `meeting_binary` (an absolute path to a fake `copilot`), keeping every other
/// production code path intact (issue #2732).
#[cfg(unix)]
fn meeting_adapter(id: &str, meeting_binary: &str) -> CopilotSdkAdapter {
    CopilotSdkAdapter::registered(id)
        .unwrap()
        .with_meeting_binary_override(meeting_binary)
}

/// Classifies the two transient, whole-suite-parallelism spawn/wait races that
/// the multithreaded test harness — not production — can inject while exec'ing a
/// freshly-written fake `copilot` binary alongside thousands of other
/// subprocess-spawning unit tests:
///
/// * `ETXTBSY` ("Text file busy") — another parallel test thread `fork()`ed
///   while this test's just-written fake binary was momentarily open for
///   writing, so the exec transiently fails.
/// * `ECHILD` ("No child processes", os error 10) — another parallel test's
///   subprocess handling reaped this turn's child before its own
///   `wait_with_output`, so the specific-pid wait fails spuriously even though
///   the fake ran to completion.
///
/// Both are pure artifacts of running the entire unit-test suite inside a single
/// process; production meeting turns exec the long-lived external `copilot`
/// binary in a process that never SIG_IGNs `SIGCHLD` nor broad-reaps children,
/// so neither race can occur in the field. Any *other* error is a real
/// regression and is never retried, so genuine defects are still surfaced loudly.
#[cfg(unix)]
fn is_transient_meeting_spawn_race(reason: &str) -> bool {
    reason.contains("Text file busy") || reason.contains("No child processes")
}

/// Open a meeting session against the fake `copilot` at `binary` and run one
/// turn with `objective`, returning the outcome.
///
/// Retries ONLY the transient, harness-only spawn/wait races classified by
/// [`is_transient_meeting_spawn_race`] (`ETXTBSY` on exec, `ECHILD` on wait).
/// Any other error fails the test immediately, so real regressions are never
/// masked.
#[cfg(unix)]
fn run_fake_meeting_turn(id: &str, binary: &str, objective: &str) -> BaseTypeOutcome {
    let mut last_reason = String::new();
    for attempt in 0..8 {
        let adapter = meeting_adapter(id, binary);
        let mut session = adapter
            .open_session(make_request(OperatingMode::Meeting))
            .unwrap();
        session.open().unwrap();
        match session.run_turn(BaseTypeTurnInput::objective_only(objective)) {
            Ok(outcome) => return outcome,
            Err(SimardError::AdapterInvocationFailed { reason, .. })
                if is_transient_meeting_spawn_race(&reason) =>
            {
                last_reason = reason;
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
            Err(other) => panic!("meeting turn failed unexpectedly: {other:?}"),
        }
    }
    panic!("meeting turn kept hitting a transient spawn/wait race: {last_reason}");
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
// Hermetic (issue #2732): each test injects a fake `copilot` binary via
// `meeting_binary_override` (see `fake_copilot` / `meeting_adapter`) so the
// real `run_meeting_turn` path runs end-to-end — prompt streamed on stdin,
// subprocess spawned, stdout captured, plan/evidence assembled — WITHOUT
// depending on a real `copilot` binary, its auth state, the network, or PATH.
// `#[cfg(unix)]` because the fake is a `/bin/sh` script; CI runs on Linux.

/// A meeting turn captures the copilot subprocess stdout verbatim and records a
/// meeting-mode dispatch in the plan (real behavior, not a plan-substring tautology).
#[cfg(unix)]
#[test]
fn meeting_turn_captures_copilot_output_and_records_meeting_dispatch() {
    let (_dir, bin) = fake_copilot("FAKE-COPILOT-OK: meeting reply body");
    let outcome = run_fake_meeting_turn("copilot-meeting-turn", &bin, "Hello from meeting test");
    // Real behavior: the turn returned exactly what the (fake) copilot emitted.
    assert_eq!(
        outcome.execution_summary.trim(),
        "FAKE-COPILOT-OK: meeting reply body",
        "meeting turn should return the copilot subprocess stdout verbatim"
    );
    // And the plan records a meeting-mode dispatch (not the PTY/amplihack path).
    assert!(
        outcome.plan.to_lowercase().contains("meeting"),
        "meeting-mode plan should mention 'meeting', got: {}",
        outcome.plan
    );
}

/// Meeting-mode evidence should include copilot-meeting-session-id.
#[cfg(unix)]
#[test]
fn meeting_turn_evidence_includes_session_id() {
    let (_dir, bin) = fake_copilot("ok");
    let outcome = run_fake_meeting_turn("copilot-meeting-evidence", &bin, "Evidence test");
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
#[cfg(unix)]
#[test]
fn meeting_turn_evidence_has_no_pty_artifacts() {
    let (_dir, bin) = fake_copilot("ok");
    let outcome = run_fake_meeting_turn("copilot-meeting-no-pty", &bin, "No PTY test");
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
#[cfg(unix)]
#[test]
fn meeting_turn_evidence_shows_direct_copilot_command() {
    let (_dir, bin) = fake_copilot("ok");
    let outcome = run_fake_meeting_turn("copilot-meeting-cmd", &bin, "Command check");
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
// Cost accounting: meeting prompt tokens reflect the FULL enriched prompt
// streamed on stdin, not the bare objective (issue #4164).
// ---------------------------------------------------------------------------

/// Run one hermetic meeting turn under a caller-supplied session id so the
/// recorded cost ledger entry can be isolated from any concurrently-running
/// meeting test that shares the default `make_request` session id. Mirrors the
/// transient-`ETXTBSY` retry contract of [`run_fake_meeting_turn`].
#[cfg(unix)]
fn run_fake_meeting_turn_with_session(
    id: &str,
    binary: &str,
    session_id: &str,
    objective: &str,
) -> BaseTypeOutcome {
    let mut last_reason = String::new();
    for attempt in 0..8 {
        let request = BaseTypeSessionRequest {
            session_id: SessionId::parse(session_id).unwrap(),
            mode: OperatingMode::Meeting,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::new("test-node"),
            mailbox_address: RuntimeAddress::new("test://addr"),
        };
        let adapter = meeting_adapter(id, binary);
        let mut session = adapter.open_session(request).unwrap();
        session.open().unwrap();
        match session.run_turn(BaseTypeTurnInput::objective_only(objective)) {
            Ok(outcome) => return outcome,
            Err(SimardError::AdapterInvocationFailed { reason, .. })
                if reason.contains("Text file busy") =>
            {
                last_reason = reason;
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
            Err(other) => panic!("meeting turn failed unexpectedly: {other:?}"),
        }
    }
    panic!("meeting turn kept hitting a transient ETXTBSY race: {last_reason}");
}

/// Regression for issue #4164: a meeting turn must record the size of the FULL
/// enriched prompt it streams to copilot on stdin — the preamble + identity
/// context + objective wrapped in the `## Objective` / `## Instructions`
/// scaffold — as its prompt-token cost, NOT the bare `input.objective`.
///
/// Before the fix, `run_meeting_turn` recorded `input.objective.len()`, so the
/// dashboard Cost tab (`GET /api/costs`) undercounted meeting prompt tokens and
/// showed an impossible `prompt_tokens ≪ completion_tokens` ratio, understating
/// spend. The rendered prompt is strictly larger than the bare objective (the
/// scaffold alone adds well over one token), so the recorded prompt tokens must
/// exceed the bare objective's token count. This assertion FAILS on the buggy
/// code (recorded == bare) and PASSES on the fix (recorded > bare).
///
/// HOME is redirected to a per-test temp dir so the cost ledger
/// (`$HOME/.simard/costs/ledger.jsonl`) is isolated; the entry is matched by a
/// unique session id so a concurrent meeting test sharing the process-global
/// temp HOME cannot substitute its own entry. Mutating HOME requires the
/// `cognitive_memory` serial key (see
/// docs/testing/cognitive-memory-serial-isolation.md).
#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective() {
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    // SAFETY: serialised via #[serial(cognitive_memory)] — no concurrent env
    // mutation can tear this write (see the EnvBinding invariant in
    // test_support::hermetic).
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let result = std::panic::catch_unwind(|| {
        let session_id = "session-00000000-0000-0000-0000-000000004164";
        let objective = "Meeting objective body for the #4164 cost-accounting regression.";
        let (_dir, bin) = fake_copilot("FAKE-COPILOT-OK: meeting reply");
        run_fake_meeting_turn_with_session(
            "copilot-meeting-cost-4164",
            &bin,
            session_id,
            objective,
        );

        let ledger = home
            .path()
            .join(".simard")
            .join("costs")
            .join("ledger.jsonl");
        let contents = std::fs::read_to_string(&ledger)
            .expect("meeting turn must write a cost ledger entry under the temp HOME");
        let entry = contents
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|e| {
                e.get("session_id").and_then(|v| v.as_str()) == Some(session_id)
                    && e.get("model").and_then(|v| v.as_str()) == Some("copilot-meeting")
            })
            .expect("a copilot-meeting cost entry for this session must be recorded");

        let recorded_prompt_tokens = entry
            .get("prompt_tokens_est")
            .and_then(|v| v.as_u64())
            .expect("prompt_tokens_est must be a number");
        let bare_objective_tokens = crate::cost_tracking::estimate_tokens(objective.len());

        assert!(
            recorded_prompt_tokens > bare_objective_tokens,
            "meeting prompt cost must reflect the full enriched prompt streamed on \
             stdin (issue #4164), not the bare objective: \
             recorded={recorded_prompt_tokens} bare_objective_tokens={bare_objective_tokens}"
        );
    });

    // SAFETY: restore HOME before propagating any panic (same serial key).
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

// ---------------------------------------------------------------------------
// Error handling for meeting-mode subprocess
// ---------------------------------------------------------------------------

/// Missing copilot binary → `AdapterInvocationFailed`, not panic. Hermetic:
/// the meeting binary is overridden to a path that provably does not exist, so
/// the assertion no longer depends on whether a real `copilot` is on PATH.
#[cfg(unix)]
#[test]
fn meeting_turn_with_missing_binary_returns_adapter_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("definitely-not-copilot-2732");
    let adapter = meeting_adapter("copilot-meeting-missing", &missing.to_string_lossy());
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
                reason.to_lowercase().contains("spawn")
                    || reason.to_lowercase().contains("copilot")
                    || reason.to_lowercase().contains("failed"),
                "error reason should mention copilot spawn failure, got: {reason}"
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
// Before this fix `CopilotSdkAdapter::open_session` hardcoded both readers to
// `None`, so every production turn ran `prepare_turn_context(objective, None,
// None)` and the memory-facts / known-procedures / domain-knowledge prompt
// blocks were never reached. These tests prove (1) that supplied readers are
// actually consumed, (2) that the absence of readers still produces a valid
// objective-only prompt, and (3) that a supplied reader whose query fails
// surfaces the error rather than silently degrading. The production
// `launch_enrichment_clients` helper now lives in `base_type_turn` (shared with
// the RustyClawd adapter, issue #2383); its real-reader / degradation tests
// live there alongside it, and `open_session_with_native_enrichment_*` below
// still guards the Copilot factory seam.

use super::CopilotSdkSession;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::knowledge_client::KnowledgeClient;
use crate::memory_client::CognitiveMemoryClient;
use crate::rpc::RpcErrorPayload;
use crate::rpc_transport::InMemoryRpcTransport;
use serde_json::json;

/// Mock cognitive-memory reader returning a single semantic fact for any
/// `search_facts` query and no procedures.
fn enrichment_memory() -> Box<dyn CognitiveMemoryOps> {
    Box::new(CognitiveMemoryClient::new(Box::new(
        InMemoryRpcTransport::new("test-copilot-mem", |method, _params| match method {
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
            other => Err(RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {other}"),
            }),
        }),
    )))
}

/// Mock cognitive-memory reader whose calls always error — used to verify a
/// supplied reader's query failure surfaces (no silent swallow) per the
/// `prepare_turn_context` contract.
fn failing_memory() -> Box<dyn CognitiveMemoryOps> {
    Box::new(CognitiveMemoryClient::new(Box::new(
        InMemoryRpcTransport::new("test-copilot-mem-fail", |_method, _params| {
            Err(RpcErrorPayload {
                code: -32000,
                message: "simulated memory backend failure".to_string(),
            })
        }),
    )))
}

/// Mock knowledge reader with one pack that matches a "rust" objective and a
/// canned non-empty query answer.
fn enrichment_knowledge() -> KnowledgeClient {
    KnowledgeClient::new(Box::new(InMemoryRpcTransport::new(
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
            other => Err(RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {other}"),
            }),
        },
    )))
}

/// With both readers supplied, the enriched prompt must include the memory
/// facts and domain knowledge sections — proving the readers are consumed.
#[test]
fn enrichment_injects_memory_facts_and_knowledge_into_prompt() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer))
        .with_test_readers(Some(enrichment_memory()), Some(enrichment_knowledge()));
    let input = BaseTypeTurnInput::objective_only("implement rust ownership feature");

    let prompt_file = session
        .build_meeting_prompt(&input)
        .expect("prompt build must succeed with enrichment readers");
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
        .with_test_readers(Some(enrichment_memory()), Some(enrichment_knowledge()));
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

/// With no readers, the prompt is objective-only — the clean degraded path
/// that must never break turn dispatch.
#[test]
fn no_enrichment_produces_objective_only_prompt() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer));
    let input = BaseTypeTurnInput::objective_only("plain objective without enrichment");

    let prompt_file = session
        .build_meeting_prompt(&input)
        .expect("prompt build must succeed without readers");
    let prompt = std::fs::read_to_string(prompt_file.path()).expect("read prompt file");

    assert!(prompt.contains("plain objective without enrichment"));
    assert!(
        !prompt.contains("## Relevant Memory Facts"),
        "no memory section without a memory reader: {prompt}"
    );
    assert!(
        !prompt.contains("## Domain Knowledge"),
        "no knowledge section without a knowledge reader: {prompt}"
    );
}

/// A supplied reader whose query fails must surface the error (the
/// `prepare_turn_context` no-silent-degradation contract), not panic.
#[test]
fn enrichment_query_failure_propagates_not_panics() {
    let session = CopilotSdkSession::new_for_test(make_request(OperatingMode::Engineer))
        .with_test_readers(Some(failing_memory()), None);
    let input = BaseTypeTurnInput::objective_only("objective triggering failing memory");

    let result = session.build_meeting_prompt(&input);
    assert!(
        result.is_err(),
        "a supplied reader whose query fails must surface an error, not silently degrade"
    );
}

/// `open_session` (via `build_session`) must wire both readers when the
/// adapter has enrichment configured — the direct regression guard for the
/// hardcoded-`None` defect of issue #1664 at the factory seam.
#[test]
#[serial_test::serial(cognitive_memory)]
fn open_session_with_native_enrichment_populates_readers() {
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
        "open_session must wire the memory reader when enrichment is Native"
    );
    assert!(
        session.enrichment.knowledge.is_some(),
        "open_session must wire the knowledge reader when enrichment is Native"
    );
}

/// The default adapter (no `with_enrichment`) must leave both readers `None`
/// so unit tests and lightweight callers incur no filesystem side effects.
#[test]
fn open_session_without_enrichment_leaves_readers_none() {
    let adapter = CopilotSdkAdapter::registered("copilot-no-enrich").unwrap();
    let session = adapter
        .build_session(make_request(OperatingMode::Engineer))
        .expect("session must open without enrichment");

    assert!(
        session.enrichment.memory.is_none(),
        "default adapter must not wire a memory reader"
    );
    assert!(
        session.enrichment.knowledge.is_none(),
        "default adapter must not wire a knowledge reader"
    );
}
