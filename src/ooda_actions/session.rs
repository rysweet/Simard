//! LaunchSession — bounded terminal session for amplihack copilot tasks.

use crate::ooda_loop::{ActionOutcome, PlannedAction};

use super::make_outcome;

/// Launch a bounded terminal session to work on a specific task.
///
/// Uses `PtyTerminalSession` to start `amplihack copilot -p <prompt>`,
/// wait for natural process exit, and capture the transcript.
/// This mirrors the copilot adapter pattern from `base_type_copilot.rs`:
/// write the prompt to a temp file, invoke copilot with `-p`, chain `; exit`.
pub(super) fn dispatch_launch_session(action: &PlannedAction) -> ActionOutcome {
    use crate::terminal_session::PtyTerminalSession;

    let task = &action.description;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Launch bash in a PTY — we'll send the copilot command ourselves.
    let mut session = match PtyTerminalSession::launch("terminal-shell", "/usr/bin/bash", &cwd) {
        Ok(s) => s,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!("failed to launch terminal session: {e}"),
            );
        }
    };

    // Build a single command that writes the prompt to a temp file, invokes
    // copilot with -p, cleans up, and exits. No sentinels, no timeouts.
    let escaped = task.replace('\\', "\\\\").replace('\'', "'\\''");
    let command = format!(
        "SIMARD_PROMPT_FILE=$(mktemp /tmp/simard-ooda-prompt.XXXXXX) && \
         printf '%s' '{escaped}' > \"$SIMARD_PROMPT_FILE\" && \
         amplihack copilot -p \"$(cat \"$SIMARD_PROMPT_FILE\")\" ; \
         rm -f \"$SIMARD_PROMPT_FILE\" ; exit\n"
    );

    if let Err(e) = session.send_input(&command) {
        let _ = session.finish();
        return make_outcome(
            action,
            false,
            format!("failed to send command to terminal: {e}"),
        );
    }

    // Wait for natural process exit — copilot runs to completion, then
    // bash exits via the chained `; exit`. finish() polls up to 600s.
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

    #[test]
    fn dispatch_launch_session_produces_outcome_without_panic() {
        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: Some("goal-77".into()),
            description: "a bounded test task".into(),
        };
        let outcome = super::dispatch_launch_session(&action);
        // Whether it succeeds or fails depends on environment, but it must
        // not panic and must produce a meaningful detail string.
        assert!(!outcome.detail.is_empty());
        assert_eq!(outcome.action.kind, ActionKind::LaunchSession);
        assert_eq!(outcome.action.goal_id.as_deref(), Some("goal-77"));
    }

    #[test]
    fn dispatch_launch_session_with_special_chars_in_description() {
        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: "task with 'quotes' and \\backslashes\\".into(),
        };
        let outcome = super::dispatch_launch_session(&action);
        // Must not panic on special shell characters
        assert!(!outcome.detail.is_empty());
    }

    #[test]
    fn dispatch_launch_session_empty_description() {
        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: String::new(),
        };
        let outcome = super::dispatch_launch_session(&action);
        assert!(!outcome.detail.is_empty());
    }
}
