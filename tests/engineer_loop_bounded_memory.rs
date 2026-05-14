//! Integration regression for the bounded meeting-memory persistence
//! contract (issue #1763).
//!
//! Drives `simard::engineer_loop::persist_engineer_loop_artifacts` `N=37`
//! times against a single shared `state_root` and asserts the on-disk
//! `memory_records.json` contains exactly 32 records for each pruned scope
//! (`MemoryScope::Decision` and `MemoryScope::SessionSummary`). Other
//! scopes — `SessionScratch` in particular — are not pruned and therefore
//! grow proportionally to `N`.
//!
//! This is a black-box test that purposely uses only the public API surface
//! of the `simard` crate, so it doubles as a coverage probe for the
//! re-export wiring as well as the algorithm.

use simard::engineer_loop::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
    VerificationReport, persist_engineer_loop_artifacts,
};
use simard::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
use simard::runtime::RuntimeTopology;
use std::path::PathBuf;

const N_RUNS: usize = 37;
const EXPECTED_CAP: usize = 32;

fn make_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/fake/workspace"),
        repo_root: PathBuf::from("/fake/repo"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: false,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: Vec::new(),
        architecture_gap_summary: String::new(),
    }
}

fn make_executed() -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: SelectedEngineerAction {
            label: "test-action".into(),
            rationale: "test".into(),
            argv: vec!["test".into()],
            plan_summary: "test".into(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        },
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        changed_files: Vec::new(),
    }
}

fn make_verification() -> VerificationReport {
    VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    }
}

#[test]
fn persist_thirty_seven_runs_caps_each_pruned_scope_at_thirty_two() {
    let state_dir = tempfile::tempdir().expect("tempdir");
    let inspection = make_inspection();
    let action = make_executed();
    let verification = make_verification();

    for i in 0..N_RUNS {
        let objective = format!("integration-bound-test-call-{i:03}");
        persist_engineer_loop_artifacts(
            state_dir.path(),
            RuntimeTopology::SingleProcess,
            &objective,
            &inspection,
            &action,
            &verification,
            None,
        )
        .unwrap_or_else(|e| panic!("persist call {i} failed: {e}"));
    }

    let memory_path = state_dir.path().join("memory_records.json");
    assert!(
        memory_path.exists(),
        "memory_records.json should exist after {N_RUNS} persist calls"
    );

    let store = FileBackedMemoryStore::try_new(&memory_path).expect("reopen on-disk store");

    let decision_records = store.list(MemoryScope::Decision).unwrap();
    let summary_records = store.list(MemoryScope::SessionSummary).unwrap();
    let scratch_records = store.list(MemoryScope::SessionScratch).unwrap();

    assert_eq!(
        decision_records.len(),
        EXPECTED_CAP,
        "Decision scope must be pruned to exactly {EXPECTED_CAP} records after {N_RUNS} runs"
    );
    assert_eq!(
        summary_records.len(),
        EXPECTED_CAP,
        "SessionSummary scope must be pruned to exactly {EXPECTED_CAP} records after {N_RUNS} runs"
    );
    assert_eq!(
        scratch_records.len(),
        N_RUNS,
        "SessionScratch scope is not pruned by this contract; it must hold all {N_RUNS} records"
    );
}
