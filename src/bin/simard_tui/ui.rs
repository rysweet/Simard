//! Tab bar layout, footer, and dispatch to tab renderers.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::app::{ALL_TABS, App, Tab};
use crate::tabs;

/// Render the full TUI frame: tab bar + active tab content + footer.
pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    let titles: Vec<Line> = ALL_TABS
        .iter()
        .map(|t| Line::from(format!(" {} {} ", t.number(), t.label())))
        .collect();

    let selected = ALL_TABS
        .iter()
        .position(|t| *t == app.active_tab)
        .unwrap_or(0);

    let tabs_widget = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("simard-tui"))
        .select(selected)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider("|");

    f.render_widget(tabs_widget, chunks[0]);

    match app.active_tab {
        Tab::Overview => tabs::overview::draw(f, app, chunks[1]),
        Tab::Goals => tabs::goals::draw(f, app, chunks[1]),
        Tab::Engineers => tabs::engineers::draw(f, app, chunks[1]),
        Tab::Activity => tabs::activity::draw(f, app, chunks[1]),
        Tab::Meeting => tabs::meeting::draw(f, app, chunks[1]),
        Tab::Stats => tabs::stats::draw(f, app, chunks[1]),
    }

    // If an update notice is available, show it in the footer area
    let footer_text = if let Some(ref notice) = app.update_notice {
        format!(
            "{notice}  | Alt+1\u{2013}6: tabs | Tab/Shift+Tab: cycle | \u{2190}/\u{2192}: prev/next | q: quit"
        )
    } else {
        "Alt+1\u{2013}6: tabs | Tab/Shift+Tab: cycle | \u{2190}/\u{2192}: prev/next | q: quit"
            .to_string()
    };
    let footer_style = if app.update_notice.is_some() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let footer = Paragraph::new(footer_text).style(footer_style);
    f.render_widget(footer, chunks[2]);
}
