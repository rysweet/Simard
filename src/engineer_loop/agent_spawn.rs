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

/// Which amplihack agent subcommand to use for engineering work.
///
/// The pivot in PR #1652 hardcoded `RustyClawd` as the engineer subprocess.
/// In practice multiple amplihack autonomous agents are valid choices
/// (e.g. `amplihack copilot -p <prompt>`), and operators need to be able
/// to swap kinds without recompiling Simard.
///
/// Configure via the `SIMARD_ENGINEER_AGENT` env var. Recognised values
/// (case-insensitive): `rustyclawd` (default), `copilot`. Unknown values
/// fall back to the default with a stderr warning so operator typos do
/// not silently change behaviour.
///
/// Visibility is `pub` (rather than `pub(crate)`) so the integration test
/// `tests/engineer_copilot_permissions.rs` can drive `engineer_argv` /
/// `run_engineer_subprocess` end-to-end against a stub `amplihack` shim.
/// This is not part of the long-term stable API; treat it as test-visible
/// internal scaffolding.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    RustyClawd,
    Copilot,
}

impl AgentKind {
    /// Subcommand name as accepted by `amplihack <subcommand>`.
    #[doc(hidden)]
    pub fn subcommand(self) -> &'static str {
        match self {
            AgentKind::RustyClawd => "RustyClawd",
            AgentKind::Copilot => "copilot",
        }
    }

    /// Resolve the configured agent kind from the `SIMARD_ENGINEER_AGENT`
    /// environment variable. Unknown values warn to stderr and fall back
    /// to the default (`Copilot`).
    ///
    /// Default flipped from `RustyClawd` to `Copilot` on
    /// fix/forward-engineer-env-and-copilot-default: upstream RustyClawd
    /// crashes with `aclose(): asynchronous generator is already running`
    /// (rysweet/amplihack#4537), so every engineer dispatch fails closed
    /// with the previous default. Operators who still want RustyClawd
    /// can opt in by setting `SIMARD_ENGINEER_AGENT=rustyclawd`.
    pub(crate) fn from_env() -> AgentKind {
        match std::env::var("SIMARD_ENGINEER_AGENT") {
            Err(_) => AgentKind::Copilot,
            Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "" | "copilot" => AgentKind::Copilot,
                "rustyclawd" | "rusty-clawd" | "rusty_clawd" => AgentKind::RustyClawd,
                other => {
                    eprintln!(
                        "[simard] SIMARD_ENGINEER_AGENT={other:?} not recognised; \
                         falling back to Copilot. Valid: copilot, rustyclawd."
                    );
                    AgentKind::Copilot
                }
            },
        }
    }
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

    prompt.push_str("\n\n");
    prompt.push_str(QUALITY_STANDARDS_BLOCK);

    prompt
}

/// Merge-ready contract appended to every engineer brief.
///
/// This block is appended verbatim to every prompt produced by
/// [`build_agent_prompt`] so that engineers always see the six merge-ready
/// criteria and the forbidden-paths guardrail. The brain may add
/// task-specific instructions in the objective itself, but this block is
/// always present.
///
/// The ordered list mirrors the canonical six criteria documented in
/// `prompt_assets/simard/engineer_system.md` (the "Merge-Ready Contract"
/// section). When updating one, update the other.
pub(crate) const QUALITY_STANDARDS_BLOCK: &str = "## Quality Standards\n\
Every PR you open MUST satisfy the merge-ready criteria before you mark it \
ready for review or request merge.\n\
\n\
1. qa-team scenarios written, validated with `gadugi-test validate`, run with `gadugi-test run`.\n\
2. Docs updated for any user-facing surfaces OR explicit list of changed surfaces with internal-only justification.\n\
3. quality-audit completed >=3 SEEK→VALIDATE→FIX cycles, ended on a clean final cycle (zero critical/high; zero medium correctness/security findings).\n\
4. CI 100% green with 0 failures.\n\
5. PR description contains concrete evidence for criteria 1–4 and 6.\n\
6. Diff focused; no unrelated edits.\n\
\n\
Do NOT mark a PR ready for review or merge until merge-ready criteria are \
satisfied AND the PR description has been updated with evidence for criteria \
1–4 and 6.\n\
\n\
### Forbidden paths\n\
You may NEVER write to or modify any file under `~/.simard/prompt_assets/` \
or any path under `$SIMARD_PROMPT_ASSETS_DIR`. All prompt changes must be \
PRs to the Simard repository under `prompt_assets/`. The deployed prompts \
at `~/.simard/prompt_assets/` are derived from main; do not edit the \
deployed copy.\n";

/// Build the argv passed to `amplihack <subcommand>` for the chosen
/// [`AgentKind`]. Each kind has its own prompt-passing convention.
///
/// * `RustyClawd` uses `--auto -- -p <prompt>` (the `--` separator is
///   required so the inner `-p` reaches the autonomous loop).
/// * `copilot` accepts `-p <prompt>` directly with `--allow-all-paths`
///   so it can read/write across the workspace.
///
/// Visibility note: `pub` for the integration regression test
/// `tests/engineer_copilot_permissions.rs`. Treat as test-visible internal
/// scaffolding, not a stable API.
#[doc(hidden)]
pub fn engineer_argv(kind: AgentKind, prompt: &str, max_turns: u32) -> Vec<String> {
    match kind {
        AgentKind::RustyClawd => vec![
            kind.subcommand().to_string(),
            "--auto".to_string(),
            "--subprocess-safe".to_string(),
            "--no-reflection".to_string(),
            "--max-turns".to_string(),
            max_turns.to_string(),
            "--".to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
        AgentKind::Copilot => vec![
            kind.subcommand().to_string(),
            // Issue #1717: without --allow-all-tools the Copilot CLI's
            // tool allow-list defaults to interactive prompting, and a
            // headless engineer subprocess (no TTY) can only *read*: every
            // file write, `git commit`, `gh pr create`, `amplihack recipe
            // run`, etc. fail with "Permission denied and could not request
            // permission from user". Both permission flags MUST precede -p
            // so the Copilot CLI parser treats them as flags rather than
            // prompt content.
            "--allow-all-tools".to_string(),
            "--allow-all-paths".to_string(),
            "-p".to_string(),
            prompt.to_string(),
        ],
    }
}

/// Backwards-compatible wrapper kept so existing callers (and tests pinned
/// to the RustyClawd argv shape) keep working unchanged.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rustyclawd_argv(prompt: &str, max_turns: u32) -> Vec<String> {
    engineer_argv(AgentKind::RustyClawd, prompt, max_turns)
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

/// Run the `amplihack <agent>` subprocess and return its trailing output.
/// `kind` selects which agent subcommand to invoke; see [`AgentKind`].
///
/// Visibility note: `pub` for the integration regression test
/// `tests/engineer_copilot_permissions.rs`. Treat as test-visible internal
/// scaffolding, not a stable API.
#[doc(hidden)]
pub fn run_engineer_subprocess(
    prompt: &str,
    workspace: &Path,
    kind: AgentKind,
) -> SimardResult<String> {
    let bin = amplihack_binary();
    let argv = engineer_argv(kind, prompt, DEFAULT_MAX_TURNS);
    let action_label = format!("{bin} {}", kind.subcommand());

    let mut cmd = Command::new(&bin);
    cmd.args(&argv)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Belt-and-suspenders permission grant for the Copilot CLI subprocess.
    //
    // `--allow-all-tools` (in `engineer_argv`) is the primary mechanism for
    // auto-approving non-interactive write/git/gh tool calls. If a future
    // upstream Copilot CLI release renames or removes that flag, the
    // documented `COPILOT_ALLOW_ALL` env var keeps the engineer subprocess
    // from regressing back to the symptom motivating issue #1717 (every
    // engineer plan ending in a permission-denied table). Setting only for
    // the Copilot kind keeps the RustyClawd path byte-identical.
    if matches!(kind, AgentKind::Copilot) {
        cmd.env("COPILOT_ALLOW_ALL", "1");
    }

    let mut child = cmd
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
                "{} exited with status {}; stderr_tail=\n{}",
                kind.subcommand(),
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

/// Backwards-compatible wrapper that always uses [`AgentKind::RustyClawd`].
///
/// Older test code and any callers that pre-date the configurable agent
/// kind keep using this entrypoint unchanged. Production code paths read
/// the kind from the environment via [`AgentKind::from_env`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_rustyclawd_subprocess(prompt: &str, workspace: &Path) -> SimardResult<String> {
    run_engineer_subprocess(prompt, workspace, AgentKind::RustyClawd)
}

/// Start an agent session in a background thread and return the channel
/// receiver. The thread spawns the configured `amplihack <agent>` subprocess
/// (via [`AgentKind::from_env`]) and reports its summary back to the caller.
pub(crate) fn start_agent_session(
    prompt: String,
    workspace: PathBuf,
) -> mpsc::Receiver<SimardResult<String>> {
    let (tx, rx) = mpsc::channel();
    let kind = AgentKind::from_env();
    thread::spawn(move || {
        let result = run_engineer_subprocess(&prompt, &workspace, kind);
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
///
/// **Drain-aware**: refuses to dispatch when the safe-update orchestrator
/// has marked `~/.simard/state/draining.flag`. The brain wires this so a
/// safe-update in progress can quiesce in-flight work without racing
/// against new dispatches.
pub fn spawn_agent_for_goal(
    objective: &str,
    inspection: &RepoInspection,
    workspace_path: &Path,
) -> SimardResult<String> {
    let state_dir = crate::safe_update::default_state_dir();
    refuse_if_draining(&state_dir)?;
    let prompt = build_agent_prompt(objective, inspection);
    let rx = start_agent_session(prompt, workspace_path.to_path_buf());
    await_agent_session(rx)
}

/// Refuse a dispatch if the safe-update orchestrator has marked the
/// dispatch gate closed. Logs to stderr so operator-facing tools can see
/// the refusal.
pub(crate) fn refuse_if_draining(state_dir: &Path) -> SimardResult<()> {
    if crate::safe_update::is_draining(state_dir) {
        let flag = crate::safe_update::draining_flag_path(state_dir);
        eprintln!(
            "[engineer] dispatch refused: safe-update is draining (flag at {})",
            flag.display()
        );
        return Err(SimardError::BridgeCallFailed {
            bridge: "engineer".to_string(),
            method: "spawn_agent_for_goal".to_string(),
            reason: format!(
                "safe-update in progress: draining flag {} is present",
                flag.display()
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer_loop::types::RepoInspection;
    use serial_test::serial;

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

    #[test]
    fn agent_kind_subcommand_strings_are_stable() {
        assert_eq!(AgentKind::RustyClawd.subcommand(), "RustyClawd");
        assert_eq!(AgentKind::Copilot.subcommand(), "copilot");
    }

    #[test]
    fn agent_kind_from_env_defaults_to_copilot() {
        let original = std::env::var("SIMARD_ENGINEER_AGENT").ok();
        unsafe {
            std::env::remove_var("SIMARD_ENGINEER_AGENT");
        }
        assert_eq!(AgentKind::from_env(), AgentKind::Copilot);
        unsafe {
            if let Some(v) = original {
                std::env::set_var("SIMARD_ENGINEER_AGENT", v);
            }
        }
    }

    #[test]
    #[serial(simard_engineer_agent_env)]
    fn agent_kind_from_env_recognises_rustyclawd_explicitly() {
        let original = std::env::var("SIMARD_ENGINEER_AGENT").ok();
        for raw in [
            "rustyclawd",
            "RustyClawd",
            "RUSTYCLAWD",
            "rusty-clawd",
            "  rusty_clawd  ",
        ] {
            unsafe {
                std::env::set_var("SIMARD_ENGINEER_AGENT", raw);
            }
            assert_eq!(AgentKind::from_env(), AgentKind::RustyClawd, "raw={raw:?}");
        }
        unsafe {
            match original {
                Some(v) => std::env::set_var("SIMARD_ENGINEER_AGENT", v),
                None => std::env::remove_var("SIMARD_ENGINEER_AGENT"),
            }
        }
    }

    #[test]
    #[serial(simard_engineer_agent_env)]
    fn agent_kind_from_env_recognises_copilot_case_insensitive() {
        let original = std::env::var("SIMARD_ENGINEER_AGENT").ok();
        for raw in ["copilot", "Copilot", "COPILOT", "  copilot  "] {
            unsafe {
                std::env::set_var("SIMARD_ENGINEER_AGENT", raw);
            }
            assert_eq!(AgentKind::from_env(), AgentKind::Copilot, "raw={raw:?}");
        }
        unsafe {
            match original {
                Some(v) => std::env::set_var("SIMARD_ENGINEER_AGENT", v),
                None => std::env::remove_var("SIMARD_ENGINEER_AGENT"),
            }
        }
    }

    #[test]
    #[serial(simard_engineer_agent_env)]
    fn agent_kind_from_env_unknown_falls_back_to_default() {
        let original = std::env::var("SIMARD_ENGINEER_AGENT").ok();
        unsafe {
            std::env::set_var("SIMARD_ENGINEER_AGENT", "totally-not-a-real-agent");
        }
        // Falls back rather than panicking; warning is emitted to stderr.
        assert_eq!(AgentKind::from_env(), AgentKind::Copilot);
        unsafe {
            match original {
                Some(v) => std::env::set_var("SIMARD_ENGINEER_AGENT", v),
                None => std::env::remove_var("SIMARD_ENGINEER_AGENT"),
            }
        }
    }

    #[test]
    fn engineer_argv_copilot_uses_p_without_dash_separator() {
        let argv = engineer_argv(AgentKind::Copilot, "hello world", 7);
        assert_eq!(argv[0], "copilot");
        // The original (broken) contract only granted filesystem reads via
        // --allow-all-paths. Issue #1717 added --allow-all-tools so writes
        // (gh/git/file/amplihack) are also auto-approved in non-interactive
        // mode. Both must be present and both must come before -p so the
        // Copilot CLI parser sees them as flags rather than prompt content.
        assert!(
            argv.iter().any(|a| a == "--allow-all-tools"),
            "copilot argv must include --allow-all-tools so non-interactive \
             writes/git/gh/amplihack tools are auto-approved (issue #1717): {argv:?}"
        );
        assert!(argv.iter().any(|a| a == "--allow-all-paths"));
        assert!(argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "hello world"));
        // copilot does not use the `--` separator that RustyClawd needs.
        assert!(
            !argv.iter().any(|a| a == "--"),
            "copilot argv should not include `--` separator: {argv:?}"
        );
        // copilot is not driven by --auto / --max-turns; ensure those
        // RustyClawd-specific flags are absent so behaviour matches the
        // amplihack copilot subcommand surface.
        assert!(!argv.iter().any(|a| a == "--auto"));
        assert!(!argv.iter().any(|a| a == "--max-turns"));
    }

    /// Pin the exact positional ordering of the Copilot permission flags.
    ///
    /// The Copilot CLI's argument parser treats anything after `-p` as
    /// part of the prompt (or as a positional argument). If
    /// `--allow-all-tools` or `--allow-all-paths` ever lands *after* `-p`,
    /// the subprocess will silently regress to interactive prompting and
    /// fail closed in headless mode (the symptom that motivated #1717:
    /// engineer plans with empty PR pipelines).
    ///
    /// This test pins the canonical order:
    ///   `copilot --allow-all-tools --allow-all-paths -p <prompt>`
    #[test]
    fn engineer_argv_copilot_grants_tool_permissions_for_non_interactive() {
        let argv = engineer_argv(AgentKind::Copilot, "any prompt", 1);

        let tools_pos = argv.iter().position(|a| a == "--allow-all-tools").expect(
            "--allow-all-tools must be present in Copilot argv (issue #1717: \
                 non-interactive writes were failing closed without it)",
        );
        let paths_pos = argv
            .iter()
            .position(|a| a == "--allow-all-paths")
            .expect("--allow-all-paths must be present in Copilot argv");
        let p_pos = argv
            .iter()
            .position(|a| a == "-p")
            .expect("-p must be present in Copilot argv");

        assert!(
            tools_pos < paths_pos,
            "--allow-all-tools must precede --allow-all-paths: {argv:?}"
        );
        assert!(
            paths_pos < p_pos,
            "permission flags must precede -p so the parser treats them as \
             flags not prompt content: {argv:?}"
        );

        // The subcommand itself stays at index 0; permission flags follow.
        assert_eq!(argv[0], "copilot");
        assert_eq!(argv[1], "--allow-all-tools", "exact slot 1: {argv:?}");
        assert_eq!(argv[2], "--allow-all-paths", "exact slot 2: {argv:?}");
        assert_eq!(argv[3], "-p", "exact slot 3: {argv:?}");
        assert_eq!(argv[4], "any prompt", "exact slot 4: {argv:?}");
    }

    #[test]
    fn engineer_argv_rustyclawd_matches_legacy_wrapper() {
        let new = engineer_argv(AgentKind::RustyClawd, "x", 30);
        let legacy = rustyclawd_argv("x", 30);
        assert_eq!(new, legacy);
    }

    #[test]
    fn refuse_if_draining_returns_ok_when_flag_absent() {
        let dir = tempfile::tempdir().unwrap();
        // No draining.flag in this state_dir — dispatch is allowed.
        assert!(refuse_if_draining(dir.path()).is_ok());
    }

    #[test]
    fn refuse_if_draining_returns_bridge_error_when_flag_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("draining.flag"), b"").unwrap();
        let err = refuse_if_draining(dir.path()).unwrap_err();
        match err {
            SimardError::BridgeCallFailed { bridge, method, .. } => {
                assert_eq!(bridge, "engineer");
                assert_eq!(method, "spawn_agent_for_goal");
            }
            other => panic!("expected BridgeCallFailed, got {other:?}"),
        }
    }

    #[test]
    fn agent_brief_includes_quality_standards_block() {
        let prompt = build_agent_prompt("any objective", &fake_inspection());
        assert!(
            prompt.contains("## Quality Standards"),
            "brief must always include the '## Quality Standards' header; got:\n{prompt}"
        );
    }

    #[test]
    fn agent_brief_lists_six_merge_ready_criteria() {
        let prompt = build_agent_prompt("any objective", &fake_inspection());
        for marker in [
            "1. qa-team scenarios",
            "2. Docs updated",
            "3. quality-audit",
            "4. CI 100% green",
            "5. PR description contains concrete evidence",
            "6. Diff focused",
        ] {
            assert!(
                prompt.contains(marker),
                "merge-ready criterion missing from brief: {marker:?}\nfull brief:\n{prompt}"
            );
        }
    }

    #[test]
    fn agent_brief_contains_forbidden_paths_section() {
        let prompt = build_agent_prompt("any objective", &fake_inspection());
        assert!(
            prompt.contains("### Forbidden paths"),
            "brief must include the '### Forbidden paths' sub-section; got:\n{prompt}"
        );
        assert!(
            prompt.contains("~/.simard/prompt_assets/"),
            "brief must mention the deployed prompt assets path; got:\n{prompt}"
        );
        assert!(
            prompt.contains("SIMARD_PROMPT_ASSETS_DIR"),
            "brief must mention the env-var override; got:\n{prompt}"
        );
    }
}
