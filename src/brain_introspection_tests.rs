//! TDD (RED) tests for the periodic **brain self-examination + memory hygiene**
//! pass (issue #2419).
//!
//! Written **before** the production code. Expected to FAIL until
//! `crate::brain_introspection` (the module), `prompt_assets/simard/recipes/
//! brain-introspection.yaml`, and `prompt_assets/simard/brain_introspection.md`
//! land. The unresolved-path / unresolved-import / missing-`include_str!`
//! errors are the intended, deterministic RED signal under `cargo test`.
//!
//! `cargo build` / `cargo build --release` stay GREEN — this module is
//! `#[cfg(test)]`, so the not-yet-existing references are never compiled into
//! the library or the daemon binary (mirrors
//! `memory_consolidation/promotion_scheduler_tests.rs`).
//!
//! ## Contract pinned by these tests
//!
//! The first increment is **SAFE: read + safe-consolidate + recommend**. The
//! daemon-side Rust hook owns the verified, RPC-backed memory ops + the
//! deterministic prune cap + metrics; the agentic recipe (a `recipe-runner-rs`
//! subprocess) owns LLM judgment + the GitHub-issue output. Crucially:
//!
//!   * `prune_expired_sensory()` (already-expired transient rows) is
//!     **non-discretionary TTL cleanup — NOT clamped by the cap**.
//!   * `enforce_prune_cap` bounds ONLY the recipe's *value-bearing prune
//!     recommendation* count (`-c max_prune`).
//!   * `consolidate_episodes()` is additive distillation; the authoritative
//!     `consolidated_facts` is the post−pre `(semantic+procedural)` stats delta
//!     (the call returns `Option<String>`, not a count).
//!   * NO destructive superseded/semantic deletes happen daemon-side
//!     (`prune_superseded` over the IPC memory is a `Ok(0)` no-op — calling it
//!     would be a silent-degradation hazard, so the hook must not call it).
//!
//! ```ignore
//! // crate::brain_introspection — surface under test
//! pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;
//! pub const DEFAULT_MAX_PRUNE: usize = 25;
//! pub const DEFAULT_BASELINE_RUNS: u32 = 7;
//!
//! pub fn interval_secs_from_env(raw: Option<&str>) -> u64;
//! pub fn max_prune_from_env(raw: Option<&str>) -> usize;
//! pub fn baseline_runs_from_env(raw: Option<&str>) -> u32;
//! pub fn should_run_introspection(elapsed: Duration, interval_secs: u64) -> bool;
//! pub fn enforce_prune_cap(requested: usize, cap: usize) -> usize;
//!
//! pub struct MemoryHygieneOutcome {
//!     pub live_memories: u64, pub sensory_pruned: usize, pub consolidated_facts: u64,
//! }
//! pub fn run_memory_hygiene(mem: &dyn CognitiveMemoryOps, batch_size: u32)
//!     -> SimardResult<MemoryHygieneOutcome>;
//!
//! pub struct BrainIntrospectionReport {
//!     pub live_memories: u64, pub sensory_pruned: usize, pub consolidated_facts: u64,
//!     pub prune_requested: usize, pub brain_health: Vec<String>,
//!     pub patterns: Vec<String>, pub regressions: Vec<String>,
//!     pub issue_url: Option<String>,
//! }
//! impl BrainIntrospectionReport { pub fn summary(&self) -> String; }
//!
//! // #4968: the brittle `parse_brain_introspection_text` marker scraper is
//! //  RETIRED — the rail now reads a typed record fail-closed instead:
//! pub fn read_verified_brain_introspection(path: &Path, invoke_start: SystemTime)
//!     -> SimardResult<BrainIntrospectionRecord>;   // R1–R7, in brain_introspection_record.rs
//! pub(crate) fn resolve_recipe_path(repo_root: &Path, home_override: Option<&Path>)
//!     -> Option<PathBuf>;
//! pub fn run_brain_introspection(mem: &dyn CognitiveMemoryOps, repo_root: &Path,
//!     state_root: &Path, home_override: Option<&Path>)
//!     -> SimardResult<BrainIntrospectionReport>;
//! ```

#![allow(clippy::bool_assert_comparison)]

use crate::brain_introspection::{
    BrainIntrospectionReport, DEFAULT_BASELINE_RUNS, DEFAULT_INTERVAL_SECS, DEFAULT_MAX_PRUNE,
    baseline_runs_from_env, enforce_prune_cap, interval_secs_from_env, max_prune_from_env,
    resolve_recipe_path, run_brain_introspection, run_memory_hygiene, should_run_introspection,
};
// #4968: the typed-record read path that replaces `parse_brain_introspection_text`.
// These symbols do not exist yet, so their unresolved import is the intended,
// deterministic RED signal under `cargo test` until the Builder lands
// `src/brain_introspection_record.rs` + the `record-brain-introspection` verb.
use crate::brain_introspection_record::{
    BRAIN_INTROSPECTION_SCHEMA, BrainIntrospectionRecord, MAX_AGE_SECS,
    read_verified_brain_introspection,
};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::ooda_brain::prompt_store::embedded_fallback;
use crate::operator_cli::dispatch_operator_cli;

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECIPE_FILENAME: &str = "brain-introspection.yaml";
const PROMPT_NAME: &str = "brain_introspection.md";

// ───────────────────────────────────────────────────────────────────────────
// Stub CognitiveMemoryOps — instruments the safe memory-hygiene ops.
//
// `get_statistics` returns a live, mutating snapshot so that a read BEFORE and
// AFTER `consolidate_episodes` yields the post−pre fact delta. `prune_superseded`
// is overridden purely to assert the hook NEVER calls it (no destructive
// value-prune daemon-side).
// ───────────────────────────────────────────────────────────────────────────
struct HygieneStub {
    stats: Mutex<CognitiveStatistics>,
    /// Facts the (additive) distillation pass adds per `consolidate_episodes`.
    facts_added_per_consolidate: u64,
    /// Expired transient rows `prune_expired_sensory` removes (uncapped).
    sensory_to_prune: usize,
    sensory_prune_calls: AtomicU32,
    consolidate_calls: AtomicU32,
    stats_reads: AtomicU32,
    /// Set true iff the destructive superseded-prune was (wrongly) invoked.
    superseded_prune_called: AtomicBool,
}

impl HygieneStub {
    fn new(stats: CognitiveStatistics, facts_added: u64, sensory_to_prune: usize) -> Self {
        Self {
            stats: Mutex::new(stats),
            facts_added_per_consolidate: facts_added,
            sensory_to_prune,
            sensory_prune_calls: AtomicU32::new(0),
            consolidate_calls: AtomicU32::new(0),
            stats_reads: AtomicU32::new(0),
            superseded_prune_called: AtomicBool::new(false),
        }
    }
}

impl CognitiveMemoryOps for HygieneStub {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen_x".to_string())
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        self.sensory_prune_calls.fetch_add(1, Ordering::SeqCst);
        let mut s = self.stats.lock().unwrap();
        let pruned = self.sensory_to_prune.min(s.sensory_count as usize);
        s.sensory_count -= pruned as u64;
        Ok(pruned)
    }

    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("wrk_x".to_string())
    }

    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }

    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }

    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("epi_x".to_string())
    }

    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        self.consolidate_calls.fetch_add(1, Ordering::SeqCst);
        let mut s = self.stats.lock().unwrap();
        // Additive distillation: episodic -> semantic. Episodic shrinks by the
        // number distilled, semantic grows. Never a lossy delete.
        let moved = self.facts_added_per_consolidate.min(s.episodic_count);
        s.episodic_count -= moved;
        s.semantic_count += self.facts_added_per_consolidate;
        Ok(Some("distilled-batch".to_string()))
    }

    fn store_fact(
        &self,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        Ok("sem_x".to_string())
    }

    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
    }

    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc_x".to_string())
    }

    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }

    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro_x".to_string())
    }

    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        self.stats_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.stats.lock().unwrap().clone())
    }

    // SAFETY assertion seam: the first increment must NOT call this over the
    // daemon's IPC memory (it is a `Ok(0)` no-op there — a silent-degradation
    // hazard). Any call flips the flag and the `no_destructive_value_prune`
    // test fails.
    fn prune_superseded(&self) -> SimardResult<usize> {
        self.superseded_prune_called.store(true, Ordering::SeqCst);
        Ok(0)
    }
}

fn stats(
    sensory: u64,
    working: u64,
    episodic: u64,
    semantic: u64,
    procedural: u64,
    prospective: u64,
) -> CognitiveStatistics {
    CognitiveStatistics {
        sensory_count: sensory,
        working_count: working,
        episodic_count: episodic,
        semantic_count: semantic,
        procedural_count: procedural,
        prospective_count: prospective,
    }
}

// ===========================================================================
// 1. Config knobs — env parsing + defaults
// ===========================================================================

#[test]
fn defaults_match_design_contract() {
    assert_eq!(DEFAULT_INTERVAL_SECS, 86_400, "cadence defaults to 24h");
    assert_eq!(DEFAULT_MAX_PRUNE, 25, "prune cap defaults to 25");
    assert_eq!(
        DEFAULT_BASELINE_RUNS, 7,
        "baseline window defaults to 7 runs"
    );
}

#[test]
fn interval_secs_unset_uses_default() {
    assert_eq!(interval_secs_from_env(None), DEFAULT_INTERVAL_SECS);
}

#[test]
fn interval_secs_parses_valid_value() {
    assert_eq!(interval_secs_from_env(Some("3600")), 3600);
}

#[test]
fn interval_secs_zero_is_honored_as_disabled_value() {
    // 0 is a *valid* parse (means "disabled"); it must NOT fall back to default.
    assert_eq!(interval_secs_from_env(Some("0")), 0);
}

#[test]
fn interval_secs_garbage_falls_back_to_default() {
    assert_eq!(
        interval_secs_from_env(Some("not-a-number")),
        DEFAULT_INTERVAL_SECS
    );
    assert_eq!(interval_secs_from_env(Some("")), DEFAULT_INTERVAL_SECS);
}

#[test]
fn max_prune_env_parsing() {
    assert_eq!(max_prune_from_env(None), DEFAULT_MAX_PRUNE);
    assert_eq!(max_prune_from_env(Some("10")), 10);
    assert_eq!(max_prune_from_env(Some("0")), 0);
    assert_eq!(max_prune_from_env(Some("bogus")), DEFAULT_MAX_PRUNE);
}

#[test]
fn baseline_runs_env_parsing() {
    assert_eq!(baseline_runs_from_env(None), DEFAULT_BASELINE_RUNS);
    assert_eq!(baseline_runs_from_env(Some("3")), 3);
    assert_eq!(baseline_runs_from_env(Some("bogus")), DEFAULT_BASELINE_RUNS);
}

// ===========================================================================
// 2. Daemon interval gating — 0 disables; elapsed >= interval triggers
// ===========================================================================

#[test]
fn gating_disabled_when_interval_zero() {
    // Disabled regardless of how much time has elapsed.
    assert_eq!(
        should_run_introspection(Duration::from_secs(10_000_000), 0),
        false,
        "interval 0 must disable the pass entirely"
    );
}

#[test]
fn gating_triggers_when_elapsed_reaches_interval() {
    assert_eq!(
        should_run_introspection(Duration::from_secs(86_400), 86_400),
        true
    );
    assert_eq!(
        should_run_introspection(Duration::from_secs(90_000), 86_400),
        true
    );
}

#[test]
fn gating_holds_when_not_yet_elapsed() {
    assert_eq!(
        should_run_introspection(Duration::from_secs(100), 86_400),
        false
    );
}

// ===========================================================================
// 3. enforce_prune_cap — bounds the recipe's value-bearing recommendation count
// ===========================================================================

#[test]
fn cap_passes_through_when_below_cap() {
    assert_eq!(enforce_prune_cap(3, 25), 3);
}

#[test]
fn cap_allows_exactly_cap() {
    assert_eq!(enforce_prune_cap(25, 25), 25);
}

#[test]
fn cap_clamps_when_above_cap() {
    assert_eq!(enforce_prune_cap(100, 25), 25);
}

#[test]
fn cap_zero_yields_zero_recommendations() {
    // cap=0 => no value-bearing prune recommendations are honored.
    assert_eq!(enforce_prune_cap(50, 0), 0);
}

#[test]
fn cap_never_exceeds_cap_or_request_property() {
    for requested in 0usize..200 {
        for cap in 0usize..50 {
            let got = enforce_prune_cap(requested, cap);
            assert!(got <= cap, "result {got} must not exceed cap {cap}");
            assert!(
                got <= requested,
                "result {got} must not exceed request {requested}"
            );
        }
    }
}

// ===========================================================================
// 4. #4968 — typed BrainIntrospectionRecord read path (replaces the deleted
//    parse_brain_introspection_text marker grammar).
//
//    The recipe's final ACT step writes ONE typed, owner-only (0o600),
//    freshness-checked record; the rail reads it FAIL-CLOSED via
//    read_verified_brain_introspection (R1–R7). Every failure mode is a distinct
//    AdapterInvocationFailed carrying its R-code — NEVER a silent default. Also
//    covers the gated `simard cognition record-brain-introspection` writer verb
//    and writer/reader parity (one shared type, no drift).
// ===========================================================================

/// Current unix-epoch seconds (records must be fresh for R7 to pass).
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

/// Write `bytes` verbatim to `path` as an owner-only `0o600` file (so R6 passes
/// for every non-R6 case), creating parents. Used to author records the typed
/// writer would never produce — malformed JSON, an unknown key, over-bounds
/// lists — so the reader's fail-closed matrix is exercised directly.
fn write_raw_600(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(path, bytes).expect("write raw record");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod 0o600");
    }
}

/// A fully-valid, fresh `BrainIntrospectionRecord` as raw JSON (a base to tweak
/// per fail-closed test).
fn valid_brain_json(epoch: u64) -> serde_json::Value {
    serde_json::json!({
        "schema": "brain-introspection/v1",
        "written_at_epoch": epoch,
        "brain_health": ["fallback rate 4.2% (baseline 1.1%)"],
        "patterns": ["coverage-comment step flakes on cold CI"],
        "regressions": [],
        "prune_candidates": [],
        "prune_requested": 0,
        "issue_url": serde_json::Value::Null
    })
}

/// Assert a reader result is the fail-closed `Err` for R-code `code` (e.g.
/// `"R4"`) — an `AdapterInvocationFailed` whose reason names that check. The
/// reader must NEVER return `Ok` (a defaulted/partial record) on a bad input.
fn assert_read_r(result: SimardResult<BrainIntrospectionRecord>, code: &str) {
    match result {
        Ok(rec) => panic!("expected fail-closed {code}, got Ok({rec:?})"),
        Err(SimardError::AdapterInvocationFailed { reason, .. }) => assert!(
            reason.contains(code),
            "expected fail-closed reason to carry {code}, got: {reason}"
        ),
        Err(other) => panic!("expected AdapterInvocationFailed carrying {code}, got: {other:?}"),
    }
}

/// Drive the operator CLI (the gated writer verb entry point).
fn cli(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

// --- pins: schema + freshness constant ---

#[test]
fn brain_schema_pin_is_v1() {
    assert_eq!(BRAIN_INTROSPECTION_SCHEMA, "brain-introspection/v1");
}

#[test]
fn brain_max_age_secs_is_five_minutes() {
    assert_eq!(MAX_AGE_SECS, 300);
}

// --- happy path: a valid, fresh, 0o600 record is accepted ---

#[test]
fn read_accepts_a_valid_fresh_owner_only_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let epoch = now_epoch();
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_brain_json(epoch)).unwrap(),
    );
    // `invoke_start` is the rail's pre-spawn clock; a record written after it,
    // moments ago, is fresh.
    let invoke_start = SystemTime::now() - Duration::from_secs(5);

    let rec = read_verified_brain_introspection(&path, invoke_start)
        .expect("a valid, fresh, 0o600 record must be accepted");
    assert_eq!(rec.schema, "brain-introspection/v1");
    assert_eq!(rec.brain_health.len(), 1);
    assert!(rec.brain_health[0].contains("fallback rate"));
    assert_eq!(rec.prune_requested, 0);
    assert!(rec.issue_url.is_none());
}

// --- R1: absent / unreadable ---

#[test]
fn read_r1_absent_record_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R1");
}

// --- R2: present but not valid JSON ---

#[test]
fn read_r2_malformed_json_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    write_raw_600(&path, b"this is not valid json {{{");
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R2");
}

// --- R3: schema version pin ---

#[test]
fn read_r3_wrong_schema_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let mut json = valid_brain_json(now_epoch());
    json["schema"] = serde_json::json!("brain-introspection/v2");
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R3");
}

// --- R4: closed-type parse & bounds (deny_unknown_fields / over-count / over-byte) ---

#[test]
fn read_r4_unknown_top_level_key_is_fail_closed() {
    // Well-formed JSON (so it clears R2), but an extra top-level key that a
    // `#[serde(deny_unknown_fields)]` struct must reject at R4.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let mut json = valid_brain_json(now_epoch());
    json["bogus_extra_field"] = serde_json::json!(true);
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R4");
}

#[test]
fn read_r4_over_count_brain_health_list_is_fail_closed() {
    // brain_health caps at 32 elements; 33 is rejected (never truncated).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let mut json = valid_brain_json(now_epoch());
    let over: Vec<String> = (0..33).map(|i| format!("finding {i}")).collect();
    json["brain_health"] = serde_json::json!(over);
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R4");
}

#[test]
fn read_r4_over_byte_brain_health_element_is_fail_closed() {
    // A single element over the 256-byte per-element cap is rejected.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let mut json = valid_brain_json(now_epoch());
    json["brain_health"] = serde_json::json!(["x".repeat(300)]);
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R4");
}

// --- R5: required-field validity (brain_health must be non-empty) ---

#[test]
fn read_r5_empty_brain_health_is_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let mut json = valid_brain_json(now_epoch());
    json["brain_health"] = serde_json::json!([]);
    write_raw_600(&path, &serde_json::to_vec(&json).unwrap());
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R5");
}

// --- R6: owner-only permissions ---

#[cfg(unix)]
#[test]
fn read_r6_non_owner_only_permissions_is_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    // A valid, fresh record — but group/other-readable (0o644), which the reader
    // must reject as R6 (a record the trusted 0o600 writer would never produce).
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_brain_json(now_epoch())).unwrap(),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R6");
}

// --- R7: freshness / anti-replay ---

#[test]
fn read_r7_mtime_predates_invoke_start_is_fail_closed() {
    // A record whose mtime predates `invoke_start` is a prior-run artifact the
    // rail's pre-truncate would have removed; simulate by capturing an
    // invoke_start in the FUTURE relative to the just-written file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_brain_json(now_epoch())).unwrap(),
    );
    let invoke_start = SystemTime::now() + Duration::from_secs(1_000);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R7");
}

#[test]
fn read_r7_stale_written_at_epoch_is_fail_closed() {
    // mtime is fresh, but the embedded written_at_epoch skews far from now,
    // which the R7 defense-in-depth check must reject even though the file is new.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let stale = now_epoch().saturating_sub(10_000);
    write_raw_600(
        &path,
        &serde_json::to_vec(&valid_brain_json(stale)).unwrap(),
    );
    let invoke_start = SystemTime::now() - Duration::from_secs(5);
    assert_read_r(read_verified_brain_introspection(&path, invoke_start), "R7");
}

// --- gated writer verb `simard cognition record-brain-introspection` + parity ---

#[test]
fn cli_record_brain_introspection_writes_a_record_the_reader_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let epoch = now_epoch();
    let invoke_start = SystemTime::now() - Duration::from_secs(5);

    cli(&[
        "cognition",
        "record-brain-introspection",
        "--record-path",
        path.to_str().unwrap(),
        "--written-at-epoch",
        &epoch.to_string(),
        "--brain-health",
        "fallback rate 4.2% (baseline 1.1%)",
        "--brain-health",
        "0-succeeded-action cycles: 3 of 40",
        "--pattern",
        "coverage-comment step flakes on cold CI",
        "--regression",
        "brain_lifecycle_decision parse-failure rate up 3.1x",
        "--prune-candidate",
        "duplicate semantic fact #A/#B (superseded)",
        "--prune-requested",
        "4",
        "--issue-url",
        "https://github.com/rysweet/Simard/issues/5012",
    ])
    .expect("a valid brain-introspection record write must exit Ok");

    // Writer/reader parity — the reader accepts exactly what the writer produced.
    let rec = read_verified_brain_introspection(&path, invoke_start)
        .expect("the reader must accept the record the writer just produced (no drift)");
    assert_eq!(rec.schema, "brain-introspection/v1");
    assert_eq!(rec.brain_health.len(), 2);
    assert_eq!(rec.prune_requested, 4);
    assert_eq!(
        rec.issue_url.as_deref(),
        Some("https://github.com/rysweet/Simard/issues/5012")
    );

    // And the file is owner-only 0o600 (persist_json's atomic 0o600 write).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "record must be written owner-only 0o600"
        );
    }
}

#[test]
fn cli_record_brain_introspection_requires_at_least_one_brain_health() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brain_introspection/record.json");
    let res = cli(&[
        "cognition",
        "record-brain-introspection",
        "--record-path",
        path.to_str().unwrap(),
        "--written-at-epoch",
        &now_epoch().to_string(),
        // no --brain-health at all
    ]);
    assert!(
        res.is_err(),
        "at least one --brain-health finding is required (validate-all-then-write-once)"
    );
    assert!(
        !path.exists(),
        "a validation failure must leave NO file on disk"
    );
}

#[test]
fn cli_record_brain_introspection_rejects_non_absolute_record_path() {
    let res = cli(&[
        "cognition",
        "record-brain-introspection",
        "--record-path",
        "relative/record.json",
        "--written-at-epoch",
        &now_epoch().to_string(),
        "--brain-health",
        "ok",
    ]);
    assert!(res.is_err(), "--record-path must be absolute");
}

#[test]
fn cli_record_brain_introspection_rejects_parent_dir_traversal() {
    let res = cli(&[
        "cognition",
        "record-brain-introspection",
        "--record-path",
        "/tmp/../etc/record.json",
        "--written-at-epoch",
        &now_epoch().to_string(),
        "--brain-health",
        "ok",
    ]);
    assert!(res.is_err(), "--record-path must not contain '..'");
}

// ===========================================================================
// 5. BrainIntrospectionReport::summary — one-line daemon log
// ===========================================================================

#[test]
fn summary_mentions_key_counts() {
    let report = BrainIntrospectionReport {
        live_memories: 142,
        sensory_pruned: 9,
        consolidated_facts: 4,
        prune_requested: 12,
        brain_health: vec!["ok".to_string()],
        patterns: vec!["p".to_string()],
        regressions: vec![],
        issue_url: Some("https://github.com/rysweet/Simard/issues/1".to_string()),
    };
    let s = report.summary();
    assert!(
        s.contains("142"),
        "summary should report live memory count: {s}"
    );
    assert!(
        s.to_lowercase().contains("sensory"),
        "summary should mention sensory prune: {s}"
    );
    assert!(
        s.to_lowercase().contains("consolidat"),
        "summary should mention consolidation: {s}"
    );
}

// ===========================================================================
// 6. resolve_recipe_path — hot-reload vs in-tree (home_override)
// ===========================================================================

#[test]
fn resolve_recipe_path_none_when_absent() {
    let home = tempfile::tempdir().unwrap();
    assert!(resolve_recipe_path(Path::new("/nonexistent/repo"), Some(home.path())).is_none());
}

#[test]
fn resolve_recipe_path_finds_in_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
    std::fs::create_dir_all(&recipe_dir).unwrap();
    std::fs::write(
        recipe_dir.join(RECIPE_FILENAME),
        "name: brain-introspection",
    )
    .unwrap();

    // home_override points at an EMPTY home so the hot-reload path misses and
    // resolution falls through to the in-tree copy.
    let empty_home = tempfile::tempdir().unwrap();
    let resolved = resolve_recipe_path(tmp.path(), Some(empty_home.path()))
        .expect("in-tree recipe must resolve");
    assert!(resolved.ends_with(RECIPE_FILENAME));
}

#[test]
fn resolve_recipe_path_prefers_hot_reload_over_in_tree() {
    // Both present -> hot-reload (~/.simard/...) wins.
    let home = tempfile::tempdir().unwrap();
    let hot_dir = home.path().join(".simard/prompt_assets/simard/recipes");
    std::fs::create_dir_all(&hot_dir).unwrap();
    std::fs::write(hot_dir.join(RECIPE_FILENAME), "name: hot").unwrap();

    let repo = tempfile::tempdir().unwrap();
    let in_tree_dir = repo.path().join("prompt_assets/simard/recipes");
    std::fs::create_dir_all(&in_tree_dir).unwrap();
    std::fs::write(in_tree_dir.join(RECIPE_FILENAME), "name: in-tree").unwrap();

    let resolved = resolve_recipe_path(repo.path(), Some(home.path())).unwrap();
    assert!(
        resolved.starts_with(home.path()),
        "hot-reload path under HOME must take precedence: {resolved:?}"
    );
}

// ===========================================================================
// 7. run_memory_hygiene — the SAFE, RPC-backed core (deterministic, no subprocess)
// ===========================================================================

#[test]
fn hygiene_reads_stats_prunes_sensory_and_measures_consolidation_delta() {
    // sensory=1000, working=2, episodic=10, semantic=5, procedural=3, prospective=1
    // distillation adds 4 semantic facts; ALL 1000 expired sensory rows pruned.
    let stub = HygieneStub::new(stats(1000, 2, 10, 5, 3, 1), 4, 1000);

    let outcome = run_memory_hygiene(&stub, 50).expect("hygiene must succeed on a healthy stub");

    // Stats were read at least twice (pre + post) to compute the delta.
    assert!(
        stub.stats_reads.load(Ordering::SeqCst) >= 2,
        "must read statistics before AND after consolidation"
    );
    // Distillation ran exactly once (the higher-level pass invokes it once).
    assert_eq!(stub.consolidate_calls.load(Ordering::SeqCst), 1);

    // consolidated_facts = post(semantic+procedural) - pre(semantic+procedural)
    //                    = (9+3) - (5+3) = 4
    assert_eq!(
        outcome.consolidated_facts, 4,
        "consolidated_facts must be the measured (semantic+procedural) delta, not a marker echo"
    );

    // live_memories excludes sensory: 2 + 10 + 9 + 3 + 1 = 25
    assert_eq!(
        outcome.live_memories, 25,
        "live_memories must be the non-sensory modality sum (post-state)"
    );
}

#[test]
fn hygiene_sensory_prune_is_unbounded_by_cap() {
    // 1000 expired sensory rows is FAR above DEFAULT_MAX_PRUNE (25). Past-TTL
    // cleanup is non-discretionary and must NOT be clamped by the prune cap.
    let stub = HygieneStub::new(stats(1000, 0, 0, 0, 0, 0), 0, 1000);
    let outcome = run_memory_hygiene(&stub, 50).unwrap();
    assert_eq!(
        outcome.sensory_pruned, 1000,
        "expired-sensory cleanup must run uncapped (no enforce_prune_cap applied)"
    );
    assert!(
        outcome.sensory_pruned > DEFAULT_MAX_PRUNE,
        "test must demonstrate sensory prune exceeding the cap"
    );
}

#[test]
fn hygiene_performs_no_destructive_value_prune() {
    // The first increment must never call the destructive superseded-prune
    // daemon-side (it is a no-op over the IPC memory — a silent-degradation
    // hazard). Value-bearing pruning is recommendation-only via the recipe.
    let stub = HygieneStub::new(stats(10, 1, 5, 5, 2, 0), 1, 5);
    let _ = run_memory_hygiene(&stub, 50).unwrap();
    assert_eq!(
        stub.superseded_prune_called.load(Ordering::SeqCst),
        false,
        "run_memory_hygiene must NOT call prune_superseded (no destructive daemon-side delete)"
    );
}

#[test]
fn hygiene_consolidation_is_additive_not_lossy() {
    // Distillation moves episodic -> semantic; the live (non-sensory) total must
    // not shrink as a result of consolidation alone.
    let before = stats(0, 0, 20, 0, 0, 0);
    let before_live = before.working_count
        + before.episodic_count
        + before.semantic_count
        + before.procedural_count
        + before.prospective_count;
    let stub = HygieneStub::new(before, 6, 0);
    let outcome = run_memory_hygiene(&stub, 50).unwrap();
    assert!(
        outcome.live_memories >= before_live,
        "additive distillation must not reduce live memory count: {} < {}",
        outcome.live_memories,
        before_live
    );
}

// ===========================================================================
// 8. run_brain_introspection — full hook, agentic layer best-effort (graceful)
// ===========================================================================

#[test]
fn run_is_graceful_when_recipe_not_found() {
    // No recipe on disk -> the deterministic hygiene pass STILL runs and the
    // hook returns Ok (the agentic layer is best-effort; its absence must not
    // block safe memory hygiene). issue_url is None (recipe never ran).
    let stub = HygieneStub::new(stats(500, 1, 10, 4, 2, 0), 3, 500);
    let empty_home = tempfile::tempdir().unwrap();

    let report = run_brain_introspection(
        &stub,
        Path::new("/nonexistent/repo"),
        Path::new("/nonexistent/state"),
        Some(empty_home.path()),
    )
    .expect("missing recipe must degrade gracefully, not error");

    assert_eq!(stub.sensory_prune_calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.sensory_pruned, 500);
    assert_eq!(report.consolidated_facts, 3);
    assert_eq!(report.live_memories, 1 + 10 + 7 + 2); // working+episodic+(4+3)+prospective
    assert!(
        report.issue_url.is_none(),
        "no issue when the recipe never ran"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn run_is_graceful_when_recipe_runner_unavailable() {
    // Recipe resolves (temp in-tree copy) but `recipe-runner-rs` is missing OR
    // rejects the deliberately-invalid recipe. Either way the agentic step fails
    // and the hook must WARN + still return Ok with the hygiene outcomes filled.
    let repo = tempfile::tempdir().unwrap();
    let recipe_dir = repo.path().join("prompt_assets/simard/recipes");
    std::fs::create_dir_all(&recipe_dir).unwrap();
    std::fs::write(recipe_dir.join(RECIPE_FILENAME), "name: test").unwrap();

    let empty_home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let stub = HygieneStub::new(stats(300, 0, 8, 2, 1, 0), 2, 300);

    let report = run_brain_introspection(&stub, repo.path(), state.path(), Some(empty_home.path()))
        .expect("a failed agentic recipe must degrade gracefully");

    // The safe, deterministic hygiene ran regardless of the agentic failure.
    assert_eq!(report.sensory_pruned, 300);
    assert_eq!(report.consolidated_facts, 2);
    assert!(report.issue_url.is_none());
    // And no destructive value-prune happened on this path either.
    assert_eq!(stub.superseded_prune_called.load(Ordering::SeqCst), false);
}

// ===========================================================================
// 9. Prompt content-pin — embedded_fallback must register the standing prompt
// ===========================================================================

#[test]
fn prompt_has_embedded_fallback() {
    assert!(
        embedded_fallback(PROMPT_NAME).is_some(),
        "prompt_store::embedded_fallback must register {PROMPT_NAME}"
    );
}

#[test]
fn prompt_fallback_is_nonempty() {
    let content = embedded_fallback(PROMPT_NAME).expect("prompt must be registered");
    assert!(
        !content.trim().is_empty(),
        "embedded prompt must not be empty"
    );
}

#[test]
fn prompt_covers_the_five_introspection_phases() {
    let content = embedded_fallback(PROMPT_NAME)
        .expect("prompt must be registered")
        .to_lowercase();
    assert!(
        content.contains("brain health") || content.contains("brain-health"),
        "prompt must describe the BRAIN HEALTH phase"
    );
    assert!(
        content.contains("pattern"),
        "prompt must describe the PATTERNS phase"
    );
    // Safe, BOUNDED pruning (recommendation-only in the first increment).
    assert!(content.contains("prune"), "prompt must describe pruning");
    assert!(
        content.contains("bound") || content.contains("cap") || content.contains("safe"),
        "prompt must stress SAFE/BOUNDED pruning"
    );
    assert!(
        content.contains("consolidat") || content.contains("distill"),
        "prompt must describe the CONSOLIDATE phase"
    );
    assert!(
        content.contains("issue"),
        "prompt must route findings to a GitHub issue (no snapshot doc)"
    );
}

#[test]
fn prompt_forbids_snapshot_doc_output() {
    // Per the no-point-in-time-docs rule, output goes to an issue/metrics, not a
    // committed snapshot markdown file.
    let content = embedded_fallback(PROMPT_NAME)
        .expect("prompt must be registered")
        .to_lowercase();
    assert!(
        content.contains("issue") || content.contains("metric"),
        "prompt must direct output to an issue and/or metrics"
    );
}

// ===========================================================================
// 10. Recipe YAML content-pin — runtime recipe mirrors the prompt's contract
//
// `include_str!` of a not-yet-existing file is a COMPILE error: that is the
// intended RED until the recipe asset lands.
// ===========================================================================

const RECIPE_YAML: &str = include_str!("../prompt_assets/simard/recipes/brain-introspection.yaml");

#[test]
fn recipe_yaml_is_nonempty() {
    assert!(
        !RECIPE_YAML.trim().is_empty(),
        "recipe yaml must not be empty"
    );
}

#[test]
fn recipe_yaml_covers_all_phases_and_markers() {
    let lower = RECIPE_YAML.to_lowercase();
    // Phases.
    assert!(
        lower.contains("brain") && lower.contains("health"),
        "recipe must include the brain-health step"
    );
    assert!(
        lower.contains("pattern"),
        "recipe must include the pattern-mining step"
    );
    assert!(
        lower.contains("prune"),
        "recipe must include the prune-recommendation step"
    );
    assert!(
        lower.contains("consolidat") || lower.contains("distill"),
        "recipe must reference consolidation/distillation"
    );
    // Output contract: the recipe's final ACT step writes the typed record via
    // the gated `record-brain-introspection` verb (no more stdout markers).
    assert!(
        RECIPE_YAML.contains("record-brain-introspection"),
        "recipe must call `simard cognition record-brain-introspection` as its final ACT step"
    );
    assert!(
        RECIPE_YAML.contains("--brain-health"),
        "recipe must pass the required --brain-health finding(s) to the record verb"
    );
    assert!(
        RECIPE_YAML.contains("--prune-requested"),
        "recipe must pass the --prune-requested count to the record verb"
    );
    assert!(
        RECIPE_YAML.contains("--issue-url"),
        "recipe must pass the --issue-url flag to the record verb"
    );
}

#[test]
fn recipe_yaml_writes_to_github_issue_with_dedup_label() {
    let lower = RECIPE_YAML.to_lowercase();
    assert!(
        lower.contains("gh issue") || (lower.contains("gh ") && lower.contains("issue")),
        "output step must create/update a GitHub issue via gh"
    );
    assert!(
        lower.contains("brain-introspection"),
        "recipe must use a stable `brain-introspection` label for issue dedup"
    );
}

#[test]
fn recipe_yaml_passes_through_prune_cap_context() {
    // The Rust hook supplies `-c max_prune=<cap>`; the recipe must thread it so
    // recommendations stay bounded.
    assert!(
        RECIPE_YAML.contains("max_prune"),
        "recipe must consume the max_prune cap context var"
    );
}
