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

// ---------------------------------------------------------------------------
// Step 7 (TDD, issue #2717): Agent Terminal available-agents dropdown.
//
// These tests specify the contract for the Workers-tab "Agent Terminal"
// dropdown that lets the operator pick which live agent session to attach to.
// They fail until Step 8 wires the UI in.
//
// Design contract (from the locked design spec):
//   * A `<select id="agent-terminal-select">` lives in the Agent Terminal
//     control row; its `onchange` calls `onAgentTerminalSelect()`.
//   * `populateAgentSelect()` reads the SAME live source the Workers tab
//     already uses (`subagentSessionsCache.live[]`), so the list reflects
//     reality and rides the existing background refresh.
//   * `renderSubagentSessions()` invokes `populateAgentSelect()` so the
//     dropdown stays in sync with the 5s poll.
//   * `onAgentTerminalSelect()` attaches via the existing `openTmuxAttach()`
//     mechanism, using per-option data attributes (not the human-readable
//     label) as the attach target.
//   * The empty case renders an explicit "no agents available" state rather
//     than a broken control.
// ---------------------------------------------------------------------------

/// Return the `window` chars of `hay` immediately following the first
/// occurrence of `marker`. Returns `""` when the marker is absent, so a
/// region-scoped `contains` assertion fails cleanly before the code exists.
fn region_after<'a>(hay: &'a str, marker: &str, window: usize) -> &'a str {
    match hay.find(marker) {
        Some(i) => {
            let start = i + marker.len();
            let end = (start + window).min(hay.len());
            &hay[start..end]
        }
        None => "",
    }
}

#[test]
fn index_html_has_agent_terminal_select_dropdown() {
    assert!(
        INDEX_HTML.contains("agent-terminal-select"),
        "INDEX_HTML must include a <select id=\"agent-terminal-select\"> in the \
         Agent Terminal control row so the operator can pick which agent to attach to"
    );
}

#[test]
fn agent_terminal_select_onchange_wires_handler() {
    assert!(
        INDEX_HTML.contains("onAgentTerminalSelect()"),
        "The agent-terminal-select dropdown must call onAgentTerminalSelect() on change \
         so picking an agent attaches the terminal to that session"
    );
}

#[test]
fn index_html_defines_populate_agent_select_function() {
    assert!(
        INDEX_HTML.contains("function populateAgentSelect"),
        "INDEX_HTML must define populateAgentSelect() to build the dropdown options"
    );
}

#[test]
fn populate_agent_select_reads_live_subagent_cache() {
    let body = region_after(&INDEX_HTML, "function populateAgentSelect", 900);
    assert!(
        body.contains("subagentSessionsCache"),
        "populateAgentSelect() must read the shared live source subagentSessionsCache \
         (single live reader — no stale/hardcoded agent list)"
    );
    assert!(
        body.contains(".live"),
        "populateAgentSelect() must populate from subagentSessionsCache.live[] \
         (the attachable/live sessions), not recently-ended ones"
    );
}

#[test]
fn render_subagent_sessions_invokes_populate_agent_select() {
    let body = region_after(&INDEX_HTML, "function renderSubagentSessions", 1400);
    assert!(
        body.contains("populateAgentSelect("),
        "renderSubagentSessions() must call populateAgentSelect() so the dropdown \
         rides the existing background refresh and stays current"
    );
}

#[test]
fn agent_terminal_select_handler_attaches_via_open_tmux_attach() {
    let body = region_after(&INDEX_HTML, "function onAgentTerminalSelect", 900);
    assert!(
        !body.is_empty(),
        "INDEX_HTML must define onAgentTerminalSelect() to handle dropdown selection"
    );
    assert!(
        body.contains("openTmuxAttach("),
        "onAgentTerminalSelect() must attach through the existing openTmuxAttach() \
         mechanism so choosing an agent switches the terminal target"
    );
}

#[test]
fn agent_select_wires_attach_target_from_data_attributes() {
    // Attach target must come from per-option data attributes carrying the
    // real host + tmux session_name — never from the human-readable label.
    let populate = region_after(&INDEX_HTML, "function populateAgentSelect", 900);
    assert!(
        populate.contains("session_name"),
        "populateAgentSelect() must carry each session's real session_name onto the \
         <option> (e.g. via a data-session attribute) as the attach target"
    );
    let handler = region_after(&INDEX_HTML, "function onAgentTerminalSelect", 900);
    assert!(
        handler.contains("dataset"),
        "onAgentTerminalSelect() must read the selected option's dataset (host/session) \
         to build the attach target rather than parsing the display label"
    );
}

#[test]
fn agent_terminal_select_has_no_agents_available_empty_state() {
    assert!(
        INDEX_HTML.contains("no agents available"),
        "The Agent Terminal dropdown must render an explicit 'no agents available' \
         empty state when subagentSessionsCache.live[] is empty (not a broken control)"
    );
}

#[test]
fn agent_select_falls_back_to_neutral_prompt_for_stale_pick() {
    // #2717 review (Finding A): when the operator's previously-selected agent
    // leaves the live roster, the dropdown must fall back to a neutral, disabled
    // prompt rather than silently repointing the label at the first live agent —
    // which would name a different agent than the terminal stays attached to.
    assert!(
        INDEX_HTML.contains("select an agent"),
        "populateAgentSelect() must keep a neutral 'select an agent' prompt option so a \
         stale prior pick falls back to it instead of an unrelated live agent"
    );
    assert!(
        INDEX_HTML.contains("prompt.selected=true"),
        "populateAgentSelect() must select the neutral prompt when the prior pick is no \
         longer live (design intent: never jump the label to a different agent)"
    );
}
