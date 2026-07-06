//! Overview tab: the consolidated #2627 landing view.
//!
//! Absorbs the former standalone **Status** and **Stats** tabs so the seven-tab
//! TUI mirrors the dashboard's Overview → Summary · Health · Stats grouping
//! without losing any data. Three stacked panels:
//!
//! * **Summary** — daemon service, state, PID, uptime, CPU, memory.
//! * **Health** — the canonical operational status snapshot (`simard status`).
//! * **Stats** — aggregate counters (state files, sessions, issues, PRs, goals).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::system::DaemonState;

/// Render the Overview tab: Summary + Health + Stats panels.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(6),
            Constraint::Length(9),
        ])
        .split(area);

    draw_summary(f, app, chunks[0]);
    draw_health(f, app, chunks[1]);
    draw_stats(f, app, chunks[2]);
}

/// Summary panel: live daemon status, PID, uptime, CPU%, memory.
fn draw_summary(f: &mut Frame, app: &App, area: Rect) {
    let info = &app.daemon_info;

    let state_color = match info.state {
        DaemonState::Running => Color::Green,
        DaemonState::Stopped => Color::Red,
        DaemonState::NotFound => Color::Yellow,
        DaemonState::Unavailable => Color::DarkGray,
    };

    let bold = Style::default().add_modifier(Modifier::BOLD);

    let text = vec![
        Line::from(vec![
            Span::styled("Service: ", bold),
            Span::raw(&info.service_name),
        ]),
        Line::from(vec![
            Span::styled("Status:  ", bold),
            Span::styled(info.state.to_string(), Style::default().fg(state_color)),
        ]),
        Line::from(vec![
            Span::styled("PID:     ", bold),
            Span::raw(info.pid.map_or_else(|| "—".to_string(), |p| p.to_string())),
        ]),
        Line::from(vec![
            Span::styled("Uptime:  ", bold),
            Span::raw(format_uptime(info.uptime_secs)),
        ]),
        Line::from(vec![
            Span::styled("CPU:     ", bold),
            Span::raw(
                info.cpu_percent
                    .map_or_else(|| "—".to_string(), |c| format!("{c:.1}%")),
            ),
        ]),
        Line::from(vec![
            Span::styled("Memory:  ", bold),
            Span::raw(format_memory(info.memory_rss_kb)),
        ]),
    ];

    let block = Block::default().borders(Borders::ALL).title("Summary");
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

/// Health panel: the canonical operational status snapshot.
fn draw_health(f: &mut Frame, app: &App, area: Rect) {
    let _ = app;

    let snapshot = simard::status::assemble(&simard::status::provider::AssembleOptions::default());
    let text = simard::status::render::to_terminal(&snapshot);
    let lines: Vec<Line> = text.lines().map(Line::from).collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Health"))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

/// Stats panel: aggregate counters and daemon uptime.
fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
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
        Line::from(""),
        Line::from(Span::styled(
            " Alt+1\u{2013}7 / Tab / \u{2190}\u{2192} to switch tabs, q to quit",
            Style::default().fg(Color::DarkGray),
        )),
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

fn format_memory(kb: Option<u64>) -> String {
    match kb {
        None => "—".to_string(),
        Some(k) if k >= 1024 * 1024 => format!("{:.1} GB", k as f64 / (1024.0 * 1024.0)),
        Some(k) if k >= 1024 => format!("{:.1} MB", k as f64 / 1024.0),
        Some(k) => format!("{k} kB"),
    }
}
