use super::repl::*;
use super::test_support::{MockAgentSession, mock_bridge};
use crate::meeting_facilitator::MeetingSessionStatus;
use serial_test::serial;

#[test]
#[serial]
fn repl_sends_message_and_closes() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("I understand the concern.");
    let input = b"Let's discuss testing\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    let session = run_meeting_repl(
        "Sprint planning",
        &bridge,
        Some(Box::new(agent)),
        "You are Simard.",
        &mut reader,
        &mut output,
    )
    .unwrap();

    assert_eq!(session.status, MeetingSessionStatus::Closed);
    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("I understand the concern"),
        "should show agent response: {output_str}"
    );
}

#[test]
#[serial]
fn repl_shows_help() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/help\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Help test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("/status"),
        "help should mention /status: {output_str}"
    );
    assert!(
        output_str.contains("/close"),
        "help should mention /close: {output_str}"
    );
}

#[test]
#[serial]
fn repl_shows_status() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("noted");
    let input = b"First message\n/status\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Status test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Messages: 2"),
        "status should show 2 messages: {output_str}"
    );
}

#[test]
#[serial]
fn repl_eof_closes() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"just one line\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    let session = run_meeting_repl(
        "EOF test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    assert_eq!(session.status, MeetingSessionStatus::Closed);
}

#[test]
#[serial]
fn repl_no_agent_returns_error() {
    let bridge = mock_bridge();
    let input = b"some note\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    let result = run_meeting_repl("No-agent test", &bridge, None, "", &mut reader, &mut output);

    assert!(
        result.is_err(),
        "should fail without agent, not silently degrade"
    );
}

#[test]
#[serial]
fn repl_theme_command_records_theme() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("noted");
    let input = b"/theme performance\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Theme test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Theme recorded: performance"),
        "should confirm theme: {output_str}"
    );
}

#[test]
#[serial]
fn repl_recap_shows_session_info() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/theme scalability\n/recap\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Recap test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Meeting Recap"),
        "should show recap: {output_str}"
    );
    assert!(
        output_str.contains("scalability"),
        "recap should include recorded theme: {output_str}"
    );
}

#[test]
#[serial]
fn repl_preview_shows_handoff_preview() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/preview\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Preview test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Handoff Preview"),
        "should show preview: {output_str}"
    );
}

#[test]
#[serial]
fn repl_live_output_uses_colored_prompt_and_assistant_label() {
    // Ensure NO_COLOR is unset so ANSI escapes are emitted.
    // SAFETY: serial_test guards against parallel access to env vars.
    unsafe { std::env::remove_var("NO_COLOR") };

    let bridge = mock_bridge();
    let agent = MockAgentSession::new("Acknowledged.");
    let input = b"Quick check\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Live label test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    // Prompt text is preserved literally so existing tests/scripts still match.
    assert!(
        output_str.contains("simard:meeting>"),
        "prompt text should be present: {output_str}"
    );
    // Cyan ANSI escape (\x1b[36m) should wrap the prompt when NO_COLOR is unset.
    assert!(
        output_str.contains("\x1b[36msimard:meeting> \x1b[0m"),
        "prompt should be color-coded cyan: {output_str:?}"
    );
    // Assistant responses get a green-coded "Simard:" role label.
    assert!(
        output_str.contains("\x1b[32mSimard:\x1b[0m Acknowledged."),
        "assistant response should be prefixed with colored role label: {output_str:?}"
    );
}

#[test]
#[serial]
fn repl_no_color_env_strips_prompt_and_label_escapes() {
    // SAFETY: serial_test guards against parallel access to env vars.
    unsafe { std::env::set_var("NO_COLOR", "1") };

    let bridge = mock_bridge();
    let agent = MockAgentSession::new("Plain reply.");
    let input = b"Plain please\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "NO_COLOR test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    unsafe { std::env::remove_var("NO_COLOR") };

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        !output_str.contains("\x1b["),
        "no ANSI escapes should appear when NO_COLOR is set: {output_str:?}"
    );
    assert!(
        output_str.contains("simard:meeting>"),
        "plain prompt still present: {output_str}"
    );
    assert!(
        output_str.contains("Simard: Plain reply."),
        "plain assistant label still present: {output_str}"
    );
}

#[test]
#[serial]
fn repl_help_includes_theme_recap_preview() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/help\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Help extended test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("/theme"),
        "help should mention /theme: {output_str}"
    );
    assert!(
        output_str.contains("/recap"),
        "help should mention /recap: {output_str}"
    );
    assert!(
        output_str.contains("/preview"),
        "help should mention /preview: {output_str}"
    );
}
