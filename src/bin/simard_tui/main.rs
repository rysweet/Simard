//! simard-tui: CLI TUI monitoring client for Simard.
//!
//! Displays daemon status, goals, and activity in a terminal UI.
//! Tabs switch with 1–6 keys, auto-refreshes every 2s, quit with q.

mod app;
mod goals;
mod system;
mod tabs;
mod types;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// RAII guard that restores terminal state on drop.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("simard-tui: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Chain panic hook to restore terminal before default panic output.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    let service_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "simard-ooda.service".to_string());

    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;

    let mut app = app::App::new(service_name);
    app.refresh();

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_secs(2))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && let KeyCode::Char(c) = key.code
            {
                app.handle_key(c);
            }
        } else {
            // Timeout — auto-refresh
            app.refresh();
        }
    }

    Ok(())
}
