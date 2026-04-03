use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

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
fn engineer_loop_probe_reports_repo_state_runs_verified_action_and_persists_truthful_artifacts() {
    let expected_dirty = worktree_dirty(&repo_root());
    let isolated_state = TempDirGuard::new("simard-engineer-loop-isolated-state");
    let output = run_engineer_loop_probe_with_state_root(
        &repo_root(),
        engineer_loop_objective(),
        Some(isolated_state.path()),
    );
    let rendered = rendered_output(&output);

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
    assert!(
        rendered.contains("Verification status: verified"),
        "engineer-loop probe should only claim verified outcomes after explicit checks:\n{rendered}"
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
        evidence_payload.contains("verification-status=verified"),
        "evidence payload should preserve verification status:\n{evidence_payload}"
    );
    assert!(
        memory_payload.contains("engineer-loop-summary"),
        "memory payload should preserve a durable engineer-loop summary:\n{memory_payload}"
    );
    assert!(
        handoff_payload.contains("verification-status=verified"),
        "handoff payload should preserve verified outcome status for truthful resume behavior:\n{handoff_payload}"
    );
    assert!(
        evidence_payload.contains("carried-meeting-decisions=<none>"),
        "evidence payload should preserve whether prior meeting decisions were available:\n{evidence_payload}"
    );
}

#[test]
fn engineer_loop_probe_can_apply_a_bounded_structured_text_edit_on_a_clean_repo() {
    let repo = init_fixture_repo("simard-engineer-loop-edit-fixture");
    let state_root = TempDirGuard::new("simard-engineer-loop-edit-state");
    let objective = "\
edit-file: README.md
replace: Current status: TODO
with: Current status: DONE
verify-contains: Current status: DONE";

    let output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo.path())
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("engineer-loop edit probe should launch");
    let rendered = rendered_output(&output);

    assert!(
        output.status.success(),
        "bounded structured edit should succeed:\n{rendered}"
    );
    assert!(
        rendered.contains("Selected action: structured-text-replace"),
        "probe should reveal the bounded edit action:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "Action plan: Inspect the clean repo, replace the requested text once in 'README.md'"
        ),
        "probe should expose the edit plan:\n{rendered}"
    );
    assert!(
        rendered.contains("Changed files after action: README.md"),
        "probe should expose the changed file:\n{rendered}"
    );
    assert!(
        rendered.contains("Verification status: verified"),
        "bounded edit should still verify explicitly:\n{rendered}"
    );
    assert!(
        rendered
            .contains("Verification steps: confirm 'README.md' contains 'Current status: DONE'"),
        "probe should show the concrete verification step:\n{rendered}"
    );

    let readme_payload = fs::read_to_string(repo.path().join("README.md"))
        .expect("edited readme should be readable");
    assert!(
        readme_payload.contains("Current status: DONE"),
        "bounded edit should update the target file:\n{readme_payload}"
    );

    let status = run_command(
        repo.path(),
        &["git", "status", "--short", "--untracked-files=all"],
    );
    let status_rendered = rendered_output(&status);
    assert!(
        status.status.success(),
        "git status should succeed in fixture repo:\n{status_rendered}"
    );
    assert!(
        status_rendered.contains(" M README.md") || status_rendered.contains("M  README.md"),
        "fixture repo should show the bounded edit in git status:\n{status_rendered}"
    );

    let evidence_payload = fs::read_to_string(state_root.path().join("evidence_records.json"))
        .expect("bounded edit evidence should be readable");
    assert!(
        evidence_payload.contains("selected-action=structured-text-replace"),
        "evidence should preserve the selected bounded edit action:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("changed-files-after-action=README.md"),
        "evidence should preserve the changed file:\n{evidence_payload}"
    );
    assert!(
        evidence_payload.contains("verify-contains=README.md::Current status: DONE")
            || evidence_payload.contains("Current status: DONE"),
        "evidence should preserve the verification trace:\n{evidence_payload}"
    );
}

#[test]
fn engineer_loop_probe_fails_visibly_when_structured_replacement_target_is_missing() {
    let repo = init_fixture_repo("simard-engineer-loop-edit-miss");
    let state_root = TempDirGuard::new("simard-engineer-loop-edit-miss-state");
    let objective = "\
edit-file: README.md
replace: Current status: MISSING
with: Current status: DONE
verify-contains: Current status: DONE";

    let output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo.path())
        .arg(objective)
        .arg(state_root.path())
        .output()
        .expect("engineer-loop failing edit probe should launch");
    let rendered = rendered_output(&output);

    assert!(
        !output.status.success(),
        "missing replacement target should fail visibly:\n{rendered}"
    );
    assert!(
        rendered.contains("replacement target was not found in 'README.md'"),
        "failure should explain why the bounded edit could not proceed:\n{rendered}"
    );

    let readme_payload = fs::read_to_string(repo.path().join("README.md"))
        .expect("fixture readme should remain readable");
    assert!(
        readme_payload.contains("Current status: TODO"),
        "failed bounded edit should not mutate the file:\n{readme_payload}"
    );
}
