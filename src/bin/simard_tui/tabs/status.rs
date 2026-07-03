//! Status tab: canonical Simard status snapshot rendered as terminal text.

use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// Render the Status tab content within the given area.
pub fn draw(f: &mut ratatui::Frame, app: &crate::app::App, area: ratatui::layout::Rect) {
    let _ = app;

    let snapshot = simard::status::assemble(&simard::status::provider::AssembleOptions::default());
    let text = simard::status::render::to_terminal(&snapshot);
    let lines: Vec<Line> = text.lines().map(Line::from).collect();

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}
