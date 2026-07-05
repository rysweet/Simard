//! Tests: the dashboard HTML render (PR table + narrative, XSS-safe) and the
//! TUI pane render both show the entry, and an empty day renders honestly
//! (issue #2606).

use chrono::Utc;

use super::test_support::{day, pr};
use crate::journal::render::{render_entry_html, render_entry_tui_lines};
use crate::journal::types::JournalEntry;

fn entry_with_prs() -> JournalEntry {
    JournalEntry {
        date: day(),
        generated_at: Utc::now(),
        narrative: "Today I fixed things.\n\n<script>alert('x')</script> lingered in a memory."
            .to_string(),
        draft: String::new(),
        prs: vec![
            pr(12, "Made login <b>safer</b>", "merged"),
            pr(15, "Sped up the dashboard", "open"),
        ],
        quiet_day: false,
    }
}

fn quiet_entry() -> JournalEntry {
    JournalEntry {
        date: day(),
        generated_at: Utc::now(),
        narrative: "2026-07-05 was a quiet day. Nothing remarkable happened.".to_string(),
        draft: String::new(),
        prs: vec![],
        quiet_day: true,
    }
}

#[test]
fn html_renders_narrative_and_pr_table() {
    let html = render_entry_html(&entry_with_prs());

    assert!(html.contains("Today I fixed things."), "narrative present");
    assert!(
        html.contains("<table class=\"journal-prs\""),
        "PR table present"
    );
    assert!(html.contains("#12"), "PR #12 row present");
    assert!(html.contains("#15"), "PR #15 row present");
    assert!(html.contains("Sped up the dashboard"), "PR summary present");
    assert!(html.contains("merged"), "PR outcome present");
    // header row + two data rows.
    assert_eq!(html.matches("<tr>").count(), 3, "one header + two PR rows");
}

#[test]
fn html_is_xss_safe() {
    let html = render_entry_html(&entry_with_prs());

    // Untrusted narrative markup is escaped, not emitted live.
    assert!(
        !html.contains("<script>"),
        "raw <script> must not survive: {html}"
    );
    assert!(html.contains("&lt;script&gt;"), "script is escaped");

    // Untrusted PR-summary markup is escaped too.
    assert!(!html.contains("<b>safer</b>"), "raw <b> must not survive");
    assert!(
        html.contains("&lt;b&gt;safer&lt;/b&gt;"),
        "PR markup is escaped"
    );
}

#[test]
fn html_empty_day_is_honest() {
    let html = render_entry_html(&quiet_entry());
    assert!(html.to_lowercase().contains("quiet"), "says it was quiet");
    assert!(
        html.contains("No code changes were proposed"),
        "honest no-changes note"
    );
    assert!(!html.contains("<table"), "no PR table on a quiet day");
}

#[test]
fn tui_renders_narrative_and_prs() {
    let lines = render_entry_tui_lines(&entry_with_prs());
    let joined = lines.join("\n");

    assert!(joined.contains("2026-07-05"), "date header present");
    assert!(
        joined.contains("Today I fixed things."),
        "narrative present"
    );
    assert!(
        joined.contains("Code changes today:"),
        "PR section header present"
    );
    assert!(joined.contains("#12"), "PR #12 present");
    assert!(
        joined.contains("Sped up the dashboard"),
        "PR summary present"
    );
}

#[test]
fn tui_empty_day_is_honest() {
    let lines = render_entry_tui_lines(&quiet_entry());
    let joined = lines.join("\n");
    assert!(joined.to_lowercase().contains("quiet"), "says it was quiet");
    assert!(
        joined.contains("No code changes were proposed"),
        "honest no-changes line"
    );
}
