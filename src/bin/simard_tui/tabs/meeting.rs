//! Meeting tab: interactive REPL for `simard meeting start`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, MeetingStatus};

/// Render the Meeting tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(5)])
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
    let input = Paragraph::new(input_text)
        .block(Block::default().borders(Borders::ALL).title("Input"))
        .wrap(Wrap { trim: false });
    f.render_widget(input, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Render meeting tab into an 80×24 test terminal and return the buffer.
    fn render(app: &App) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw(f, app, Rect::new(0, 0, 80, 24)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// Read a row of inner text (cols 1..79) from the buffer.
    fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (1..79u16)
            .map(|x| {
                buf.cell((x, y))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect()
    }

    #[test]
    fn input_area_uses_length_5() {
        let app = App::new("simard-ooda.service".to_string());
        let buf = render(&app);

        // Find the "Input" title row dynamically
        let title_y = (0..24u16)
            .find(|&y| {
                let text: String = (0..80u16)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol().chars().next().unwrap_or(' '))
                            .unwrap_or(' ')
                    })
                    .collect();
                text.contains("Input")
            })
            .expect("Input title not found in rendered buffer");

        assert_eq!(
            title_y, 19,
            "Input block should start at row 19 (Length(5) constraint), found at row {title_y}"
        );
    }

    #[test]
    fn long_input_wraps_to_second_line() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.meeting_input = "a".repeat(120);

        let buf = render(&app);

        let text = row_text(&buf, 21);
        assert!(
            text.contains('a'),
            "with Wrap enabled, long input should wrap to second content row, got: {text:?}"
        );
    }

    #[test]
    fn input_area_has_at_least_3_content_lines() {
        let app = App::new("simard-ooda.service".to_string());
        let buf = render(&app);

        let left_border = buf
            .cell((0u16, 22u16))
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .unwrap_or(' ');
        let right_border = buf
            .cell((79u16, 22u16))
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .unwrap_or(' ');
        assert_ne!(left_border, ' ', "row 22 col 0 should have border char");
        assert_ne!(right_border, ' ', "row 22 col 79 should have border char");
    }

    #[test]
    fn very_long_input_fills_three_content_lines() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.meeting_input = "abcd ".repeat(50);

        let buf = render(&app);

        for row in [21u16, 22u16] {
            let text = row_text(&buf, row);
            assert!(
                text.contains('a'),
                "content row {row} should contain wrapped text, got: {text:?}"
            );
        }
    }

    #[test]
    fn empty_input_renders_prompt_only() {
        let app = App::new("simard-ooda.service".to_string());
        let buf = render(&app);

        let text = row_text(&buf, 20);
        assert!(
            text.contains('>'),
            "empty input should still show '>' prompt, got: {text:?}"
        );
    }

    #[test]
    fn wrap_preserves_spaces_with_trim_false() {
        let mut app = App::new("simard-ooda.service".to_string());
        app.meeting_input = format!("{}   trailing spaces", "x".repeat(76));

        let buf = render(&app);

        let text = row_text(&buf, 21);
        let has_content = text.contains('x') || text.contains("trailing");
        assert!(
            has_content,
            "wrapped line should contain text (trim: false preserves spaces), got: {text:?}"
        );
    }
}
