mod feedback_widget;
mod part_00;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
pub mod tab_meta;

#[cfg(test)]
mod tests_tab_meta;

#[cfg(test)]
mod tests_tab_prefetch;

#[cfg(test)]
mod tests_memory_tab;

use part_00::PART_00;
use part_01::PART_01;
use part_02::PART_02;
use part_03::PART_03;
use part_04::PART_04;
use part_05::PART_05;

/// Concatenated dashboard HTML/JS, assembled from per-segment string consts
/// so that no single Rust source file exceeds the 400 LOC cap (#1266).
///
/// Template markers are substituted from [`tab_meta`]:
///
/// * `{{TAB_NAV}}` — the full `<div class="tabs">…</div>` nav bar, so the
///   one-label-per-route invariant flows from [`tab_meta::TAB_METADATA`].
/// * `{{TAB_META_JS}}` — inline `<script>` exporting `window.__TAB_META`
///   so the client-side tab handler can swap `document.title` per tab.
/// * `{{BANNED_JARGON_JS}}` — a JS array literal of [`tab_meta::BANNED_JARGON`]
///   so the client-side `humanizeCycleSummary` strips the same jargon the
///   ledes are forbidden from containing (#2358).
/// * `{{DEFAULT_TITLE}}` — the `<title>` for the initial render, matching
///   the default-active tab.
/// * `{{FEEDBACK_WIDGET_BUTTON}}` — the shared "Report bug / Request feature"
///   control, anchored in the `<header>` so it renders on every tab (#2629).
///   The matching modal/styles/script ([`feedback_widget::FEEDBACK_WIDGET_BODY`])
///   are injected just before `</body>`.
///
/// All per-tab `<h1>` / `<p class="page-lede">` blocks are inlined
/// directly in the parts so they survive a `grep` audit; the
/// `tests_tab_meta::rendered_html_contains_every_*` cross-check
/// tests guarantee they stay in sync with [`tab_meta::TAB_METADATA`].
pub(crate) fn index_html_string() -> String {
    let raw = format!("{PART_00} {PART_01} {PART_02} {PART_03} {PART_04} {PART_05}");
    let rendered = raw
        .replace("{{TAB_NAV}}", &tab_meta::tab_nav_html())
        .replace("{{TAB_META_JS}}", &tab_meta::tab_meta_js())
        .replace("{{BANNED_JARGON_JS}}", &tab_meta::banned_jargon_js())
        .replace(
            "{{FEEDBACK_WIDGET_BUTTON}}",
            feedback_widget::FEEDBACK_WIDGET_BUTTON,
        )
        .replace("{{DEFAULT_TITLE}}", tab_meta::default_title())
        .replace(
            "</body>",
            &format!("{}\n</body>", feedback_widget::FEEDBACK_WIDGET_BODY),
        );
    debug_assert!(
        !rendered.contains("{{"),
        "unresolved template marker remains in dashboard HTML"
    );
    rendered
}

#[cfg(test)]
pub(crate) static INDEX_HTML: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(index_html_string);
