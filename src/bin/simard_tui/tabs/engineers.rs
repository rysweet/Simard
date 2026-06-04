//! Engineers tab: child processes of the daemon with CPU, memory, runtime.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::app::App;

/// Render the Engineers tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    if app.child_processes.is_empty() {
        let msg = if app.daemon_info.pid.is_some() {
            "No child processes found."
        } else {
            "Daemon is not running."
        };
        let paragraph =
            Paragraph::new(msg).block(Block::default().borders(Borders::ALL).title("Engineers"));
        f.render_widget(paragraph, area);
        return;
    }

    let header = Row::new(vec![
        "PID", "Command", "CPU%", "Memory", "Runtime", "Category",
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .child_processes
        .iter()
        .map(|p| {
            Row::new(vec![
                p.pid.to_string(),
                p.command.clone(),
                p.cpu_percent
                    .map_or_else(|| "—".to_string(), |c| format!("{c:.1}%")),
                format_memory(p.memory_kb),
                format_runtime(p.runtime_secs),
                p.category.clone().unwrap_or_else(|| "—".to_string()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(16),
    ];

    let title = format!("Engineers ({} processes)", app.child_processes.len());
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(table, area);
}

fn format_memory(kb: Option<u64>) -> String {
    match kb {
        None => "—".to_string(),
        Some(k) if k >= 1024 * 1024 => format!("{:.1} GB", k as f64 / (1024.0 * 1024.0)),
        Some(k) if k >= 1024 => format!("{:.1} MB", k as f64 / 1024.0),
        Some(k) => format!("{k} kB"),
    }
}

fn format_runtime(secs: Option<u64>) -> String {
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
