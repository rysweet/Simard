use super::live_context::*;
use super::test_support::*;

/// Mutex to serialize tests that mutate environment variables. `set_var` /
/// `remove_var` are process-global so concurrent tests would race.
static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ── build_live_meeting_context ──────────────────────────────────────

#[test]
fn defaults_with_empty_bridge() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // Ensure the env var is unset so we test the true default path.
    unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };

    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();

    assert!(
        ctx.starts_with("## Live State (from cognitive memory)"),
        "expected live-state header, got: {ctx}"
    );
    assert!(
        ctx.contains("## Operator Context"),
        "expected default operator section"
    );
    // No hardcoded personal names — falls back to env var or "operator"
    assert!(
        !ctx.contains("Ryan Sweet"),
        "must not contain hardcoded personal name"
    );
    assert!(
        ctx.contains("operator"),
        "expected generic operator fallback"
    );
    // No hardcoded project list when memory is empty
    assert!(
        !ctx.contains("## Known Projects"),
        "projects section should be omitted when memory has no project facts"
    );
}

#[test]
fn empty_bridge_uses_env_var_operator_name() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: Serialized by ENV_MUTEX; restored immediately after use.
    unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "Test User") };
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };

    assert!(
        ctx.contains("Test User"),
        "expected operator name from SIMARD_OPERATOR_NAME env var"
    );
    assert!(
        !ctx.contains("Ryan Sweet"),
        "must not contain hardcoded name"
    );
}

#[test]
fn empty_env_var_falls_back_to_generic() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // SAFETY: Serialized by ENV_MUTEX; restored immediately after use.
    unsafe { std::env::set_var("SIMARD_OPERATOR_NAME", "") };
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    unsafe { std::env::remove_var("SIMARD_OPERATOR_NAME") };

    assert!(
        ctx.contains("Your operator is operator"),
        "empty env var should fall back to generic 'operator'"
    );
}

#[test]
fn includes_bridge_meeting_facts() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge).unwrap();

    assert!(
        ctx.contains("Previous Meeting Summaries"),
        "expected meeting summaries section"
    );
    assert!(
        ctx.contains("Discussed deployment timeline"),
        "expected meeting content from bridge"
    );
}

#[test]
fn includes_decision_facts() {
    let bridge = bridge_with_specific_facts("decision:", "decision", "Use Rust for backend");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Past Decisions"),
        "expected past decisions section"
    );
    assert!(
        ctx.contains("Use Rust for backend"),
        "expected decision content"
    );
}

#[test]
fn includes_goal_facts() {
    let bridge = bridge_with_specific_facts("goal:", "goal", "Complete API refactor");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Active Goals"),
        "expected active goals section"
    );
    assert!(
        ctx.contains("Complete API refactor"),
        "expected goal content"
    );
}

#[test]
fn includes_operator_facts() {
    let bridge = bridge_with_specific_facts("operator:", "operator", "Custom operator identity");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Operator Context"),
        "expected operator context section"
    );
    assert!(
        ctx.contains("Custom operator identity"),
        "expected operator content from bridge"
    );
    // Should NOT contain any fallback operator text when bridge provides facts
    assert!(
        !ctx.contains("Ryan Sweet"),
        "should not contain default operator when bridge provides custom operator"
    );
}

#[test]
fn includes_project_facts() {
    let bridge = bridge_with_specific_facts("project:", "project", "CustomProject — custom suite");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Known Projects"),
        "expected known projects section"
    );
    assert!(
        ctx.contains("CustomProject"),
        "expected project content from bridge"
    );
}

#[test]
fn includes_research_facts() {
    let bridge = bridge_with_specific_facts("research:", "research", "Investigating LLM patterns");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Research Topics"),
        "expected research topics section"
    );
    assert!(
        ctx.contains("Investigating LLM patterns"),
        "expected research content"
    );
}

#[test]
fn includes_improvement_facts() {
    let bridge =
        bridge_with_specific_facts("improvement:", "improvement", "Add better error handling");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("Improvement Backlog"),
        "expected improvement backlog section"
    );
    assert!(
        ctx.contains("Add better error handling"),
        "expected improvement content"
    );
}

#[test]
fn with_all_fact_types() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("Previous Meeting Summaries"));
    assert!(ctx.contains("Past Decisions"));
    assert!(ctx.contains("Active Goals"));
    assert!(ctx.contains("Operator Context"));
    assert!(ctx.contains("Known Projects"));
    assert!(ctx.contains("Research Topics"));
    assert!(ctx.contains("Improvement Backlog"));
    // Should NOT contain the "No cognitive memory" fallback
    assert!(!ctx.contains("No cognitive memory available"));
}

#[test]
fn has_live_state_header() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    // Even with only the operator fallback, the section is present so it uses the live header
    assert!(ctx.starts_with("## Live State"));
}

#[test]
fn no_defaults_when_operator_present() {
    let bridge = bridge_with_specific_facts("operator:", "operator", "Custom operator");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    // When operator facts present, should NOT use default operator
    assert!(
        !ctx.contains("Ryan Sweet"),
        "should not have default operator"
    );
    assert!(ctx.contains("Custom operator"));
}

#[test]
fn no_defaults_when_project_present() {
    let bridge = bridge_with_specific_facts("project:", "proj", "My Custom Project");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    // When project facts present, should use bridge data not defaults
    assert!(ctx.contains("My Custom Project"));
}

#[test]
fn contains_numbered_items() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("1."), "meeting summaries should be numbered");
}

#[test]
fn has_markdown_headers() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    // All sections use ## headers
    let header_count = ctx.matches("## ").count();
    assert!(
        header_count >= 7,
        "expected at least 7 markdown headers, got {header_count}"
    );
}

// ── empty_bridge helper validation ─────────────────────────────────

#[test]
fn empty_bridge_returns_empty_search_results() {
    let bridge = empty_bridge();
    let facts = bridge
        .search_facts("anything:", 10, 0.0)
        .unwrap_or_default();
    assert!(facts.is_empty());
}

// ── structural checks ──────────────────────────────────────────────

#[test]
fn empty_bridge_has_operator_section_only() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    // With empty bridge, only the operator fallback section appears (no hardcoded projects)
    assert!(ctx.contains("## Operator Context"));
    assert!(!ctx.contains("## Known Projects"));
    let section_count = ctx.matches("## ").count();
    // Live State header + Operator Context = at least 2
    assert!(
        section_count >= 2,
        "expected at least 2 sections (header + operator), got {section_count}"
    );
}

#[test]
fn empty_bridge_omits_hardcoded_projects() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        !ctx.contains("RustyClawd"),
        "must not contain hardcoded project names"
    );
    assert!(
        !ctx.contains("amplihack-memory-lib"),
        "must not contain hardcoded project names"
    );
}

#[test]
fn live_state_header_always_present() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.starts_with("## Live State"));
}

#[test]
fn with_all_types_does_not_contain_no_memory_fallback() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(!ctx.contains("No cognitive memory available"));
}

#[test]
fn meeting_facts_numbered() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("1. "), "items should be numbered");
}

// ── validate each category uses bullet points ──────────────────────

#[test]
fn research_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("research:", "research", "LLM alignment research");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("- LLM alignment research"),
        "research section should use bullet points"
    );
}

#[test]
fn improvement_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("improvement:", "improvement", "Better error recovery");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("- Better error recovery"),
        "improvement section should use bullet points"
    );
}

#[test]
fn operator_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("operator:", "operator", "Custom operator context");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("- Custom operator context"),
        "operator section should use bullet points"
    );
}

#[test]
fn project_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("project:", "project", "CustomProject — testing");
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(
        ctx.contains("- CustomProject"),
        "project section should use bullet points"
    );
}

// ── empty_bridge: additional validation ─────────────────────────────

#[test]
fn empty_bridge_search_returns_empty_for_various_prefixes() {
    let bridge = empty_bridge();
    for prefix in &[
        "meeting:",
        "decision:",
        "goal:",
        "operator:",
        "project:",
        "research:",
        "improvement:",
    ] {
        let facts = bridge.search_facts(prefix, 10, 0.0).unwrap_or_default();
        assert!(facts.is_empty(), "expected empty for prefix {prefix}");
    }
}

// ── all_fact_types: specific content checks ────────────────────────

#[test]
fn all_types_contains_sprint_review() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("Sprint review completed"));
}

#[test]
fn all_types_contains_migration_plan() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("Approved migration plan"));
}

#[test]
fn all_types_contains_api_refactor() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("Complete API refactor"));
}

#[test]
fn all_types_contains_error_handling() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge).unwrap();
    assert!(ctx.contains("Add better error handling"));
}
