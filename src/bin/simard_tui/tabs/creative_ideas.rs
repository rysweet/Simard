//! Creative Ideas tab: browse Simard's self-improvement idea pool and search it
//! by review status or free text.
//!
//! Layout mirrors the Journal tab: a newest-first idea list on the left (the
//! selected idea highlighted, each tagged with its review status) and the
//! selected idea's detail on the right (text, status, rationale, review and
//! link counts, and its success metric when one has been set). `/` starts a
//! search whose query filters the list by text **or** status; an empty pool
//! renders an honest "no ideas" message.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::creative_ideas::status_label;

/// Render the Creative Ideas tab content within the given area.
pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let filtered = app.creative_ideas_filtered();

    if filtered.is_empty() {
        let msg = if app.creative_ideas_entries.is_empty() {
            "No creative ideas yet — Simard fills this pool as its Creative Ideas thread runs.\n\n\
             Press 'r' to refresh."
                .to_string()
        } else {
            format!(
                "No ideas match \"{}\".\n\nPress '/' to edit the search, Esc to clear it.",
                app.creative_ideas_search
            )
        };
        let p = Paragraph::new(msg)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(search_title(app)),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(40), Constraint::Min(20)])
        .split(area);

    // ── Left: idea list, selected highlighted, status-tagged ────────────
    let selected = app.creative_ideas_selected.min(filtered.len() - 1);
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, idea)| {
            let marker = if i == selected { "▶ " } else { "  " };
            let line = format!("{marker}[{}] {}", status_label(idea.status), idea.idea);
            let style = if i == selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(status_color(idea.status))
            };
            ListItem::new(Line::from(line)).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(search_title(app)),
    );
    f.render_widget(list, chunks[0]);

    // ── Right: the selected idea's detail ───────────────────────────────
    let detail_lines: Vec<Line> = match app.creative_ideas_selected_entry() {
        Some(idea) => {
            let mut lines = vec![
                Line::from(idea.idea.clone()),
                Line::from(""),
                Line::from(format!("Status: {}", status_label(idea.status))),
                Line::from(format!(
                    "Reviews: {}   Links: {}",
                    idea.reviews.len(),
                    idea.links.len()
                )),
            ];
            if let Some(metric) = &idea.success_metric {
                lines.push(Line::from(format!(
                    "Success metric: {} — target {}",
                    metric.name, metric.target
                )));
            }
            if !idea.context.rationale.trim().is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from("Why:"));
                lines.push(Line::from(idea.context.rationale.clone()));
            }
            lines
        }
        None => vec![Line::from("Select an idea to read its detail.")],
    };
    let detail = Paragraph::new(detail_lines)
        .block(Block::default().borders(Borders::ALL).title("Idea"))
        .wrap(Wrap { trim: false });
    f.render_widget(detail, chunks[1]);
}

/// Title for the list block, reflecting search state and key hints.
fn search_title(app: &App) -> String {
    if app.creative_ideas_search_mode {
        format!("Search: {}_ (Enter/Esc)", app.creative_ideas_search)
    } else if app.creative_ideas_search.trim().is_empty() {
        "Creative Ideas — ↑/↓ idea · / search · r reload".to_string()
    } else {
        format!("Creative Ideas — filter: {}", app.creative_ideas_search)
    }
}

fn status_color(status: simard::cognitive_memory::creative_idea::IdeaStatus) -> Color {
    use simard::cognitive_memory::creative_idea::IdeaStatus;
    match status {
        IdeaStatus::New => Color::Cyan,
        IdeaStatus::NeedsRevision => Color::Yellow,
        IdeaStatus::NeedsHumanReview => Color::LightRed,
        IdeaStatus::AcceptedForImplementation | IdeaStatus::ImplementationStarted => Color::Green,
        IdeaStatus::ImplementationCompleted => Color::LightGreen,
        IdeaStatus::Deferred => Color::Gray,
        IdeaStatus::Rejected => Color::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use simard::cognitive_memory::creative_idea::{CreativeIdea, IdeaContext, IdeaStatus};

    fn ctx() -> IdeaContext {
        IdeaContext {
            source: "creative-ideas-thread".into(),
            goals_snapshot: vec![],
            observation_digest: String::new(),
            rationale: "recall precision plateaued for three days".into(),
        }
    }

    fn idea(text: &str, status: IdeaStatus) -> CreativeIdea {
        let mut i = CreativeIdea::new(text, ctx(), 1);
        i.status = status;
        i
    }

    fn render_text(app: &App) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw(f, app, Rect::new(0, 0, 100, 30)))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn pane_renders_ideas_with_status() {
        let mut app = App::new("simard-ooda.service".to_string(), None);
        app.creative_ideas_entries = vec![idea("improve recall ranking", IdeaStatus::New)];
        app.creative_ideas_loaded = true;

        let text = render_text(&app);
        assert!(
            text.contains("improve recall ranking"),
            "idea text shown: {text}"
        );
        assert!(text.contains("new"), "status label shown");
        assert!(
            text.contains("recall precision plateaued"),
            "rationale shown"
        );
    }

    #[test]
    fn pane_no_ideas_is_honest() {
        let app = App::new("simard-ooda.service".to_string(), None);
        let text = render_text(&app);
        assert!(
            text.contains("No creative ideas yet"),
            "honest empty state: {text}"
        );
    }

    #[test]
    fn search_filters_by_status() {
        let mut app = App::new("simard-ooda.service".to_string(), None);
        app.creative_ideas_entries = vec![
            idea("keep exploring recall", IdeaStatus::New),
            idea("risky worktree cleanup", IdeaStatus::NeedsHumanReview),
        ];
        app.creative_ideas_loaded = true;

        app.creative_ideas_search = "human".to_string();
        let filtered = app.creative_ideas_filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, IdeaStatus::NeedsHumanReview);
    }
}
