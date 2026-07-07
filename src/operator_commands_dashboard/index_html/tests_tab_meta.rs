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
    assert_eq!(TAB_METADATA.len(), 11, "expected 11 tabs");
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
    // #1995 + #2627 consolidation: the former standalone `workboard` tab is
    // now the **Work Board** sub-section of the **Goals** tab, so it must no
    // longer appear as a top-level nav button — but the label lineage
    // (`Whiteboard → Workboard → Work Board`) must have shed "Whiteboard"
    // entirely from user-facing text.
    assert!(
        !INDEX_HTML.contains(r#"data-tab="workboard""#),
        "workboard must no longer be a top-level nav tab after consolidation; \
         it lives as the `Work Board` sub-section of the Goals tab"
    );
    assert!(
        INDEX_HTML.contains(r#"<h2 class="subsection">Work Board</h2>"#),
        "the retired workboard view must render as a `Work Board` sub-section \
         header inside the Goals tab"
    );
    assert!(
        !INDEX_HTML.contains("Whiteboard"),
        "no user-facing text may still say `Whiteboard` (label lineage ends at \
         `Work Board`)"
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

// ─────────────────────────── Feedback widget (#2629) ────────────────────────
//
// The "Report bug / Request feature" widget must be a SINGLE control anchored
// in the shared <header> so it appears on every dashboard tab with consistent
// placement, and its client JS must POST the report + captured page context to
// the auth-gated `/api/feedback` endpoint, rendering results safely.

#[test]
fn feedback_widget_button_lives_in_shared_header() {
    let html = INDEX_HTML.as_str();
    let header_start = html
        .find("<header>")
        .expect("dashboard must have a <header>");
    let header_end = html
        .find("</header>")
        .expect("dashboard must close its <header>");
    let button = html
        .find("id=\"feedback-widget-button\"")
        .expect("a #feedback-widget-button control must exist");

    assert!(
        header_start < button && button < header_end,
        "the feedback button must live inside <header> so it renders on every tab"
    );
    assert_eq!(
        html.matches("id=\"feedback-widget-button\"").count(),
        1,
        "there must be exactly ONE shared feedback widget, not one per tab"
    );
}

#[test]
fn feedback_widget_control_is_labeled_for_bug_and_feature() {
    let html = INDEX_HTML.as_str();
    assert!(
        html.contains("Report bug") && html.contains("Request feature"),
        "the widget must offer both 'Report bug' and 'Request feature'"
    );
}

#[test]
fn feedback_widget_modal_and_form_fields_present() {
    let html = INDEX_HTML.as_str();
    for hook in [
        "id=\"feedback-modal\"",
        "id=\"feedback-form\"",
        "id=\"feedback-type\"",
        "id=\"feedback-title\"",
        "id=\"feedback-description\"",
    ] {
        assert!(html.contains(hook), "feedback widget markup missing {hook}");
    }
    // The type selector must offer exactly bug|feature.
    assert!(
        html.contains("value=\"bug\"") && html.contains("value=\"feature\""),
        "feedback type selector must offer value=\"bug\" and value=\"feature\""
    );
}

#[test]
fn feedback_widget_posts_report_and_context_to_authed_endpoint() {
    let html = INDEX_HTML.as_str();
    assert!(
        html.contains("/api/feedback"),
        "widget JS must POST to /api/feedback"
    );
    assert!(
        html.contains("/api/feedback/status/"),
        "widget JS must poll /api/feedback/status/<id> for the workstream result"
    );
    // Cookie-based auth: the fetch must send the session cookie.
    assert!(
        html.contains("same-origin"),
        "widget fetch must use credentials:'same-origin' so the auth cookie is sent"
    );
    // The captured page context fields must be gathered client-side.
    for key in ["page", "state", "timestamp", "identifiers"] {
        assert!(
            html.contains(key),
            "widget JS must capture the '{key}' context field"
        );
    }
}
// ----- #2627: tab consolidation (17 → 9 canonical tabs) -----
//
// The dashboard nav is consolidated from 17 adjacent/overlapping tabs to a
// single coherent 9-tab taxonomy. Views that answer the same operator
// question are grouped into labelled **sub-sections** (rendered as `<h2>`,
// never a second `page-h1`) inside one parent tab, and every retired
// top-level slug keeps working as a deep-link alias. The durable definition
// of this taxonomy lives in `docs/dashboard.md#canonical-tab-taxonomy`; these
// tests pin the source-of-truth table and the rendered HTML to it.

/// The canonical dashboard tabs, in nav-render order, paired with the
/// label each must expose. This is the single in-test statement of the
/// consolidated taxonomy that `docs/dashboard.md` documents. The trailing
/// `memory` entry is the #2627 regression fix (its viz was dropped by the
/// 17->9 consolidation and is restored as a dedicated tab).
const CANONICAL_TABS: &[(&str, &str)] = &[
    ("overview", "Overview"),
    ("goals", "Goals"),
    ("activity", "Activity"),
    ("workers", "Workers"),
    ("pull-requests", "Pull Requests"),
    // #2627 regression fix: the memory-graph visualization dropped during the
    // 17->9 consolidation is restored as its OWN dedicated top-level tab
    // (previously folded into Resources as a sub-section / deep-link alias),
    // sitting alongside Resources.
    ("memory", "Memory"),
    ("resources", "Resources"),
    ("chat", "Chat"),
    ("overseer", "Overseer"),
    ("journal", "Journal"),
    ("creative-ideas", "Creative Ideas"),
];

/// Slugs that were real top-level tabs in the 17-tab set and must NOT survive
/// as nav tabs after consolidation — each is now a sub-section reachable via
/// its deep-link alias.
///
/// `memory` is deliberately absent: issue #2627 promotes it back to a
/// first-class top-level tab, so it now lives in [`CANONICAL_TABS`] instead.
const RETIRED_SLUGS: &[&str] = &[
    "traces",
    "logs",
    "processes",
    "costs",
    "workboard",
    "thinking",
    "brain-failures",
    "merge-decisions",
    "pr-readiness",
    "terminal",
    "status",
];

#[test]
fn tab_meta_matches_canonical_taxonomy() {
    // Exact slug + label + order. Anchors the whole consolidation: if a tab is
    // added, removed, renamed, or reordered, this fails until the taxonomy and
    // docs/dashboard.md agree again.
    assert_eq!(
        TAB_METADATA.len(),
        CANONICAL_TABS.len(),
        "expected exactly {} canonical tabs, found {}",
        CANONICAL_TABS.len(),
        TAB_METADATA.len()
    );
    for (i, (slug, label)) in CANONICAL_TABS.iter().enumerate() {
        assert_eq!(
            TAB_METADATA[i].slug, *slug,
            "tab #{i} slug should be {slug:?}, got {:?}",
            TAB_METADATA[i].slug
        );
        assert_eq!(
            TAB_METADATA[i].label, *label,
            "tab #{i} ({slug}) label should be {label:?}, got {:?}",
            TAB_METADATA[i].label
        );
    }
}

#[test]
fn tab_meta_has_no_retired_top_level_slugs() {
    let live: HashSet<&str> = TAB_METADATA.iter().map(|t| t.slug).collect();
    for retired in RETIRED_SLUGS {
        assert!(
            !live.contains(retired),
            "retired slug {retired:?} must not remain a top-level tab; it should \
             be a sub-section reachable via its deep-link alias"
        );
    }
}

#[test]
fn tab_meta_uses_no_bridge_names() {
    // Binding constraint: no consolidated tab may be named "Bridge".
    for t in TAB_METADATA {
        assert!(
            !t.label.contains("Bridge") && !t.h1.contains("Bridge") && !t.slug.contains("bridge"),
            "tab {:?} must not use a `Bridge` name",
            t.slug
        );
    }
}

#[test]
fn js_canonical_tabs_allowlist_includes_every_tab() {
    // Regression guard: the client-side `activateTab`/`resolveHashTab` allowlist
    // (`const CANONICAL_TABS=[...]` in the rendered JS) MUST list every top-level
    // tab. A slug present in `TAB_METADATA` but missing from the JS allowlist
    // renders a nav button whose panel never becomes visible (activateTab falls
    // back to Overview) — exactly the e2e tab-identity failure this catches at
    // the unit level.
    let start = INDEX_HTML
        .find("const CANONICAL_TABS=[")
        .expect("rendered HTML has the CANONICAL_TABS JS allowlist");
    let tail = &INDEX_HTML[start..];
    let end = tail.find("];").expect("CANONICAL_TABS array is closed");
    let array = &tail[..end];
    for t in TAB_METADATA {
        let needle = format!("'{}'", t.slug);
        assert!(
            array.contains(&needle),
            "JS CANONICAL_TABS allowlist is missing tab slug {:?}; its panel would \
             never activate. Array was: {array}",
            t.slug
        );
    }
}

#[test]
fn rendered_html_has_exactly_eleven_page_h1s() {
    // Invariant 2 across the consolidated set plus Creative Ideas and the
    // restored Memory tab (#2627): each tab panel owns exactly one
    // `<h1 class="page-h1">`, so the rendered HTML has exactly eleven of them.
    // Sub-sections must use `<h2>`, never a second page-h1. (Resources keeps a
    // `<h2 class="subsection">Memory</h2>` recall summary — an h2, not counted.)
    let count = INDEX_HTML.matches(r#"<h1 class="page-h1">"#).count();
    assert_eq!(
        count, 11,
        "expected exactly 11 page-h1 headings (one per tab), found {count}; \
         a stray page-h1 usually means an absorbed panel kept its old <h1> instead of \
         being demoted to an <h2 class=\"subsection\">"
    );
}

#[test]
fn rendered_html_contains_consolidated_sub_section_headers() {
    // Data-preservation contract (invariant 5): every former standalone view
    // survives as a labelled sub-section header inside its parent tab. Sub-
    // section headers render as `<h2 class="subsection">…</h2>` so invariant 2
    // (one page-h1 per tab) still holds.
    let required_subsections = [
        // Overview absorbs the old `overview` + `status` tabs, plus a Stats panel.
        "Summary",
        "Health",
        "Stats",
        // Goals absorbs the old `workboard` tab.
        "Work Board",
        // Activity absorbs logs/traces/thinking/brain-failures.
        "Logs",
        "Traces",
        "Thinking",
        "Failures",
        // Workers absorbs processes/terminal (+ the engineers process-tree view).
        "Processes",
        "Engineers",
        "Terminal",
        // Pull Requests absorbs merge-decisions/pr-readiness.
        "Merge Decisions",
        "Readiness",
        // Resources absorbs memory/costs.
        "Memory",
        "Costs",
    ];
    for name in required_subsections {
        let needle = format!(r#"<h2 class="subsection">{name}</h2>"#);
        assert!(
            INDEX_HTML.contains(&needle),
            "consolidated dashboard is missing the {name:?} sub-section header; \
             expected to find: {needle} — a merged view must survive as a labelled \
             `<h2 class=\"subsection\">` inside its parent tab so no data is lost"
        );
    }
}

#[test]
fn rendered_html_wires_tab_alias_allowlist() {
    // Deep-link continuity: every retired top-level slug resolves to its new
    // parent tab through a client-side allowlist. The resolver treats
    // `location.hash` as untrusted — it validates against `^[a-z-]+$` and falls
    // back to `overview` — and never concatenates the hash into a selector.
    assert!(
        INDEX_HTML.contains("TAB_ALIASES"),
        "rendered HTML must define the client-side TAB_ALIASES deep-link allowlist"
    );
    // Every retired slug maps to its documented parent tab (compact JSON form).
    let alias_pairs = [
        ("status", "overview"),
        ("workboard", "goals"),
        ("logs", "activity"),
        ("traces", "activity"),
        ("thinking", "activity"),
        ("brain-failures", "activity"),
        ("processes", "workers"),
        ("terminal", "workers"),
        ("merge-decisions", "pull-requests"),
        ("pr-readiness", "pull-requests"),
        ("costs", "resources"),
    ];
    for (retired, parent) in alias_pairs {
        let needle = format!(r#""{retired}":"{parent}""#);
        assert!(
            INDEX_HTML.contains(&needle),
            "TAB_ALIASES must map retired slug {retired:?} to parent tab {parent:?}; \
             expected to find: {needle}"
        );
    }
    // #2627: `memory` is now a canonical top-level tab, so its old
    // `"memory":"resources"` deep-link alias MUST be removed. The resolver
    // checks CANONICAL_TABS before TAB_ALIASES, so a stale alias would be dead
    // code that silently misroutes a `#memory` bookmark into Resources.
    assert!(
        !INDEX_HTML.contains(r#""memory":"resources""#),
        "the retired-slug alias \"memory\":\"resources\" must be removed once \
         Memory is a canonical tab (issue #2627); it would shadow the real tab"
    );
    // The untrusted-hash validator must be present so a crafted hash can never
    // reach a DOM selector.
    assert!(
        INDEX_HTML.contains("^[a-z-]+$"),
        "the deep-link resolver must validate location.hash against ^[a-z-]+$ \
         before using it"
    );
    // Sub-sections introduced by consolidation (never standalone tabs) must NOT
    // gain a bogus alias — there are no old bookmarks to preserve.
    assert!(
        !INDEX_HTML.contains(r#""stats":"overview""#),
        "the Stats sub-section was never a standalone tab and must not have an alias"
    );
    assert!(
        !INDEX_HTML.contains(r#""engineers":"workers""#),
        "the Engineers sub-section was never a standalone tab and must not have an alias"
    );
}

// ----- issue #20: Goals tab renders each goal's LIVE lifecycle status -----
//
// BUG: the active-goals Status column dumped the raw free-form `g.status`
// string (`<td>${esc(g.status)}</td>`). Paired with the prominent red activity
// chip in the Current Activity column, this made EVERY goal read as
// "failed/blocked" even though the goals were in mixed states (not-started /
// in-progress / blocked+reason / completed).
//
// FIX (frontend half): the Status cell renders a distinctly-colored lifecycle
// badge driven by the additive serialized-enum `g.status_progress` field via
// the existing `humanizeGoalProgress` (escape-last). A `goalLifecycleKey`
// classifier maps the enum VARIANT (never the free-form reason text — G3,
// agentic-over-brittle) to a canonical key that indexes a hardcoded
// `GOAL_STATUS_COLORS` allowlist. Blocked uses amber (#d29922), DELIBERATELY
// distinct from the activity-Failed red (#f85149), so a lifecycle-blocked goal
// is never mistaken for an activity failure.
//
// This is the Rust half of the contract; the behavioral half is
// tests/gadugi/dashboard-goals-lifecycle.sh.

#[test]
fn rendered_html_goals_status_column_uses_lifecycle_badge() {
    // The Status cell must render the humanized lifecycle status from the
    // additive serialized-enum field.
    assert!(
        INDEX_HTML.contains("humanizeGoalProgress(g.status_progress)"),
        "the active-goals Status column must render humanizeGoalProgress(g.status_progress) \
         so each goal shows its real lifecycle status, not a uniform failed/blocked dump"
    );
    // escape-last invariant: humanize the RAW enum, then esc() the result;
    // never humanize already-escaped text.
    assert!(
        !INDEX_HTML.contains("humanizeGoalProgress(esc("),
        "humanizeGoalProgress must run on the raw g.status_progress enum, never on \
         already-escaped text (escape-last invariant)"
    );
    // The old uniform raw-status dump — the exact cell that made every goal
    // look failed/blocked — must be gone.
    assert!(
        !INDEX_HTML.contains("<td>${esc(g.status)}</td>"),
        "the Status column must no longer dump the raw free-form g.status string \
         uniformly; it must render a per-status lifecycle badge"
    );
    // A genuinely-blocked goal must surface its reason via the humanizer's
    // Blocked-object branch ('Blocked — <reason>').
    assert!(
        INDEX_HTML.contains("'Blocked — '+r"),
        "humanizeGoalProgress must render a blocked goal's REASON ('Blocked — <reason>') \
         so the Status column shows why a goal is blocked, not a bare 'failed'"
    );
}

#[test]
fn rendered_html_goals_status_classifier_and_color_allowlist() {
    // A classifier keys the color allowlist off the enum VARIANT, not by
    // parsing the free-form Display/reason string (G3: agentic-over-brittle).
    assert!(
        INDEX_HTML.contains("function goalLifecycleKey("),
        "a goalLifecycleKey() classifier must map the serialized GoalProgress enum to a \
         canonical lifecycle key by variant, not by parsing the Display string"
    );
    // A hardcoded color allowlist drives the badge color so goal data is never
    // interpolated into a style= attribute.
    assert!(
        INDEX_HTML.contains("GOAL_STATUS_COLORS"),
        "a hardcoded GOAL_STATUS_COLORS allowlist must drive the lifecycle badge color \
         (goal data must never be interpolated into a style= attribute)"
    );
    // Blocked's amber must be present and DISTINCT from the activity-Failed red,
    // so a lifecycle-blocked goal is not mistaken for an activity failure.
    assert!(
        INDEX_HTML.contains("#d29922"),
        "the blocked lifecycle badge must use amber #d29922"
    );
    assert!(
        !INDEX_HTML.contains("blocked:'#f85149'") && !INDEX_HTML.contains("Blocked:'#f85149'"),
        "the blocked lifecycle badge must NOT reuse the activity-Failed red #f85149; \
         it must be a distinct amber so blocked != failed"
    );
}

// ===========================================================================
// Issue #2695 follow-up — Goals tab HIERARCHY + differentiated PRIORITY (front).
//
// The rendered Goals tab must (1) NEST decomposed sub-goals under their parent
// using the structured `parent_goal_id` field (not brittle parsing), (2) render
// each goal's priority VISIBLY with a distinct tier, and (3) ORDER goals by
// priority (highest first). These are the frontend half of the contract; the
// backend `/api/goals` shape is pinned in `tests_goals_crud.rs` and the
// prioritization substance in `goal_curation/tests_prioritize.rs`.
//
// Following the escape-last + hardcoded-color-allowlist invariants already used
// for the lifecycle badge (#20): a `priorityTierKey()` classifier maps the
// NUMERIC priority to a canonical tier key that indexes a hardcoded
// `GOAL_PRIORITY_COLORS` allowlist (goal data is never interpolated into a
// style= attribute), and `humanizePriority()` returns PLAIN TEXT (esc()'d last).
//
// RED until the render adds the tier helpers, the priority sort, and the
// parent-child nesting.
// ===========================================================================

#[test]
fn rendered_html_goals_defines_priority_tier_helpers() {
    for needle in ["function priorityTierKey(", "function humanizePriority("] {
        assert!(
            INDEX_HTML.contains(needle),
            "the Goals tab must define {needle:?} so each goal's priority is shown with a \
             distinct, human-readable tier"
        );
    }
}

#[test]
fn rendered_html_goals_priority_tier_color_allowlist_is_hardcoded() {
    // A hardcoded color allowlist drives the tier stripe/pill so goal-supplied
    // data is never interpolated into a style= attribute.
    assert!(
        INDEX_HTML.contains("GOAL_PRIORITY_COLORS"),
        "a hardcoded GOAL_PRIORITY_COLORS allowlist must drive the priority-tier color \
         (priority data must never be interpolated into a style= attribute)"
    );
    // The tier key must index the color map — classification by the numeric
    // value, not by parsing free-form text.
    assert!(
        INDEX_HTML.contains("GOAL_PRIORITY_COLORS[priorityTierKey("),
        "the priority-tier color must be looked up as GOAL_PRIORITY_COLORS[priorityTierKey(...)] \
         so only allowlisted colors reach the DOM"
    );
}

#[test]
fn rendered_html_goals_priority_is_humanized_escape_last() {
    // The priority cell/pill must render the humanized priority label.
    assert!(
        INDEX_HTML.contains("humanizePriority("),
        "the Goals tab must render humanizePriority(...) so priority is visible and labeled, \
         not a bare number with no tier"
    );
    // escape-last invariant: humanize the RAW priority, then esc() the result;
    // never humanize already-escaped text.
    assert!(
        !INDEX_HTML.contains("humanizePriority(esc("),
        "humanizePriority must run on the raw priority value, never on already-escaped text \
         (escape-last invariant)"
    );
}

#[test]
fn rendered_html_goals_ordered_by_priority() {
    // A named comparator sorts goals by priority ascending (highest first). It
    // is required at BOTH the top level and within a parent's children, so the
    // priority-first ordering holds at every level of the hierarchy.
    assert!(
        INDEX_HTML.contains("function sortGoalsByPriority("),
        "the Goals tab must define sortGoalsByPriority(...) to order goals by priority \
         (highest first) at every level of the tree"
    );
    assert!(
        INDEX_HTML.contains("sortGoalsByPriority("),
        "sortGoalsByPriority(...) must actually be applied to the goals before rendering"
    );
    // Regression guard: the goals list must no longer be rendered in raw
    // server/insertion order via a bare `d.active.map(` with no ordering pass.
    assert!(
        !INDEX_HTML.contains("d.active.map(g=>{"),
        "the Goals tab must not render d.active in raw order; goals must be ordered by \
         priority (and grouped by parent) first"
    );
}

#[test]
fn rendered_html_goals_render_parent_child_hierarchy() {
    // Nesting is driven by the structured parent_goal_id field (G3), never by
    // parsing the description or a graph query at render time.
    assert!(
        INDEX_HTML.contains("parent_goal_id"),
        "the Goals tab render must reference parent_goal_id to nest sub-goals under their \
         parent (structured hierarchy edge, not brittle parsing)"
    );
    // A child must be visually indented/nested under its parent — the render
    // groups children by parent id. A dedicated grouping helper keeps the tree
    // build cycle-safe (visited-set + depth cap) rather than an ad-hoc inline map.
    assert!(
        INDEX_HTML.contains("function groupGoalsByParent("),
        "the Goals tab must define groupGoalsByParent(...) so decomposed children are \
         grouped/nested under their active parent (orphans/backlog-parent children at root)"
    );
}

#[test]
fn rendered_html_goals_render_label_chips_and_tag_filter() {
    // Issue #2743: the Goals tab renders each goal's labels as chips and offers
    // a client-side tag filter over the already-fetched live goal data.
    assert!(
        INDEX_HTML.contains("goalLabelChips(g.labels)"),
        "each goal row must render its labels as chips via goalLabelChips(g.labels)"
    );
    assert!(
        INDEX_HTML.contains("goal-label-chip"),
        "label chips must carry the goal-label-chip class for styling/testing"
    );
    // The tag filter is client-side: a filter control + a predicate over the
    // fetched goals. No new route is added.
    assert!(
        INDEX_HTML.contains("id=\"goals-tag-filter\""),
        "the Goals tab must host a tag-filter control container"
    );
    assert!(
        INDEX_HTML.contains("function goalMatchesTagFilter(")
            && INDEX_HTML.contains("window.setGoalTagFilter"),
        "the Goals tab must filter goals client-side by the selected tag"
    );
    // Filtering runs over the fetched goal data, not a server round-trip, and
    // is applied at the GROUP level (#2743 review Finding #2): the full active
    // list is grouped first, then whole entries are kept when the root or any
    // child matches, so a child-only tag can never orphan children from their
    // parent/umbrella header.
    assert!(
        INDEX_HTML.contains("function entryMatchesTagFilter(")
            && INDEX_HTML.contains(
                "groupGoalsByParent(d.active||[],d.backlog).filter(entryMatchesTagFilter)"
            ),
        "the render must group the full active list and filter at the group level"
    );
    // Review hardening (#2743): a tag is user-influenced text emitted into the
    // `value="…"` attribute of each <option>. It must be attribute-escaped with
    // escAttr() (which also neutralises the `\"` that closes the attribute), not
    // plain esc() — otherwise a tag containing a double-quote could break out of
    // the attribute and inject markup.
    assert!(
        INDEX_HTML.contains("'<option value=\"'+escAttr(t)+'\"'"),
        "the tag-filter <option> value must be attribute-hardened with escAttr(t)"
    );
    assert!(
        !INDEX_HTML.contains("'<option value=\"'+esc(t)+'\"'"),
        "the tag-filter <option> value must not use plain esc(t) in the \
         attribute context (attribute-injection risk)"
    );
}
