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

    // Input prompt with visible cursor
    let (before_cursor, after_cursor) = app.meeting_input.split_at(app.cursor_position);
    let input_text = if app.meeting_status == MeetingStatus::Running {
        use ratatui::style::{Modifier, Style};
        use ratatui::text::Span;
        // Show cursor as reversed char (or block at end)
        let cursor_char = after_cursor.chars().next().unwrap_or(' ');
        let after_skip = if after_cursor.is_empty() {
            ""
        } else {
            &after_cursor[cursor_char.len_utf8()..]
        };
        Line::from(vec![
            Span::raw(format!("> {before_cursor}")),
            Span::styled(
                cursor_char.to_string(),
                Style::default().add_modifier(Modifier::REVERSED),
            ),
            Span::raw(after_skip.to_string()),
        ])
    } else {
        Line::from(format!("> {}", app.meeting_input))
    };
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

    #[test]
    fn input_area_uses_length_5() {
        let app = App::new("simard-ooda.service".to_string());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw(f, &app, Rect::new(0, 0, 80, 24));
            })
            .unwrap();

        let buf = terminal.backend().buffer();

        // Find the "Input" title row dynamically
        let mut input_title_y = None;
        for y in 0..24u16 {
            let row_text: String = (0..80u16)
                .map(|x| {
                    buf.cell((x, y))
                        .map(|c| c.symbol().chars().next().unwrap_or(' '))
                        .unwrap_or(' ')
                })
                .collect();
            if row_text.contains("Input") {
                input_title_y = Some(y);
                break;
            }
        }
        let title_y = input_title_y.expect("Input title not found in rendered buffer");

        // With Length(5) constraint in a 24-row area:
        // Output area = rows 0..19 (Min(3) gets 19 rows)
        // Input area  = rows 19..24 (Length(5) gets 5 rows)
        // Title should appear at row 19
        assert_eq!(
            title_y, 19,
            "Input block should start at row 19 (Length(5) constraint), found at row {title_y}"
        );
    }

    #[test]
    fn long_input_wraps_to_second_line() {
        let mut app = App::new("simard-ooda.service".to_string());
        // "> " prefix (2 chars) + 120 'a's = 122 chars, exceeds 78 inner width
        app.meeting_input = "a".repeat(120);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                draw(f, &app, Rect::new(0, 0, 80, 24));
            })
            .unwrap();

        let buf = terminal.backend().buffer();

        // With Length(5), input block spans rows 19–23:
        //   Row 19: top border ("Input" title)
        //   Row 20: first content line ("> aaa...")
        //   Row 21: second content line (wrapped text should appear here)
        //   Row 22: third content line
        //   Row 23: bottom border
        let second_content_row = 21u16;
        let row_text: String = (1..79u16)
            .map(|x| {
                buf.cell((x, second_content_row))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            row_text.contains('a'),
            "with Wrap enabled, long input should wrap to second content row, got: {row_text:?}"
        );
    }
}
