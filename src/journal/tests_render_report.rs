//! src/journal/tests_render_report.rs
//!
//! Tests (issue #2606): the shared renderer turns the report's `##` headings and
//! its pull-request table into real **structure** on both surfaces — the
//! dashboard emits heading elements (never literal `##`) and the TUI emits
//! heading lines with terminal control bytes neutralised — so the same
//! jargon-free report renders cleanly in the dashboard Journal tab AND the TUI
//! Journal pane.
//!
//! Specifies the TARGET behaviour: the pre-fix #2618 renderer wraps the whole
//! narrative in `<p>` (leaking literal `##` into the dashboard) and does not
//! neutralise terminal control bytes in the TUI.

use chrono::Utc;

use super::test_support::{day, pr};
use crate::journal::render::{render_entry_html, render_entry_tui_lines};
use crate::journal::types::JournalEntry;

/// A report-shaped entry: an overview paragraph plus two `##`-headed sections.
fn report_entry() -> JournalEntry {
    JournalEntry {
        date: day(),
        generated_at: Utc::now(),
        narrative: "Overview. Simard improved how it recalls its own work.\n\n\
                    ## Engineering work\n\n\
                    A change was combined into the main code.\n\n\
                    ## Outcomes\n\n\
                    The dashboard is faster."
            .to_string(),
        draft: String::new(),
        prs: vec![pr(
            12,
            "made the dashboard faster",
            "still open — ready to combine into the main code",
        )],
        quiet_day: false,
    }
}

#[test]
fn html_renders_report_headings_as_structure_not_literal_markdown() {
    let html = render_entry_html(&report_entry());

    // Section headings become real structure, never raw '##' text.
    assert!(
        !html.contains("## "),
        "raw markdown headings must not leak into the dashboard HTML: {html}"
    );
    assert!(
        html.contains("Engineering work"),
        "first section heading text present: {html}"
    );
    assert!(
        html.contains("Outcomes"),
        "second section heading text present: {html}"
    );
    assert!(html.contains("Overview"), "overview present: {html}");
    // The plain-language PR table is still rendered.
    assert!(
        html.contains("<table class=\"journal-prs\""),
        "PR table present: {html}"
    );
    assert!(
        html.contains("made the dashboard faster"),
        "PR summary present: {html}"
    );
}

#[test]
fn html_report_is_still_xss_safe() {
    let mut entry = report_entry();
    entry
        .narrative
        .push_str("\n\n## Notes\n\n<script>alert('x')</script> lingered.");
    entry.prs = vec![pr(
        7,
        "made login <b>safer</b>",
        "still open — not ready yet",
    )];

    let html = render_entry_html(&entry);
    assert!(
        !html.contains("<script>"),
        "raw <script> must not survive: {html}"
    );
    assert!(html.contains("&lt;script&gt;"), "script is escaped: {html}");
    assert!(
        !html.contains("<b>safer</b>"),
        "raw PR markup must not survive: {html}"
    );
}

#[test]
fn tui_renders_headings_and_neutralises_control_bytes() {
    let mut entry = report_entry();
    // Inject terminal control bytes into untrusted text: an ANSI escape in the
    // narrative and a bell byte in a PR summary.
    entry.narrative.push_str("\n\n\u{1b}[31mALERT\u{1b}[0m");
    entry.prs = vec![pr(9, "ring the \u{7} bell", "still open — not ready yet")];

    let lines = render_entry_tui_lines(&entry);
    let joined = lines.join("\n");

    assert!(
        joined.contains("Engineering work"),
        "TUI shows section headings: {joined:?}"
    );
    assert!(
        !joined.contains('\u{1b}'),
        "ANSI escape byte must be neutralised in the TUI render"
    );
    assert!(
        !joined.contains('\u{7}'),
        "bell byte must be neutralised in the TUI render"
    );
}
