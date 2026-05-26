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
        lede: "A live look at what the Simard daemon is doing right now, plus quick stats on system health, open work items, and any other Simard hosts in your cluster.",
        tooltip: "System health and what the agent is doing right now",
    },
    TabMeta {
        slug: "goals",
        label: "Goals",
        title: "Goals · Simard",
        h1: "Goals",
        lede: "The list of things you have asked Simard to accomplish — active goals are being worked on now and backlog goals are queued for later.",
        tooltip: "Active goals and progress toward each one",
    },
    TabMeta {
        slug: "traces",
        label: "Traces",
        title: "Traces · Simard",
        h1: "Traces",
        lede: "Step-by-step OpenTelemetry traces of how the daemon made each recent decision, useful for debugging slow or surprising agent behaviour.",
        tooltip: "Step-by-step OpenTelemetry traces of agent decisions",
    },
    TabMeta {
        slug: "logs",
        label: "Logs",
        title: "Logs · Simard",
        h1: "Logs",
        lede: "Raw daemon logs, the cost ledger, and per-cycle reports — the lowest-level view for debugging what the daemon did and why.",
        tooltip: "Raw daemon logs and recent cycle reports for debugging",
    },
    TabMeta {
        slug: "processes",
        label: "Processes",
        title: "Processes · Simard",
        h1: "Processes",
        lede: "Background OS processes and tmux sessions Simard is running on this host, with a tree view for spotting stuck or zombie workers.",
        tooltip: "Background processes and tmux sessions running on this host",
    },
    TabMeta {
        slug: "memory",
        label: "Memory",
        title: "Memory · Simard",
        h1: "Memory",
        lede: "Everything Simard has learned and remembered \u{2014} what it's thinking about, facts learned, events remembered, known procedures, planned actions, and recent observations \u{2014} with full-text search.",
        tooltip: "What the agent has learned and remembered, across all memory types",
    },
    TabMeta {
        slug: "costs",
        label: "Costs",
        title: "Costs · Simard",
        h1: "Costs",
        lede: "Token and dollar spending by model and provider, plus your daily and weekly budget caps, computed from real provider invoices rather than estimates.",
        tooltip: "Token and dollar usage by model, plus daily and weekly budget",
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
        slug: "workboard",
        label: "Workboard",
        title: "Workboard · Simard",
        h1: "Workboard",
        lede: "A kanban-style view of Simard's current work — queued, in-progress, blocked, and done items alongside the agent's working memory and recent actions.",
        tooltip: "Tasks the agent is working on, like a kanban board",
    },
    TabMeta {
        slug: "thinking",
        label: "🧠 Thinking",
        title: "Thinking · Simard",
        h1: "Thinking",
        lede: "A live stream of the daemon's internal reasoning between actions, showing what it considered before deciding what to do next.",
        tooltip: "Live stream of the agent's reasoning between actions",
    },
    TabMeta {
        slug: "brain-failures",
        label: "Brain Failures",
        title: "Brain Failures · Simard",
        h1: "Brain Failures",
        lede: "Every time the daemon's language-model brain returned an unparseable or invalid response and fell back to safe deterministic rules, listed with the failure type, which component triggered it, when it happened, and whether recovery succeeded.",
        tooltip: "When and how the agent's brain failed, and whether it recovered",
    },
    TabMeta {
        slug: "merge-decisions",
        label: "Merge Decisions",
        title: "Merge Decisions · Simard",
        h1: "Merge Decisions",
        lede: "A record of every pull request the merge judge has evaluated — which PRs were approved, rejected, or deferred, along with the reasoning and timestamp for each decision.",
        tooltip: "History of merge-judge verdicts for each evaluated pull request",
    },
    TabMeta {
        slug: "terminal",
        label: "Terminal",
        title: "Terminal · Simard",
        h1: "Terminal",
        lede: "Attach to the live terminal of a running Simard sub-agent and watch its standard output and standard error stream in real time.",
        tooltip: "Attach to the agent's tmux terminal session and watch live stdout",
    },
];

/// Browser title shown on first page load. The client-side tab handler
/// updates this when a different tab is activated. Uses `TAB_METADATA[0]`
/// directly because [`tab_meta_slugs_unique`] asserts the table has
/// exactly 13 entries — an empty table would already fail other tests.
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
