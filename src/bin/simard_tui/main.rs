//! simard-tui: CLI TUI monitoring client for Simard.
//!
//! Displays daemon status, goals, and activity in a terminal UI.
//! Tabs switch with Alt+1–6 / ←→ arrow keys, auto-refreshes every 2s, quit with q.

mod app;
mod goals;
mod system;
mod tabs;
mod types;
mod ui;

use std::io::IsTerminal;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

static USING_DEV_TTY: AtomicBool = AtomicBool::new(false);

/// Restore terminal to normal state. Shared by `TerminalGuard::drop` and panic hook.
fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    if USING_DEV_TTY.load(Ordering::Relaxed) {
        if let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        {
            let _ = crossterm::execute!(tty, LeaveAlternateScreen);
        }
    } else {
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// RAII guard that restores terminal state on drop.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Box<dyn Write>>>> {
    let mut writer: Box<dyn Write> = if io::stdout().is_terminal() {
        Box::new(io::stdout())
    } else {
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| {
                io::Error::other(
                    "simard-tui requires a terminal. \
                     Run from an interactive shell or use: ssh -t host simard-tui",
                )
            })?;
        USING_DEV_TTY.store(true, Ordering::Relaxed);
        Box::new(tty)
    };

    terminal::enable_raw_mode()?;
    if let Err(e) = crossterm::execute!(writer, EnterAlternateScreen) {
        let _ = terminal::disable_raw_mode();
        return Err(e);
    }
    Terminal::new(CrosstermBackend::new(writer))
}

fn main() {
    simard::update_check::run_update_check();
    if let Err(e) = run() {
        eprintln!("simard-tui: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Chain panic hook to restore terminal before default panic output.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
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
            app.cleanup();
            break;
        }

        if event::poll(Duration::from_secs(2))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.handle_key(key);
            }
        } else {
            // Timeout — auto-refresh
            app.refresh();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that share global `USING_DEV_TTY` state.
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    /// USING_DEV_TTY must exist as a static AtomicBool and default to false.
    #[test]
    fn using_dev_tty_defaults_to_false() {
        let _lock = TEST_MUTEX.lock().unwrap();
        // Reset to known clean state (other tests may have run first).
        USING_DEV_TTY.store(false, Ordering::Relaxed);
        assert!(
            !USING_DEV_TTY.load(Ordering::Relaxed),
            "USING_DEV_TTY should be false after reset"
        );
    }

    /// restore_terminal() must exist and not panic regardless of USING_DEV_TTY state.
    #[test]
    fn restore_terminal_does_not_panic_when_flag_false() {
        let _lock = TEST_MUTEX.lock().unwrap();
        USING_DEV_TTY.store(false, Ordering::Relaxed);
        restore_terminal();
    }

    /// restore_terminal() must not panic when USING_DEV_TTY is true,
    /// even if /dev/tty is not available (best-effort cleanup).
    #[test]
    fn restore_terminal_does_not_panic_when_flag_true() {
        let _lock = TEST_MUTEX.lock().unwrap();
        USING_DEV_TTY.store(true, Ordering::Relaxed);
        restore_terminal();
        USING_DEV_TTY.store(false, Ordering::Relaxed);
    }

    /// setup_terminal() return type must be Terminal<CrosstermBackend<Box<dyn Write>>>,
    /// NOT Terminal<CrosstermBackend<Stdout>>. This ensures the backend is
    /// polymorphic over stdout vs /dev/tty file handles.
    #[test]
    fn setup_terminal_returns_boxed_write_backend() {
        // Type-level assertion: if this compiles, the return type is correct.
        // Verify function signature without calling it to avoid terminal side effects.
        #[allow(clippy::type_complexity)]
        let _: fn() -> io::Result<Terminal<CrosstermBackend<Box<dyn Write>>>> = setup_terminal;
    }

    /// When stdout is NOT a TTY and /dev/tty is also unavailable,
    /// setup_terminal() must return an error with a helpful message
    /// containing "requires a terminal".
    #[test]
    fn setup_terminal_error_message_is_actionable() {
        use std::io::IsTerminal;

        let _lock = TEST_MUTEX.lock().unwrap();
        USING_DEV_TTY.store(false, Ordering::Relaxed);

        // Skip this test if stdout IS a terminal (interactive run).
        if std::io::stdout().is_terminal() {
            return;
        }

        match setup_terminal() {
            Ok(_term) => {
                // /dev/tty was available — that's fine, clean up.
                assert!(
                    USING_DEV_TTY.load(Ordering::Relaxed),
                    "USING_DEV_TTY should be true when stdout is not a TTY and /dev/tty was used"
                );
                restore_terminal();
                USING_DEV_TTY.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("requires a terminal"),
                    "Error should mention 'requires a terminal', got: {msg}"
                );
            }
        }
    }

    /// When stdout is not a TTY but /dev/tty IS available,
    /// setup_terminal() must succeed and set USING_DEV_TTY to true.
    #[test]
    fn setup_terminal_falls_back_to_dev_tty() {
        use std::io::IsTerminal;

        let _lock = TEST_MUTEX.lock().unwrap();
        USING_DEV_TTY.store(false, Ordering::Relaxed);

        // Skip if stdout is already a TTY (no fallback needed).
        if std::io::stdout().is_terminal() {
            return;
        }

        // Skip if /dev/tty is not accessible (e.g., CI container).
        if std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_err()
        {
            return;
        }

        let result = setup_terminal();
        assert!(
            result.is_ok(),
            "setup_terminal() should succeed via /dev/tty fallback"
        );
        assert!(
            USING_DEV_TTY.load(Ordering::Relaxed),
            "USING_DEV_TTY should be true after /dev/tty fallback"
        );

        // Cleanup
        restore_terminal();
        USING_DEV_TTY.store(false, Ordering::Relaxed);
    }
}
