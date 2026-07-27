//! Pure tmux command-line builder for wrapping engineer subprocesses (WS-2).

use std::collections::HashSet;
use std::path::Path;

use crate::agent_supervisor::types::SubordinateConfig;
use crate::overseer::config;

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

/// Default root for per-worktree cargo target dirs when
/// `SIMARD_CARGO_TARGETS_ROOT` is unset (issue #4803). This is the
/// large-volume relocation target: the 28G `/` volume that holds `$HOME`
/// and `~/.simard` saturates under accumulated `target/debug` +
/// `target/llvm-cov-target` artifacts, so the default deliberately routes
/// build artifacts off `/` onto the roomier `/tmp` volume. Operators who
/// have a dedicated data volume can point `SIMARD_CARGO_TARGETS_ROOT` at it.
pub const DEFAULT_CARGO_TARGETS_ROOT_FALLBACK: &str = "/tmp/simard-cargo-targets";

/// Default subdirectory of `$HOME` that HISTORICALLY held the per-worktree
/// cargo targets root (pre-issue #4803). The resolver no longer routes here
/// — `$HOME` lives on the saturating `/` volume — but the name is retained
/// so the legacy `cap_home_cargo_targets` cleanup (`cmd_cleanup/disk.rs`)
/// can still LRU-rotate any artifacts left behind by older daemons.
pub const DEFAULT_CARGO_TARGETS_HOME_SUBDIR: &str = ".cargo-targets";

/// Compute the default `CARGO_TARGET_DIR` for an engineer worktree at
/// `worktree_path`. Pure — pulls `SIMARD_CARGO_TARGETS_ROOT` from
/// `parent_pairs` only (never `std::env`).
///
/// Resolution order (issue #4803 — the `$HOME` branch was removed):
/// 1. `<SIMARD_CARGO_TARGETS_ROOT>/<worktree_basename>` if the env var is set.
/// 2. `/tmp/simard-cargo-targets/<worktree_basename>` otherwise — the
///    large-volume default. The previous `<HOME>/.cargo-targets/...` default
///    piled build artifacts onto the 28G `/` volume and drove the ~25-min
///    emergency-cleanup crash-loop, so it is no longer used even when `HOME`
///    is set.
///
/// The basename is taken from `worktree_path.file_name()`. If the path has
/// no terminal component (extremely unlikely — would require `/`), the
/// literal string `"engineer-worktree"` is substituted so the resulting
/// path is still well-formed and per-engineer (the worktree path's full
/// hash gets folded in by callers via the directory layout, but for this
/// purely defensive branch we accept a shared fallback dir).
///
/// `pub(crate)` so the direct-exec spawn path (`lifecycle/spawn.rs`) shares
/// this single source of truth instead of hardcoding a divergent root.
pub(crate) fn default_cargo_target_for_worktree(
    worktree_path: &Path,
    parent_pairs: &[(String, String)],
) -> String {
    let basename = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "engineer-worktree".to_string());

    let lookup = |key: &str| -> Option<String> {
        parent_pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };

    // Issue #4803: the `$HOME` default branch is REMOVED. `HOME` lives on the
    // 28G `/` volume that saturates; routing per-worktree cargo target dirs
    // there piled `target/debug` + `target/llvm-cov-target` (0.7–2.6 GB each)
    // onto `/` and drove the ~25-min emergency-cleanup crash-loop. Absent an
    // explicit `SIMARD_CARGO_TARGETS_ROOT` override, the default now relocates
    // onto the large-volume fallback `/tmp/simard-cargo-targets` EVEN WHEN
    // HOME IS SET, so build artifacts stop refilling `/`.
    let root = lookup("SIMARD_CARGO_TARGETS_ROOT")
        .unwrap_or_else(|| DEFAULT_CARGO_TARGETS_ROOT_FALLBACK.to_string());

    format!("{root}/{basename}")
}

/// Build the `(KEY, VALUE)` pairs that must be passed to
/// `tmux new-session -e KEY=VAL` so the engineer subprocess inherits them.
///
/// Composition rules (kept stable so issue #1658 can regression-test this):
///
/// 1. Always-set vars seeded from `config`:
///    - `SIMARD_AGENT_NAME`        = `config.agent_name`
///    - `SIMARD_SUBORDINATE_DEPTH` = `config.current_depth + 1`
///    - `CARGO_BUILD_JOBS`         = `cargo_jobs_from(SIMARD_CARGO_JOBS)` (issues #373, #2199 OOM guard)
///    - `WORKFLOW_PR_LABELS`       = `simard-autonomous` (engineer-PR marker for
///      the amplihack publish step, #4097). Unlike the other seeds this is NOT a
///      `SIMARD_*` var, so it is not covered by rule (3)'s auto-forwarding — it
///      is seeded explicitly here because the direct-exec `Command::env()` in
///      spawn.rs never crosses the tmux boundary.
/// 2. `CARGO_TARGET_DIR` honors a `parent_env` override; otherwise defaults
///    to a **per-worktree** path so concurrent engineers never share one
///    cargo target dir (which would deadlock cargo's file lock or corrupt
///    incremental output). The default is
///    `<root>/<basename(config.worktree_path)>`, where `<root>` resolves
///    in this order (issue #4803 relocated the default off the `/` volume):
///     1. `SIMARD_CARGO_TARGETS_ROOT` env (operator override),
///     2. `/tmp/simard-cargo-targets` (large-volume default). The former
///        `<HOME>/.cargo-targets` default was removed: `$HOME` is on the 28G
///        `/` volume that saturates, so per-worktree target dirs there refilled
///        `/` within one build cycle and thrashed the emergency cleanup.
///
///    This intentionally REPLACES the previous shared
///    `/tmp/simard-engineer-target` default: that path caused 7-12 GB
///    target dirs to be created in every engineer worktree once
///    concurrent engineers deadlocked on the cargo build lock and fell
///    back to per-worktree builds (the disk-fill incident, issue #1697).
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
    let parent_pairs: Vec<(String, String)> = parent_env.into_iter().collect();

    // Resolve CARGO_BUILD_JOBS from SIMARD_CARGO_JOBS in parent env (issues #373, #2199).
    let simard_jobs_override = parent_pairs
        .iter()
        .find(|(k, _)| k == "SIMARD_CARGO_JOBS")
        .map(|(_, v)| v.as_str());
    let cargo_jobs = crate::cargo_jobs::cargo_jobs_from(simard_jobs_override);

    let mut tmux_env: Vec<(String, String)> = vec![
        ("SIMARD_AGENT_NAME".to_string(), config.agent_name.clone()),
        (
            "SIMARD_SUBORDINATE_DEPTH".to_string(),
            (config.current_depth + 1).to_string(),
        ),
        ("CARGO_BUILD_JOBS".to_string(), cargo_jobs),
        // Best-effort engineer-PR label for the amplihack publish step
        // (workflow_publish_pr.sh, amplihack-rs #979). Production engineers are
        // tmux-wrapped, and `WORKFLOW_PR_LABELS` is NOT a `SIMARD_*` var, so the
        // `Command::env()` set on the direct-exec path in spawn.rs never crosses
        // the tmux boundary — it MUST be seeded here explicitly or the label
        // silently no-ops for every real engineer PR (#4097). Keyed by the shared
        // constants so there is a single grep-able source of truth.
        (
            config::WORKFLOW_PR_LABELS_ENV.to_string(),
            config::SIMARD_ENGINEER_PR_LABEL.to_string(),
        ),
    ];

    let cargo_target = parent_pairs
        .iter()
        .find(|(k, _)| k == "CARGO_TARGET_DIR")
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_cargo_target_for_worktree(&config.worktree_path, &parent_pairs));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_roles::AgentRole;
    use crate::overseer::config;
    use std::path::PathBuf;

    fn make_config(name: &str) -> SubordinateConfig {
        SubordinateConfig {
            agent_name: name.to_string(),
            goal: "do the thing".to_string(),
            role: AgentRole::Engineer,
            worktree_path: PathBuf::from("/tmp/wt/engineer-x"),
            current_depth: 0,
        }
    }

    /// Seam (b): production engineers are tmux-wrapped, and `compute_tmux_env`
    /// only auto-forwards `SIMARD_*` vars from the parent env plus a fixed seed
    /// set. `WORKFLOW_PR_LABELS` is NOT a `SIMARD_*` var, so a `Command::env()`
    /// set in spawn.rs never reaches the tmux path. It MUST be seeded explicitly
    /// here or the label silently no-ops for every real engineer PR. This test
    /// pins that the seeded pair is always present, keyed by the shared
    /// constants (never a magic string) and valued at the engineer label.
    #[test]
    fn compute_tmux_env_seeds_workflow_pr_labels() {
        let cfg = make_config("engineer-1");
        // Empty parent env: the pair must be seeded unconditionally, not merely
        // forwarded from the parent.
        let env = compute_tmux_env(&cfg, std::iter::empty());

        let pair = env
            .iter()
            .find(|(k, _)| k == config::WORKFLOW_PR_LABELS_ENV)
            .expect("compute_tmux_env must seed WORKFLOW_PR_LABELS into the tmux -e vec");
        assert_eq!(
            pair.1,
            config::SIMARD_ENGINEER_PR_LABEL,
            "the seeded label must be the durable engineer-PR marker"
        );
    }

    /// The seeded key is the exact frozen wire name — guards against a silent
    /// constant-value rename breaking the shell contract with #979.
    #[test]
    fn compute_tmux_env_uses_frozen_wire_name() {
        let cfg = make_config("engineer-2");
        let env = compute_tmux_env(&cfg, std::iter::empty());
        assert!(
            env.iter()
                .any(|(k, v)| k == "WORKFLOW_PR_LABELS" && v == "simard-autonomous"),
            "expected literal WORKFLOW_PR_LABELS=simard-autonomous in the tmux env vec"
        );
    }
}
