//! Integration test: assert that env vars set on the engineer supervisor's
//! parent environment actually propagate across the tmux boundary into the
//! engineer subprocess.
//!
//! Closes issue #1658. Pinned regression coverage for the fix landed in
//! PR #1661 / commit aca976ea ("fix(engineer): forward SIMARD_* env to
//! engineer subprocess and default to Copilot"):
//!
//! > ... env vars set on the spawning Command don't propagate to tmux
//! > sessions when a tmux server already exists. The fix is `-e KEY=VAL`
//! > arguments to `tmux new-session`. Without this loop,
//! > `SIMARD_ENGINEER_AGENT=copilot` set on the systemd unit reaches the
//! > daemon but is silently dropped at the tmux boundary because the
//! > long-running tmux server forks new sessions from its own environment,
//! > not from the tmux client's.
//!
//! Strategy: drive `compute_tmux_env` + `build_tmux_wrapped_command` end to
//! end with a real `tmux` server. Use `printenv` as the inner "engineer"
//! command and read back the wrapper's tee'd log file. If `printenv` shows
//! the sentinel value, the env reached the inner subprocess across the tmux
//! boundary; if it doesn't, the propagation regressed.
//!
//! Skipped when `tmux` is not on PATH so CI environments without tmux do
//! not break.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use simard::agent_roles::AgentRole;
use simard::agent_supervisor::SubordinateConfig;
use simard::agent_supervisor::tmux::{build_tmux_wrapped_command, compute_tmux_env};

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{pid}-{nanos}")
}

fn make_engineer_config(name: &str) -> SubordinateConfig {
    SubordinateConfig {
        agent_name: name.to_string(),
        goal: "tmux-env-propagation-integration-test".to_string(),
        role: AgentRole::Engineer,
        worktree_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        current_depth: 0,
    }
}

/// Wait up to `deadline` for `path` to exist and contain non-empty content.
fn wait_for_log_content(path: &std::path::Path, deadline: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Ok(s) = std::fs::read_to_string(path)
            && !s.is_empty()
        {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::read_to_string(path).ok()
}

/// Best-effort cleanup: kill the tmux session and remove the log file.
fn cleanup(session_name: &str, log_path: &std::path::Path) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = std::fs::remove_file(log_path);
}

/// End-to-end propagation contract for issue #1658.
///
/// Builds the tmux env vec from a synthetic parent environment containing a
/// sentinel `SIMARD_*` value, then spawns a real detached tmux session whose
/// inner command is `printenv SIMARD_TEST_TMUX_PROPAGATE`. The wrapper pipes
/// inner stdout through `tee -a <log>`, so the log file ends up containing
/// whatever value `printenv` saw in its own environment.
///
/// If the `-e KEY=VAL` propagation regresses (e.g. a future refactor drops
/// `compute_tmux_env`'s SIMARD_* loop), `printenv` will print an empty line
/// and this test fails.
#[test]
fn simard_env_var_propagates_through_tmux_to_engineer_subprocess() {
    if !tmux_available() {
        eprintln!(
            "[skip] tmux is not on PATH; cannot exercise the engineer-supervisor \
             tmux env propagation path. Install tmux or run this test in a \
             tmux-capable environment to enable issue #1658 regression coverage."
        );
        return;
    }

    let suffix = unique_suffix();
    let sentinel = format!("propagated-{suffix}");
    let session_name = format!("simard-itest-tmuxenv-{suffix}");
    let log_path = std::env::temp_dir().join(format!("simard-itest-tmuxenv-{suffix}.log"));
    // Pre-clean any stale log/session from a previous interrupted run.
    let _ = std::fs::remove_file(&log_path);

    let config = make_engineer_config("engineer-itest-tmuxenv");

    // Synthetic parent environment carrying the sentinel — note we do NOT
    // touch std::env so the test is safe under cargo test's parallel runner.
    let synthetic_parent_env = vec![
        ("SIMARD_TEST_TMUX_PROPAGATE".to_string(), sentinel.clone()),
        // Confirm a non-SIMARD var is NOT propagated (whitelist guarantee).
        (
            "ITEST_NON_SIMARD_VAR".to_string(),
            "should-not-propagate".to_string(),
        ),
    ];

    let tmux_env = compute_tmux_env(&config, synthetic_parent_env);
    assert!(
        tmux_env
            .iter()
            .any(|(k, v)| k == "SIMARD_TEST_TMUX_PROPAGATE" && v == &sentinel),
        "Pre-condition: compute_tmux_env must include the sentinel SIMARD_* var. \
         Got: {tmux_env:?}"
    );
    assert!(
        !tmux_env.iter().any(|(k, _)| k == "ITEST_NON_SIMARD_VAR"),
        "Pre-condition: compute_tmux_env must NOT propagate non-SIMARD_ vars. \
         Got: {tmux_env:?}"
    );

    // Inner argv = `printenv SIMARD_TEST_TMUX_PROPAGATE`. The wrapper's
    // `tee -a` will dump printenv's stdout into log_path so we can read it.
    let inner_argv = vec![
        "printenv".to_string(),
        "SIMARD_TEST_TMUX_PROPAGATE".to_string(),
    ];

    let argv = build_tmux_wrapped_command(&session_name, &inner_argv, &log_path, &tmux_env);

    // Run the tmux command. Critically: do NOT set the sentinel on tmux_cmd
    // itself — the whole point of issue #1658 is that vars on the tmux
    // client are silently dropped. The only mechanism under test is the
    // `-e KEY=VAL` propagation that compute_tmux_env wires up.
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("tmux new-session should launch");
    assert!(
        status.success(),
        "tmux new-session exited with {status}; argv was {argv:?}"
    );

    // Give the inner shell up to 10s to run `printenv` and tee its output.
    let log_contents = wait_for_log_content(&log_path, Duration::from_secs(10)).unwrap_or_default();

    cleanup(&session_name, &log_path);

    assert!(
        log_contents.contains(&sentinel),
        "engineer subprocess did not see SIMARD_TEST_TMUX_PROPAGATE={sentinel} \
         after tmux propagation. This is the issue #1658 regression — env \
         vars set on the tmux client without `-e KEY=VAL` are silently \
         dropped by the long-running tmux server. log_contents={log_contents:?}"
    );
}

/// Negative control: when the SIMARD_* var is NOT included in the parent
/// env passed to `compute_tmux_env`, it must NOT appear in the inner
/// engineer subprocess. This pins the contract that propagation is opt-in
/// via `compute_tmux_env`, not implicit through tmux server inheritance —
/// which is the very behaviour issue #1658 needs to guard against.
#[test]
fn missing_simard_env_var_does_not_propagate_implicitly_through_tmux() {
    if !tmux_available() {
        eprintln!(
            "[skip] tmux is not on PATH; cannot exercise negative-control case \
             for issue #1658 propagation."
        );
        return;
    }

    let suffix = unique_suffix();
    let session_name = format!("simard-itest-tmuxenv-neg-{suffix}");
    let log_path = std::env::temp_dir().join(format!("simard-itest-tmuxenv-neg-{suffix}.log"));
    let _ = std::fs::remove_file(&log_path);

    let config = make_engineer_config("engineer-itest-tmuxenv-neg");
    // Empty parent env — no SIMARD_TEST_TMUX_PROPAGATE present.
    let tmux_env = compute_tmux_env(&config, std::iter::empty::<(String, String)>());
    assert!(
        !tmux_env
            .iter()
            .any(|(k, _)| k == "SIMARD_TEST_TMUX_PROPAGATE"),
        "Pre-condition: compute_tmux_env with empty parent must not include \
         SIMARD_TEST_TMUX_PROPAGATE. Got: {tmux_env:?}"
    );

    // Inner: print SIMARD_TEST_TMUX_PROPAGATE if present, else a sentinel.
    let inner_argv = vec![
        "sh".to_string(),
        "-c".to_string(),
        "printenv SIMARD_TEST_TMUX_PROPAGATE || echo NOT_SET".to_string(),
    ];

    let argv = build_tmux_wrapped_command(&session_name, &inner_argv, &log_path, &tmux_env);

    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("tmux new-session should launch");
    assert!(status.success(), "tmux new-session exited with {status}");

    let log_contents = wait_for_log_content(&log_path, Duration::from_secs(10)).unwrap_or_default();

    cleanup(&session_name, &log_path);

    assert!(
        log_contents.contains("NOT_SET"),
        "negative control: with no -e flag the inner subprocess should not \
         see the var (printenv exits non-zero, then `|| echo NOT_SET` runs). \
         log_contents={log_contents:?}"
    );
}
