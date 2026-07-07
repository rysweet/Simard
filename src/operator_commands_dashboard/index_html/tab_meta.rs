//! Tab-Identity Single Source of Truth (#1993 / #1994 / #1995).
//!
//! Every user-visible string per dashboard tab — `label`, browser `title`,
//! page `<h1>`, plain-English `lede`, and hover `tooltip` — is declared
//! exactly once here. The HTML template references this table through:
//!
//! * a `{{TAB_META_JS}}` marker in the inline `<script>` block (so the
//!   client-side tab handler can swap `document.title` per tab); and
//! * Rust cross-check tests in [`tests_tab_meta`] that assert every label,
//!   H1, and lede in this table also appears in the rendered HTML.
//!
//! This avoids the historical bug where the visible `Whiteboard` label
//! drifted away from the underlying `workboard` slug, API endpoint, and
//! Playwright spec.
//!
//! See `docs/dashboard.md#tab-identity-contract` for the full design.
//!
//! The `lede` field is consumed by `#[cfg(test)]` cross-check code that
//! confirms every lede appears in the rendered HTML; `#![allow(dead_code)]`
//! silences Rust's "field never used" analysis without weakening the
//! contract.
#![allow(dead_code)]

use std::fmt::Write as _;

/// One row in the dashboard nav, plus the page identity a tab is required
/// to render. **All five user-visible fields live in this struct** so that
/// renaming a tab or rewriting a lede is a one-line edit, not a `git grep`.
#[derive(Debug, Clone, Copy)]
pub struct TabMeta {
    /// URL-safe identifier matching the underlying route /
    /// `data-tab="…"` attribute / API endpoint. Stable; never user-visible.
    pub slug: &'static str,
    /// Nav-button text. Must equal `h1` for visual consistency.
    pub label: &'static str,
    /// Browser `<title>`. Convention: `"{Label} · Simard"`.
    pub title: &'static str,
    /// Page `<h1 class="page-h1">`. Usually the same as `label`.
    pub h1: &'static str,
    /// One-sentence plain-English explanation of what the page is for,
    /// rendered as `<p class="page-lede">` immediately under the H1.
    /// MUST NOT contain any string in [`BANNED_JARGON`].
    pub lede: &'static str,
    /// Substantive hover tooltip on the nav button (rendered as the
    /// browser-native `title=` attribute).
    pub tooltip: &'static str,
}

/// Consultant-speak / acronym jargon that must not appear in any lede.
/// Domain vocabulary that an operator legitimately needs (`episodic`,
/// `procedural`, `facilitator`, …) is *not* on this list — the bar is
/// "no corporate jargon and no insider acronyms", not "no jargon at all".
pub const BANNED_JARGON: &[&str] = &[
    "OODA",
    "Observe-Orient-Decide-Act",
    "spawn_engineer",
    "LadybugDB",
    "cognitive memory",
    "synergize",
    "leverage",
    "ideate",
];

/// The dashboard tab catalogue, in nav-render order.
///
/// Adding a tab is a single-file edit: append a new [`TabMeta`] here and
/// add a matching `<div class="tab-content" id="tab-{slug}">` panel in
/// `part_00.rs` / `part_01.rs` that includes a `<h1 class="page-h1">` and
/// `<p class="page-lede">` whose text matches the entry below.
///
/// The cross-check tests in [`tests_tab_meta`] verify the
/// `TAB_METADATA ↔ HTML` correspondence at build time, so a typo or a
/// forgotten panel header fails CI rather than shipping a tab with no
/// heading or with the wrong label.
pub const TAB_METADATA: &[TabMeta] = &[
    TabMeta {
        slug: "overview",
        label: "Overview",
        title: "Overview · Simard",
        h1: "Overview",
        lede: "A live look at what the Simard daemon is doing right now, plus system health, open work items, aggregate run counters, and any other Simard hosts in your cluster.",
        tooltip: "System health and what the agent is doing right now",
    },
    TabMeta {
        slug: "goals",
        label: "Goals",
        title: "Goals · Simard",
        h1: "Goals",
        lede: "The things you have asked Simard to accomplish — active goals in progress now, the queued backlog, and a work board of the tasks it is moving through.",
        tooltip: "Active goals, the backlog, and the current work board",
    },
    TabMeta {
        slug: "activity",
        label: "Activity",
        title: "Activity · Simard",
        h1: "Activity",
        lede: "Everything Simard has been doing recently in one place — the background service log, step-by-step decision traces, the live thinking stream, and where its brain failed to parse a response.",
        tooltip: "Logs, traces, the thinking stream, and brain failures",
    },
    TabMeta {
        slug: "workers",
        label: "Workers",
        title: "Workers · Simard",
        h1: "Workers",
        lede: "The background processes and engineer subprocesses Simard is running on this host, with a tree view for spotting stuck workers and a live attach to any agent's terminal.",
        tooltip: "Processes, engineer subprocesses, and live terminals",
    },
    TabMeta {
        slug: "pull-requests",
        label: "Pull Requests",
        title: "Pull Requests · Simard",
        h1: "Pull Requests",
        lede: "Every pull request Simard is managing — the merge judge's approve, reject, and defer decisions plus the CI, review, and blocker status that shows what is ready to merge.",
        tooltip: "Merge decisions and per-PR readiness for managed PRs",
    },
    // #2627 regression fix: the memory-graph visualization dropped during the
    // 17->9 tab consolidation (folded into Resources as a collapsed sub-section
    // and a deep-link alias) is restored here as its OWN dedicated top-level tab,
    // sitting alongside Resources (which keeps the memory recall summary).
    TabMeta {
        slug: "memory",
        label: "Memory",
        title: "Memory · Simard",
        h1: "Memory",
        lede: "A living map of what Simard knows — an interactive graph of the facts, events, procedures, and plans it holds, colour-coded by memory type and drawn live from what it currently remembers.",
        tooltip: "Interactive live graph of what Simard remembers, by memory type",
    },
    TabMeta {
        slug: "resources",
        label: "Resources",
        title: "Resources · Simard",
        h1: "Resources",
        lede: "What Simard has learned and remembered alongside what it costs to run — searchable memory of facts, events, and plans, plus token and dollar spending by model and provider.",
        tooltip: "What the agent remembers, plus token and dollar costs",
    },
    TabMeta {
        slug: "chat",
        label: "Chat",
        title: "Chat · Simard",
        h1: "Chat",
        lede: "Talk to the running Simard agent in real time — anything you say here can become a new goal, and slash-commands like /close, /goals, and /status are available.",
        tooltip: "Talk to the running agent (uses the meeting protocol)",
    },
    TabMeta {
        slug: "overseer",
        label: "Overseer",
        title: "Overseer · Simard",
        h1: "Overseer",
        lede: "What Simard's steward has been doing on its own — what it noticed across the system, what it changed, and, when it chose to wait, why it held back. Refreshes automatically.",
        tooltip: "What the steward has done on its own, and why it sometimes waits",
    },
    TabMeta {
        slug: "journal",
        label: "Journal",
        title: "Journal · Simard",
        h1: "Journal",
        lede: "A plain-language daily diary of what Simard and its steward the Overseer did each day, with a simple table of the code changes proposed. Browse by date and search the full history.",
        tooltip: "A plain-language daily diary of what Simard did, browseable by date",
    },
    TabMeta {
        slug: "creative-ideas",
        label: "Creative Ideas",
        title: "Creative Ideas · Simard",
        h1: "Creative Ideas",
        lede: "A pool of candidate improvements Simard dreams up for herself, each reviewed for feasibility, worth, and how to measure success. Browse and search by their review status, from brand-new to accepted or parked.",
        tooltip: "Simard's pool of self-improvement ideas, searchable by review status",
    },
];

/// Browser title shown on first page load. The client-side tab handler
/// updates this when a different tab is activated. Uses `TAB_METADATA[0]`
/// directly because [`tab_meta_slugs_unique`] asserts the table has
/// exactly eleven entries — an empty table would already fail other tests.
pub fn default_title() -> &'static str {
    TAB_METADATA[0].title
}

/// Render the `{{TAB_NAV}}` block: the full `<div class="tabs">…</div>`
/// nav bar with one `<div class="tab">` per [`TAB_METADATA`] entry.
/// The first entry receives `class="tab active"` so the initial render
/// highlights the default-active tab without any client-side bootstrap.
///
/// This is the **only** place tab labels, tooltips and slugs flow into
/// the rendered HTML, so a future edit to a tooltip is a one-line change
/// in [`TAB_METADATA`] rather than two-places-to-keep-in-sync.
pub fn tab_nav_html() -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(r#"<div class="tabs">"#);
    for (i, t) in TAB_METADATA.iter().enumerate() {
        let class = if i == 0 { "tab active" } else { "tab" };
        let _ = write!(
            out,
            r#"<div class="{class}" data-tab="{slug}" title="{tooltip}">{label}</div>"#,
            slug = t.slug,
            tooltip = t.tooltip,
            label = t.label,
        );
    }
    out.push_str("</div>");
    out
}

/// Render the `{{TAB_META_JS}}` block: an inline `<script>` that exports
/// `window.__TAB_META = { slug: {title, h1, label}, … }` for the
/// client-side tab handler to consume. The map is serialised with
/// `serde_json::to_string` so that any future change introducing a value
/// containing `</script>` or `\u2028` / `\u2029` does not break the inline
/// script.
pub fn tab_meta_js() -> String {
    use serde_json::json;
    let mut map = serde_json::Map::new();
    for t in TAB_METADATA {
        map.insert(
            t.slug.to_string(),
            json!({ "title": t.title, "h1": t.h1, "label": t.label }),
        );
    }
    let mut payload = serde_json::to_string(&map).expect("TAB_METADATA JSON-safe");
    // `serde_json::to_string` does not escape `<`/`>`, so `</script>` inside
    // a string value would terminate the inline script early. Belt-and-
    // braces escape the `<` so a future lede containing `</script>` (or
    // any tag-close sequence) is rendered as literal text inside the JS
    // string instead of breaking the HTML parser.
    payload = payload.replace('<', "\\u003c");
    let mut out = String::with_capacity(payload.len() + 64);
    out.push_str("<script>window.__TAB_META=");
    out.push_str(&payload);
    out.push_str(";</script>");
    out
}

/// Render the `{{BANNED_JARGON_JS}}` marker: a JSON array literal of
/// [`BANNED_JARGON`] so the client-side `humanizeCycleSummary` strips the
/// very same jargon the ledes are forbidden from containing. This keeps a
/// single source of truth — the jargon ban now extends from the static
/// ledes to the dynamically rendered cycle/summary text (#2358).
pub fn banned_jargon_js() -> String {
    // Mirror `tab_meta_js`'s `<` escaping so a future maintainer adding a term
    // containing `</script>` cannot break out of the inline script (the terms
    // are static constants today, so this is defense-in-depth).
    serde_json::to_string(BANNED_JARGON)
        .expect("BANNED_JARGON is JSON-safe")
        .replace('<', "\\u003c")
}
