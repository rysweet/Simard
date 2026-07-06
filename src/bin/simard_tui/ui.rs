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
        Tab::Activity => tabs::activity::draw(f, app, chunks[1]),
        Tab::Workers => tabs::workers::draw(f, app, chunks[1]),
        Tab::Chat => tabs::chat::draw(f, app, chunks[1]),
        Tab::Overseer => tabs::overseer::draw(f, app, chunks[1]),
        Tab::Journal => tabs::journal::draw(f, app, chunks[1]),
        Tab::CreativeIdeas => tabs::creative_ideas::draw(f, app, chunks[1]),
    }

    // If an update notice is available, show it in the footer area
    let footer_text = if let Some(ref notice) = app.update_notice {
        format!(
            "{notice}  | Alt+1\u{2013}7: tabs | Tab/Shift+Tab: cycle | \u{2190}/\u{2192}: prev/next | q: quit"
        )
    } else {
        "Alt+1\u{2013}7: tabs | Tab/Shift+Tab: cycle | \u{2190}/\u{2192}: prev/next | q: quit"
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Flatten the rendered buffer into a single string (rows joined by `\n`),
    /// so tests can assert on visible text without a real terminal/PTY.
    fn buffer_to_string(terminal: &Terminal<TestBackend>, width: u16, height: u16) -> String {
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                let sym = buf
                    .cell((x, y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_else(|| " ".to_string());
                out.push_str(&sym);
            }
            out.push('\n');
        }
        out
    }

    /// Headless (no-PTY) smoke test: rendering the full frame on every tab must
    /// succeed against the in-memory `TestBackend`. This lets CI exercise the
    /// consolidated seven-tab TUI without allocating a pseudo-terminal.
    #[test]
    fn draw_renders_every_tab_without_pty() {
        const W: u16 = 100;
        const H: u16 = 40;

        for tab in ALL_TABS {
            let mut app = App::new("simard-ooda.service".to_string(), None);
            app.active_tab = tab;

            let backend = TestBackend::new(W, H);
            let mut terminal = Terminal::new(backend).unwrap();

            // Must not panic for any tab.
            terminal.draw(|f| draw(f, &app)).unwrap();

            let rendered = buffer_to_string(&terminal, W, H);
            // The bordered tab-bar block title is always present, proving the
            // frame reached the render stage rather than short-circuiting.
            assert!(
                rendered.contains("simard-tui"),
                "tab {tab:?}: expected the tab-bar block to render:\n{rendered}"
            );
        }
    }

    /// The tab bar must advertise every tab in the consolidated set, so no
    /// consolidated view becomes unreachable. Verified purely from the
    /// in-memory buffer — no PTY required.
    #[test]
    fn tab_bar_lists_all_tabs_without_pty() {
        const W: u16 = 140;
        const H: u16 = 40;

        let app = App::new("simard-ooda.service".to_string(), None);
        let backend = TestBackend::new(W, H);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        let rendered = buffer_to_string(&terminal, W, H);
        for tab in ALL_TABS {
            let label = tab.label();
            assert!(
                rendered.contains(label),
                "tab bar is missing label {label:?} for {tab:?}:\n{rendered}"
            );
        }
    }
}
