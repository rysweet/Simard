//! Goals tab: goal list with status, priority, assigned_to columns.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::app::App;

/// Render the Goals tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let board = &app.goal_board;

    if board.active.is_empty() && board.backlog.is_empty() {
        let msg = Paragraph::new("No goals found — goals stored in cognitive memory.")
            .block(Block::default().borders(Borders::ALL).title("Goals"));
        f.render_widget(msg, area);
        return;
    }

    let backlog_height = if board.backlog.is_empty() {
        0
    } else {
        (board.backlog.len() as u16).saturating_add(2).min(10)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(backlog_height)])
        .split(area);

    // Active goals table
    let header = Row::new(vec!["Pri", "Description", "Status", "Assigned To"])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let rows: Vec<Row> = board
        .active
        .iter()
        .map(|g| {
            Row::new(vec![
                g.priority.to_string(),
                g.description.clone(),
                g.status.to_string(),
                g.assigned_to.clone().unwrap_or_default(),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Min(20),
        Constraint::Length(22),
        Constraint::Length(16),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Active Goals ({})", board.active.len())),
    );

    f.render_widget(table, chunks[0]);

    if !board.backlog.is_empty() {
        let lines: Vec<Line> = board
            .backlog
            .iter()
            .map(|b| {
                Line::from(format!(
                    "  {} (score: {:.2}, source: {})",
                    b.description, b.score, b.source
                ))
            })
            .collect();
        let backlog = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Backlog ({})", board.backlog.len())),
        );
        f.render_widget(backlog, chunks[1]);
    }
}
