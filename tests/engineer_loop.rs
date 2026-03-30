use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn engineer_loop_objective() -> &'static str {
    "inspect the repository state, execute one safe local engineering action, verify the outcome explicitly, and persist truthful local evidence and memory"
}

fn run_engineer_loop_probe(workspace_root: &Path, objective: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(workspace_root)
        .arg(objective)
        .output()
        .expect("engineer-loop probe should launch")
}

fn worktree_dirty(path: &Path) -> bool {
    let output = Command::new("git")
        .args(["status", "--short", "--untracked-files=all"])
        .current_dir(path)
        .output()
        .expect("git status should launch");
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

fn output_field<'a>(output: &'a str, label: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(label).map(str::trim))
        .unwrap_or_else(|| panic!("missing output field '{label}' in:\n{output}"))
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
    let output = run_engineer_loop_probe(&repo_root(), engineer_loop_objective());
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
        rendered.contains("Selected action: "),
        "engineer-loop probe should report the grounded engineering action it chose:\n{rendered}"
    );
    assert!(
        rendered.contains("Action status: success"),
        "engineer-loop probe should report the action result explicitly:\n{rendered}"
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
}
