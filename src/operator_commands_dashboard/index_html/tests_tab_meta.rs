//! Tests for [`super::tab_meta`] and the cross-check between
//! `TAB_METADATA` and the rendered `INDEX_HTML`. These tests are the
//! Rust half of the Tab-Identity Contract (#1993 / #1994 / #1995); the
//! other half is `tests/e2e-dashboard/smoke_python/test_tab_clarity.py`.

#![cfg(test)]

use super::INDEX_HTML;
use super::tab_meta::{
    BANNED_JARGON, TAB_METADATA, banned_jargon_js, default_title, tab_meta_js, tab_nav_html,
};
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
    assert_eq!(TAB_METADATA.len(), 15, "expected 15 tabs");
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

// ----- #2358: jargon ban extends to rendered cycle/summary text -----

#[test]
fn banned_jargon_js_is_valid_json_array() {
    let js = banned_jargon_js();
    let parsed: serde_json::Value =
        serde_json::from_str(&js).expect("banned_jargon_js must be valid JSON");
    let arr = parsed
        .as_array()
        .expect("BANNED_JARGON renders as a JS array");
    assert_eq!(arr.len(), BANNED_JARGON.len());
    for banned in BANNED_JARGON {
        assert!(
            arr.iter().any(|v| v.as_str() == Some(*banned)),
            "banned_jargon_js missing term {banned:?}"
        );
    }
}

#[test]
fn rendered_html_injects_banned_jargon_for_summary_humanizer() {
    // The `{{BANNED_JARGON_JS}}` marker must be substituted with the live
    // BANNED_JARGON list so the client-side humanizer strips the same jargon
    // the ledes are forbidden from containing (single source of truth).
    assert!(
        !INDEX_HTML.contains("{{BANNED_JARGON_JS}}"),
        "the BANNED_JARGON marker was not substituted"
    );
    assert!(
        INDEX_HTML.contains("const BANNED_JARGON="),
        "rendered HTML must define the client-side BANNED_JARGON list"
    );
    for banned in BANNED_JARGON {
        assert!(
            INDEX_HTML.contains(banned),
            "rendered HTML missing injected jargon term {banned:?} for the summary humanizer"
        );
    }
}

#[test]
fn rendered_html_humanizes_cycle_summaries() {
    // The humanizer must be defined and applied; raw `esc(rpt.summary)` must
    // no longer reach the user-facing summary slots (that path leaked `OODA`,
    // `goals=2`, `tree=clean`).
    assert!(
        INDEX_HTML.contains("function humanizeCycleSummary("),
        "humanizeCycleSummary helper must be defined"
    );
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(rpt.summary)"),
        "Thinking legacy cycle summary must be humanized"
    );
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(rpt.summary||'')"),
        "Thinking inline cycle summary must be humanized"
    );
    assert!(
        INDEX_HTML.contains("humanizeCycleSummary(c.summary||'')"),
        "OODA cycle-history summary must be humanized"
    );
    assert!(
        !INDEX_HTML.contains("esc(rpt.summary)"),
        "raw esc(rpt.summary) still leaks the machine cycle summary"
    );
}

#[test]
fn rendered_html_drops_false_invoice_cost_claim() {
    // #2358 P1: the Costs lede claimed "real provider invoices rather than
    // estimates" while the metric is labeled "Estimated Cost". The lede must
    // no longer make the invoice claim.
    assert!(
        !INDEX_HTML.contains("real provider invoices rather than estimates"),
        "Costs lede must not claim invoice-derived figures while labeling them Estimated"
    );
}

#[test]
fn rendered_html_defines_token_humanizers() {
    for needle in [
        "function humanizeGoalId(",
        "function humanizePeriod(",
        "humanizeGoalId(top.goal_id)",
    ] {
        assert!(
            INDEX_HTML.contains(needle),
            "rendered HTML missing humanizer wiring: {needle}"
        );
    }
}

#[test]
fn rendered_html_humanizes_p3_units_and_scales() {
    // #2358 P3: durations and bare-float urgency scores must be humanized.
    for needle in [
        "function humanizeDuration(",
        "function urgencyPhrase(",
        "humanizeDuration(intervalSecs)",
        "urgencyPhrase(top.urgency)",
        "urgencyPhrase(p.urgency)",
    ] {
        assert!(
            INDEX_HTML.contains(needle),
            "rendered HTML missing P3 humanizer wiring: {needle}"
        );
    }
    // The bare-minute interval label and bare urgency floats must be gone.
    assert!(
        !INDEX_HTML.contains("intervalMin"),
        "memory growth interval still renders a bare minute count"
    );
    assert!(
        !INDEX_HTML.contains("urgency ${top.urgency.toFixed(2)}"),
        "Overview still renders a bare unexplained urgency float"
    );
    assert!(
        !INDEX_HTML.contains("(urgency: ${p.urgency.toFixed(2)})"),
        "Thinking priorities still render a bare unexplained urgency float"
    );
}

// ----- #2358 P2 item 3: Overview raw brain-action-detail humanization -----
//
// The Overview tab renders two action-detail slots that currently leak raw
// machine strings (e.g. `brain: continue_skipping`, `no-action: no decision
// keyword found...defaulting to...`, `<x>-brain: prefix-routed`). A new
// client-side `humanizeActionDetail(detail)` helper must clean these for
// display while preserving the `agent='engineer-...'` substring the Attach
// button keys off, and the escape-last (XSS) invariant must hold at both
// render sites. These are the Rust half of the contract; the behavioral half
// lives in `tests/gadugi/dashboard-jargon-clarity.sh` and the Playwright
// audit (which exercise the served page and real markup escaping at runtime).

#[test]
fn rendered_html_humanizes_overview_action_detail() {
    // The helper must be defined alongside the other #2358 humanizers.
    assert!(
        INDEX_HTML.contains("function humanizeActionDetail("),
        "humanizeActionDetail helper must be defined so the Overview \
         action-detail slots stop leaking raw brain strings"
    );

    // Site 1 — "Last Cycle Actions" (~part_01.rs L396). The detail must be
    // humanized *then* escaped (escape-last): esc() stays the outermost call.
    assert!(
        INDEX_HTML.contains("esc(humanizeActionDetail(o.detail).substring(0,120))"),
        "Last Cycle Actions must render esc(humanizeActionDetail(o.detail)...) \
         so the raw detail is humanized before the terminal esc()"
    );

    // Site 2 — "Recent actions" IIFE (~part_01.rs L431). The raw string is
    // humanized inside the truncation IIFE before renderActionDetail() escapes
    // it once.
    assert!(
        INDEX_HTML.contains("humanizeActionDetail(a.detail"),
        "Recent actions must humanize a.detail inside the truncation IIFE \
         before handing it to renderActionDetail()"
    );

    // The old raw, un-humanized path must be gone from Site 1.
    assert!(
        !INDEX_HTML.contains("esc(o.detail.substring(0,120))"),
        "Last Cycle Actions still renders the raw, un-humanized detail string"
    );
}

#[test]
fn rendered_html_action_detail_humanizer_preserves_escape_last() {
    // SR-D1 (escape-last invariant, CRITICAL): on every humanized value esc()
    // must remain the *terminal* operation so an attacker-controlled detail
    // such as `<img src=x onerror=alert(1)>` or `<script>` can only ever reach
    // the DOM as escaped entities. We verify the source-level ordering here;
    // runtime escaping is exercised by the Playwright audit.

    // Site 1: esc() wraps the humanizer output (esc-outermost), and the
    // truncation happens on the *raw* humanized text (truncate-before-escape)
    // so an entity is never split.
    assert!(
        INDEX_HTML.contains("esc(humanizeActionDetail(o.detail).substring(0,120))"),
        "Site 1 must keep esc() as the outermost call wrapping the humanizer"
    );

    // The escape-first anti-pattern (humanizing already-escaped text, which
    // would leave the humanizer's own output unescaped) must never appear.
    assert!(
        !INDEX_HTML.contains("humanizeActionDetail(esc("),
        "humanizeActionDetail must never run on already-escaped text \
         (escape-first would let its output reach the DOM unescaped)"
    );

    // Site 2: renderActionDetail() performs the single, internal esc(). Feeding
    // it the raw humanized string keeps esc() terminal there too; double-
    // escaping (renderActionDetail(esc(...))) must not be introduced.
    assert!(
        INDEX_HTML.contains("const safe=esc(detail||'')"),
        "renderActionDetail must retain its single internal esc(detail||'') \
         as the terminal escape for Site 2"
    );
    assert!(
        !INDEX_HTML.contains("renderActionDetail(esc("),
        "Recent actions must not pre-escape before renderActionDetail() \
         (that double-escapes and corrupts the markup)"
    );
}

#[test]
fn rendered_html_action_detail_humanizer_preserves_attach_button_contract() {
    // SR-D5 (Attach-button integrity): humanizing the Recent-actions detail
    // must not break the inline Attach button. Site 2 must still route through
    // renderActionDetail(), which detects `agent='engineer-...'` and swaps in
    // the Attach button when a matching tmux session is cached. The humanizer
    // is applied to the *input* of renderActionDetail (so the agent substring
    // it preserves verbatim still reaches the matcher), never around it.
    assert!(
        INDEX_HTML.contains("function renderActionDetail("),
        "renderActionDetail must remain defined to host the Attach-button logic"
    );
    assert!(
        INDEX_HTML.contains("humanizeActionDetail(a.detail"),
        "Site 2 must humanize a.detail as the input fed into renderActionDetail"
    );
    // The agent matcher renderActionDetail relies on must remain intact.
    assert!(
        INDEX_HTML.contains(r"agent='(engineer-"),
        "renderActionDetail must keep matching agent='engineer-...' so the \
         Attach button contract survives the humanization change"
    );
}

// ----- #2552 finding #4: Workboard "Task Memory" JSON/enum de-jargon -----
//
// The Workboard's Task Memory table surfaces raw semantic-fact contents. Some
// facts are goal-board snapshots serialized as JSON, e.g.
// {"active":[{"id":…,"status":{"InProgress":{"percent":5}}}]}, which leaked the
// raw GoalProgress enum ("InProgress") onto the page. The client-side
// humanizeTaskMemory / humanizeGoalProgress helpers must render such a snapshot
// as plain-English lines while the raw JSON survives as a title= tooltip, and
// non-JSON facts must pass through byte-identically. This is the Rust half of
// the contract; the behavioral half is tests/gadugi/dashboard-workboard-clarity.sh.

#[test]
fn rendered_html_defines_task_memory_humanizers() {
    assert!(
        INDEX_HTML.contains("function humanizeTaskMemory("),
        "humanizeTaskMemory must be defined so the Task Memory table stops \
         rendering raw goal-board JSON blobs"
    );
    assert!(
        INDEX_HTML.contains("function humanizeGoalProgress("),
        "humanizeGoalProgress must be defined to map the raw GoalProgress enum \
         (InProgress/Blocked/…) to plain English"
    );
    // The InProgress struct-variant enum must be turned into a plain phrase.
    assert!(
        INDEX_HTML.contains("'In progress — '+status.InProgress.percent+'%'"),
        "humanizeGoalProgress must render the InProgress percent variant as \
         'In progress — N%' instead of leaking the raw enum name"
    );
}

#[test]
fn rendered_html_task_memory_wires_humanizer_with_raw_tooltip() {
    // The content cell must be humanized before the terminal esc() (escape-last),
    // and the truncation happens on the humanized text.
    assert!(
        INDEX_HTML.contains("const humanizedContent=humanizeTaskMemory(rawContent)"),
        "Task Memory must humanize the fact content via humanizeTaskMemory"
    );
    assert!(
        INDEX_HTML.contains("esc(humanizedContent.substring(0,200))"),
        "Task Memory content must be esc()'d as the terminal op over the \
         humanized text (escape-last, truncate-before-escape)"
    );
    // The raw content survives as an attribute-escaped title= tooltip so power
    // users lose nothing — only when the humanizer actually transformed it.
    assert!(
        INDEX_HTML.contains("' title=\"'+escAttr(rawContent)+'\"'"),
        "the raw fact content must survive as an escAttr()-hardened title= \
         tooltip when humanizeTaskMemory transforms it"
    );
    // The old raw-JSON path (esc of the raw fact content) must be gone.
    assert!(
        !INDEX_HTML.contains("esc((f.content||'').substring(0,200))"),
        "Task Memory must no longer render the raw fact content directly"
    );
    // escape-first anti-pattern must never appear.
    assert!(
        !INDEX_HTML.contains("humanizeTaskMemory(esc("),
        "humanizeTaskMemory must never run on already-escaped text"
    );
}

// ----- #2552 finding #5: Workboard "Recent Actions" brain-enum de-jargon -----
//
// The Workboard "Recent Actions" list rendered the raw daemon result string —
// e.g. `brain: continue_skipping (recipe-engineer-lifecycle-brain: no decision
// keyword found…)`. It must route through the existing, tested
// humanizeActionDetail brain-decision humanizer (the same one the Overview
// uses) before renderActionDetail() escapes it once, and preserve the raw
// result as an escAttr()-hardened title= tooltip.

#[test]
fn rendered_html_workboard_recent_actions_humanized_with_raw_tooltip() {
    assert!(
        INDEX_HTML.contains("renderActionDetail(humanizeActionDetail(a.result))"),
        "Workboard Recent Actions must humanize a.result via humanizeActionDetail \
         before renderActionDetail() escapes it (finding #5)"
    );
    // The old raw path must be gone.
    assert!(
        !INDEX_HTML.contains("<span style=\"flex:1\">${renderActionDetail(a.result)}</span>"),
        "Workboard Recent Actions must no longer render the raw a.result string"
    );
    // Raw result preserved as a hover tooltip, attribute-hardened.
    assert!(
        INDEX_HTML.contains("title=\"${escAttr(a.result||'')}\""),
        "raw a.result must survive as an escAttr()-hardened title= tooltip so \
         power users lose nothing"
    );
    // escape-last invariant: no double-escape / escape-first anti-patterns.
    assert!(
        !INDEX_HTML.contains("renderActionDetail(esc("),
        "Workboard must not pre-escape before renderActionDetail() (double-escape)"
    );
}
