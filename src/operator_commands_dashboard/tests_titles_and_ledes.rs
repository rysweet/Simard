//! Unit tests for issues #1993 (per-route H1 + document title) and #1994
//! (one-sentence plain-English lede above the data).
//!
//! These tests inspect the rendered dashboard HTML (the concatenated
//! `index_html_string()`) and assert that every tab pane:
//!
//! * declares a `<h1 class="page-title" data-page-title="<slug>">` whose
//!   text is non-empty and unique across the dashboard;
//! * carries a `<div class="page-intro">` lede that is non-empty;
//! * is reachable via a JS hash-routing handler so a deep-link like
//!   `/#/memory` activates the right tab and sets `document.title`.
//!
//! The integration-level cross-route uniqueness check is enforced again
//! against a live server by `scripts/dashboard_audit/test_titles_and_ledes.py`
//! (Playwright, requires `~/.simard/.dashkey`).

#[cfg(test)]
mod tests {
    use crate::operator_commands_dashboard::index_html::INDEX_HTML;

    /// Every visible tab id we expect a per-page H1 + lede for.
    /// Keep this list in sync with the `.tab[data-tab=...]` buttons in
    /// `index_html/part_00.rs`.
    const TAB_IDS: &[&str] = &[
        "overview",
        "goals",
        "traces",
        "logs",
        "processes",
        "memory",
        "costs",
        "chat",
        "workboard",
        "thinking",
        "terminal",
    ];

    fn page_title_for(slug: &str) -> Option<String> {
        // The marker is: <h1 class="page-title" data-page-title="<slug>">TEXT</h1>
        let needle = format!(r#"data-page-title="{slug}""#);
        let idx = INDEX_HTML.find(&needle)?;
        let after = &INDEX_HTML[idx..];
        let gt = after.find('>')? + 1;
        let close = after[gt..].find("</h1>")?;
        Some(after[gt..gt + close].trim().to_string())
    }

    fn page_intro_after(slug: &str) -> Option<String> {
        // Look inside the corresponding tab-content for the first
        // `<div class="page-intro">…</div>`.
        let pane = format!(r#"id="tab-{slug}""#);
        let idx = INDEX_HTML.find(&pane)?;
        let from = &INDEX_HTML[idx..];
        let intro_start = from.find(r#"class="page-intro""#)?;
        let from = &from[intro_start..];
        let gt = from.find('>')? + 1;
        let close = from[gt..].find("</div>")?;
        Some(from[gt..gt + close].trim().to_string())
    }

    #[test]
    fn every_tab_has_a_non_empty_page_title_h1() {
        for slug in TAB_IDS {
            let title = page_title_for(slug)
                .unwrap_or_else(|| panic!("no <h1 data-page-title=\"{slug}\"> in dashboard HTML"));
            assert!(
                !title.is_empty(),
                "tab {slug} has an empty <h1 class=\"page-title\">"
            );
            // Plain-English smell test: avoid leftover brand text as the page name.
            let lower = title.to_lowercase();
            assert!(
                !lower.contains("simard dashboard"),
                "tab {slug} H1 ({title:?}) duplicates the brand mark"
            );
        }
    }

    #[test]
    fn every_tab_page_title_is_unique() {
        use std::collections::BTreeMap;
        let mut by_title: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for slug in TAB_IDS {
            let title = page_title_for(slug).expect("page title");
            by_title.entry(title).or_default().push(slug);
        }
        let dupes: Vec<_> = by_title.iter().filter(|(_, v)| v.len() > 1).collect();
        assert!(
            dupes.is_empty(),
            "duplicate H1 page titles across tabs: {dupes:?}"
        );
    }

    #[test]
    fn every_tab_has_a_non_empty_page_intro_lede() {
        for slug in TAB_IDS {
            let intro = page_intro_after(slug)
                .unwrap_or_else(|| panic!("no .page-intro inside tab-{slug}"));
            // strip HTML tags for the length check
            let visible = strip_tags(&intro);
            assert!(
                visible.trim().len() >= 30,
                "tab {slug} lede is too short to be a sentence ({visible:?})"
            );
            // A one-sentence lede should end in a period (or close).
            let v = visible.trim();
            assert!(
                v.ends_with('.') || v.ends_with('?') || v.ends_with('!'),
                "tab {slug} lede {v:?} does not end with sentence punctuation"
            );
        }
    }

    #[test]
    fn dashboard_wires_hash_routing_and_per_page_title() {
        // The JS must read window.location.hash on load and on hashchange
        // and set document.title for the active tab; #1993 acceptance test.
        let html: &str = &INDEX_HTML;
        assert!(
            html.contains("addEventListener('hashchange'"),
            "dashboard must wire a hashchange listener so /#/<slug> activates the tab"
        );
        assert!(
            html.contains("document.title"),
            "dashboard must update document.title when the active tab changes"
        );
        assert!(
            html.contains("data-page-title"),
            "dashboard must mark the per-page H1 with data-page-title for the route map"
        );
        // Default <title> is no longer the generic 'Simard Dashboard v2'.
        assert!(
            !html.contains("<title>Simard Dashboard v2</title>"),
            "default <title> should be page-specific (issue #1993)"
        );
    }

    #[test]
    fn whiteboard_route_alias_is_present() {
        // Issue #1995 (Whiteboard disambiguation) follow-up: both /#/workboard
        // and /#/whiteboard must resolve to the same pane so deep links from
        // either nav label keep working.
        assert!(
            INDEX_HTML.contains("'whiteboard':'workboard'")
                || INDEX_HTML.contains(r#""whiteboard":"workboard""#),
            "JS TAB_ALIASES map should alias whiteboard -> workboard"
        );
    }

    fn strip_tags(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_tag = false;
        for ch in s.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        out
    }
}
