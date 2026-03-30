use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rendered_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
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
fn review_artifacts_can_be_promoted_into_durable_improvement_goals() {
    let state_root = TempDirGuard::new("simard-improvement-curation-state");

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("review-run")
        .arg("local-harness")
        .arg("single-process")
        .arg("inspect the current Simard review surface and preserve concrete proposals")
        .arg(state_root.path())
        .output()
        .expect("review probe should launch");
    let review_rendered = rendered_output(&review_output);

    assert!(
        review_output.status.success(),
        "review probe should succeed:\n{review_rendered}"
    );
    assert!(review_rendered.contains("Probe mode: review-run"));
    assert!(review_rendered.contains("Review proposals: 2"));

    let improvement_objective = "\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now\n\
approve: Promote this pattern into a repeatable benchmark | priority=2 | status=proposed | rationale=carry this into the next benchmark planning pass";

    let improvement_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("improvement-curation-run")
        .arg("local-harness")
        .arg("single-process")
        .arg(improvement_objective)
        .arg(state_root.path())
        .output()
        .expect("improvement curation probe should launch");
    let improvement_rendered = rendered_output(&improvement_output);

    assert!(
        improvement_output.status.success(),
        "improvement curation probe should succeed:\n{improvement_rendered}"
    );
    assert!(improvement_rendered.contains("Probe mode: improvement-curation-run"));
    assert!(improvement_rendered.contains("Identity: simard-improvement-curator"));
    assert!(improvement_rendered.contains("Approved proposals: 2"));
    assert!(improvement_rendered.contains("Deferred proposals: 0"));
    assert!(improvement_rendered.contains("Active goals count: 1"));
    assert!(
        improvement_rendered
            .contains("Active goal 1: p1 [active] Capture denser execution evidence")
    );
    assert!(improvement_rendered.contains("Proposed goals count: 1"));
    assert!(improvement_rendered.contains(
        "Proposed goal 1: p2 [proposed] Promote this pattern into a repeatable benchmark"
    ));

    let engineer_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .arg("engineer-loop-run")
        .arg("single-process")
        .arg(repo_root())
        .arg("inspect the repo and preserve explicit improvement context")
        .arg(state_root.path())
        .output()
        .expect("engineer loop probe should launch");
    let engineer_rendered = rendered_output(&engineer_output);

    assert!(
        engineer_output.status.success(),
        "engineer loop probe should succeed with promoted improvement goals:\n{engineer_rendered}"
    );
    assert!(engineer_rendered.contains("Active goals count: 1"));
    assert!(
        engineer_rendered.contains("Active goal 1: p1 [active] Capture denser execution evidence")
    );
    assert!(engineer_rendered.contains("Verification status: verified"));
}
