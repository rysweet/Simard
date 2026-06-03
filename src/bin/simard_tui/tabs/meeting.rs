//! Meeting tab: interactive REPL for `simard meeting start`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, MeetingStatus};

/// Render the Meeting tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    // Title shows meeting status
    let title = match &app.meeting_status {
        MeetingStatus::NotStarted => "Meeting — Not Started".to_string(),
        MeetingStatus::Running => "Meeting — Running".to_string(),
        MeetingStatus::Exited(code) => format!("Meeting — Exited ({code})"),
        MeetingStatus::Error(e) => format!("Meeting — Error: {e}"),
    };

    // Output area (auto-scroll to bottom)
    let visible_height = chunks[0].height.saturating_sub(2) as usize;
    let skip = app.meeting_output.len().saturating_sub(visible_height);
    let output_lines: Vec<Line> = app
        .meeting_output
        .iter()
        .skip(skip)
        .map(|l| Line::from(l.as_str()))
        .collect();

    let output =
        Paragraph::new(output_lines).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(output, chunks[0]);

    // Input prompt
    let input_text = format!("> {}", app.meeting_input);
    let input =
        Paragraph::new(input_text).block(Block::default().borders(Borders::ALL).title("Input"));
    f.render_widget(input, chunks[1]);
}
