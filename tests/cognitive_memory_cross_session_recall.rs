//! Hermetic outside-in cross-session recall gate.
//!
//! **Purpose.** This is a deliberately RED two-tier forcing function for
//! the cognitive-memory persistence workstream. It encodes — as compilable
//! Rust — the cross-session recall invariant from issue #1916 and demands
//! the two follow-up workstreams whose absence keeps the invariant
//! unenforceable today:
//!
//! * **Tier 1 (compile-time red, gates #1919).** This file references
//!   `NativeCognitiveMemory::open_with_recovery` — a symbol that does not
//!   exist yet. Until #1919 wires `load_latest_snapshot` into a real
//!   recovery ladder behind a public constructor, `cargo build --tests`
//!   fails for this binary with `no function or associated item named
//!   open_with_recovery`. That compile error IS the acceptance signal.
//!
//! * **Tier 2 (runtime red, gates #1917).** Once #1919 lands and the file
//!   compiles, the test asserts the on-disk snapshot JSON contains a
//!   top-level `schema_version` field. Until #1917 introduces the
//!   `PersistedEnvelope { schema_version }` wrapper, the assertion fails.
//!
//! Both reds resolve to the same artifact (this file), so the same CI
//! run that turns the test green for #1919 will surface the #1917 gap.
//!
//! **References.**
//! * Parent task: #1916 (hermetic cross-session recall test gating every PR).
//! * Schema envelope: #1917 (PersistedEnvelope { schema_version }).
//! * Recovery ladder wiring: #1919 (load_latest_snapshot → public ctor).
//! * Migration policy: #1941 (forward-compat decision).
//! * Anti-pattern reference: commit `ce418fd4` (fixes for #1923 / #1925)
//!   eliminated test-fixture leakage into live cognitive memory by
//!   forbidding ambient `$HOME` / `XDG_*` / pre-existing fixture reliance.
//!   This test defends that boundary: it pins `SIMARD_STATE_ROOT` to a
//!   `tempfile::TempDir` via an inline `EnvGuard` and unsets
//!   `SIMARD_MEMORY_SOCKET` so nothing leaks into or out of the suite.
//!
//! **DO NOT** add `#[ignore]` to silence this test. Doing so disarms the
//! forcing function and defeats the entire purpose of the PR that
//! introduced it. The PR is intentionally Draft / DO NOT MERGE until
//! both #1917 and #1919 have landed and the test is genuinely green.

#![cfg(unix)]

use std::ffi::OsString;
use std::path::Path;

use serial_test::serial;
use tempfile::TempDir;

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use simard::gym_bridge::ScoreDimensions;
use simard::gym_scoring::GymSuiteScore;
use simard::memory_snapshot::save_session_snapshot;
use simard::self_improve::{
    ImprovementCycle, ImprovementDecision, ImprovementHistory, ImprovementPhase, ProposedChange,
};

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use simard::memory_cognitive::{CognitiveFact, CognitiveStatistics, CognitiveWorkingSlot};

// ────────────────────────────────────────────────────────────────────────────
// Hermetic env guard (inline by design — task forbids widening test_support)
// ────────────────────────────────────────────────────────────────────────────

/// Env-var names this test pins. Hardcoded rather than imported so a
/// rename of the constants in `src/` cannot silently weaken the guard.
const STATE_ROOT_ENV: &str = "SIMARD_STATE_ROOT";
const MEMORY_SOCKET_ENV: &str = "SIMARD_MEMORY_SOCKET";

/// RAII guard that pins `SIMARD_STATE_ROOT` to the hermetic temp root and
/// clears `SIMARD_MEMORY_SOCKET` for the duration of the test, restoring
/// prior values on drop (including during panic unwind).
///
/// Construction captures the prior values BEFORE mutation; `Drop` restores
/// them unconditionally. Pattern mirrors `src/state_root.rs::tests::EnvGuard`
/// and `src/test_support/hermetic.rs::HermeticState` deliberately — we
/// inline rather than reuse so the test diff stays surgical (no `pub`
/// widening of `test_support` is permitted by this PR's scope).
struct EnvGuard {
    prev_state_root: Option<OsString>,
    prev_memory_socket: Option<OsString>,
}

impl EnvGuard {
    fn pin(state_root: &Path) -> Self {
        let prev_state_root = std::env::var_os(STATE_ROOT_ENV);
        let prev_memory_socket = std::env::var_os(MEMORY_SOCKET_ENV);
        // SAFETY: serialised by `#[serial(cognitive_memory)]` — no other
        // env-mutating test in this lock group runs concurrently.
        unsafe {
            std::env::set_var(STATE_ROOT_ENV, state_root);
            std::env::remove_var(MEMORY_SOCKET_ENV);
        }
        Self {
            prev_state_root,
            prev_memory_socket,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: same serial guarantee as `pin`; restoration is
        // idempotent and panic-safe so a failing assertion above does
        // not leak env state to the next test.
        unsafe {
            match self.prev_state_root.take() {
                Some(v) => std::env::set_var(STATE_ROOT_ENV, v),
                None => std::env::remove_var(STATE_ROOT_ENV),
            }
            match self.prev_memory_socket.take() {
                Some(v) => std::env::set_var(MEMORY_SOCKET_ENV, v),
                None => std::env::remove_var(MEMORY_SOCKET_ENV),
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Deterministic fixtures
// ────────────────────────────────────────────────────────────────────────────

/// Deterministic canary fact written by Session A and recalled by Session B.
/// No time/PID/env derivation — pure constants so failures are reproducible.
fn known_fact() -> (&'static str, &'static str, f64, &'static str) {
    (
        "hermetic_recall_canary",
        "issue-1916: cross-session recall must round-trip via the recovery ladder",
        0.97,
        "test-suite::cognitive_memory_cross_session_recall",
    )
}

/// Deterministic improvement cycle appended by Session A and recalled by Session B.
fn known_cycle() -> ImprovementCycle {
    let dims = ScoreDimensions {
        factual_accuracy: 0.91,
        specificity: 0.82,
        temporal_awareness: 0.73,
        source_attribution: 0.64,
        confidence_calibration: 0.85,
    };
    let baseline = GymSuiteScore {
        suite_id: "hermetic_recall_canary_suite".to_string(),
        overall: 0.80,
        dimensions: dims,
        scenario_count: 4,
        scenarios_passed: 4,
        pass_rate: 1.0,
        recorded_at_unix_ms: None,
    };
    ImprovementCycle {
        baseline,
        proposed_changes: vec![ProposedChange {
            file_path: "prompts/hermetic_recall.md".into(),
            description: "issue-1916 canary cycle".into(),
            expected_impact: "must survive Session A → Session B round-trip".into(),
        }],
        post_score: None,
        regressions: Vec::new(),
        decision: Some(ImprovementDecision::Commit {
            net_improvement: 0.05,
        }),
        final_phase: ImprovementPhase::Decide,
        weak_dimensions: Vec::new(),
        weak_dimension_details: Vec::new(),
        target_dimension: None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The gate
// ────────────────────────────────────────────────────────────────────────────

#[test]
#[serial(cognitive_memory)]
fn cognitive_memory_cross_session_recall() {
    let temp = TempDir::new().expect("tempdir for hermetic state root");
    let _guard = EnvGuard::pin(temp.path());

    let (fact_concept, fact_content, fact_confidence, fact_source) = known_fact();
    let cycle = known_cycle();

    // Snapshot directory derived deterministically from the hermetic root.
    // We do NOT call `memory_snapshot::snapshot_dir(None)` because that
    // resolves against `$HOME`, which the (#1923 / #1925) fix at commit
    // ce418fd4 explicitly forbade. Override path keeps the test hermetic.
    let snapshot_dir = temp.path().join("snapshots");
    std::fs::create_dir_all(&snapshot_dir).expect("create hermetic snapshot dir");

    // ─── Session A ──────────────────────────────────────────────────────
    // Scoped so the `NativeCognitiveMemory` handle (and its LadybugDB
    // flock) drops BEFORE Session B opens. Mirrors the lifecycle pattern
    // used by `tests/daemon_sigterm_durability.rs`.
    {
        let mem = NativeCognitiveMemory::open(temp.path())
            .expect("Session A: open native cognitive memory under hermetic root");

        mem.store_fact(
            fact_concept,
            fact_content,
            fact_confidence,
            &["issue-1916".to_string(), "hermetic".to_string()],
            fact_source,
        )
        .expect("Session A: store canary fact");

        let history = ImprovementHistory::open(temp.path())
            .expect("Session A: open improvement history under hermetic root");
        history
            .append(&cycle)
            .expect("Session A: append canary improvement cycle");

        mem.checkpoint()
            .expect("Session A: WAL checkpoint before snapshot");

        save_session_snapshot(&mem, "hermetic-recall-canary-agent", &snapshot_dir)
            .expect("Session A: save session snapshot");

        // `mem` and `history` drop here, releasing the LadybugDB flock
        // and any in-process state. Subsequent assertions and Session B
        // must observe the system as if a fresh process had started.
    }

    // ─── Tier-2 red (gates #1917): snapshot must carry schema_version ───
    // The on-disk snapshot today is a bare `MemorySnapshot` JSON. Issue
    // #1917 introduces `PersistedEnvelope { schema_version, payload }`
    // so consumers can migrate forward. This assertion is the acceptance
    // criterion for that envelope.
    let snapshot_files: Vec<_> = std::fs::read_dir(&snapshot_dir)
        .expect("list snapshot dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert!(
        !snapshot_files.is_empty(),
        "Session A must have written at least one .json snapshot under {}",
        snapshot_dir.display()
    );
    let snapshot_path = snapshot_files
        .iter()
        .max()
        .expect("at least one snapshot")
        .clone();
    let snapshot_text = std::fs::read_to_string(&snapshot_path).expect("read latest snapshot file");
    let snapshot_json: serde_json::Value =
        serde_json::from_str(&snapshot_text).expect("snapshot must be valid JSON");
    assert!(
        snapshot_json.get("schema_version").is_some(),
        "RED (gates #1917): snapshot at {} must include a top-level \
         `schema_version` field (PersistedEnvelope). Current keys: {:?}",
        snapshot_path.display(),
        snapshot_json
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    );

    // ─── Session B ──────────────────────────────────────────────────────
    // Tier-1 red (gates #1919): `NativeCognitiveMemory::open_with_recovery`
    // does not yet exist. This call is a compile-time error today; that
    // error IS the forcing function. The contract is: opening with
    // recovery against a state root that contains a prior snapshot MUST
    // make every Session A write recallable via the public API surface,
    // with zero ambient global state and zero env leakage.
    {
        let mem = NativeCognitiveMemory::open_with_recovery(temp.path()).expect(
            "RED (gates #1919): Session B must open the cognitive memory via the \
             recovery ladder. `open_with_recovery` is the missing public constructor \
             that wires `memory_snapshot::load_latest_snapshot` + `restore_snapshot` \
             behind a single entry point so cross-session recall is the default, \
             not an opt-in dance.",
        );

        // Recall the canary fact through the public search API.
        let facts = mem
            .search_facts(fact_concept, 16, 0.0)
            .expect("Session B: search_facts after recovery");
        let recalled = facts
            .iter()
            .find(|f| f.concept == fact_concept)
            .unwrap_or_else(|| {
                panic!(
                    "RED (gates #1919): Session B failed to recall canary fact \
                     `{fact_concept}` after `open_with_recovery`. Got {} facts: {:?}",
                    facts.len(),
                    facts.iter().map(|f| &f.concept).collect::<Vec<_>>(),
                )
            });
        assert_eq!(
            recalled.content, fact_content,
            "Session B: recalled content must match Session A's write"
        );
        assert!(
            (recalled.confidence - fact_confidence).abs() < 1e-6,
            "Session B: recalled confidence ({}) must match Session A's ({fact_confidence})",
            recalled.confidence
        );

        // Recall the improvement cycle through the durable JSONL history.
        // `ImprovementHistory` is already hermetic (no env reads), so this
        // half passes today on its own — coupling it to the same test
        // keeps the cross-session recall contract single-sourced.
        let history = ImprovementHistory::open(temp.path())
            .expect("Session B: open improvement history under hermetic root");
        let cycles = history.load().expect("Session B: load improvement cycles");
        assert!(
            cycles
                .iter()
                .any(|c| c.baseline.suite_id == cycle.baseline.suite_id && c.is_committed()),
            "Session B: improvement history must contain Session A's canary cycle \
             (suite_id={}). Got {} cycles.",
            cycle.baseline.suite_id,
            cycles.len(),
        );
    }
}

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
