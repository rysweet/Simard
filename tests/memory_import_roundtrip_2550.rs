//! Issue #2550 (P0), Fix #2: `simard memory import <snapshot.json> [state-root]`
//! must ingest a `cognitive_snapshot.json` back into the store, and be
//! idempotent (dedup by content) so re-running a restore never duplicates
//! memories.
//!
//! Process-boundary tests (a real `simard` process opening an on-disk store),
//! matching `tests/bin_simard_memory_cli.rs`. No sleeps, no network: a
//! `TempDir`-rooted store and a hand-written snapshot file.

use assert_cmd::Command;
use simard::memory_cognitive::{CognitiveFact, CognitiveProcedure};
use simard::remote_transfer::MemorySnapshot;
use tempfile::TempDir;

/// `simard` binary, pinned hermetic: no network update check, no socket
/// override, so reads/writes resolve to the per-`state_root` on-disk store.
fn bin() -> Command {
    let mut cmd = Command::cargo_bin("simard").expect("simard must build");
    cmd.env("SIMARD_NO_UPDATE_CHECK", "1")
        .env_remove("SIMARD_MEMORY_SOCKET")
        .env_remove("SIMARD_STATE_ROOT");
    cmd
}

/// Write a `cognitive_snapshot.json` (the same bare `MemorySnapshot` shape the
/// periodic backup writes) with `facts` facts and `procs` procedures.
fn write_snapshot(path: &std::path::Path, facts: usize, procs: usize) {
    let snapshot = MemorySnapshot {
        facts: (0..facts)
            .map(|i| CognitiveFact {
                node_id: format!("f{i}"),
                concept: format!("concept-{i}"),
                content: format!("imported fact {i}"),
                confidence: 0.9,
                source_id: "issue-2550".to_string(),
                tags: vec!["import".to_string()],
                usage_count: 0,
                last_accessed_at: None,
            })
            .collect(),
        procedures: (0..procs)
            .map(|i| CognitiveProcedure {
                node_id: format!("p{i}"),
                name: format!("proc-{i}"),
                steps: vec![format!("step {i}")],
                prerequisites: vec![],
                usage_count: 0,
            })
            .collect(),
        exported_at: 1_751_500_000,
        source_agent: "issue-2550".to_string(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();
}

/// Run `simard memory stats <state-root> --json` and return the parsed report.
fn stats(state_root: &std::path::Path) -> serde_json::Value {
    let assert = bin()
        .args(["memory", "stats", state_root.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stats --json must emit JSON, got: {stdout} (err: {e})"))
}

fn semantic(report: &serde_json::Value) -> u64 {
    report["counts"]["semantic"].as_u64().unwrap_or(0)
}

fn procedural(report: &serde_json::Value) -> u64 {
    report["counts"]["procedural"].as_u64().unwrap_or(0)
}

#[test]
fn memory_import_ingests_a_snapshot_into_the_store() {
    let scratch = TempDir::new().unwrap();
    let snap = scratch.path().join("cognitive_snapshot.json");
    write_snapshot(&snap, 3, 2);

    let state_root = TempDir::new().unwrap();
    bin()
        .args([
            "memory",
            "import",
            snap.to_str().unwrap(),
            state_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let report = stats(state_root.path());
    assert!(
        semantic(&report) >= 3,
        "imported facts must be present: {report}"
    );
    assert!(
        procedural(&report) >= 2,
        "imported procedures must be present: {report}"
    );
}

#[test]
fn memory_import_is_idempotent_by_content() {
    let scratch = TempDir::new().unwrap();
    let snap = scratch.path().join("cognitive_snapshot.json");
    write_snapshot(&snap, 4, 0);

    let state_root = TempDir::new().unwrap();
    let import = |root: &std::path::Path| {
        bin()
            .args([
                "memory",
                "import",
                snap.to_str().unwrap(),
                root.to_str().unwrap(),
            ])
            .assert()
            .success();
    };

    import(state_root.path());
    let first = semantic(&stats(state_root.path()));
    assert!(first >= 4, "first import must land 4 facts (got {first})");

    // Re-importing the SAME snapshot must not duplicate content.
    import(state_root.path());
    let second = semantic(&stats(state_root.path()));
    assert_eq!(
        first, second,
        "re-import must dedup by content, not double the count (was {first}, now {second})"
    );
}

#[test]
fn memory_import_help_is_documented() {
    let assert = bin().args(["memory", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("simard memory import"),
        "memory --help must document the import subcommand:\n{stdout}"
    );
}
