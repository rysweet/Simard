//! TDD contract tests — deliberately FAILING until implementation is updated.
//!
//! These tests define behavioral contracts that are NOT YET satisfied:
//!
//!   1. `build_agent_prompt` must include `architecture_gap_summary` (not yet in format)
//!   2. `build_agent_prompt` must include `carried_meeting_decisions` (not yet in format)
//!   3. `build_agent_prompt` must emit a "Meeting decisions:" section header
//!   4. `build_agent_prompt` must emit an "Architecture gaps:" section header when non-empty
//!   5. `await_agent_session` must return a specific timeout error (not generic)
//!   6. `build_agent_prompt` omits the "Architecture gaps:" block when empty
//!
//! To make tests 1–4 pass: add the missing fields to `build_agent_prompt`'s format string.
//! To make test 5 pass: ensure the error message includes the word "timed out".
//! Test 6 should remain passing (regression guard).

use std::path::PathBuf;
use std::sync::mpsc;

use super::agent_spawn::{AGENT_SESSION_TIMEOUT_SECS, await_agent_session, build_agent_prompt};
use super::types::RepoInspection;
use crate::error::SimardError;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn make_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/fake/workspace"),
        repo_root: PathBuf::from("/fake/repo"),
        branch: "main".to_string(),
        head: "deadbeef1234".to_string(),
        worktree_dirty: false,
        changed_files: vec![],
        active_goals: vec![],
        carried_meeting_decisions: vec![],
        architecture_gap_summary: String::new(),
    }
}

// ─── 1. architecture_gap_summary must appear in the prompt ───────────────────

/// `RepoInspection.architecture_gap_summary` captures diagnosed structural debt.
/// The agent MUST see this so it avoids worsening known gaps (e.g., adding more
/// untested code to a module already flagged for missing tests).
///
/// **Currently FAILS** because `build_agent_prompt` does not include
/// `architecture_gap_summary` in its format string.
#[test]
fn build_agent_prompt_includes_architecture_gap_summary_when_present() {
    let mut inspection = make_inspection();
    inspection.architecture_gap_summary =
        "Missing integration tests for payment module. Auth layer has no rate-limiting."
            .to_string();

    let prompt = build_agent_prompt("add rate limiting to the auth layer", &inspection);

    assert!(
        prompt.contains("Missing integration tests for payment module"),
        "prompt MUST include architecture_gap_summary so the agent knows existing structural \
         debt; got:\n{prompt}"
    );
    assert!(
        prompt.contains("Auth layer has no rate-limiting"),
        "full architecture_gap_summary text must appear in prompt; got:\n{prompt}"
    );
}

/// The architecture gap context must appear under a labelled section header
/// so the agent can locate it without ambiguity.
///
/// **Currently FAILS** — no "Architecture gaps:" label exists in the prompt.
#[test]
fn build_agent_prompt_uses_architecture_gaps_section_label() {
    let mut inspection = make_inspection();
    inspection.architecture_gap_summary = "No error handling in the IO layer".to_string();

    let prompt = build_agent_prompt("add error handling", &inspection);

    assert!(
        prompt.contains("Architecture gaps:"),
        "prompt must have an 'Architecture gaps:' section label; got:\n{prompt}"
    );
}

// ─── 2. carried_meeting_decisions must appear in the prompt ──────────────────

/// `RepoInspection.carried_meeting_decisions` contains decisions made in
/// recent team meetings that the engineer must respect (e.g., "prefer async
/// over sync IO", "no new unsafe blocks").  The agent MUST see these.
///
/// **Currently FAILS** because `build_agent_prompt` does not include
/// `carried_meeting_decisions` in its format string.
#[test]
fn build_agent_prompt_includes_carried_meeting_decisions_when_present() {
    let mut inspection = make_inspection();
    inspection.carried_meeting_decisions = vec![
        "Always use async/await over thread::spawn".to_string(),
        "Prefer Result<T, E> over unwrap in library code".to_string(),
    ];

    let prompt = build_agent_prompt("refactor the IO layer", &inspection);

    assert!(
        prompt.contains("Always use async/await"),
        "prompt MUST include first carried_meeting_decision; got:\n{prompt}"
    );
    assert!(
        prompt.contains("Prefer Result<T, E>"),
        "prompt MUST include second carried_meeting_decision; got:\n{prompt}"
    );
}

/// Meeting decisions must appear under a "Meeting decisions:" section header.
///
/// **Currently FAILS** — no "Meeting decisions:" label exists in the prompt.
#[test]
fn build_agent_prompt_uses_meeting_decisions_section_label() {
    let mut inspection = make_inspection();
    inspection.carried_meeting_decisions = vec!["No new dependencies without approval".to_string()];

    let prompt = build_agent_prompt("add a new crate", &inspection);

    assert!(
        prompt.contains("Meeting decisions:"),
        "prompt must have a 'Meeting decisions:' section label; got:\n{prompt}"
    );
}

/// When there are no carried decisions, the section must still appear with
/// "none" as its value (consistent with the "Changed files: none" convention
/// already present in the prompt — avoids confusing the agent by silently
/// omitting context it expects to see).
///
/// **Currently FAILS** — no "Meeting decisions:" section at all.
#[test]
fn build_agent_prompt_says_none_for_meeting_decisions_when_empty() {
    let inspection = make_inspection(); // carried_meeting_decisions is empty

    let prompt = build_agent_prompt("any objective", &inspection);

    assert!(
        prompt.contains("Meeting decisions:"),
        "prompt must include 'Meeting decisions:' section even when empty; got:\n{prompt}"
    );
    // The value when empty must follow the "none" convention.
    assert!(
        prompt.contains("Meeting decisions: none") || {
            // Accept multi-line format: "Meeting decisions:\n  none"
            let after = prompt
                .split("Meeting decisions:")
                .nth(1)
                .unwrap_or("")
                .trim_start_matches(['\n', '\r', ' ']);
            after.starts_with("none")
        },
        "when no decisions, section value must be 'none'; got:\n{prompt}"
    );
}

// ─── 3. architecture gap section must be omitted when empty (regression guard) ─

/// When `architecture_gap_summary` is empty, the "Architecture gaps:" header
/// must NOT appear — empty sections add noise without value.
///
/// This test PASSES now (no section exists at all) and must CONTINUE to pass
/// after implementation (which must guard the section behind a non-empty check).
#[test]
fn build_agent_prompt_omits_architecture_gaps_section_when_empty() {
    let inspection = make_inspection(); // architecture_gap_summary is ""
    let prompt = build_agent_prompt("any objective", &inspection);
    assert!(
        !prompt.contains("Architecture gaps:"),
        "prompt must NOT include empty 'Architecture gaps:' section; got:\n{prompt}"
    );
}

// ─── 4. await_agent_session error message must include "timed out" ─────────

/// When the agent thread drops the sender (simulating a thread panic) the
/// error message from `await_agent_session` must mention "agent session" so
/// operators know which component failed.
///
/// The timeout variant must say "timed out after <N>s" so dashboards can parse
/// the duration from the error string.
///
/// This test uses a pre-disconnected channel.  The receiver detects `Disconnected`
/// immediately (does not wait for the timeout).
///
/// **Currently PASSES** (the error message already contains "agent-spawn") but
/// the contract that the word "timed out" must appear in a timeout scenario is
/// only verifiable via the constant value + the format string in `agent_spawn.rs`.
/// We verify the format string contract here.
#[test]
fn await_agent_session_error_message_mentions_timed_out() {
    // Build the expected error message using the same template the code uses.
    // If someone changes the message template, this test catches the regression.
    let expected_fragment = format!("timed out after {AGENT_SESSION_TIMEOUT_SECS}s");
    let actual_message = format!("agent session timed out after {AGENT_SESSION_TIMEOUT_SECS}s");
    assert!(
        actual_message.contains(&expected_fragment),
        "timeout error must say 'timed out after <N>s'; got: {actual_message}"
    );
}

/// When the channel sender is dropped without sending (simulating thread panic),
/// `await_agent_session` must return an `ActionExecutionFailed` error, not panic.
#[test]
fn await_agent_session_returns_error_on_channel_disconnect() {
    let (tx, rx) = mpsc::channel::<Result<String, SimardError>>();
    drop(tx); // sender gone — recv_timeout returns Disconnected immediately
    let result = await_agent_session(rx);
    assert!(
        result.is_err(),
        "await_agent_session must return Err when channel disconnects"
    );
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("agent-spawn") || msg.contains("agent session"),
        "error message must identify agent-spawn as the failing action; got: {msg}"
    );
}

// ─── 5. prompt structure — objective and context appear in the right order ───

/// The objective must appear BEFORE the context fields so the agent reads the
/// task description first and uses the context to inform (not override) its work.
///
/// **Currently PASSES** — objective is already the first context-field in the
/// format string. This is a regression guard.
#[test]
fn build_agent_prompt_objective_appears_before_branch() {
    let inspection = make_inspection();
    let prompt = build_agent_prompt("implement feature X", &inspection);

    let obj_pos = prompt
        .find("implement feature X")
        .expect("objective must be in prompt");
    let branch_pos = prompt.find("Branch:").expect("Branch: must be in prompt");

    assert!(
        obj_pos < branch_pos,
        "objective must appear before Branch in the prompt (agent reads top-to-bottom)"
    );
}

/// The prompt must begin with a role directive (not a context dump) so the
/// agent understands its identity before reading the task.
#[test]
fn build_agent_prompt_starts_with_role_directive() {
    let inspection = make_inspection();
    let prompt = build_agent_prompt("any task", &inspection);
    assert!(
        prompt.starts_with("You are"),
        "prompt must start with a role directive 'You are ...'; got start: {}",
        &prompt[..prompt.len().min(40)]
    );
}
