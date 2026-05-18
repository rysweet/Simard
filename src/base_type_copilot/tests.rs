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
