//! Tests for [`super::tab_meta`] and the cross-check between
//! `TAB_METADATA` and the rendered `INDEX_HTML`. These tests are the
//! Rust half of the Tab-Identity Contract (#1993 / #1994 / #1995); the
//! other half is `tests/e2e-dashboard/smoke_python/test_tab_clarity.py`.

#![cfg(test)]

use super::INDEX_HTML;
use super::tab_meta::{BANNED_JARGON, TAB_METADATA, default_title, tab_meta_js, tab_nav_html};
use std::collections::HashSet;

#[test]
fn tab_meta_slugs_unique() {
    let mut seen = HashSet::new();
    for t in TAB_METADATA {
        assert!(
            seen.insert(t.slug),
            "duplicate slug {:?} in TAB_METADATA",
            t.slug
        );
    }
    assert_eq!(TAB_METADATA.len(), 14, "expected 14 tabs");
}

#[test]
fn tab_meta_labels_unique() {
    let mut seen = HashSet::new();
    for t in TAB_METADATA {
        assert!(
            seen.insert(t.label),
            "duplicate label {:?} in TAB_METADATA",
            t.label
        );
    }
}

#[test]
fn tab_meta_titles_unique_and_non_empty() {
    let mut seen = HashSet::new();
    for t in TAB_METADATA {
        assert!(!t.title.is_empty(), "tab {:?} has empty title", t.slug);
        assert!(
            seen.insert(t.title),
            "duplicate title {:?} in TAB_METADATA",
            t.title
        );
    }
}

#[test]
fn tab_meta_h1s_unique_and_non_empty() {
    let mut seen = HashSet::new();
    for t in TAB_METADATA {
        assert!(!t.h1.is_empty(), "tab {:?} has empty h1", t.slug);
        assert!(seen.insert(t.h1), "duplicate h1 {:?} in TAB_METADATA", t.h1);
    }
}

#[test]
fn tab_meta_titles_follow_label_dot_simard_format() {
    // Every title is "{H1} · Simard". H1 (not label, since some labels
    // carry decorative emoji like "🧠 Thinking") drives the format so
    // the browser-tab text stays tidy.
    for t in TAB_METADATA {
        let expected = format!("{} · Simard", t.h1);
        assert_eq!(
            t.title, expected,
            "tab {:?} title must be {:?}, got {:?}",
            t.slug, expected, t.title
        );
    }
}

#[test]
fn tab_meta_ledes_non_empty_and_single_sentence_ish() {
    for t in TAB_METADATA {
        assert!(!t.lede.is_empty(), "tab {:?} has empty lede", t.slug);
        // Sentence-ish: ends in a period or other terminal punctuation.
        let last = t.lede.chars().last().expect("non-empty lede");
        assert!(
            matches!(last, '.' | '!' | '?'),
            "tab {:?} lede should end in terminal punctuation, got {:?}",
            t.slug,
            t.lede
        );
        // Plain-English bar: at least 40 chars so it actually explains
        // something. Anything shorter is almost certainly a label echo.
        assert!(
            t.lede.len() >= 40,
            "tab {:?} lede is suspiciously short ({} chars): {:?}",
            t.slug,
            t.lede.len(),
            t.lede
        );
    }
}

#[test]
fn tab_meta_ledes_no_banned_jargon() {
    for t in TAB_METADATA {
        for banned in BANNED_JARGON {
            assert!(
                !t.lede.contains(banned),
                "tab {:?} lede contains banned jargon {:?}: {:?}",
                t.slug,
                banned,
                t.lede
            );
        }
    }
}

#[test]
fn tab_meta_tooltips_substantive() {
    // Tooltips need to be ≥18 chars (same threshold as the existing
    // `index_html_tab_tooltips_are_substantive` check in
    // tests_routes_a.rs).
    for t in TAB_METADATA {
        assert!(
            t.tooltip.len() >= 18,
            "tab {:?} tooltip is too short ({} chars): {:?}",
            t.slug,
            t.tooltip.len(),
            t.tooltip
        );
    }
}

#[test]
fn tab_meta_js_is_valid_json_assignment() {
    let js = tab_meta_js();
    assert!(js.starts_with("<script>window.__TAB_META="));
    assert!(js.ends_with(";</script>"));
    // Extract the JSON payload and round-trip it.
    let payload = js
        .trim_start_matches("<script>window.__TAB_META=")
        .trim_end_matches(";</script>");
    // The payload may contain "\u003c" escapes for `<`; un-escape so
    // serde_json can parse it.
    let unescaped = payload.replace("\\u003c", "<");
    let parsed: serde_json::Value =
        serde_json::from_str(&unescaped).expect("__TAB_META payload must parse as JSON");
    let obj = parsed.as_object().expect("__TAB_META must be an object");
    assert_eq!(obj.len(), TAB_METADATA.len());
    for t in TAB_METADATA {
        let entry = obj
            .get(t.slug)
            .unwrap_or_else(|| panic!("__TAB_META missing slug {:?}", t.slug));
        assert_eq!(entry["title"], t.title);
        assert_eq!(entry["h1"], t.h1);
        assert_eq!(entry["label"], t.label);
    }
}

#[test]
fn tab_meta_js_resists_script_breakout() {
    // The JS payload must escape `<` so that a future lede or title
    // containing `</script>` cannot terminate the inline script tag.
    let js = tab_meta_js();
    assert!(
        !js[js.find("=").unwrap()..js.rfind(";").unwrap()].contains("</"),
        "tab_meta_js payload must not contain a literal `</` sequence"
    );
}

#[test]
fn default_title_is_first_tab_title() {
    assert_eq!(default_title(), TAB_METADATA[0].title);
}

// ----- Cross-check: SoT ↔ rendered INDEX_HTML -----

#[test]
fn rendered_html_contains_every_label() {
    for t in TAB_METADATA {
        assert!(
            INDEX_HTML.contains(t.label),
            "rendered INDEX_HTML missing tab label {:?} — the nav bar and \
             TAB_METADATA have drifted",
            t.label
        );
    }
}

#[test]
fn rendered_html_contains_every_h1() {
    // Each h1 should appear inside `<h1 class="page-h1">…</h1>`.
    for t in TAB_METADATA {
        let needle = format!(r#"<h1 class="page-h1">{}</h1>"#, t.h1);
        assert!(
            INDEX_HTML.contains(&needle),
            "rendered INDEX_HTML missing per-tab h1 for slug {:?}; \
             expected to find: {needle}",
            t.slug
        );
    }
}

#[test]
fn rendered_html_contains_every_lede() {
    // Each lede should appear inside `<p class="page-lede">…</p>`.
    for t in TAB_METADATA {
        let needle = format!(r#"<p class="page-lede">{}</p>"#, t.lede);
        assert!(
            INDEX_HTML.contains(&needle),
            "rendered INDEX_HTML missing per-tab lede for slug {:?}; \
             expected to find: {needle}",
            t.slug
        );
    }
}

#[test]
fn rendered_html_contains_every_tooltip_from_sot() {
    // The nav is rendered from TAB_METADATA via tab_nav_html(), so every
    // tooltip in the SoT must appear verbatim in the rendered nav as
    // `data-tab="{slug}" title="{tooltip}"`. This is the test that
    // would have caught the historical drift where the visible logs
    // tooltip said "OODA cycle reports" while the SoT said "cycle reports".
    for t in TAB_METADATA {
        let needle = format!(r#"data-tab="{}" title="{}""#, t.slug, t.tooltip);
        assert!(
            INDEX_HTML.contains(&needle),
            "rendered INDEX_HTML missing nav tooltip for slug {:?}; \
             expected to find: {needle}",
            t.slug
        );
    }
}

#[test]
fn tab_nav_html_marks_first_tab_active_and_rest_inactive() {
    let nav = tab_nav_html();
    // The first tab carries `class="tab active"` so the initial render
    // highlights it without any client-side bootstrap.
    let first = TAB_METADATA[0];
    let active_needle = format!(r#"<div class="tab active" data-tab="{}""#, first.slug);
    assert!(
        nav.contains(&active_needle),
        "first tab {:?} should be rendered with class=\"tab active\"; nav: {nav}",
        first.slug
    );
    // No other tab may carry `tab active`.
    let active_count = nav.matches(r#"class="tab active""#).count();
    assert_eq!(
        active_count, 1,
        "exactly one tab should be rendered as active, found {active_count}; nav: {nav}"
    );
    // Every non-first tab is plain `class="tab"`.
    for t in &TAB_METADATA[1..] {
        let needle = format!(r#"<div class="tab" data-tab="{}""#, t.slug);
        assert!(
            nav.contains(&needle),
            "non-first tab {:?} should render with class=\"tab\" (no active); nav: {nav}",
            t.slug
        );
    }
}

#[test]
fn rendered_html_default_title_matches_sot() {
    // The hardcoded `<title>` in part_00.rs is gone; the initial title
    // comes from default_title() via the {{DEFAULT_TITLE}} marker.
    let needle = format!("<title>{}</title>", default_title());
    assert!(
        INDEX_HTML.contains(&needle),
        "rendered INDEX_HTML should contain <title>{}</title> for the \
         default-active tab; this is substituted from default_title() at \
         render time",
        default_title()
    );
}

#[test]
fn rendered_html_contains_tab_meta_js_block() {
    assert!(
        INDEX_HTML.contains("window.__TAB_META="),
        "INDEX_HTML missing the __TAB_META JS block"
    );
}

#[test]
fn rendered_html_has_no_unresolved_template_markers() {
    // Belt-and-braces: after substitution the rendered HTML should not
    // contain any `{{` markers (an unsubstituted marker would surface as
    // raw text on the page).
    assert!(
        !INDEX_HTML.contains("{{"),
        "rendered INDEX_HTML still contains a template marker — \
         look for the leftover `{{` in the source parts"
    );
}

#[test]
fn rendered_html_demotes_brand_h1_to_div() {
    // The header brand used to be `<h1>🌲 Simard Dashboard</h1>`, which
    // collided with the per-tab `<h1>` requirement. It now lives in a
    // `<div class="brand">` so each active panel owns the only `<h1>`.
    assert!(
        INDEX_HTML.contains(r#"<div class="brand">"#),
        "header brand must be a <div class=\"brand\">"
    );
    assert!(
        !INDEX_HTML.contains("<h1>🌲 Simard Dashboard</h1>"),
        "header must not still render an <h1> for the brand text"
    );
}

#[test]
fn rendered_html_workboard_label_replaces_whiteboard() {
    // #1995: the visible label must match the slug. There should be no
    // remaining "Whiteboard" in user-facing nav text.
    let nav_slice = {
        let start = INDEX_HTML
            .find(r#"data-tab="workboard""#)
            .expect("workboard nav entry should be present");
        // Take a small window around the nav entry.
        let end = INDEX_HTML[start..]
            .find("</div>")
            .map(|e| start + e)
            .unwrap_or(INDEX_HTML.len());
        &INDEX_HTML[start..end]
    };
    assert!(
        nav_slice.contains("Workboard"),
        "workboard nav entry should render the label `Workboard`; got: {nav_slice}"
    );
    assert!(
        !nav_slice.contains("Whiteboard"),
        "workboard nav entry must not still say `Whiteboard`: {nav_slice}"
    );
}

#[test]
fn rendered_html_tab_click_handler_swaps_document_title() {
    // The client-side tab handler must update document.title from the
    // __TAB_META map so each tab's browser-tab text matches its
    // `TAB_METADATA.title`.
    assert!(
        INDEX_HTML.contains("document.title"),
        "tab-click handler must set document.title"
    );
    assert!(
        INDEX_HTML.contains("__TAB_META"),
        "tab-click handler must read window.__TAB_META"
    );
}

// ----- Issue #2358: jargon humanizers for rendered (non-lede) text -----

#[test]
fn rendered_html_defines_jargon_humanizers() {
    for helper in [
        "function humanizeGoalId(",
        "function humanizePeriod(",
        "function humanizeCycleSummary(",
    ] {
        assert!(
            INDEX_HTML.contains(helper),
            "dashboard must define JS helper `{helper}` (issue #2358)"
        );
    }
}

#[test]
fn cycle_summary_render_sites_are_humanized() {
    // The cycle-report `summary` field is persisted by the daemon in the
    // technical "OODA cycle #N: … goals=2, issues=20, tree=clean" form. Every
    // human-facing render site must route it through humanizeCycleSummary so
    // the acronym and key=value shorthand never leak (issue #2358).
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(rpt.summary)"),
        "Thinking-tab legacy summary must be humanized"
    );
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(rpt.summary||'')"),
        "Thinking-tab inline summary must be humanized"
    );
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(c.summary"),
        "Logs/history cycle summary must be humanized"
    );
    // No render site should pipe a raw cycle summary straight into esc().
    assert!(
        !INDEX_HTML.contains("esc(rpt.summary)"),
        "a cycle summary is rendered without humanizeCycleSummary"
    );
}

#[test]
fn cycle_summary_humanizer_neutralizes_banned_jargon_and_shorthand() {
    // Locate the humanizeCycleSummary body and assert it targets the acronym
    // and the key=value shorthand the issue flagged. This extends the
    // BANNED_JARGON guarantee from ledes to rendered cycle/summary text.
    let start = INDEX_HTML
        .find("function humanizeCycleSummary(")
        .expect("humanizeCycleSummary must be defined");
    let body = &INDEX_HTML[start..start + 700.min(INDEX_HTML.len() - start)];

    // The OODA acronym (a BANNED_JARGON term) must be a replace target.
    assert!(
        BANNED_JARGON.contains(&"OODA"),
        "OODA should remain in BANNED_JARGON"
    );
    assert!(
        body.contains("OODA"),
        "humanizeCycleSummary must rewrite the OODA acronym: {body}"
    );
    for shorthand in ["tree=clean", "tree=dirty", "goals=", "issues="] {
        assert!(
            body.contains(shorthand),
            "humanizeCycleSummary must rewrite `{shorthand}` shorthand: {body}"
        );
    }
}

#[test]
fn synthetic_goal_ids_are_humanized_at_render_sites() {
    // The `__memory__` family of synthetic goal ids must be mapped to labels
    // wherever a priority is shown to a human (issue #2358).
    assert!(
        INDEX_HTML.contains("humanizeGoalId(top.goal_id)"),
        "overview top-priority must humanize the goal id"
    );
    assert!(
        INDEX_HTML.contains("humanizeGoalId(p.goal_id)"),
        "Thinking-tab priorities must humanize the goal id"
    );
    assert!(
        INDEX_HTML.contains("'__memory__':"),
        "humanizeGoalId must map the __memory__ sentinel"
    );
}

#[test]
fn cost_period_is_humanized_and_lede_is_honest() {
    // The cost data is estimated (cost_tracking.rs), so the lede must not
    // claim it comes from real provider invoices, and the raw period key must
    // be humanized (issue #2358).
    let costs_lede = TAB_METADATA
        .iter()
        .find(|t| t.slug == "costs")
        .expect("costs tab present")
        .lede;
    assert!(
        !costs_lede.contains("real provider invoices"),
        "costs lede must not claim invoice-derived figures: {costs_lede}"
    );
    assert!(
        costs_lede.contains("estimated"),
        "costs lede should describe the figures as estimated: {costs_lede}"
    );
    assert!(
        INDEX_HTML.contains("humanizePeriod(v)"),
        "cost summary must humanize the raw period key"
    );
}
