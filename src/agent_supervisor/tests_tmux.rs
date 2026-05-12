//! TDD tests for the tmux command wrapper.
//!
//! Specifies the contract for `build_tmux_wrapped_command`: it must produce
//! a `tmux new-session -d -s <name> sh -c '<quoted inner> 2>&1 | tee -a <log>'`
//! invocation so existing log-tailing dashboard endpoints keep working.

use std::path::PathBuf;

use super::tmux::build_tmux_wrapped_command;

#[test]
fn produces_expected_argv_prefix() {
    let argv = build_tmux_wrapped_command(
        "simard-engineer-engineer-abc",
        &[
            "/usr/bin/simard".to_string(),
            "engineer".to_string(),
            "run".to_string(),
        ],
        &PathBuf::from("/tmp/agent_logs/engineer-abc.log"),
        &[],
    );
    assert_eq!(argv[0], "tmux", "argv[0] must be tmux");
    assert_eq!(argv[1], "new-session");
    assert_eq!(argv[2], "-d");
    assert_eq!(argv[3], "-s");
    assert_eq!(argv[4], "simard-engineer-engineer-abc");
    assert_eq!(argv[5], "sh");
    assert_eq!(argv[6], "-c");
    assert_eq!(argv.len(), 8, "must be exactly 8 argv entries");
}

#[test]
fn shell_command_pipes_through_tee_to_log() {
    let argv = build_tmux_wrapped_command(
        "simard-engineer-x",
        &["/bin/echo".to_string(), "hi".to_string()],
        &PathBuf::from("/tmp/agent_logs/x.log"),
        &[],
    );
    let shell = &argv[7];
    assert!(
        shell.contains("2>&1 | tee -a"),
        "shell command must redirect stderr→stdout and tee to log: {shell}"
    );
    assert!(
        shell.contains("/tmp/agent_logs/x.log"),
        "shell command must reference the log path: {shell}"
    );
}

#[test]
fn shell_command_includes_inner_argv() {
    let argv = build_tmux_wrapped_command(
        "simard-engineer-y",
        &[
            "/usr/bin/simard".to_string(),
            "engineer".to_string(),
            "run".to_string(),
            "single-process".to_string(),
        ],
        &PathBuf::from("/tmp/y.log"),
        &[],
    );
    let shell = &argv[7];
    assert!(shell.contains("simard"), "must contain inner exe: {shell}");
    assert!(
        shell.contains("engineer"),
        "must contain 'engineer' arg: {shell}"
    );
    assert!(
        shell.contains("single-process"),
        "must contain subcommand: {shell}"
    );
}

#[test]
fn shell_command_quotes_arguments_safely() {
    // Ensure that paths containing spaces or shell metacharacters survive
    // composition (defensive: paths under /home/Some User/...).
    let argv = build_tmux_wrapped_command(
        "simard-engineer-z",
        &[
            "/usr/bin/simard".to_string(),
            "engineer".to_string(),
            "/path with spaces/wt".to_string(),
            "implement \"feature\"".to_string(),
        ],
        &PathBuf::from("/tmp/z.log"),
        &[],
    );
    let shell = &argv[7];
    // The quoted form must NOT allow the spaces to split arguments. We
    // require either single-quoting or backslash escaping somewhere in the
    // assembled shell string.
    assert!(
        shell.contains('\'') || shell.contains('\\'),
        "inner argv must be shell-quoted/escaped: {shell}"
    );
}

#[test]
fn session_name_is_passed_verbatim_in_dash_s_slot() {
    // The builder is pure — caller is responsible for sanitizing. The
    // builder must NOT silently rewrite the session name.
    let argv = build_tmux_wrapped_command(
        "simard-engineer-custom-id",
        &["/bin/true".to_string()],
        &PathBuf::from("/tmp/t.log"),
        &[],
    );
    assert_eq!(argv[4], "simard-engineer-custom-id");
}

#[test]
fn extra_env_emits_dash_e_flags_before_dash_s() {
    // Regression: env vars set on the spawning Command don't propagate to
    // tmux sessions when a tmux server already exists. The fix is `-e KEY=VAL`
    // arguments to `tmux new-session`. Builder must emit them BEFORE `-s`.
    let argv = build_tmux_wrapped_command(
        "simard-engineer-env",
        &["/bin/true".to_string()],
        &PathBuf::from("/tmp/env.log"),
        &[
            ("CARGO_TARGET_DIR".to_string(), "/tmp/shared".to_string()),
            ("SIMARD_AGENT_NAME".to_string(), "eng-1".to_string()),
        ],
    );
    let s_pos = argv.iter().position(|a| a == "-s").expect("must have -s");
    let e_positions: Vec<_> = argv
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "-e")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(e_positions.len(), 2, "must emit one -e per env var");
    for &p in &e_positions {
        assert!(p < s_pos, "-e at {p} must come before -s at {s_pos}");
    }
    let env_values: Vec<&String> = e_positions.iter().map(|&p| &argv[p + 1]).collect();
    assert!(
        env_values.contains(&&"CARGO_TARGET_DIR=/tmp/shared".to_string()),
        "must include CARGO_TARGET_DIR=...: {env_values:?}"
    );
    assert!(
        env_values.contains(&&"SIMARD_AGENT_NAME=eng-1".to_string()),
        "must include SIMARD_AGENT_NAME=...: {env_values:?}"
    );
}

#[test]
fn empty_extra_env_emits_no_dash_e_flags() {
    let argv = build_tmux_wrapped_command(
        "s",
        &["/bin/true".to_string()],
        &PathBuf::from("/tmp/n.log"),
        &[],
    );
    assert!(
        !argv.iter().any(|a| a == "-e"),
        "empty extra_env must not emit any -e flags: {argv:?}"
    );
}

// ---------------------------------------------------------------------------
// compute_tmux_env unit tests (issue #1658).
//
// These pin the contract that PR #1661 / commit aca976ea established: every
// SIMARD_* var present in the daemon's environment must be forwarded to the
// engineer subprocess via `tmux new-session -e KEY=VAL`. A future refactor
// that drops the SIMARD_* propagation loop would otherwise silently regress
// the operator-set `SIMARD_ENGINEER_AGENT=copilot` override.
// ---------------------------------------------------------------------------

use super::tmux::compute_tmux_env;
use crate::agent_roles::AgentRole;
use crate::agent_supervisor::types::SubordinateConfig;

fn make_test_config(name: &str, depth: u32) -> SubordinateConfig {
    SubordinateConfig {
        agent_name: name.to_string(),
        goal: "test goal".to_string(),
        role: AgentRole::Engineer,
        worktree_path: PathBuf::from("/fake/worktree"),
        current_depth: depth,
    }
}

fn env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn compute_tmux_env_seeds_required_simard_vars_from_config() {
    let config = make_test_config("engineer-abc", 2);
    let env = compute_tmux_env(&config, std::iter::empty::<(String, String)>());

    assert_eq!(
        env_value(&env, "SIMARD_AGENT_NAME"),
        Some("engineer-abc"),
        "SIMARD_AGENT_NAME must come from config.agent_name"
    );
    assert_eq!(
        env_value(&env, "SIMARD_SUBORDINATE_DEPTH"),
        Some("3"),
        "SIMARD_SUBORDINATE_DEPTH must be config.current_depth + 1"
    );
    assert_eq!(
        env_value(&env, "CARGO_BUILD_JOBS"),
        Some("4"),
        "CARGO_BUILD_JOBS must be capped at 4 (issue #373 OOM guard)"
    );
}

#[test]
fn compute_tmux_env_uses_per_worktree_default_when_parent_unset() {
    // No HOME, no SIMARD_CARGO_TARGETS_ROOT, no CARGO_TARGET_DIR — must
    // fall back to /tmp/simard-cargo-targets/<basename>. The basename is
    // taken from config.worktree_path so concurrent engineers never share
    // one cargo target dir.
    let config = make_test_config("e1", 0);
    let env = compute_tmux_env(&config, std::iter::empty::<(String, String)>());
    assert_eq!(
        env_value(&env, "CARGO_TARGET_DIR"),
        Some("/tmp/simard-cargo-targets/worktree"),
        "fallback default must be /tmp/simard-cargo-targets/<basename>"
    );
}

#[test]
fn compute_tmux_env_default_uses_home_when_present() {
    // Production case: the OODA daemon inherits HOME from the operator
    // shell. Default must be <HOME>/.cargo-targets/<basename>.
    let config = make_test_config("e1", 0);
    let parent = vec![("HOME".to_string(), "/home/azureuser".to_string())];
    let env = compute_tmux_env(&config, parent);
    assert_eq!(
        env_value(&env, "CARGO_TARGET_DIR"),
        Some("/home/azureuser/.cargo-targets/worktree"),
        "default must be <HOME>/.cargo-targets/<basename>"
    );
}

#[test]
fn compute_tmux_env_default_honors_simard_cargo_targets_root_override() {
    // Operators can pin a custom root via SIMARD_CARGO_TARGETS_ROOT —
    // useful for routing cargo target dirs onto a separate, larger
    // filesystem (e.g. ephemeral SSD).
    let config = make_test_config("e1", 0);
    let parent = vec![
        ("HOME".to_string(), "/home/azureuser".to_string()),
        (
            "SIMARD_CARGO_TARGETS_ROOT".to_string(),
            "/srv/cargo-targets".to_string(),
        ),
    ];
    let env = compute_tmux_env(&config, parent);
    assert_eq!(
        env_value(&env, "CARGO_TARGET_DIR"),
        Some("/srv/cargo-targets/worktree"),
        "SIMARD_CARGO_TARGETS_ROOT must override the HOME-derived default"
    );
}

#[test]
fn compute_tmux_env_default_is_per_worktree_basename() {
    // Two configs with different worktree paths must produce two distinct
    // CARGO_TARGET_DIR values. This is the property that prevents the
    // disk-fill incident's "concurrent cargo builds collide on shared
    // target dir" failure mode.
    let mut a = make_test_config("e1", 0);
    a.worktree_path = PathBuf::from("/tmp/wt-a-12345");
    let mut b = make_test_config("e2", 0);
    b.worktree_path = PathBuf::from("/tmp/wt-b-67890");

    let parent = || vec![("HOME".to_string(), "/h".to_string())];
    let env_a = compute_tmux_env(&a, parent());
    let env_b = compute_tmux_env(&b, parent());

    let target_a = env_value(&env_a, "CARGO_TARGET_DIR").expect("a has target");
    let target_b = env_value(&env_b, "CARGO_TARGET_DIR").expect("b has target");
    assert_ne!(
        target_a, target_b,
        "different worktree basenames must yield different CARGO_TARGET_DIR (got {target_a} vs {target_b})"
    );
    assert!(
        target_a.ends_with("/wt-a-12345"),
        "target_a must end with worktree basename: {target_a}"
    );
    assert!(
        target_b.ends_with("/wt-b-67890"),
        "target_b must end with worktree basename: {target_b}"
    );
}

#[test]
fn compute_tmux_env_default_falls_back_when_home_empty() {
    // Defensive: an explicitly-empty HOME must not produce
    // `<empty>/.cargo-targets/<basename>` (which would be a relative path
    // and thus depend on CWD). Treat empty HOME as missing.
    let config = make_test_config("e1", 0);
    let parent = vec![("HOME".to_string(), String::new())];
    let env = compute_tmux_env(&config, parent);
    assert_eq!(
        env_value(&env, "CARGO_TARGET_DIR"),
        Some("/tmp/simard-cargo-targets/worktree"),
        "empty HOME must be treated as missing and fall back to /tmp"
    );
}

#[test]
fn compute_tmux_env_uses_default_cargo_target_when_parent_unset() {
    // Backwards-compatible alias for the original test name. Some external
    // grep-based tooling looks for this exact identifier; keep it pointing
    // at the new contract so that tooling does not silently miss the
    // regression target.
    let config = make_test_config("e1", 0);
    let env = compute_tmux_env(&config, std::iter::empty::<(String, String)>());
    assert!(
        env_value(&env, "CARGO_TARGET_DIR")
            .map(|v| v.starts_with("/tmp/simard-cargo-targets/"))
            .unwrap_or(false),
        "fallback default must live under /tmp/simard-cargo-targets/ (issue #1697)"
    );
}

#[test]
fn compute_tmux_env_honors_parent_cargo_target_override() {
    let config = make_test_config("e1", 0);
    let parent = vec![(
        "CARGO_TARGET_DIR".to_string(),
        "/srv/shared-target".to_string(),
    )];
    let env = compute_tmux_env(&config, parent);
    assert_eq!(
        env_value(&env, "CARGO_TARGET_DIR"),
        Some("/srv/shared-target"),
        "CARGO_TARGET_DIR from parent_env must override the default"
    );
}

#[test]
fn compute_tmux_env_forwards_simard_vars_from_parent_env() {
    // Regression target: PR #1661 added a loop that forwards every SIMARD_*
    // parent env var. If a future refactor drops this loop, the operator's
    // SIMARD_ENGINEER_AGENT=copilot override silently fails to reach the
    // engineer and engineers fall back to the broken default agent.
    let config = make_test_config("e1", 0);
    let parent = vec![
        ("SIMARD_ENGINEER_AGENT".to_string(), "copilot".to_string()),
        ("SIMARD_KEEP_TRANSCRIPTS".to_string(), "1".to_string()),
        ("SIMARD_LLM_PROVIDER".to_string(), "openai".to_string()),
        // Non-SIMARD vars must NOT be forwarded.
        ("HOME".to_string(), "/home/azureuser".to_string()),
        ("PATH".to_string(), "/usr/bin".to_string()),
    ];
    let env = compute_tmux_env(&config, parent);

    assert_eq!(env_value(&env, "SIMARD_ENGINEER_AGENT"), Some("copilot"));
    assert_eq!(env_value(&env, "SIMARD_KEEP_TRANSCRIPTS"), Some("1"));
    assert_eq!(env_value(&env, "SIMARD_LLM_PROVIDER"), Some("openai"));
    assert!(
        env_value(&env, "HOME").is_none(),
        "non-SIMARD_ vars must not leak into tmux_env: {env:?}"
    );
    assert!(
        env_value(&env, "PATH").is_none(),
        "non-SIMARD_ vars must not leak into tmux_env: {env:?}"
    );
}

#[test]
fn compute_tmux_env_does_not_double_add_seeded_vars() {
    // SIMARD_AGENT_NAME and SIMARD_SUBORDINATE_DEPTH are seeded from config.
    // Even if they appear in parent_env, the seeded values must win and the
    // key must appear exactly once.
    let config = make_test_config("from-config", 5);
    let parent = vec![
        (
            "SIMARD_AGENT_NAME".to_string(),
            "from-parent-env".to_string(),
        ),
        ("SIMARD_SUBORDINATE_DEPTH".to_string(), "999".to_string()),
        ("SIMARD_OTHER".to_string(), "ok".to_string()),
    ];
    let env = compute_tmux_env(&config, parent);

    let agent_name_count = env.iter().filter(|(k, _)| k == "SIMARD_AGENT_NAME").count();
    let depth_count = env
        .iter()
        .filter(|(k, _)| k == "SIMARD_SUBORDINATE_DEPTH")
        .count();
    assert_eq!(
        agent_name_count, 1,
        "SIMARD_AGENT_NAME must be unique: {env:?}"
    );
    assert_eq!(
        depth_count, 1,
        "SIMARD_SUBORDINATE_DEPTH must be unique: {env:?}"
    );
    assert_eq!(
        env_value(&env, "SIMARD_AGENT_NAME"),
        Some("from-config"),
        "config-seeded SIMARD_AGENT_NAME must win over parent_env"
    );
    assert_eq!(
        env_value(&env, "SIMARD_SUBORDINATE_DEPTH"),
        Some("6"),
        "config-derived SIMARD_SUBORDINATE_DEPTH (depth+1) must win over parent_env"
    );
    assert_eq!(env_value(&env, "SIMARD_OTHER"), Some("ok"));
}

#[test]
fn compute_tmux_env_sorts_simard_extras_for_stable_ordering() {
    let config = make_test_config("e1", 0);
    let parent = vec![
        ("SIMARD_ZULU".to_string(), "z".to_string()),
        ("SIMARD_ALPHA".to_string(), "a".to_string()),
        ("SIMARD_MIKE".to_string(), "m".to_string()),
    ];
    let env = compute_tmux_env(&config, parent);

    // Find the positions of the SIMARD_ extras (they come after the seeded vars).
    let positions: Vec<(String, usize)> = env
        .iter()
        .enumerate()
        .filter_map(|(i, (k, _))| {
            if matches!(k.as_str(), "SIMARD_ALPHA" | "SIMARD_MIKE" | "SIMARD_ZULU") {
                Some((k.clone(), i))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(positions.len(), 3, "all three extras must appear: {env:?}");
    let alpha_idx = positions
        .iter()
        .find(|(k, _)| k == "SIMARD_ALPHA")
        .unwrap()
        .1;
    let mike_idx = positions
        .iter()
        .find(|(k, _)| k == "SIMARD_MIKE")
        .unwrap()
        .1;
    let zulu_idx = positions
        .iter()
        .find(|(k, _)| k == "SIMARD_ZULU")
        .unwrap()
        .1;
    assert!(
        alpha_idx < mike_idx && mike_idx < zulu_idx,
        "SIMARD_ extras must be sorted alphabetically: alpha={alpha_idx} mike={mike_idx} zulu={zulu_idx}"
    );
}

#[test]
fn compute_tmux_env_output_threads_through_build_tmux_wrapped_command() {
    // Round-trip: compute_tmux_env's output must be accepted by
    // build_tmux_wrapped_command and produce the corresponding `-e KEY=VAL`
    // flags on the resulting tmux argv.
    let config = make_test_config("engineer-x", 0);
    let parent = vec![("SIMARD_ENGINEER_AGENT".to_string(), "copilot".to_string())];
    let tmux_env = compute_tmux_env(&config, parent);
    let argv = build_tmux_wrapped_command(
        "simard-engineer-x",
        &["/bin/printenv".to_string()],
        &PathBuf::from("/tmp/x.log"),
        &tmux_env,
    );

    let env_flags: Vec<&String> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            if a == "-e" && i + 1 < argv.len() {
                Some(&argv[i + 1])
            } else {
                None
            }
        })
        .collect();

    assert!(
        env_flags
            .iter()
            .any(|s| s.as_str() == "SIMARD_ENGINEER_AGENT=copilot"),
        "tmux argv must carry SIMARD_ENGINEER_AGENT=copilot via -e: {env_flags:?}"
    );
    assert!(
        env_flags
            .iter()
            .any(|s| s.as_str() == "SIMARD_AGENT_NAME=engineer-x"),
        "tmux argv must carry seeded SIMARD_AGENT_NAME via -e: {env_flags:?}"
    );
}
