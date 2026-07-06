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
/// tab: an `<h2>` date, the report narrative rendered as **structure** (section
/// `<h3>` headings, `<ul>` bullet lists, and `<p>` paragraphs — never literal
/// `##`/`-` markdown), and a plain-language table of the day's code-change
/// proposals (or an honest "no changes" note).
///
/// Every piece of untrusted text is passed through [`html_escape`], so a
/// narrative, heading, bullet, or PR summary containing `<script>` renders as
/// inert text.
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
    render_markdown_html(&mut h, &entry.narrative);
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

/// Render the report narrative's lightweight markdown (section `## ` headings,
/// `- ` bullets, blank-line-separated paragraphs) into escaped, XSS-safe HTML
/// structure. Unknown lines are ordinary paragraph text; every emitted piece of
/// text is [`html_escape`]d, so raw markup can never break out.
fn render_markdown_html(h: &mut String, narrative: &str) {
    let mut para: Vec<String> = Vec::new();
    let mut in_list = false;

    for raw_line in narrative.split('\n') {
        let trimmed = raw_line.trim();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            flush_paragraph(h, &mut para);
            close_list(h, &mut in_list);
            let _ = write!(
                h,
                "<h3 class=\"journal-heading\">{}</h3>",
                html_escape(rest.trim())
            );
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            flush_paragraph(h, &mut para);
            if !in_list {
                h.push_str("<ul class=\"journal-list\">");
                in_list = true;
            }
            let _ = write!(h, "<li>{}</li>", html_escape(rest.trim()));
        } else if trimmed.is_empty() {
            flush_paragraph(h, &mut para);
            close_list(h, &mut in_list);
        } else {
            close_list(h, &mut in_list);
            para.push(html_escape(trimmed));
        }
    }
    flush_paragraph(h, &mut para);
    close_list(h, &mut in_list);
}

/// Emit the buffered paragraph lines (joined by `<br>`) as a `<p>`, then clear.
fn flush_paragraph(h: &mut String, para: &mut Vec<String>) {
    if para.is_empty() {
        return;
    }
    h.push_str("<p>");
    h.push_str(&para.join("<br>"));
    h.push_str("</p>");
    para.clear();
}

/// Close an open `<ul>` if one is in progress.
fn close_list(h: &mut String, in_list: &mut bool) {
    if *in_list {
        h.push_str("</ul>");
        *in_list = false;
    }
}

/// Render an entry as plain text lines for the TUI Journal pane: a date header,
/// the report narrative (section `## ` markers dropped so headings read as plain
/// heading lines), and a plain-language list of the day's code-change proposals
/// (or an honest "no changes" line). All untrusted free text has terminal
/// control bytes neutralised so a crafted narrative or PR summary can never
/// emit ANSI escapes into the operator's terminal.
#[must_use]
pub fn render_entry_tui_lines(entry: &JournalEntry) -> Vec<String> {
    let mut lines = Vec::with_capacity(entry.prs.len() + 16);
    lines.push(format!("── Journal · {} ──", entry.date.format("%Y-%m-%d")));
    lines.push(String::new());
    for raw in entry.narrative.split('\n') {
        // Section headings render as plain heading text (drop the `## ` marker).
        let text = raw.strip_prefix("## ").unwrap_or(raw);
        lines.push(neutralize_control(text));
    }
    lines.push(String::new());
    if entry.prs.is_empty() {
        lines.push("No code changes were proposed on this day.".to_string());
    } else {
        lines.push("Code changes today:".to_string());
        for pr in &entry.prs {
            lines.push(neutralize_control(&format!(
                "  #{}  {} — {}",
                pr.number, pr.outcome, pr.plain_summary
            )));
        }
    }
    lines
}

/// Strip terminal control bytes (ANSI escapes, bell, and other C0/C1 controls)
/// from untrusted text so the TUI can render it verbatim without risk.
fn neutralize_control(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
