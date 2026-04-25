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
