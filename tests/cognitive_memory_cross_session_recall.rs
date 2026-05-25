//! Cross-session recall integration test (issue #1974, epic #1972).
//!
//! Proves the core "durable recall across sessions" claim at the process
//! boundary rather than with in-memory mocks. Spawns the
//! `cross_session_recall_helper` example binary twice — first as a
//! **write** process that stores deterministic entries across all four
//! memory tiers (facts, episodes, working memory, sensory), then as a
//! **read** process that opens the same `state_root` from a fresh process
//! context and asserts field-identical recall.
//!
//! Complements (does not replace) the in-process mock test at
//! `tests/memory_consolidation_lifecycle.rs::cross_session_recall_hydrates_prior_facts`.
//!
//! Serialised via `serial_test` group `cognitive_memory_state_root` to
//! avoid colliding with other state-root tests.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serial_test::serial;
use simard::memory_cognitive::{CognitiveFact, CognitiveStatistics, CognitiveWorkingSlot};

// ============================================================================
// Helper utilities
// ============================================================================

/// Locate the `cross_session_recall_helper` example binary that Cargo
/// places at `target/<profile>/examples/<name>`.
fn helper_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    // exe = .../target/<profile>/deps/<test_binary>-<hash>
    let deps = exe.parent().expect("deps dir");
    let profile = deps.parent().expect("profile dir");
    let candidate = profile.join("examples").join("cross_session_recall_helper");
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "cross_session_recall_helper binary not found at {}; \
         build with: cargo build --example cross_session_recall_helper",
        candidate.display()
    );
}

/// Run the helper in the given phase, collect stdout lines, and return them.
/// Panics if the helper exits non-zero or times out.
fn run_phase(state_root: &Path, phase: &str) -> Vec<String> {
    let mut child = Command::new(helper_binary())
        .arg("--state-root")
        .arg(state_root)
        .arg("--phase")
        .arg(phase)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn helper (phase={phase}): {e}"));

    let stdout = child.stdout.take().expect("stdout pipe");
    let reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(60);

    let mut lines = Vec::new();
    for line_result in reader.lines() {
        let line = line_result.expect("read stdout line");
        if line.trim() == "DONE" {
            break;
        }
        lines.push(line);
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("helper (phase={phase}) timed out after 60s");
        }
    }

    let status = child.wait().expect("wait for helper");
    assert!(
        status.success(),
        "helper (phase={phase}) exited with status {status:?}"
    );

    lines
}

/// Parse a tagged line like `FACTS [...]` into the JSON portion.
fn parse_tagged_line<'a>(lines: &'a [String], tag: &str) -> &'a str {
    for line in lines {
        if let Some(rest) = line.strip_prefix(tag) {
            return rest.trim();
        }
    }
    panic!("expected a line starting with '{tag}' in helper output: {lines:?}");
}

// ============================================================================
// Tests
// ============================================================================

/// Core cross-session recall test: write all tiers in one process, read
/// them back in a fresh process, assert field-identical results.
#[test]
#[serial(cognitive_memory_state_root)]
fn cross_session_recall_all_tiers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");

    // Phase 1: write
    let write_lines = run_phase(&state_root, "write");
    assert!(
        write_lines.is_empty(),
        "write phase should only print DONE, got: {write_lines:?}"
    );

    // Phase 2: read (fresh process, same state_root)
    let read_lines = run_phase(&state_root, "read");

    // --- Facts ---
    let facts_json = parse_tagged_line(&read_lines, "FACTS ");
    let facts: Vec<CognitiveFact> = serde_json::from_str(facts_json).expect("deserialize facts");
    assert_eq!(
        facts.len(),
        3,
        "expected 3 facts, got {}: {facts:?}",
        facts.len()
    );
    // Verify each fact's fields (search_facts returns ORDER BY id DESC,
    // so iterate by concept for deterministic matching).
    for i in 0..3_usize {
        let concept = format!("recall-fact-{i}");
        let fact = facts
            .iter()
            .find(|f| f.concept == concept)
            .unwrap_or_else(|| panic!("fact with concept '{concept}' not found in {facts:?}"));
        assert_eq!(fact.content, format!("fact-content-{i}"));
        let expected_conf = 0.80 + (i as f64) * 0.05;
        assert!(
            (fact.confidence - expected_conf).abs() < 1e-6,
            "fact {concept}: expected confidence {expected_conf}, got {}",
            fact.confidence
        );
        assert_eq!(fact.source_id, "cross-session-test");
        assert!(
            fact.tags.contains(&format!("tag-{i}")),
            "fact {concept}: missing tag-{i} in {:?}",
            fact.tags
        );
        assert!(
            fact.tags.contains(&"cross-session".to_string()),
            "fact {concept}: missing 'cross-session' tag in {:?}",
            fact.tags
        );
    }

    // --- Working memory ---
    let working_json = parse_tagged_line(&read_lines, "WORKING ");
    let working: Vec<CognitiveWorkingSlot> =
        serde_json::from_str(working_json).expect("deserialize working");
    assert_eq!(
        working.len(),
        3,
        "expected 3 working memory slots, got {}: {working:?}",
        working.len()
    );
    for i in 0..3_usize {
        let content = format!("working-content-{i}");
        let slot = working
            .iter()
            .find(|w| w.content == content)
            .unwrap_or_else(|| {
                panic!("working slot with content '{content}' not found in {working:?}")
            });
        assert_eq!(slot.slot_type, format!("slot-type-{i}"));
        assert_eq!(slot.task_id, "cross-session-task");
        let expected_rel = 0.5 + (i as f64) * 0.1;
        assert!(
            (slot.relevance - expected_rel).abs() < 1e-6,
            "working slot {content}: expected relevance {expected_rel}, got {}",
            slot.relevance
        );
    }

    // --- Episodes + Sensory (verified via statistics counts) ---
    let stats_json = parse_tagged_line(&read_lines, "STATS ");
    let stats: CognitiveStatistics = serde_json::from_str(stats_json).expect("deserialize stats");
    assert_eq!(
        stats.episodic_count, 3,
        "expected 3 episodes, got {}",
        stats.episodic_count
    );
    assert_eq!(
        stats.sensory_count, 3,
        "expected 3 sensory records, got {}",
        stats.sensory_count
    );
    assert_eq!(
        stats.semantic_count, 3,
        "expected 3 facts (semantic), got {}",
        stats.semantic_count
    );
    assert_eq!(
        stats.working_count, 3,
        "expected 3 working memory slots, got {}",
        stats.working_count
    );
}

/// Idempotency: running write→read→write→read on the same state_root
/// accumulates data correctly (second read sees both batches).
#[test]
#[serial(cognitive_memory_state_root)]
fn cross_session_recall_accumulates_across_cycles() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");

    // Cycle 1
    run_phase(&state_root, "write");
    let read1 = run_phase(&state_root, "read");
    let stats1: CognitiveStatistics = serde_json::from_str(parse_tagged_line(&read1, "STATS "))
        .expect("deserialize stats cycle 1");
    assert_eq!(stats1.semantic_count, 3);

    // Cycle 2 — writes another batch of the same shape
    run_phase(&state_root, "write");
    let read2 = run_phase(&state_root, "read");
    let stats2: CognitiveStatistics = serde_json::from_str(parse_tagged_line(&read2, "STATS "))
        .expect("deserialize stats cycle 2");

    // Each cycle writes 3 of each tier; IDs are unique (UUID-based),
    // so the second read must see 6.
    assert_eq!(
        stats2.semantic_count, 6,
        "expected 6 facts after 2 write cycles, got {}",
        stats2.semantic_count
    );
    assert_eq!(
        stats2.episodic_count, 6,
        "expected 6 episodes after 2 write cycles, got {}",
        stats2.episodic_count
    );
    assert_eq!(
        stats2.working_count, 6,
        "expected 6 working slots after 2 write cycles, got {}",
        stats2.working_count
    );
    assert_eq!(
        stats2.sensory_count, 6,
        "expected 6 sensory records after 2 write cycles, got {}",
        stats2.sensory_count
    );
}
