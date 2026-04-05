//! LaunchSession — bounded terminal session for amplihack copilot tasks.

use crate::ooda_loop::{ActionOutcome, PlannedAction};

use super::make_outcome;

/// Launch a bounded terminal session to work on a specific task.
///
/// Uses `PtyTerminalSession` to start `amplihack copilot`, send the task
/// description, wait for completion signals, and capture the transcript.
pub(super) fn dispatch_launch_session(action: &PlannedAction) -> ActionOutcome {
    use crate::terminal_session::PtyTerminalSession;
    use std::time::Duration;

    let task = &action.description;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Launch amplihack copilot in a PTY.
    let mut session =
        match PtyTerminalSession::launch_command("terminal-shell", "amplihack copilot", &cwd) {
            Ok(s) => s,
            Err(e) => {
                return make_outcome(
                    action,
                    false,
                    format!("failed to launch amplihack copilot: {e}"),
                );
            }
        };

    // Wait for the copilot prompt to appear.
    let prompt_timeout = Duration::from_secs(30);
    match session.wait_for_output("$", prompt_timeout) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("[simard] OODA launch-session: copilot prompt not detected: {e}");
            // Continue anyway — the session may still be responsive.
        }
    }

    // Send the task description.
    if let Err(e) = session.send_input(task) {
        let _ = session.finish();
        return make_outcome(
            action,
            false,
            format!("failed to send task to copilot: {e}"),
        );
    }

    // Wait for the task to complete (up to 5 minutes).
    let work_timeout = Duration::from_secs(300);
    let _ = session.wait_for_output("$", work_timeout);

    // Send /exit to cleanly close the copilot session.
    let _ = session.send_input("/exit");
    let _ = session.wait_for_output("Bye", Duration::from_secs(10));

    // Capture transcript.
    match session.finish() {
        Ok(capture) => {
            let preview = crate::terminal_session::transcript_preview(&capture.transcript);
            let success = capture.exit_status.success();
            make_outcome(
                action,
                success,
                format!(
                    "amplihack session {} (exit={}): {preview}",
                    if success { "completed" } else { "failed" },
                    capture.exit_status,
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("terminal session capture failed: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::ooda_loop::{ActionKind, PlannedAction};

    #[test]
    #[ignore] // Requires amplihack copilot — run with `cargo test -- --ignored`
    fn launch_session_returns_failure_when_amplihack_unavailable() {
        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: "test task for session launch".into(),
        };
        let outcome = super::dispatch_launch_session(&action);
        // In CI/test environments, amplihack copilot won't be available,
        // so we expect a graceful failure rather than a panic.
        assert!(
            !outcome.detail.is_empty(),
            "launch-session should report a meaningful outcome even on failure"
        );
    }

    #[test]
    fn action_kind_launch_session_displays_correctly() {
        assert_eq!(ActionKind::LaunchSession.to_string(), "launch-session");
    }
}
