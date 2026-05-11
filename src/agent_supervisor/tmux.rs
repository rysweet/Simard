//! Pure tmux command-line builder for wrapping engineer subprocesses (WS-2).

use std::collections::HashSet;
use std::path::Path;

use crate::agent_supervisor::types::SubordinateConfig;

/// POSIX shell single-quote escape: wrap the value in single quotes,
/// replacing any embedded `'` with the sequence `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Build the argv vector for launching `inner_argv` inside a detached tmux
/// session named `session_name`, redirecting stdout+stderr through `tee -a`
/// so `<log_path>` continues to receive the engineer log stream that the
/// dashboard `/ws/agent_log/{agent}` viewer tails.
///
/// `extra_env` injects environment variables into the new tmux session via
/// `tmux new-session -e KEY=VALUE` flags. This is REQUIRED because env vars
/// set on the spawning `Command` only reach the tmux client, not the new
/// session (the tmux server is typically a long-running daemon and forks
/// new sessions from its own environment, not the client's). Without this,
/// vars like `CARGO_TARGET_DIR` silently fail to propagate, causing each
/// engineer worktree to build its own ~12 GB cargo target dir.
///
/// Returned shape:
/// ```text
/// ["tmux", "new-session", "-d",
///  "-e", "K1=V1", "-e", "K2=V2", ...,
///  "-s", <session_name>,
///  "sh", "-c", "<shell-quoted inner argv> 2>&1 | tee -a <quoted log_path>"]
/// ```
pub fn build_tmux_wrapped_command(
    session_name: &str,
    inner_argv: &[String],
    log_path: &Path,
    extra_env: &[(String, String)],
) -> Vec<String> {
    let inner_quoted: Vec<String> = inner_argv.iter().map(|s| shell_single_quote(s)).collect();
    let log_quoted = shell_single_quote(&log_path.to_string_lossy());
    let shell_cmd = format!("{} 2>&1 | tee -a {}", inner_quoted.join(" "), log_quoted);

    let mut argv = vec![
        "tmux".to_string(),
        "new-session".to_string(),
        "-d".to_string(),
    ];
    for (k, v) in extra_env {
        argv.push("-e".to_string());
        argv.push(format!("{k}={v}"));
    }
    argv.extend([
        "-s".to_string(),
        session_name.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        shell_cmd,
    ]);
    argv
}

/// Build the `(KEY, VALUE)` pairs that must be passed to
/// `tmux new-session -e KEY=VAL` so the engineer subprocess inherits them.
///
/// Composition rules (kept stable so issue #1658 can regression-test this):
///
/// 1. Always-set vars seeded from `config`:
///    - `SIMARD_AGENT_NAME`        = `config.agent_name`
///    - `SIMARD_SUBORDINATE_DEPTH` = `config.current_depth + 1`
///    - `CARGO_BUILD_JOBS`         = `"4"` (issue #373 OOM guard)
/// 2. `CARGO_TARGET_DIR` honors a `parent_env` override; otherwise defaults
///    to `/tmp/simard-engineer-target` (issue #1197 — share one target dir
///    across all engineer worktrees).
/// 3. Every `SIMARD_*` entry from `parent_env` that isn't already in (1) is
///    appended, sorted by key for stable test/debug ordering.
///
/// The function is pure (it does not touch `std::env` itself), so unit tests
/// can drive it with synthetic parent environments and the integration test
/// `tests/engineer_supervisor_tmux_env.rs` can pin the propagation contract
/// across the real tmux boundary without mutating process-wide state.
pub fn compute_tmux_env<I>(config: &SubordinateConfig, parent_env: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut tmux_env: Vec<(String, String)> = vec![
        ("SIMARD_AGENT_NAME".to_string(), config.agent_name.clone()),
        (
            "SIMARD_SUBORDINATE_DEPTH".to_string(),
            (config.current_depth + 1).to_string(),
        ),
        ("CARGO_BUILD_JOBS".to_string(), "4".to_string()),
    ];

    let parent_pairs: Vec<(String, String)> = parent_env.into_iter().collect();

    let cargo_target = parent_pairs
        .iter()
        .find(|(k, _)| k == "CARGO_TARGET_DIR")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "/tmp/simard-engineer-target".to_string());
    tmux_env.push(("CARGO_TARGET_DIR".to_string(), cargo_target));

    // Forward every SIMARD_* var from parent_env that isn't already set.
    // Convention landed in PR #1661 / commit aca976ea: any SIMARD_* var
    // present in the daemon environment is propagated; vars seeded above
    // are skipped to avoid double-add.
    let already_set: HashSet<&str> = tmux_env.iter().map(|(k, _)| k.as_str()).collect();
    let mut simard_extras: Vec<(String, String)> = parent_pairs
        .into_iter()
        .filter(|(k, _)| k.starts_with("SIMARD_") && !already_set.contains(k.as_str()))
        .collect();
    simard_extras.sort_by(|a, b| a.0.cmp(&b.0));
    tmux_env.extend(simard_extras);

    tmux_env
}
