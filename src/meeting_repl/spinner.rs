//! Lightweight progress spinner for LLM calls in the meeting REPL.
//!
//! Shows an animated spinner on stderr after a configurable delay (default
//! 500 ms). Automatically suppressed when stderr is not a terminal.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_INTERVAL: Duration = Duration::from_millis(80);

/// Default delay before the spinner becomes visible.
pub const DEFAULT_DELAY: Duration = Duration::from_millis(500);

/// A progress spinner that animates on stderr in a background thread.
///
/// Create with [`Spinner::new`], which returns immediately. The spinner
/// thread starts animating after `delay` elapses. Call [`Spinner::stop`]
/// (or drop) to clear the line and join the thread.
///
/// In non-TTY mode the spinner is a no-op: no thread is spawned and
/// `stop` returns instantly.
pub struct Spinner {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner that will display `message` on stderr after `delay`.
    ///
    /// If stderr is not a terminal, returns a no-op spinner.
    pub fn new(message: &str, delay: Duration) -> Self {
        if !std::io::stderr().is_terminal() {
            return Self::noop();
        }
        Self::spawn(message.to_string(), delay)
    }

    /// Start a spinner with the default 500 ms delay.
    pub fn after_default_delay(message: &str) -> Self {
        Self::new(message, DEFAULT_DELAY)
    }

    /// Create a no-op spinner (no thread, no output).
    fn noop() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(true)),
            handle: None,
        }
    }

    fn spawn(message: String, delay: Duration) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let flag = stop_flag.clone();

        let handle = thread::spawn(move || {
            // Wait for the delay period, checking for early cancellation.
            let deadline = std::time::Instant::now() + delay;
            while std::time::Instant::now() < deadline {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }

            // Animate until stopped.
            let mut idx = 0;
            let mut stderr = std::io::stderr().lock();
            while !flag.load(Ordering::Relaxed) {
                let frame = FRAMES[idx % FRAMES.len()];
                let _ = write!(stderr, "\r  {frame} {message}");
                let _ = stderr.flush();
                idx += 1;
                // Sleep in small increments so we can check the flag.
                for _ in 0..(FRAME_INTERVAL.as_millis() / 20) {
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
            // Clear the spinner line.
            let _ = write!(stderr, "\r\x1b[K");
            let _ = stderr.flush();
        });

        Self {
            stop_flag,
            handle: Some(handle),
        }
    }

    /// Stop the spinner and clear the line. Idempotent.
    pub fn stop(mut self) {
        self.cancel();
    }

    fn cancel(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.cancel();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_spinner_stop_is_harmless() {
        let spinner = Spinner::noop();
        spinner.stop(); // should not panic
    }

    #[test]
    fn spinner_cancelled_before_delay_produces_no_output() {
        // Use a very long delay so we can cancel before any frames render.
        let spinner = Spinner::spawn("Testing...".to_string(), Duration::from_secs(60));
        // Cancel immediately — the thread should exit without writing frames.
        spinner.stop();
    }

    #[test]
    fn spinner_drop_cleans_up() {
        let spinner = Spinner::spawn("Drop test".to_string(), Duration::from_millis(10));
        // Let a few frames render.
        thread::sleep(Duration::from_millis(100));
        drop(spinner); // should join cleanly
    }

    #[test]
    fn noop_when_not_tty() {
        // In a test environment stderr is typically not a TTY, so `new`
        // should return a no-op spinner. We verify the handle is None.
        let spinner = Spinner::new("should be noop", DEFAULT_DELAY);
        assert!(
            spinner.handle.is_none(),
            "Spinner::new should be no-op when stderr is not a TTY"
        );
        spinner.stop();
    }

    #[test]
    fn default_delay_is_500ms() {
        assert_eq!(DEFAULT_DELAY, Duration::from_millis(500));
    }
}
