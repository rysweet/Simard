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

    // Active goals table. Filtered by the operator's tag filter (issue #2743):
    // `None` shows all; `Some(tag)` shows only goals carrying that exact label.
    let tag_filter = app.goals_tag_filter.as_deref();
    let filtered: Vec<&crate::types::ActiveGoal> = board
        .active
        .iter()
        .filter(|g| tag_filter.is_none_or(|t| g.labels.iter().any(|l| l == t)))
        .collect();

    let header = Row::new(vec![
        "Pri",
        "Description",
        "Status",
        "Assigned To",
        "Labels",
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = filtered
        .iter()
        .map(|g| Row::new(goal_row_cells(g)))
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Min(20),
        Constraint::Length(22),
        Constraint::Length(16),
        Constraint::Length(24),
    ];

    let title = match tag_filter {
        Some(tag) => format!(
            "Active Goals ({}/{}) [tag: {}] — 't' to cycle",
            filtered.len(),
            board.active.len(),
            tag
        ),
        None => format!(
            "Active Goals ({}) — 't' to filter by tag",
            board.active.len()
        ),
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title));

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

/// The five display cells for one active goal row: priority, description,
/// status, assignee, and the comma-joined labels (issue #2743). Kept as a pure
/// function so the row content — including the label text — is unit-testable
/// without constructing a ratatui `Frame`.
pub(crate) fn goal_row_cells(g: &crate::types::ActiveGoal) -> Vec<String> {
    vec![
        g.priority.to_string(),
        g.description.clone(),
        g.status.to_string(),
        g.assigned_to.clone().unwrap_or_default(),
        g.labels.join(", "),
    ]
}

#[cfg(test)]
mod tests {
    use super::goal_row_cells;
    use crate::types::{ActiveGoal, GoalProgress};

    fn goal(id: &str, labels: Vec<String>) -> ActiveGoal {
        ActiveGoal {
            id: id.to_string(),
            description: format!("desc {id}"),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            repo: None,
            current_activity: None,
            wip_refs: Vec::new(),
            labels,
        }
    }

    #[test]
    fn goal_row_cells_includes_comma_joined_label_text() {
        let cells = goal_row_cells(&goal(
            "g1",
            vec![
                "source:creative-ideas".to_string(),
                "area:dashboard".to_string(),
            ],
        ));
        assert_eq!(cells.len(), 5, "row has a trailing Labels cell");
        assert_eq!(cells[4], "source:creative-ideas, area:dashboard");
    }

    #[test]
    fn goal_row_cells_labels_cell_empty_for_unlabelled_goal() {
        let cells = goal_row_cells(&goal("g2", Vec::new()));
        assert_eq!(cells[4], "", "an unlabelled goal has an empty Labels cell");
    }
}
