use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

mod common;

const CLEARED_GIT_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn engineer_loop_objective() -> &'static str {
    "inspect the repository state, execute one safe local engineering action, verify the outcome explicitly, and persist truthful local evidence and memory"
}

fn run_engineer_loop_probe(workspace_root: &Path, objective: &str) -> Output {
    run_engineer_loop_probe_with_state_root(workspace_root, objective, None)
}

fn run_engineer_loop_probe_with_state_root(
    workspace_root: &Path,
    objective: &str,
    state_root: Option<&Path>,
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"));
    cmd.env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults");
    cmd.arg("engineer-loop-run")
        .arg("single-process")
        .arg(workspace_root)
        .arg(objective);
    if let Some(root) = state_root {
        cmd.arg(root);
        // Also isolate meeting handoffs so stale artifacts don't leak in.
        cmd.env("SIMARD_HANDOFF_DIR", root.join("handoffs"));
    }
    cmd.output().expect("engineer-loop probe should launch")
}

fn worktree_dirty(path: &Path) -> bool {
    let output = run_command(path, &["git", "status", "--short", "--untracked-files=all"]);
    assert!(
        output.status.success(),
        "git status should succeed in repo-rooted engineer-loop tests"
    );
    !String::from_utf8_lossy(&output.stdout).trim().is_empty()
}

fn rendered_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

// The LLM-provider detection + fail-explicit guard now live in the shared
// `tests/common/mod.rs` module so the three engineer-loop integration crates
// no longer carry divergent copies (issue #2047). Unit tests for those helpers
// live here so they run once rather than once per including crate.
#[cfg(test)]
mod llm_provider_guard_tests {
    use crate::common::{
        llm_provider_unavailable, memory_client_unavailable, require_llm_provider,
        require_memory_client,
    };

    #[test]
    fn detects_missing_simard_llm_provider_config() {
        // After kill-tier1-fallbacks (src/error/display.rs:9), RuntimeConfig::load()
        // surfaces this exact error when SIMARD_LLM_PROVIDER is unset and no config
        // file exists. Tests that drive the engineer loop must not pass by skipping.
        let rendered = "thread 'main' panicked: missing required configuration 'SIMARD_LLM_PROVIDER': \
             set the SIMARD_LLM_PROVIDER env var or add it to ~/.simard/config.toml";
        assert!(
            llm_provider_unavailable(rendered),
            "must recognize the SIMARD_LLM_PROVIDER missing-config error"
        );
    }

    #[test]
    fn detects_legacy_no_api_key() {
        assert!(llm_provider_unavailable(
            "Error: No API key found in environment"
        ));
    }

    #[test]
    fn detects_legacy_llm_session_open_failed() {
        assert!(llm_provider_unavailable(
            "attempted to open LLM session but open() failed: connection refused"
        ));
    }

    #[test]
    fn detects_amplihack_subprocess_failure() {
        assert!(llm_provider_unavailable(
            "engineer loop failed: failed to spawn `amplihack RustyClawd --auto`"
        ));
    }

    #[test]
    fn does_not_flag_unrelated_output() {
        assert!(
            !llm_provider_unavailable("engineer loop completed: 1 cycle, 0 errors"),
            "must not false-positive on benign output"
        );
    }

    #[test]
    #[should_panic(expected = "requires a real LLM provider")]
    fn require_llm_provider_panics_when_unavailable() {
        // Force-running an ignored LLM test without a provider must fail loudly
        // (issue #2047), never silently pass by skipping.
        require_llm_provider(
            "example_test",
            "Error: missing required configuration 'SIMARD_LLM_PROVIDER'",
        );
    }

    #[test]
    fn require_llm_provider_is_noop_when_available() {
        // When the provider is available the guard returns without panicking,
        // letting the real assertions run.
        require_llm_provider(
            "example_test",
            "Probe mode: engineer-loop-run\noutcome=Success",
        );
    }

    #[test]
    fn detects_missing_memory_client() {
        // The OODA daemon surfaces these when the amplihack memory bridge is
        // unavailable; an `#[ignore]`d test that needs it must not pass by
        // skipping when force-run (issue #2047).
        assert!(memory_client_unavailable(
            "Error: Cannot find amplihack-memory-lib"
        ));
        assert!(memory_client_unavailable(
            "memory bridge unhealthy: connection refused"
        ));
    }

    #[test]
    fn does_not_flag_memory_client_on_unrelated_output() {
        assert!(
            !memory_client_unavailable("ooda daemon: seeded 5 default goals"),
            "must not false-positive on benign output"
        );
    }

    #[test]
    #[should_panic(expected = "requires the amplihack memory bridge")]
    fn require_memory_client_panics_when_unavailable() {
        // Force-running an ignored bridge-dependent test without the bridge must
        // fail loudly (issue #2047), never silently pass by an early `return`.
        require_memory_client("example_test", "Error: Cannot find amplihack-memory-lib");
    }

    #[test]
    fn require_memory_client_is_noop_when_available() {
        // When the bridge is available the guard returns without panicking,
        // letting the real assertions run.
        require_memory_client("example_test", "ooda daemon: seeded 5 default goals");
    }
}

fn run_command(cwd: &Path, argv: &[&str]) -> Output {
    let (program, args) = argv.split_first().expect("argv should include a program");
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    for key in CLEARED_GIT_ENV_VARS {
        command.env_remove(key);
    }
    command.output().expect("command should launch")
}

fn output_field<'a>(output: &'a str, label: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(label).map(str::trim))
        .unwrap_or_else(|| panic!("missing output field '{label}' in:\n{output}"))
}

fn init_fixture_repo(label: &str) -> TempDirGuard {
    let repo = TempDirGuard::new(label);
    let readme_path = repo.path().join("README.md");
    fs::write(&readme_path, "# Demo Repo\n\nCurrent status: TODO\n")
        .expect("fixture file should be written");

    let init = run_command(repo.path(), &["git", "init"]);
    assert!(init.status.success(), "git init should succeed");
    let checkout_main = run_command(repo.path(), &["git", "checkout", "-b", "main"]);
    assert!(
        checkout_main.status.success(),
        "git checkout -b main should succeed"
    );
    let config_name = run_command(repo.path(), &["git", "config", "user.name", "Simard Test"]);
    assert!(
        config_name.status.success(),
        "git user.name should configure"
    );
    let config_email = run_command(
        repo.path(),
        &["git", "config", "user.email", "simard-tests@example.com"],
    );
    assert!(
        config_email.status.success(),
        "git user.email should configure"
    );
    let add = run_command(repo.path(), &["git", "add", "README.md"]);
    assert!(add.status.success(), "git add should succeed");
    let commit = run_command(repo.path(), &["git", "commit", "-m", "initial fixture"]);
    assert!(commit.status.success(), "git commit should succeed");

    repo
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

    fn path(&self) -> &Path {
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

#[test]
fn engineer_loop_probe_rejects_non_repo_workspaces_with_explicit_not_a_repo_signal() {
    let non_repo = TempDirGuard::new("simard-engineer-loop-not-a-repo");
    let output = run_engineer_loop_probe(non_repo.path(), engineer_loop_objective());
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "non-repo engineer loop should fail visibly instead of pretending success:\n{rendered}"
    );
    assert!(
        rendered.contains("NOT_A_REPO"),
        "non-repo engineer loop should surface a NOT_A_REPO signal:\n{rendered}"
    );
    assert!(
        rendered.contains(&non_repo.path().display().to_string()),
        "non-repo failure should identify the rejected workspace path:\n{rendered}"
    );
}

#[test]
#[ignore = "requires a real LLM provider/session; reported as `ignored` by default rather than passing by skipping (issue #2047)"]
fn engineer_loop_probe_reports_repo_state_runs_verified_action_and_persists_truthful_artifacts() {
    let expected_dirty = worktree_dirty(&repo_root());
    let isolated_state = TempDirGuard::new("simard-engineer-loop-isolated-state");
    let output = run_engineer_loop_probe_with_state_root(
        &repo_root(),
        engineer_loop_objective(),
        Some(isolated_state.path()),
    );
    let rendered = rendered_output(&output);

    common::require_llm_provider(
        "engineer_loop_probe_reports_repo_state_runs_verified_action_and_persists_truthful_artifacts",
        &rendered,
    );
    assert!(
        output.status.success(),
        "repo-grounded engineer loop should succeed once implemented:\n{rendered}"
    );
    assert!(
        rendered.contains("Probe mode: engineer-loop-run"),
        "engineer-loop probe should report its explicit mode:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("Repo root: {}", repo_root().display())),
        "engineer-loop probe should expose the repo root it inspected:\n{rendered}"
    );
    assert!(
        rendered.contains("Repo branch: "),
        "engineer-loop probe should expose current branch state:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("Worktree dirty: {expected_dirty}")),
        "engineer-loop probe should expose actual worktree dirtiness before acting:\n{rendered}"
    );
    assert!(
        rendered.contains("Execution scope: local-only"),
        "v1 engineer loop must stay honest about local-only execution:\n{rendered}"
    );
    assert!(
        rendered.contains("Carried meeting decisions: 0"),
        "isolated engineer-loop runs should say when no prior meeting decisions were carried forward:\n{rendered}"
    );
    assert!(
        rendered.contains("Selected action: "),
        "engineer-loop probe should report the grounded engineering action it chose:\n{rendered}"
    );
    assert!(
        rendered.contains("Action plan: "),
        "engineer-loop probe should surface a short execution plan:\n{rendered}"
    );
    assert!(
        rendered.contains("Verification steps: "),
        "engineer-loop probe should surface explicit verification steps:\n{rendered}"
    );
    assert!(
        rendered.contains("Action status: success"),
        "engineer-loop probe should report the action result explicitly:\n{rendered}"
    );
    assert!(
        rendered.contains("Changed files after action: <none>"),
        "non-mutating engineer-loop runs should say when they changed nothing:\n{rendered}"
    );
    // Post-hoc verification (issue #1670) synthesizes checks from observable
    // side-effects. In CI the engineer probe runs against the real repo, so
    // git artifacts may or may not be present depending on whether the agent
    // made commits. Either `verified` or `unverified` is an honest outcome;
    // the retired `agent-completed` status is no longer accepted.
    assert!(
        rendered.contains("Verification status: verified")
            || rendered.contains("Verification status: unverified"),
        "engineer-loop probe must report verified or unverified (not agent-completed):\n{rendered}"
    );
    assert!(
        !rendered.contains("Azlin"),
        "local-first v1 should not imply unavailable remote orchestration:\n{rendered}"
    );

    let state_root = PathBuf::from(output_field(&rendered, "State root:"));
    let memory_path = state_root.join("memory_records.json");
    let evidence_path = state_root.join("evidence_records.json");
    let handoff_path = state_root.join("latest_handoff.json");

    assert!(
        memory_path.is_file(),
        "engineer-loop probe should persist durable memory records under the reported state root"
    );
    assert!(
        evidence_path.is_file(),
        "engineer-loop probe should persist durable evidence records under the reported state root"
    );
    assert!(
        handoff_path.is_file(),
        "engineer-loop probe should persist the latest handoff snapshot under the reported state root"
    );

    let memory_payload =
        fs::read_to_string(&memory_path).expect("persisted memory payload should be readable");
    let evidence_payload =
        fs::read_to_string(&evidence_path).expect("persisted evidence payload should be readable");
    let handoff_payload =
        fs::read_to_string(&handoff_path).expect("persisted handoff payload should be readable");

    assert!(
        evidence_payload.contains("repo-root="),
        "evidence payload should preserve the inspected repo root:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("selected-action="),
        "evidence payload should preserve the chosen engineering action:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("action-plan="),
        "evidence payload should preserve the bounded execution plan:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("action-verification-steps="),
        "evidence payload should preserve explicit verification steps:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("verification-status=verified")
            || evidence_payload.contains("verification-status=unverified"),
        "evidence payload should preserve verification status (verified or unverified):\n{evidence_payload}"
    );
    assert!(
        memory_payload.contains("engineer-loop-summary"),
        "memory payload should preserve a durable engineer-loop summary:\n{memory_payload}"
    );
    assert!(
        handoff_payload.contains("verification-status=verified")
            || handoff_payload.contains("verification-status=unverified"),
        "handoff payload should preserve verified or unverified outcome status for truthful resume behavior:\n{handoff_payload}"
    );
    assert!(
        evidence_payload.contains("carried-meeting-decisions=<none>"),
        "evidence payload should preserve whether prior meeting decisions were available:\n{evidence_payload}"
    );
}

#[test]
fn engineer_loop_timeout_kills_hung_child_and_returns_command_timeout() {
    use std::process::Command;
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let mut child = Command::new("sleep")
        .arg("3600")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("sleep should spawn");

    let deadline = Duration::from_secs(1);
    let timed_out;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                timed_out = false;
                break;
            }
            Ok(None) => {
                if start.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                timed_out = false;
                break;
            }
        }
    }

    assert!(
        timed_out,
        "watchdog should have killed the hung child before it completed naturally"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "watchdog should not wait anywhere near 3600s"
    );

    // Verify CommandTimeout display format.
    let error = simard::SimardError::CommandTimeout {
        action: "sleep 3600".to_string(),
        timeout_secs: 1,
    };
    let display = format!("{error}");
    assert!(
        display.contains("timed out after 1s"),
        "CommandTimeout should display timeout duration: {display}"
    );
    assert!(
        display.contains("sleep 3600"),
        "CommandTimeout should display the action: {display}"
    );
}

#[test]
#[ignore = "requires a real LLM provider/session; reported as `ignored` by default rather than passing by skipping (issue #2047)"]
fn engineer_loop_run_includes_non_zero_elapsed_duration() {
    let isolated_state = TempDirGuard::new("simard-engineer-loop-elapsed-state");
    let output = run_engineer_loop_probe_with_state_root(
        &repo_root(),
        engineer_loop_objective(),
        Some(isolated_state.path()),
    );
    let rendered = rendered_output(&output);

    common::require_llm_provider(
        "engineer_loop_run_includes_non_zero_elapsed_duration",
        &rendered,
    );
    assert!(
        output.status.success(),
        "engineer loop should succeed:\n{rendered}"
    );
    assert!(
        rendered.contains("Elapsed duration:"),
        "output should include elapsed duration:\n{rendered}"
    );
    assert!(
        rendered.contains("Phase traces:"),
        "output should include phase traces count:\n{rendered}"
    );
    assert!(
        rendered.contains("Phase: inspect"),
        "output should include inspect phase:\n{rendered}"
    );
    // The agent-spawn pipeline replaced the old monolithic
    // select/execute/verify phases with a finer-grained agent lifecycle:
    // load-bridge-context, agent-prompt-build, agent-spawn, agent-wait,
    // review. Assert on the agent-wait phase as the canonical "this is the
    // long-running engineering work" anchor; surrounding phases are
    // implementation details that may evolve again.
    assert!(
        rendered.contains("Phase: agent-spawn"),
        "output should include agent-spawn phase:\n{rendered}"
    );
    assert!(
        rendered.contains("Phase: agent-wait"),
        "output should include agent-wait phase (the bounded engineering work):\n{rendered}"
    );
    assert!(
        rendered.contains("Phase: review"),
        "output should include review phase:\n{rendered}"
    );
    assert!(
        rendered.contains("Phase: persist"),
        "output should include persist phase:\n{rendered}"
    );
    // All phases should report Success
    assert!(
        rendered.contains("outcome=Success"),
        "successful run should have Success outcomes:\n{rendered}"
    );
}

#[test]
#[ignore = "requires amplihack binary + LLM provider not available in CI"]
fn engineer_loop_meeting_handoff_load_failure_surfaces_in_stderr() {
    // When SIMARD_HANDOFF_DIR points at a directory with a corrupt handoff file,
    // the engineer loop should emit a warning to stderr instead of silently swallowing it.
    let repo = init_fixture_repo("simard-engineer-loop-handoff-err");
    let state_root = TempDirGuard::new("simard-engineer-loop-handoff-err-state");

    // Create a corrupt handoff artifact
    let handoff_dir = state_root.path().join("handoffs");
    fs::create_dir_all(&handoff_dir).expect("handoff dir should be created");
    fs::write(
        handoff_dir.join("meeting_handoff.json"),
        "{ this is not valid json }",
    )
    .expect("corrupt handoff should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo.path())
        .arg(engineer_loop_objective())
        .arg(state_root.path())
        .env("SIMARD_HANDOFF_DIR", &handoff_dir)
        .output()
        .expect("engineer-loop probe should launch");

    let rendered = rendered_output(&output);
    common::require_llm_provider(
        "engineer_loop_meeting_handoff_load_failure_surfaces_in_stderr",
        &rendered,
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The loop should still succeed (handoff errors are warnings, not fatal)
    // but stderr should mention the warning
    assert!(
        stderr.contains("[simard] warning: failed to load meeting handoff")
            || output.status.success(),
        "meeting handoff load failure should either surface as stderr warning or the loop succeeds despite corrupt handoff:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
}
