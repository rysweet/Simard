//! Engineer-loop agent spawn — thin subprocess delegation to `amplihack RustyClawd`.
//!
//! Architectural pivot (issue #1648 / per @rysweet): the engineer loop must NOT
//! be custom in-process LLM orchestration. It must be a single subprocess
//! invocation of the upstream `amplihack RustyClawd --auto` autonomous engineer
//! and consume its summary output. Simard's role is to act as a PM architect
//! orchestrating fleets of coding agents — not to reimplement the agent loop.
//!
//! Benefits of the subprocess model:
//!   * SIGTERM to Simard cleanly orphans the child to init; the daemon can
//!     respond to shutdown without waiting on internal LLM SDK state.
//!   * The agent loop logic, retries, tool selection, and reflection all live
//!     in `amplihack` (a single source of truth) instead of being duplicated
//!     in a bespoke Rust state machine.
//!   * The amplihack binary can be upgraded independently of Simard.
//!
//! Override binary path with `SIMARD_AMPLIHACK_BIN` (used by tests and
//! environments where `amplihack` is not on PATH).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};

use super::types::RepoInspection;

pub(crate) const AGENT_SESSION_TIMEOUT_SECS: u64 = 3600;

/// Default upper bound on RustyClawd autonomous turns. Aligns with the
/// `amplihack RustyClawd --auto --max-turns` complex-task guidance.
pub(crate) const DEFAULT_MAX_TURNS: u32 = 30;

/// Maximum number of summary bytes returned to callers. The full subprocess
/// stdout/stderr are streamed to Simard's own stdout/stderr (via inherit) for
/// operator visibility; only this trailing window is captured for the
/// in-process summary string used by `run_optional_review` and persistence.
pub(crate) const SUMMARY_TAIL_BYTES: usize = 8 * 1024;

/// Resolve the amplihack binary. Defaults to `amplihack` (PATH lookup); can be
/// overridden with `SIMARD_AMPLIHACK_BIN`.
pub(crate) fn amplihack_binary() -> String {
    std::env::var("SIMARD_AMPLIHACK_BIN").unwrap_or_else(|_| "amplihack".to_string())
}

pub(crate) fn build_agent_prompt(objective: &str, inspection: &RepoInspection) -> String {
    let files = if inspection.changed_files.is_empty() {
        "none".to_string()
    } else {
        inspection.changed_files.join(", ")
    };
    let dirty = if inspection.worktree_dirty {
        "dirty"
    } else {
        "clean"
    };
    let goals: Vec<&str> = inspection
        .active_goals
        .iter()
        .map(|g| g.title.as_str())
        .collect();
    let goals_list = if goals.is_empty() {
        "none".to_string()
    } else {
        goals.join("; ")
    };

    let mut prompt = format!(
        "You are an autonomous software engineer working on a git repository.\n\
         Use your tools to implement the following objective completely and correctly.\n\
         When done, summarize what you changed.\n\n\
         Objective: {objective}\n\
         Branch: {branch}\n\
         HEAD: {head}\n\
         Worktree: {dirty}\n\
         Changed files: {files}\n\
         Active goals: {goals_list}",
        objective = objective,
        branch = inspection.branch,
        head = inspection.head,
    );

    if !inspection.architecture_gap_summary.is_empty() {
        prompt.push_str("\n\nArchitecture notes: ");
        prompt.push_str(&inspection.architecture_gap_summary);
    }

    prompt
}

/// Build the argv passed to `amplihack RustyClawd --auto`. Exposed for tests.
pub(crate) fn rustyclawd_argv(prompt: &str, max_turns: u32) -> Vec<String> {
    vec![
        "RustyClawd".to_string(),
        "--auto".to_string(),
        "--subprocess-safe".to_string(),
        "--no-reflection".to_string(),
        "--max-turns".to_string(),
        max_turns.to_string(),
        "--".to_string(),
        "-p".to_string(),
        prompt.to_string(),
    ]
}

fn keep_summary_tail(buf: &[u8]) -> String {
    let len = buf.len();
    let start = len.saturating_sub(SUMMARY_TAIL_BYTES);
    let slice = &buf[start..];
    let mut s = String::from_utf8_lossy(slice).into_owned();
    if start > 0 {
        s.insert_str(
            0,
            &format!("[truncated {start} earlier bytes; tail follows]\n\n"),
        );
    }
    s
}

/// Run the `amplihack RustyClawd` subprocess and return its trailing output.
pub(crate) fn run_rustyclawd_subprocess(prompt: &str, workspace: &Path) -> SimardResult<String> {
    let bin = amplihack_binary();
    let argv = rustyclawd_argv(prompt, DEFAULT_MAX_TURNS);
    let action_label = format!("{bin} RustyClawd --auto");

    let mut child = Command::new(&bin)
        .args(&argv)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: action_label.clone(),
            reason: format!("failed to spawn `{bin}`: {e}"),
        })?;

    let deadline = Instant::now() + Duration::from_secs(AGENT_SESSION_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SimardError::CommandTimeout {
                        action: action_label,
                        timeout_secs: AGENT_SESSION_TIMEOUT_SECS,
                    });
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                return Err(SimardError::ActionExecutionFailed {
                    action: action_label,
                    reason: format!("failed to poll child process: {e}"),
                });
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: action_label.clone(),
            reason: format!("failed to collect child output: {e}"),
        })?;

    let stdout_tail = keep_summary_tail(&output.stdout);
    let stderr_tail = keep_summary_tail(&output.stderr);

    if !output.status.success() {
        return Err(SimardError::ActionExecutionFailed {
            action: action_label,
            reason: format!(
                "RustyClawd exited with status {}; stderr_tail=\n{}",
                output.status,
                stderr_tail.trim()
            ),
        });
    }

    let summary = if stdout_tail.trim().is_empty() {
        stderr_tail
    } else {
        stdout_tail
    };
    Ok(summary)
}

/// Start an agent session in a background thread and return the channel
/// receiver. The thread spawns `amplihack RustyClawd --auto` as a subprocess
/// and reports its summary back to the caller.
pub(crate) fn start_agent_session(
    prompt: String,
    workspace: PathBuf,
) -> mpsc::Receiver<SimardResult<String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = run_rustyclawd_subprocess(&prompt, &workspace);
        let _ = tx.send(result);
    });
    rx
}

/// Wait for a running agent session to complete and return the execution
/// summary (the trailing window of the subprocess's combined output).
pub(crate) fn await_agent_session(
    rx: mpsc::Receiver<SimardResult<String>>,
) -> SimardResult<String> {
    rx.recv_timeout(Duration::from_secs(AGENT_SESSION_TIMEOUT_SECS + 30))
        .map_err(|_| SimardError::ActionExecutionFailed {
            action: "agent-spawn".to_string(),
            reason: format!("agent session channel timed out after {AGENT_SESSION_TIMEOUT_SECS}s"),
        })?
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: "agent-spawn".to_string(),
            reason: format!("agent session failed: {e}"),
        })
}

/// Spawn an autonomous agent session to accomplish `objective`.
///
/// This delegates fully to `amplihack RustyClawd --auto`: Simard does not
/// implement its own LLM loop, tool selection, or reflection. The summary
/// returned is the trailing window of the subprocess's stdout/stderr.
pub fn spawn_agent_for_goal(
    objective: &str,
    inspection: &RepoInspection,
    workspace_path: &Path,
) -> SimardResult<String> {
    let prompt = build_agent_prompt(objective, inspection);
    let rx = start_agent_session(prompt, workspace_path.to_path_buf());
    await_agent_session(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer_loop::types::RepoInspection;

    fn fake_inspection() -> RepoInspection {
        RepoInspection {
            workspace_root: "/tmp".into(),
            repo_root: "/tmp".into(),
            branch: "main".into(),
            head: "abc".into(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        }
    }

    #[test]
    fn build_agent_prompt_includes_objective() {
        let prompt = build_agent_prompt("fix the bug", &fake_inspection());
        assert!(prompt.contains("fix the bug"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn build_agent_prompt_lists_changed_files() {
        let mut inspection = fake_inspection();
        inspection.worktree_dirty = true;
        inspection.changed_files = vec!["src/lib.rs".to_string()];
        let prompt = build_agent_prompt("add tests", &inspection);
        assert!(prompt.contains("src/lib.rs"));
        assert!(prompt.contains("dirty"));
    }

    #[test]
    fn build_agent_prompt_includes_architecture_gap_summary_when_set() {
        let mut inspection = fake_inspection();
        inspection.architecture_gap_summary = "session_builder.rs exceeds 400 lines".to_string();
        let prompt = build_agent_prompt("improve quality", &inspection);
        assert!(prompt.contains("Architecture notes:"));
        assert!(prompt.contains("session_builder.rs exceeds 400 lines"));
    }

    #[test]
    fn build_agent_prompt_omits_architecture_notes_when_empty() {
        let prompt = build_agent_prompt("improve quality", &fake_inspection());
        assert!(!prompt.contains("Architecture notes:"));
    }

    #[test]
    fn rustyclawd_argv_includes_required_flags() {
        let argv = rustyclawd_argv("hello", 7);
        assert_eq!(argv[0], "RustyClawd");
        assert!(argv.iter().any(|a| a == "--auto"));
        assert!(argv.iter().any(|a| a == "--subprocess-safe"));
        assert!(argv.iter().any(|a| a == "--max-turns"));
        assert!(argv.iter().any(|a| a == "7"));
        assert!(argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "hello"));
        let dash_pos = argv.iter().position(|a| a == "--").expect("`--` separator");
        let p_pos = argv.iter().position(|a| a == "-p").expect("`-p` flag");
        assert!(
            dash_pos < p_pos,
            "`--` must precede inner `-p` flag: {argv:?}"
        );
    }

    #[test]
    fn amplihack_binary_respects_env_override() {
        // Use a sentinel value that is not a real binary; verify the function
        // honours the override regardless of whether the file exists.
        let original = std::env::var("SIMARD_AMPLIHACK_BIN").ok();
        // SAFETY: this test runs in a single-thread per test runner; env var
        // mutation is bounded by the cleanup below.
        unsafe {
            std::env::set_var("SIMARD_AMPLIHACK_BIN", "/nonexistent/test-amplihack");
        }
        assert_eq!(amplihack_binary(), "/nonexistent/test-amplihack");
        // restore
        unsafe {
            match original {
                Some(v) => std::env::set_var("SIMARD_AMPLIHACK_BIN", v),
                None => std::env::remove_var("SIMARD_AMPLIHACK_BIN"),
            }
        }
    }

    #[test]
    fn keep_summary_tail_truncates_large_buffers() {
        let big = vec![b'x'; SUMMARY_TAIL_BYTES + 1024];
        let s = keep_summary_tail(&big);
        assert!(s.starts_with("[truncated"));
        assert!(s.len() <= SUMMARY_TAIL_BYTES + 200);
    }

    #[test]
    fn keep_summary_tail_passes_small_buffers_through() {
        let s = keep_summary_tail(b"small message\n");
        assert_eq!(s, "small message\n");
    }

    #[test]
    fn run_rustyclawd_subprocess_propagates_spawn_failure() {
        // Override binary to a path that does not exist; spawn must fail with
        // ActionExecutionFailed (not panic, not block).
        let original = std::env::var("SIMARD_AMPLIHACK_BIN").ok();
        unsafe {
            std::env::set_var(
                "SIMARD_AMPLIHACK_BIN",
                "/nonexistent/definitely-not-a-binary",
            );
        }
        let result = run_rustyclawd_subprocess("hi", Path::new("/tmp"));
        unsafe {
            match original {
                Some(v) => std::env::set_var("SIMARD_AMPLIHACK_BIN", v),
                None => std::env::remove_var("SIMARD_AMPLIHACK_BIN"),
            }
        }
        assert!(result.is_err(), "expected spawn failure for fake binary");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed to spawn") || err.contains("RustyClawd"),
            "unexpected error message: {err}"
        );
    }
}
