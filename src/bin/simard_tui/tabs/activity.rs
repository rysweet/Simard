//! Activity tab: color-coded journal log entries from simard-ooda.service.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;

/// Render the Activity tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if app.log_lines.is_empty() {
        let msg = Paragraph::new("No log entries available.")
            .block(Block::default().borders(Borders::ALL).title("Activity"));
        f.render_widget(msg, area);
        return;
    }

    // Auto-scroll: skip non-visible lines before allocating Line objects
    let visible_height = area.height.saturating_sub(2) as usize;
    let skip = app.log_lines.len().saturating_sub(visible_height);
    let lines: Vec<Line> = app
        .log_lines
        .iter()
        .skip(skip)
        .map(|l| {
            let color = if l.contains("ERROR") || l.contains("error") {
                Color::Red
            } else if l.contains("WARN") || l.contains("warn") {
                Color::Yellow
            } else {
                Color::Reset
            };
            Line::from(Span::styled(l.as_str(), Style::default().fg(color)))
        })
        .collect();

    let paragraph =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Activity"));
    f.render_widget(paragraph, area);
}
