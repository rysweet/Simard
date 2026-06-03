//! Stats tab: key-value overview of state files, issues, PRs, goals, uptime.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;

/// Render the Stats tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let fmt_opt = |v: Option<usize>| v.map_or_else(|| "—".to_string(), |n| n.to_string());

    let text = vec![
        Line::from(vec![
            Span::styled("State files:   ", bold),
            Span::raw(fmt_opt(app.stats_cache.state_files)),
        ]),
        Line::from(vec![
            Span::styled("Session dirs:  ", bold),
            Span::raw(fmt_opt(app.stats_cache.session_dirs)),
        ]),
        Line::from(vec![
            Span::styled("Open issues:   ", bold),
            Span::raw(fmt_opt(app.stats_cache.open_issues)),
        ]),
        Line::from(vec![
            Span::styled("Open PRs:      ", bold),
            Span::raw(fmt_opt(app.stats_cache.open_prs)),
        ]),
        Line::from(vec![
            Span::styled("Active goals:  ", bold),
            Span::raw(app.goal_board.active.len().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Daemon uptime: ", bold),
            Span::raw(format_uptime(app.daemon_info.uptime_secs)),
        ]),
    ];

    let paragraph =
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Stats"));
    f.render_widget(paragraph, area);
}

fn format_uptime(secs: Option<u64>) -> String {
    match secs {
        None => "—".to_string(),
        Some(s) => {
            let h = s / 3600;
            let m = (s % 3600) / 60;
            let sec = s % 60;
            if h > 0 {
                format!("{h}h {m}m {sec}s")
            } else if m > 0 {
                format!("{m}m {sec}s")
            } else {
                format!("{sec}s")
            }
        }
    }
}
