//! spawn_subordinate extracted from lifecycle.rs (#1266).

use std::process::{Command, Stdio};

use crate::error::{SimardError, SimardResult};
use crate::subagent_sessions::session_name_for;

use super::{open_agent_log, query_pane_pid, supervisor_state_root};
use crate::agent_supervisor::tmux::{build_tmux_wrapped_command, compute_tmux_env};
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

    let exe = std::env::current_exe().map_err(|e| SimardError::RpcSpawnFailed {
        endpoint: "subordinate".to_string(),
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
        // Limit concurrent cargo parallelism per agent to prevent OOM (issues #373, #2199).
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs())
        // Best-effort engineer-PR label for the amplihack publish step
        // (workflow_publish_pr.sh, amplihack-rs #979). Stamps this engineer's
        // PRs with the durable `simard-autonomous` marker so the self-merge
        // queue can tell them apart from the operator's own review PRs (#4097).
        // Inert until the consumer lands. The production tmux path seeds this
        // separately in `compute_tmux_env` (it is not a SIMARD_* var and so is
        // not auto-forwarded across the tmux boundary).
        .env(
            crate::overseer::config::WORKFLOW_PR_LABELS_ENV,
            crate::overseer::config::SIMARD_ENGINEER_PR_LABEL,
        )
        .current_dir(&config.worktree_path);
    // Issue #1197 / #4803: per-engineer git worktrees would otherwise force a
    // cold cargo rebuild (incl. lbug, ~40min) every spawn. Give each worktree
    // its own target dir, but delegate to the SINGLE per-worktree resolver the
    // tmux path uses (`compute_tmux_env` → `default_cargo_target_for_worktree`)
    // so both spawn paths agree on an off-`/` default (#4803: no longer the
    // shared `/tmp/simard-engineer-target`, which deadlocked cargo's build lock
    // across concurrent engineers). Respect any operator override already set.
    if std::env::var_os("CARGO_TARGET_DIR").is_none() {
        let parent_pairs: Vec<(String, String)> = std::env::vars().collect();
        let target = crate::agent_supervisor::tmux::default_cargo_target_for_worktree(
            &config.worktree_path,
            &parent_pairs,
        );
        cmd.env("CARGO_TARGET_DIR", target);
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
        // each engineer worktree builds its own ~12 GB cargo target dir
        // (issue #1197), and operator-set `SIMARD_ENGINEER_AGENT=copilot`
        // never reaches the engineer (issue #1658 / PR #1661).
        let tmux_env = compute_tmux_env(config, std::env::vars());
        let argv = build_tmux_wrapped_command(&session_name, &inner_argv, &log_path, &tmux_env);

        // Run the tmux command. `tmux new-session -d` returns immediately
        // after the session is created; the inner shell runs detached inside.
        let mut tmux_cmd = Command::new(&argv[0]);
        tmux_cmd
            .args(&argv[1..])
            .current_dir(&config.worktree_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = tmux_cmd.status().map_err(|e| SimardError::RpcSpawnFailed {
            endpoint: "subordinate".to_string(),
            reason: format!(
                "failed to spawn tmux-wrapped subordinate '{}': {e}",
                config.agent_name
            ),
        })?;

        if !status.success() {
            return Err(SimardError::RpcSpawnFailed {
                endpoint: "subordinate".to_string(),
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
        let child = cmd.spawn().map_err(|e| SimardError::RpcSpawnFailed {
            endpoint: "subordinate".to_string(),
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
