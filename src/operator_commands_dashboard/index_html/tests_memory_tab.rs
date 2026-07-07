//! Contract tests for the dedicated **Memory** dashboard tab (issue #2627).
//!
//! REGRESSION: the ~17->9 tab consolidation dropped the memory-graph
//! visualization — it was folded into a collapsed `<details id="mem-advanced-toggle">`
//! inside the Resources tab and demoted to a deep-link alias
//! (`"memory":"resources"`). This restores it as its OWN first-class top-level
//! "Memory" tab, wired to the LIVE cognitive-memory read path
//! (`GET /api/memory/graph`, see `memory.rs`).
//!
//! These assertions are the Rust half of the contract; the behavioural half is
//! `tests/e2e-dashboard/specs/memory-tab.spec.ts`.
//!
//! TDD note (Step 7 / red): every assertion here is expected to **FAIL**
//! against the current build — `memory` is still a RETIRED slug, absent from
//! `TAB_METADATA` and the JS `CANONICAL_TABS` allowlist, and the graph canvas
//! is buried under `mem-advanced-toggle` inside `#tab-resources`. They pass
//! once the Memory tab is registered and the viz is promoted into its panel.

#![cfg(test)]

use super::INDEX_HTML;
use super::tab_meta::TAB_METADATA;

/// The six memory-type literals the client renderer keys on (`mgColors` /
/// `data-type=` filters in the parts). Every restored filter must map to one.
const MEMORY_TYPES: [&str; 6] = [
    "WorkingMemory",
    "SemanticFact",
    "EpisodicMemory",
    "ProceduralMemory",
    "ProspectiveMemory",
    "SensoryBuffer",
];

/// Return the HTML of the `id="tab-<slug>"` panel: everything from that panel's
/// id marker up to the next `id="tab-` (the next sibling panel), or end of
/// document. Panics if the panel is absent so a missing Memory tab fails loudly
/// with a clear message rather than a silent false-negative.
fn tab_panel<'h>(html: &'h str, slug: &str) -> &'h str {
    let open = format!(r#"id="tab-{slug}""#);
    let start = html.find(&open).unwrap_or_else(|| {
        panic!(
            "no tab-content panel with id=\"tab-{slug}\" in the rendered dashboard — \
             the dedicated Memory tab (#2627) is not rendered"
        )
    });
    let rest = &html[start + open.len()..];
    let end = rest.find(r#"id="tab-"#).unwrap_or(rest.len());
    &rest[..end]
}

/// The `memory` tab must be registered in the single-source-of-truth tab table
/// with the human label/H1 "Memory" and no `Bridge` naming (binding constraint).
#[test]
fn memory_tab_registered_in_metadata() {
    let meta = TAB_METADATA.iter().find(|t| t.slug == "memory").expect(
        "TAB_METADATA must register a top-level `memory` tab (issue #2627); \
             it is currently a RETIRED slug folded into Resources",
    );
    assert_eq!(
        meta.label, "Memory",
        "the memory tab's nav label must be \"Memory\", got {:?}",
        meta.label
    );
    assert_eq!(
        meta.h1, "Memory",
        "the memory tab's page-h1 must be \"Memory\", got {:?}",
        meta.h1
    );
    assert!(
        !meta.slug.contains("bridge")
            && !meta.label.contains("Bridge")
            && !meta.h1.contains("Bridge"),
        "the memory tab must not use any `Bridge` identifier (binding constraint)"
    );
}

/// The tab must be reachable: a nav button plus a content panel carrying the
/// standard page-h1 + page-lede identity block like every other tab.
#[test]
fn memory_tab_has_nav_button_and_panel() {
    let html: &str = &INDEX_HTML;
    assert!(
        html.contains(r#"data-tab="memory""#),
        "the rendered nav must include a data-tab=\"memory\" button so the \
         Memory tab is clickable"
    );
    assert!(
        html.contains(r#"id="tab-memory""#),
        "the rendered HTML must include the id=\"tab-memory\" content panel"
    );
    let panel = tab_panel(html, "memory");
    assert!(
        panel.contains(r#"<h1 class="page-h1">Memory</h1>"#),
        "the Memory panel must own a <h1 class=\"page-h1\">Memory</h1> heading"
    );
    assert!(
        panel.contains(r#"class="page-lede""#),
        "the Memory panel must carry a <p class=\"page-lede\"> intro like every \
         other tab (tab-identity contract)"
    );
}

/// The client-side `CANONICAL_TABS` allowlist gates which panels `activateTab`
/// will show; a slug missing here renders a nav button whose panel never
/// activates (falls back to Overview).
#[test]
fn memory_tab_in_js_canonical_allowlist() {
    let html: &str = &INDEX_HTML;
    let start = html
        .find("const CANONICAL_TABS=[")
        .expect("rendered HTML has the CANONICAL_TABS JS allowlist");
    let tail = &html[start..];
    let end = tail.find("];").expect("CANONICAL_TABS array is closed");
    assert!(
        tail[..end].contains("'memory'"),
        "the JS CANONICAL_TABS allowlist must list 'memory' or its panel never \
         activates (activateTab falls back to Overview). Array was: {}",
        &tail[..end]
    );
}

/// The restored visualization itself: the force-directed graph canvas, the six
/// memory-type filters, and the live-data fetch — all INSIDE the dedicated
/// Memory panel (not buried in Resources' `<details>`).
#[test]
fn memory_tab_renders_the_graph_visualization() {
    let html: &str = &INDEX_HTML;
    let panel = tab_panel(html, "memory");
    assert!(
        panel.contains(r#"id="mem-graph-canvas""#),
        "the Memory tab must render the memory-graph <canvas id=\"mem-graph-canvas\">"
    );
    for ty in MEMORY_TYPES {
        let needle = format!(r#"data-type="{ty}""#);
        assert!(
            panel.contains(&needle),
            "the Memory tab must expose the {ty} type filter ({needle}) so all six \
             memory types are toggleable"
        );
    }
    assert!(
        panel.contains("fetchMemoryGraph"),
        "the Memory tab must wire fetchMemoryGraph() to pull the LIVE graph from \
         /api/memory/graph"
    );
}

/// Moving (not copying) the canvas is mandatory: a duplicated DOM id breaks the
/// `getElementById('mem-graph-canvas')` the renderer relies on.
#[test]
fn memory_graph_canvas_is_not_duplicated() {
    let html: &str = &INDEX_HTML;
    let n = html.matches(r#"id="mem-graph-canvas""#).count();
    assert_eq!(
        n, 1,
        "the memory-graph canvas must exist exactly once (MOVED into the Memory \
         tab, not copied) — {n} occurrences means a duplicate DOM id that breaks \
         document.getElementById"
    );
}

/// Background prefetch/refresh parity: the Memory tab must have its own loader
/// entry in the `TAB_LOADERS` registry (the graph loader moves out of the old
/// `resources` entry), so it refreshes on its own like the sibling tabs.
#[test]
fn memory_tab_has_background_loader() {
    let html: &str = &INDEX_HTML;
    let start = html
        .find("const TAB_LOADERS=")
        .expect("rendered HTML has the TAB_LOADERS registry");
    let tail = &html[start..];
    let end = tail.find("};").expect("TAB_LOADERS object is closed");
    let registry = &tail[..end];
    let mstart = registry.find("'memory':[").unwrap_or_else(|| {
        panic!(
            "TAB_LOADERS must register a 'memory':[...] entry so the Memory tab \
             background-prefetches/refreshes like the other tabs (#2627)"
        )
    });
    let mtail = &registry[mstart..];
    let mend = mtail.find(']').unwrap_or(mtail.len());
    assert!(
        mtail[..mend].contains("fetchMemoryGraph"),
        "the 'memory' loader entry must call fetchMemoryGraph for live refresh; \
         got: {}",
        &mtail[..mend]
    );
}

/// Deep-link migration: `#memory` now resolves to the canonical Memory tab, so
/// the dead `"memory":"resources"` alias must be gone (the resolver checks
/// canonical before alias — a stale alias silently misroutes the bookmark).
#[test]
fn memory_deeplink_alias_to_resources_removed() {
    let html: &str = &INDEX_HTML;
    assert!(
        !html.contains(r#""memory":"resources""#),
        "the retired-slug alias \"memory\":\"resources\" must be removed once \
         Memory is a canonical tab (#2627)"
    );
}
