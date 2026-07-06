//! LaunchSession — bounded terminal session that dispatches through the
//! configured base type ([`LlmProvider`]).
//!
//! Per #1162: the launcher must consult [`LlmProvider::resolve`] (which
//! reads `SIMARD_LLM_PROVIDER` then `~/.simard/config.toml`, with no
//! silent default) and fail loud if the configured provider is one
//! the launcher cannot drive yet. Today only `Copilot` is wired —
//! `RustyClawd` returns an explicit "not implemented" error rather
//! than silently degrading to amplihack.

use std::path::Path;

use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::{ActionOutcome, PlannedAction};
use crate::session_builder::LlmProvider;

use super::make_outcome;

/// Launch a bounded terminal session to work on a specific task.
///
/// Routes through the configured base type:
/// - `LlmProvider::Copilot` → `amplihack copilot` via PTY, prompt piped on stdin
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

/// Copilot base-type implementation: shell out to `amplihack copilot` via a
/// PTY-wrapped bash session and capture the transcript.
///
/// The task prompt is written to a temp file in Rust and piped into copilot on
/// STDIN (`cat 'PATH' | amplihack copilot …`); it is never passed as an argv
/// token. The old `-p "$(cat "$F")"` form inlined the whole prompt into `argv`,
/// which for large goals exceeded `ARG_MAX` and made `exec` fail with `E2BIG`
/// ("Argument list too long", exit 126), breaking Simard's OODA loop (#2640).
fn dispatch_launch_session_copilot(action: &PlannedAction) -> ActionOutcome {
    use std::io::Write;

    use crate::terminal_session::PtyTerminalSession;

    let task = &action.description;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Write the task prompt to a temp file out-of-band (Rust-owned), so only the
    // path — never the prompt body — appears on the command line. The guard is
    // held until after `session.finish()`, then Drop unlinks the file.
    let mut prompt_file = match tempfile::Builder::new()
        .prefix("simard-ooda-prompt-")
        .tempfile()
    {
        Ok(file) => file,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!("failed to create OODA launch prompt temp file: {e}"),
            );
        }
    };
    if let Err(e) = prompt_file.write_all(task.as_bytes()) {
        return make_outcome(
            action,
            false,
            format!("failed to write OODA launch prompt temp file: {e}"),
        );
    }
    if let Err(e) = prompt_file.flush() {
        return make_outcome(
            action,
            false,
            format!("failed to flush OODA launch prompt temp file: {e}"),
        );
    }

    // Build the argv-free launch command (prompt piped on stdin). Fail loud if
    // the temp path is unexpectedly unsafe — no silent fallback.
    let command = match build_ooda_launch_command("amplihack copilot", prompt_file.path()) {
        Ok(command) => command,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!("failed to build OODA launch command: {e}"),
            );
        }
    };

    // Launch bash in a PTY — we'll send the copilot command ourselves.
    #[cfg(target_os = "macos")]
    const SHELL_PATH: &str = "/bin/bash";
    #[cfg(not(target_os = "macos"))]
    const SHELL_PATH: &str = "/usr/bin/bash";
    let mut session = match PtyTerminalSession::launch("terminal-shell", SHELL_PATH, &cwd) {
        Ok(s) => s,
        Err(e) => {
            return make_outcome(
                action,
                false,
                format!("failed to launch terminal session: {e}"),
            );
        }
    };

    if let Err(e) = session.send_input(&command) {
        let _ = session.finish();
        return make_outcome(
            action,
            false,
            format!("failed to send command to terminal: {e}"),
        );
    }

    // Wait for natural process exit — copilot runs to completion, then bash
    // exits via the chained `; exit`. finish() waits indefinitely for transcript
    // activity; if idle for 5 min, sends SIGTERM to hung wrapper.
    let outcome = match session.finish() {
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
    };

    // Keep the prompt file alive until copilot has finished reading it, then
    // unlink it explicitly (also happens on Drop).
    drop(prompt_file);
    outcome
}

/// Build the OODA launch-session shell command: pipe the prompt file into
/// copilot on STDIN (`cat 'PATH' | <program> --subprocess-safe --allow-all-tools
/// ; exit`) so the prompt is never an argv token — immune to the E2BIG /
/// "Argument list too long" failure that broke Simard's OODA loop (issue #2640).
///
/// Fails closed if `prompt_path` contains a single quote, which would break out
/// of the single-quoted `cat 'PATH'` context and allow shell injection: the
/// builder refuses rather than emitting an injectable command (no silent
/// fallback). `NamedTempFile` paths are safe ASCII, so this never fires in
/// production — it is a defense-in-depth guard on the contract.
pub fn build_ooda_launch_command(program: &str, prompt_path: &Path) -> SimardResult<String> {
    let path_str = prompt_path.to_string_lossy();
    if path_str.contains('\'') {
        return Err(SimardError::InvalidConfigValue {
            key: "ooda_launch_prompt_path".to_string(),
            value: path_str.to_string(),
            help: "the OODA launch prompt file path must not contain a single quote".to_string(),
        });
    }
    Ok(format!(
        "cat '{path_str}' | {program} --subprocess-safe --allow-all-tools ; exit"
    ))
}

#[cfg(test)]
mod tests {
    use crate::ooda_loop::{ActionKind, PlannedAction};

    #[test]
    #[ignore = "spawns real `amplihack copilot` subprocess; runs pip install and can take 30+ minutes — opt in with `cargo test -- --ignored`"]
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
    #[ignore = "spawns real `amplihack copilot` subprocess (see sibling); opt in with `cargo test -- --ignored`"]
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
    #[ignore = "spawns real `amplihack copilot` subprocess (see sibling); opt in with `cargo test -- --ignored`"]
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
    #[ignore = "spawns real `amplihack copilot` subprocess (see sibling); opt in with `cargo test -- --ignored`"]
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
    // #2360: process-global env mutation. cargo runs tests MULTI-threaded, so
    // this `set_var("SIMARD_LLM_PROVIDER")` can tear a concurrent reader (e.g.
    // operator_commands_dashboard::chat). Every cognitive-memory/provider env
    // reader and writer shares this key so mutation is never concurrent with a
    // read. See docs/testing/cognitive-memory-serial-isolation.md.
    #[serial_test::serial(cognitive_memory)]
    fn dispatch_launch_session_fails_loud_on_unsupported_rustyclawd_1162() {
        // The `serial(cognitive_memory)` key (not cargo single-threading)
        // guarantees no concurrent reader; the prior value is still restored
        // before the test exits.
        let prev = std::env::var("SIMARD_LLM_PROVIDER").ok();
        // SAFETY: serialised via `serial(cognitive_memory)`, so no other test
        // mutates or reads SIMARD_LLM_PROVIDER concurrently with this set_var.
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
