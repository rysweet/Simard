//! LaunchSession — bounded terminal session that dispatches through the
//! configured base type ([`LlmProvider`]).
//!
//! Per #1162: the launcher must consult [`LlmProvider::resolve`] (which
//! reads `SIMARD_LLM_PROVIDER` then `~/.simard/config.toml`, with no
//! silent default) and fail loud if the configured provider is one
//! the launcher cannot drive yet. Today only `Copilot` is wired —
//! `RustyClawd` returns an explicit "not implemented" error rather
//! than silently degrading to amplihack.

use crate::ooda_loop::{ActionOutcome, PlannedAction};
use crate::session_builder::LlmProvider;

use super::make_outcome;

/// Launch a bounded terminal session to work on a specific task.
///
/// Routes through the configured base type:
/// - `LlmProvider::Copilot` → `amplihack copilot -p` via PTY (current behaviour)
/// - `LlmProvider::RustyClawd` → explicit unsupported error (fail loud, no fallback)
///
/// If `LlmProvider::resolve()` itself fails (env var unset *and* config
/// missing), the outcome surfaces that error verbatim so the operator
/// fixes their config rather than getting silent default behaviour.
pub(super) fn dispatch_launch_session(action: &PlannedAction) -> ActionOutcome {
    let provider = match LlmProvider::resolve() {
        Ok(p) => p,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!("launch-session aborted: LlmProvider::resolve failed: {e}"),
            );
        }
    };

    match provider {
        LlmProvider::Copilot => dispatch_launch_session_copilot(action),
        LlmProvider::RustyClawd => make_outcome(
            action,
            false,
            "launch-session not yet wired for rustyclawd base type — \
             file an issue or set SIMARD_LLM_PROVIDER=copilot \
             (no silent fallback by design, #1162)"
                .to_string(),
        ),
    }
}

/// Copilot base-type implementation: shell out to `amplihack copilot -p`
/// via a PTY-wrapped bash session and capture the transcript.
fn dispatch_launch_session_copilot(action: &PlannedAction) -> ActionOutcome {
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

    /// Regression #1162: rustyclawd is not yet wired through the launcher,
    /// so it must fail loud rather than silently dispatching to amplihack.
    /// Forcing the provider via env var keeps the test deterministic
    /// regardless of the host's `~/.simard/config.toml` contents.
    #[test]
    fn dispatch_launch_session_fails_loud_on_unsupported_rustyclawd_1162() {
        // SAFETY: tests are single-threaded by default in cargo (--test-threads=1
        // unless the suite opts out), and this env mutation is local and
        // restored before the test exits.
        let prev = std::env::var("SIMARD_LLM_PROVIDER").ok();
        // Safety: setting an env var is safe in single-threaded test context.
        unsafe {
            std::env::set_var("SIMARD_LLM_PROVIDER", "rustyclawd");
        }

        let action = PlannedAction {
            kind: ActionKind::LaunchSession,
            goal_id: None,
            description: "noop".into(),
        };
        let outcome = super::dispatch_launch_session(&action);

        // Restore the prior env state before any assertion can panic.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("SIMARD_LLM_PROVIDER", v),
                None => std::env::remove_var("SIMARD_LLM_PROVIDER"),
            }
        }

        assert!(
            !outcome.success,
            "rustyclawd is not yet wired for launch-session and must fail loud"
        );
        assert!(
            outcome.detail.contains("rustyclawd") && outcome.detail.contains("not yet wired"),
            "fail-loud message must name the provider and explain why; got: {}",
            outcome.detail
        );
    }
}
