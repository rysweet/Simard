use super::*;

fn test_mem() -> NativeCognitiveMemory {
    NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
}

#[test]
fn open_in_memory_creates_schema() {
    let mem = test_mem();
    let stats = mem.get_statistics().unwrap();
    assert_eq!(stats.total(), 0);
}

#[test]
fn store_and_search_fact() {
    let mem = test_mem();
    let id = mem
        .store_fact("rust", "systems language", 0.9, &[], "test")
        .unwrap();
    assert!(id.starts_with("sem_"));

    let facts = mem.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert!((facts[0].confidence - 0.9).abs() < f64::EPSILON);
}

#[test]
fn search_facts_respects_min_confidence() {
    let mem = test_mem();
    mem.store_fact("low", "low confidence", 0.1, &[], "test")
        .unwrap();
    mem.store_fact("high", "high confidence", 0.9, &[], "test")
        .unwrap();

    let results = mem.search_facts("confidence", 10, 0.5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].concept, "high");
}

#[test]
fn record_and_prune_sensory() {
    let mem = test_mem();
    mem.record_sensory("test", "data", 0).unwrap(); // expires immediately
    let pruned = mem.prune_expired_sensory().unwrap();
    assert!(pruned >= 1);
}

#[test]
fn push_get_clear_working() {
    let mem = test_mem();
    mem.push_working("goal", "build it", "task-1", 1.0).unwrap();
    mem.push_working("context", "extra", "task-1", 0.5).unwrap();

    let slots = mem.get_working("task-1").unwrap();
    assert_eq!(slots.len(), 2);

    let cleared = mem.clear_working("task-1").unwrap();
    assert_eq!(cleared, 2);
    assert!(mem.get_working("task-1").unwrap().is_empty());
}

#[test]
fn store_episode_and_consolidate() {
    let mem = test_mem();
    for i in 0..5 {
        mem.store_episode(&format!("event {i}"), "test", None)
            .unwrap();
    }
    let consolidated = mem.consolidate_episodes(5).unwrap();
    assert!(consolidated.is_some());
    let stats = mem.get_statistics().unwrap();
    // 5 original (now compressed=1) + 1 summary = 6
    assert_eq!(stats.episodic_count, 6);
}

#[test]
fn consolidate_episodes_returns_none_when_insufficient() {
    let mem = test_mem();
    mem.store_episode("only one", "test", None).unwrap();
    assert!(mem.consolidate_episodes(5).unwrap().is_none());
}

#[test]
fn store_and_recall_procedure() {
    let mem = test_mem();
    let steps = vec!["compile".to_string(), "test".to_string()];
    mem.store_procedure("build", &steps, &[]).unwrap();

    let procs = mem.recall_procedure("build", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "build");
    assert_eq!(procs[0].steps, steps);
}

// G17 (issue #1604): `recall_procedure` previously called
// `serde_json::from_str(...).unwrap_or_default()` on the `steps` and
// `prerequisites` columns.  A row whose JSON was corrupted (schema drift,
// truncation, partial flush before fsync) was silently surfaced as a
// "valid procedure with zero steps" — the exact silent-empty-recall
// failure mode that the #1711/#1748/#1754 "no silent fallback" pattern
// targets across the cognitive substrate.
//
// This test plants a Procedure node whose `steps` column is **not** valid
// JSON and asserts that recall returns a loud `BridgeCallFailed` error
// that names the offending node id, the column, and the corrupt payload —
// instead of cheerfully returning an empty-steps procedure.
#[test]
fn recall_procedure_loudly_errors_on_corrupt_steps_json() {
    let mem = test_mem();

    // Plant a Procedure row with deliberately corrupt steps JSON.
    // `escape_cypher` is used so the corrupt payload survives Cypher
    // string parsing — what we want to corrupt is the *JSON* payload
    // after escape, not the Cypher literal itself.
    let corrupt_steps = "not-valid-json{[";
    mem.execute(&format!(
        "CREATE (p:Procedure {{id: 'proc_corrupt_steps', name: 'corrupt', steps: '{}', prerequisites: '[]', usage_count: 0}})",
        escape_cypher(corrupt_steps),
    ))
    .expect("planted corrupt row should insert");

    let err = mem
        .recall_procedure("corrupt", 5)
        .expect_err("recall must fail loudly on corrupt steps JSON, not return empty steps");

    let msg = format!("{err}");
    assert!(
        msg.contains("proc_corrupt_steps"),
        "error must name offending procedure id, got: {msg}"
    );
    assert!(
        msg.contains("steps"),
        "error must name the corrupt column, got: {msg}"
    );
    assert!(
        msg.contains("corrupt steps JSON")
            || msg.contains("corrupt_steps")
            || msg.contains("not-valid-json"),
        "error must surface the corruption, got: {msg}"
    );
}

// Companion test for the prerequisites column — same gap, different field.
#[test]
fn recall_procedure_loudly_errors_on_corrupt_prerequisites_json() {
    let mem = test_mem();

    let corrupt_prereqs = "{not valid";
    mem.execute(&format!(
        "CREATE (p:Procedure {{id: 'proc_corrupt_prereqs', name: 'corrupt2', steps: '[]', prerequisites: '{}', usage_count: 0}})",
        escape_cypher(corrupt_prereqs),
    ))
    .expect("planted corrupt row should insert");

    let err = mem
        .recall_procedure("corrupt2", 5)
        .expect_err("recall must fail loudly on corrupt prerequisites JSON");

    let msg = format!("{err}");
    assert!(
        msg.contains("proc_corrupt_prereqs"),
        "error must name offending procedure id, got: {msg}"
    );
    assert!(
        msg.contains("prerequisites"),
        "error must name the corrupt column, got: {msg}"
    );
}

// Negative control: a healthy row sitting alongside a corrupt one still
// causes the whole recall to fail loudly, rather than partial-success
// where the healthy row hides the corrupt one.  This protects against a
// "skip corrupt rows" regression that would re-introduce silent recall.
#[test]
fn recall_procedure_corrupt_row_taints_whole_batch() {
    let mem = test_mem();

    mem.store_procedure("healthy", &["step-a".to_string()], &[])
        .expect("healthy row should insert");
    mem.execute(&format!(
        "CREATE (p:Procedure {{id: 'proc_mixed_corrupt', name: 'healthy_mixed', steps: '{}', prerequisites: '[]', usage_count: 0}})",
        escape_cypher("garbage{"),
    ))
    .expect("planted corrupt row should insert");

    // Query that matches both rows.
    let result = mem.recall_procedure("healthy", 10);
    let err = result.expect_err(
        "recall must surface corruption from any matched row — silent skip would let bad data hide",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("proc_mixed_corrupt"),
        "error must name the corrupt row even when a healthy row matched first, got: {msg}"
    );
}

#[test]
fn store_prospective_and_check_triggers() {
    let mem = test_mem();
    mem.store_prospective("watch errors", "error", "alert", 5)
        .unwrap();
    let triggered = mem.check_triggers("an error occurred").unwrap();
    assert_eq!(triggered.len(), 1);
    assert_eq!(triggered[0].description, "watch errors");
}

#[test]
fn check_triggers_ignores_non_matching() {
    let mem = test_mem();
    mem.store_prospective("watch errors", "error", "alert", 5)
        .unwrap();
    let triggered = mem.check_triggers("all good").unwrap();
    assert!(triggered.is_empty());
}

#[test]
fn get_statistics_counts_all_types() {
    let mem = test_mem();
    mem.record_sensory("vis", "img", 300).unwrap();
    mem.push_working("ctx", "data", "t1", 1.0).unwrap();
    mem.store_episode("event", "src", None).unwrap();
    mem.store_fact("f", "fact", 0.5, &[], "").unwrap();
    mem.store_procedure("p", &[], &[]).unwrap();
    mem.store_prospective("desc", "trigger", "action", 1)
        .unwrap();
    let stats = mem.get_statistics().unwrap();
    assert_eq!(stats.total(), 6);
}

#[test]
fn cypher_injection_escaped() {
    let mem = test_mem();
    let result = mem.store_fact("test'DROP", "con'tent", 0.5, &[], "src");
    assert!(result.is_ok(), "single quotes should be escaped");
}

#[test]
fn escape_cypher_handles_special_chars() {
    assert_eq!(escape_cypher("a'b"), "a\\'b");
    assert_eq!(escape_cypher("a\\b"), "a\\\\b");
    assert_eq!(escape_cypher("line\nbreak"), "line\\nbreak");
    assert_eq!(escape_cypher("tab\there"), "tab\\there");
    assert_eq!(escape_cypher("null\0byte"), "null\\0byte");
    assert_eq!(escape_cypher("cr\rreturn"), "cr\\rreturn");
}

#[test]
fn newline_in_content_does_not_break_query() {
    let mem = test_mem();
    let result = mem.store_fact("key", "line1\nline2\ttab", 0.5, &[], "src");
    assert!(result.is_ok(), "newlines and tabs should be safely escaped");
    let facts = mem.search_facts("key", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn disk_persist_facts_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        mem.store_fact("rust", "systems language", 0.95, &[], "test")
            .unwrap();
    } // drop closes the DB

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    let facts = mem2.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert_eq!(facts[0].content, "systems language");
    assert!((facts[0].confidence - 0.95).abs() < f64::EPSILON);
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn disk_persist_procedures_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        let steps = vec![
            "compile".to_string(),
            "test".to_string(),
            "deploy".to_string(),
        ];
        mem.store_procedure("release", &steps, &[]).unwrap();
    }

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    let procs = mem2.recall_procedure("release", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "release");
    assert_eq!(
        procs[0].steps,
        vec![
            "compile".to_string(),
            "test".to_string(),
            "deploy".to_string()
        ]
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn disk_persist_episodes_and_consolidation_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        for i in 0..5 {
            mem.store_episode(&format!("event {i}"), "test", None)
                .unwrap();
        }
        let consolidated = mem.consolidate_episodes(5).unwrap();
        assert!(consolidated.is_some());
    }

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    // Query for the consolidated episode (compressed=1 with source_label='consolidation')
    let rows = mem2
        .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
        .unwrap();
    assert_eq!(rows.len(), 1, "consolidated episode should survive reopen");
    let content = super::as_str(&rows[0][0]).unwrap();
    assert!(
        content.starts_with("[consolidated 5"),
        "consolidated content should start with marker, got: {content}"
    );
}

#[test]
fn consolidate_episodes_deduplicates() {
    let mem = test_mem();
    // Store duplicate episodes
    mem.store_episode("duplicate event", "test", None).unwrap();
    mem.store_episode("duplicate event", "test", None).unwrap();
    mem.store_episode("  duplicate event  ", "test", None)
        .unwrap();
    mem.store_episode("unique event", "test", None).unwrap();

    let consolidated = mem.consolidate_episodes(10).unwrap();
    assert!(consolidated.is_some());

    let rows = mem
        .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let content = super::as_str(&rows[0][0]).unwrap();
    // 4 original → 2 unique
    assert!(
        content.contains("4→2"),
        "should show dedup ratio, got: {content}"
    );
}

// ---------------------------------------------------------------------------
// Issue #2044 (G4): crash-atomic `consolidate_episodes`
//
// Simulates a mid-consolidation crash by running the consolidation inside
// a transaction, aborting partway, and verifying that on recovery no
// duplicate summaries exist and the source episodes remain unconsolidated.
// ---------------------------------------------------------------------------

/// Regression test: a crash between the summary-insert and the last
/// SET compressed=1 must not leave a partial state where a summary node
/// exists but some source episodes are still uncompressed. After recovery
/// (here simulated by aborting a raw transaction mid-loop), a subsequent
/// `consolidate_episodes` call must succeed cleanly with no duplicate
/// summaries.
#[test]
fn consolidate_episodes_crash_injection_no_duplicate_summaries() {
    let mem = test_mem();

    // Seed 5 episodes.
    for i in 0..5 {
        mem.store_episode(&format!("crash-test-event {i}"), "test", None)
            .unwrap();
    }

    // --- Simulate a crash mid-transaction ---
    // Open a connection, BEGIN TRANSACTION, insert the summary node,
    // mark only 2 of 5 episodes as compressed, then ROLLBACK (simulating
    // a crash before COMMIT).
    {
        let conn = lbug::Connection::new(&mem.db).expect("conn for crash injection");
        conn.query("BEGIN TRANSACTION").expect("begin");
        conn.query(
            "CREATE (e:Episode {id: 'epi_crash_summary', content: '[crash summary]', source_label: 'consolidation', temporal_index: 0, compressed: 1})"
        ).expect("summary insert");

        // Mark only 2 source rows — partial apply.
        let partial_rows = mem
            .query("MATCH (e:Episode) WHERE e.compressed = 0 RETURN e.id ORDER BY e.id LIMIT 2")
            .unwrap();
        for row in &partial_rows {
            if let Some(eid) = super::as_str(&row[0]) {
                conn.query(&format!(
                    "MATCH (e:Episode {{id: '{eid}'}}) SET e.compressed = 1"
                ))
                .expect("partial compress");
            }
        }
        // Crash! — ROLLBACK instead of COMMIT.
        conn.query("ROLLBACK").expect("rollback (simulated crash)");
    }

    // Verify: the crash-summary node must NOT be visible after rollback.
    let ghost_rows = mem
        .query("MATCH (e:Episode) WHERE e.id = 'epi_crash_summary' RETURN e.id")
        .unwrap();
    assert!(
        ghost_rows.is_empty(),
        "rolled-back summary must not be visible, found {} row(s)",
        ghost_rows.len()
    );

    // Verify: all 5 source episodes remain uncompressed (compressed=0).
    let uncompressed = mem
        .query("MATCH (e:Episode) WHERE e.compressed = 0 RETURN count(e)")
        .unwrap();
    let uncompressed_count = super::as_i64(&uncompressed[0][0]).unwrap_or(0);
    assert_eq!(
        uncompressed_count, 5,
        "all source episodes must remain uncompressed after crash, got {uncompressed_count}"
    );

    // --- Now run real consolidation — should succeed with no duplicates ---
    let result = mem.consolidate_episodes(10).unwrap();
    assert!(
        result.is_some(),
        "consolidation should produce a summary after crash recovery"
    );

    // Exactly 1 consolidation summary must exist.
    let summary_rows = mem
        .query("MATCH (e:Episode) WHERE e.source_label = 'consolidation' RETURN e.id, e.content")
        .unwrap();
    assert_eq!(
        summary_rows.len(),
        1,
        "expected exactly 1 consolidation summary (no duplicates), found {}",
        summary_rows.len()
    );

    // All original episodes must now be compressed.
    let still_uncompressed = mem
        .query("MATCH (e:Episode) WHERE e.compressed = 0 RETURN count(e)")
        .unwrap();
    let still_count = super::as_i64(&still_uncompressed[0][0]).unwrap_or(-1);
    assert_eq!(
        still_count, 0,
        "after successful consolidation all episodes must be compressed, got {still_count}"
    );
}

// ---------------------------------------------------------------------------
// Backup-restore recovery tests (issue #1710)
//
// These tests pin the contract for `open_db_with_recovery`:
// when the main DB file is corrupt, recovery MUST attempt restore from
// available backups (newest-first, skipping any that fail verification)
// BEFORE falling back to a fresh empty DB. Falling back silently destroys
// user data — the previous implementation did exactly this because Step 4
// of the recovery sequence always succeeded with an empty DB and made
// Step 5 (restore-from-backup) dead code.
// ---------------------------------------------------------------------------

/// Overwrite a file with garbage bytes so LadybugDB will refuse to open it
/// or fail the post-open health check. Used by the recovery tests below.
fn corrupt_db_file(path: &std::path::Path) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .expect("open file for corruption");
    // 4 KiB of non-zero garbage — enough to defeat any header check while
    // also not being a recognizable empty/zeroed file.
    let garbage = vec![0xABu8; 4096];
    f.write_all(&garbage).expect("write garbage");
    f.sync_all().expect("fsync garbage");
}

/// Move the most recent verified backup file to a deterministic epoch suffix
/// so tests don't race against the wall clock. Returns the new path.
fn rename_backup_to_epoch(state_root: &std::path::Path, epoch: u64) -> std::path::PathBuf {
    let backup_dir = state_root.join("backups");
    let prefix = "cognitive_memory.ladybug.";
    let mut found: Option<std::path::PathBuf> = None;
    let mut newest_ts: u64 = 0;
    for entry in std::fs::read_dir(&backup_dir)
        .expect("read backup_dir")
        .flatten()
    {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if let Some(ts_str) = name_str.strip_prefix(prefix)
            && let Ok(ts) = ts_str.parse::<u64>()
            && ts >= newest_ts
        {
            newest_ts = ts;
            found = Some(entry.path());
        }
    }
    let src = found.expect("at least one backup file present");
    let dst = backup_dir.join(format!("{prefix}{epoch}"));
    if src != dst {
        std::fs::rename(&src, &dst).expect("rename backup to deterministic epoch");
    }
    dst
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn recovery_uses_backup_when_main_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let db_path = state_root.join("cognitive_memory.ladybug");

    // Step 1: open, store a marker fact, close.
    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("marker_one", "from-original-db", 0.9, &[], "test")
            .unwrap();
    }

    // Step 2: snapshot via the production backup path — this verifies the
    // backup before returning, so we know it's restorable.
    NativeCognitiveMemory::create_verified_backup(&state_root)
        .expect("create_verified_backup should succeed for a healthy DB");
    rename_backup_to_epoch(&state_root, 100);

    // Step 3: corrupt the main DB so open_db_with_recovery is forced into
    // its recovery path.
    corrupt_db_file(&db_path);

    // Step 4: re-open. The CONTRACT under test: recovery must restore from
    // the verified backup (NOT silently create a fresh empty DB).
    let mem2 = NativeCognitiveMemory::open(&state_root)
        .expect("open should succeed via backup-restore recovery");

    // Step 5: the marker fact must still be searchable. On the buggy
    // implementation this returns 0 results because Step 4 of the recovery
    // sequence created a fresh empty DB and never tried the backup.
    let facts = mem2.search_facts("marker_one", 10, 0.0).unwrap();
    assert_eq!(
        facts.len(),
        1,
        "recovery must restore from backup, not create empty DB (issue #1710)"
    );
    assert_eq!(facts[0].concept, "marker_one");
    assert_eq!(facts[0].content, "from-original-db");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn recovery_falls_back_to_empty_when_no_backups() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let db_path = state_root.join("cognitive_memory.ladybug");

    // Open + store + close so a real DB file exists to corrupt. No backup
    // is ever taken.
    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("would_be_lost", "no-backup", 0.5, &[], "test")
            .unwrap();
    }
    corrupt_db_file(&db_path);

    // No backups exist → recovery must produce a usable empty DB rather
    // than propagating an error.
    let mem = NativeCognitiveMemory::open(&state_root)
        .expect("open should fall through to empty-DB creation");
    let facts = mem.search_facts("would_be_lost", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 0, "fresh empty DB must contain no facts");
    let stats = mem.get_statistics().unwrap();
    assert_eq!(stats.total(), 0, "fresh empty DB must be empty");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn recovery_falls_back_to_empty_when_all_backups_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let db_path = state_root.join("cognitive_memory.ladybug");
    let backup_dir = state_root.join("backups");

    // Create a real DB and two backups, then corrupt every backup AND the
    // main file. Recovery must report data loss but still hand back a
    // usable empty DB.
    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("first", "v1", 0.9, &[], "test").unwrap();
    }
    NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
    let backup_a = rename_backup_to_epoch(&state_root, 100);

    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("second", "v2", 0.9, &[], "test").unwrap();
    }
    NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
    let backup_b = rename_backup_to_epoch(&state_root, 200);

    // Sanity: both backup files are present before we corrupt them.
    assert!(backup_a.exists(), "backup A should exist before corruption");
    assert!(backup_b.exists(), "backup B should exist before corruption");
    // Drop any paired .wal sibling backups too so they can't accidentally
    // make a half-corrupt restore succeed.
    for wal_name in ["cognitive_memory.ladybug.wal", "cognitive_memory.wal"] {
        for epoch in [100u64, 200] {
            let p = backup_dir.join(format!("{wal_name}.{epoch}"));
            if p.exists() {
                std::fs::remove_file(&p).unwrap();
            }
        }
    }

    corrupt_db_file(&backup_a);
    corrupt_db_file(&backup_b);
    corrupt_db_file(&db_path);

    let mem = NativeCognitiveMemory::open(&state_root)
        .expect("open should succeed with empty DB when all backups are corrupt");

    let stats = mem.get_statistics().unwrap();
    assert_eq!(
        stats.total(),
        0,
        "all backups corrupt → recovery must fall back to empty DB (data loss)"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn recovery_skips_corrupt_backups_uses_next() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let db_path = state_root.join("cognitive_memory.ladybug");
    let backup_dir = state_root.join("backups");

    // Build three DB snapshots whose contents differ so we can identify
    // which one was restored:
    //   epoch 100 (oldest valid)  → contains {fact_a}
    //   epoch 200 (middle valid)  → contains {fact_a, fact_b}
    //   epoch 300 (newest, will be corrupted) → contains {fact_a, fact_b, fact_c}

    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("fact_a", "alpha", 0.9, &[], "test").unwrap();
    }
    NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
    rename_backup_to_epoch(&state_root, 100);

    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("fact_b", "bravo", 0.9, &[], "test").unwrap();
    }
    NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
    rename_backup_to_epoch(&state_root, 200);

    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("fact_c", "charlie", 0.9, &[], "test")
            .unwrap();
    }
    NativeCognitiveMemory::create_verified_backup(&state_root).unwrap();
    let backup_newest = rename_backup_to_epoch(&state_root, 300);

    // Drop any .wal sibling backups so corruption of the main backup file
    // is the only signal recovery sees.
    for wal_name in ["cognitive_memory.ladybug.wal", "cognitive_memory.wal"] {
        for epoch in [100u64, 200, 300] {
            let p = backup_dir.join(format!("{wal_name}.{epoch}"));
            if p.exists() {
                std::fs::remove_file(&p).unwrap();
            }
        }
    }

    // Corrupt the NEWEST backup and the main DB. Recovery must skip the
    // corrupt newest backup and fall through to the next-newest (epoch 200).
    corrupt_db_file(&backup_newest);
    corrupt_db_file(&db_path);

    let mem = NativeCognitiveMemory::open(&state_root)
        .expect("open should succeed by skipping the corrupt newest backup");

    let a = mem.search_facts("fact_a", 10, 0.0).unwrap();
    let b = mem.search_facts("fact_b", 10, 0.0).unwrap();
    let c = mem.search_facts("fact_c", 10, 0.0).unwrap();
    assert_eq!(a.len(), 1, "fact_a (in oldest+middle) must be restored");
    assert_eq!(
        b.len(),
        1,
        "fact_b (in middle backup) must be restored — proves middle was used, not oldest"
    );
    assert_eq!(
        c.len(),
        0,
        "fact_c (only in corrupt newest backup) must NOT appear"
    );
}

// ============================================================================
// Per-write fsync barrier tests (issue #1973)
// ============================================================================

/// In-memory backend must skip the barrier (no on-disk file to fsync).
/// This keeps the unit-test suite fast and confirms the opt-out path.
#[test]
fn in_memory_writes_succeed_without_barrier() {
    let mem = NativeCognitiveMemory::in_memory().unwrap();
    // If post_write_barrier were not no-op'd, this would attempt to open
    // and fsync a path that may not even be a regular file — the test
    // would either fail or be slow. Each of these mutating ops exercises
    // a different post_write_barrier callsite.
    mem.store_fact("in_mem", "value", 0.9, &[], "test").unwrap();
    mem.record_sensory("modality", "data", 60).unwrap();
    mem.push_working("goal", "x", "t1", 1.0).unwrap();
    mem.store_episode("event", "label", None).unwrap();
    mem.store_prospective("desc", "trig", "act", 1).unwrap();
    mem.store_procedure("p", &["s1".to_string()], &[]).unwrap();
    let stats = mem.get_statistics().unwrap();
    assert!(stats.total() >= 6, "all writes must land");
}

/// Open-and-write path on a real on-disk DB must succeed end-to-end with
/// the barrier engaged. This is the positive-control for the barrier
/// pipeline (`fsync data → fsync parent dir`) running on every write —
/// if either step regressed to a hard error on a healthy DB, this test
/// catches it before the SIGKILL integration test runs. (The pipeline
/// deliberately omits `CHECKPOINT;` — see the `// No CHECKPOINT here`
/// note on `NativeCognitiveMemory::post_write_barrier`.)
#[test]
#[serial_test::serial(cognitive_memory)]
fn on_disk_writes_succeed_with_barrier_engaged() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let mem = NativeCognitiveMemory::open(&state_root).unwrap();
    for i in 0..5 {
        mem.store_fact(
            &format!("barrier_fact_{i}"),
            "barrier-payload",
            0.9,
            &[],
            "test",
        )
        .unwrap();
    }
    // Re-open from a fresh handle (still same process; the SIGKILL
    // integration test covers the cross-process variant).
    drop(mem);
    let mem2 = NativeCognitiveMemory::open(&state_root).unwrap();
    let facts = mem2.search_facts("barrier_fact", 100, 0.0).unwrap();
    assert_eq!(
        facts.len(),
        5,
        "all 5 barrier-protected writes must persist"
    );
}

/// `create_verified_backup` must produce a file whose SHA-256 matches the
/// source DB byte-for-byte. This is the directly-observable consequence
/// of the verified-fsync readback added to `atomic_copy_with_fsync` —
/// before issue #1973 the function dropped fsync errors and could return
/// Ok with a drifted backup.
#[test]
#[serial_test::serial(cognitive_memory)]
fn verified_backup_is_bit_exact_replica_of_source() {
    use sha2::{Digest, Sha256};

    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    let db_path = state_root.join("cognitive_memory.ladybug");

    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("backup_verify", "payload", 0.9, &[], "test")
            .unwrap();
    }

    let backup_path = NativeCognitiveMemory::create_verified_backup(&state_root)
        .expect("create_verified_backup must succeed for a healthy DB");

    let hash_of = |p: &std::path::Path| -> String {
        let bytes = std::fs::read(p).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        let digest = h.finalize();
        digest.iter().map(|b| format!("{b:02x}")).collect()
    };

    assert_eq!(
        hash_of(&db_path),
        hash_of(&backup_path),
        "verified-fsync readback (issue #1973) must guarantee that the \
         backup file on disk is a bit-exact replica of the source DB"
    );
}

/// When the source file disappears mid-flight, the readback verify path
/// surfaces a `PersistentStoreIo` error rather than silently returning
/// Ok. Exercises the new error variant introduced by decision D3.
///
/// This test calls `create_verified_backup` indirectly to drive the
/// `atomic_copy_with_fsync` codepath, then asserts the typed error
/// shape when the readback can no longer hash the source.
#[test]
#[serial_test::serial(cognitive_memory)]
fn verified_backup_errors_with_persistent_store_io_on_readback_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = tmp.path().to_path_buf();
    {
        let mem = NativeCognitiveMemory::open(&state_root).unwrap();
        mem.store_fact("would_be_backed_up", "v", 0.9, &[], "test")
            .unwrap();
    }
    // Delete the source DB between handle drop and backup call — the
    // backup precondition check returns an error early, before readback
    // would be reached. This still exercises the typed-error contract:
    // we must never get back an `Ok(...)` for a missing source.
    let db_path = state_root.join("cognitive_memory.ladybug");
    std::fs::remove_file(&db_path).unwrap();

    let err = NativeCognitiveMemory::create_verified_backup(&state_root)
        .expect_err("missing source file must propagate as an error, not Ok");
    let msg = err.to_string();
    assert!(
        msg.contains("nothing to back up") || msg.contains("does not exist"),
        "error should describe the missing source: {msg}"
    );
}
