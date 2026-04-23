//! TDD smoke tests for the dashboard Subagent Sessions card and the
//! Recent Actions Attach deep-link renderer.
//!
//! These assert that the embedded `INDEX_HTML` contains the expected JS
//! helper, the registry session-name prefix, and the agent_id-extracting
//! regex source. They will fail until Step 8 wires the UI in.

use super::routes::INDEX_HTML;

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

// =====================================================================
// Stewardship + Self-Understanding (issue #1172)
//
// These tests pin the static UI/API contract for the new "Stewardship"
// tab and its two cards (Repos Under Stewardship, Self-Understanding).
// They will FAIL until Step 8 wires the markup, JS, and routes in.
// =====================================================================

#[test]
fn index_html_has_stewardship_tab_in_tab_bar() {
    assert!(
        INDEX_HTML.contains(r#"data-tab="stewardship""#),
        "INDEX_HTML must declare the Stewardship tab via \
         <div class=\"tab\" data-tab=\"stewardship\">…</div> in the tab bar"
    );
}

#[test]
fn index_html_has_stewardship_tab_content_panel() {
    assert!(
        INDEX_HTML.contains(r#"id="tab-stewardship""#),
        "INDEX_HTML must declare the Stewardship tab-content panel via \
         <div class=\"tab-content\" id=\"tab-stewardship\">"
    );
}

#[test]
fn index_html_has_repos_under_stewardship_card() {
    assert!(
        INDEX_HTML.contains("Repos Under Stewardship"),
        "INDEX_HTML must render a 'Repos Under Stewardship' card heading \
         inside the Stewardship tab"
    );
}

#[test]
fn index_html_has_self_understanding_card() {
    assert!(
        INDEX_HTML.contains("Self-Understanding"),
        "INDEX_HTML must render a 'Self-Understanding' card heading \
         inside the Stewardship tab"
    );
}

#[test]
fn index_html_calls_stewardship_api() {
    assert!(
        INDEX_HTML.contains("/api/stewardship"),
        "INDEX_HTML must fetch /api/stewardship to populate the \
         Repos Under Stewardship card"
    );
}

#[test]
fn index_html_calls_self_understanding_api() {
    assert!(
        INDEX_HTML.contains("/api/self-understanding"),
        "INDEX_HTML must fetch /api/self-understanding to populate the \
         Self-Understanding card"
    );
}

#[test]
fn index_html_stewardship_uses_textcontent_not_innerhtml() {
    // XSS hardening: any JS fragment that handles stewardship/self-understanding
    // dynamic data MUST use textContent or innerText, never innerHTML, when
    // injecting user/file-controlled strings (notes, repo, role, metric values).
    //
    // Heuristic: locate the stewardship loader region and assert it does not
    // assign to innerHTML for dynamic field injection.
    let needle = "/api/stewardship";
    let pos = INDEX_HTML
        .find(needle)
        .expect("INDEX_HTML must reference /api/stewardship before this assertion runs");
    // Inspect a generous window around the loader to catch nearby JS.
    let start = pos.saturating_sub(200);
    let end = (pos + 1500).min(INDEX_HTML.len());
    let region = &INDEX_HTML[start..end];
    assert!(
        !region.contains(".innerHTML ="),
        "Stewardship loader JS must use textContent / innerText for dynamic \
         field injection, not innerHTML (XSS hardening). Offending region:\n{region}"
    );
}
