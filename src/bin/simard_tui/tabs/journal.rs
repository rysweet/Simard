//! Journal tab: browse Simard's daily diary by date and read the day's
//! plain-language narrative + code-change table (issue #2606).
//!
//! Layout mirrors the dashboard Journal tab: a newest-first date list on the
//! left (the selected day highlighted) and the rendered entry on the right. The
//! entry text comes from the shared library renderer
//! ([`simard::journal::render_entry_tui_lines`]) so the TUI and the dashboard
//! show the same jargon-free story. `/` starts a search whose query filters the
//! date list; an empty store renders an honest "no entries" message.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::journal::render_entry_tui_lines;

/// Render the Journal tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let filtered = app.journal_filtered();

    // No entries at all (or none matching the search): honest empty state.
    if filtered.is_empty() {
        let msg = if app.journal_entries.is_empty() {
            "No journal entries yet — Simard writes one per day, stored in its memory.\n\n\
             Press 'r' to refresh."
                .to_string()
        } else {
            format!(
                "No entries match \"{}\".\n\nPress '/' to edit the search, Esc to clear it.",
                app.journal_search
            )
        };
        let title = search_title(app);
        let p = Paragraph::new(msg)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Min(20)])
        .split(area);

    // ── Left: newest-first date list, selected day highlighted ──────────
    let selected = app.journal_selected.min(filtered.len() - 1);
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let marker = if i == selected { "▶ " } else { "  " };
            let tag = if e.quiet_day {
                " (quiet)".to_string()
            } else if e.prs.is_empty() {
                String::new()
            } else {
                format!(" ({} chg)", e.prs.len())
            };
            let line = format!("{marker}{}{tag}", e.date.format("%Y-%m-%d"));
            let style = if i == selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(Line::from(line)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(search_title(app)),
    );
    f.render_widget(list, chunks[0]);

    // ── Right: the selected day's rendered narrative + PR list ──────────
    let entry_lines: Vec<Line> = match app.journal_selected_entry() {
        Some(entry) => render_entry_tui_lines(entry)
            .into_iter()
            .map(Line::from)
            .collect(),
        None => vec![Line::from("Select a day to read its journal.")],
    };
    let entry = Paragraph::new(entry_lines)
        .block(Block::default().borders(Borders::ALL).title("Entry"))
        .wrap(Wrap { trim: false });
    f.render_widget(entry, chunks[1]);
}

/// Title for the date-list block, reflecting search state and key hints.
fn search_title(app: &App) -> String {
    if app.journal_search_mode {
        format!("Search: {}_ (Enter/Esc)", app.journal_search)
    } else if app.journal_search.trim().is_empty() {
        "Journal — ↑/↓ day · / search · r reload".to_string()
    } else {
        format!("Journal — filter: {}", app.journal_search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::JournalEntry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use simard::journal::PrSummary;

    fn ymd(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn entry_with_prs() -> JournalEntry {
        JournalEntry {
            date: ymd(2026, 7, 5),
            generated_at: chrono::Utc::now(),
            narrative: "Today I helped fix the login page.".to_string(),
            draft: String::new(),
            prs: vec![
                PrSummary {
                    number: 12,
                    plain_summary: "Made login safer".to_string(),
                    outcome: "merged".to_string(),
                },
                PrSummary {
                    number: 15,
                    plain_summary: "Sped up the dashboard".to_string(),
                    outcome: "open".to_string(),
                },
            ],
            quiet_day: false,
        }
    }

    fn quiet_entry() -> JournalEntry {
        JournalEntry {
            date: ymd(2026, 7, 6),
            generated_at: chrono::Utc::now(),
            narrative: "2026-07-06 was a quiet day. Nothing remarkable happened.".to_string(),
            draft: String::new(),
            prs: vec![],
            quiet_day: true,
        }
    }

    /// Render the Journal pane into an 100x30 test terminal and return the
    /// flattened buffer text.
    fn render_text(app: &App) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw(f, app, Rect::new(0, 0, 100, 30)))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn pane_renders_date_narrative_and_prs() {
        let mut app = App::new("simard-ooda.service".to_string(), None);
        app.journal_entries = vec![entry_with_prs()];
        app.journal_loaded = true;

        let text = render_text(&app);
        assert!(text.contains("2026-07-05"), "date shown: {text}");
        assert!(
            text.contains("helped fix the login page"),
            "narrative shown"
        );
        assert!(text.contains("Code changes today:"), "PR section shown");
        assert!(text.contains("#12"), "PR #12 shown");
        assert!(text.contains("Sped up the dashboard"), "PR summary shown");
    }

    #[test]
    fn pane_empty_day_is_honest() {
        let mut app = App::new("simard-ooda.service".to_string(), None);
        app.journal_entries = vec![quiet_entry()];
        app.journal_loaded = true;

        let text = render_text(&app);
        assert!(text.to_lowercase().contains("quiet"), "quiet day narrated");
        assert!(
            text.contains("No code changes were proposed"),
            "honest no-changes line: {text}"
        );
    }

    #[test]
    fn pane_no_entries_is_honest() {
        let app = App::new("simard-ooda.service".to_string(), None);
        // No journal_entries seeded.
        let text = render_text(&app);
        assert!(
            text.contains("No journal entries yet"),
            "honest empty state: {text}"
        );
    }

    #[test]
    fn search_filters_the_date_list() {
        let mut app = App::new("simard-ooda.service".to_string(), None);
        app.journal_entries = vec![entry_with_prs(), quiet_entry()];
        app.journal_loaded = true;

        // Filter to the login day only.
        app.journal_search = "login".to_string();
        let filtered = app.journal_filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].date, ymd(2026, 7, 5));

        let text = render_text(&app);
        assert!(text.contains("2026-07-05"), "matching day listed");
        assert!(
            !text.contains("2026-07-06"),
            "non-matching day filtered out"
        );
    }
}
