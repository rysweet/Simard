//! Placeholder tab for unimplemented tabs (Engineers, Activity, Meeting, Stats).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use crate::app::Tab;

/// Render a placeholder message for tabs not yet implemented.
pub fn draw(f: &mut Frame, tab: Tab, area: Rect) {
    let msg = format!("{} tab — coming soon", tab.label());
    f.render_widget(Paragraph::new(msg), area);
}
