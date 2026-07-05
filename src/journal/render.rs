//! Jargon-free, XSS-safe rendering of a journal entry (issue #2606).
//!
//! These are pure functions — the same rendered entry feeds the dashboard
//! Journal tab ([`render_entry_html`]) and the TUI Journal pane
//! ([`render_entry_tui_lines`]). Keeping the render logic here (rather than
//! inside the axum handler or the ratatui widget) means both surfaces render
//! identically and both are unit-testable without a server or a terminal.
//!
//! All operator-visible free text (the narrative and every PR summary/outcome)
//! is untrusted — it originates from model output and repository data — so the
//! HTML renderer escapes it. The narrative itself is already jargon-scrubbed by
//! the mandatory review pass, so the renderer only has to worry about markup
//! safety, not readability.

use std::fmt::Write as _;

use crate::journal::types::JournalEntry;

/// HTML-escape untrusted text so it can never break out of a text node or
/// attribute (`&`, `<`, `>`, `"`, `'`).
#[must_use]
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render an entry as a self-contained HTML fragment for the dashboard Journal
/// tab: an `<h2>` date, the narrative in paragraphs, and a plain-language table
/// of the day's code-change proposals (or an honest "no changes" note).
///
/// Every piece of untrusted text is passed through [`html_escape`], so a
/// narrative or PR summary containing `<script>` renders as inert text.
#[must_use]
pub fn render_entry_html(entry: &JournalEntry) -> String {
    let mut h = String::with_capacity(entry.narrative.len() + 512);
    h.push_str("<div class=\"journal-entry\">");
    let _ = write!(
        h,
        "<h2 class=\"journal-date\">{}</h2>",
        html_escape(&entry.date.format("%Y-%m-%d").to_string())
    );

    h.push_str("<div class=\"journal-narrative\">");
    for para in entry.narrative.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        h.push_str("<p>");
        h.push_str(&html_escape(para).replace('\n', "<br>"));
        h.push_str("</p>");
    }
    h.push_str("</div>");

    if entry.prs.is_empty() {
        h.push_str("<p class=\"journal-no-prs\">No code changes were proposed on this day.</p>");
    } else {
        h.push_str(
            "<table class=\"journal-prs\"><thead><tr>\
             <th>PR #</th><th>What changed &amp; why it matters</th><th>Outcome</th>\
             </tr></thead><tbody>",
        );
        for pr in &entry.prs {
            // `pr.number` is a `u64`; the free-text columns are escaped.
            let _ = write!(
                h,
                "<tr><td>#{}</td><td>{}</td><td>{}</td></tr>",
                pr.number,
                html_escape(&pr.plain_summary),
                html_escape(&pr.outcome)
            );
        }
        h.push_str("</tbody></table>");
    }

    h.push_str("</div>");
    h
}

/// Render an entry as plain text lines for the TUI Journal pane: a date header,
/// the narrative, and a plain-language list of the day's code-change proposals
/// (or an honest "no changes" line).
#[must_use]
pub fn render_entry_tui_lines(entry: &JournalEntry) -> Vec<String> {
    let mut lines = Vec::with_capacity(entry.prs.len() + 8);
    lines.push(format!("── Journal · {} ──", entry.date.format("%Y-%m-%d")));
    lines.push(String::new());
    for para in entry.narrative.split('\n') {
        lines.push(para.to_string());
    }
    lines.push(String::new());
    if entry.prs.is_empty() {
        lines.push("No code changes were proposed on this day.".to_string());
    } else {
        lines.push("Code changes today:".to_string());
        for pr in &entry.prs {
            lines.push(format!(
                "  #{}  {} — {}",
                pr.number, pr.outcome, pr.plain_summary
            ));
        }
    }
    lines
}
