//! Integration tests for rysweet/Simard#1909 — read subcommands must
//! hard-fail when the positional `<state-root>` argument is absent. They
//! must **never** silently fall back to a synthesized default path nor
//! honor the `SIMARD_STATE_ROOT` environment variable.
//!
//! These tests define the contract for the resolver-layer guard
//! `require_explicit_state_root_for_read` (see
//! `docs/reference/operator-read-state-root-contract.md`). They are TDD
//! tests: they FAIL against `main` (which silently synthesizes a default
//! probe state root for these three read subcommands) and PASS once the
//! guard is wired into the three read wrappers.
//!
//! Acceptance criteria mapping (from Step 2c / A1–A10):
//! - A1: no implicit fallback (neither synthesized default nor env var)
//! - A2: positional `<state-root>` is the equivalent of `--state-root`
//! - A3: review read uses its own helper (RUN path untouched)
//! - A6: unified error wording across the three subcommands
//! - A7: error variant is `SimardError::MissingRequiredConfig`
//! - A8: seven tests, names mirroring the contract page table

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serial_test::serial;

// ---------------------------------------------------------------------
// Local helpers (duplicated rather than imported so the file is
// self-contained; matches the conventions used by other per-issue
// integration test files like `bin_simard_operator_probe_cli.rs`).
// ---------------------------------------------------------------------

fn rendered_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

fn simard_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simard"))
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{label}-{unique}"));
        fs::create_dir_all(&path).expect("temp dir should be created");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct CleanupDirGuard {
    path: PathBuf,
}

impl CleanupDirGuard {
    fn new(path: PathBuf) -> Self {
        let _ = fs::remove_dir_all(&path);
        Self { path }
    }
}

impl Drop for CleanupDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn default_meeting_state_root(base_type: &str, topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("meeting-run")
        .join("simard-meeting")
        .join(base_type)
        .join(topology)
}

fn default_review_state_root(base_type: &str, topology: &str) -> PathBuf {
    repo_root()
        .join("target/operator-probe-state")
        .join("review-run")
        .join("simard-engineer")
        .join(base_type)
        .join(topology)
}

// Per-default-root mutexes — heavyweight `_succeeds_*` tests poke at
// well-known paths shared with the existing simard_cli.rs sibling tests.
// Serializing here prevents cross-test races when `cargo test` schedules
// our `#[ignore]`d tests alongside those.
fn meeting_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn review_default_root_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Unified error-wording assertion shared by every hard-fail test in
/// this file. Mirrors the contract page (A6 + A7):
///   `missing required configuration 'state-root': state-root is required
///    for `simard <subcommand> read <base-type>`: pass the positional
///    <state-root> argument explicitly. The SIMARD_STATE_ROOT environment
///    variable is not honored for this command.`
fn assert_hard_fail_message(rendered: &str, subcommand: &str, base_type: &str) {
    assert!(
        rendered.contains("missing required configuration 'state-root'"),
        "expected unified `SimardError::MissingRequiredConfig` wording with \
         key='state-root', got:\n{rendered}"
    );
    assert!(
        rendered.contains("state-root is required"),
        "expected `state-root is required` phrase, got:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("simard {subcommand} read {base_type}")),
        "expected error to name `simard {subcommand} read {base_type}`, got:\n{rendered}"
    );
    assert!(
        rendered.contains("pass the positional <state-root> argument explicitly"),
        "expected explicit-argument guidance, got:\n{rendered}"
    );
    assert!(
        rendered.contains("SIMARD_STATE_ROOT environment variable is not honored"),
        "expected explicit env-var disclaimer, got:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 1. meeting_read_hard_fails_without_state_root
// ---------------------------------------------------------------------

#[test]
fn meeting_read_hard_fails_without_state_root() {
    let output = simard_bin()
        .arg("meeting")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect("simard meeting read should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "meeting read must hard-fail when no positional <state-root> is given:\n{rendered}"
    );
    assert_hard_fail_message(&rendered, "meeting", "local-harness");

    // Negative guard: the resolver must NOT silently synthesize a probe
    // default path or otherwise leak a "State root:" trace before failing.
    assert!(
        !rendered.contains("Probe mode: meeting-read"),
        "meeting read must not begin probe execution before failing on \
         missing state-root:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 2. improvement_curation_read_hard_fails_without_state_root
// ---------------------------------------------------------------------

#[test]
fn improvement_curation_read_hard_fails_without_state_root() {
    let output = simard_bin()
        .arg("improvement-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect("simard improvement-curation read should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "improvement-curation read must hard-fail when no positional \
         <state-root> is given:\n{rendered}"
    );
    assert_hard_fail_message(&rendered, "improvement-curation", "local-harness");

    assert!(
        !rendered.contains("Probe mode: improvement-curation-read"),
        "improvement-curation read must not begin probe execution before \
         failing on missing state-root:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 3. review_read_hard_fails_without_state_root
// ---------------------------------------------------------------------

#[test]
fn review_read_hard_fails_without_state_root() {
    let output = simard_bin()
        .arg("review")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .output()
        .expect("simard review read should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "review read must hard-fail when no positional <state-root> is \
         given:\n{rendered}"
    );
    assert_hard_fail_message(&rendered, "review", "local-harness");

    assert!(
        !rendered.contains("Probe mode: review-read"),
        "review read must not begin probe execution before failing on \
         missing state-root:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 4. read_subcommands_ignore_simard_state_root_env_var
//
// Acceptance criterion: even with `SIMARD_STATE_ROOT` exported to a
// fully-valid existing directory, the three read subcommands must still
// hard-fail with the unified wording (env-var fallback is NOT honored).
// ---------------------------------------------------------------------

#[test]
#[serial]
fn read_subcommands_ignore_simard_state_root_env_var() {
    let bogus = TempDirGuard::new("issue-1909-env-fallback-disallowed");
    // Create a directory that would otherwise satisfy a fallback resolver
    // — proves the env var path is rejected by intent, not by accident
    // of being missing on disk.
    fs::create_dir_all(bogus.path().join("review-artifacts"))
        .expect("review-artifacts directory should be created");
    fs::write(bogus.path().join("memory_records.json"), "[]")
        .expect("memory_records.json should be writable");

    for (subcommand, base_type) in [
        ("meeting", "local-harness"),
        ("improvement-curation", "local-harness"),
        ("review", "local-harness"),
    ] {
        let output = simard_bin()
            .env("SIMARD_STATE_ROOT", bogus.path())
            .arg(subcommand)
            .arg("read")
            .arg(base_type)
            .arg("single-process")
            .output()
            .unwrap_or_else(|err| {
                panic!("simard {subcommand} read should launch ({err})");
            });
        let rendered = rendered_output(&output);

        assert!(
            !output.status.success(),
            "{subcommand} read must hard-fail even when SIMARD_STATE_ROOT \
             is set to a valid directory (env fallback is not honored for \
             read paths):\n{rendered}"
        );
        assert_hard_fail_message(&rendered, subcommand, base_type);
    }
}

// ---------------------------------------------------------------------
// 5. meeting_read_succeeds_with_explicit_state_root
//
// Heavyweight: populates a real meeting state-root via `meeting run`,
// then runs `meeting read` with the explicit positional. Mirrors the
// existing sibling tests in `tests/simard_cli.rs` that are `#[ignore]`d
// because they spawn the full simard binary and hang in pre-commit.
// Asserts the new guard accepts an explicit positional.
// ---------------------------------------------------------------------

#[test]
#[ignore = "spawns simard binary end-to-end; matches sibling pattern in tests/simard_cli.rs"]
fn meeting_read_succeeds_with_explicit_state_root() {
    let _lock = meeting_default_root_lock()
        .lock()
        .expect("meeting default root test lock should not be poisoned");
    let state_root = default_meeting_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());

    let meeting_objective = "agenda: align the next Simard workstream\n\
update: durable memory merged\n\
decision: preserve meeting-to-engineer continuity\n\
risk: workflow routing is still unreliable\n\
next-step: keep durable priorities visible\n\
open-question: how aggressively should Simard reprioritize?\n\
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work";

    let run_output = simard_bin()
        .arg("meeting")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(meeting_objective)
        .output()
        .expect("simard meeting run should launch with its canonical default state root");
    assert!(
        run_output.status.success(),
        "meeting run prerequisite must succeed:\n{}",
        rendered_output(&run_output)
    );

    let read_output = simard_bin()
        .arg("meeting")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&state_root)
        .output()
        .expect("simard meeting read should launch with explicit state-root");
    let rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "meeting read with explicit <state-root> must succeed:\n{rendered}"
    );
    assert!(
        rendered.contains("Probe mode: meeting-read"),
        "meeting read should surface its probe mode:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("State root: {}", state_root.display())),
        "meeting read should echo the explicit state-root it used:\n{rendered}"
    );
    assert!(
        !rendered.contains("missing required configuration 'state-root'"),
        "explicit state-root must bypass the new hard-fail guard:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 6. improvement_curation_read_succeeds_with_explicit_state_root
// ---------------------------------------------------------------------

#[test]
#[ignore = "spawns simard binary end-to-end; matches sibling pattern in tests/simard_cli.rs"]
fn improvement_curation_read_succeeds_with_explicit_state_root() {
    let _lock = review_default_root_lock()
        .lock()
        .expect("review default root test lock should not be poisoned");
    let state_root = default_review_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());

    let review_run = simard_bin()
        .arg("review")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("inspect the current Simard review surface and preserve concrete proposals")
        .output()
        .expect("simard review run should launch");
    assert!(
        review_run.status.success(),
        "review run prerequisite must succeed:\n{}",
        rendered_output(&review_run)
    );

    let improvement_run = simard_bin()
        .arg("improvement-curation")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg(
            "approve: Capture denser execution evidence | priority=1 | status=active | \
             rationale=operators need denser execution evidence now",
        )
        .output()
        .expect("simard improvement-curation run should launch");
    assert!(
        improvement_run.status.success(),
        "improvement-curation run prerequisite must succeed:\n{}",
        rendered_output(&improvement_run)
    );

    let read_output = simard_bin()
        .arg("improvement-curation")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&state_root)
        .output()
        .expect("simard improvement-curation read should launch with explicit state-root");
    let rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "improvement-curation read with explicit <state-root> must succeed:\n{rendered}"
    );
    assert!(
        rendered.contains("Probe mode: improvement-curation-read"),
        "improvement-curation read should surface its probe mode:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("State root: {}", state_root.display())),
        "improvement-curation read should echo the explicit state-root it used:\n{rendered}"
    );
    assert!(
        !rendered.contains("missing required configuration 'state-root'"),
        "explicit state-root must bypass the new hard-fail guard:\n{rendered}"
    );
}

// ---------------------------------------------------------------------
// 7. review_read_succeeds_with_explicit_state_root
// ---------------------------------------------------------------------

#[test]
#[ignore = "spawns simard binary end-to-end; matches sibling pattern in tests/simard_cli.rs"]
fn review_read_succeeds_with_explicit_state_root() {
    let _lock = review_default_root_lock()
        .lock()
        .expect("review default root test lock should not be poisoned");
    let state_root = default_review_state_root("local-harness", "single-process");
    let _cleanup = CleanupDirGuard::new(state_root.clone());

    let review_run = simard_bin()
        .arg("review")
        .arg("run")
        .arg("local-harness")
        .arg("single-process")
        .arg("inspect the current Simard review surface and preserve concrete proposals")
        .output()
        .expect("simard review run should launch");
    assert!(
        review_run.status.success(),
        "review run prerequisite must succeed:\n{}",
        rendered_output(&review_run)
    );

    let read_output = simard_bin()
        .arg("review")
        .arg("read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&state_root)
        .output()
        .expect("simard review read should launch with explicit state-root");
    let rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "review read with explicit <state-root> must succeed:\n{rendered}"
    );
    assert!(
        rendered.contains("Probe mode: review-read"),
        "review read should surface its probe mode:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("State root: {}", state_root.display())),
        "review read should echo the explicit state-root it used:\n{rendered}"
    );
    assert!(
        !rendered.contains("missing required configuration 'state-root'"),
        "explicit state-root must bypass the new hard-fail guard:\n{rendered}"
    );
}
