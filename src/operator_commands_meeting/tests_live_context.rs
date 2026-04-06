use super::live_context::*;
use super::test_support::*;

// ── build_live_meeting_context ──────────────────────────────────────

#[test]
fn defaults_with_empty_bridge() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge);

    assert!(
        ctx.starts_with("## Live State (from cognitive memory)"),
        "expected live-state header, got: {ctx}"
    );
    assert!(
        ctx.contains("## Operator Context"),
        "expected default operator section"
    );
    assert!(ctx.contains("Ryan Sweet"), "expected default operator name");
    assert!(
        ctx.contains("## Known Projects"),
        "expected default projects section"
    );
    assert!(
        ctx.contains("Simard"),
        "expected Simard in default projects"
    );
}

#[test]
fn includes_bridge_meeting_facts() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge);

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
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
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
    let bridge =
        bridge_with_specific_facts("operator:", "operator", "Custom operator identity");
    let ctx = build_live_meeting_context(&bridge);
    assert!(
        ctx.contains("Operator Context"),
        "expected operator context section"
    );
    assert!(
        ctx.contains("Custom operator identity"),
        "expected operator content from bridge"
    );
    // Should NOT contain the default operator text when bridge provides facts
    assert!(
        !ctx.contains("Ryan Sweet"),
        "should not contain default operator when bridge provides custom operator"
    );
}

#[test]
fn includes_project_facts() {
    let bridge =
        bridge_with_specific_facts("project:", "project", "CustomProject — custom suite");
    let ctx = build_live_meeting_context(&bridge);
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
    let bridge =
        bridge_with_specific_facts("research:", "research", "Investigating LLM patterns");
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
    // Even with only defaults, the sections are present so it uses the live header
    assert!(ctx.starts_with("## Live State"));
}

#[test]
fn no_defaults_when_operator_present() {
    let bridge = bridge_with_specific_facts("operator:", "operator", "Custom operator");
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
    // When project facts present, should use bridge data not defaults
    assert!(ctx.contains("My Custom Project"));
}

#[test]
fn contains_numbered_items() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("1."), "meeting summaries should be numbered");
}

#[test]
fn has_markdown_headers() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
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
fn empty_bridge_has_at_least_two_sections() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge);
    // Even with empty bridge, default operator and projects sections appear
    let section_count = ctx.matches("## ").count();
    assert!(
        section_count >= 2,
        "expected at least 2 sections, got {section_count}"
    );
}

#[test]
fn empty_bridge_includes_known_projects() {
    let bridge = empty_bridge();
    let ctx = build_live_meeting_context(&bridge);
    assert!(
        ctx.contains("RustyClawd"),
        "expected RustyClawd in defaults"
    );
    assert!(ctx.contains("amplihack"), "expected amplihack in defaults");
}

#[test]
fn live_state_header_always_present() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.starts_with("## Live State"));
}

#[test]
fn with_all_types_does_not_contain_no_memory_fallback() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
    assert!(!ctx.contains("No cognitive memory available"));
}

#[test]
fn meeting_facts_numbered() {
    let bridge = bridge_with_meeting_facts();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("1. "), "items should be numbered");
}

// ── validate each category uses bullet points ──────────────────────

#[test]
fn research_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("research:", "research", "LLM alignment research");
    let ctx = build_live_meeting_context(&bridge);
    assert!(
        ctx.contains("- LLM alignment research"),
        "research section should use bullet points"
    );
}

#[test]
fn improvement_section_is_bulleted() {
    let bridge =
        bridge_with_specific_facts("improvement:", "improvement", "Better error recovery");
    let ctx = build_live_meeting_context(&bridge);
    assert!(
        ctx.contains("- Better error recovery"),
        "improvement section should use bullet points"
    );
}

#[test]
fn operator_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("operator:", "operator", "Custom operator context");
    let ctx = build_live_meeting_context(&bridge);
    assert!(
        ctx.contains("- Custom operator context"),
        "operator section should use bullet points"
    );
}

#[test]
fn project_section_is_bulleted() {
    let bridge = bridge_with_specific_facts("project:", "project", "CustomProject — testing");
    let ctx = build_live_meeting_context(&bridge);
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
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("Sprint review completed"));
}

#[test]
fn all_types_contains_migration_plan() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("Approved migration plan"));
}

#[test]
fn all_types_contains_api_refactor() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("Complete API refactor"));
}

#[test]
fn all_types_contains_error_handling() {
    let bridge = bridge_with_all_fact_types();
    let ctx = build_live_meeting_context(&bridge);
    assert!(ctx.contains("Add better error handling"));
}
