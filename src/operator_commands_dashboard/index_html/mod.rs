mod part_00;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;
pub mod tab_meta;

#[cfg(test)]
mod tests_tab_meta;

use part_00::PART_00;
use part_01::PART_01;
use part_02::PART_02;
use part_03::PART_03;
use part_04::PART_04;
use part_05::PART_05;

/// Concatenated dashboard HTML/JS, assembled from per-segment string consts
/// so that no single Rust source file exceeds the 400 LOC cap (#1266).
///
/// A single template marker `{{TAB_META_JS}}` is substituted from
/// [`tab_meta::tab_meta_js`] so the client-side tab handler can swap
/// `document.title` per tab without duplicating the tab catalogue in JS.
/// All per-tab `<h1>` / `<p class="page-lede">` blocks are inlined
/// directly in the parts so they survive a `grep` audit; the
/// `tests_tab_meta::rendered_html_contains_every_*` cross-check
/// tests guarantee they stay in sync with [`tab_meta::TAB_METADATA`].
pub(crate) fn index_html_string() -> String {
    let raw = format!("{PART_00} {PART_01} {PART_02} {PART_03} {PART_04} {PART_05}");
    let rendered = raw.replace("{{TAB_META_JS}}", &tab_meta::tab_meta_js());
    debug_assert!(
        !rendered.contains("{{"),
        "unresolved template marker remains in dashboard HTML"
    );
    rendered
}

#[cfg(test)]
pub(crate) static INDEX_HTML: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(index_html_string);
