use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

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

fn load_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_str(&fs::read_to_string(path.as_ref()).expect("artifact should be readable"))
        .expect("artifact should deserialize as JSON")
}

#[test]
fn review_artifacts_can_be_promoted_into_durable_improvement_goals() {
    let state_root = TempDirGuard::new("simard-improvement-curation-state");

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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

#[test]
fn improvement_curation_read_probe_surfaces_persisted_review_decisions_without_mutating_state() {
    let state_root = TempDirGuard::new("simard-improvement-curation-read-probe");

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        "review probe should prepare durable review state for readback:\n{review_rendered}"
    );

    let improvement_objective = "\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now\n\
defer: Promote this pattern into a repeatable benchmark | rationale=wait for the next benchmark planning pass";
    let improvement_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        "improvement curation probe should persist the decisions that readback inspects:\n{improvement_rendered}"
    );

    let memory_before =
        fs::read(state_root.path().join("memory_records.json")).expect("memory store should exist");
    let goals_before =
        fs::read(state_root.path().join("goal_records.json")).expect("goal store should exist");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("improvement-curation-read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("improvement curation read probe should launch");
    let read_rendered = rendered_output(&read_output);

    assert!(
        read_output.status.success(),
        "improvement curation read should expose durable review/improvement state through the read probe:\n{read_rendered}"
    );
    for expected in [
        "Probe mode: improvement-curation-read",
        &format!("State root: {}", state_root.path().display()),
        "Latest review artifact:",
        "Review id:",
        "Review target:",
        "Approved proposals: 1",
        "Approved proposal 1: p1 [active] Capture denser execution evidence",
        "Deferred proposals: 1",
        "Deferred proposal 1: Promote this pattern into a repeatable benchmark (wait for the next benchmark planning pass)",
        "Active goals count: 1",
        "Active goal 1: p1 [active] Capture denser execution evidence",
        "Proposed goals count: 0",
        "Latest improvement record: review=",
    ] {
        assert!(
            read_rendered.contains(expected),
            "improvement curation read should surface '{expected}' for operators:\n{read_rendered}"
        );
    }

    let memory_after =
        fs::read(state_root.path().join("memory_records.json")).expect("memory store should exist");
    let goals_after =
        fs::read(state_root.path().join("goal_records.json")).expect("goal store should exist");
    assert_eq!(
        memory_before, memory_after,
        "improvement curation read must stay read-only with respect to persisted decision state"
    );
    assert_eq!(
        goals_before, goals_after,
        "improvement curation read must not rewrite the durable goal register"
    );
}

#[test]
fn improvement_curation_read_probe_rejects_nonexistent_and_empty_state_roots_before_probe_access() {
    let temp_dir = TempDirGuard::new("simard-improvement-curation-read-invalid-root");
    let missing_root = temp_dir.path().join("missing-layout");
    let empty_root = temp_dir.path().join("empty-layout");
    fs::create_dir_all(&empty_root).expect("empty state root fixture should be created");

    let missing_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("improvement-curation-read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&missing_root)
        .output()
        .expect("improvement curation read probe missing-root check should launch");
    let missing_rendered = rendered_output(&missing_output);

    assert!(
        !missing_output.status.success(),
        "improvement curation read must fail visibly for a nonexistent explicit state root:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("invalid state root")
            || missing_rendered.contains("InvalidStateRoot"),
        "improvement curation read should keep the failing state-root contract explicit:\n{missing_rendered}"
    );
    assert!(
        missing_rendered.contains("requires an existing state root directory"),
        "improvement curation read should explain why a nonexistent explicit root was rejected:\n{missing_rendered}"
    );
    assert!(
        !missing_rendered.contains("expected persisted review artifact"),
        "improvement curation read should reject a nonexistent root before probing for artifacts:\n{missing_rendered}"
    );

    let empty_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("improvement-curation-read")
        .arg("local-harness")
        .arg("single-process")
        .arg(&empty_root)
        .output()
        .expect("improvement curation read probe empty-root check should launch");
    let empty_rendered = rendered_output(&empty_output);

    assert!(
        !empty_output.status.success(),
        "improvement curation read must fail visibly for an empty explicit state root:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("invalid state root")
            || empty_rendered.contains("InvalidStateRoot"),
        "improvement curation read should keep empty-root failures in the state-root contract:\n{empty_rendered}"
    );
    assert!(
        empty_rendered.contains("review-artifacts"),
        "improvement curation read should pinpoint the missing read-only layout entry for an empty root:\n{empty_rendered}"
    );
    assert!(
        !empty_rendered.contains("expected persisted review artifact"),
        "improvement curation read should reject an empty root before probing for review artifacts:\n{empty_rendered}"
    );
}

#[test]
fn improvement_curation_read_probe_fails_closed_on_malformed_latest_improvement_record() {
    let state_root = TempDirGuard::new("simard-improvement-curation-read-malformed");

    let review_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        "review probe should prepare durable state for the malformed-record failure case:\n{review_rendered}"
    );

    let improvement_objective = "\
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now\n\
defer: Promote this pattern into a repeatable benchmark | rationale=wait for the next benchmark planning pass";
    let improvement_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
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
        "improvement curation probe should persist an initial valid record:\n{improvement_rendered}"
    );

    let memory_path = state_root.path().join("memory_records.json");
    let memory_envelope = load_json(&memory_path);
    // The memory store now uses a checksummed envelope (`{"crc32":…,"records":[…]}`).
    // Extract the records array, corrupt the target, and write back as a legacy
    // plain JSON array so the store's fallback loader accepts it without a CRC gate.
    let mut records = memory_envelope
        .get("records")
        .and_then(|v| v.as_array())
        .expect("memory store should contain a records array")
        .clone();
    let latest_improvement_record = records
        .iter_mut()
        .find(|record| {
            record["key"]
                .as_str()
                .map(|key| key.ends_with("improvement-curation-record"))
                .unwrap_or(false)
        })
        .expect("improvement curation should persist a decision record");
    latest_improvement_record["value"] = Value::String(
        "review=session-42-review target=operator-review approvals=not-a-list deferred=[]"
            .to_string(),
    );
    fs::write(
        &memory_path,
        serde_json::to_string_pretty(&Value::Array(records))
            .expect("corrupted memory store should serialize"),
    )
    .expect("corrupted memory store should be written");

    let read_output = Command::new(env!("CARGO_BIN_EXE_simard_operator_probe"))
        .env("SIMARD_BOOTSTRAP_MODE", "builtin-defaults")
        .arg("improvement-curation-read")
        .arg("local-harness")
        .arg("single-process")
        .arg(state_root.path())
        .output()
        .expect("improvement curation read probe should launch");
    let read_rendered = rendered_output(&read_output);

    assert!(
        !read_output.status.success(),
        "improvement curation read must fail closed when the persisted improvement record is malformed:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("invalid improvement record field"),
        "improvement curation read should surface the parser failure instead of silently degrading:\n{read_rendered}"
    );
    assert!(
        read_rendered.contains("approvals"),
        "improvement curation read should pinpoint the malformed approvals section:\n{read_rendered}"
    );
    assert!(
        !read_rendered.contains("Approved proposals:"),
        "improvement curation read should not print a success-shaped summary after malformed-state failure:\n{read_rendered}"
    );
}
