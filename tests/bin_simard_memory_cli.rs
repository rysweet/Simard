//! Process-boundary integration tests for `simard memory <stats|dump>`.
//!
//! These exercise the *external-service integration* the unit tests in
//! `operator_cli::memory` cannot: a real `simard` process opening an on-disk
//! cognitive-memory store written by a *separate* process. This is the
//! tier-2 "direct open" path of `open_reader_client` — the same path the
//! operator uses against a live store when the OODA daemon is down.
//!
//! The store is seeded here with one row of every one of the six cognitive
//! memory types, so the assertions double as the binary-level proof that
//! issue #2308 ("make all cognitive memory types demonstrably populated") is
//! satisfied end to end, not just in-process.

use assert_cmd::Command;
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use tempfile::TempDir;

/// `simard` binary, pinned hermetic: no network update check and no socket
/// override, so reads resolve to the per-`state_root` `memory.sock` (absent
/// here) and fall through to a direct on-disk open.
fn bin() -> Command {
    let mut cmd = Command::cargo_bin("simard").expect("simard must build");
    cmd.env("SIMARD_NO_UPDATE_CHECK", "1")
        .env_remove("SIMARD_MEMORY_SOCKET")
        .env_remove("SIMARD_STATE_ROOT");
    cmd
}

/// Seed a fresh store under `state_root` with one row of every introspectable
/// type, then drop the writer so its file lock is released before a separate
/// `simard` process opens the same DB.
fn seed_all_types(state_root: &std::path::Path) {
    let mem = LibraryCognitiveMemory::open(state_root).expect("open store");

    mem.record_sensory("text", "operator typed: status", 3600)
        .expect("record_sensory");
    mem.push_working("focus", "investigating memory CLI", "task-2308", 0.8)
        .expect("push_working");
    mem.store_episode("ran cargo test; 0 failures", "engineer-cycle", None)
        .expect("store_episode");
    mem.store_fact(
        "rust",
        "Rust is a systems language",
        0.9,
        &["language".to_string()],
        "test",
    )
    .expect("store_fact");
    mem.store_procedure(
        "ooda:consolidate-memory",
        &["distill episodes".to_string()],
        &[],
    )
    .expect("store_procedure");
    mem.store_prospective("goal:Ship the CLI", "ship the cli", "Pursue goal", 1)
        .expect("store_prospective");
    // `mem` drops here, releasing the store before the child process opens it.
}

fn count(json: &serde_json::Value, key: &str) -> u64 {
    json["counts"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("counts.{key} missing/non-numeric in: {json}"))
}

/// Regression guard for the rev-`34d6a75` dependency bump: the `amplihack-memory`
/// stack logs at `info` on store open (e.g. its "effective LadybugDB limits"
/// line). Those diagnostics must go to **stderr**, never stdout, so
/// `simard memory stats --json` stays pipe-safe (`… | jq`). Asserting that
/// stdout is exactly one JSON document — with nothing before the leading `{`
/// — fails fast if a future change reroutes logs back onto stdout.
#[test]
fn stats_json_stdout_is_pure_json_logs_routed_to_stderr() {
    let tmp = TempDir::new().unwrap();
    seed_all_types(tmp.path());

    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with('{') && trimmed.ends_with('}'),
        "stats --json stdout must be pure JSON (diagnostic logs belong on \
         stderr), got:\n{stdout}"
    );
    serde_json::from_str::<serde_json::Value>(trimmed)
        .unwrap_or_else(|e| panic!("stats --json stdout must parse as JSON: {e}\n{stdout}"));
}

#[test]
fn stats_json_reports_every_populated_type_via_direct_open() {
    let tmp = TempDir::new().unwrap();
    seed_all_types(tmp.path());

    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stats --json must emit JSON, got: {stdout} (err: {e})"));

    // A separate process opening a store it did not write must route through
    // the on-disk direct-open tier.
    assert_eq!(
        report["access_tier"].as_str(),
        Some("direct-open"),
        "report: {report}"
    );
    assert!(
        report["store_path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("cognitive"),
        "store_path should point at the cognitive subdir: {report}"
    );

    // Every one of the six types must be demonstrably populated (issue #2308).
    for key in [
        "sensory",
        "working",
        "episodic",
        "semantic",
        "procedural",
        "prospective",
    ] {
        assert!(
            count(&report, key) >= 1,
            "type '{key}' must have a non-zero count: {report}"
        );
    }
    assert!(
        count(&report, "total") >= 6,
        "total must sum all six populated types: {report}"
    );
}

#[test]
fn stats_human_shows_count_table_and_access_banner() {
    let tmp = TempDir::new().unwrap();
    seed_all_types(tmp.path());

    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap()])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    for needle in [
        "via direct open",
        "sensory",
        "working",
        "episodic",
        "semantic",
        "(facts)",
        "procedural",
        "(procedures)",
        "prospective",
        "(triggers)",
        "total",
    ] {
        assert!(
            stdout.contains(needle),
            "human stats output missing {needle:?}:\n{stdout}"
        );
    }
}

/// Issue #2331: the "edges / connections" section must appear end-to-end over a
/// direct on-disk open, surfacing the provenance edge and snapshot dedup it was
/// seeded with — in both the human and `--json` renderings.
#[test]
fn stats_shows_edges_and_dedup_section_via_direct_open() {
    let tmp = TempDir::new().unwrap();
    {
        let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open store");
        let ep = mem
            .store_episode("ran cargo test; 0 failures", "engineer-cycle", None)
            .expect("store_episode");
        mem.store_fact_with_provenance(
            "lesson",
            "tests must stay green",
            0.9,
            "distill:cycle",
            None,
            None,
            std::slice::from_ref(&ep),
        )
        .expect("store_fact_with_provenance");
        // A caller-key snapshot fact so the dedup line has something to report.
        mem.store_fact_with_caller_key(
            "goal-board:snapshot",
            "goal-board:snapshot",
            "{\"rev\":1}",
            1.0,
            &["goal-board".to_string()],
            "goal-curator",
        )
        .expect("store_fact_with_caller_key");
        // `mem` drops here, releasing the lock before the child process opens it.
    }

    // Human rendering: the section labels and the seeded provenance coverage.
    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap()])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    for needle in [
        "edges / connections",
        "DERIVES_FROM",
        "PROCEDURE_DERIVES_FROM",
        "SIMILAR_TO",
        "SUPERSEDES",
        "facts with provenance:",
        "snapshot dedup:",
    ] {
        assert!(
            stdout.contains(needle),
            "human edges section missing {needle:?}:\n{stdout}"
        );
    }

    // JSON rendering: stable keys with the seeded DERIVES_FROM edge counted.
    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stats --json must emit JSON");
    assert!(
        report["edges"]["derives_from"].as_u64().unwrap_or(0) >= 1,
        "DERIVES_FROM edge must be counted via direct open: {report}"
    );
    assert_eq!(
        report["provenance"]["facts_with_provenance"].as_u64(),
        Some(1),
        "exactly the one provenance-linked fact: {report}"
    );
    assert!(
        report["snapshot_dedup"]["snapshot_facts"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "snapshot fact must be counted: {report}"
    );
    assert_eq!(
        report["snapshot_dedup"]["distinct_caller_keys"].as_u64(),
        Some(1),
        "the snapshot caller key must be grouped: {report}"
    );
    assert!(
        report.get("edges_note").is_none(),
        "direct open must compute the edges, not note them: {report}"
    );
}

#[test]
fn dump_facts_json_runs_against_on_disk_store() {
    let tmp = TempDir::new().unwrap();
    seed_all_types(tmp.path());

    let assert = bin()
        .args([
            "memory",
            "dump",
            tmp.path().to_str().unwrap(),
            "--type=facts",
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let report: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("dump --json must emit JSON, got: {stdout} (err: {e})"));

    // Counts are authoritative even for a single-type dump.
    assert!(count(&report, "semantic") >= 1, "report: {report}");
    // dump includes the samples object (rows are best-effort and may be empty).
    assert!(report.get("samples").is_some(), "report: {report}");
}

#[test]
fn empty_store_reports_zero_counts_without_error() {
    let tmp = TempDir::new().unwrap();
    // No seeding: open_reader_client creates the store on first open.
    let assert = bin()
        .args(["memory", "stats", tmp.path().to_str().unwrap(), "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stats --json must emit JSON");
    assert_eq!(
        count(&report, "total"),
        0,
        "fresh store must be empty: {report}"
    );
}

#[test]
fn unknown_memory_subcommand_exits_failure() {
    let assert = bin().args(["memory", "frobnicate"]).assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("unsupported command") && stderr.contains("frobnicate"),
        "stderr: {stderr}"
    );
}

#[test]
fn memory_help_succeeds_and_documents_subcommands() {
    let assert = bin().args(["memory", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("Usage:"), "stdout: {stdout}");
    assert!(stdout.contains("simard memory stats"), "stdout: {stdout}");
    assert!(stdout.contains("simard memory dump"), "stdout: {stdout}");
}
