use super::repl::*;
use super::test_support::{FailingThenOkMockAgent, MockAgentSession, mock_bridge};
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
    // Assistant responses get a green-coded "[facilitator HH:MM:SS]" turn prefix.
    // We match on the structural pattern since the exact time varies.
    assert!(
        output_str.contains("\x1b[32m[facilitator "),
        "assistant response should be prefixed with colored turn prefix: {output_str:?}"
    );
    assert!(
        output_str.contains("Acknowledged."),
        "assistant response content should be present: {output_str:?}"
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
        output_str.contains("[facilitator "),
        "plain facilitator turn prefix still present: {output_str}"
    );
    assert!(
        output_str.contains("Plain reply."),
        "plain assistant content still present: {output_str}"
    );
    // User turn prefix should also be present
    assert!(
        output_str.contains("[user "),
        "plain user turn prefix still present: {output_str}"
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

#[test]
#[serial]
fn repl_close_prints_one_line_headline_summary() {
    // Suppress ANSI color codes so substring assertions are robust.
    // SAFETY: serial_test guards against parallel env mutations.
    unsafe { std::env::set_var("NO_COLOR", "1") };

    let bridge = mock_bridge();
    let agent = MockAgentSession::new("noted");
    // Two conversational turns, then close. MockAgent emits no action items
    // so the count should render as "0 action items".
    let input = b"Discuss release plan\nAny blockers?\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Release planning",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    unsafe { std::env::remove_var("NO_COLOR") };

    let output_str = String::from_utf8(output).unwrap();
    // One-line headline must include the topic, the bottom-line action item
    // count, and must appear before the detailed "Meeting Summary" section.
    let headline_pos = output_str
        .find("✓ Meeting closed: \"Release planning\" — 0 action items")
        .unwrap_or_else(|| {
            panic!("missing one-line headline summary in output: {output_str}");
        });
    let detailed_pos = output_str
        .find("── Meeting Summary ──")
        .unwrap_or_else(|| panic!("missing detailed summary section: {output_str}"));
    assert!(
        headline_pos < detailed_pos,
        "headline summary must precede the detailed Meeting Summary section: {output_str}"
    );
}

// ── /state slash command (issue #1646 — TDD red phase) ──────────────────
//
// The /state command re-displays the running list of decisions, open
// questions, and action items extracted from the live meeting transcript
// without closing the meeting. Sections are rendered in a canonical order
// (Decisions → Open Questions → Action Items), each with a colored heading
// and `_(none)_` placeholders when empty. Reuses the existing extractors in
// `meeting_backend::persist::extract` — no new parsing logic.

/// /state on an empty transcript renders all three section headings with
/// `_(none)_` placeholders so the operator gets a predictable, complete UI
/// instead of a blank screen. Headings are always present.
#[test]
#[serial]
fn repl_state_empty_renders_all_section_headings_with_none_placeholders() {
    let bridge = mock_bridge();
    // Use a benign canned response so any history entries don't accidentally
    // trip the heuristic extractors.
    let agent = MockAgentSession::new("ok");
    let input = b"/state\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Empty state test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();

    // All three section headings appear when /state is invoked on an empty
    // transcript. Match on the section labels (color-stripped) so the test
    // is robust to ANSI changes.
    assert!(
        output_str.contains("Decisions"),
        "state output should include 'Decisions' heading: {output_str}"
    );
    assert!(
        output_str.contains("Open Questions"),
        "state output should include 'Open Questions' heading: {output_str}"
    );
    assert!(
        output_str.contains("Action Items"),
        "state output should include 'Action Items' heading: {output_str}"
    );

    // The empty-state placeholder convention is `_(none)_`. We expect at
    // least three occurrences (one per section) for a fully empty state.
    let none_count = output_str.matches("_(none)_").count();
    assert!(
        none_count >= 3,
        "expected at least 3 '_(none)_' placeholders (one per section) on empty state, got {none_count}: {output_str}"
    );
}

/// /state on a populated transcript renders the extracted decisions, open
/// questions, and action items derived from the in-memory history via the
/// existing `meeting_backend::persist::extract` helpers.
#[test]
#[serial]
fn repl_state_populated_renders_extracted_decisions_action_items_open_questions() {
    let bridge = mock_bridge();
    // Agent response embeds explicit signals the extractors recognize:
    //   - "Decision:" → extract_decisions
    //   - "TODO:"     → extract_action_items
    //   - "OPEN:"     → extract_open_questions (explicit marker)
    let agent = MockAgentSession::new(
        "Decision: We will adopt TDD. TODO: Set up CI pipeline. OPEN: Who will own the rollout plan for the new pipeline?",
    );
    // First send a user message so the agent reply (with all the signals)
    // is appended to backend.history(); then invoke /state.
    let input = b"Let's plan the sprint\n/state\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Populated state test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();

    // Find the /state output region by locating the first "Decisions"
    // heading after the opening prompt — this is the /state-rendered block,
    // not the post-/close summary block (which appears later).
    let state_region_start = output_str
        .find("Decisions")
        .unwrap_or_else(|| panic!("expected /state to render a 'Decisions' heading: {output_str}"));
    // Bound the region by the next prompt or by the end-of-meeting summary.
    let state_region_end = output_str[state_region_start..]
        .find("simard:meeting>")
        .map(|i| state_region_start + i)
        .unwrap_or(output_str.len());
    let state_region = &output_str[state_region_start..state_region_end];

    // Each extracted item should appear in the /state output region.
    assert!(
        state_region.contains("TDD"),
        "state output should include the extracted decision (TDD): {state_region}"
    );
    assert!(
        state_region.contains("CI pipeline"),
        "state output should include the extracted action item (CI pipeline): {state_region}"
    );
    assert!(
        state_region.contains("rollout plan"),
        "state output should include the extracted open question (rollout plan): {state_region}"
    );
}

/// /state must render sections in the canonical order specified by the
/// task: Decisions → Open Questions → Action Items. This is intentionally
/// different from the /close summary order; verify the ordering directly.
#[test]
#[serial]
fn repl_state_renders_sections_in_canonical_order_decisions_then_questions_then_actions() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new(
        "Decision: Use Rust. TODO: Write the migration script. OPEN: What is the rollback strategy here?",
    );
    let input = b"Plan the migration\n/state\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Order test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();

    // Bound the region to the /state-rendered block (between the input echo
    // and the next prompt) so we don't accidentally match the post-/close
    // "── Decisions ──" / "── Action Items ──" summary headings (which use a
    // different ordering and would hide a real bug).
    let first_state_idx = output_str
        .find("Decisions")
        .unwrap_or_else(|| panic!("expected /state output to contain 'Decisions': {output_str}"));
    let region_end = output_str[first_state_idx..]
        .find("simard:meeting>")
        .map(|i| first_state_idx + i)
        .unwrap_or(output_str.len());
    let region = &output_str[first_state_idx..region_end];

    let dec_pos = region
        .find("Decisions")
        .unwrap_or_else(|| panic!("missing 'Decisions' in /state region: {region}"));
    let oq_pos = region
        .find("Open Questions")
        .unwrap_or_else(|| panic!("missing 'Open Questions' in /state region: {region}"));
    let ai_pos = region
        .find("Action Items")
        .unwrap_or_else(|| panic!("missing 'Action Items' in /state region: {region}"));

    assert!(
        dec_pos < oq_pos,
        "Decisions must appear before Open Questions in /state output. \
         dec_pos={dec_pos} oq_pos={oq_pos} region={region}"
    );
    assert!(
        oq_pos < ai_pos,
        "Open Questions must appear before Action Items in /state output. \
         oq_pos={oq_pos} ai_pos={ai_pos} region={region}"
    );
}

/// Security S1: any ANSI escape sequence (e.g. `\x1b[2J` clear-screen)
/// embedded in an LLM-sourced message must be stripped before /state
/// renders the bullet content to the terminal — otherwise an attacker
/// controlling the LLM could reposition the cursor, clear the screen,
/// or hijack the operator's terminal session.
#[test]
#[serial]
fn repl_state_strips_ansi_escapes_from_llm_sourced_content_before_rendering() {
    // Force NO_COLOR off so the REPL itself emits ANSI for headings — but
    // any ANSI in the LLM content must still be stripped.
    unsafe { std::env::remove_var("NO_COLOR") };

    let bridge = mock_bridge();
    // Embed a clear-screen escape inside the canned agent reply. After
    // sanitization the literal ESC byte must NOT appear inside any extracted
    // bullet shown by /state.
    let agent =
        MockAgentSession::new("Decision: Use \x1b[2JRust for the rewrite project unconditionally.");
    let input = b"Make a call\n/state\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Sanitize test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();

    // Locate the /state region (between first Decisions heading and next prompt).
    let region_start = output_str
        .find("Decisions")
        .unwrap_or_else(|| panic!("expected /state to render Decisions heading: {output_str}"));
    let region_end = output_str[region_start..]
        .find("simard:meeting>")
        .map(|i| region_start + i)
        .unwrap_or(output_str.len());
    let region = &output_str[region_start..region_end];

    // The decision text body should still be present (so we know the bullet
    // was rendered) but the malicious clear-screen escape must be stripped.
    assert!(
        region.contains("Rust for the rewrite"),
        "decision body should still be rendered after sanitization: {region}"
    );
    assert!(
        !region.contains("\x1b[2J"),
        "clear-screen escape from LLM content must be stripped from /state output: {region:?}"
    );
}

/// /help text must mention the new /state command so operators discover it.
#[test]
#[serial]
fn repl_help_mentions_state_command() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/help\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Help mentions /state",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("/state"),
        "help text should advertise the new /state command: {output_str}"
    );
}

// ── Inline /decision /action /question (issue #1730 seam (b)) ─────────

#[test]
#[serial]
fn repl_help_mentions_inline_recording_commands() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/help\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Help mentions inline commands",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    for cmd in ["/decision", "/action", "/question"] {
        assert!(
            output_str.contains(cmd),
            "help should advertise {cmd}: {output_str}"
        );
    }
}

#[test]
#[serial]
fn repl_decision_command_records_and_confirms() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/decision Adopt TDD for new modules\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Decision recording test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Decision recorded:"),
        "REPL should confirm explicit decision: {output_str}"
    );
    assert!(
        output_str.contains("Adopt TDD for new modules"),
        "REPL should echo the decision text back: {output_str}"
    );
}

#[test]
#[serial]
fn repl_action_command_records_and_confirms() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/action Bob will write tests by friday\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Action recording test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Action recorded:"),
        "REPL should confirm explicit action item: {output_str}"
    );
    assert!(
        output_str.contains("write tests"),
        "REPL should echo the action text back: {output_str}"
    );
}

#[test]
#[serial]
fn repl_question_command_records_and_confirms() {
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/question What is our SLO target?\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Question recording test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Question recorded:"),
        "REPL should confirm explicit open question: {output_str}"
    );
    assert!(
        output_str.contains("What is our SLO target?"),
        "REPL should echo the question text back: {output_str}"
    );
}

#[test]
#[serial]
fn repl_state_shows_explicit_items_immediately() {
    // /decision, /action, /question add items that don't enter the
    // conversation history; /state must still surface them so the operator
    // sees the running list.
    let bridge = mock_bridge();
    let agent = MockAgentSession::new("ok");
    let input = b"/decision Use Rust for CLI\n/action Carol will set up CI by next sprint\n/question Who owns rollout?\n/state\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Inline state test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("Use Rust for CLI"),
        "/state should show explicit decision: {output_str}"
    );
    assert!(
        output_str.contains("Who owns rollout?"),
        "/state should show explicit question: {output_str}"
    );
    assert!(
        output_str.contains("Carol will set up CI") || output_str.contains("set up CI"),
        "/state should show explicit action item: {output_str}"
    );
    assert!(
        output_str.contains("*(explicit)*"),
        "/state should mark explicit questions as such: {output_str}"
    );
}

// ── Structured backend-error banner (issue #1983) ───────────────────────
//
// When `send_message` returns `Err`, the REPL should render a structured
// `[meeting:error]` banner with severity, source, and recovery hint —
// instead of the old inline yellow `[agent error: …]` line. The closed
// session should report the orphan-turn count.

#[test]
#[serial]
fn renders_structured_banner_on_agent_error() {
    // SAFETY: serial_test guards against parallel env mutations.
    unsafe { std::env::set_var("NO_COLOR", "1") };

    let bridge = mock_bridge();
    // First call fails, second call succeeds, then /close.
    let agent = FailingThenOkMockAgent::new(1, "recovered");
    let input = b"this will fail\nthis will succeed\n/close\n";
    let mut reader = &input[..];
    let mut output = Vec::new();

    run_meeting_repl(
        "Error banner test",
        &bridge,
        Some(Box::new(agent)),
        "",
        &mut reader,
        &mut output,
    )
    .unwrap();

    unsafe { std::env::remove_var("NO_COLOR") };

    let output_str = String::from_utf8(output).unwrap();

    // 1. Structured banner must contain the stable marker.
    assert!(
        output_str.contains("[meeting:error] WARNING: backend error"),
        "should contain stable [meeting:error] marker: {output_str}"
    );

    // 2. Banner must include the source label.
    assert!(
        output_str.contains("source=conversation"),
        "should identify source=conversation: {output_str}"
    );

    // 3. Banner must include severity classification.
    assert!(
        output_str.contains("severity=transient"),
        "simulated failure should classify as transient: {output_str}"
    );

    // 4. Recovery hint must be present.
    assert!(
        output_str.contains("meeting is still usable"),
        "transient banner should include usability hint: {output_str}"
    );

    // 5. Old inline format must NOT appear.
    assert!(
        !output_str.contains("[agent error:"),
        "old [agent error: …] format must not appear: {output_str}"
    );

    // 6. The successful second turn should still render normally.
    assert!(
        output_str.contains("recovered"),
        "second turn should succeed and render: {output_str}"
    );

    // 7. Close-time orphan-turn banner should report exactly 1 orphan.
    assert!(
        output_str.contains("[meeting] WARNING: 1 orphan turn has no assistant reply"),
        "close banner should report orphan turn count: {output_str}"
    );
}
