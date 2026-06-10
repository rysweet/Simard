//! Overview tab: daemon status, PID, uptime, CPU%, memory.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::system::DaemonState;

/// Render the Overview tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
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
        Line::from(""),
        Line::from(Span::styled(
            " Alt+1\u{2013}6 / Tab / \u{2190}\u{2192} to switch tabs, q to quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default().borders(Borders::ALL).title("Overview");
    let paragraph = Paragraph::new(text).block(block);
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
