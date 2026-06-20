//! TDD smoke tests for the dashboard Subagent Sessions card and the
//! Recent Actions Attach deep-link renderer.
//!
//! These assert that the embedded `INDEX_HTML` contains the expected JS
//! helper, the registry session-name prefix, and the agent_id-extracting
//! regex source. They will fail until Step 8 wires the UI in.

use super::index_html::INDEX_HTML;

#[test]
fn index_html_defines_render_action_detail_helper() {
    assert!(
        INDEX_HTML.contains("renderActionDetail"),
        "INDEX_HTML must define a shared renderActionDetail helper used by \
         both the overview and workboard Recent Actions renderers"
    );
}

#[test]
fn index_html_references_simard_engineer_session_prefix() {
    assert!(
        INDEX_HTML.contains("simard-engineer-"),
        "INDEX_HTML must reference the 'simard-engineer-' tmux session prefix \
         (used to construct attach commands)"
    );
}

#[test]
fn index_html_contains_agent_id_extraction_regex() {
    assert!(
        INDEX_HTML.contains("agent='(engineer-"),
        "INDEX_HTML must contain the agent='engineer-...' regex source \
         used to extract agent_id from outcome detail strings"
    );
}

#[test]
fn index_html_has_subagent_sessions_card() {
    assert!(
        INDEX_HTML.contains("subagent-sessions"),
        "INDEX_HTML must include the SubagentSessions dashboard card \
         (id=\"subagent-sessions\")"
    );
}

#[test]
fn index_html_calls_subagent_sessions_api() {
    assert!(
        INDEX_HTML.contains("/api/subagent-sessions"),
        "INDEX_HTML must fetch /api/subagent-sessions for the live registry"
    );
}

#[test]
fn index_html_has_attach_button_class_or_label() {
    let has_class = INDEX_HTML.contains("attach-btn");
    let has_label = INDEX_HTML.contains("Attach");
    assert!(
        has_class && has_label,
        "INDEX_HTML must render Attach buttons (class=\"attach-btn\" + label \"Attach\")"
    );
}

#[test]
fn index_html_workboard_working_memory_count_uses_working_count() {
    // #1679: the Workboard "Working Memory" count must read the authoritative
    // working_count statistic (the same value the Memory tab shows), not the
    // length of the per-active-goal working_memory array, so the two panels
    // can no longer disagree.
    assert!(
        INDEX_HTML.contains("d.cognitive_statistics.working_count"),
        "INDEX_HTML must bind the Workboard working-memory count to \
         cognitive_statistics.working_count (#1679)"
    );
    assert!(
        INDEX_HTML.contains("wmCount+' slots'"),
        "INDEX_HTML must render the working-memory count from wmCount (#1679)"
    );
}
