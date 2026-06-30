//! End-to-end cross-session cognitive-memory recall gate (library backend).
//!
//! **Purpose.** Prove the goal's core durability claim — *cognitive memory
//! written by one session is durably recalled by a later, independent
//! session* — across a **real process boundary**, against the merged
//! graph-edge / dedup introspection surface (issue #2331, PR #2332).
//!
//! Session A writes through [`LibraryCognitiveMemory`] (the sole backend after
//! the de-fork, #2308) and releases its file lock. **Session B is a separate
//! real `simard` process** that opens the same on-disk store via
//! `simard memory stats --json` / `simard memory dump --type=facts --json`
//! and must recall every type count, the provenance / dedup graph edges, and
//! the literal fact content. This is strictly stronger than the in-process
//! unit tests in `cognitive_memory::tests_*`: it exercises the tier-2
//! "direct open" path the operator uses against a store written by a *different*
//! process — the actual cross-session scenario.
//!
//! **What the three gates prove**
//! * `cognitive_memory_cross_session_recall` — the clean path: Session A
//!   checkpoints durably, Session B's `simard` process recalls counts, edges
//!   (`DERIVES_FROM`, `PROCEDURE_DERIVES_FROM`), fact-provenance coverage, the
//!   snapshot-dedup (`SUPERSEDES` proxy) signal, and the literal fact content.
//! * `cross_session_recall_survives_abrupt_reopen` — the abrupt-shutdown path:
//!   Session A writes but is dropped **without** the application's graceful
//!   consolidation `checkpoint()`, then Session B's `simard` process reopens
//!   the same store and still recalls the write. Acknowledged writes are durable
//!   regardless, because every store op issues a per-write `fsync` barrier and
//!   the handle's `Drop` folds the write-ahead log into the main DB file
//!   (memory-lib `lbug_store`); the reopen routes through the library's
//!   `open_with_recovery` ladder.
//! * `cross_session_recall_corrupt_catalog_self_heals` — the corruption path:
//!   the on-disk catalog is overwritten with garbage between sessions (the
//!   shape of the #95 "table 0 doesn't exist in catalog" incident). Session B's
//!   `simard` process must **self-heal instead of crash-looping** — open via
//!   `open_with_recovery`, quarantine the corrupt file to a `*.corrupt-*`
//!   sibling (never delete it), and rebuild a fresh, empty, usable store
//!   (memory-lib #95/#96). This gates resilience, not recall: a corrupt catalog
//!   is *not* recoverable, so the honest guarantee is "no crash loop + the
//!   damaged store is preserved aside", which the assertions check exactly.
//!
//! **Hermeticity.** Each test pins its own `tempfile::TempDir` state root and
//! the child `simard` is spawned with `SIMARD_NO_UPDATE_CHECK=1`, no
//! `SIMARD_MEMORY_SOCKET`, and no inherited `SIMARD_STATE_ROOT`, so reads
//! resolve to the per-state-root store with no daemon and fall through to the
//! direct on-disk open. Nothing leaks into or out of the suite.

use assert_cmd::Command;
use serde_json::Value;
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use tempfile::TempDir;

// ───────────────────────────────────────────────────────────────────────────
// Deterministic canary fixtures (pure constants — reproducible failures)
// ───────────────────────────────────────────────────────────────────────────

/// Canary semantic fact written by Session A and recalled by Session B.
const FACT_CONCEPT: &str = "cross_session_recall_canary";
const FACT_CONTENT: &str =
    "issue-2331: durable cross-session recall must round-trip through simard memory stats";

/// Caller key reused with changed content so Session A produces a snapshot
/// `SUPERSEDES` (one distinct caller key, two snapshot facts) — the
/// operator-visible dedup signal surfaced by `simard memory stats`.
const SNAPSHOT_CALLER_KEY: &str = "goal-board:snapshot";

// ───────────────────────────────────────────────────────────────────────────
// Real `simard` process helpers (Session B)
// ───────────────────────────────────────────────────────────────────────────

/// A hermetic, pinned `simard` binary: no network update check, no socket
/// override (so reads resolve to the per-state-root store, absent here, and
/// fall through to a direct on-disk open), and no inherited `SIMARD_STATE_ROOT`.
fn simard_bin() -> Command {
    let mut cmd = Command::cargo_bin("simard").expect("simard must build");
    cmd.env("SIMARD_NO_UPDATE_CHECK", "1")
        .env_remove("SIMARD_MEMORY_SOCKET")
        .env_remove("SIMARD_STATE_ROOT");
    cmd
}

/// Run `simard memory <args...> --json` against `state_root` in a *separate*
/// process and parse stdout as JSON. Asserts the process succeeded.
fn run_memory_json(state_root: &std::path::Path, args: &[&str]) -> Value {
    let mut full = vec!["memory"];
    full.extend_from_slice(args);
    full.push(state_root.to_str().expect("utf-8 state root"));
    full.push("--json");

    let assert = simard_bin().args(&full).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("`simard memory {args:?} --json` must emit JSON, got: {stdout} (err: {e})")
    })
}

/// Extract a `u64` from a nested `report[section][key]`, panicking with the
/// full report on a missing / non-numeric value so failures are diagnosable.
fn nested_u64(report: &Value, section: &str, key: &str) -> u64 {
    report[section][key]
        .as_u64()
        .unwrap_or_else(|| panic!("{section}.{key} missing/non-numeric in: {report}"))
}

// ───────────────────────────────────────────────────────────────────────────
// Session A: durable writer
// ───────────────────────────────────────────────────────────────────────────

/// Seed `state_root` as "Session A" with one row of every cognitive-memory
/// type plus the provenance / dedup edges that power the `simard memory stats`
/// "edges / connections" section, then **checkpoint** (fold the WAL into the
/// main DB) and drop the handle so its file lock is released before the
/// separate `simard` process opens the store.
fn session_a_write_durable(state_root: &std::path::Path) {
    let mem = LibraryCognitiveMemory::open(state_root).expect("Session A: open store");

    // One row of every introspectable type (counts coverage, mirrors #2308).
    mem.record_sensory("text", "operator typed: status", 3600)
        .expect("record_sensory");
    mem.push_working(
        "focus",
        "investigating cross-session recall",
        "task-2331",
        0.8,
    )
    .expect("push_working");

    // An episode + a fact distilled FROM it -> one DERIVES_FROM edge and
    // fact-provenance coverage.
    let episode = mem
        .store_episode(
            "ran cargo test; 0 failures; durable recall verified",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    mem.store_fact_with_provenance(
        FACT_CONCEPT,
        FACT_CONTENT,
        0.97,
        &format!("distill:{episode}"),
        Some(&["issue-2331".to_string(), "durable".to_string()]),
        None,
        std::slice::from_ref(&episode),
    )
    .expect("store_fact_with_provenance");

    // A procedure distilled from the same episode -> one PROCEDURE_DERIVES_FROM.
    mem.store_procedure_with_provenance(
        "ooda:verify-durable-recall",
        &[
            "write facts in session A".to_string(),
            "recall in session B".to_string(),
        ],
        &[],
        std::slice::from_ref(&episode),
    )
    .expect("store_procedure_with_provenance");

    // A prospective trigger (prospective count coverage).
    mem.store_prospective(
        "goal:Prove durable recall",
        "durable recall",
        "Pursue goal",
        1,
    )
    .expect("store_prospective");

    // Snapshot dedup: reuse one caller key with CHANGED content -> SUPERSEDES
    // (live rev:2 + archived rev:1). distinct_caller_keys == 1, snapshot_facts == 2.
    mem.store_fact_with_caller_key(
        SNAPSHOT_CALLER_KEY,
        SNAPSHOT_CALLER_KEY,
        "{\"rev\":1}",
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )
    .expect("snapshot rev 1");
    mem.store_fact_with_caller_key(
        SNAPSHOT_CALLER_KEY,
        SNAPSHOT_CALLER_KEY,
        "{\"rev\":2}",
        1.0,
        &["goal-board".to_string()],
        "goal-curator",
    )
    .expect("snapshot rev 2");

    // Graceful shutdown: CHECKPOINT folds the WAL into the main DB so a clean
    // reopen needs no replay. `mem` drops here, releasing the file lock.
    mem.checkpoint()
        .expect("Session A: checkpoint before close");
}

// ───────────────────────────────────────────────────────────────────────────
// Gate 1 — clean cross-session recall through the real `simard` binary
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn cognitive_memory_cross_session_recall() {
    let temp = TempDir::new().expect("tempdir for hermetic state root");
    session_a_write_durable(temp.path());

    // ── Session B: a separate `simard` process reads `memory stats --json`. ──
    let report = run_memory_json(temp.path(), &["stats"]);

    // A process opening a store it did not write must route through the
    // on-disk direct-open tier (no daemon socket present).
    assert_eq!(
        report["access_tier"].as_str(),
        Some("direct-open"),
        "Session B must direct-open the cross-session store: {report}"
    );

    // Every populated type survives the process boundary.
    for key in [
        "sensory",
        "working",
        "episodic",
        "semantic",
        "procedural",
        "prospective",
    ] {
        assert!(
            nested_u64(&report, "counts", key) >= 1,
            "type '{key}' must be recalled with a non-zero count: {report}"
        );
    }

    // Graph edges (#2331): the provenance-linked fact and procedure survive.
    assert!(
        nested_u64(&report, "edges", "derives_from") >= 1,
        "the provenance-linked fact's DERIVES_FROM edge must survive recall: {report}"
    );
    assert!(
        nested_u64(&report, "edges", "procedure_derives_from") >= 1,
        "the procedure's PROCEDURE_DERIVES_FROM edge must survive recall: {report}"
    );
    assert!(
        nested_u64(&report, "provenance", "facts_with_provenance") >= 1,
        "fact-provenance coverage must survive recall: {report}"
    );

    // Snapshot dedup (SUPERSEDES proxy): one distinct caller key, both revs
    // retained -> the dedup volume is operator-visible across sessions.
    assert_eq!(
        nested_u64(&report, "snapshot_dedup", "distinct_caller_keys"),
        1,
        "the two snapshot writes must collapse onto one caller key: {report}"
    );
    assert_eq!(
        nested_u64(&report, "snapshot_dedup", "snapshot_facts"),
        2,
        "both the superseded and live snapshot revisions must be retained: {report}"
    );

    // ── Session B: `memory dump --type=facts --json` recalls literal content. ──
    let dump = run_memory_json(temp.path(), &["dump", "--type=facts"]);
    assert!(
        nested_u64(&dump, "counts", "semantic") >= 1,
        "dump must count the recalled facts: {dump}"
    );
    let facts = dump["samples"]["facts"]
        .as_array()
        .unwrap_or_else(|| panic!("dump must include a samples.facts array: {dump}"));
    let recalled: Vec<&str> = facts.iter().filter_map(Value::as_str).collect();
    assert!(
        recalled
            .iter()
            .any(|row| row.contains(FACT_CONCEPT) && row.contains("durable cross-session recall")),
        "Session B failed to recall the canary fact content. Got facts: {recalled:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Gate 2 — durable recall survives an abrupt (un-checkpointed) cross-process reopen
// ───────────────────────────────────────────────────────────────────────────

/// Abrupt-shutdown durability: Session A writes an acknowledged fact but is
/// dropped **without** the application's graceful consolidation `checkpoint()`,
/// then a separate Session B `simard` process reopens the store and must still
/// recall the write.
///
/// The write is durable regardless of the missing app-level checkpoint: every
/// store op issues a per-write `fsync` barrier, and the handle's `Drop` folds
/// the write-ahead log into the main DB file (memory-lib `lbug_store`). Session
/// B's open routes through `LibraryCognitiveMemory::open` ->
/// `open_with_recovery`, so the reopen is the same recovery-wired path the
/// operator uses in production. This gate proves recall survives an abrupt
/// cross-process reopen; it does **not** inject corruption (that is Gate 3).
#[test]
fn cross_session_recall_survives_abrupt_reopen() {
    let temp = TempDir::new().expect("tempdir for hermetic state root");

    // ── Session A: write, then drop WITHOUT the app's graceful checkpoint. ──
    {
        let mem = LibraryCognitiveMemory::open(temp.path()).expect("Session A: open store");
        mem.store_fact(
            FACT_CONCEPT,
            FACT_CONTENT,
            0.97,
            &["issue-2331".to_string(), "abrupt-reopen".to_string()],
            "abrupt-test",
        )
        .expect("Session A: store canary fact");
        // NOTE: deliberately NO `mem.checkpoint()` — the application's graceful
        // consolidation flush never ran. The handle drops here; durability rests
        // on the per-write fsync barrier + the Drop-time WAL fold, not on an
        // explicit app checkpoint.
    }

    // ── Session B: a separate `simard` process reopens and must recall. ──
    let report = run_memory_json(temp.path(), &["stats"]);
    assert!(
        nested_u64(&report, "counts", "semantic") >= 1,
        "an acknowledged write must survive an un-checkpointed abrupt reopen: {report}"
    );

    let dump = run_memory_json(temp.path(), &["dump", "--type=facts"]);
    let facts = dump["samples"]["facts"]
        .as_array()
        .unwrap_or_else(|| panic!("dump must include a samples.facts array: {dump}"));
    let recalled: Vec<&str> = facts.iter().filter_map(Value::as_str).collect();
    assert!(
        recalled.iter().any(|row| row.contains(FACT_CONCEPT)),
        "abrupt reopen: Session B failed to recall the canary fact. Got facts: {recalled:?}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Gate 3 — corrupt catalog self-heals without crash-looping (memory-lib #95/#96)
// ───────────────────────────────────────────────────────────────────────────

/// Corruption resilience: Session A writes durably, then the on-disk catalog
/// (the main DB file at `state_root/cognitive`) is overwritten with garbage —
/// the shape of the #95 incident where a failed CHECKPOINT corrupted the main
/// file ("table 0 doesn't exist in catalog"). A separate Session B `simard`
/// process must **self-heal instead of crash-looping**: `LibraryCognitiveMemory::open`
/// routes through `open_with_recovery`, which quarantines the corrupt file to a
/// `*.corrupt-*` sibling (never deletes it) and rebuilds a fresh, empty,
/// readable store (memory-lib #95/#96).
///
/// This is the honest scope: a corrupt catalog is **not** recoverable, so the
/// guarantee is "no crash loop, the command still succeeds, and the damaged
/// store is preserved aside" — *not* that the prior facts are recalled (the
/// rebuilt store is empty). Asserting empty-with-quarantine is what makes this a
/// real recovery gate rather than an accidental pass.
#[test]
fn cross_session_recall_corrupt_catalog_self_heals() {
    let temp = TempDir::new().expect("tempdir for hermetic state root");
    let db_file = temp.path().join("cognitive");

    // ── Session A: write + checkpoint + drop so the main DB file is durable. ──
    {
        let mem = LibraryCognitiveMemory::open(temp.path()).expect("Session A: open store");
        mem.store_fact(
            FACT_CONCEPT,
            FACT_CONTENT,
            0.97,
            &["issue-2331".to_string(), "corruption".to_string()],
            "corruption-test",
        )
        .expect("Session A: store canary fact");
        mem.checkpoint()
            .expect("Session A: checkpoint to fold WAL into main DB");
    }
    assert!(
        db_file.is_file(),
        "Session A must have produced the main DB file at {}",
        db_file.display()
    );

    // ── Corrupt the catalog: overwrite the main DB file with garbage. ──
    let len = std::fs::metadata(&db_file).expect("stat db file").len();
    assert!(len > 0, "db file must be non-empty to corrupt");
    std::fs::write(&db_file, vec![0xABu8; len as usize]).expect("overwrite db with garbage");

    // ── Session B: a separate `simard` process must self-heal, not crash-loop. ──
    // `stats --json` succeeding at all is the no-crash-loop proof.
    let report = run_memory_json(temp.path(), &["stats"]);
    assert_eq!(
        nested_u64(&report, "counts", "total"),
        0,
        "a corrupt catalog must rebuild to a fresh, empty store: {report}"
    );

    // Recovery must have quarantined the corrupt file aside (never deleted it).
    let quarantined: Vec<String> = std::fs::read_dir(temp.path())
        .expect("read state root")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("cognitive") && name.contains(".corrupt-"))
        .collect();
    assert!(
        !quarantined.is_empty(),
        "the corrupt catalog must be quarantined to a `cognitive*.corrupt-*` sibling \
         (recovery must run, not silently pass); state-root entries: {:?}",
        std::fs::read_dir(temp.path())
            .map(|rd| rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
}
