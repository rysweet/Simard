//! spawn_subordinate extracted from lifecycle.rs (#1266).

use std::process::{Command, Stdio};

use crate::error::{SimardError, SimardResult};
use crate::subagent_sessions::session_name_for;

use super::{open_agent_log, query_pane_pid, supervisor_state_root};
use crate::agent_supervisor::tmux::build_tmux_wrapped_command;
use crate::agent_supervisor::types::{SubordinateConfig, SubordinateHandle};

/// Spawn a subordinate agent as a real child process.
///
/// Forks a new Simard process via `Command::new(current_exe())` in the
/// given worktree, passing `--agent-name`, `--goal`, and `--depth` as
/// arguments. The child process inherits the parent's environment.
///
/// stdout and stderr are redirected to
/// `<state_root>/agent_logs/<agent_name>.log` (append mode) so the
/// dashboard's `/ws/agent_log/{agent_name}` endpoint can tail the live
/// output. If the log file cannot be opened the spawn proceeds with
/// inherited stdio (fail-open, see `open_agent_log`).
///
/// The function validates the configuration (depth limits, non-empty
/// fields) before spawning.
#[tracing::instrument(skip_all, fields(identity = %config.agent_name))]
pub fn spawn_subordinate(config: &SubordinateConfig) -> SimardResult<SubordinateHandle> {
    config.validate()?;

    let now = super::current_epoch_seconds()?;

    let exe = std::env::current_exe().map_err(|e| SimardError::BridgeSpawnFailed {
        bridge: "subordinate".to_string(),
        reason: format!("cannot resolve current executable: {e}"),
    })?;

    let mut cmd = Command::new(&exe);
    cmd.arg("engineer")
        .arg("run")
        .arg("single-process")
        .arg(&config.worktree_path)
        .arg(&config.goal)
        .env("SIMARD_AGENT_NAME", &config.agent_name)
        .env(
            "SIMARD_SUBORDINATE_DEPTH",
            (config.current_depth + 1).to_string(),
        )
        // Limit concurrent cargo parallelism per agent to prevent OOM (issue #373).
        .env("CARGO_BUILD_JOBS", "4")
        .current_dir(&config.worktree_path);
    // Issue #1197: per-engineer git worktrees would otherwise force a
    // cold cargo rebuild (incl. lbug, ~40min) every spawn. Share one
    // target dir across all engineer worktrees, but respect any operator
    // override already in the environment.
    if std::env::var_os("CARGO_TARGET_DIR").is_none() {
        cmd.env("CARGO_TARGET_DIR", "/tmp/simard-engineer-target");
    }

    if let Some((out, err)) = open_agent_log(&config.agent_name) {
        cmd.stdout(out).stderr(err);
    }

    // --- WS-2: Wrap inner command in a detached tmux session when tmux is
    //     available, so the dashboard can offer `tmux attach` deep-links.
    //     If tmux is not on PATH, fall back to direct exec (preserves the
    //     pre-WS-2 behavior).
    let tmux_available = Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let session_name = session_name_for(&config.agent_name);
    let log_path = supervisor_state_root()
        .join("agent_logs")
        .join(format!("{}.log", config.agent_name));

    let (child_pid, applied_session_name) = if tmux_available {
        // Build the inner argv (must mirror the direct-exec path above).
        let inner_argv: Vec<String> = vec![
            exe.to_string_lossy().into_owned(),
            "engineer".to_string(),
            "run".to_string(),
            "single-process".to_string(),
            config.worktree_path.to_string_lossy().into_owned(),
            config.goal.clone(),
        ];
        // Env vars must be passed via `tmux new-session -e KEY=VAL`. Setting
        // them on `tmux_cmd` only reaches the tmux client; the long-running
        // tmux server forks new sessions from its own env. Without explicit
        // `-e`, vars like CARGO_TARGET_DIR silently fail to propagate and
        // each engineer worktree builds its own ~12 GB cargo target dir.
        let mut tmux_env: Vec<(String, String)> = vec![
            ("SIMARD_AGENT_NAME".to_string(), config.agent_name.clone()),
            (
                "SIMARD_SUBORDINATE_DEPTH".to_string(),
                (config.current_depth + 1).to_string(),
            ),
            ("CARGO_BUILD_JOBS".to_string(), "4".to_string()),
        ];
        if let Some(existing) = std::env::var_os("CARGO_TARGET_DIR") {
            tmux_env.push((
                "CARGO_TARGET_DIR".to_string(),
                existing.to_string_lossy().into_owned(),
            ));
        } else {
            tmux_env.push((
                "CARGO_TARGET_DIR".to_string(),
                "/tmp/simard-engineer-target".to_string(),
            ));
        }
        // Forward every SIMARD_* env var from the daemon's environment to the
        // engineer subprocess (issue #4537 / fix/forward-engineer-env-and-copilot-default).
        // Without this loop, `SIMARD_ENGINEER_AGENT=copilot` set on the systemd
        // unit reaches the daemon but is silently dropped at the tmux boundary
        // because the long-running tmux server forks new sessions from its own
        // environment, not from the tmux client's. The result: engineers
        // ignored the operator's agent override and fell back to the broken
        // upstream RustyClawd default. Convention: any SIMARD_* var present in
        // the daemon environment is propagated; vars already explicitly added
        // above (SIMARD_AGENT_NAME, SIMARD_SUBORDINATE_DEPTH) are skipped to
        // avoid double-add.
        let already_set: std::collections::HashSet<&str> = tmux_env
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut simard_extras: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("SIMARD_") && !already_set.contains(k.as_str()))
            .collect();
        // Stable ordering helps test/debug reproducibility.
        simard_extras.sort_by(|a, b| a.0.cmp(&b.0));
        tmux_env.extend(simard_extras);
        let argv = build_tmux_wrapped_command(&session_name, &inner_argv, &log_path, &tmux_env);

        // Run the tmux command. `tmux new-session -d` returns immediately
        // after the session is created; the inner shell runs detached inside.
        let mut tmux_cmd = Command::new(&argv[0]);
        tmux_cmd
            .args(&argv[1..])
            .current_dir(&config.worktree_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = tmux_cmd
            .status()
            .map_err(|e| SimardError::BridgeSpawnFailed {
                bridge: "subordinate".to_string(),
                reason: format!(
                    "failed to spawn tmux-wrapped subordinate '{}': {e}",
                    config.agent_name
                ),
            })?;

        if !status.success() {
            return Err(SimardError::BridgeSpawnFailed {
                bridge: "subordinate".to_string(),
                reason: format!(
                    "tmux new-session for subordinate '{}' exited with {status}",
                    config.agent_name
                ),
            });
        }

        // Query the engineer pid via the pane's pane_pid. Brief retry to
        // allow the shell to fork its child.
        let pid = query_pane_pid(&session_name).unwrap_or(0);
        (pid, session_name.clone())
    } else {
        tracing::warn!(
            target: "simard::supervisor",
            agent = %config.agent_name,
            "tmux not available; spawning subordinate directly (no attach support)",
        );
        let child = cmd.spawn().map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "subordinate".to_string(),
            reason: format!(
                "failed to spawn subordinate '{}' at '{}': {e}",
                config.agent_name,
                exe.display()
            ),
        })?;
        (child.id(), String::new())
    };

    Ok(SubordinateHandle {
        pid: child_pid,
        agent_name: config.agent_name.clone(),
        goal: config.goal.clone(),
        worktree_path: config.worktree_path.clone(),
        spawn_time: now,
        retry_count: 0,
        killed: false,
        session_name: applied_session_name,
    })
}
